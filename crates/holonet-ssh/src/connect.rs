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
use russh::{MethodKind, MethodSet};
use tokio::net::TcpStream;

use crate::hostkey::PresentedKey;

/// Keep-alive interval from docs/decisions.md.
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
    pub keep_alive: bool,
    pub prompts: Arc<dyn Prompts>,
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
}

pub(crate) type Handle = client::Handle<Handler>;

/// Open TCP, run the handshake (with the host key decision) and sign in.
pub(crate) async fn connect(opts: &ConnectOptions) -> Result<(Handle, SessionInfo), ConnectError> {
    let stream = open_tcp(&opts.host, opts.port, opts.connect_timeout).await?;
    let config =
        Arc::new(client::Config { keepalive_interval: opts.keep_alive.then_some(KEEPALIVE), ..Default::default() });
    let info = Arc::new(Mutex::new(SessionInfo::default()));
    let rejected = Arc::new(Mutex::new(false));
    let handler = Handler {
        host: opts.host.clone(),
        port: opts.port,
        prompts: opts.prompts.clone(),
        info: info.clone(),
        rejected: rejected.clone(),
    };
    let mut handle = match client::connect_stream(config, stream, handler).await {
        Ok(handle) => handle,
        Err(_) if *rejected.lock().unwrap() => return Err(ConnectError::HostKeyRejected),
        Err(e) => return Err(e.into()),
    };
    let method = sign_in(&mut handle, opts).await?;
    let mut info = info.lock().unwrap().clone();
    info.auth_method = method.to_string();
    Ok((handle, info))
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
