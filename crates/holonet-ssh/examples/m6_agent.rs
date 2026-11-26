//! Milestone 6 phase 0, spike (b): Windows' OpenSSH agent with russh 0.65.
//!
//!   cargo run -p holonet-ssh --example m6_agent --release
//!
//! Needs the `ssh-agent` service running. It:
//! - reports what connecting to a pipe that does not exist looks like, for
//!   the "agent is not running" message;
//! - adds the WSL test server's own Ed25519 key (`.dev/`) to the agent under
//!   the comment `holonet-m6-spike`, and lists the agent's keys;
//! - signs in to the test server with that key through the agent
//!   (`authenticate_publickey_with`), so Holonet never holds the key;
//! - asks for agent forwarding on a session channel and runs `ssh-add -l` on
//!   the host, whose agent channels are bridged to the same pipe;
//! - removes the key from the agent again.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use russh::client::{self, ChannelOpenHandle};
use russh::keys::agent::client::AgentClient;
use russh::keys::{PublicKeyOrCertificate, check_known_hosts_path, decode_secret_key};
use russh::{Channel, ChannelMsg};
use tokio::io::copy_bidirectional;
use tokio::net::windows::named_pipe::ClientOptions;

#[path = "../tests/support/mod.rs"]
mod support;

const PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
const COMMENT: &str = "holonet-m6-spike";

struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else { return Ok(false) };
        Ok(check_known_hosts_path(&self.host, self.port, key, &self.known_hosts).unwrap_or(false))
    }

    /// The host's `ssh-add` (or a further `ssh -A`) talks to this PC's agent.
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<client::Msg>,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        match ClientOptions::new().open(PIPE) {
            Ok(mut pipe) => {
                reply.accept().await;
                println!("  host opened an agent channel; bridged to {PIPE}");
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = copy_bidirectional(&mut stream, &mut pipe).await;
                });
            }
            Err(e) => {
                println!("  host opened an agent channel, but the pipe failed: {e}");
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match AgentClient::connect_named_pipe(r"\\.\pipe\holonet-no-such-agent").await {
        Ok(_) => println!("a missing pipe connected?"),
        Err(e) => println!("connecting to a pipe that does not exist: {e:?}"),
    }
    let mut agent = match AgentClient::connect_named_pipe(PIPE).await {
        Ok(agent) => agent,
        Err(e) => bail!("Windows' OpenSSH agent is not reachable at {PIPE}: {e:?}"),
    };
    let before = agent.request_identities().await?;
    println!("agent reachable; it holds {} key(s) before the spike", before.len());

    let env = support::load_env()?;
    let key_path = support::root().join(&env["HOLONET_TEST_KEY_ED25519"]);
    let mut key = decode_secret_key(&std::fs::read_to_string(&key_path)?, None)?;
    key.set_comment(COMMENT);
    let public = key.public_key().clone();
    agent.add_identity(&key, &[]).await.context("adding the test key")?;
    drop(key);
    let listed = agent.request_identities().await?;
    println!("added the test key; the agent lists:");
    for id in &listed {
        println!("  {} {}", id.public_key().algorithm(), id.comment());
    }
    ensure!(
        listed.iter().any(|id| id.public_key().key_data() == public.key_data()),
        "the agent does not list the test key"
    );

    let result = async {
        let host = env["HOLONET_TEST_HOST"].clone();
        let port: u16 = env["HOLONET_TEST_PORT"].parse()?;
        let handler = Handler { host: host.clone(), port, known_hosts: support::root().join(".dev/known_hosts") };
        let mut handle = client::connect(Default::default(), (host.as_str(), port), handler).await?;
        let auth = handle
            .authenticate_publickey_with(&env["HOLONET_TEST_USER"], public.clone(), None, &mut agent)
            .await
            .map_err(|e| anyhow::anyhow!("agent signing failed: {e:?}"))?;
        ensure!(auth.success(), "the server refused the agent's key");
        println!("signed in to {host}:{port} with the agent's key");

        let mut channel = handle.channel_open_session().await?;
        channel.agent_forward(true).await?;
        channel.exec(true, "echo SSH_AUTH_SOCK=$SSH_AUTH_SOCK; ssh-add -l").await?;
        let mut out = Vec::new();
        let mut status = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        println!("on the host (exit {status:?}):");
        for line in String::from_utf8_lossy(&out).lines() {
            println!("  {line}");
        }
        handle.disconnect(russh::Disconnect::ByApplication, "", "en").await?;
        anyhow::Ok(())
    }
    .await;

    agent.remove_identity(&public).await.context("removing the test key")?;
    let after = agent.request_identities().await?;
    println!("removed the test key; the agent holds {} key(s)", after.len());
    result
}
