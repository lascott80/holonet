//! Live check of `holonet-net` on this PC: Tailscale's state (no peer names
//! printed), and the route check and "skip the wait" answer for a range of
//! addresses.
//!
//!   cargo run -p holonet-net --example net_check --release

use std::time::Instant;

use holonet_net::tailscale::{self, Tailscale};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let t = Instant::now();
    let ts = tailscale::status().await;
    let took = t.elapsed();
    match &ts {
        Tailscale::Running(s) => println!(
            "Tailscale: running ({} own addresses, {} peers, {} online) in {:.1} ms",
            s.own_ips.len(),
            s.peers.len(),
            s.peers.iter().filter(|p| p.online).count(),
            took.as_secs_f64() * 1e3
        ),
        other => println!("Tailscale: {other:?} in {:.1} ms; says: {:?}", took.as_secs_f64() * 1e3, other.problem()),
    }
    let own = match &ts {
        Tailscale::Running(s) => s.own_ips.iter().find(|ip| ip.is_ipv4()).map(|ip| ip.to_string()),
        _ => None,
    };
    let mut addresses: Vec<(String, &str)> = vec![
        ("127.0.0.1".into(), "loopback"),
        ("192.168.1.51".into(), "private, typical home LAN"),
        ("10.255.255.1".into(), "private, unlikely to be local"),
        ("172.31.255.1".into(), "private, near WSL's range"),
        ("100.100.100.100".into(), "Tailscale's own service address"),
        ("100.127.255.254".into(), "Tailscale range, no device"),
        ("192.0.2.1".into(), "public documentation address"),
        ("8.8.8.8".into(), "public"),
        ("fd00::1".into(), "IPv6 unique local"),
    ];
    if let Some(own) = own {
        addresses.push((own, "this PC's own Tailscale address"));
    }
    for (address, what) in &addresses {
        let ip = address.parse().unwrap();
        let t = Instant::now();
        let check = holonet_net::route_to(ip);
        let skip = holonet_net::primary_unreachable(address, ts.is_running());
        let label = if what.contains("own Tailscale") { "100.x (own)" } else { address.as_str() };
        println!(
            "{label:<16} {what:<34} route {:<26} skip wait: {skip:<5} ({:.2} ms)",
            format!("{check:?}"),
            t.elapsed().as_secs_f64() * 1e3
        );
    }
}
