//! Development helpers: connection options for the local WSL test server
//! described in docs/milestone-1-plan.md, read from `.dev/test-server.env`.
//! Not for production use; the vault replaces this in milestone 2.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{Auth, ConnectOptions, HostKeyCheck};

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

/// Sign-in for one of the test modes: password, ed25519, rsa or passphrase.
pub fn auth_for(mode: &str, env: &HashMap<String, String>) -> Result<Auth> {
    let key = |name: &str| get(env, name).map(|p| root().join(p));
    Ok(match mode {
        "password" => Auth::Password(get(env, "HOLONET_TEST_PASSWORD")?.clone()),
        "ed25519" => Auth::KeyFile { path: key("HOLONET_TEST_KEY_ED25519")?, passphrase: None },
        "rsa" => Auth::KeyFile { path: key("HOLONET_TEST_KEY_RSA")?, passphrase: None },
        "passphrase" => Auth::KeyFile {
            path: key("HOLONET_TEST_KEY_ED25519_PASS")?,
            passphrase: Some(get(env, "HOLONET_TEST_KEY_PASSPHRASE")?.clone()),
        },
        other => bail!("unknown sign-in mode {other}; use password, ed25519, rsa or passphrase"),
    })
}

/// Options for the test server, trusting keys in `.dev/known_hosts`.
pub fn options(mode: &str) -> Result<ConnectOptions> {
    let env = load_env()?;
    options_from(mode, &env)
}

pub fn options_from(mode: &str, env: &HashMap<String, String>) -> Result<ConnectOptions> {
    Ok(ConnectOptions {
        host: get(env, "HOLONET_TEST_HOST")?.clone(),
        port: get(env, "HOLONET_TEST_PORT")?.parse()?,
        user: get(env, "HOLONET_TEST_USER")?.clone(),
        auth: auth_for(mode, env)?,
        host_key: HostKeyCheck::KnownHostsFile(root().join(".dev/known_hosts")),
    })
}
