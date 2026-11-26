//! Reading private key files for import into the vault.

use russh::keys::ssh_key::{HashAlg, PrivateKey};
use russh::keys::{PublicKey, decode_secret_key};

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
    if let Ok(key) = PrivateKey::from_openssh(text) {
        let public = key.public_key();
        return Ok(KeyInfo {
            algorithm: key.algorithm().as_str().to_string(),
            public_key: public_line(public),
            fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
            encrypted: key.is_encrypted(),
            comment: key.comment().to_string(),
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
            })
        }
        Err(_) => Err(KeyFileError::NotAKey),
    }
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
}
