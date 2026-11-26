//! Milestone 4 phase 0, spike (e): a jump host built from a `direct-tcpip`
//! channel. Signs in to the WSL test server, opens a channel from it to
//! `<target>` (default the same server, 127.0.0.1:2222), runs a second SSH
//! session over that channel, and runs a command there.
//!
//!   cargo run -p holonet-ssh --example m4_jump -- [target-host] [target-port]
//!
//! Both host keys are checked against `.dev/known_hosts`, each with its own
//! address. Nothing is trusted automatically.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use russh::client::{self, Handle};
use russh::keys::{PublicKeyOrCertificate, check_known_hosts_path};
use russh::{ChannelMsg, Disconnect};

#[path = "../tests/support/mod.rs"]
mod support;

struct Checked {
    host: String,
    port: u16,
}

impl client::Handler for Checked {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else { return Ok(false) };
        let path = support::root().join(".dev/known_hosts");
        let ok = check_known_hosts_path(&self.host, self.port, key, &path).unwrap_or(false);
        println!(
            "  host key for {}:{} {}",
            self.host,
            self.port,
            if ok { "matches known_hosts" } else { "NOT trusted, stopping" }
        );
        Ok(ok)
    }
}

async fn sign_in<S: client::Handler>(handle: &mut Handle<S>, user: &str, password: &str) -> Result<()> {
    let auth = handle.authenticate_password(user, password).await?;
    if !auth.success() {
        bail!("password refused");
    }
    Ok(())
}

async fn run(handle: &Handle<Checked>, command: &str) -> Result<String> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut out = Vec::new();
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.extend_from_slice(&data),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = support::load_env()?;
    let host = env.get("HOLONET_TEST_HOST").context("host")?.clone();
    let port: u16 = env.get("HOLONET_TEST_PORT").context("port")?.parse()?;
    let user = env.get("HOLONET_TEST_USER").context("user")?.clone();
    let password = env.get("HOLONET_TEST_PASSWORD").context("password")?.clone();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = args.first().cloned().unwrap_or_else(|| "127.0.0.1".into());
    let target_port: u16 = args.get(1).map(|p| p.parse()).transpose()?.unwrap_or(2222);

    let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), ..Default::default() });

    let t = Instant::now();
    println!("jump host {host}:{port}");
    let mut outer =
        client::connect(config.clone(), (host.as_str(), port), Checked { host: host.clone(), port }).await?;
    sign_in(&mut outer, &user, &password).await?;
    println!("  signed in after {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    println!("target {target}:{target_port}, through the jump host");
    let channel = outer.channel_open_direct_tcpip(target.as_str(), target_port as u32, "127.0.0.1", 0).await?;
    let stream = channel.into_stream();
    let mut inner = client::connect_stream(config, stream, Checked { host: target.clone(), port: target_port }).await?;
    sign_in(&mut inner, &user, &password).await?;
    println!("  signed in after {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    println!("  outer sees: {}", run(&outer, "echo $SSH_CONNECTION").await?);
    println!("  inner sees: {}", run(&inner, "echo $SSH_CONNECTION").await?);

    // Close inner first, then outer.
    inner.disconnect(Disconnect::ByApplication, "", "en").await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("  inner closed: {}, outer closed: {}", inner.is_closed(), outer.is_closed());
    outer.disconnect(Disconnect::ByApplication, "", "en").await?;
    Ok(())
}
