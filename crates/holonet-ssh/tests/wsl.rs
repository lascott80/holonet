//! Integration tests against the WSL test server (docs/milestone-1-plan.md).
//!
//! Skipped unless `HOLONET_TEST_SERVER=1`, so CI passes without the server:
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh --test wsl

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use holonet_model::Secret;
mod support;

use holonet_ssh::{
    BoxFuture, ConnectError, Credential, HostKeyDecision, LostReason, PresentedKey, Prompts, PtySize, SecretRequest,
    Session, Shell, ShellEvent, ShellInput,
};
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

/// How a shell ended, collected until the shell task stops (or 20 s pass).
#[derive(Debug, PartialEq)]
enum End {
    ExitThenClosed(u32),
    Closed,
    Lost(LostReason),
    Timeout,
}

async fn wait_for_end(shell: &mut Shell) -> End {
    let mut exit = None;
    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return End::Timeout,
            event = shell.events.recv() => match event {
                Some(ShellEvent::Output(_)) => {}
                Some(ShellEvent::Exit(code)) => exit = Some(code),
                Some(ShellEvent::Closed) | None => {
                    return exit.map_or(End::Closed, End::ExitThenClosed);
                }
                Some(ShellEvent::Lost(reason)) => return End::Lost(reason),
            },
        }
    }
}

fn type_line(shell: &Shell, line: &str) {
    shell.input.send(ShellInput::Data(format!("{line}\n").into_bytes())).unwrap();
}

#[tokio::test]
async fn shell_exit_is_not_a_lost_connection() {
    if !enabled() {
        return;
    }
    let session = Session::connect(options_from("ed25519", &env()).unwrap()).await.unwrap();
    let mut shell = session.open_shell(PtySize::cells(80, 24)).await.unwrap();
    type_line(&shell, "exit 3");
    assert_eq!(wait_for_end(&mut shell).await, End::ExitThenClosed(3));
    assert_eq!(session.lost_reason(), None);
}

#[tokio::test]
async fn killed_connection_is_lost() {
    if !enabled() {
        return;
    }
    let session = Session::connect(options_from("ed25519", &env()).unwrap()).await.unwrap();
    let mut shell = session.open_shell(PtySize::cells(80, 24)).await.unwrap();
    // The login shell's parent is this connection's sshd process: killing it
    // drops the TCP connection with no SSH goodbye, like a crashed server.
    type_line(&shell, "kill -9 $PPID");
    match wait_for_end(&mut shell).await {
        End::Lost(reason @ (LostReason::Network(_) | LostReason::Other(_))) => eprintln!("lost: {reason}"),
        other => panic!("expected a lost connection, got {other:?}"),
    }
}

/// Accepts any host key: the keep-alive test connects through a local proxy,
/// whose address is not in `.dev/known_hosts`.
struct TrustAny;

impl Prompts for TrustAny {
    fn host_key(&self, _key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        Box::pin(async { HostKeyDecision::Continue })
    }
    fn secret(&self, _request: SecretRequest) -> BoxFuture<Option<Secret>> {
        Box::pin(async { None })
    }
}

/// A one-connection TCP proxy that can stop forwarding while keeping the
/// socket open, as a sleeping PC or a dead route looks to the client.
async fn freezable_proxy(target: (String, u16)) -> (u16, Arc<AtomicBool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let frozen = Arc::new(AtomicBool::new(false));
    let f = frozen.clone();
    tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let server = tokio::net::TcpStream::connect(target).await.unwrap();
        let (cr, cw) = client.into_split();
        let (sr, sw) = server.into_split();
        tokio::spawn(pump(cr, sw, f.clone()));
        tokio::spawn(pump(sr, cw, f));
    });
    (port, frozen)
}

async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, frozen: Arc<AtomicBool>) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if frozen.load(Ordering::Relaxed) {
            // Swallow everything from now on, without closing either socket.
            std::future::pending::<()>().await;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

#[tokio::test]
async fn unanswered_keep_alives_are_lost() {
    if !enabled() {
        return;
    }
    let env = env();
    let (port, frozen) =
        freezable_proxy((env["HOLONET_TEST_HOST"].clone(), env["HOLONET_TEST_PORT"].parse().unwrap())).await;
    let mut opts = options_from("ed25519", &env).unwrap();
    opts.host = "127.0.0.1".into();
    opts.port = port;
    opts.prompts = Arc::new(TrustAny);
    opts.keep_alive = Some(Duration::from_secs(1));
    let session = Session::connect(opts).await.unwrap();
    let mut shell = session.open_shell(PtySize::cells(80, 24)).await.unwrap();
    // Let a few keep-alives succeed first.
    tokio::time::sleep(Duration::from_secs(3)).await;
    frozen.store(true, Ordering::Relaxed);
    let started = std::time::Instant::now();
    assert_eq!(wait_for_end(&mut shell).await, End::Lost(LostReason::NoResponse));
    // Three missed keep-alives at 1 s, give or take a tick.
    let waited = started.elapsed();
    assert!(waited >= Duration::from_secs(2) && waited < Duration::from_secs(8), "{waited:?}");
}
