//! Windows Hello for the vault's Hello key slot (milestone 4 phase 0, spike
//! a): a Hello key credential signs the slot's challenge, and the vault turns
//! the signature into the key that unwraps its data key. Phase 0 showed the
//! signatures repeat on this PC and the prompt opens in front without help.
//!
//! Every call blocks until Windows answers (including the person's Hello
//! check), so callers run them off the UI thread.

/// Why a Hello request did not give a signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelloError {
    /// The person closed the Hello prompt, or chose the password instead.
    Cancelled,
    /// The credential named in the vault is not on this PC (removed, or the
    /// vault was copied from another PC).
    Missing,
    /// Hello is not set up on this PC.
    Unavailable,
    /// The security device is locked after too many tries, or another error.
    Failed(String),
}

impl HelloError {
    pub fn message(&self) -> String {
        match self {
            HelloError::Cancelled => "Windows Hello was cancelled.".into(),
            HelloError::Missing => {
                "This PC no longer has Windows Hello's key for this vault. Unlock with the vault password, then turn Windows Hello on again in Settings.".into()
            }
            HelloError::Unavailable => UNAVAILABLE.into(),
            HelloError::Failed(detail) => format!("Windows Hello did not finish: {detail}"),
        }
    }
}

/// Why the Settings switch is disabled when Hello cannot be used.
pub const UNAVAILABLE: &str = "Windows Hello is not set up on this PC. Add a PIN, fingerprint or face in Windows Settings, Accounts, Sign-in options.";

#[cfg(windows)]
mod imp {
    use super::HelloError;
    use windows::Security::Credentials::{
        KeyCredential, KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
    };
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::core::{Array, HSTRING};

    fn failed(e: windows::core::Error) -> HelloError {
        HelloError::Failed(e.message().to_string())
    }

    fn status(s: KeyCredentialStatus) -> Result<(), HelloError> {
        match s {
            KeyCredentialStatus::Success => Ok(()),
            KeyCredentialStatus::UserCanceled | KeyCredentialStatus::UserPrefersPassword => Err(HelloError::Cancelled),
            KeyCredentialStatus::NotFound => Err(HelloError::Missing),
            KeyCredentialStatus::SecurityDeviceLocked => {
                Err(HelloError::Failed("the security device is locked after too many tries".into()))
            }
            other => Err(HelloError::Failed(format!("status {}", other.0))),
        }
    }

    pub fn available() -> bool {
        KeyCredentialManager::IsSupportedAsync().and_then(|op| op.join()).unwrap_or(false)
    }

    fn sign_with(credential: &KeyCredential, challenge: &[u8]) -> Result<Vec<u8>, HelloError> {
        let data = CryptographicBuffer::CreateFromByteArray(challenge).map_err(failed)?;
        let result = credential.RequestSignAsync(&data).and_then(|op| op.join()).map_err(failed)?;
        status(result.Status().map_err(failed)?)?;
        let mut bytes = Array::<u8>::new();
        CryptographicBuffer::CopyToByteArray(&result.Result().map_err(failed)?, &mut bytes).map_err(failed)?;
        Ok(bytes.to_vec())
    }

    /// Create (or replace) the credential `name` and sign `challenge` with it.
    pub fn create_and_sign(name: &str, challenge: &[u8]) -> Result<Vec<u8>, HelloError> {
        if !available() {
            return Err(HelloError::Unavailable);
        }
        let name = HSTRING::from(name);
        let created = KeyCredentialManager::RequestCreateAsync(&name, KeyCredentialCreationOption::ReplaceExisting)
            .and_then(|op| op.join())
            .map_err(failed)?;
        status(created.Status().map_err(failed)?)?;
        // Sign through a freshly opened credential, as unlocking will, so a
        // signature that would not repeat shows up now rather than at unlock.
        let opened = KeyCredentialManager::OpenAsync(&name).and_then(|op| op.join()).map_err(failed)?;
        status(opened.Status().map_err(failed)?)?;
        sign_with(&opened.Credential().map_err(failed)?, challenge)
    }

    /// Sign `challenge` with the existing credential `name`.
    pub fn sign(name: &str, challenge: &[u8]) -> Result<Vec<u8>, HelloError> {
        if !available() {
            return Err(HelloError::Unavailable);
        }
        let opened = KeyCredentialManager::OpenAsync(&HSTRING::from(name)).and_then(|op| op.join()).map_err(failed)?;
        status(opened.Status().map_err(failed)?)?;
        sign_with(&opened.Credential().map_err(failed)?, challenge)
    }

    pub fn delete(name: &str) {
        let _ = KeyCredentialManager::DeleteAsync(&HSTRING::from(name)).and_then(|op| op.join());
    }
}

#[cfg(not(windows))]
mod imp {
    use super::HelloError;
    pub fn available() -> bool {
        false
    }
    pub fn create_and_sign(_: &str, _: &[u8]) -> Result<Vec<u8>, HelloError> {
        Err(HelloError::Unavailable)
    }
    pub fn sign(_: &str, _: &[u8]) -> Result<Vec<u8>, HelloError> {
        Err(HelloError::Unavailable)
    }
    pub fn delete(_: &str) {}
}

pub use imp::{available, create_and_sign, delete, sign};

/// A new credential name for a vault's Hello key.
pub fn new_credential_name() -> String {
    let challenge = holonet_vault::new_hello_challenge();
    let id: String = challenge.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("holonet-vault-{id}")
}
