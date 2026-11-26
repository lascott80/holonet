//! Milestone 4 phase 8 test helper: set a throwaway test vault's idle lock
//! to a number of minutes (Settings offers 15, 60 or Never), so the idle
//! lock can be watched without waiting 15 minutes. Never point it at a real
//! vault.
//!
//!   cargo run -p holonet-app --example m4_idle_seed -- <vault file> <vault password> <minutes>

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, password) = (std::path::PathBuf::from(&args[0]), args[1].clone());
    let minutes: u32 = args[2].parse().expect("minutes");
    let mut vault = holonet_vault::LockedVault::open(&path).unwrap().unlock(&password).unwrap();
    vault.update(|d| d.settings.lock_after_idle_mins = Some(minutes)).unwrap();
    println!("idle lock set to {minutes} min");
}
