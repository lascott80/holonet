//! Milestone 4 phase 2: check `.ppk` import against files made by PuTTY's
//! own `puttygen` (putty-tools 0.83 in WSL).
//!
//!   cargo run -p holonet-ssh --example m4_ppk -- <dir>
//!
//! For each `<name>.ppk` in `<dir>` with `<name>.openssh` beside it (the
//! same key exported by puttygen as OpenSSH), reads the .ppk with Holonet's
//! import, converts it, and checks the result is the same key. Names
//! containing `enc` use the passphrase "test pass".

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("dir"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".ppk").map(String::from))
        .collect();
    names.sort();
    let (mut ok, mut failed) = (0, 0);
    for name in names {
        let ppk = std::fs::read_to_string(dir.join(format!("{name}.ppk"))).unwrap();
        let openssh = std::fs::read_to_string(dir.join(format!("{name}.openssh"))).unwrap();
        let pass = name.contains("enc").then_some("test pass");
        let want = match russh::keys::decode_secret_key(&openssh, pass) {
            Ok(k) => k,
            Err(e) => {
                let convert = holonet_ssh::convert_ppk(&ppk, pass).map(|_| "converted").map_err(|e| e.to_string());
                println!("skip {name}: puttygen's OpenSSH copy does not decode ({e}); the .ppk alone: {convert:?}");
                continue;
            }
        };
        let info = holonet_ssh::inspect_private_key(&ppk);
        let result = holonet_ssh::convert_ppk(&ppk, pass).map_err(|e| e.to_string()).and_then(|c| {
            let back = russh::keys::decode_secret_key(c.expose(), pass).map_err(|e| e.to_string())?;
            Ok(back.public_key().key_data() == want.public_key().key_data() && back.key_data() == want.key_data())
        });
        let header = ppk.lines().next().unwrap_or("");
        let fine = info.as_ref().is_ok_and(|i| i.encrypted == pass.is_some()) && result == Ok(true);
        println!(
            "{} {name:<22} {header:<38} inspect {:<5} convert {:?}",
            if fine { "ok  " } else { "FAIL" },
            info.is_ok(),
            result
        );
        if fine { ok += 1 } else { failed += 1 }
    }
    println!("{ok} imported correctly, {failed} failed");
}
