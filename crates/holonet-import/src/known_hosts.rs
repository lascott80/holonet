//! OpenSSH `known_hosts`: read for import, written for export.
//!
//! A line is `[marker] names key-type key [comment]`. Names are
//! comma-separated: `host`, `[host]:port`, or a hashed `|1|salt|hash`
//! (HMAC-SHA1 of the name under the salt, as `HashKnownHosts` writes them).
//! Hashed names cannot be read back, only matched against a host, so they
//! are kept as they are.
//!
//! Not imported, and counted: `@revoked` and `@cert-authority` lines,
//! wildcard and negated names, and lines that cannot be read.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};

use crate::config::{Skipped, Source};
use crate::{is_pattern, split_host_port};

/// One name a key is trusted for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Name {
    /// Lowercased, with its port (22 unless written as `[host]:port`).
    Plain {
        host: String,
        port: u16,
    },
    Hashed {
        salt: Vec<u8>,
        hash: Vec<u8>,
    },
}

impl Name {
    pub fn matches(&self, host: &str, port: u16) -> bool {
        match self {
            Name::Plain { host: h, port: p } => *p == port && h.eq_ignore_ascii_case(host),
            Name::Hashed { salt, hash } => hashed(salt, host, port).as_slice() == hash.as_slice(),
        }
    }

    /// As `known_hosts` writes it.
    pub fn to_text(&self) -> String {
        match self {
            Name::Plain { host, port } => written_name(host, *port),
            Name::Hashed { salt, hash } => format!("|1|{}|{}", B64.encode(salt), B64.encode(hash)),
        }
    }

    fn parse(text: &str) -> Option<Self> {
        if let Some(rest) = text.strip_prefix("|1|") {
            let (salt, hash) = rest.split_once('|')?;
            let (salt, hash) = (B64.decode(salt).ok()?, B64.decode(hash).ok()?);
            return (hash.len() == 20).then_some(Name::Hashed { salt, hash });
        }
        let (host, port) = if text.starts_with('[') { split_host_port(text)? } else { (text.to_string(), None) };
        Some(Name::Plain { host: host.to_ascii_lowercase(), port: port.unwrap_or(22) })
    }
}

/// `host`, or `[host]:port` when the port is not 22.
fn written_name(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 { host } else { format!("[{host}]:{port}") }
}

/// HMAC-SHA1 of the name as `ssh` hashes it.
pub fn hashed(salt: &[u8], host: &str, port: u16) -> [u8; 20] {
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(salt).expect("HMAC takes any key length");
    mac.update(written_name(host, port).as_bytes());
    mac.finalize().into_bytes().into()
}

/// A trusted key and the names it is trusted for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub names: Vec<Name>,
    pub key_type: String,
    /// Base64, as written.
    pub key: String,
    pub source: Source,
}

impl Entry {
    /// `key-type base64`, as Holonet keeps public keys.
    pub fn public_key(&self) -> String {
        format!("{} {}", self.key_type, self.key)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownHosts {
    pub entries: Vec<Entry>,
    pub skipped: Vec<Skipped>,
}

pub fn parse(file: &str, text: &str) -> KnownHosts {
    let mut out = KnownHosts::default();
    for (i, raw) in text.lines().enumerate() {
        let source = Source { file: file.to_string(), line: i + 1 };
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut skip = |reason: &str| {
            out.skipped.push(Skipped { what: shorten(line), reason: reason.into(), source: Some(source.clone()) })
        };
        let fields: Vec<&str> = line.split_whitespace().collect();
        if let Some(marker) = fields[0].strip_prefix('@') {
            skip(match marker {
                "revoked" => "A revoked key is not imported. Holonet never trusts it either way.",
                "cert-authority" => "Certificate authorities are not imported.",
                _ => "This marker is not one Holonet knows.",
            });
            continue;
        }
        let [names, key_type, key, ..] = fields[..] else {
            skip("This line does not have a name, a key type and a key.");
            continue;
        };
        if !(key_type.starts_with("ssh-") || key_type.starts_with("ecdsa-") || key_type.starts_with("sk-"))
            || B64.decode(key).is_err()
        {
            skip("The key on this line could not be read.");
            continue;
        }
        let mut parsed = Vec::new();
        let mut patterns = Vec::new();
        for n in names.split(',').filter(|n| !n.is_empty()) {
            if !n.starts_with('|') && is_pattern(n) {
                patterns.push(n);
                continue;
            }
            match Name::parse(n) {
                Some(name) => parsed.push(name),
                None => patterns.push(n),
            }
        }
        if !patterns.is_empty() {
            out.skipped.push(Skipped {
                what: patterns.join(","),
                reason: "Wildcard and unreadable names are not imported. The other names on the line are.".into(),
                source: Some(source.clone()),
            });
        }
        if !parsed.is_empty() {
            out.entries.push(Entry { names: parsed, key_type: key_type.into(), key: key.into(), source });
        }
    }
    out
}

fn shorten(line: &str) -> String {
    if line.chars().count() <= 60 { line.into() } else { format!("{}…", line.chars().take(59).collect::<String>()) }
}

/// A `known_hosts` file of `(host, port, "key-type base64")`, one line each.
pub fn write<'a>(keys: impl IntoIterator<Item = (&'a str, u16, &'a str)>) -> String {
    let mut out = String::from("# Exported from Holonet\n");
    for (host, port, key) in keys {
        out.push_str(&written_name(host, port));
        out.push(' ');
        out.push_str(key.trim());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

    #[test]
    fn plain_and_ported_names() {
        let text = format!(
            "# comment\n\nprod-web-01,10.20.0.11 ssh-ed25519 {KEY} deploy@laptop\n[Bastion.example.com]:2222 ecdsa-sha2-nistp256 {KEY}\n"
        );
        let k = parse("known_hosts", &text);
        assert_eq!(k.entries.len(), 2);
        let e = &k.entries[0];
        assert_eq!(
            e.names,
            [Name::Plain { host: "prod-web-01".into(), port: 22 }, Name::Plain { host: "10.20.0.11".into(), port: 22 }]
        );
        assert_eq!(e.public_key(), format!("ssh-ed25519 {KEY}"));
        assert_eq!(e.source.line, 3);
        assert!(k.entries[1].names[0].matches("bastion.example.com", 2222));
        assert!(!k.entries[1].names[0].matches("bastion.example.com", 22));
        assert!(k.skipped.is_empty());
    }

    #[test]
    fn hashed_names_match_as_ssh_writes_them() {
        // Written by `ssh-keygen -H` for "github.com" and "[nas-01]:2200".
        let salt = [7u8; 20];
        let line = |host: &str, port| {
            format!("|1|{}|{} ssh-ed25519 {KEY}", B64.encode(salt), B64.encode(hashed(&salt, host, port)))
        };
        let k = parse("known_hosts", &format!("{}\n{}\n", line("github.com", 22), line("nas-01", 2200)));
        assert_eq!(k.entries.len(), 2);
        let (a, b) = (&k.entries[0].names[0], &k.entries[1].names[0]);
        assert!(a.matches("GitHub.com", 22) && !a.matches("github.com", 2222) && !a.matches("gitlab.com", 22));
        assert!(b.matches("nas-01", 2200) && !b.matches("nas-01", 22));
        // They are written back as they were read.
        assert!(a.to_text().starts_with("|1|BwcH"));
        assert_eq!(Name::parse(&a.to_text()).as_ref(), Some(a));
    }

    #[test]
    fn names_hashed_by_openssh_match() {
        // Hashed by `ssh-keygen -H` (OpenSSH 10.2) from "localhost" and
        // "[nas-01]:2200" lines.
        let text = format!(
            "|1|+kWW2e027W0ntGRif7cTkehL3qQ=|e7w2aLcIgIutyBL2seoXohL+9LU= ssh-ed25519 {KEY}
|1|ZhDEZpjNLM0u56bewQHxmOP7LPM=|4+YOBUvXqr/xPCbxDEg28LBRnvQ= ssh-ed25519 {KEY}
"
        );
        let k = parse("known_hosts", &text);
        let (a, b) = (&k.entries[0].names[0], &k.entries[1].names[0]);
        assert!(a.matches("localhost", 22) && a.matches("LocalHost", 22) && !a.matches("localhost", 2200));
        assert!(b.matches("nas-01", 2200) && !b.matches("nas-01", 22) && !b.matches("localhost", 2200));
    }

    #[test]
    fn what_is_skipped() {
        let text = format!(
            "@revoked bad.example ssh-ed25519 {KEY}\n@cert-authority *.corp ssh-ed25519 {KEY}\n*.lan,nas-01,!x ssh-ed25519 {KEY}\nshort-line ssh-ed25519\nhost ssh-rsa not*base64\n|1|bad|hash ssh-ed25519 {KEY}\n"
        );
        let k = parse("known_hosts", &text);
        // nas-01 is kept from the line with wildcards.
        assert_eq!(k.entries.len(), 1);
        assert_eq!(k.entries[0].names, [Name::Plain { host: "nas-01".into(), port: 22 }]);
        let reasons: Vec<&str> = k.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(k.skipped.len(), 6, "{reasons:#?}");
        assert!(reasons[0].starts_with("A revoked key"));
        assert!(reasons[1].starts_with("Certificate authorities"));
        assert_eq!(k.skipped[2].what, "*.lan,!x");
        assert!(reasons[3].starts_with("This line does not have"));
        assert!(reasons[4].starts_with("The key on this line"));
        assert_eq!(k.skipped[5].what, "|1|bad|hash");
    }

    #[test]
    fn export_reads_back() {
        let pk = format!("ssh-ed25519 {KEY}");
        let text = write([("Prod-Web-01", 22, pk.as_str()), ("10.0.0.4", 2222, pk.as_str())]);
        assert_eq!(text, format!("# Exported from Holonet\nprod-web-01 {pk}\n[10.0.0.4]:2222 {pk}\n"));
        let k = parse("export", &text);
        assert_eq!(k.entries.len(), 2);
        assert!(k.entries[1].names[0].matches("10.0.0.4", 2222));
    }
}
