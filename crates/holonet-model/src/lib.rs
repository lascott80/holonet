//! Holonet's data: hosts, stored keys, known hosts and settings.
//!
//! Plain data with serde, no I/O. The vault (`holonet-vault`) encrypts and
//! stores a [`VaultData`]; everything else reads and edits it.
//!
//! Fields for later milestones (groups, Tailscale, jump hosts, snippets,
//! logging) are added when their milestone lands. `#[serde(default)]` on every
//! collection and optional field keeps older vault files readable.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Schema version of [`VaultData`]. Bump when a change needs a migration.
pub const SCHEMA_VERSION: u32 = 1;

/// Seconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        Self(secs)
    }
}

/// A secret string (password or passphrase). Wiped from memory on drop and
/// never printed by `Debug`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

id_type!(HostId);
id_type!(KeyId);

/// Icon shown for a host. Chosen by the person; defaults to a server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleIcon {
    #[default]
    Server,
    Database,
    Shield,
    Nas,
    Chip,
    Cloud,
}

/// How a host signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Auth {
    /// Password sign-in. `None` means ask on every connect.
    Password {
        #[serde(default)]
        saved: Option<Secret>,
    },
    /// A key stored in the vault. If the key has a passphrase and none is
    /// saved, it is asked for on connect.
    Key {
        key_id: KeyId,
        #[serde(default)]
        passphrase: Option<Secret>,
    },
}

impl Default for Auth {
    fn default() -> Self {
        Auth::Password { saved: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: HostId,
    /// Shown name; empty means "use the address".
    #[serde(default)]
    pub label: String,
    /// Hostname or IP address. Required.
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default = "yes")]
    pub keep_alive: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub icon: RoleIcon,
    pub created_at: Timestamp,
    #[serde(default)]
    pub last_connected_at: Option<Timestamp>,
}

fn default_port() -> u16 {
    22
}

fn yes() -> bool {
    true
}

/// A problem with a field on the host form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostProblem {
    AddressMissing,
    AddressInvalid,
    PortZero,
}

impl HostProblem {
    /// Interface text for the problem, in the design's voice.
    pub fn message(self) -> &'static str {
        match self {
            HostProblem::AddressMissing => "Add an address to test or save this host.",
            HostProblem::AddressInvalid => "Use a hostname or an IP address, without spaces or a user name.",
            HostProblem::PortZero => "Use a port from 1 to 65535.",
        }
    }
}

impl Host {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            id: HostId::new(),
            label: String::new(),
            address: address.into(),
            port: 22,
            username: String::new(),
            auth: Auth::default(),
            keep_alive: true,
            tags: Vec::new(),
            icon: RoleIcon::default(),
            created_at: Timestamp::now(),
            last_connected_at: None,
        }
    }

    /// Name to show: the label, or the address when there is none.
    pub fn display_name(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() { self.address.trim() } else { label }
    }

    /// `user@address:port` as shown on cards, with `:22` left out.
    pub fn target(&self) -> String {
        let user = if self.username.is_empty() { String::new() } else { format!("{}@", self.username) };
        let port = if self.port == 22 { String::new() } else { format!(":{}", self.port) };
        format!("{user}{}{port}", self.address)
    }

    /// The equivalent `ssh` command shown on the host form.
    pub fn ssh_command(&self) -> String {
        let address = if self.address.trim().is_empty() { "[address]" } else { self.address.trim() };
        let user = if self.username.is_empty() { String::new() } else { format!("{}@", self.username) };
        let port = if self.port == 22 { String::new() } else { format!(" -p {}", self.port) };
        format!("ssh {user}{address}{port}")
    }

    /// Problems that stop the host from being saved, in form order.
    pub fn problems(&self) -> Vec<HostProblem> {
        let mut problems = Vec::new();
        let address = self.address.trim();
        if address.is_empty() {
            problems.push(HostProblem::AddressMissing);
        } else if !is_valid_address(address) {
            problems.push(HostProblem::AddressInvalid);
        }
        if self.port == 0 {
            problems.push(HostProblem::PortZero);
        }
        problems
    }

    /// Copy with a new id and "copy" label, for Duplicate.
    pub fn duplicate(&self) -> Self {
        let mut copy = self.clone();
        copy.id = HostId::new();
        copy.label = format!("{} copy", self.display_name());
        copy.created_at = Timestamp::now();
        copy.last_connected_at = None;
        copy
    }
}

/// Hostname, IPv4, or IPv6 (optionally in brackets). Deliberately loose: it
/// rejects obvious mistakes (spaces, `user@`, `:port`, URLs), not unusual names.
pub fn is_valid_address(address: &str) -> bool {
    let a = address.strip_prefix('[').and_then(|a| a.strip_suffix(']')).unwrap_or(address);
    if a.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !a.is_empty()
        && a.len() <= 253
        && a.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// A private key copied into the vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredKey {
    pub id: KeyId,
    /// Shown name, e.g. the file name "id_ed25519".
    pub name: String,
    /// e.g. "ssh-ed25519", "rsa-sha2-512", "ecdsa-sha2-nistp256".
    pub algorithm: String,
    /// The OpenSSH private key exactly as imported. If it had a passphrase it
    /// is still encrypted with it (owner decision, milestone 2 plan).
    pub private_key: Secret,
    pub encrypted: bool,
    /// OpenSSH public key line, e.g. "ssh-ed25519 AAAA... comment".
    pub public_key: String,
    /// Where it was imported from, for the person's reference.
    #[serde(default)]
    pub source: Option<String>,
    pub imported_at: Timestamp,
}

/// Where a trusted host key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownHostSource {
    FirstConnection,
    ImportedKnownHosts,
}

/// A different key a server presented, recorded when the connection was stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mismatch {
    pub presented_key: String,
    pub seen_at: Timestamp,
}

/// A trusted server key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHost {
    /// Address as connected to (hostname or IP), lowercased.
    pub host: String,
    pub port: u16,
    /// OpenSSH public key line without comment, e.g. "ssh-ed25519 AAAA...".
    pub public_key: String,
    pub trusted_at: Timestamp,
    #[serde(default)]
    pub last_matched_at: Option<Timestamp>,
    pub source: KnownHostSource,
    #[serde(default)]
    pub mismatch: Option<Mismatch>,
}

impl KnownHost {
    /// The key type token, e.g. "ssh-ed25519".
    pub fn key_type(&self) -> &str {
        self.public_key.split_whitespace().next().unwrap_or("")
    }

    /// Short label for the key type as the design shows it: ED25519, ECDSA, RSA.
    pub fn key_type_label(&self) -> &'static str {
        key_type_label(self.key_type())
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.port == port && self.host.eq_ignore_ascii_case(host)
    }
}

pub fn key_type_label(key_type: &str) -> &'static str {
    match key_type {
        t if t.contains("ed25519") => "ED25519",
        t if t.contains("ecdsa") => "ECDSA",
        t if t.contains("rsa") => "RSA",
        t if t.contains("dss") => "DSA",
        _ => "Unknown",
    }
}

/// What to do when a server presents a key that is not in Known hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownHostKeys {
    #[default]
    Ask,
    Refuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub unknown_host_keys: UnknownHostKeys,
    /// Seconds before an unanswered connection attempt fails.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u32,
    /// Minutes idle before the vault locks; `None` means never.
    /// Enforced from milestone 4.
    #[serde(default = "default_lock_after")]
    pub lock_after_idle_mins: Option<u32>,
}

fn default_connect_timeout() -> u32 {
    10
}

fn default_lock_after() -> Option<u32> {
    Some(15)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unknown_host_keys: UnknownHostKeys::Ask,
            connect_timeout_secs: default_connect_timeout(),
            lock_after_idle_mins: default_lock_after(),
        }
    }
}

/// Everything the vault stores.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultData {
    pub schema: u32,
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub keys: Vec<StoredKey>,
    #[serde(default)]
    pub known_hosts: Vec<KnownHost>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for VaultData {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            hosts: Vec::new(),
            keys: Vec::new(),
            known_hosts: Vec::new(),
            settings: Settings::default(),
        }
    }
}

impl VaultData {
    pub fn host(&self, id: HostId) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    pub fn host_mut(&mut self, id: HostId) -> Option<&mut Host> {
        self.hosts.iter_mut().find(|h| h.id == id)
    }

    pub fn key(&self, id: KeyId) -> Option<&StoredKey> {
        self.keys.iter().find(|k| k.id == id)
    }

    pub fn known_host(&self, host: &str, port: u16) -> Option<&KnownHost> {
        self.known_hosts.iter().find(|k| k.matches(host, port))
    }

    pub fn known_host_mut(&mut self, host: &str, port: u16) -> Option<&mut KnownHost> {
        self.known_hosts.iter_mut().find(|k| k.matches(host, port))
    }

    /// Saved hosts that connect to this address and port.
    pub fn hosts_for(&self, host: &str, port: u16) -> impl Iterator<Item = &Host> {
        self.hosts.iter().filter(move |h| h.port == port && h.address.trim().eq_ignore_ascii_case(host))
    }

    /// Trust a key for `host:port`, replacing any key trusted before.
    pub fn trust_host_key(&mut self, host: &str, port: u16, public_key: &str, source: KnownHostSource) {
        let entry = KnownHost {
            host: host.to_ascii_lowercase(),
            port,
            public_key: public_key.to_string(),
            trusted_at: Timestamp::now(),
            last_matched_at: Some(Timestamp::now()),
            source,
            mismatch: None,
        };
        match self.known_host_mut(host, port) {
            Some(existing) => *existing = entry,
            None => self.known_hosts.push(entry),
        }
    }

    /// Remove a trusted key. Returns whether one was removed.
    pub fn forget_host_key(&mut self, host: &str, port: u16) -> bool {
        let before = self.known_hosts.len();
        self.known_hosts.retain(|k| !k.matches(host, port));
        self.known_hosts.len() != before
    }

    /// Delete a host. Its server key is kept unless `forget_key` is set and
    /// no other saved host uses the same address and port (decisions.md).
    pub fn delete_host(&mut self, id: HostId, forget_key: bool) -> Option<Host> {
        let index = self.hosts.iter().position(|h| h.id == id)?;
        let host = self.hosts.remove(index);
        if forget_key && self.hosts_for(host.address.trim(), host.port).next().is_none() {
            self.forget_host_key(host.address.trim(), host.port);
        }
        Some(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_and_target() {
        let mut h = Host::new("10.0.0.5");
        assert_eq!(h.display_name(), "10.0.0.5");
        assert_eq!(h.target(), "10.0.0.5");
        h.label = "nas-01".into();
        h.username = "admin".into();
        h.port = 2222;
        assert_eq!(h.display_name(), "nas-01");
        assert_eq!(h.target(), "admin@10.0.0.5:2222");
        assert_eq!(h.ssh_command(), "ssh admin@10.0.0.5 -p 2222");
        assert_eq!(Host::new("").ssh_command(), "ssh [address]");
    }

    #[test]
    fn address_validation() {
        for ok in ["nas-01", "pi.local", "10.0.0.5", "::1", "[fe80::1]", "a_b.example.com"] {
            assert!(is_valid_address(ok), "{ok} should be valid");
        }
        for bad in ["root@host", "host:22", "has space", "http://x", "-bad", "a..b"] {
            assert!(!is_valid_address(bad), "{bad} should be invalid");
        }
        assert_eq!(Host::new(" ").problems(), vec![HostProblem::AddressMissing]);
        let mut h = Host::new("ok");
        h.port = 0;
        assert_eq!(h.problems(), vec![HostProblem::PortZero]);
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let auth = Auth::Password { saved: Some(Secret::new("hunter2")) };
        assert!(!format!("{auth:?}").contains("hunter2"));
    }

    #[test]
    fn known_hosts_trust_replace_forget() {
        let mut d = VaultData::default();
        d.trust_host_key("Pi.Local", 22, "ssh-ed25519 AAAA1", KnownHostSource::FirstConnection);
        assert_eq!(d.known_host("pi.local", 22).unwrap().key_type_label(), "ED25519");
        d.trust_host_key("pi.local", 22, "ssh-ed25519 AAAA2", KnownHostSource::FirstConnection);
        assert_eq!(d.known_hosts.len(), 1);
        assert_eq!(d.known_host("pi.local", 22).unwrap().public_key, "ssh-ed25519 AAAA2");
        assert!(d.known_host("pi.local", 2222).is_none());
        assert!(d.forget_host_key("pi.local", 22));
        assert!(d.known_hosts.is_empty());
    }

    #[test]
    fn delete_host_keeps_key_unless_asked_and_unshared() {
        let mut d = VaultData::default();
        let a = Host::new("pi.local");
        let b = Host::new("pi.local");
        let (a_id, b_id) = (a.id, b.id);
        d.hosts.extend([a, b]);
        d.trust_host_key("pi.local", 22, "ssh-ed25519 K", KnownHostSource::FirstConnection);
        // Another host still uses the key, so forgetting is skipped.
        d.delete_host(a_id, true);
        assert!(d.known_host("pi.local", 22).is_some());
        // Default keeps the key.
        d.delete_host(b_id, false);
        assert!(d.known_host("pi.local", 22).is_some());
    }

    #[test]
    fn json_round_trip_and_old_files_load() {
        let mut d = VaultData::default();
        let mut h = Host::new("pi.local");
        h.auth = Auth::Key { key_id: KeyId::new(), passphrase: Some(Secret::new("p")) };
        d.hosts.push(h);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<VaultData>(&json).unwrap(), d);
        // A minimal file with only required fields still loads with defaults.
        let old =
            r#"{"schema":1,"hosts":[{"id":"00000000-0000-0000-0000-000000000001","address":"x","created_at":0}]}"#;
        let d: VaultData = serde_json::from_str(old).unwrap();
        assert_eq!(d.hosts[0].port, 22);
        assert!(d.hosts[0].keep_alive);
        assert_eq!(d.settings.connect_timeout_secs, 10);
    }
}
