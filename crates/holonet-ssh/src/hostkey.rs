//! Server keys as presented during the handshake, and how they compare with
//! what Known hosts trusts.

use holonet_model::{KnownHost, key_type_label};
use russh::keys::{HashAlg, PublicKey};

/// The key a server presented, with the details the host key prompt shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedKey {
    /// Address as connected to.
    pub host: String,
    pub port: u16,
    /// OpenSSH public key line without a comment, e.g. "ssh-ed25519 AAAA...".
    pub public_key: String,
    /// OpenSSH SHA-256 fingerprint, e.g. "SHA256:abc..." (unpadded base64).
    pub fingerprint: String,
}

impl PresentedKey {
    pub fn from_key(host: &str, port: u16, key: &PublicKey) -> Self {
        let mut key = key.clone();
        key.set_comment("");
        let public_key = key.to_openssh().map(|s| s.trim().to_string()).unwrap_or_default();
        Self { host: host.to_string(), port, public_key, fingerprint: key.fingerprint(HashAlg::Sha256).to_string() }
    }

    /// The design's short label: ED25519, ECDSA or RSA.
    pub fn key_type_label(&self) -> &'static str {
        key_type_label(self.public_key.split_whitespace().next().unwrap_or(""))
    }
}

/// SHA-256 fingerprint of an OpenSSH public key line, or `None` if the line
/// does not parse.
pub fn fingerprint(public_key: &str) -> Option<String> {
    PublicKey::from_openssh(public_key).ok().map(|k| k.fingerprint(HashAlg::Sha256).to_string())
}

/// How a presented key compares with Known hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    /// Matches the trusted key: connect without asking.
    Trusted,
    /// Nothing trusted for this address and port: ask (or refuse, by setting).
    Unknown,
    /// A different key is trusted: always stop (decisions.md).
    Changed { trusted_key: String },
}

/// Compare a presented key with the trusted entry for its address, if any.
/// Keys are compared by algorithm and key data; comments are ignored.
pub fn classify(trusted: Option<&KnownHost>, presented: &PresentedKey) -> KeyStatus {
    let Some(entry) = trusted else { return KeyStatus::Unknown };
    if same_key(&entry.public_key, &presented.public_key) {
        KeyStatus::Trusted
    } else {
        KeyStatus::Changed { trusted_key: entry.public_key.clone() }
    }
}

fn same_key(a: &str, b: &str) -> bool {
    let fields = |s: &str| s.split_whitespace().take(2).map(str::to_string).collect::<Vec<_>>();
    fields(a) == fields(b) && !fields(a).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holonet_model::{KnownHostSource, Timestamp};

    // Throwaway public keys generated for these tests (private halves discarded).
    const ED_A: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv+WaolLihpQrEDVKW7x3nFV6UNRu1tnH5rRVHTn/O2";
    const ED_B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIiz9/8t90hFvCBdDbtvdac7F8nUj6zg5Nl/OKS5cpuZ";

    fn known(key: &str) -> KnownHost {
        KnownHost {
            host: "pi.local".into(),
            port: 22,
            public_key: key.into(),
            trusted_at: Timestamp(0),
            last_matched_at: None,
            source: KnownHostSource::FirstConnection,
            mismatch: None,
        }
    }

    fn presented(key: &str) -> PresentedKey {
        PresentedKey { host: "pi.local".into(), port: 22, public_key: key.into(), fingerprint: String::new() }
    }

    #[test]
    fn classifies_trusted_unknown_changed() {
        assert_eq!(classify(None, &presented(ED_A)), KeyStatus::Unknown);
        assert_eq!(classify(Some(&known(ED_A)), &presented(ED_A)), KeyStatus::Trusted);
        // Comments do not matter.
        assert_eq!(classify(Some(&known(&format!("{ED_A} me@pc"))), &presented(ED_A)), KeyStatus::Trusted);
        assert_eq!(
            classify(Some(&known(ED_A)), &presented(ED_B)),
            KeyStatus::Changed { trusted_key: ED_A.to_string() }
        );
    }

    #[test]
    fn fingerprints_match_openssh_format() {
        let fp = fingerprint(ED_A).unwrap();
        assert!(fp.starts_with("SHA256:"));
        assert!(!fp.ends_with('='), "OpenSSH fingerprints are unpadded");
        assert_eq!(fingerprint("not a key"), None);
    }

    #[test]
    fn presented_key_drops_comment() {
        let key = PublicKey::from_openssh(&format!("{ED_A} someone@somewhere")).unwrap();
        let p = PresentedKey::from_key("pi.local", 22, &key);
        assert_eq!(p.public_key, ED_A);
        assert_eq!(p.key_type_label(), "ED25519");
        assert_eq!(Some(p.fingerprint.clone()), fingerprint(ED_A));
    }
}
