//! Finding and reading private keys to copy into the vault (`decisions.md`:
//! "a key file anywhere, `~/.ssh` in the Windows profile, and `~/.ssh` in an
//! installed WSL distribution"). Shared by the Keychain and the host form.
//!
//! Everything here blocks (file reads, `wsl.exe`), so callers run it on a
//! background thread. Files are only read, never changed.

use std::path::{Path, PathBuf};

use holonet_model::{KeyId, Secret, StoredKey, Timestamp};
use holonet_ssh::{KeyFileError, KeyInfo, convert_ppk, inspect_private_key};

/// A file that might be a private key, and what reading it found.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    /// Where it came from, for the list: "~/.ssh" or "Ubuntu-26.04".
    pub place: String,
    pub text: String,
    pub result: Result<KeyInfo, String>,
}

impl Found {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Whether it can be imported from a list without asking anything:
    /// readable, and not a PuTTY key that needs its passphrase to convert.
    pub fn importable(&self) -> bool {
        matches!(&self.result, Ok(info) if !(info.ppk && info.encrypted))
    }
}

/// Files in `~/.ssh` that are never private keys.
fn skip(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".pub")
        || lower.starts_with("known_hosts")
        || lower.starts_with("authorized_keys")
        || matches!(lower.as_str(), "config" | "environment" | "rc")
        || lower.ends_with(".old")
        || lower.ends_with(".bak")
}

/// Read one file as a possible key. Reading stops at 64 KB: keys are small.
pub fn read_file(path: &Path, place: &str) -> Found {
    let text = std::fs::metadata(path)
        .ok()
        .filter(|m| m.len() <= 64 * 1024)
        .and_then(|_| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let result = if text.is_empty() {
        Err("Holonet could not read this file.".to_string())
    } else {
        inspect_private_key(&text).map_err(|e| e.to_string())
    };
    Found { path: path.to_path_buf(), place: place.to_string(), text, result }
}

/// Every possible key in a folder, private keys first, then by name.
/// Files that are clearly not keys (`.pub`, `known_hosts`, `config`) are
/// left out; anything else that does not read as a key is listed with why.
pub fn scan_dir(dir: &Path, place: &str) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Found> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| !skip(&e.file_name().to_string_lossy()))
        .map(|e| read_file(&e.path(), place))
        // Not-a-key files (notes, scripts) are noise in the list.
        .filter(|f| f.result.as_ref().err().is_none_or(|e| e != &KeyFileError::NotAKey.to_string()))
        .collect();
    found.sort_by_key(|f| (f.result.is_err(), f.name().to_lowercase()));
    found
}

/// `~/.ssh` in the Windows profile.
pub fn windows_ssh_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".ssh"))
}

/// Installed WSL distributions and each one's `~/.ssh`, as a Windows path
/// (`\\wsl.localhost\<distro>\home\<user>\.ssh`). Asks `wsl.exe`, which
/// starts a distribution that is not running. Docker's own distributions
/// are left out.
pub fn wsl_ssh_dirs() -> Result<Vec<(String, PathBuf)>, String> {
    let list = wsl(&["-l", "-q"]).map_err(|e| format!("WSL did not answer: {e}"))?;
    let distros: Vec<String> = decode_wsl_text(&list)
        .lines()
        .map(|l| l.trim().trim_matches('\u{feff}').to_string())
        .filter(|l| !l.is_empty() && !l.to_ascii_lowercase().starts_with("docker-desktop"))
        .collect();
    if distros.is_empty() {
        return Err("No WSL distributions are installed.".into());
    }
    let mut dirs = Vec::new();
    for distro in distros {
        let Ok(out) = wsl(&["-d", &distro, "-e", "sh", "-c", "printf %s \"$HOME\""]) else { continue };
        let home = String::from_utf8_lossy(&out).trim().to_string();
        if !home.starts_with('/') {
            continue;
        }
        let path = format!(r"\\wsl.localhost\{distro}{}\.ssh", home.replace('/', "\\"));
        dirs.push((distro, PathBuf::from(path)));
    }
    Ok(dirs)
}

fn wsl(args: &[&str]) -> std::io::Result<Vec<u8>> {
    let mut command = std::process::Command::new("wsl.exe");
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = command.output()?;
    if out.status.success() { Ok(out.stdout) } else { Err(std::io::Error::other(decode_wsl_text(&out.stderr))) }
}

/// `wsl.exe` writes its own messages as UTF-16 (little-endian) and a
/// distribution's output as UTF-8.
fn decode_wsl_text(bytes: &[u8]) -> String {
    let looks_utf16 =
        bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count() * 2 >= bytes.len() / 2;
    if looks_utf16 {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// The comment generated keys get: `holonet@<computer name>`.
pub fn default_comment() -> String {
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this-pc".into()).to_lowercase();
    format!("holonet@{computer}")
}

/// A key ready for the vault from an imported file. A PuTTY key is
/// converted to OpenSSH format, keeping its passphrase (which it needs).
pub fn stored_key(found: &Found, ppk_passphrase: Option<&str>) -> Result<StoredKey, KeyFileError> {
    let info = found.result.clone().map_err(|_| KeyFileError::NotAKey)?;
    let private_key =
        if info.ppk { convert_ppk(&found.text, ppk_passphrase)? } else { Secret::new(found.text.as_str()) };
    let path = found.path.display().to_string();
    let source = if found.place.is_empty() {
        path
    } else if path.to_ascii_lowercase().starts_with(r"\\wsl") {
        // `\\wsl.localhost\Ubuntu\home\me\.ssh\id_ed25519` reads better as
        // the distribution's own path.
        format!("{}: ~/.ssh/{}", found.place, found.name())
    } else {
        format!("{}: {path}", found.place)
    };
    Ok(StoredKey {
        id: KeyId::new(),
        name: found.name(),
        algorithm: info.algorithm,
        private_key,
        encrypted: info.encrypted,
        comment: info.comment,
        public_key: info.public_key,
        saved_passphrase: None,
        source: Some(source),
        imported_at: Timestamp::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_list_output_is_utf16() {
        let utf16: Vec<u8> =
            "Ubuntu-26.04\r\ndocker-desktop\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_wsl_text(&utf16), "Ubuntu-26.04\r\ndocker-desktop\r\n");
        assert_eq!(decode_wsl_text(b"/home/me"), "/home/me");
    }

    #[test]
    fn scan_lists_keys_and_skips_known_non_keys() {
        let dir = std::env::temp_dir().join(format!("holonet-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = holonet_ssh::generate_key(holonet_ssh::KeyKind::Ed25519, "scan@test", None).unwrap();
        std::fs::write(dir.join("id_ed25519"), key.private_key.expose()).unwrap();
        std::fs::write(dir.join("id_ed25519.pub"), &key.info.public_key).unwrap();
        std::fs::write(dir.join("known_hosts"), "x").unwrap();
        std::fs::write(dir.join("config"), "Host x").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a key").unwrap();
        let found = scan_dir(&dir, "~/.ssh");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found.iter().map(Found::name).collect::<Vec<_>>(), ["id_ed25519"]);
        assert!(found[0].importable());
        let stored = stored_key(&found[0], None).unwrap();
        assert_eq!((stored.public_key, stored.comment), (key.info.public_key, "scan@test".to_string()));
        assert!(stored.source.unwrap().starts_with("~/.ssh: "));
    }
}
