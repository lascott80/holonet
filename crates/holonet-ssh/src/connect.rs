//! Connecting and signing in.
//!
//! The interface takes part through [`Prompts`]: russh calls back during the
//! handshake to decide on the server's key, and sign-in asks for a password or
//! passphrase that is not saved. Both await the person's answer without
//! blocking anything else.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use holonet_model::Secret;
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, decode_secret_key};
use russh::{ChannelOpenFailure, MethodKind, MethodSet};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::hostkey::PresentedKey;

/// Keep-alive interval from docs/decisions.md.
/// Default keep-alive interval (`decisions.md`).
pub const KEEPALIVE: Duration = Duration::from_secs(30);
/// Most rounds of keyboard-interactive questions before giving up.
const MAX_KBD_ROUNDS: usize = 5;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What the person decided about a server key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// Continue the connection (the key is trusted, or "Connect once").
    Continue,
    /// Stop: "Cancel", a changed key, or "Refuse to connect".
    Stop,
}

/// A secret Holonet needs and does not have saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretRequest {
    Password {
        user: String,
        host: String,
    },
    Passphrase {
        key_name: String,
    },
    /// A keyboard-interactive question that is not the password (e.g. a code).
    Challenge {
        title: String,
        instructions: String,
        prompt: String,
        echo: bool,
    },
}

/// The interface's side of a connection.
pub trait Prompts: Send + Sync + 'static {
    /// Decide on the server's key. Called once per connection, during the
    /// handshake, before any credential is sent.
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision>;
    /// Ask for a secret. `None` means the person cancelled.
    fn secret(&self, request: SecretRequest) -> BoxFuture<Option<Secret>>;
}

/// How to sign in.
#[derive(Clone, Debug)]
pub enum Credential {
    /// `None`: ask on connect.
    Password(Option<Secret>),
    /// An OpenSSH private key as stored in the vault, possibly encrypted.
    Key { name: String, private_key: Secret, passphrase: Option<Secret> },
}

#[derive(Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub credential: Credential,
    /// Limit for the TCP connection to be established.
    pub connect_timeout: Duration,
    /// Keep-alive interval; `None` sends none. Three unanswered keep-alives
    /// end the connection as lost.
    pub keep_alive: Option<Duration>,
    pub prompts: Arc<dyn Prompts>,
    /// Connect through this host first. `host` and `port` above are then as
    /// the jump host sees them, and the host key is checked against them. A
    /// jump host may have its own jump host.
    pub jump: Option<Box<ConnectOptions>>,
}

/// Why a connection did not come up. Each maps to a connection state screen.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("could not find {host}: {detail}")]
    Resolve { host: String, detail: String },
    #[error("the connection was refused")]
    Refused,
    #[error("no answer after {0} seconds")]
    TimedOut(u64),
    #[error("network error: {0}")]
    Network(String),
    #[error("the server's host key was not accepted")]
    HostKeyRejected,
    #[error("the server refused {method} sign-in")]
    SignInRefused {
        /// "password", "publickey" or "keyboard-interactive".
        method: &'static str,
        /// Methods the server says it still accepts.
        remaining: Vec<String>,
    },
    #[error("cancelled")]
    Cancelled,
    #[error("the key passphrase is wrong")]
    WrongPassphrase,
    #[error("the key could not be read: {0}")]
    BadKey(String),
    #[error("SSH error: {0}")]
    Protocol(String),
    /// Connecting to a jump host failed; `error` says how.
    #[error("the jump host {host} did not connect: {error}")]
    JumpHost { host: String, error: Box<ConnectError> },
    /// The jump host was signed in to, but could not reach the next host.
    #[error("{jump} could not reach {target}: {reason}")]
    JumpCouldNotReach { jump: String, target: String, reason: String },
}

impl ConnectError {
    /// The error at the end of a chain of jump hosts, and the jump hosts it
    /// passed through (outermost first). For choosing a connection state.
    pub fn innermost(&self) -> (&ConnectError, Vec<&str>) {
        let mut hops = Vec::new();
        let mut error = self;
        while let ConnectError::JumpHost { host, error: inner } = error {
            hops.push(host.as_str());
            error = inner;
        }
        (error, hops)
    }
}

fn open_failure_text(failure: &ChannelOpenFailure) -> String {
    match failure {
        ChannelOpenFailure::ConnectFailed => "the connection was refused, or nothing answered".into(),
        ChannelOpenFailure::AdministrativelyProhibited => "the jump host does not allow forwarding".into(),
        ChannelOpenFailure::ResourceShortage => "the jump host is out of resources".into(),
        ChannelOpenFailure::UnknownChannelType => "the jump host does not support forwarding".into(),
        ChannelOpenFailure::Other { reason, .. } if !reason.trim().is_empty() => reason.trim().to_string(),
        ChannelOpenFailure::Other { code, .. } => format!("refused with code {code}"),
    }
}

impl From<russh::Error> for ConnectError {
    fn from(e: russh::Error) -> Self {
        ConnectError::Protocol(e.to_string())
    }
}

/// Negotiated details for the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub kex: String,
    pub cipher: String,
    pub host_key_type: String,
    /// "password", "publickey" or "keyboard-interactive".
    pub auth_method: String,
    /// The host key the server presented (and that was accepted).
    pub host_key: Option<PresentedKey>,
}

pub(crate) struct Handler {
    host: String,
    port: u16,
    prompts: Arc<dyn Prompts>,
    info: Arc<Mutex<SessionInfo>>,
    rejected: Arc<Mutex<bool>>,
    /// Why the connection ended, once it has.
    lost: Arc<Mutex<Option<LostReason>>>,
}

/// Why an established connection ended without the shell exiting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LostReason {
    /// The server sent a disconnect message, with its text.
    ServerDisconnected(String),
    /// Keep-alives went unanswered (sleep, a network change, a dead route).
    NoResponse,
    /// The network connection failed or was reset.
    Network(String),
    /// Some other protocol error.
    Other(String),
    /// The connection to a jump host on the way was lost.
    JumpHost { host: String, reason: Box<LostReason> },
}

impl std::fmt::Display for LostReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerDisconnected(m) if m.trim().is_empty() => f.write_str("The server closed the connection."),
            Self::ServerDisconnected(m) => write!(f, "The server closed the connection: {}", m.trim()),
            Self::NoResponse => f.write_str("The server stopped answering keep-alives."),
            Self::Network(m) => write!(f, "The network connection failed: {m}"),
            Self::Other(m) => write!(f, "The connection failed: {m}"),
            Self::JumpHost { host, reason } => write!(f, "The connection to the jump host {host} was lost. {reason}"),
        }
    }
}

impl LostReason {
    fn from_error(e: &russh::Error) -> Self {
        match e {
            russh::Error::KeepaliveTimeout | russh::Error::InactivityTimeout => Self::NoResponse,
            russh::Error::IO(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
                Self::Network("it closed without warning".into())
            }
            russh::Error::IO(io) => Self::Network(io.to_string()),
            russh::Error::HUP | russh::Error::Disconnect => Self::Network("closed by the remote side".into()),
            other => Self::Other(other.to_string()),
        }
    }
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // Host certificates are a later milestone; refuse for now.
            *self.rejected.lock().unwrap() = true;
            return Ok(false);
        };
        let presented = PresentedKey::from_key(&self.host, self.port, key);
        let decision = self.prompts.host_key(presented.clone()).await;
        if decision == HostKeyDecision::Continue {
            let mut info = self.info.lock().unwrap();
            info.host_key_type = presented.key_type_label().to_string();
            info.host_key = Some(presented);
            Ok(true)
        } else {
            *self.rejected.lock().unwrap() = true;
            Ok(false)
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let mut info = self.info.lock().unwrap();
        info.kex = names.kex.as_ref().to_string();
        info.cipher = names.cipher.as_ref().to_string();
        Ok(())
    }

    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> Result<(), Self::Error> {
        let lost = match &reason {
            client::DisconnectReason::ReceivedDisconnect(info) => LostReason::ServerDisconnected(info.message.clone()),
            client::DisconnectReason::Error(e) => LostReason::from_error(e),
        };
        self.lost.lock().unwrap().get_or_insert(lost);
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(e) => Err(e),
        }
    }
}

pub(crate) type Handle = client::Handle<Handler>;

/// A signed-in connection, and the jump host session it runs through.
pub(crate) struct Connected {
    pub handle: Handle,
    pub info: SessionInfo,
    pub lost: Arc<Mutex<Option<LostReason>>>,
    pub via: Option<Box<crate::Session>>,
}

/// Connect (through the jump host, if any), run the handshake with the host
/// key decision, and sign in.
pub(crate) async fn connect(opts: &ConnectOptions) -> Result<Connected, ConnectError> {
    let Some(jump) = &opts.jump else {
        let stream = open_tcp(&opts.host, opts.port, opts.connect_timeout).await?;
        return handshake(opts, stream).await;
    };
    let via = Box::pin(crate::Session::connect((**jump).clone()))
        .await
        .map_err(|e| ConnectError::JumpHost { host: jump.host.clone(), error: Box::new(e) })?;
    let target = format!("{}:{}", opts.host, opts.port);
    let open = via.handle.channel_open_direct_tcpip(opts.host.clone(), opts.port.into(), "127.0.0.1", 0);
    let channel = match tokio::time::timeout(opts.connect_timeout, open).await {
        Ok(Ok(channel)) => channel,
        failed => {
            let _ = via.close().await;
            return Err(match failed {
                Err(_) => ConnectError::TimedOut(opts.connect_timeout.as_secs()),
                Ok(Err(russh::Error::ChannelOpenFailure(f))) => {
                    ConnectError::JumpCouldNotReach { jump: jump.host.clone(), target, reason: open_failure_text(&f) }
                }
                Ok(Err(e)) => e.into(),
                Ok(Ok(_)) => unreachable!(),
            });
        }
    };
    match handshake(opts, channel.into_stream()).await {
        Ok(mut connected) => {
            connected.via = Some(Box::new(via));
            Ok(connected)
        }
        Err(e) => {
            let _ = via.close().await;
            Err(e)
        }
    }
}

async fn handshake<R>(opts: &ConnectOptions, stream: R) -> Result<Connected, ConnectError>
where
    R: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let config =
        Arc::new(client::Config { keepalive_interval: opts.keep_alive, keepalive_max: 3, ..Default::default() });
    let lost = Arc::new(Mutex::new(None));
    let info = Arc::new(Mutex::new(SessionInfo::default()));
    let rejected = Arc::new(Mutex::new(false));
    let handler = Handler {
        host: opts.host.clone(),
        port: opts.port,
        prompts: opts.prompts.clone(),
        info: info.clone(),
        rejected: rejected.clone(),
        lost: lost.clone(),
    };
    let mut handle = match client::connect_stream(config, stream, handler).await {
        Ok(handle) => handle,
        Err(_) if *rejected.lock().unwrap() => return Err(ConnectError::HostKeyRejected),
        Err(e) => return Err(e.into()),
    };
    let method = sign_in(&mut handle, opts).await?;
    let mut info = info.lock().unwrap().clone();
    info.auth_method = method.to_string();
    Ok(Connected { handle, info, lost, via: None })
}

async fn open_tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, ConnectError> {
    let addresses: Vec<_> = match tokio::net::lookup_host((host, port)).await {
        Ok(addresses) => addresses.collect(),
        Err(e) => return Err(ConnectError::Resolve { host: host.to_string(), detail: e.to_string() }),
    };
    if addresses.is_empty() {
        return Err(ConnectError::Resolve { host: host.to_string(), detail: "no addresses".into() });
    }
    let attempt = async {
        let mut last = None;
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap())
    };
    match tokio::time::timeout(timeout, attempt).await {
        Err(_) => Err(ConnectError::TimedOut(timeout.as_secs())),
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => Err(ConnectError::Refused),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::TimedOut => Err(ConnectError::TimedOut(timeout.as_secs())),
        Ok(Err(e)) => Err(ConnectError::Network(e.to_string())),
    }
}

fn names(methods: &MethodSet) -> Vec<String> {
    methods.iter().map(String::from).collect()
}

async fn sign_in(handle: &mut Handle, opts: &ConnectOptions) -> Result<&'static str, ConnectError> {
    match &opts.credential {
        Credential::Password(saved) => {
            let password = match saved {
                Some(p) if !p.is_empty() => p.clone(),
                _ => opts
                    .prompts
                    .secret(SecretRequest::Password { user: opts.user.clone(), host: opts.host.clone() })
                    .await
                    .ok_or(ConnectError::Cancelled)?,
            };
            match handle.authenticate_password(&opts.user, password.expose()).await? {
                AuthResult::Success => Ok("password"),
                AuthResult::Failure { remaining_methods, .. } => {
                    // Many servers allow passwords only through keyboard-interactive.
                    if remaining_methods.contains(&MethodKind::KeyboardInteractive) {
                        keyboard_interactive(handle, opts, &password).await
                    } else {
                        Err(ConnectError::SignInRefused { method: "password", remaining: names(&remaining_methods) })
                    }
                }
            }
        }
        Credential::Key { name, private_key, passphrase } => {
            let key = match decode_secret_key(private_key.expose(), passphrase.as_ref().map(Secret::expose)) {
                Ok(key) => key,
                Err(russh::keys::Error::KeyIsEncrypted) if passphrase.is_none() => {
                    let asked = opts
                        .prompts
                        .secret(SecretRequest::Passphrase { key_name: name.clone() })
                        .await
                        .ok_or(ConnectError::Cancelled)?;
                    decode_secret_key(private_key.expose(), Some(asked.expose()))
                        .map_err(|_| ConnectError::WrongPassphrase)?
                }
                Err(_) if passphrase.is_some() => return Err(ConnectError::WrongPassphrase),
                Err(e) => return Err(ConnectError::BadKey(e.to_string())),
            };
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            match handle.authenticate_publickey(&opts.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await? {
                AuthResult::Success => Ok("publickey"),
                AuthResult::Failure { remaining_methods, .. } => {
                    Err(ConnectError::SignInRefused { method: "publickey", remaining: names(&remaining_methods) })
                }
            }
        }
    }
}

/// Answer keyboard-interactive rounds: hidden prompts get the password the
/// first time; anything else is asked of the person.
async fn keyboard_interactive(
    handle: &mut Handle,
    opts: &ConnectOptions,
    password: &Secret,
) -> Result<&'static str, ConnectError> {
    let mut response = handle.authenticate_keyboard_interactive_start(&opts.user, None::<String>).await?;
    let mut password_used = false;
    for _ in 0..MAX_KBD_ROUNDS {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok("keyboard-interactive"),
            KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                return Err(ConnectError::SignInRefused {
                    method: "keyboard-interactive",
                    remaining: names(&remaining_methods),
                });
            }
            KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                let mut answers = Vec::with_capacity(prompts.len());
                for prompt in prompts {
                    let looks_like_password = !prompt.echo && prompt.prompt.to_ascii_lowercase().contains("password");
                    if looks_like_password && !password_used {
                        password_used = true;
                        answers.push(password.expose().to_string());
                    } else {
                        let asked = opts
                            .prompts
                            .secret(SecretRequest::Challenge {
                                title: name.clone(),
                                instructions: instructions.clone(),
                                prompt: prompt.prompt.clone(),
                                echo: prompt.echo,
                            })
                            .await
                            .ok_or(ConnectError::Cancelled)?;
                        answers.push(asked.expose().to_string());
                    }
                }
                response = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Err(ConnectError::SignInRefused { method: "keyboard-interactive", remaining: Vec::new() })
}

/// Records the presented key and stops the handshake: no credentials are sent.
struct Capture(Mutex<Option<PresentedKey>>);

impl Prompts for Capture {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        *self.0.lock().unwrap() = Some(key);
        Box::pin(async { HostKeyDecision::Stop })
    }

    fn secret(&self, _: SecretRequest) -> BoxFuture<Option<Secret>> {
        Box::pin(async { None })
    }
}

/// "Test connection": open TCP, run the key exchange, and return the host
/// key the server presents. Stops before sign-in, so nothing is sent but the
/// SSH handshake.
pub async fn probe(host: &str, port: u16, timeout: Duration) -> Result<PresentedKey, ConnectError> {
    let stream = open_tcp(host, port, timeout).await?;
    let capture = Arc::new(Capture(Mutex::new(None)));
    let handler = Handler {
        host: host.to_string(),
        port,
        prompts: capture.clone(),
        info: Arc::new(Mutex::new(SessionInfo::default())),
        rejected: Arc::new(Mutex::new(false)),
        lost: Arc::new(Mutex::new(None)),
    };
    let config = Arc::new(client::Config::default());
    let handshake = client::connect_stream(config, stream, handler);
    match tokio::time::timeout(timeout, handshake).await {
        Err(_) => Err(ConnectError::TimedOut(timeout.as_secs())),
        Ok(result) => {
            if let Ok(handle) = result {
                // Not expected: the capture always stops. Close politely.
                let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            }
            capture.0.lock().unwrap().take().ok_or_else(|| ConnectError::Protocol("no host key was presented".into()))
        }
    }
}
