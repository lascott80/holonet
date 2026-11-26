//! Reading private key files for import into the vault, converting PuTTY
//! keys, and generating new keys.

use holonet_model::Secret;
use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey};
use russh::keys::{Algorithm, EcdsaCurve, PublicKey, decode_secret_key};

/// What Holonet shows and stores about an imported private key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    /// Algorithm name as OpenSSH writes it, e.g. "ssh-ed25519".
    pub algorithm: String,
    /// OpenSSH public key line without the comment.
    pub public_key: String,
    pub fingerprint: String,
    /// The key needs a passphrase.
    pub encrypted: bool,
    /// The comment stored in the key, if any.
    pub comment: String,
    /// A PuTTY `.ppk` file. Holonet stores it converted to OpenSSH format
    /// ([`convert_ppk`]), keeping its passphrase.
    pub ppk: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyFileError {
    #[error("This file is not a private key Holonet can read.")]
    NotAKey,
    #[error(
        "This key is in the old encrypted PEM format. Convert it with: ssh-keygen -p -f <file> (it keeps the passphrase)."
    )]
    EncryptedPem,
    #[error("This is a public key. Choose the private key file (the one without .pub).")]
    PublicKey,
    #[error("The passphrase did not unlock this key.")]
    WrongPassphrase,
    #[error("This key needs its passphrase to be converted.")]
    NeedsPassphrase,
}

fn public_line(key: &PublicKey) -> String {
    let mut key = key.clone();
    key.set_comment("");
    key.to_openssh().map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Inspect a private key file's text without needing its passphrase.
///
/// OpenSSH-format keys (`BEGIN OPENSSH PRIVATE KEY`) keep their public half
/// unencrypted, so it is read directly. Unencrypted PEM and PKCS#8 keys are
/// decoded. Encrypted PEM keys are refused with a conversion hint.
pub fn inspect_private_key(text: &str) -> Result<KeyInfo, KeyFileError> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("ssh-") || trimmed.starts_with("ecdsa-") {
        return Err(KeyFileError::PublicKey);
    }
    if trimmed.starts_with("PuTTY-User-Key-File-") {
        return inspect_ppk(text);
    }
    if let Ok(key) = PrivateKey::from_openssh(text) {
        let public = key.public_key();
        return Ok(KeyInfo {
            algorithm: key.algorithm().as_str().to_string(),
            public_key: public_line(public),
            fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
            encrypted: key.is_encrypted(),
            comment: key.comment().to_string(),
            ppk: false,
        });
    }
    if text.contains("Proc-Type: 4,ENCRYPTED") || text.contains("BEGIN ENCRYPTED PRIVATE KEY") {
        return Err(KeyFileError::EncryptedPem);
    }
    match decode_secret_key(text, None) {
        Ok(key) => {
            let public = key.public_key();
            Ok(KeyInfo {
                algorithm: key.algorithm().as_str().to_string(),
                public_key: public_line(public),
                fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
                encrypted: false,
                comment: String::new(),
                ppk: false,
            })
        }
        Err(_) => Err(KeyFileError::NotAKey),
    }
}

/// A `.ppk` file's headers and public half, readable without its passphrase:
///
/// ```text
/// PuTTY-User-Key-File-3: ssh-ed25519
/// Encryption: aes256-cbc
/// Comment: ...
/// Public-Lines: 2
/// <base64 public key blob>
/// ...
/// ```
fn inspect_ppk(text: &str) -> Result<KeyInfo, KeyFileError> {
    use base64::Engine;
    let mut lines = text.lines().map(str::trim_end);
    let mut algorithm = String::new();
    let mut encrypted = false;
    let mut comment = String::new();
    let mut blob = String::new();
    while let Some(line) = lines.next() {
        let Some((name, value)) = line.split_once(": ") else { continue };
        match name {
            n if n.starts_with("PuTTY-User-Key-File-") => algorithm = value.to_string(),
            "Encryption" => encrypted = value != "none",
            "Comment" => comment = value.to_string(),
            "Public-Lines" => {
                let count: usize = value.parse().map_err(|_| KeyFileError::NotAKey)?;
                for _ in 0..count {
                    blob.push_str(lines.next().ok_or(KeyFileError::NotAKey)?.trim());
                }
            }
            _ => {}
        }
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(blob).map_err(|_| KeyFileError::NotAKey)?;
    let public = PublicKey::from_bytes(&bytes).map_err(|_| KeyFileError::NotAKey)?;
    if public.algorithm().as_str() != algorithm {
        return Err(KeyFileError::NotAKey);
    }
    Ok(KeyInfo {
        algorithm,
        public_key: public_line(&public),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        encrypted,
        comment,
        ppk: true,
    })
}

/// Convert a PuTTY key to OpenSSH format for the vault. An encrypted key
/// needs its passphrase and stays encrypted with it.
pub fn convert_ppk(text: &str, passphrase: Option<&str>) -> Result<Secret, KeyFileError> {
    let info = inspect_ppk(text)?;
    let passphrase = passphrase.filter(|p| !p.is_empty());
    if info.encrypted && passphrase.is_none() {
        return Err(KeyFileError::NeedsPassphrase);
    }
    let key = PrivateKey::from_ppk(text, passphrase.map(String::from))
        .map_err(|_| if info.encrypted { KeyFileError::WrongPassphrase } else { KeyFileError::NotAKey })?;
    let key = match passphrase {
        Some(p) => key.encrypt(&mut rand::rng(), p).map_err(|_| KeyFileError::NotAKey)?,
        None => key,
    };
    let text = key.to_openssh(LineEnding::LF).map_err(|_| KeyFileError::NotAKey)?;
    Ok(Secret::new(text.as_str()))
}

/// A stored private key as an OpenSSH file for export: opened with its own
/// passphrase if it has one, then protected with `new_passphrase`, or left
/// unprotected when that is `None` or empty.
pub fn export_private_key(
    text: &str,
    passphrase: Option<&str>,
    new_passphrase: Option<&str>,
) -> Result<Secret, KeyFileError> {
    let key = PrivateKey::from_openssh(text).map_err(|_| KeyFileError::NotAKey)?;
    let key = if key.is_encrypted() {
        let p = passphrase.filter(|p| !p.is_empty()).ok_or(KeyFileError::NeedsPassphrase)?;
        key.decrypt(p).map_err(|_| KeyFileError::WrongPassphrase)?
    } else {
        key
    };
    let key = match new_passphrase.filter(|p| !p.is_empty()) {
        Some(p) => key.encrypt(&mut rand::rng(), p).map_err(|_| KeyFileError::NotAKey)?,
        None => key,
    };
    let text = key.to_openssh(LineEnding::LF).map_err(|_| KeyFileError::NotAKey)?;
    Ok(Secret::new(text.as_str()))
}

/// Key types offered by "Generate key" (Keychain.dc.html).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Ed25519,
    Rsa4096,
    EcdsaP256,
}

impl KeyKind {
    pub const ALL: [KeyKind; 3] = [KeyKind::Ed25519, KeyKind::Rsa4096, KeyKind::EcdsaP256];

    /// The label on the Generate panel's segmented control.
    pub fn label(self) -> &'static str {
        match self {
            KeyKind::Ed25519 => "Ed25519",
            KeyKind::Rsa4096 => "RSA 4096",
            KeyKind::EcdsaP256 => "ECDSA P-256",
        }
    }

    /// The note under the control, as drawn.
    pub fn note(self) -> &'static str {
        match self {
            KeyKind::Ed25519 => "Small, fast and the recommended default for modern servers.",
            KeyKind::Rsa4096 => "Widest compatibility with older servers and appliances.",
            KeyKind::EcdsaP256 => "For systems that require NIST curves.",
        }
    }
}

/// A newly generated key, ready for the vault.
pub struct GeneratedKey {
    /// OpenSSH private key text, encrypted when a passphrase was given.
    pub private_key: Secret,
    pub info: KeyInfo,
}

/// Generate a key. RSA 4096 takes a few hundred milliseconds in a release
/// build (a prime search, so it varies), so call this off the UI thread.
pub fn generate_key(kind: KeyKind, comment: &str, passphrase: Option<&str>) -> Result<GeneratedKey, KeyFileError> {
    let mut rng = rand::rng();
    let mut key = match kind {
        KeyKind::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519),
        KeyKind::EcdsaP256 => PrivateKey::random(&mut rng, Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }),
        KeyKind::Rsa4096 => {
            RsaKeypair::random(&mut rng, 4096).and_then(|kp| PrivateKey::new(KeypairData::from(kp), ""))
        }
    }
    .map_err(|_| KeyFileError::NotAKey)?;
    key.set_comment(comment);
    let passphrase = passphrase.filter(|p| !p.is_empty());
    let stored = match passphrase {
        Some(p) => key.encrypt(&mut rng, p).map_err(|_| KeyFileError::NotAKey)?,
        None => key.clone(),
    };
    let text = stored.to_openssh(LineEnding::LF).map_err(|_| KeyFileError::NotAKey)?;
    let public = key.public_key();
    Ok(GeneratedKey {
        private_key: Secret::new(text.as_str()),
        info: KeyInfo {
            algorithm: key.algorithm().as_str().to_string(),
            public_key: public_line(public),
            fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
            encrypted: passphrase.is_some(),
            comment: comment.to_string(),
            ppk: false,
        },
    })
}

/// The design's short label for an algorithm: "Ed25519", "RSA", "ECDSA P-256".
pub fn algorithm_label(algorithm: &str) -> String {
    match algorithm {
        a if a.contains("ed25519") => "Ed25519".into(),
        a if a.contains("rsa") => "RSA".into(),
        a if a.contains("nistp256") => "ECDSA P-256".into(),
        a if a.contains("nistp384") => "ECDSA P-384".into(),
        a if a.contains("nistp521") => "ECDSA P-521".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test key named in `.dev/test-server.env`, if the local test setup exists.
    fn read(var: &str) -> Option<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let env = std::fs::read_to_string(root.join(".dev/test-server.env")).ok()?;
        let path = env.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == var)?.1.trim().to_string();
        std::fs::read_to_string(root.join(path)).ok()
    }

    #[test]
    fn rejects_non_keys_and_public_keys() {
        assert_eq!(inspect_private_key("hello"), Err(KeyFileError::NotAKey));
        assert_eq!(
            inspect_private_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv+WaolLihpQrEDVKW7x3nFV6UNRu1tnH5rRVHTn/O2"),
            Err(KeyFileError::PublicKey)
        );
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert_eq!(inspect_private_key(pem), Err(KeyFileError::EncryptedPem));
    }

    #[test]
    fn reads_test_keys_when_present() {
        // Uses the local test keys in .dev/ when they exist (not in CI).
        let Some(plain) = read("HOLONET_TEST_KEY_ED25519") else { return };
        let info = inspect_private_key(&plain).unwrap();
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert!(!info.encrypted && info.fingerprint.starts_with("SHA256:"));
        let encrypted = read("HOLONET_TEST_KEY_ED25519_PASS").unwrap();
        let info = inspect_private_key(&encrypted).unwrap();
        assert!(info.encrypted, "passphrase key must be reported as encrypted");
        assert!(info.public_key.starts_with("ssh-ed25519 "));
        let rsa = inspect_private_key(&read("HOLONET_TEST_KEY_RSA").unwrap()).unwrap();
        assert_eq!(algorithm_label(&rsa.algorithm), "RSA");
    }

    #[test]
    fn generated_keys_read_back() {
        for kind in KeyKind::ALL {
            let plain = generate_key(kind, "holonet@test", None).unwrap();
            let info = inspect_private_key(plain.private_key.expose()).unwrap();
            assert_eq!(info.public_key, plain.info.public_key, "{kind:?}");
            assert!(!info.encrypted && info.comment == "holonet@test");
            assert!(decode_secret_key(plain.private_key.expose(), None).is_ok());
        }
        let locked = generate_key(KeyKind::Ed25519, "c", Some("pass phrase")).unwrap();
        assert!(locked.info.encrypted && inspect_private_key(locked.private_key.expose()).unwrap().encrypted);
        assert!(decode_secret_key(locked.private_key.expose(), Some("pass phrase")).is_ok());
        assert!(decode_secret_key(locked.private_key.expose(), Some("wrong")).is_err());
        assert_eq!(algorithm_label(&generate_key(KeyKind::EcdsaP256, "", None).unwrap().info.algorithm), "ECDSA P-256");
    }

    /// Throwaway keys made by PuTTY 0.83's own `puttygen` (putty-tools in
    /// WSL), with puttygen's OpenSSH export of the same key beside each.
    /// Encrypted ones use the passphrase "test pass".
    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn puttygen_keys_convert_to_the_same_openssh_key() {
        for (name, encrypted) in [
            ("putty-ed25519-v3-plain-1", false),
            ("putty-ed25519-v3-enc-2", true),
            ("putty-rsa-v2-enc-1", true),
            ("putty-ecdsa-v3-plain-1", false),
        ] {
            let ppk = fixture(&format!("{name}.ppk"));
            let pass = encrypted.then_some("test pass");
            let want = decode_secret_key(&fixture(&format!("{name}.openssh")), pass).unwrap();
            // The public half and comment are read without the passphrase.
            let info = inspect_private_key(&ppk).unwrap();
            assert!(info.ppk && info.encrypted == encrypted && info.comment == name);
            assert_eq!(info.public_key, public_line(want.public_key()), "{name}");
            // Converted, it is the same private key, still protected by the same passphrase.
            let converted = convert_ppk(&ppk, pass).unwrap();
            assert_eq!(inspect_private_key(converted.expose()).unwrap().encrypted, encrypted, "{name}");
            let back = decode_secret_key(converted.expose(), pass).unwrap();
            assert_eq!(back.key_data(), want.key_data(), "{name}");
            if encrypted {
                assert_eq!(convert_ppk(&ppk, None).err(), Some(KeyFileError::NeedsPassphrase));
                assert_eq!(convert_ppk(&ppk, Some("wrong")).err(), Some(KeyFileError::WrongPassphrase));
            }
        }
        // A passphrase given for an unencrypted .ppk protects the converted key.
        let converted = convert_ppk(&fixture("putty-ed25519-v3-plain-1.ppk"), Some("new")).unwrap();
        assert!(inspect_private_key(converted.expose()).unwrap().encrypted);
        // A damaged file is refused.
        let damaged = fixture("putty-ed25519-v3-plain-1.ppk").replace("Public-Lines: 2", "Public-Lines: 9");
        assert_eq!(inspect_private_key(&damaged), Err(KeyFileError::NotAKey));
    }

    #[test]
    fn export_rewraps_the_passphrase() {
        let key = generate_key(KeyKind::Ed25519, "export test", Some("old pass")).unwrap();
        let text = key.private_key.expose();
        assert_eq!(export_private_key(text, None, None).err(), Some(KeyFileError::NeedsPassphrase));
        assert_eq!(export_private_key(text, Some("wrong"), None).err(), Some(KeyFileError::WrongPassphrase));
        // A new passphrase, then none: the same key either way.
        let out = export_private_key(text, Some("old pass"), Some("new pass")).unwrap();
        let info = inspect_private_key(out.expose()).unwrap();
        assert!(info.encrypted && info.public_key == key.info.public_key);
        assert!(decode_secret_key(out.expose(), Some("new pass")).is_ok());
        let plain = export_private_key(text, Some("old pass"), Some("")).unwrap();
        assert!(!inspect_private_key(plain.expose()).unwrap().encrypted);
        assert!(plain.expose().starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
    }
}
