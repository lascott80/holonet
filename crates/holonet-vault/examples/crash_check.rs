//! Crash safety check: a child process saves in a tight loop and is killed at
//! a random moment, many times over. After every kill the vault must still
//! open with the right password, from the main file or its backup.
//!
//!   cargo run -p holonet-vault --example crash_check --release -- [rounds]

use std::process::Command;
use std::time::Duration;

use holonet_model::Host;
use holonet_vault::{KdfParams, LockedVault, UnlockedVault};

const PASSWORD: &str = "crash check password";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--child") {
        child(&args[2]);
        return;
    }
    let rounds: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);

    let dir = std::env::temp_dir().join(format!("holonet-crash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("vault.holonet");
    UnlockedVault::create_with(&path, PASSWORD, KdfParams::insecure_for_tests()).unwrap();

    let me = std::env::current_exe().unwrap();
    let (mut from_main, mut from_backup, mut restored) = (0, 0, 0);
    let mut seed = std::process::id() as u64;
    for round in 1..=rounds {
        let mut child = Command::new(&me).arg("--child").arg(&path).spawn().unwrap();
        // Pseudo-random kill time between 20 and 220 ms.
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        std::thread::sleep(Duration::from_millis(20 + (seed >> 33) % 200));
        child.kill().unwrap();
        child.wait().unwrap();

        let locked = LockedVault::open(&path).unwrap_or_else(|e| panic!("round {round}: vault did not open: {e}"));
        let vault = locked.unlock(PASSWORD).unwrap_or_else(|e| panic!("round {round}: vault did not unlock: {e}"));
        if locked.from_backup {
            from_backup += 1;
        } else if vault.restored_from_backup {
            restored += 1;
        } else {
            from_main += 1;
        }
    }
    println!(
        "{rounds} kills mid-save: vault opened every time ({from_main} from main file, \
         {from_backup} from backup with main missing, {restored} from backup with main damaged); \
         last vault has {} hosts",
        LockedVault::open(&path).unwrap().unlock(PASSWORD).unwrap().data().hosts.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save as fast as possible until killed.
fn child(path: &str) {
    let path = std::path::Path::new(path);
    let mut vault = LockedVault::open(path).unwrap().unlock(PASSWORD).unwrap();
    loop {
        vault
            .update(|d| {
                d.hosts.push(Host::new(format!("host-{}", d.hosts.len())));
                if d.hosts.len() > 200 {
                    d.hosts.clear();
                }
            })
            .unwrap();
    }
}
