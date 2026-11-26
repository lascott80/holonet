//! Integration tests against the WSL test server (docs/milestone-1-plan.md).
//!
//! Skipped unless `HOLONET_TEST_SERVER=1`, so CI passes without the server:
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh --test wsl

use std::time::Duration;

use holonet_model::Secret;
mod support;

use holonet_ssh::{ConnectError, Credential, SecretRequest, Session};
use support::{FileKeyStatus, FilePrompts, MODES, load_env, options_from, root};

fn enabled() -> bool {
    let on = std::env::var("HOLONET_TEST_SERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set HOLONET_TEST_SERVER=1 to run against the WSL test server");
    }
    on
}

fn env() -> std::collections::HashMap<String, String> {
    load_env().expect(".dev/test-server.env")
}

#[tokio::test]
async fn every_sign_in_mode_connects() {
    if !enabled() {
        return;
    }
    for mode in MODES {
        let opts = options_from(mode, &env()).unwrap();
        let session = Session::connect(opts).await.unwrap_or_else(|e| panic!("{mode}: {e}"));
        let info = session.info();
        assert!(!info.cipher.is_empty() && !info.kex.is_empty(), "{mode}: negotiated names missing");
        assert_eq!(info.host_key_type, "ED25519");
        let expected = if mode.starts_with("password") { "password" } else { "publickey" };
        assert_eq!(info.auth_method, expected, "{mode}");
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn host_key_is_decided_before_any_credential() {
    if !enabled() {
        return;
    }
    let env = env();
    let dir = root().join("target/hostkey-check");
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("it_empty_known_hosts");
    std::fs::write(&empty, "").unwrap();
    let impostor =
        std::fs::read_to_string(root().join(&env["HOLONET_TEST_KEY_ED25519"]).with_extension("pub")).unwrap();
    let impostor: Vec<_> = impostor.split_whitespace().take(2).collect();
    let changed = dir.join("it_changed_known_hosts");
    std::fs::write(
        &changed,
        format!("[{}]:{} {}\n", env["HOLONET_TEST_HOST"], env["HOLONET_TEST_PORT"], impostor.join(" ")),
    )
    .unwrap();

    for (file, expected) in [(empty, FileKeyStatus::Unknown), (changed, FileKeyStatus::Changed)] {
        // Password mode with no saved password: if sign-in were attempted,
        // the prompt would be asked for one.
        let mut opts = options_from("password-ask", &env).unwrap();
        let prompts = FilePrompts::new(file);
        opts.prompts = prompts.clone();
        let result = Session::connect(opts).await;
        assert!(matches!(result, Err(ConnectError::HostKeyRejected)), "{expected:?}: {:?}", result.err());
        assert_eq!(*prompts.last_status.lock().unwrap(), Some(expected));
        assert!(prompts.requests.lock().unwrap().is_empty(), "a secret was requested before the key was accepted");
    }
}

#[tokio::test]
async fn wrong_password_is_refused_with_methods() {
    if !enabled() {
        return;
    }
    let mut opts = options_from("password", &env()).unwrap();
    opts.credential = Credential::Password(Some(Secret::new("definitely not it")));
    match Session::connect(opts).await {
        // The server allows keyboard-interactive too, so the fallback runs and is refused.
        Err(ConnectError::SignInRefused { method, remaining }) => {
            assert!(method == "password" || method == "keyboard-interactive", "{method}");
            assert!(remaining.iter().any(|m| m == "publickey"), "{remaining:?}");
        }
        other => panic!("expected SignInRefused, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn keyboard_interactive_fallback_signs_in() {
    if !enabled() {
        return;
    }
    let env = env();
    let mut opts = options_from("password", &env).unwrap();
    opts.user = env["HOLONET_TEST_KBD_USER"].clone();
    let session = Session::connect(opts).await.expect("keyboard-interactive sign-in");
    assert_eq!(session.info().auth_method, "keyboard-interactive");
}

#[tokio::test]
async fn passphrase_wrong_and_cancelled() {
    if !enabled() {
        return;
    }
    let env = env();
    // Wrong saved passphrase.
    let mut opts = options_from("passphrase", &env).unwrap();
    if let Credential::Key { passphrase, .. } = &mut opts.credential {
        *passphrase = Some(Secret::new("wrong passphrase"));
    }
    assert!(matches!(Session::connect(opts).await, Err(ConnectError::WrongPassphrase)));

    // Asked on connect, and the person cancels.
    let mut opts = options_from("passphrase-ask", &env).unwrap();
    let prompts = FilePrompts::new(root().join(".dev/known_hosts")); // no answers = cancel
    opts.prompts = prompts.clone();
    assert!(matches!(Session::connect(opts).await, Err(ConnectError::Cancelled)));
    assert!(matches!(prompts.requests.lock().unwrap()[0], SecretRequest::Passphrase { .. }));
}

#[tokio::test]
async fn network_failures_are_classified() {
    if !enabled() {
        return;
    }
    let env = env();
    let base = options_from("ed25519", &env).unwrap();

    let mut refused = base.clone();
    refused.port = 2299; // nothing listens here
    assert!(matches!(Session::connect(refused).await, Err(ConnectError::Refused)));

    let mut unknown = base.clone();
    unknown.host = "no-such-host.invalid".into();
    assert!(matches!(Session::connect(unknown).await, Err(ConnectError::Resolve { .. })));

    // A non-routable address never answers.
    let mut silent = base.clone();
    silent.host = "10.255.255.1".into();
    silent.connect_timeout = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let result = Session::connect(silent).await;
    assert!(matches!(result, Err(ConnectError::TimedOut(1)) | Err(ConnectError::Network(_))), "{:?}", result.err());
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn probe_returns_host_key_without_signing_in() {
    if !enabled() {
        return;
    }
    let env = env();
    let port: u16 = env["HOLONET_TEST_PORT"].parse().unwrap();
    let key = holonet_ssh::probe(&env["HOLONET_TEST_HOST"], port, Duration::from_secs(5)).await.unwrap();
    assert_eq!(key.key_type_label(), "ED25519");
    assert!(key.fingerprint.starts_with("SHA256:"));
    assert!(matches!(
        holonet_ssh::probe(&env["HOLONET_TEST_HOST"], 2299, Duration::from_secs(5)).await,
        Err(ConnectError::Refused)
    ));
}
