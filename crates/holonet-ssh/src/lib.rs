//! SSH sessions for Holonet: connecting, the host key decision, sign-in,
//! PTY shells, the SFTP subsystem and port forwarding ([`forward`]). Builds
//! and tests without a window.
//!
//! The interface takes part through [`Prompts`] (host key decisions and
//! secrets that are not saved). A [`Shell`] is a pair of channels, so the
//! interface never touches russh: send [`ShellInput`] in, receive
//! [`ShellEvent`] out. The output channel is bounded, so a slow reader pushes
//! back on the SSH window instead of buffering without limit.
//!
//! A shell ends one of two ways: it exits ([`ShellEvent::Exit`] then
//! [`ShellEvent::Closed`]), or the connection under it is lost
//! ([`ShellEvent::Lost`] with a [`LostReason`]). Only a lost connection is a
//! reason to reconnect.
//!
//! A session can run through jump hosts ([`ConnectOptions::jump`]): each hop
//! is its own signed-in session, and the next one runs over a `direct-tcpip`
//! channel from it. A lost jump host is reported as
//! [`LostReason::JumpHost`].

mod connect;
pub mod forward;
pub mod hostkey;
pub mod keys;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;

pub use connect::{
    BoxFuture, ConnectError, ConnectOptions, Credential, HostKeyDecision, KEEPALIVE, LostReason, Prompts,
    SecretRequest, SessionInfo, probe,
};
pub use forward::{Forward, ForwardError, ForwardSpec, ForwardStatus};
pub use hostkey::{KeyStatus, PresentedKey, classify, fingerprint};
pub use keys::{
    GeneratedKey, KeyFileError, KeyInfo, KeyKind, algorithm_label, convert_ppk, generate_key, inspect_private_key,
};
pub use russh_sftp;

/// Output chunks buffered before the reader applies back-pressure.
const OUTPUT_BUFFER: usize = 256;

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

/// A signed-in SSH connection.
pub struct Session {
    /// Shared with the forwards running on this session.
    handle: Arc<connect::Handle>,
    /// Remote forwards' destinations, shared with the session's handler.
    routes: forward::Routes,
    info: SessionInfo,
    lost: Arc<Mutex<Option<LostReason>>>,
    /// The address as connected to, for messages about this hop.
    host: String,
    /// The jump host this session runs through. Kept open while this one is.
    via: Option<Box<Session>>,
}

/// One hop's lost-connection record, for the shell's watcher.
type LostWatch = (String, Arc<Mutex<Option<LostReason>>>);

/// What [`Session::install_public_key`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Added,
    AlreadyPresent,
}

/// A finished remote command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_status: Option<u32>,
    pub stdout: String,
    pub stderr: String,
}

impl Session {
    /// Connect (through the jump hosts, if any), decide on each host key
    /// through `opts.prompts`, and sign in.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ConnectError> {
        let host = opts.host.clone();
        let c = connect::connect(&opts).await?;
        Ok(Self { handle: Arc::new(c.handle), routes: c.routes, info: c.info, lost: c.lost, host, via: c.via })
    }

    /// The jump host session this one runs through, if any.
    pub fn jump_host(&self) -> Option<&Session> {
        self.via.as_deref()
    }

    /// Every hop's record, outermost jump host first, this session last.
    fn lost_watches(&self) -> Vec<LostWatch> {
        let mut watches = self.via.as_ref().map(|v| v.lost_watches()).unwrap_or_default();
        watches.push((self.host.clone(), self.lost.clone()));
        watches
    }

    /// Start a port forward over this session. The port is bound (or the
    /// host has agreed to listen) when this returns; the forward runs until
    /// it is stopped or dropped, or the connection is lost.
    pub async fn forward(&self, spec: ForwardSpec) -> Result<Forward, ForwardError> {
        forward::start(self.handle.clone(), self.routes.clone(), spec).await
    }

    /// Open the `sftp` subsystem on a new channel of this session.
    pub async fn open_sftp(&self) -> Result<russh_sftp::client::SftpSession> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(russh_sftp::client::SftpSession::new(channel.into_stream()).await?)
    }

    /// Run a command and collect its output (Install on host).
    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        let (mut stdout, mut stderr, mut exit_status) = (Vec::new(), Vec::new(), None);
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status: s } => exit_status = Some(s),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(ExecOutput {
            exit_status,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }

    /// Add an OpenSSH public key line to `~/.ssh/authorized_keys` on this
    /// host, creating the folder (mode 700) and file (mode 600) if needed.
    /// Existing lines are never changed; a key already there (by type and
    /// key, whatever its comment) is left alone.
    pub async fn install_public_key(&self, public_key: &str) -> Result<InstallOutcome> {
        let line = public_key.trim();
        let mut parts = line.split_whitespace();
        let (Some(kind), Some(body)) = (parts.next(), parts.next()) else {
            anyhow::bail!("not an OpenSSH public key line");
        };
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let command = format!(
            "umask 077; mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && \
             chmod 600 ~/.ssh/authorized_keys && \
             if grep -qF {needle} ~/.ssh/authorized_keys; then echo HOLONET_PRESENT; else \
             if [ -s ~/.ssh/authorized_keys ] && [ -n \"$(tail -c 1 ~/.ssh/authorized_keys)\" ]; then echo >> ~/.ssh/authorized_keys; fi; \
             printf '%s\\n' {line} >> ~/.ssh/authorized_keys && echo HOLONET_ADDED; fi",
            needle = quote(&format!("{kind} {body}")),
            line = quote(line),
        );
        let out = self.exec(&command).await?;
        if out.stdout.contains("HOLONET_PRESENT") {
            Ok(InstallOutcome::AlreadyPresent)
        } else if out.stdout.contains("HOLONET_ADDED") {
            Ok(InstallOutcome::Added)
        } else {
            let detail = out.stderr.trim();
            anyhow::bail!(if detail.is_empty() {
                "the server did not confirm the change".to_string()
            } else {
                detail.to_string()
            })
        }
    }

    /// Negotiated algorithms, host key and sign-in method.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Why the connection ended, if it has ended other than by [`Session::close`].
    /// A lost jump host is reported first: it is the cause.
    pub fn lost_reason(&self) -> Option<LostReason> {
        chain_reason(&self.lost_watches())
    }

    /// Wait for the connection to end: the reason if it was lost, `None`
    /// if it was closed on purpose ([`Session::close`]). For sessions without
    /// a shell (SFTP), which have no channel of their own to watch.
    pub async fn closed(&self) -> Option<LostReason> {
        while !self.handle.is_closed() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let watches = self.lost_watches();
        // The reason can be recorded a moment after the handle closes.
        for _ in 0..12 {
            if let Some(reason) = chain_reason(&watches) {
                return Some(reason);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
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
        let lost = self.lost_watches();

        tokio::spawn(async move {
            let mut channel = channel;
            let mut input_open = true;
            // The server ended the channel itself (exit status, EOF or close),
            // or we closed it: either way the connection was not lost.
            let mut ended = false;
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
                            ended = true;
                            let _ = channel.eof().await;
                            let _ = channel.close().await;
                        }
                    },
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if event_tx.send(ShellEvent::Output(data.to_vec())).await.is_err() { break; }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            ended = true;
                            let _ = event_tx.send(ShellEvent::Exit(exit_status)).await;
                        }
                        Some(ChannelMsg::ExitSignal { .. } | ChannelMsg::Eof | ChannelMsg::Close) => ended = true,
                        Some(_) => {}
                        None => break,
                    },
                }
            }
            if ended {
                let _ = event_tx.send(ShellEvent::Closed).await;
            } else {
                let _ = event_tx.send(ShellEvent::Lost(lost_reason(&lost).await)).await;
            }
        });

        Ok(Shell { input: input_tx, events: event_rx })
    }

    /// Close this session, then the jump hosts it ran through.
    pub async fn close(&self) -> Result<()> {
        self.handle.disconnect(Disconnect::ByApplication, "", "en").await?;
        if let Some(via) = &self.via {
            Box::pin(via.close()).await?;
        }
        Ok(())
    }
}

/// A jump host's reason if one was recorded (it is the cause), otherwise
/// the last hop's own.
fn chain_reason(watches: &[LostWatch]) -> Option<LostReason> {
    let (last, hops) = watches.split_last()?;
    for (host, lost) in hops {
        if let Some(reason) = lost.lock().unwrap().clone() {
            return Some(LostReason::JumpHost { host: host.clone(), reason: Box::new(reason) });
        }
    }
    last.1.lock().unwrap().clone()
}

/// The reason the session recorded, waiting briefly for it: the channel can
/// end a moment before russh reports why the connection went. When a jump
/// host is involved, its reason can arrive just after the last hop's, so
/// that case waits a little longer for it.
async fn lost_reason(watches: &[LostWatch]) -> LostReason {
    let has_hops = watches.len() > 1;
    let mut own_seen_for = 0;
    for _ in 0..30 {
        match chain_reason(watches) {
            Some(reason @ LostReason::JumpHost { .. }) => return reason,
            Some(reason) if !has_hops || own_seen_for >= 8 => return reason,
            Some(_) => own_seen_for += 1,
            None => {}
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    chain_reason(watches).unwrap_or_else(|| LostReason::Network("the connection closed unexpectedly".into()))
}

pub enum ShellInput {
    Data(Vec<u8>),
    Resize(PtySize),
    Close,
}

#[derive(Debug)]
pub enum ShellEvent {
    Output(Vec<u8>),
    /// The remote shell's exit status. `Closed` follows.
    Exit(u32),
    /// The shell's channel closed normally.
    Closed,
    /// The connection was lost under a running shell. Nothing follows.
    Lost(LostReason),
}

/// A running remote shell.
pub struct Shell {
    pub input: mpsc::UnboundedSender<ShellInput>,
    pub events: mpsc::Receiver<ShellEvent>,
}
