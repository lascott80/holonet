//! Session logs: the saved output of terminals with logging on
//! (`decisions.md`, owner decision 2 of milestone 7).
//!
//! Each log is its own file in a folder beside the vault file
//! (`vault.holonet.logs\<record id>.log`), so the vault stays small and
//! quick to save. Logs are encrypted under a key derived from the vault's
//! data key, so only the vault opens them, and they keep the vault's key
//! when its password changes.
//!
//! # File format (version 1)
//!
//! ```text
//! "HOLONETL"      8 bytes, magic
//! format version  u16 little endian
//! record id       16 bytes
//! chunks, each:
//!   length        u32 little endian, of the ciphertext
//!   nonce         24 bytes
//!   ciphertext    XChaCha20-Poly1305 of UTF-8 text, with its tag
//! ```
//!
//! Each chunk's associated data is the file's header and the chunk's number,
//! so chunks cannot be reordered, dropped from the middle, or moved to
//! another log. The key is per log: SHA-256 over a domain string, the
//! vault's log key and the record id.
//!
//! Text is sealed in chunks of up to 64 KB as it arrives, or when the app
//! flushes (on a timer and when the session ends). A crash loses at most
//! the text not yet sealed; a torn last chunk is skipped on reading.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use holonet_model::{RecordId, Timestamp};
use zeroize::Zeroizing;

use crate::{Result, VaultError, random};

const MAGIC: &[u8; 8] = b"HOLONETL";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 8 + 2 + 16;
/// Text sealed per chunk.
pub const CHUNK: usize = 64 * 1024;
/// Larger chunks than any writer makes mean the file is damaged.
const MAX_CHUNK: usize = 4 * CHUNK;
const LOG_KEY_DOMAIN: &[u8] = b"holonet-vault/log-key/v1";
const FILE_KEY_DOMAIN: &[u8] = b"holonet-vault/log-file/v1";

/// The folder of logs for the vault at `vault`: its file name plus `.logs`.
pub fn logs_dir(vault: &Path) -> PathBuf {
    let mut name = vault.file_name().unwrap_or_default().to_os_string();
    name.push(".logs");
    vault.with_file_name(name)
}

/// The key the vault's logs are encrypted under. Kept by the app while the
/// vault is locked, so open sessions keep logging (`decisions.md`: sessions
/// stay connected while locked). Wiped on drop.
#[derive(Clone)]
pub struct LogKey(Zeroizing<[u8; 32]>);

impl LogKey {
    pub(crate) fn from_data_key(data_key: &[u8; 32]) -> Self {
        Self(digest(&[LOG_KEY_DOMAIN, data_key]))
    }

    fn cipher_for(&self, id: RecordId) -> XChaCha20Poly1305 {
        let key = digest(&[FILE_KEY_DOMAIN, self.0.as_ref(), id.0.as_bytes()]);
        XChaCha20Poly1305::new(&(*key).into())
    }
}

impl std::fmt::Debug for LogKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LogKey(<redacted>)")
    }
}

fn digest(parts: &[&[u8]]) -> Zeroizing<[u8; 32]> {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    for part in parts {
        hash.update(part);
    }
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&hash.finalize());
    out
}

fn header(id: RecordId) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..8].copy_from_slice(MAGIC);
    h[8..10].copy_from_slice(&VERSION.to_le_bytes());
    h[10..].copy_from_slice(id.0.as_bytes());
    h
}

fn aad(header: &[u8; HEADER_LEN], n: u64) -> [u8; HEADER_LEN + 8] {
    let mut a = [0u8; HEADER_LEN + 8];
    a[..HEADER_LEN].copy_from_slice(header);
    a[HEADER_LEN..].copy_from_slice(&n.to_le_bytes());
    a
}

/// A log read back.
#[derive(Debug, PartialEq, Eq)]
pub struct LogText {
    pub text: String,
    /// The file ended part-way through a chunk, as after a crash. The
    /// chunks before it are in `text`.
    pub torn: bool,
}

/// Where a vault's logs live, and their key. Cheap to clone.
#[derive(Clone, Debug)]
pub struct LogStore {
    dir: PathBuf,
    key: LogKey,
}

impl LogStore {
    pub fn new(dir: PathBuf, key: LogKey) -> Self {
        Self { dir, key }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, id: RecordId) -> PathBuf {
        self.dir.join(format!("{}.log", id.0))
    }

    /// Start a new log for a session.
    pub fn create(&self, id: RecordId) -> Result<LogWriter> {
        fs::create_dir_all(&self.dir)?;
        let header = header(id);
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(self.path(id))?;
        file.write_all(&header)?;
        Ok(LogWriter { file, cipher: self.key.cipher_for(id), header, next: 0, buf: String::new(), bytes: 0 })
    }

    /// Read a whole log.
    pub fn read(&self, id: RecordId) -> Result<LogText> {
        let path = self.path(id);
        let bytes = fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => VaultError::NotFound(path.clone()),
            _ => VaultError::Io(e),
        })?;
        if bytes.len() < HEADER_LEN || bytes[..HEADER_LEN] != header(id) {
            return Err(VaultError::Format("this log's header does not match its session".into()));
        }
        let cipher = self.key.cipher_for(id);
        let head = header(id);
        let mut rest = &bytes[HEADER_LEN..];
        let mut text = String::new();
        let mut n = 0u64;
        while !rest.is_empty() {
            if rest.len() < 4 + 24 {
                return Ok(LogText { text, torn: true });
            }
            let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            if len > MAX_CHUNK + 16 {
                return Err(VaultError::Corrupt);
            }
            if rest.len() < 28 + len {
                return Ok(LogText { text, torn: true });
            }
            let nonce: [u8; 24] = rest[4..28].try_into().unwrap();
            let plain = Zeroizing::new(
                cipher
                    .decrypt(&nonce.into(), Payload { msg: &rest[28..28 + len], aad: &aad(&head, n) })
                    .map_err(|_| VaultError::Corrupt)?,
            );
            text.push_str(std::str::from_utf8(&plain).map_err(|_| VaultError::Corrupt)?);
            rest = &rest[28 + len..];
            n += 1;
        }
        Ok(LogText { text, torn: false })
    }

    /// Delete a log. A log that is not there is already deleted.
    pub fn delete(&self, id: RecordId) -> Result<()> {
        match fs::remove_file(self.path(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// When a log was last written, for closing records a crash left open.
    pub fn last_write(&self, id: RecordId) -> Option<Timestamp> {
        let modified = fs::metadata(self.path(id)).ok()?.modified().ok()?;
        let secs = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        Some(Timestamp(secs as i64))
    }

    /// Delete log files that belong to no record in `keep` (their record
    /// aged out or was deleted while a file was in use). Returns how many.
    pub fn remove_orphans(&self, keep: impl IntoIterator<Item = RecordId>) -> usize {
        let keep: HashSet<String> = keep.into_iter().map(|id| format!("{}.log", id.0)).collect();
        let Ok(entries) = fs::read_dir(&self.dir) else { return 0 };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".log") && !keep.contains(&name) && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// An open log, appended to as output arrives.
pub struct LogWriter {
    file: fs::File,
    cipher: XChaCha20Poly1305,
    header: [u8; HEADER_LEN],
    next: u64,
    buf: String,
    bytes: u64,
}

impl LogWriter {
    /// Add text. Full chunks are sealed and written at once.
    pub fn append(&mut self, text: &str) -> Result<()> {
        self.buf.push_str(text);
        self.bytes += text.len() as u64;
        while self.buf.len() >= CHUNK {
            let mut cut = CHUNK;
            while !self.buf.is_char_boundary(cut) {
                cut -= 1;
            }
            let rest = self.buf.split_off(cut);
            let chunk = std::mem::replace(&mut self.buf, rest);
            self.seal(&chunk)?;
        }
        Ok(())
    }

    /// Seal what is buffered, so it survives a crash.
    pub fn flush(&mut self) -> Result<()> {
        if !self.buf.is_empty() {
            let chunk = std::mem::take(&mut self.buf);
            self.seal(&chunk)?;
        }
        self.file.sync_data()?;
        Ok(())
    }

    /// Plain-text bytes taken so far, sealed or not (History's Log column).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Seal the rest and close. Returns the log's size in plain text.
    pub fn finish(mut self) -> Result<u64> {
        self.flush()?;
        Ok(self.bytes)
    }

    fn seal(&mut self, text: &str) -> Result<()> {
        let nonce = random::<24>();
        let sealed = self
            .cipher
            .encrypt(&nonce.into(), Payload { msg: text.as_bytes(), aad: &aad(&self.header, self.next) })
            .map_err(|_| VaultError::Format("could not encrypt the log".into()))?;
        let mut record = Vec::with_capacity(28 + sealed.len());
        record.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        record.extend_from_slice(&nonce);
        record.extend_from_slice(&sealed);
        // One write per chunk, so a crash tears at most the last one.
        self.file.write_all(&record)?;
        self.next += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path, key: u8) -> LogStore {
        LogStore::new(dir.join("vault.holonet.logs"), LogKey::from_data_key(&[key; 32]))
    }

    #[test]
    fn chunks_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let logs = store(dir.path(), 1);
        let id = RecordId::new();
        let mut w = logs.create(id).unwrap();
        // Several full chunks, a multi-byte character across a chunk edge,
        // then a short tail sealed by finish.
        let line = "deploy@prod-web-01:~$ systemctl is-active nginx ✓\n";
        let mut want = String::new();
        while want.len() < 3 * CHUNK + 100 {
            w.append(line).unwrap();
            want.push_str(line);
        }
        assert_eq!(w.finish().unwrap(), want.len() as u64);
        assert_eq!(logs.read(id).unwrap(), LogText { text: want.clone(), torn: false });
        // Nothing of the text is readable on disk.
        let raw = fs::read(logs.path(id)).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"deploy"));
        // Another vault's key does not open it.
        assert!(matches!(store(dir.path(), 2).read(id), Err(VaultError::Corrupt)));
    }

    #[test]
    fn torn_last_chunk_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let logs = store(dir.path(), 1);
        let id = RecordId::new();
        let mut w = logs.create(id).unwrap();
        w.append("first\n").unwrap();
        w.flush().unwrap();
        w.append("second\n").unwrap();
        w.flush().unwrap();
        drop(w);
        let path = logs.path(id);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
        assert_eq!(logs.read(id).unwrap(), LogText { text: "first\n".into(), torn: true });
        // Only a length's worth left of the torn chunk.
        let cut = HEADER_LEN + 28 + "first\n".len() + 16 + 3;
        fs::write(&path, &bytes[..cut]).unwrap();
        assert_eq!(logs.read(id).unwrap(), LogText { text: "first\n".into(), torn: true });
    }

    #[test]
    fn tampering_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let logs = store(dir.path(), 1);
        let (a, b) = (RecordId::new(), RecordId::new());
        for (id, text) in [(a, "one\n"), (b, "two\n")] {
            let mut w = logs.create(id).unwrap();
            w.append(text).unwrap();
            w.flush().unwrap();
            w.append(text).unwrap();
            w.finish().unwrap();
        }
        // Swapping the two chunks of a log.
        let bytes = fs::read(logs.path(a)).unwrap();
        let one = 28 + 4 + 16;
        let (head, body) = bytes.split_at(HEADER_LEN);
        let swapped = [head, &body[one..], &body[..one]].concat();
        fs::write(logs.path(a), swapped).unwrap();
        assert!(matches!(logs.read(a), Err(VaultError::Corrupt)));
        // Another log's file under this log's name.
        fs::copy(logs.path(b), logs.path(a)).unwrap();
        assert!(matches!(logs.read(a), Err(VaultError::Format(_))));
        // A flipped bit.
        let mut bytes = fs::read(logs.path(b)).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(logs.path(b), bytes).unwrap();
        assert!(matches!(logs.read(b), Err(VaultError::Corrupt)));
    }

    #[test]
    fn delete_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let logs = store(dir.path(), 1);
        let ids: Vec<RecordId> = (0..3).map(|_| RecordId::new()).collect();
        for id in &ids {
            logs.create(*id).unwrap().finish().unwrap();
        }
        fs::write(logs.dir().join("notes.txt"), "not a log").unwrap();
        assert!(logs.last_write(ids[0]).is_some());
        logs.delete(ids[0]).unwrap();
        logs.delete(ids[0]).unwrap();
        assert!(matches!(logs.read(ids[0]), Err(VaultError::NotFound(_))));
        assert_eq!(logs.remove_orphans([ids[1]]), 1);
        assert!(logs.read(ids[1]).is_ok() && logs.path(ids[1]).exists() && !logs.path(ids[2]).exists());
        assert!(logs.dir().join("notes.txt").exists());
        // A second log for the same record is refused, never overwritten.
        assert!(logs.create(ids[1]).is_err());
    }
}
