//! SSH sessions for Holonet: connecting, the host key decision, sign-in and
//! PTY shells. Forwarding and SFTP come in later milestones. Builds and tests
//! without a window.
//!
//! The interface takes part through [`Prompts`] (host key decisions and
//! secrets that are not saved). A [`Shell`] is a pair of channels, so the
//! interface never touches russh: send [`ShellInput`] in, receive
//! [`ShellEvent`] out. The output channel is bounded, so a slow reader pushes
//! back on the SSH window instead of buffering without limit.

mod connect;
pub mod dev;
pub mod hostkey;
pub mod keys;

use anyhow::Result;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;

pub use connect::{
    BoxFuture, ConnectError, ConnectOptions, Credential, HostKeyDecision, KEEPALIVE, Prompts, SecretRequest,
    SessionInfo, probe,
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
}

impl Session {
    /// Connect, decide on the host key through `opts.prompts`, and sign in.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ConnectError> {
        let (handle, info) = connect::connect(&opts).await?;
        Ok(Self { handle, info })
    }

    /// Negotiated algorithms, host key and sign-in method.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
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
