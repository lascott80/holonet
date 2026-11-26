//! Milestone 4 phase 6 test helper: add route test hosts to a throwaway test
//! vault. Each copies the sign-in of the vault's existing host `wsl-a` (the
//! WSL test server) and gets this PC's own Tailscale address, which
//! `ts_relay` forwards to the test server. Never point it at a real vault.
//!
//!   cargo run -p holonet-app --example m4_routes_seed -- <vault file> <vault password>
//!
//! Hosts added (port 2222):
//! - ts-route:    127.0.0.1 (trusted), route Tailscale: shared trust
//! - ts-fallback: 192.0.2.1 (public, no answer), Automatic: waits, then Tailscale
//! - ts-skip:     10.255.255.1 (private, no route), Automatic: straight to Tailscale
//! - ts-primary:  192.0.2.1, route Primary: times out, offers "Try Tailscale"

use holonet_model::{Host, Route};
use holonet_net::tailscale::{self, Tailscale};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, password) = (std::path::PathBuf::from(&args[0]), args[1].clone());
    let Tailscale::Running(status) = tailscale::status().await else { panic!("Tailscale is not running") };
    let own = status.own_ips.iter().find(|ip| ip.is_ipv4()).expect("Tailscale IPv4").to_string();
    let mut vault = holonet_vault::LockedVault::open(&path).unwrap().unlock(&password).unwrap();
    let template = vault.data().hosts.iter().find(|h| h.label == "wsl-a").expect("host wsl-a").clone();
    let make = |label: &str, address: &str, route: Route| {
        let mut h = Host::new(address);
        h.label = label.into();
        h.port = template.port;
        h.username = template.username.clone();
        h.auth = template.auth.clone();
        h.tailscale_address = Some(own.clone());
        h.route = route;
        h
    };
    let hosts = [
        make("ts-route", "127.0.0.1", Route::Tailscale),
        make("ts-fallback", "192.0.2.1", Route::Automatic),
        make("ts-skip", "10.255.255.1", Route::Automatic),
        make("ts-primary", "192.0.2.1", Route::Primary),
    ];
    vault
        .update(|d| {
            d.hosts.retain(|h| !h.label.starts_with("ts-"));
            d.hosts.extend(hosts);
        })
        .unwrap();
    println!("added 4 route test hosts");
}
