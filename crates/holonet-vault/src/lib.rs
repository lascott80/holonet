//! Holonet's encrypted vault file.
//!
//! # File format (version 1)
//!
//! ```text
//! "HOLONETV"            8 bytes, magic
//! format version        u16 little endian
//! header length         u32 little endian
//! header                JSON (see `Header`)
//! body                  XChaCha20-Poly1305 ciphertext of `VaultData` as JSON
//! ```
//!
//! A random 256-bit **data key** encrypts the body. The whole prefix (magic,
//! version, length, header) is the body's associated data, so any change to
//! the header is detected. The header holds **key slots**, each of which wraps
//! the data key under a key-encryption key:
//!
//! - `password` (milestone 2): Argon2id over the vault password, with the salt
//!   and cost stored in the slot so they can be raised later.
//! - `hello` (milestone 4): SHA-256 of a Windows Hello signature over a random
//!   challenge stored in the slot. Hello's key signs the same challenge the
//!   same way every time (milestone 4 phase 0), so the signature is a stable
//!   secret that only Hello on this PC can produce. This crate never calls
//!   Windows: the app asks Hello for the signature and passes it in.
//!
//! Files with a Hello slot are written as format 2, so older builds report
//! "saved by a newer version" instead of failing on the unknown slot.
//! Files without one stay format 1.
//!
//! Every save writes a fresh body nonce. Saves are atomic: the new file is
//! written beside the old one and renamed over it, and the previous good file
//! is kept as `<name>.bak`. Opening falls back to the backup if the main file
//! is missing or unreadable.
//!
//! Session logs are separate files beside the vault, under a key derived from
//! the data key: see [`logs`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use holonet_model::VaultData;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

pub use holonet_model;

pub mod logs;
pub use logs::{LogKey, LogStore, LogText, LogWriter, logs_dir};

const MAGIC: &[u8; 8] = b"HOLONETV";
/// Newest file format version this build reads and writes.
pub const FORMAT_VERSION: u16 = 2;
/// Minimum vault password length in characters (owner decision, 25 Nov 2026).
pub const MIN_PASSWORD_LEN: usize = 8;
/// Associated data for wrapping the data key in a password slot.
const SLOT_AAD: &[u8] = b"holonet-vault/password-slot/v1";
/// Associated data for wrapping the data key in a Windows Hello slot.
const HELLO_AAD: &[u8] = b"holonet-vault/hello-slot/v1";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("no vault file at {0}")]
    NotFound(PathBuf),
    #[error("vault file could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("this is not a Holonet vault file, or it is damaged: {0}")]
    Format(String),
    #[error("this vault was saved by a newer version of Holonet (format {0})")]
    NewerFormat(u16),
    #[error("wrong vault password")]
    WrongPassword,
    #[error("this vault has no Windows Hello key")]
    NoHello,
    #[error("Windows Hello's key did not open this vault")]
    HelloRejected,
    #[error("the vault contents failed their integrity check")]
    Corrupt,
    #[error("the vault password must be at least {MIN_PASSWORD_LEN} characters")]
    PasswordTooShort,
    #[error("a vault already exists at {0}")]
    AlreadyExists(PathBuf),
}

pub type Result<T> = std::result::Result<T, VaultError>;

/// Argon2id cost for password slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Iterations.
    pub t: u32,
    /// Parallelism.
    pub p: u32,
}

impl Default for KdfParams {
    /// 256 MiB, 3 passes, 1 lane: about 0.33 s to unlock on the Snapdragon X2
    /// Plus. The planned 64 MiB took only 77 ms, so memory was raised.
    /// Measured with `examples/kdf_timing.rs`; see docs/prototype-notes.md.
    fn default() -> Self {
        Self { m_kib: 256 * 1024, t: 3, p: 1 }
    }
}

impl KdfParams {
    /// Very cheap parameters for tests only.
    pub fn insecure_for_tests() -> Self {
        Self { m_kib: 64, t: 1, p: 1 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Slot {
    Password {
        kdf: KdfParams,
        #[serde(with = "b64")]
        salt: Vec<u8>,
        #[serde(with = "b64")]
        nonce: Vec<u8>,
        #[serde(with = "b64")]
        wrapped_key: Vec<u8>,
    },
    Hello {
        /// Name of the Hello key credential (`KeyCredentialManager`).
        credential: String,
        #[serde(with = "b64")]
        challenge: Vec<u8>,
        #[serde(with = "b64")]
        nonce: Vec<u8>,
        #[serde(with = "b64")]
        wrapped_key: Vec<u8>,
    },
}

/// What the app needs to ask Windows Hello for a vault's key: the
/// credential's name and the challenge to sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloRequest {
    pub credential: String,
    pub challenge: Vec<u8>,
}

/// A new random challenge for [`UnlockedVault::set_hello`].
pub fn new_hello_challenge() -> Vec<u8> {
    random::<32>().to_vec()
}

/// The key-encryption key from a Hello signature.
fn hello_kek(signature: &[u8]) -> DataKey {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(signature);
    let mut kek = Zeroizing::new([0u8; 32]);
    kek.copy_from_slice(&digest);
    kek
}

/// How an unlock proves itself.
#[derive(Clone, Copy)]
enum Proof<'a> {
    Password(&'a str),
    HelloSignature(&'a [u8]),
}

fn hello_request(slots: &[Slot]) -> Option<HelloRequest> {
    slots.iter().find_map(|s| match s {
        Slot::Hello { credential, challenge, .. } => {
            Some(HelloRequest { credential: credential.clone(), challenge: challenge.clone() })
        }
        Slot::Password { .. } => None,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Header {
    slots: Vec<Slot>,
    #[serde(with = "b64")]
    body_nonce: Vec<u8>,
}

mod b64 {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(v))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(s).map_err(serde::de::Error::custom)
    }
}

type DataKey = Zeroizing<[u8; 32]>;

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("operating system random number generator failed");
    out
}

fn derive_kek(password: &str, salt: &[u8], kdf: KdfParams) -> Result<DataKey> {
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
        .map_err(|e| VaultError::Format(format!("bad key derivation parameters: {e}")))?;
    let mut kek = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, kek.as_mut())
        .map_err(|e| VaultError::Format(format!("key derivation failed: {e}")))?;
    Ok(kek)
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&(*key).into())
}

fn nonce(bytes: &[u8]) -> Result<XNonce> {
    let array: [u8; 24] = bytes.try_into().map_err(|_| VaultError::Format("bad nonce length".into()))?;
    Ok(array.into())
}

fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LEN { Err(VaultError::PasswordTooShort) } else { Ok(()) }
}

fn password_slot(password: &str, data_key: &[u8; 32], kdf: KdfParams) -> Result<Slot> {
    let salt = random::<16>();
    let kek = derive_kek(password, &salt, kdf)?;
    let slot_nonce = random::<24>();
    let wrapped_key = cipher(&kek)
        .encrypt(&slot_nonce.into(), Payload { msg: data_key, aad: SLOT_AAD })
        .map_err(|_| VaultError::Format("could not wrap the data key".into()))?;
    Ok(Slot::Password { kdf, salt: salt.to_vec(), nonce: slot_nonce.to_vec(), wrapped_key })
}

/// Split a file into (associated data prefix, header, body).
fn parse(bytes: &[u8]) -> Result<(&[u8], Header, &[u8])> {
    let fixed = MAGIC.len() + 2 + 4;
    if bytes.len() < fixed || &bytes[..8] != MAGIC {
        return Err(VaultError::Format("missing Holonet vault signature".into()));
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version > FORMAT_VERSION {
        return Err(VaultError::NewerFormat(version));
    }
    if version == 0 {
        return Err(VaultError::Format("format version 0".into()));
    }
    let header_len = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    let header_end = fixed.checked_add(header_len).filter(|&end| end <= bytes.len());
    let header_end = header_end.ok_or_else(|| VaultError::Format("header runs past the end of the file".into()))?;
    let header: Header = serde_json::from_slice(&bytes[fixed..header_end])
        .map_err(|e| VaultError::Format(format!("header is not readable: {e}")))?;
    Ok((&bytes[..header_end], header, &bytes[header_end..]))
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Write `bytes` to `path` atomically, keeping the previous file as `.bak`.
///
/// Order: write and flush `.tmp`; move the current file to `.bak`; move
/// `.tmp` into place. If the process dies between the two moves, `open`
/// finds no main file and uses `.bak`, the last good version.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if path.exists() {
        fs::rename(path, backup_path(path))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Default vault location: `%APPDATA%\Holonet\vault.holonet`.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Holonet").join("vault.holonet"))
}

/// Whether a vault (or its backup) exists at `path`.
pub fn exists(path: &Path) -> bool {
    path.exists() || backup_path(path).exists()
}

/// A vault file that has been read but not decrypted.
pub struct LockedVault {
    path: PathBuf,
    /// Main file, if it has a readable header.
    main: Option<Vec<u8>>,
    /// Backup file, if it has a readable header.
    backup: Option<Vec<u8>>,
    /// True when the main file is missing or unreadable and only the backup
    /// can be used.
    pub from_backup: bool,
}

impl LockedVault {
    /// Read the vault at `path` and its backup.
    pub fn open(path: &Path) -> Result<Self> {
        let readable = |bytes: Vec<u8>| parse(&bytes).is_ok().then_some(bytes);
        let main_read = fs::read(path);
        let backup = fs::read(backup_path(path)).ok().and_then(readable);
        let main = match main_read {
            Ok(bytes) => match parse(&bytes) {
                Ok(_) => Some(bytes),
                // A newer format must not silently fall back to an older backup.
                Err(e @ VaultError::NewerFormat(_)) => return Err(e),
                Err(e) if backup.is_none() => return Err(e),
                Err(_) => None,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if backup.is_none() {
                    return Err(VaultError::NotFound(path.to_path_buf()));
                }
                None
            }
            Err(e) if backup.is_none() => return Err(e.into()),
            Err(_) => None,
        };
        let from_backup = main.is_none();
        Ok(Self { path: path.to_path_buf(), main, backup, from_backup })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The Windows Hello credential and challenge, if the vault has a Hello
    /// slot. Read from the main file, or the backup when only it is readable.
    pub fn hello(&self) -> Option<HelloRequest> {
        let bytes = self.main.as_ref().or(self.backup.as_ref())?;
        parse(bytes).ok().and_then(|(_, header, _)| hello_request(&header.slots))
    }

    /// Decrypt with the vault password. If the main file's contents fail
    /// their integrity check, the backup is tried; the result says so in
    /// [`UnlockedVault::restored_from_backup`].
    pub fn unlock(&self, password: &str) -> Result<UnlockedVault> {
        self.unlock_with(Proof::Password(password))
    }

    /// Decrypt with Windows Hello's signature over [`Self::hello`]'s challenge.
    pub fn unlock_with_hello(&self, signature: &[u8]) -> Result<UnlockedVault> {
        self.unlock_with(Proof::HelloSignature(signature))
    }

    fn unlock_with(&self, proof: Proof) -> Result<UnlockedVault> {
        match (&self.main, &self.backup) {
            (Some(main), backup) => match self.unlock_bytes(main, proof) {
                Err(VaultError::Corrupt) if backup.is_some() => {
                    let mut vault = self.unlock_bytes(backup.as_ref().unwrap(), proof)?;
                    vault.restored_from_backup = true;
                    Ok(vault)
                }
                other => other,
            },
            (None, Some(backup)) => {
                let mut vault = self.unlock_bytes(backup, proof)?;
                vault.restored_from_backup = true;
                Ok(vault)
            }
            (None, None) => Err(VaultError::NotFound(self.path.clone())),
        }
    }

    fn unlock_bytes(&self, bytes: &[u8], proof: Proof) -> Result<UnlockedVault> {
        let (aad, header, body) = parse(bytes)?;
        let unwrap = |kek: &[u8; 32], slot_nonce: &[u8], wrapped: &[u8], slot_aad: &[u8]| -> Result<Option<DataKey>> {
            match cipher(kek).decrypt(&nonce(slot_nonce)?, Payload { msg: wrapped, aad: slot_aad }) {
                Ok(mut key) => {
                    let array: [u8; 32] = key.as_slice().try_into().map_err(|_| VaultError::Corrupt)?;
                    key.zeroize();
                    Ok(Some(Zeroizing::new(array)))
                }
                Err(_) => Ok(None),
            }
        };
        let mut data_key = None;
        for slot in &header.slots {
            data_key = match (slot, proof) {
                (Slot::Password { kdf, salt, nonce: n, wrapped_key }, Proof::Password(password)) => {
                    unwrap(&*derive_kek(password, salt, *kdf)?, n, wrapped_key, SLOT_AAD)?
                }
                (Slot::Hello { nonce: n, wrapped_key, .. }, Proof::HelloSignature(signature)) => {
                    unwrap(&hello_kek(signature), n, wrapped_key, HELLO_AAD)?
                }
                _ => None,
            };
            if data_key.is_some() {
                break;
            }
        }
        let data_key = match (data_key, proof) {
            (Some(key), _) => key,
            (None, Proof::Password(_)) => return Err(VaultError::WrongPassword),
            (None, Proof::HelloSignature(_)) if hello_request(&header.slots).is_none() => {
                return Err(VaultError::NoHello);
            }
            (None, Proof::HelloSignature(_)) => return Err(VaultError::HelloRejected),
        };
        let plaintext = Zeroizing::new(
            cipher(&data_key)
                .decrypt(&nonce(&header.body_nonce)?, Payload { msg: body, aad })
                .map_err(|_| VaultError::Corrupt)?,
        );
        let mut data: VaultData = serde_json::from_slice(&plaintext).map_err(|_| VaultError::Corrupt)?;
        if data.schema > holonet_model::SCHEMA_VERSION {
            return Err(VaultError::NewerFormat(FORMAT_VERSION));
        }
        // Older data is brought up to date in memory; the next save writes it.
        data.migrate();
        Ok(UnlockedVault { path: self.path.clone(), slots: header.slots, data_key, data, restored_from_backup: false })
    }
}

/// An open vault. Dropping it (or calling [`UnlockedVault::lock`]) wipes the
/// data key; secrets in [`VaultData`] wipe themselves on drop.
pub struct UnlockedVault {
    path: PathBuf,
    slots: Vec<Slot>,
    data_key: DataKey,
    data: VaultData,
    /// The main file was damaged and the previous save was opened instead.
    /// The next save replaces the damaged file. The interface should say so.
    pub restored_from_backup: bool,
}

impl UnlockedVault {
    /// Create a new, empty vault at `path` and save it.
    pub fn create(path: &Path, password: &str) -> Result<Self> {
        Self::create_with(path, password, KdfParams::default())
    }

    pub fn create_with(path: &Path, password: &str, kdf: KdfParams) -> Result<Self> {
        check_password(password)?;
        if exists(path) {
            return Err(VaultError::AlreadyExists(path.to_path_buf()));
        }
        let data_key = Zeroizing::new(random::<32>());
        let slot = password_slot(password, &data_key, kdf)?;
        let vault = Self {
            path: path.to_path_buf(),
            slots: vec![slot],
            data_key,
            data: VaultData::default(),
            restored_from_backup: false,
        };
        vault.save()?;
        Ok(vault)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn data(&self) -> &VaultData {
        &self.data
    }

    /// Change the data and save. If saving fails the change is kept in memory
    /// and the error returned, so the caller can retry.
    pub fn update<R>(&mut self, change: impl FnOnce(&mut VaultData) -> R) -> Result<R> {
        let result = change(&mut self.data);
        self.save()?;
        Ok(result)
    }

    /// Encrypt the current data into file bytes. Separate from writing so a
    /// caller can encrypt on one thread and write on another.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let body_nonce = random::<24>();
        let header = Header { slots: self.slots.clone(), body_nonce: body_nonce.to_vec() };
        let header_json = serde_json::to_vec(&header).map_err(|e| VaultError::Format(e.to_string()))?;
        let version: u16 = if self.has_hello() { 2 } else { 1 };
        let mut out = Vec::with_capacity(header_json.len() + 1024);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
        out.extend_from_slice(&header_json);
        let plaintext = Zeroizing::new(serde_json::to_vec(&self.data).map_err(|e| VaultError::Format(e.to_string()))?);
        let body = cipher(&self.data_key)
            .encrypt(&body_nonce.into(), Payload { msg: &plaintext, aad: &out })
            .map_err(|_| VaultError::Format("could not encrypt the vault".into()))?;
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &self.encode()?)
    }

    /// Replace the password slot. The data key and contents stay the same.
    pub fn change_password(&mut self, new_password: &str) -> Result<()> {
        self.change_password_with(new_password, KdfParams::default())
    }

    pub fn change_password_with(&mut self, new_password: &str, kdf: KdfParams) -> Result<()> {
        check_password(new_password)?;
        let slot = password_slot(new_password, &self.data_key, kdf)?;
        let mut slots: Vec<Slot> = self.slots.iter().filter(|s| !matches!(s, Slot::Password { .. })).cloned().collect();
        slots.insert(0, slot);
        self.replace_slots(slots)
    }

    /// Save with new slots, twice, so the backup opens the same way as the
    /// main file. Otherwise a damaged main file right after the change would
    /// need the old way in to recover. On failure the old slots are kept.
    fn replace_slots(&mut self, slots: Vec<Slot>) -> Result<()> {
        let old = std::mem::replace(&mut self.slots, slots);
        if let Err(e) = self.save() {
            self.slots = old;
            return Err(e);
        }
        self.save()
    }

    pub fn has_hello(&self) -> bool {
        hello_request(&self.slots).is_some()
    }

    /// The Hello credential and challenge, if there is a Hello slot.
    pub fn hello(&self) -> Option<HelloRequest> {
        hello_request(&self.slots)
    }

    /// Add a Windows Hello slot (replacing any earlier one). The app creates
    /// the Hello credential, signs `challenge` (from [`new_hello_challenge`])
    /// and passes the signature here. The password slot stays.
    pub fn set_hello(&mut self, credential: &str, challenge: &[u8], signature: &[u8]) -> Result<()> {
        let slot_nonce = random::<24>();
        let wrapped_key = cipher(&hello_kek(signature))
            .encrypt(&slot_nonce.into(), Payload { msg: self.data_key.as_ref(), aad: HELLO_AAD })
            .map_err(|_| VaultError::Format("could not wrap the data key".into()))?;
        let mut slots: Vec<Slot> = self.slots.iter().filter(|s| !matches!(s, Slot::Hello { .. })).cloned().collect();
        slots.push(Slot::Hello {
            credential: credential.to_string(),
            challenge: challenge.to_vec(),
            nonce: slot_nonce.to_vec(),
            wrapped_key,
        });
        self.replace_slots(slots)
    }

    /// Remove the Windows Hello slot. The vault password still opens the vault.
    pub fn remove_hello(&mut self) -> Result<()> {
        let slots = self.slots.iter().filter(|s| !matches!(s, Slot::Hello { .. })).cloned().collect();
        self.replace_slots(slots)
    }

    /// The key for this vault's session logs.
    pub fn log_key(&self) -> LogKey {
        LogKey::from_data_key(&self.data_key)
    }

    /// This vault's session logs, in the folder beside it.
    pub fn logs(&self) -> LogStore {
        LogStore::new(logs_dir(&self.path), self.log_key())
    }

    /// "Vault backup: Export encrypted file". Writes the vault, encrypted as
    /// it is saved, to `dest`; it opens with the same password (and the same
    /// Windows Hello on this PC). Logs are not included.
    pub fn export_backup(&self, dest: &Path) -> Result<()> {
        let same = |a: &Path| {
            fs::canonicalize(a).ok().zip(fs::canonicalize(dest).ok()).is_some_and(|(a, b)| a == b) || a == dest
        };
        if same(&self.path) || same(&backup_path(&self.path)) {
            return Err(VaultError::AlreadyExists(dest.to_path_buf()));
        }
        let tmp = temp_path(dest);
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&self.encode()?)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, dest)?;
        Ok(())
    }

    /// Lock: wipe the key and contents, keeping only the path.
    pub fn lock(self) -> PathBuf {
        self.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holonet_model::{Auth, Host, Secret};

    fn new_vault(dir: &Path) -> UnlockedVault {
        UnlockedVault::create_with(&dir.join("vault.holonet"), "correct horse", KdfParams::insecure_for_tests())
            .unwrap()
    }

    #[test]
    fn round_trip_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault
            .update(|d| {
                let mut h = Host::new("pi.local");
                h.auth = Auth::Password { saved: Some(Secret::new("s3cret")) };
                d.hosts.push(h);
            })
            .unwrap();
        let path = vault.lock();

        let reopened = LockedVault::open(&path).unwrap().unlock("correct horse").unwrap();
        assert_eq!(reopened.data().hosts[0].address, "pi.local");
        assert!(!LockedVault::open(&path).unwrap().from_backup);
    }

    #[test]
    fn secrets_are_not_on_disk_in_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault
            .update(|d| {
                let mut h = Host::new("plain-host-name");
                h.auth = Auth::Password { saved: Some(Secret::new("s3cret-password")) };
                d.hosts.push(h);
            })
            .unwrap();
        let bytes = fs::read(vault.path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("s3cret-password"));
        assert!(!text.contains("plain-host-name"));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_vault(dir.path()).lock();
        assert!(matches!(LockedVault::open(&path).unwrap().unlock("wrong horse"), Err(VaultError::WrongPassword)));
    }

    #[test]
    fn short_passwords_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v");
        let err = UnlockedVault::create_with(&path, "1234567", KdfParams::insecure_for_tests());
        assert!(matches!(err, Err(VaultError::PasswordTooShort)));
        // Counted in characters, not bytes.
        assert!(UnlockedVault::create_with(&path, "ääääääää", KdfParams::insecure_for_tests()).is_ok());
    }

    #[test]
    fn create_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_vault(dir.path()).lock();
        let again = UnlockedVault::create_with(&path, "another one", KdfParams::insecure_for_tests());
        assert!(matches!(again, Err(VaultError::AlreadyExists(_))));
    }

    #[test]
    fn tampering_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let vault = new_vault(dir.path());
        let good = vault.encode().unwrap();
        let path = vault.lock();
        // Remove the backup so the tampered main file is the only candidate.
        let _ = fs::remove_file(backup_path(&path));

        // Flip a byte in the body.
        let mut body = good.clone();
        let last = body.len() - 1;
        body[last] ^= 1;
        fs::write(&path, &body).unwrap();
        assert!(matches!(LockedVault::open(&path).unwrap().unlock("correct horse"), Err(VaultError::Corrupt)));

        // Change the header (it's associated data): swap the body nonce.
        let (_, mut header, rest) = parse(&good).unwrap();
        header.body_nonce = vec![0; 24];
        let header_json = serde_json::to_vec(&header).unwrap();
        let mut forged = MAGIC.to_vec();
        forged.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        forged.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
        forged.extend_from_slice(&header_json);
        forged.extend_from_slice(rest);
        fs::write(&path, &forged).unwrap();
        assert!(matches!(LockedVault::open(&path).unwrap().unlock("correct horse"), Err(VaultError::Corrupt)));
    }

    #[test]
    fn newer_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let vault = new_vault(dir.path());
        let mut bytes = vault.encode().unwrap();
        bytes[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        let path = dir.path().join("newer.holonet");
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(LockedVault::open(&path), Err(VaultError::NewerFormat(_))));
    }

    #[test]
    fn falls_back_to_backup() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("first"))).unwrap();
        vault.update(|d| d.hosts.push(Host::new("second"))).unwrap();
        let path = vault.lock();

        // Main file damaged: the backup (one save older) opens.
        fs::write(&path, b"garbage").unwrap();
        let locked = LockedVault::open(&path).unwrap();
        assert!(locked.from_backup);
        assert_eq!(locked.unlock("correct horse").unwrap().data().hosts.len(), 1);

        // Main file missing (process died between the two renames): same.
        fs::remove_file(&path).unwrap();
        assert!(LockedVault::open(&path).unwrap().from_backup);
    }

    #[test]
    fn damaged_body_restores_from_backup() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("first"))).unwrap();
        vault.update(|d| d.hosts.push(Host::new("second"))).unwrap();
        let path = vault.lock();

        // Header still readable, body damaged.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&path, &bytes).unwrap();

        let locked = LockedVault::open(&path).unwrap();
        assert!(!locked.from_backup);
        let vault = locked.unlock("correct horse").unwrap();
        assert!(vault.restored_from_backup);
        assert_eq!(vault.data().hosts.len(), 1);
        // A wrong password is still reported as such, not as damage.
        assert!(matches!(locked.unlock("nope nope nope"), Err(VaultError::WrongPassword)));
    }

    #[test]
    fn change_password_keeps_contents() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("kept"))).unwrap();
        vault.change_password_with("brand new pass", KdfParams::insecure_for_tests()).unwrap();
        let path = vault.lock();
        let locked = LockedVault::open(&path).unwrap();
        assert!(matches!(locked.unlock("correct horse"), Err(VaultError::WrongPassword)));
        assert_eq!(locked.unlock("brand new pass").unwrap().data().hosts[0].address, "kept");
    }

    #[test]
    fn backup_opens_with_new_password_after_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("kept"))).unwrap();
        vault.change_password_with("brand new pass", KdfParams::insecure_for_tests()).unwrap();
        let path = vault.lock();
        // Damage the main file straight after the change.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&path, &bytes).unwrap();
        let vault = LockedVault::open(&path).unwrap().unlock("brand new pass").unwrap();
        assert!(vault.restored_from_backup);
        assert_eq!(vault.data().hosts[0].address, "kept");
    }

    #[test]
    fn kdf_params_are_read_from_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v");
        let custom = KdfParams { m_kib: 128, t: 2, p: 1 };
        UnlockedVault::create_with(&path, "correct horse", custom).unwrap();
        let bytes = fs::read(&path).unwrap();
        let (_, header, _) = parse(&bytes).unwrap();
        let Slot::Password { kdf, .. } = &header.slots[0] else { panic!("password slot first") };
        assert_eq!(*kdf, custom);
        assert!(LockedVault::open(&path).unwrap().unlock("correct horse").is_ok());
    }

    #[test]
    fn missing_vault_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(LockedVault::open(&dir.path().join("none")), Err(VaultError::NotFound(_))));
    }

    /// Stands in for Windows Hello: the same "signature" for the same
    /// challenge every time, different for another key.
    fn fake_sign(device_secret: &[u8], challenge: &[u8]) -> Vec<u8> {
        use sha2::Digest;
        let mut sig = sha2::Sha256::digest([device_secret, challenge].concat()).to_vec();
        sig.extend_from_slice(&sha2::Sha256::digest([challenge, device_secret].concat()));
        sig
    }

    #[test]
    fn hello_slot_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("pi.local"))).unwrap();
        assert!(!vault.has_hello());
        let challenge = new_hello_challenge();
        vault.set_hello("holonet-vault-test", &challenge, &fake_sign(b"this pc", &challenge)).unwrap();
        let path = vault.lock();

        let locked = LockedVault::open(&path).unwrap();
        let request = locked.hello().expect("Hello slot");
        assert_eq!(request, HelloRequest { credential: "holonet-vault-test".into(), challenge: challenge.clone() });
        // The right signature opens it; the password still does too.
        let opened = locked.unlock_with_hello(&fake_sign(b"this pc", &request.challenge)).unwrap();
        assert_eq!(opened.data().hosts[0].address, "pi.local");
        assert!(locked.unlock("correct horse").is_ok());
        // Another device's signature does not.
        assert!(matches!(
            locked.unlock_with_hello(&fake_sign(b"other pc", &request.challenge)),
            Err(VaultError::HelloRejected)
        ));
        // The backup opens with Hello as well (it was saved twice).
        let backup = parse(&fs::read(backup_path(&path)).unwrap()).map(|(_, h, _)| hello_request(&h.slots)).unwrap();
        assert_eq!(backup, Some(request));
        // Written as format 2.
        assert_eq!(u16::from_le_bytes(fs::read(&path).unwrap()[8..10].try_into().unwrap()), 2);
    }

    #[test]
    fn password_change_keeps_hello_and_removing_hello_keeps_password() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        let challenge = new_hello_challenge();
        let signature = fake_sign(b"this pc", &challenge);
        vault.set_hello("c", &challenge, &signature).unwrap();
        vault.change_password_with("new password", KdfParams::insecure_for_tests()).unwrap();
        assert!(vault.has_hello());
        let path = vault.path().to_path_buf();
        let locked = LockedVault::open(&path).unwrap();
        assert!(locked.unlock_with_hello(&signature).is_ok());
        assert!(matches!(locked.unlock("correct horse"), Err(VaultError::WrongPassword)));
        let mut vault = locked.unlock("new password").unwrap();
        vault.remove_hello().unwrap();
        let locked = LockedVault::open(&path).unwrap();
        assert!(locked.hello().is_none());
        assert!(matches!(locked.unlock_with_hello(&signature), Err(VaultError::NoHello)));
        assert!(locked.unlock("new password").is_ok());
        // Back to format 1 without a Hello slot.
        assert_eq!(u16::from_le_bytes(fs::read(&path).unwrap()[8..10].try_into().unwrap()), 1);
    }

    #[test]
    fn schema_1_data_is_migrated_on_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.schema = 1).unwrap();
        let path = vault.lock();
        let opened = LockedVault::open(&path).unwrap().unlock("correct horse").unwrap();
        assert_eq!(opened.data().schema, holonet_model::SCHEMA_VERSION);
    }

    #[test]
    fn backup_export_opens_with_the_same_password() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        vault.update(|d| d.hosts.push(Host::new("10.0.0.4"))).unwrap();
        let dest = dir.path().join("elsewhere").join("backup.holonet");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        vault.export_backup(&dest).unwrap();
        // Overwriting an earlier export is fine; the vault itself is not.
        vault.export_backup(&dest).unwrap();
        assert!(matches!(vault.export_backup(vault.path()), Err(VaultError::AlreadyExists(_))));
        let copy = LockedVault::open(&dest).unwrap().unlock("correct horse").unwrap();
        assert_eq!(copy.data(), vault.data());
        assert!(!dest.with_file_name("backup.holonet.tmp").exists());
    }

    #[test]
    fn logs_keep_their_key_across_a_password_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = new_vault(dir.path());
        assert_eq!(vault.logs().dir(), dir.path().join("vault.holonet.logs"));
        let id = holonet_model::RecordId::new();
        let mut log = vault.logs().create(id).unwrap();
        log.append(
            "hello
",
        )
        .unwrap();
        log.finish().unwrap();
        vault.change_password_with("battery staple", KdfParams::insecure_for_tests()).unwrap();
        let path = vault.lock();
        let reopened = LockedVault::open(&path).unwrap().unlock("battery staple").unwrap();
        assert_eq!(
            reopened.logs().read(id).unwrap().text,
            "hello
"
        );
        // A different vault in the same folder has its own logs and key.
        let other =
            UnlockedVault::create_with(&dir.path().join("b.holonet"), "correct horse", KdfParams::insecure_for_tests())
                .unwrap();
        assert_ne!(other.logs().dir(), reopened.logs().dir());
        assert!(LogStore::new(reopened.logs().dir().to_path_buf(), other.log_key()).read(id).is_err());
    }
}
