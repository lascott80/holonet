//! Milestone 6 phase 0, spike (a): local, remote and dynamic (SOCKS5)
//! forwards with russh 0.65 against the WSL test server, driven directly
//! through russh so the library is untouched until phase 3.
//!
//!   cargo run -p holonet-ssh --example m6_forward --release -- <local scratch folder>
//!
//! On the host it makes `~/m6spike` (a 500 MB file and a small one), serves
//! it with `python3 -m http.server` on the host's 127.0.0.1:18080, and
//! removes both at the end.
//! - Local: a listener on this PC opens `direct-tcpip` to that server per
//!   connection. One 500 MB download is timed and saved for a hash check,
//!   then 50 small downloads run at once. Binding the same port again shows
//!   the "in use" error.
//! - Remote: `tcpip-forward` on the host's 127.0.0.1:19000 reaches a small
//!   HTTP server on this PC. The host's curl downloads 300 MB through it and
//!   runs 20 requests at once. A privileged port shows the refusal.
//! - Dynamic: a SOCKS5 server on this PC (CONNECT, no authentication).
//!   Windows' curl.exe goes through it with `--socks5-hostname`, so the host
//!   resolves `localhost`; a closed port shows the refusal reply.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use russh::client::{self, ChannelOpenHandle};
use russh::keys::{PublicKeyOrCertificate, check_known_hosts_path};
use russh::{Channel, ChannelMsg, ChannelOpenFailure};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};

#[path = "../tests/support/mod.rs"]
mod support;

const HOST_HTTP: u16 = 18080;
const REMOTE_PORT: u32 = 19000;
const BIG_MB: usize = 500;
const REMOTE_MB: usize = 300;

/// Remote forwards: the host's bound port → where it goes on this PC.
type Routes = Arc<Mutex<HashMap<u32, SocketAddr>>>;

struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    routes: Routes,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else { return Ok(false) };
        Ok(check_known_hosts_path(&self.host, self.port, key, &self.known_hosts).unwrap_or(false))
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let target = self.routes.lock().unwrap().get(&connected_port).copied();
        match target {
            Some(target) => {
                reply.accept().await;
                tokio::spawn(async move {
                    if let Ok(mut tcp) = TcpStream::connect(target).await {
                        let mut stream = channel.into_stream();
                        let _ = copy_bidirectional(&mut stream, &mut tcp).await;
                    }
                });
            }
            None => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}

type Handle = Arc<client::Handle<Handler>>;

async fn exec(handle: &Handle, command: &str) -> Result<(Option<u32>, String)> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let (mut out, mut status) = (Vec::new(), None);
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => out.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok((status, String::from_utf8_lossy(&out).into_owned()))
}

/// GET `path` over HTTP/1.0 from `addr`; returns the body. With `save`, the
/// body is written there instead and only its length is returned.
async fn http_get(addr: SocketAddr, path: &str, save: Option<&std::path::Path>) -> Result<(usize, Vec<u8>)> {
    let mut tcp = TcpStream::connect(addr).await?;
    tcp.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if tcp.read(&mut byte).await? == 0 {
            bail!("connection closed in the headers");
        }
        head.push(byte[0]);
    }
    ensure!(
        head.starts_with(b"HTTP/1.0 200") || head.starts_with(b"HTTP/1.1 200"),
        "status: {}",
        String::from_utf8_lossy(&head[..head.len().min(40)])
    );
    match save {
        Some(path) => {
            let mut file = tokio::fs::File::create(path).await?;
            let n = tokio::io::copy(&mut tcp, &mut file).await?;
            Ok((n as usize, Vec::new()))
        }
        None => {
            let mut body = Vec::new();
            tcp.read_to_end(&mut body).await?;
            Ok((body.len(), body))
        }
    }
}

fn rate(bytes: usize, start: Instant) -> String {
    let secs = start.elapsed().as_secs_f64();
    format!("{:.1} MB in {:.2} s, {:.0} MB/s", bytes as f64 / 1e6, secs, bytes as f64 / 1e6 / secs)
}

/// Local forward: each connection to `listener` becomes a `direct-tcpip`
/// channel to the host's `127.0.0.1:port`.
fn local_forward(handle: Handle, listener: TcpListener, port: u16) {
    tokio::spawn(async move {
        while let Ok((mut tcp, peer)) = listener.accept().await {
            let handle = handle.clone();
            tokio::spawn(async move {
                let opened = handle
                    .channel_open_direct_tcpip("127.0.0.1", port.into(), peer.ip().to_string(), peer.port().into())
                    .await;
                match opened {
                    Ok(channel) => {
                        let mut stream = channel.into_stream();
                        let _ = copy_bidirectional(&mut tcp, &mut stream).await;
                    }
                    Err(e) => eprintln!("  local: channel refused: {e}"),
                }
            });
        }
    });
}

/// A SOCKS5 server (RFC 1928): no authentication, CONNECT only. The
/// destination is opened from the host, which resolves domain names.
fn socks_server(handle: Handle, listener: TcpListener) {
    tokio::spawn(async move {
        while let Ok((tcp, peer)) = listener.accept().await {
            let handle = handle.clone();
            tokio::spawn(async move {
                if let Err(e) = socks_one(handle, tcp, peer).await {
                    eprintln!("  socks: {e}");
                }
            });
        }
    });
}

async fn socks_one(handle: Handle, mut tcp: TcpStream, peer: SocketAddr) -> Result<()> {
    let mut hello = [0u8; 2];
    tcp.read_exact(&mut hello).await?;
    ensure!(hello[0] == 5, "not SOCKS5");
    let mut methods = vec![0u8; hello[1] as usize];
    tcp.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        tcp.write_all(&[5, 0xff]).await?;
        bail!("client offers no no-authentication method");
    }
    tcp.write_all(&[5, 0]).await?;
    let mut req = [0u8; 4];
    tcp.read_exact(&mut req).await?;
    let reply = |code: u8| [5, code, 0, 1, 0, 0, 0, 0, 0, 0];
    if req[1] != 1 {
        tcp.write_all(&reply(7)).await?; // command not supported
        bail!("command {} not supported", req[1]);
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            tcp.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        4 => {
            let mut a = [0u8; 16];
            tcp.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        3 => {
            let len = tcp.read_u8().await? as usize;
            let mut name = vec![0u8; len];
            tcp.read_exact(&mut name).await?;
            String::from_utf8(name)?
        }
        other => {
            tcp.write_all(&reply(8)).await?;
            bail!("address type {other} not supported");
        }
    };
    let port = tcp.read_u16().await?;
    match handle.channel_open_direct_tcpip(host.clone(), port.into(), peer.ip().to_string(), peer.port().into()).await {
        Ok(channel) => {
            tcp.write_all(&reply(0)).await?;
            let mut stream = channel.into_stream();
            let _ = copy_bidirectional(&mut tcp, &mut stream).await;
            Ok(())
        }
        Err(e) => {
            let code = match &e {
                russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => 5, // refused
                russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => 2,
                _ => 1,
            };
            tcp.write_all(&reply(code)).await?;
            bail!("{host}:{port}: {e}")
        }
    }
}

/// A small HTTP server on this PC for the remote forward: `/big` sends
/// `REMOTE_MB` of a byte pattern, anything else "hello from this PC".
async fn pc_http_server() -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                let n = tcp.read(&mut buf).await.unwrap_or(0);
                let big = String::from_utf8_lossy(&buf[..n]).starts_with("GET /big");
                if big {
                    let len = REMOTE_MB * 1024 * 1024;
                    let _ = tcp.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {len}\r\n\r\n").as_bytes()).await;
                    let chunk: Vec<u8> = (0..65536u32).map(|i| (i % 251) as u8).collect();
                    for _ in 0..len / chunk.len() {
                        if tcp.write_all(&chunk).await.is_err() {
                            return;
                        }
                    }
                } else {
                    let body = "hello from this PC\n";
                    let _ = tcp
                        .write_all(
                            format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
                        )
                        .await;
                }
                let _ = tcp.shutdown().await;
            });
        }
    });
    Ok(addr)
}

#[tokio::main]
async fn main() -> Result<()> {
    let scratch = PathBuf::from(std::env::args().nth(1).context("give a local scratch folder")?);
    std::fs::create_dir_all(&scratch)?;
    let env = support::load_env()?;
    let host = env["HOLONET_TEST_HOST"].clone();
    let port: u16 = env["HOLONET_TEST_PORT"].parse()?;
    let routes: Routes = Arc::default();
    let handler = Handler {
        host: host.clone(),
        port,
        known_hosts: support::root().join(".dev/known_hosts"),
        routes: routes.clone(),
    };
    let config = Arc::new(client::Config { keepalive_interval: Some(holonet_ssh::KEEPALIVE), ..Default::default() });
    let mut handle = client::connect(config, (host.as_str(), port), handler).await?;
    let auth = handle.authenticate_password(&env["HOLONET_TEST_USER"], &env["HOLONET_TEST_PASSWORD"]).await?;
    ensure!(auth.success(), "sign-in refused");
    let handle: Handle = Arc::new(handle);
    println!("connected to {host}:{port}");

    // The host's side: files and an HTTP server on its loopback only.
    let (status, hash) = exec(
        &handle,
        &format!(
            "[ -f ~/m6spike.pid ] && kill $(cat ~/m6spike.pid) ; mkdir -p ~/m6spike && cd ~/m6spike && \
             head -c {BIG_MB}M /dev/urandom > big.bin && echo hi > small.txt && sha256sum big.bin | cut -c1-64 && \
             (nohup python3 -m http.server {HOST_HTTP} --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > ~/m6spike.pid) ; sleep 1"
        ),
    )
    .await?;
    let host_hash = hash.trim().to_string();
    ensure!(status == Some(0) && host_hash.len() == 64, "host setup failed: status {status:?}, output {hash:?}");
    println!("host: big.bin sha256 {host_hash}");

    // ---------------------------------------------------------------- local
    println!("\n[local] this PC → host 127.0.0.1:{HOST_HTTP}");
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local = listener.local_addr()?;
    local_forward(handle.clone(), listener, HOST_HTTP);
    println!("  listening on {local}");
    let saved = scratch.join("m6-local-big.bin");
    let start = Instant::now();
    let (n, _) = http_get(local, "/big.bin", Some(&saved)).await?;
    println!("  500 MB download: {}", rate(n, start));
    println!("  saved to {} (host hash above)", saved.display());
    let start = Instant::now();
    let tasks: Vec<_> = (0..50).map(|_| tokio::spawn(http_get(local, "/small.txt", None))).collect();
    let mut ok = 0;
    for t in tasks {
        if let Ok(Ok((_, body))) = t.await
            && body == b"hi\n"
        {
            ok += 1;
        }
    }
    println!("  50 parallel small downloads: {ok}/50 correct in {:.0} ms", start.elapsed().as_secs_f64() * 1e3);
    let start = Instant::now();
    for _ in 0..50 {
        http_get(local, "/small.txt", None).await?;
    }
    println!("  50 small downloads one after another: {:.0} ms", start.elapsed().as_secs_f64() * 1e3);
    let start = Instant::now();
    let opens: Vec<_> = (0..50)
        .map(|_| {
            let handle = handle.clone();
            tokio::spawn(async move {
                handle.channel_open_direct_tcpip("127.0.0.1", HOST_HTTP.into(), "127.0.0.1", 1).await
            })
        })
        .collect();
    let mut opened = 0;
    for t in opens {
        if let Ok(Ok(channel)) = t.await {
            opened += 1;
            let _ = channel.close().await;
        }
    }
    println!("  50 direct-tcpip opens at once, no traffic: {opened} in {:.0} ms", start.elapsed().as_secs_f64() * 1e3);
    let (_, out) = exec(
        &handle,
        &format!("s=$(date +%s%N); for i in $(seq 50); do curl -s http://127.0.0.1:{HOST_HTTP}/small.txt >/dev/null & done; wait; echo $(( ($(date +%s%N)-s)/1000000 ))"),
    )
    .await?;
    println!("  the same 50 parallel requests made on the host itself: {} ms", out.trim());
    match TcpListener::bind(local).await {
        Ok(_) => println!("  binding {local} again: unexpectedly worked"),
        Err(e) => println!("  binding {local} again: {:?}: {e}", e.kind()),
    }

    // --------------------------------------------------------------- remote
    println!("\n[remote] host 127.0.0.1:{REMOTE_PORT} → this PC");
    let pc = pc_http_server().await?;
    routes.lock().unwrap().insert(REMOTE_PORT, pc);
    let bound = handle.tcpip_forward("127.0.0.1", REMOTE_PORT).await;
    println!("  tcpip_forward 127.0.0.1:{REMOTE_PORT}: {bound:?}");
    let start = Instant::now();
    let (_, out) =
        exec(&handle, &format!("curl -s -o /dev/null -w '%{{size_download}}' http://127.0.0.1:{REMOTE_PORT}/big"))
            .await?;
    let n: usize = out.trim().parse().unwrap_or(0);
    println!("  host curl of {REMOTE_MB} MB: {} (expected {} bytes)", rate(n, start), REMOTE_MB * 1024 * 1024);
    let (_, out) =
        exec(&handle, &format!("for i in $(seq 20); do curl -s http://127.0.0.1:{REMOTE_PORT}/small & done; wait"))
            .await?;
    println!("  20 parallel requests from the host: {} answers", out.matches("hello from this PC").count());
    println!("  tcpip_forward on port 80 (privileged): {:?}", handle.tcpip_forward("127.0.0.1", 80).await);
    let any = handle.tcpip_forward("127.0.0.1", 0).await;
    println!("  tcpip_forward on port 0: {any:?} (the host picks)");
    println!("  tcpip_forward on {REMOTE_PORT} again: {:?}", handle.tcpip_forward("127.0.0.1", REMOTE_PORT).await);
    println!("  cancel {REMOTE_PORT}: {:?}", handle.cancel_tcpip_forward("127.0.0.1", REMOTE_PORT).await);
    let (_, out) =
        exec(&handle, &format!("curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:{REMOTE_PORT}/small"))
            .await?;
    println!("  after cancel, the host's curl: http code {}", out.trim());

    // -------------------------------------------------------------- dynamic
    println!("\n[dynamic] SOCKS5 on this PC → host");
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let socks = listener.local_addr()?;
    socks_server(handle.clone(), listener);
    println!("  listening on {socks}");
    let curl = |args: Vec<String>| async move {
        tokio::process::Command::new(r"C:\Windows\System32\curl.exe").args(&args).output().await
    };
    let out = curl(vec![
        "-s".into(),
        "--socks5-hostname".into(),
        socks.to_string(),
        format!("http://localhost:{HOST_HTTP}/small.txt"),
    ])
    .await?;
    println!("  curl.exe small.txt via localhost (resolved on the host): {:?}", String::from_utf8_lossy(&out.stdout));
    let start = Instant::now();
    let out = curl(vec![
        "-s".into(),
        "-o".into(),
        "NUL".into(),
        "-w".into(),
        "%{size_download}".into(),
        "--socks5-hostname".into(),
        socks.to_string(),
        format!("http://localhost:{HOST_HTTP}/big.bin"),
    ])
    .await?;
    let n: usize = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    println!("  curl.exe big.bin: {}", rate(n, start));
    let out = curl(vec![
        "-s".into(),
        "-S".into(),
        "--socks5-hostname".into(),
        socks.to_string(),
        format!("http://localhost:{}/", HOST_HTTP + 1),
    ])
    .await?;
    println!(
        "  a closed port on the host: curl exit {:?}, {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );

    // -------------------------------------------------------------- cleanup
    exec(&handle, "kill $(cat ~/m6spike.pid) ; rm -rf ~/m6spike ~/m6spike.pid").await?;
    handle.disconnect(russh::Disconnect::ByApplication, "", "en").await?;
    println!("\ncleaned up the host");
    Ok(())
}
