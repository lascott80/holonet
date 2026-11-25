//! Test support for the local WSL test server described in
//! docs/milestone-1-plan.md, read from `.dev/test-server.env`. Shared by the
//! integration tests and the `shell` example; not part of the library.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use holonet_model::Secret;
use russh::keys::{PublicKey, check_known_hosts_path};

use holonet_ssh::{BoxFuture, ConnectOptions, Credential, HostKeyDecision, PresentedKey, Prompts, SecretRequest};

/// Workspace root, as known at compile time.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn load_env() -> Result<HashMap<String, String>> {
    let path = root().join(".dev/test-server.env");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect())
}

fn get<'a>(env: &'a HashMap<String, String>, name: &str) -> Result<&'a String> {
    env.get(name).with_context(|| format!("{name} missing from .dev/test-server.env"))
}

/// What [`FilePrompts`] decided about the host key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileKeyStatus {
    Trusted,
    Unknown,
    Changed,
}

/// Prompts backed by an OpenSSH `known_hosts` file: only keys in the file are
/// accepted. Secrets come from a fixed list of answers (empty = cancel).
pub struct FilePrompts {
    known_hosts: PathBuf,
    answers: Mutex<Vec<Secret>>,
    /// Every request made, for tests to inspect.
    pub requests: Mutex<Vec<SecretRequest>>,
    pub last_status: Mutex<Option<FileKeyStatus>>,
}

impl FilePrompts {
    pub fn new(known_hosts: impl Into<PathBuf>) -> Arc<Self> {
        Self::with_answers(known_hosts, Vec::new())
    }

    /// `answers` are given, in order, to secret requests.
    pub fn with_answers(known_hosts: impl Into<PathBuf>, answers: Vec<Secret>) -> Arc<Self> {
        let mut answers = answers;
        answers.reverse();
        Arc::new(Self {
            known_hosts: known_hosts.into(),
            answers: Mutex::new(answers),
            requests: Mutex::new(Vec::new()),
            last_status: Mutex::new(None),
        })
    }
}

impl Prompts for FilePrompts {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        let status = match PublicKey::from_openssh(&key.public_key) {
            Ok(public) => match check_known_hosts_path(&key.host, key.port, &public, &self.known_hosts) {
                Ok(true) => FileKeyStatus::Trusted,
                Err(russh::keys::Error::KeyChanged { .. }) => FileKeyStatus::Changed,
                _ => FileKeyStatus::Unknown,
            },
            Err(_) => FileKeyStatus::Unknown,
        };
        let decision = if status == FileKeyStatus::Trusted { HostKeyDecision::Continue } else { HostKeyDecision::Stop };
        *self.last_status.lock().unwrap() = Some(status);
        Box::pin(async move { decision })
    }

    fn secret(&self, request: SecretRequest) -> BoxFuture<Option<Secret>> {
        self.requests.lock().unwrap().push(request);
        let answer = self.answers.lock().unwrap().pop();
        Box::pin(async move { answer })
    }
}

/// Sign-in modes for the test server:
/// `password`, `password-ask`, `ed25519`, `rsa`, `passphrase`, `passphrase-ask`.
pub const MODES: &[&str] = &["password", "password-ask", "ed25519", "rsa", "passphrase", "passphrase-ask"];

fn read_key(env: &HashMap<String, String>, var: &str) -> Result<(String, Secret)> {
    let path = root().join(get(env, var)?);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((name, Secret::new(text)))
}

/// Credential for a mode, and the answers its prompts should get.
pub fn credential_for(mode: &str, env: &HashMap<String, String>) -> Result<(Credential, Vec<Secret>)> {
    let password = || get(env, "HOLONET_TEST_PASSWORD").map(|p| Secret::new(p.as_str()));
    let passphrase = || get(env, "HOLONET_TEST_KEY_PASSPHRASE").map(|p| Secret::new(p.as_str()));
    Ok(match mode {
        "password" => (Credential::Password(Some(password()?)), vec![]),
        "password-ask" => (Credential::Password(None), vec![password()?]),
        "ed25519" | "rsa" => {
            let var = if mode == "rsa" { "HOLONET_TEST_KEY_RSA" } else { "HOLONET_TEST_KEY_ED25519" };
            let (name, private_key) = read_key(env, var)?;
            (Credential::Key { name, private_key, passphrase: None }, vec![])
        }
        "passphrase" | "passphrase-ask" => {
            let (name, private_key) = read_key(env, "HOLONET_TEST_KEY_ED25519_PASS")?;
            if mode == "passphrase" {
                (Credential::Key { name, private_key, passphrase: Some(passphrase()?) }, vec![])
            } else {
                (Credential::Key { name, private_key, passphrase: None }, vec![passphrase()?])
            }
        }
        other => bail!("unknown sign-in mode {other}; use one of {}", MODES.join(", ")),
    })
}

/// Options for the test server, trusting keys in `.dev/known_hosts`.
pub fn options(mode: &str) -> Result<ConnectOptions> {
    let env = load_env()?;
    options_from(mode, &env)
}

pub fn options_from(mode: &str, env: &HashMap<String, String>) -> Result<ConnectOptions> {
    let (credential, answers) = credential_for(mode, env)?;
    Ok(ConnectOptions {
        host: get(env, "HOLONET_TEST_HOST")?.clone(),
        port: get(env, "HOLONET_TEST_PORT")?.parse()?,
        user: get(env, "HOLONET_TEST_USER")?.clone(),
        credential,
        connect_timeout: Duration::from_secs(10),
        keep_alive: Some(holonet_ssh::KEEPALIVE),
        prompts: FilePrompts::with_answers(root().join(".dev/known_hosts"), answers),
    })
}
