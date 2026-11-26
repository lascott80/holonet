//! Milestone 4 phase 0, spike (c): generate each key type, encrypt it with a
//! passphrase, write it as OpenSSH text and read it back. Prints timings.
//!
//!   cargo run -p holonet-ssh --example m4_keygen [--release]

use std::time::Instant;

use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
use russh::keys::{Algorithm, EcdsaCurve, PrivateKey, decode_secret_key};

type Maker = Box<dyn Fn(&mut rand::rngs::ThreadRng) -> PrivateKey>;

fn main() {
    let mut rng = rand::rng();
    let kinds: Vec<(&str, Maker)> = vec![
        ("ed25519", Box::new(|r| PrivateKey::random(r, Algorithm::Ed25519).unwrap())),
        ("ecdsa-p256", Box::new(|r| PrivateKey::random(r, Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }).unwrap())),
        (
            "rsa-4096",
            Box::new(|r| {
                let kp = RsaKeypair::random(r, 4096).unwrap();
                PrivateKey::new(KeypairData::from(kp), "holonet@spike").unwrap()
            }),
        ),
    ];
    for (name, make) in &kinds {
        let runs = if name.starts_with("rsa") { 3 } else { 1 };
        for run in 0..runs {
            let t = Instant::now();
            let key = make(&mut rng);
            let generate = t.elapsed();
            let t = Instant::now();
            let encrypted = key.encrypt(&mut rng, "spike passphrase").unwrap();
            let text = encrypted.to_openssh(LineEnding::LF).unwrap();
            let encrypt = t.elapsed();
            let t = Instant::now();
            let back = decode_secret_key(&text, Some("spike passphrase")).unwrap();
            let decode = t.elapsed();
            assert_eq!(back.public_key().key_data(), key.public_key().key_data());
            println!(
                "{name:<11} run {run}: generate {:>8.1} ms  encrypt+encode {:>6.1} ms  decode {:>6.1} ms  {}",
                generate.as_secs_f64() * 1e3,
                encrypt.as_secs_f64() * 1e3,
                decode.as_secs_f64() * 1e3,
                key.public_key().to_openssh().unwrap().split(' ').next().unwrap()
            );
        }
    }
}
