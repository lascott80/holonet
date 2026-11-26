//! Milestone 4 phase 7 test helper: add a jump host and a host behind it to
//! a throwaway test vault, and clear its Known hosts. Both copy the sign-in
//! of the vault's existing host `wsl-a` (the WSL test server). Never point
//! it at a real vault.
//!
//!   cargo run -p holonet-app --example m4_jump_seed -- <vault file> <vault password>
//!
//! Hosts added (port 2222):
//! - jump-a:   127.0.0.1, the jump host
//! - behind-a: localhost through jump-a. From WSL, localhost is the same
//!   server, but Known hosts has a separate entry for each name, so a clean
//!   Known hosts asks about two keys.

use holonet_model::Host;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, password) = (std::path::PathBuf::from(&args[0]), args[1].clone());
    let mut vault = holonet_vault::LockedVault::open(&path).unwrap().unlock(&password).unwrap();
    let template = vault.data().hosts.iter().find(|h| h.label == "wsl-a").expect("host wsl-a").clone();
    let make = |label: &str, address: &str| {
        let mut h = Host::new(address);
        h.label = label.into();
        h.port = template.port;
        h.username = template.username.clone();
        h.auth = template.auth.clone();
        h
    };
    let jump = make("jump-a", "127.0.0.1");
    let mut behind = make("behind-a", "localhost");
    behind.jump_host = Some(jump.id);
    vault
        .update(|d| {
            d.hosts.retain(|h| h.label != "jump-a" && h.label != "behind-a");
            d.hosts.extend([jump, behind]);
            d.known_hosts.clear();
        })
        .unwrap();
    println!("added jump-a and behind-a; Known hosts cleared");
}
