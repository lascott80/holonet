//! SSH sessions for Holonet: connecting, the host key decision, sign-in and
//! PTY shells. Forwarding and SFTP come in later milestones. Builds and tests
//! without a window.
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

mod connect;
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
pub use hostkey::{KeyStatus, PresentedKey, classify, fingerprint};
pub use keys::{KeyFileError, KeyInfo, algorithm_label, inspect_private_key};

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
    handle: connect::Handle,
    info: SessionInfo,
    lost: Arc<Mutex<Option<LostReason>>>,
}

impl Session {
    /// Connect, decide on the host key through `opts.prompts`, and sign in.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ConnectError> {
        let (handle, info, lost) = connect::connect(&opts).await?;
        Ok(Self { handle, info, lost })
    }

    /// Negotiated algorithms, host key and sign-in method.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Why the connection ended, if it has ended other than by [`Session::close`].
    pub fn lost_reason(&self) -> Option<LostReason> {
        self.lost.lock().unwrap().clone()
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
        let lost = self.lost.clone();

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

    pub async fn close(&self) -> Result<()> {
        self.handle.disconnect(Disconnect::ByApplication, "", "en").await?;
        Ok(())
    }
}

/// The reason the session recorded, waiting briefly for it: the channel can
/// end a moment before russh reports why the connection went.
async fn lost_reason(lost: &Mutex<Option<LostReason>>) -> LostReason {
    for _ in 0..20 {
        if let Some(reason) = lost.lock().unwrap().clone() {
            return reason;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    LostReason::Network("the connection closed unexpectedly".into())
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
