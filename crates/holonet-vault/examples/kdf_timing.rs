//! Time unlocking with several Argon2id settings, to choose `KdfParams::default()`.
//!
//!   cargo run -p holonet-vault --example kdf_timing --release

use std::time::Instant;

use holonet_vault::{KdfParams, LockedVault, UnlockedVault};

fn main() {
    let dir = std::env::temp_dir().join(format!("holonet-kdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let candidates = [
        KdfParams { m_kib: 19 * 1024, t: 2, p: 1 },
        KdfParams { m_kib: 64 * 1024, t: 3, p: 1 },
        KdfParams { m_kib: 64 * 1024, t: 3, p: 4 },
        KdfParams { m_kib: 128 * 1024, t: 3, p: 1 },
        KdfParams { m_kib: 256 * 1024, t: 2, p: 1 },
        KdfParams { m_kib: 256 * 1024, t: 3, p: 1 },
        KdfParams { m_kib: 512 * 1024, t: 2, p: 1 },
    ];
    println!("default: {:?}", KdfParams::default());
    for (i, kdf) in candidates.iter().enumerate() {
        let path = dir.join(format!("v{i}.holonet"));
        UnlockedVault::create_with(&path, "timing password", *kdf).unwrap();
        let locked = LockedVault::open(&path).unwrap();
        let runs = 3;
        let start = Instant::now();
        for _ in 0..runs {
            locked.unlock("timing password").unwrap();
        }
        let ms = start.elapsed().as_secs_f64() * 1e3 / runs as f64;
        println!("m={:>4} MiB t={} p={}  unlock {:>6.0} ms", kdf.m_kib / 1024, kdf.t, kdf.p, ms);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
