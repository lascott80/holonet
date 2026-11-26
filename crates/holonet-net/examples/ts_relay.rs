//! Milestone 4 phase 6 test helper: make the WSL test server reachable on
//! this PC's own Tailscale address, which WSL (NAT mode, localhost only)
//! does not answer on. Listens on `<this PC's Tailscale IPv4>:<port>` and
//! forwards each connection to `127.0.0.1:<port>`. Test use only; stop it
//! with Ctrl C. Windows Firewall may ask once to allow it.
//!
//!   cargo run -p holonet-net --example ts_relay --release -- [port]

use holonet_net::tailscale::{self, Tailscale};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(2222);
    let Tailscale::Running(status) = tailscale::status().await else {
        eprintln!("Tailscale is not running on this PC");
        std::process::exit(1);
    };
    let Some(own) = status.own_ips.iter().find(|ip| ip.is_ipv4()).copied() else {
        eprintln!("this PC has no Tailscale IPv4 address");
        std::process::exit(1);
    };
    let listener = TcpListener::bind((own, port)).await.expect("bind on the Tailscale address");
    println!("relaying {own}:{port} -> 127.0.0.1:{port}");
    loop {
        let Ok((inbound, from)) = listener.accept().await else { continue };
        tokio::spawn(async move {
            match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(outbound) => {
                    let _ = inbound.set_nodelay(true);
                    let _ = outbound.set_nodelay(true);
                    let (mut a, mut b) = (inbound, outbound);
                    let result = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                    println!("{from}: closed ({:?})", result.map(|(up, down)| up + down));
                }
                Err(e) => println!("{from}: could not reach 127.0.0.1:{port}: {e}"),
            }
        });
    }
}
