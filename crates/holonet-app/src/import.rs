//! OpenSSH import (FirstRun.dc.html's cards, the Hosts page's Import and
//! Known hosts' Import): finding the files, reading them with
//! `holonet-import`, and turning the result into hosts, keys and trusted
//! keys for the vault (`decisions.md`: imports are copied; originals are
//! untouched).
//!
//! Reading blocks (files, `\\wsl.localhost`), so the planning runs on a
//! background thread; applying a plan is a vault change on the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use holonet_import::config::{self, Included, Skipped};
use holonet_import::{JumpSpec, known_hosts, wildcard_match};
use holonet_model::{Auth, Host, KeyId, KnownHost, KnownHostSource, StoredKey, Timestamp, VaultData};

use crate::key_import;

/// Files larger than this are not config or known_hosts files.
const MAX_FILE: u64 = 1024 * 1024;

/// Where an import reads from.
#[derive(Clone, Debug)]
pub enum Source {
    /// `%USERPROFILE%\.ssh`.
    Windows,
    /// A WSL distribution's home, as `\\wsl.localhost\<distro>\home\<user>`.
    Wsl { distro: String, home: PathBuf },
    /// A file chosen anywhere.
    File(PathBuf),
}

impl Source {
    /// For messages: "%USERPROFILE%\.ssh\config", "Ubuntu: ~/.ssh/config".
    pub fn label(&self, file: &str) -> String {
        match self {
            Source::Windows => format!(r"%USERPROFILE%\.ssh\{file}"),
            Source::Wsl { distro, .. } => format!("{distro}: ~/.ssh/{file}"),
            Source::File(path) => path.display().to_string(),
        }
    }

    /// The home folder `~` stands for. A chosen file inside a `.ssh` folder
    /// takes that folder's parent (so a WSL path works the same way);
    /// otherwise the Windows profile.
    fn home(&self) -> Option<PathBuf> {
        let profile = || std::env::var_os("USERPROFILE").map(PathBuf::from);
        match self {
            Source::Windows => profile(),
            Source::Wsl { home, .. } => Some(home.clone()),
            Source::File(path) => {
                let dir = path.parent()?;
                if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case(".ssh")) {
                    dir.parent().map(Path::to_path_buf)
                } else {
                    profile()
                }
            }
        }
    }

    /// `\\wsl.localhost\<distro>` when reading from a distribution, where
    /// `/etc/...` paths live.
    fn linux_root(&self) -> Option<PathBuf> {
        let home = match self {
            Source::Wsl { home, .. } => home.clone(),
            Source::File(path) => path.clone(),
            Source::Windows => return None,
        };
        let text = home.to_string_lossy().to_string();
        let lower = text.to_ascii_lowercase();
        let rest = lower.strip_prefix(r"\\wsl.localhost\").or_else(|| lower.strip_prefix(r"\\wsl$\"))?;
        let distro_len = rest.find('\\').unwrap_or(rest.len());
        let prefix_len = text.len() - rest.len() + distro_len;
        Some(PathBuf::from(&text[..prefix_len]))
    }

    pub fn file(&self, name: &str) -> Option<PathBuf> {
        match self {
            Source::File(path) => Some(path.clone()),
            _ => Some(self.home()?.join(".ssh").join(name)),
        }
    }

    /// A path as a config writes it (`~/.ssh/id_ed25519`, `%d/.ssh/x`,
    /// `/etc/ssh/key`, `keys/work`, `C:\keys\work`) as a file to read.
    pub fn resolve(&self, written: &str) -> Option<PathBuf> {
        let home = self.home();
        let written = written.trim();
        let rest = written.strip_prefix("~/").or_else(|| written.strip_prefix("~\\"));
        let rest = rest.or_else(|| written.strip_prefix("%d/")).or_else(|| written.strip_prefix("%d\\"));
        if let Some(rest) = rest {
            return Some(join(&home?, rest));
        }
        if let Some(rest) = written.strip_prefix('/') {
            // A Linux path: inside the distribution, or nowhere on Windows.
            return Some(join(&self.linux_root()?, rest));
        }
        let path = Path::new(written);
        if path.is_absolute() || written.starts_with(r"\\") {
            return Some(path.to_path_buf());
        }
        // Relative paths are relative to ~/.ssh, as ssh reads Include.
        Some(join(&home?.join(".ssh"), written))
    }
}

/// Join with `/` and `\` both taken as separators.
fn join(base: &Path, rest: &str) -> PathBuf {
    rest.split(['/', '\\']).filter(|p| !p.is_empty()).fold(base.to_path_buf(), |p, part| p.join(part))
}

fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "There is no file here.".to_string(),
        _ => format!("Holonet could not read it: {e}"),
    })?;
    if meta.len() > MAX_FILE {
        return Err("It is larger than 1 MB, so it is not read.".into());
    }
    std::fs::read_to_string(path).map_err(|e| format!("Holonet could not read it: {e}"))
}

/// The files an `Include` names: one path, or a wildcard in its last part.
fn included(source: &Source, arg: &str) -> Vec<Included> {
    let Some(path) = source.resolve(arg) else {
        return vec![Included { name: arg.into(), text: Err("Holonet could not find where this points.".into()) }];
    };
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![Included { name: arg.into(), text: read_text(&path) }];
    }
    let Some(dir) = path.parent() else { return Vec::new() };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter(|e| wildcard_match(&name, &e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
        .into_iter()
        .map(|p| Included {
            name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            text: read_text(&p),
        })
        .collect()
}

/// What an imported host signs in with.
#[derive(Clone, Debug)]
pub enum KeyPlan {
    /// No `IdentityFile`: a password, asked on connect.
    Password,
    /// A key already in the Keychain (same public key).
    Existing { id: KeyId, name: String },
    /// A key file to copy into the Keychain.
    New(Box<StoredKey>),
}

/// One host from the config, ready to import.
#[derive(Clone, Debug)]
pub struct PlannedHost {
    pub host: Host,
    pub key: KeyPlan,
    pub jump: Vec<JumpSpec>,
    /// What was not used for this host, as sentences.
    pub notes: Vec<String>,
    /// A host of the same name is already in the vault.
    pub exists: bool,
}

impl PlannedHost {
    /// The preview's detail line.
    pub fn detail(&self) -> String {
        let mut parts = vec![self.host.target()];
        parts.push(match &self.key {
            KeyPlan::Password => "Password".into(),
            KeyPlan::Existing { name, .. } => format!("Key {name}"),
            KeyPlan::New(key) => format!("Key {} (new)", key.name),
        });
        if !self.jump.is_empty() {
            parts.push(format!("Through {}", self.jump.iter().map(|j| j.host.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if self.exists {
            parts.push("Already in the vault".into());
        }
        parts.join(" · ")
    }
}

#[derive(Clone, Debug)]
pub struct ConfigPlan {
    pub file: String,
    pub hosts: Vec<PlannedHost>,
    pub skipped: Vec<Skipped>,
}

/// What planning needs to know of the vault, copied for the background
/// thread.
pub struct VaultView {
    names: Vec<String>,
    keys: Vec<(String, KeyId, String)>,
}

impl VaultView {
    pub fn of(data: &VaultData) -> Self {
        Self {
            names: data.hosts.iter().map(|h| h.display_name().to_lowercase()).collect(),
            keys: data.keys.iter().map(|k| (k.public_key.clone(), k.id, k.name.clone())).collect(),
        }
    }
}

/// Read a source's `config` and plan the import.
pub fn plan_config(source: &Source, vault: &VaultView) -> Result<ConfigPlan, String> {
    let path = source.file("config").ok_or("Holonet could not find your profile folder.")?;
    let label = source.label("config");
    let text = read_text(&path).map_err(|why| format!("{label}: {why}"))?;
    let parsed = config::parse(&label, &text, &mut |arg| included(source, arg));
    // One Keychain entry per key file, however many hosts name it.
    let mut keys: HashMap<PathBuf, Result<KeyPlan, String>> = HashMap::new();
    let mut hosts = Vec::new();
    for h in parsed.hosts {
        let mut notes = h.notes.clone();
        let mut key = KeyPlan::Password;
        for written in &h.identity_files {
            let Some(path) = source.resolve(written) else {
                notes.push(format!("IdentityFile {written} could not be found."));
                continue;
            };
            let plan = keys.entry(path.clone()).or_insert_with(|| plan_key(source, &path, vault)).clone();
            match plan {
                Ok(plan) => {
                    key = plan;
                    break;
                }
                Err(why) => notes.push(format!("IdentityFile {written}: {why}")),
            }
        }
        let mut host = Host::new(h.address.clone());
        if h.name != h.address {
            host.label = h.name.clone();
        }
        host.port = h.port.unwrap_or(22);
        host.username = h.user.clone().unwrap_or_default();
        host.agent_forwarding = h.forward_agent.unwrap_or(false);
        host.compression = h.compression.unwrap_or(false);
        host.auth = match &key {
            KeyPlan::Password => Auth::Password { saved: None },
            KeyPlan::Existing { id, .. } => Auth::key(*id),
            KeyPlan::New(k) => Auth::key(k.id),
        };
        let exists = vault.names.contains(&host.display_name().to_lowercase());
        hosts.push(PlannedHost { host, key, jump: h.jump, notes, exists });
    }
    Ok(ConfigPlan { file: label, hosts, skipped: parsed.skipped })
}

fn plan_key(source: &Source, path: &Path, vault: &VaultView) -> Result<KeyPlan, String> {
    if !path.is_file() {
        return Err("there is no file there.".into());
    }
    let place = match source {
        Source::Wsl { distro, .. } => distro.clone(),
        _ if path.to_string_lossy().to_ascii_lowercase().starts_with(r"\\wsl") => "WSL".into(),
        _ => "~/.ssh".into(),
    };
    let found = key_import::read_file(path, &place);
    let info = found.result.clone().map_err(|e| lower_first(&e))?;
    if let Some((_, id, name)) = vault.keys.iter().find(|(pk, ..)| *pk == info.public_key) {
        return Ok(KeyPlan::Existing { id: *id, name: name.clone() });
    }
    if !found.importable() {
        return Err("it is a PuTTY key with a passphrase. Import it in the Keychain, which asks for it.".into());
    }
    key_import::stored_key(&found, None).map(|k| KeyPlan::New(Box::new(k))).map_err(|e| lower_first(&e.to_string()))
}

fn lower_first(text: &str) -> String {
    let mut c = text.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// Quick connect's host from `user@address`, `user@address:port` or an
/// `ssh` command, not saved. A jump host must be a saved host; a key file
/// is matched by name among the Keychain's keys.
pub fn quick_host(text: &str, d: &VaultData) -> Result<Host, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type user@address, such as deploy@10.0.0.4.".into());
    }
    let q = holonet_import::command::parse(text).map_err(|e| e.message().to_string())?;
    let (mut address, mut port) = (q.address.clone(), q.port);
    // user@host:port and user@[v6]:port, which ssh itself does not take
    // but people type.
    if port.is_none()
        && let Some(spec) = JumpSpec::parse(&address)
    {
        (address, port) = (spec.host, spec.port);
    }
    if !holonet_model::is_valid_address(&address) {
        return Err(format!("{address} is not a hostname or an IP address."));
    }
    let Some(user) = q.user.clone().filter(|u| !u.is_empty()) else {
        return Err(format!("Add the user name: user@{address}."));
    };
    let mut host = Host::new(address);
    host.username = user;
    host.port = port.unwrap_or(22).max(1);
    host.agent_forwarding = q.forward_agent.unwrap_or(false);
    host.compression = q.compression.unwrap_or(false);
    if let Some(hop) = q.jump.last() {
        let found = d
            .hosts
            .iter()
            .find(|h| h.display_name().eq_ignore_ascii_case(&hop.host) || h.address.eq_ignore_ascii_case(&hop.host));
        match found {
            Some(j) => {
                host.jump_host = Some(j.id);
                host.from_group.jump_host = false;
            }
            None => return Err(format!("The jump host {} is not saved. Add it as a host first.", hop.host)),
        }
    }
    if let Some(file) = &q.identity_file {
        let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
        match d.keys.iter().find(|k| k.name.eq_ignore_ascii_case(name)) {
            Some(k) => host.auth = Auth::key(k.id),
            None => return Err(format!("The key {name} is not in the Keychain. Import it there first.")),
        }
    }
    Ok(host)
}

/// What an applied import did, for its notice.
#[derive(Default)]
pub struct Outcome {
    pub hosts: usize,
    pub keys: usize,
    /// Hosts whose jump host is not in the vault.
    pub jumps_missing: Vec<String>,
}

impl Outcome {
    pub fn message(&self, skipped: usize) -> String {
        let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let mut text = format!("Imported {}", plural(self.hosts, "host", "hosts"));
        if self.keys > 0 {
            text.push_str(&format!(" and {}", plural(self.keys, "key", "keys")));
        }
        text.push('.');
        if skipped > 0 {
            text.push_str(&format!(" {} not imported.", plural(skipped, "entry was", "entries were")));
        }
        if !self.jumps_missing.is_empty() {
            text.push_str(&format!(
                " The jump host for {} is not in the vault: set it on the host form.",
                self.jumps_missing.join(", ")
            ));
        }
        text
    }
}

/// Add the chosen hosts, their new keys, and their jump hosts.
pub fn apply_config(d: &mut VaultData, chosen: Vec<PlannedHost>) -> Outcome {
    let mut out = Outcome::default();
    let mut added: Vec<(holonet_model::HostId, Vec<JumpSpec>)> = Vec::new();
    for p in chosen {
        if let KeyPlan::New(key) = p.key {
            // Another host in this import may have brought the same key.
            match d.keys.iter().find(|k| k.public_key == key.public_key) {
                Some(existing) => {
                    let id = existing.id;
                    let mut host = p.host;
                    host.auth = Auth::key(id);
                    added.push((host.id, p.jump));
                    d.hosts.push(host);
                    out.hosts += 1;
                    continue;
                }
                None => {
                    d.keys.push(*key);
                    out.keys += 1;
                }
            }
        }
        added.push((p.host.id, p.jump));
        d.hosts.push(p.host);
        out.hosts += 1;
    }
    // Jump hosts by name, among all hosts now in the vault. A chain
    // a,b means b's own jump host is a, which is set only on hosts this
    // import added and that have none.
    let find = |d: &VaultData, spec: &JumpSpec| {
        d.hosts
            .iter()
            .find(|h| h.display_name().eq_ignore_ascii_case(&spec.host))
            .or_else(|| {
                d.hosts
                    .iter()
                    .find(|h| h.address.eq_ignore_ascii_case(&spec.host) && spec.port.is_none_or(|p| p == h.port))
            })
            .map(|h| h.id)
    };
    let new_ids: Vec<_> = added.iter().map(|(id, _)| *id).collect();
    for (id, chain) in added {
        if chain.is_empty() {
            continue;
        }
        let ids: Vec<Option<holonet_model::HostId>> = chain.iter().map(|s| find(d, s)).collect();
        if ids.iter().any(Option::is_none) {
            if let Some(h) = d.host(id) {
                out.jumps_missing.push(h.display_name().to_string());
            }
            continue;
        }
        let ids: Vec<_> = ids.into_iter().flatten().collect();
        if let Some(host) = d.host_mut(id) {
            host.jump_host = ids.last().copied();
            host.from_group.jump_host = false;
        }
        for pair in ids.windows(2) {
            if new_ids.contains(&pair[1])
                && let Some(hop) = d.host_mut(pair[1])
                && hop.jump_host.is_none()
                && hop.id != pair[0]
            {
                hop.jump_host = Some(pair[0]);
                hop.from_group.jump_host = false;
            }
        }
    }
    out
}

/// Read a source's `known_hosts`.
pub fn read_known_hosts(source: &Source) -> Result<(String, known_hosts::KnownHosts), String> {
    let path = source.file("known_hosts").ok_or("Holonet could not find your profile folder.")?;
    let label = source.label("known_hosts");
    let text = read_text(&path).map_err(|why| format!("{label}: {why}"))?;
    Ok((label.clone(), known_hosts::parse(&label, &text)))
}

/// Add the trusted keys of a parsed `known_hosts`. A key different from
/// one already trusted for the same name is not imported (`decisions.md`:
/// changed keys never replace trusted ones on their own).
pub fn apply_known_hosts(d: &mut VaultData, parsed: &known_hosts::KnownHosts) -> String {
    let (mut added, mut same, mut differ) = (0, 0, 0);
    for entry in &parsed.entries {
        let public_key = entry.public_key();
        for name in &entry.names {
            let (host, port) = match name {
                known_hosts::Name::Plain { host, port } => (host.clone(), *port),
                known_hosts::Name::Hashed { .. } => (name.to_text(), 0),
            };
            match d
                .known_hosts
                .iter()
                .find(|k| k.host.eq_ignore_ascii_case(&host) && k.port == port)
                .or_else(|| (port != 0).then(|| d.known_host(&host, port)).flatten())
            {
                Some(k) if k.public_key == public_key => same += 1,
                Some(_) => differ += 1,
                None => {
                    d.known_hosts.push(KnownHost {
                        host,
                        port,
                        public_key: public_key.clone(),
                        trusted_at: Timestamp::now(),
                        last_matched_at: None,
                        source: KnownHostSource::ImportedKnownHosts,
                        mismatch: None,
                    });
                    added += 1;
                }
            }
        }
    }
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut text = format!("Imported {}.", plural(added, "trusted key", "trusted keys"));
    if same > 0 {
        text.push_str(&format!(" {} already trusted.", plural(same, "was", "were")));
    }
    if differ > 0 {
        text.push_str(&format!(
            " {} from the key Holonet trusts for that host, so {} not imported.",
            plural(differ, "differs", "differ"),
            if differ == 1 { "it was" } else { "they were" }
        ));
    }
    if !parsed.skipped.is_empty() {
        text.push_str(&format!(" {} skipped.", plural(parsed.skipped.len(), "line was", "lines were")));
    }
    text
}

/// Every trusted key as a `known_hosts` file.
pub fn export_known_hosts(d: &VaultData) -> String {
    known_hosts::write(d.known_hosts.iter().map(|k| (k.host.as_str(), k.port, k.public_key.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_like_ssh() {
        let wsl = Source::Wsl { distro: "Ubuntu".into(), home: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me") };
        let p = |s: &Source, w: &str| s.resolve(w).map(|p| p.display().to_string());
        assert_eq!(p(&wsl, "~/.ssh/id_ed25519").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\me\.ssh\id_ed25519"));
        assert_eq!(p(&wsl, "/etc/ssh/key").as_deref(), Some(r"\\wsl.localhost\Ubuntu\etc\ssh\key"));
        assert_eq!(p(&wsl, "conf.d/*").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\me\.ssh\conf.d\*"));
        assert_eq!(p(&wsl, r"C:\keys\work").as_deref(), Some(r"C:\keys\work"));
        // A chosen file inside a WSL .ssh folder behaves like WSL.
        let file = Source::File(PathBuf::from(r"\\wsl.localhost\Ubuntu\home\holotest\.ssh\config"));
        assert_eq!(p(&file, "~/.ssh/k").as_deref(), Some(r"\\wsl.localhost\Ubuntu\home\holotest\.ssh\k"));
        assert_eq!(p(&file, "/tmp/k").as_deref(), Some(r"\\wsl.localhost\Ubuntu\tmp\k"));
        // Linux paths have no place on Windows itself.
        let other = Source::File(PathBuf::from(r"C:\configs\ssh_config"));
        assert_eq!(p(&other, "/etc/key"), None);
    }

    fn planned(name: &str, address: &str, jump: &[&str]) -> PlannedHost {
        let mut host = Host::new(address);
        host.label = name.into();
        PlannedHost {
            host,
            key: KeyPlan::Password,
            jump: jump.iter().map(|j| JumpSpec::parse(j).unwrap()).collect(),
            notes: Vec::new(),
            exists: false,
        }
    }

    #[test]
    fn quick_hosts() {
        let mut d = VaultData::default();
        let mut gw = Host::new("203.0.113.20");
        gw.label = "bastion-east".into();
        let gw_id = gw.id;
        d.hosts.push(gw);
        let h = quick_host("deploy@10.0.0.4", &d).unwrap();
        assert_eq!((h.username.as_str(), h.address.as_str(), h.port), ("deploy", "10.0.0.4", 22));
        assert!(matches!(h.auth, Auth::Password { saved: None }));
        let h = quick_host("ssh -J bastion-east -p 2222 pi@nas-01", &d).unwrap();
        assert_eq!((h.port, h.jump_host), (2222, Some(gw_id)));
        assert_eq!(quick_host("root@[::1]:2200", &d).unwrap().port, 2200);
        assert_eq!(quick_host("pi@nas-01:2200", &d).unwrap().address, "nas-01");
        assert_eq!(quick_host("nas-01", &d).unwrap_err(), "Add the user name: user@nas-01.");
        assert!(quick_host("ssh -J nowhere a@b", &d).unwrap_err().contains("nowhere is not saved"));
        assert!(quick_host("a@b c d", &d).is_ok(), "a remote command is ignored");
        assert!(quick_host("a@host:99999", &d).is_err());
        assert!(quick_host("", &d).is_err());
    }

    #[test]
    fn jump_chains_link_by_name() {
        let mut d = VaultData::default();
        let mut gw = Host::new("203.0.113.20");
        gw.label = "gw".into();
        let gw_id = gw.id;
        d.hosts.push(gw);
        let out = apply_config(
            &mut d,
            vec![
                planned("inner", "10.0.0.2", &["gw", "mid"]),
                planned("mid", "10.0.0.1", &[]),
                planned("lost", "10.0.0.3", &["nowhere"]),
            ],
        );
        assert_eq!(out.hosts, 3);
        let id = |name: &str| d.hosts.iter().find(|h| h.label == name).unwrap().id;
        let (inner, mid) = (d.host(id("inner")).unwrap(), d.host(id("mid")).unwrap());
        assert_eq!(inner.jump_host, Some(mid.id));
        assert!(!inner.from_group.jump_host);
        // The chain's first hop becomes mid's own jump host.
        assert_eq!(mid.jump_host, Some(gw_id));
        assert_eq!(d.jump_chain(inner).unwrap(), [gw_id, mid.id]);
        assert_eq!(out.jumps_missing, ["lost"]);
        assert_eq!(
            out.message(2),
            "Imported 3 hosts. 2 entries were not imported. The jump host for lost is not in the vault: set it on the host form."
        );
    }

    #[test]
    fn known_hosts_never_replace_a_trusted_key() {
        let mut d = VaultData::default();
        d.trust_host_key("nas-01", 22, "ssh-ed25519 AAAAb2xk", KnownHostSource::FirstConnection);
        let text = "nas-01 ssh-ed25519 AAAAbmV3\nweb,10.0.0.5 ssh-ed25519 AAAAd2Vi\n[web]:2222 ssh-ed25519 AAAAd2Vi\n\
             |1|+kWW2e027W0ntGRif7cTkehL3qQ=|e7w2aLcIgIutyBL2seoXohL+9LU= ssh-ed25519 AAAAbG9j\n@revoked x ssh-ed25519 AAAA\n";
        let parsed = known_hosts::parse("kh", text);
        let message = apply_known_hosts(&mut d, &parsed);
        assert_eq!(
            message,
            "Imported 4 trusted keys. 1 differs from the key Holonet trusts for that host, so it was not imported. 1 line was skipped."
        );
        assert_eq!(d.known_host("nas-01", 22).unwrap().public_key, "ssh-ed25519 AAAAb2xk");
        assert_eq!(d.known_host("localhost", 22).unwrap().public_key, "ssh-ed25519 AAAAbG9j");
        assert_eq!(d.known_host("web", 2222).unwrap().source, KnownHostSource::ImportedKnownHosts);
        // Again: everything is already there.
        assert_eq!(
            apply_known_hosts(&mut d, &parsed),
            "Imported 0 trusted keys. 4 were already trusted. 1 differs from the key Holonet trusts for that host, so it was not imported. 1 line was skipped."
        );
        // Export writes them back, hashed names as they were.
        let out = export_known_hosts(&d);
        assert!(out.contains("nas-01 ssh-ed25519 AAAAb2xk\n") && out.contains("[web]:2222 ssh-ed25519 AAAAd2Vi\n"));
        assert!(out.contains("|1|+kWW2e027W0ntGRif7cTkehL3qQ=|e7w2aLcIgIutyBL2seoXohL+9LU= ssh-ed25519 AAAAbG9j\n"));
    }
}
