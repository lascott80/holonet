//! Port forwarding against the WSL test server (milestone 6, phase 3).
//!
//! Skipped unless `HOLONET_TEST_SERVER=1`:
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh --test forward
//!
//! Each test that needs a service on the host starts Python's web server on
//! the host's own 127.0.0.1, on a port of its own (tests run in parallel),
//! and removes it at the end.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

mod support;

use holonet_ssh::{ForwardError, ForwardSpec, ForwardStatus, Session};
use support::{load_env, options_from};

fn enabled() -> bool {
    let on = std::env::var("HOLONET_TEST_SERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set HOLONET_TEST_SERVER=1 to run against the WSL test server");
    }
    on
}

async fn connect() -> Session {
    Session::connect(options_from("ed25519", &load_env().unwrap()).unwrap()).await.unwrap()
}

/// Python's web server on the host's 127.0.0.1:`port`, serving `small.txt` ("hi").
struct HostWeb(u16);

impl HostWeb {
    async fn start(session: &Session, port: u16) -> Self {
        let out = session
            .exec(&format!(
                "mkdir -p ~/fwtest-{port} && cd ~/fwtest-{port} && echo hi > small.txt && \
                 (nohup python3 -m http.server {port} --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > ~/fwtest-{port}.pid); \
                 for i in $(seq 50); do (echo > /dev/tcp/127.0.0.1/{port}) 2>/dev/null && break; sleep 0.1; done"
            ))
            .await
            .unwrap();
        assert_eq!(out.exit_status, Some(0), "host web server: {}", out.stderr);
        HostWeb(port)
    }

    async fn stop(self, session: &Session) {
        let port = self.0;
        let _ =
            session.exec(&format!("kill $(cat ~/fwtest-{port}.pid); rm -rf ~/fwtest-{port} ~/fwtest-{port}.pid")).await;
    }
}

/// GET `path` over HTTP/1.0 on an open stream; the body.
async fn http_get_on(mut tcp: TcpStream, path: &str) -> std::io::Result<String> {
    tcp.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).await?;
    let mut all = Vec::new();
    tcp.read_to_end(&mut all).await?;
    let text = String::from_utf8_lossy(&all).into_owned();
    Ok(text.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default())
}

async fn http_get(addr: SocketAddr, path: &str) -> std::io::Result<String> {
    http_get_on(TcpStream::connect(addr).await?, path).await
}

fn port_of(status: &ForwardStatus) -> u16 {
    match status {
        ForwardStatus::Active { port, .. } => *port,
        other => panic!("not active: {other:?}"),
    }
}

/// Wait (up to 5 s) for the status to satisfy `ok`.
async fn wait_for(forward: &holonet_ssh::Forward, ok: impl Fn(&ForwardStatus) -> bool) -> ForwardStatus {
    let mut watch = forward.watch();
    let _ = tokio::time::timeout(Duration::from_secs(5), watch.wait_for(|s| ok(s))).await;
    forward.status()
}

#[tokio::test]
async fn local_forward_reaches_a_host_service() {
    if !enabled() {
        return;
    }
    let session = connect().await;
    let web = HostWeb::start(&session, 18181).await;
    let spec = |destination_port| ForwardSpec::Local {
        bind_address: "127.0.0.1".into(),
        port: 0,
        destination: "127.0.0.1".into(),
        destination_port,
    };
    let forward = session.forward(spec(18181)).await.unwrap();
    let port = port_of(&forward.status());
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    assert_eq!(http_get(addr, "/small.txt").await.unwrap(), "hi\n");
    let status = wait_for(&forward, |s| matches!(s, ForwardStatus::Active { open: 0, total: 1, .. })).await;
    assert!(matches!(status, ForwardStatus::Active { open: 0, total: 1, last_error: None, .. }), "{status:?}");

    // Nothing listens on the destination: the connection closes and the
    // forward keeps running, with the reason.
    let closed = session.forward(spec(18199)).await.unwrap();
    let closed_addr: SocketAddr = ([127, 0, 0, 1], port_of(&closed.status())).into();
    assert_eq!(http_get(closed_addr, "/").await.unwrap_or_default(), "");
    let status = wait_for(&closed, |s| matches!(s, ForwardStatus::Active { last_error: Some(_), .. })).await;
    match status {
        ForwardStatus::Active { last_error: Some(e), .. } => {
            assert_eq!(e, "127.0.0.1:18199: the host could not connect to it")
        }
        other => panic!("{other:?}"),
    }

    // Stopping frees the port.
    let watch = forward.watch();
    forward.stop().await;
    assert_eq!(*watch.borrow(), ForwardStatus::Stopped);
    TcpListener::bind(addr).await.expect("the port is free again");
    web.stop(&session).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_port_in_use_is_reported() {
    if !enabled() {
        return;
    }
    let session = connect().await;
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    for spec in [
        ForwardSpec::Local { bind_address: "127.0.0.1".into(), port, destination: "x".into(), destination_port: 1 },
        ForwardSpec::Dynamic { bind_address: "127.0.0.1".into(), port },
    ] {
        match session.forward(spec).await {
            Err(e) => {
                assert_eq!(e, ForwardError::PortInUse(port));
                assert_eq!(e.to_string(), format!("Port {port} is already in use on this PC."));
            }
            Ok(_) => panic!("bound a port in use"),
        }
    }
    let bad = session
        .forward(ForwardSpec::Dynamic { bind_address: "192.0.2.1".into(), port: 0 })
        .await
        .err()
        .expect("an address this PC does not have");
    assert!(matches!(bad, ForwardError::Bind { .. }), "{bad:?}");
    session.close().await.unwrap();
}

/// A one-request web server on this PC; answers "hello from this PC".
async fn pc_web() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = tcp.read(&mut buf).await;
                let body = "hello from this PC";
                let _ = tcp
                    .write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn remote_forward_reaches_this_pc() {
    if !enabled() {
        return;
    }
    let session = connect().await;
    let web = pc_web().await;
    let forward = session
        .forward(ForwardSpec::Remote {
            bind_address: "127.0.0.1".into(),
            port: 0,
            destination: "127.0.0.1".into(),
            destination_port: web.port(),
        })
        .await
        .unwrap();
    let port = port_of(&forward.status());
    assert_ne!(port, 0, "the host's chosen port is reported");
    let curl = format!("curl -s -m 5 http://127.0.0.1:{port}/");
    assert_eq!(session.exec(&curl).await.unwrap().stdout, "hello from this PC");
    let status = wait_for(&forward, |s| matches!(s, ForwardStatus::Active { total: 1, open: 0, .. })).await;
    assert!(matches!(status, ForwardStatus::Active { total: 1, .. }), "{status:?}");

    // A privileged port is refused by the host.
    let refused = session
        .forward(ForwardSpec::Remote {
            bind_address: "127.0.0.1".into(),
            port: 80,
            destination: "127.0.0.1".into(),
            destination_port: web.port(),
        })
        .await
        .err();
    assert_eq!(refused, Some(ForwardError::Refused(80)));
    assert_eq!(refused.unwrap().to_string(), "The host refused to forward port 80.");

    // Nothing listens on the destination on this PC.
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let dead = session
        .forward(ForwardSpec::Remote {
            bind_address: "127.0.0.1".into(),
            port: 0,
            destination: "127.0.0.1".into(),
            destination_port: unused,
        })
        .await
        .unwrap();
    let dead_port = port_of(&dead.status());
    let _ = session.exec(&format!("curl -s -m 5 http://127.0.0.1:{dead_port}/")).await;
    let status = wait_for(&dead, |s| matches!(s, ForwardStatus::Active { last_error: Some(_), .. })).await;
    match status {
        ForwardStatus::Active { last_error: Some(e), .. } => {
            assert_eq!(e, format!("127.0.0.1:{unused} on this PC: nothing is listening there"))
        }
        other => panic!("{other:?}"),
    }

    // Stopping cancels it on the host.
    forward.stop().await;
    let after = session.exec(&format!("curl -s -m 5 -o /dev/null -w '%{{http_code}}' http://127.0.0.1:{port}/")).await;
    assert_eq!(after.unwrap().stdout, "000");
    session.close().await.unwrap();
}

/// A SOCKS5 CONNECT to `host`:`port` by name; the reply code and the stream.
async fn socks_connect(proxy: SocketAddr, host: &str, port: u16) -> (u8, TcpStream) {
    let mut tcp = TcpStream::connect(proxy).await.unwrap();
    tcp.write_all(&[5, 1, 0]).await.unwrap();
    let mut choice = [0u8; 2];
    tcp.read_exact(&mut choice).await.unwrap();
    assert_eq!(choice, [5, 0]);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    tcp.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tcp.read_exact(&mut reply).await.unwrap();
    (reply[1], tcp)
}

#[tokio::test]
async fn dynamic_forward_is_a_socks_proxy() {
    if !enabled() {
        return;
    }
    let session = connect().await;
    let web = HostWeb::start(&session, 18182).await;
    let forward = session.forward(ForwardSpec::Dynamic { bind_address: String::new(), port: 0 }).await.unwrap();
    let proxy: SocketAddr = ([127, 0, 0, 1], port_of(&forward.status())).into();

    // "localhost" is resolved by the host, so this reaches its web server.
    let (code, stream) = socks_connect(proxy, "localhost", 18182).await;
    assert_eq!(code, 0);
    assert_eq!(http_get_on(stream, "/small.txt").await.unwrap(), "hi\n");

    // A closed port: "connection refused" (5), and the reason on the forward.
    let (code, _) = socks_connect(proxy, "localhost", 18198).await;
    assert_eq!(code, 5);
    let status = wait_for(&forward, |s| matches!(s, ForwardStatus::Active { last_error: Some(_), .. })).await;
    assert!(
        matches!(&status, ForwardStatus::Active { last_error: Some(e), total: 2, .. } if e == "localhost:18198: the host could not connect to it"),
        "{status:?}"
    );
    forward.stop().await;
    web.stop(&session).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn a_lost_connection_ends_the_forward() {
    if !enabled() {
        return;
    }
    let session = connect().await;
    let forward = session
        .forward(ForwardSpec::Local {
            bind_address: "127.0.0.1".into(),
            port: 0,
            destination: "127.0.0.1".into(),
            destination_port: 22,
        })
        .await
        .unwrap();
    let addr: SocketAddr = ([127, 0, 0, 1], port_of(&forward.status())).into();
    // The command's parent is this connection's sshd process: killing it
    // drops the connection without a goodbye.
    let _ = tokio::time::timeout(Duration::from_secs(5), session.exec("kill -9 $PPID")).await;
    let status = wait_for(&forward, |s| !matches!(s, ForwardStatus::Active { .. })).await;
    assert_eq!(status, ForwardStatus::Failed(ForwardError::ConnectionLost));
    assert_eq!(status_text(&status), "The connection to the host was lost.");
    // Nothing listens any more.
    assert!(TcpStream::connect(addr).await.is_err(), "the port is closed");
    // A new forward on a lost session is refused at once.
    let again = session.forward(ForwardSpec::Dynamic { bind_address: String::new(), port: 0 }).await.err();
    assert_eq!(again, Some(ForwardError::ConnectionLost));
}

fn status_text(status: &ForwardStatus) -> String {
    match status {
        ForwardStatus::Failed(e) => e.to_string(),
        other => format!("{other:?}"),
    }
}
