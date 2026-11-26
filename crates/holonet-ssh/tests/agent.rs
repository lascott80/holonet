//! Sign-in through Windows' OpenSSH agent, agent forwarding and compression
//! against the WSL test server (milestone 6, phase 8).
//!
//! Skipped unless `HOLONET_TEST_SERVER=1`, and needs the `ssh-agent` service
//! running (the owner starts it; Holonet never changes services):
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh --test agent
//!
//! The test adds the test server's Ed25519 key from `.dev/` to the agent and
//! removes it again at the end, whatever happened in between. It is one test
//! so that nothing else changes the agent while it runs.

use std::time::Duration;

use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, decode_secret_key};

mod support;

use holonet_ssh::{AGENT_PIPE, ConnectError, Credential, PtySize, Session, ShellEvent, ShellInput};
use support::{load_env, options_from, root};

fn enabled() -> bool {
    let on = std::env::var("HOLONET_TEST_SERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set HOLONET_TEST_SERVER=1 to run against the WSL test server");
    }
    on
}

/// Run `command; exit` in a login shell and return what it printed.
async fn shell_output(session: &Session, command: &str) -> String {
    let size = PtySize { cols: 200, rows: 40, pixel_width: 0, pixel_height: 0 };
    let mut shell = session.open_shell(size).await.unwrap();
    shell.input.send(ShellInput::Data(format!("{command}; exit\r").into_bytes())).unwrap();
    let mut out = Vec::new();
    let read = async {
        while let Some(event) = shell.events.recv().await {
            match event {
                ShellEvent::Output(bytes) => out.extend_from_slice(&bytes),
                ShellEvent::Closed | ShellEvent::Lost(_) => break,
                ShellEvent::Exit(_) => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), read).await.expect("the shell did not finish");
    String::from_utf8_lossy(&out).into_owned()
}

fn agent_options(forwarding: bool, compression: bool) -> holonet_ssh::ConnectOptions {
    let mut opts = options_from("ed25519", &load_env().unwrap()).unwrap();
    opts.credential = Credential::Agent;
    opts.agent_forwarding = forwarding;
    opts.compression = compression;
    opts
}

#[tokio::test]
async fn agent_sign_in_forwarding_and_compression() {
    if !enabled() {
        return;
    }
    let env = load_env().unwrap();
    let mut agent = AgentClient::connect_named_pipe(AGENT_PIPE)
        .await
        .expect("Windows' OpenSSH agent is not running: start the ssh-agent service");
    let before = agent.request_identities().await.unwrap().len();

    // With nothing in the agent, sign-in says so (only checked when the
    // agent starts empty; the test never removes keys it did not add).
    if before == 0 {
        match Session::connect(agent_options(false, false)).await {
            Err(ConnectError::AgentEmpty) => {}
            other => panic!("expected AgentEmpty, got {:?}", other.err()),
        }
    }

    let key_path = root().join(&env["HOLONET_TEST_KEY_ED25519"]);
    let key = decode_secret_key(&std::fs::read_to_string(&key_path).unwrap(), None).unwrap();
    let public = key.public_key().clone();
    let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
    agent.add_identity(&key, &[]).await.expect("adding the test key to the agent");
    drop(key);

    let result = tokio::spawn(async move {
        // Signs in with the agent's key, compression on, forwarding on.
        let session = Session::connect(agent_options(true, true)).await.expect("agent sign-in");
        assert_eq!(session.info().auth_method, "agent");
        assert_eq!(session.info().compression, "zlib@openssh.com");
        let listed = shell_output(&session, "ssh-add -l").await;
        assert!(listed.contains(&fingerprint), "the host's ssh-add -l did not list the key:\n{listed}");
        let _ = session.close().await;

        // Forwarding off: the host has no agent. Compression off: none.
        let session = Session::connect(agent_options(false, false)).await.expect("agent sign-in");
        assert_eq!(session.info().compression, "none");
        let listed = shell_output(&session, "ssh-add -l; echo status=$?").await;
        assert!(!listed.contains(&fingerprint), "the key was forwarded without asking:\n{listed}");
        assert!(listed.contains("status=2"), "ssh-add should find no agent:\n{listed}");
        let _ = session.close().await;

        // OpenSSH makes ~/.ssh/agent/ for forwarded sockets; leave the home as it was.
        let session = Session::connect(agent_options(false, false)).await.unwrap();
        let _ = session.exec("rmdir ~/.ssh/agent 2>/dev/null; true").await;
        let _ = session.close().await;
    })
    .await;

    // Always take the test key out of the agent again.
    agent.remove_identity(&public).await.expect("removing the test key from the agent");
    let after = agent.request_identities().await.unwrap();
    assert!(
        after.iter().all(|id| id.public_key().key_data() != public.key_data()),
        "the test key is still in the agent"
    );
    assert_eq!(after.len(), before);
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}
