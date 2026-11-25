//! SSH sessions for Holonet: sign-in, PTY shells, and (later) host keys,
//! forwarding and SFTP. Builds and tests without a window.
//!
//! A [`Shell`] is a pair of channels so the interface never touches russh:
//! send [`ShellInput`] in, receive [`ShellEvent`] out. The output channel is
//! bounded, so a slow reader pushes back on the SSH window instead of
//! buffering without limit.

pub mod dev;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use russh::client;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts_path, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;

/// Keep-alive interval from docs/decisions.md.
const KEEPALIVE: Duration = Duration::from_secs(30);
/// Output chunks buffered before the reader applies back-pressure.
const OUTPUT_BUFFER: usize = 256;

/// How to sign in.
#[derive(Clone)]
pub enum Auth {
    Password(String),
    KeyFile { path: PathBuf, passphrase: Option<String> },
}

/// How to check the server's host key.
#[derive(Clone)]
pub enum HostKeyCheck {
    /// Accept only keys recorded in this OpenSSH `known_hosts` file.
    /// Unknown keys and changed keys both stop the connection.
    KnownHostsFile(PathBuf),
}

#[derive(Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub host_key: HostKeyCheck,
}

/// Terminal size in cells, plus optional pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl PtySize {
    pub fn cells(cols: u16, rows: u16) -> Self {
        Self { cols, rows, pixel_width: 0, pixel_height: 0 }
    }
}

/// Outcome of the host key check, kept for a clear error message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    Trusted,
    Unknown { fingerprint: String },
    Changed { fingerprint: String, line: usize },
}

struct Handler {
    host: String,
    port: u16,
    check: HostKeyCheck,
    status: Arc<Mutex<Option<HostKeyStatus>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // Certificates are not trusted in the prototype.
            return Ok(false);
        };
        let fingerprint = key.fingerprint(Default::default()).to_string();
        let HostKeyCheck::KnownHostsFile(path) = &self.check;
        let status = match check_known_hosts_path(&self.host, self.port, key, path) {
            Ok(true) => HostKeyStatus::Trusted,
            Ok(false) => HostKeyStatus::Unknown { fingerprint },
            Err(russh::keys::Error::KeyChanged { line }) => HostKeyStatus::Changed { fingerprint, line },
            // A missing file means nothing is trusted yet.
            Err(_) => HostKeyStatus::Unknown { fingerprint },
        };
        let trusted = status == HostKeyStatus::Trusted;
        *self.status.lock().unwrap() = Some(status);
        Ok(trusted)
    }
}

/// A signed-in SSH connection.
pub struct Session {
    handle: client::Handle<Handler>,
}

impl Session {
    pub async fn connect(opts: ConnectOptions) -> Result<Self> {
        let config = Arc::new(client::Config { keepalive_interval: Some(KEEPALIVE), ..Default::default() });
        let status = Arc::new(Mutex::new(None));
        let handler =
            Handler { host: opts.host.clone(), port: opts.port, check: opts.host_key.clone(), status: status.clone() };

        let connected = client::connect(config, (opts.host.as_str(), opts.port), handler).await;
        let mut handle = match connected {
            Ok(handle) => handle,
            Err(err) => {
                return Err(match status.lock().unwrap().clone() {
                    Some(HostKeyStatus::Unknown { fingerprint }) => {
                        anyhow!("unknown host key for {}:{} ({fingerprint}); not in known_hosts", opts.host, opts.port)
                    }
                    Some(HostKeyStatus::Changed { fingerprint, line }) => anyhow!(
                        "HOST KEY CHANGED for {}:{} (now {fingerprint}, known_hosts line {line}); connection stopped",
                        opts.host,
                        opts.port
                    ),
                    _ => anyhow::Error::new(err).context(format!("connecting to {}:{}", opts.host, opts.port)),
                });
            }
        };

        let result = match &opts.auth {
            Auth::Password(password) => {
                handle.authenticate_password(&opts.user, password).await.context("password sign-in")?
            }
            Auth::KeyFile { path, passphrase } => {
                let key = load_secret_key(path, passphrase.as_deref())
                    .with_context(|| format!("loading key {}", path.display()))?;
                let hash = handle.best_supported_rsa_hash().await?.flatten();
                handle
                    .authenticate_publickey(&opts.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await
                    .context("key sign-in")?
            }
        };
        if !result.success() {
            bail!("sign-in refused for {}", opts.user);
        }
        Ok(Self { handle })
    }

    /// Open a PTY and a login shell. The shell runs on a Tokio task until it
    /// exits or [`ShellInput::Close`] is sent.
    pub async fn open_shell(&self, size: PtySize) -> Result<Shell> {
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(
                true,
                "xterm-256color",
                size.cols.into(),
                size.rows.into(),
                size.pixel_width.into(),
                size.pixel_height.into(),
                &[],
            )
            .await?;
        channel.request_shell(true).await?;

        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<ShellInput>();
        let (event_tx, event_rx) = mpsc::channel::<ShellEvent>(OUTPUT_BUFFER);

        tokio::spawn(async move {
            let mut channel = channel;
            let mut input_open = true;
            loop {
                tokio::select! {
                    input = input_rx.recv(), if input_open => match input {
                        Some(ShellInput::Data(bytes)) => {
                            if channel.data(&bytes[..]).await.is_err() { break; }
                        }
                        Some(ShellInput::Resize(s)) => {
                            let _ = channel
                                .window_change(s.cols.into(), s.rows.into(), s.pixel_width.into(), s.pixel_height.into())
                                .await;
                        }
                        Some(ShellInput::Close) | None => {
                            input_open = false;
                            let _ = channel.eof().await;
                            let _ = channel.close().await;
                        }
                    },
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if event_tx.send(ShellEvent::Output(data.to_vec())).await.is_err() { break; }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            let _ = event_tx.send(ShellEvent::Exit(exit_status)).await;
                        }
                        Some(_) => {}
                        None => break,
                    },
                }
            }
            let _ = event_tx.send(ShellEvent::Closed).await;
        });

        Ok(Shell { input: input_tx, events: event_rx })
    }

    pub async fn close(&self) -> Result<()> {
        self.handle.disconnect(Disconnect::ByApplication, "", "en").await?;
        Ok(())
    }
}

pub enum ShellInput {
    Data(Vec<u8>),
    Resize(PtySize),
    Close,
}

#[derive(Debug)]
pub enum ShellEvent {
    Output(Vec<u8>),
    Exit(u32),
    Closed,
}

/// A running remote shell.
pub struct Shell {
    pub input: mpsc::UnboundedSender<ShellInput>,
    pub events: mpsc::Receiver<ShellEvent>,
}
