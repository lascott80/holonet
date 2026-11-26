//! Port forwarding over a signed-in session (milestone 6): local, remote and
//! dynamic (SOCKS5) forwards.
//!
//! [`crate::Session::forward`] binds the listening port (on this PC, or on
//! the host for a remote forward) before it returns, so a port in use or a
//! refusal comes back as a [`ForwardError`]. A task then serves connections
//! until the [`Forward`] is stopped or dropped, or the connection is lost.
//! Its [`ForwardStatus`] says how many connections are open and the last
//! connection that failed, which does not stop the forward.
//!
//! - **Local:** each connection to the port on this PC opens a
//!   `direct-tcpip` channel to the destination as the host sees it.
//! - **Remote:** `tcpip-forward` asks the host to listen; the host opens a
//!   `forwarded-tcpip` channel per connection, which the session's handler
//!   joins to the destination on this PC (see [`Routes`]).
//! - **Dynamic:** a SOCKS5 server on this PC (RFC 1928: CONNECT, no
//!   authentication, IPv4, IPv6 and domain names). The host resolves names.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::Msg;
use russh::{Channel, ChannelOpenFailure};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;

use crate::connect::Handle;

/// How often a forward checks that its connection is still there.
const LOST_CHECK: Duration = Duration::from_millis(500);

/// What to forward. Bind addresses are IP addresses or `localhost`; empty
/// means 127.0.0.1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardSpec {
    /// A port on this PC reaches `destination` as the host sees it (`ssh -L`).
    Local { bind_address: String, port: u16, destination: String, destination_port: u16 },
    /// A port on the host reaches `destination` on this PC (`ssh -R`). Port 0
    /// lets the host choose; the status reports the port it chose.
    Remote { bind_address: String, port: u16, destination: String, destination_port: u16 },
    /// A SOCKS5 proxy on this PC whose connections leave from the host (`ssh -D`).
    Dynamic { bind_address: String, port: u16 },
}

/// A forward's state, for the Port forwarding page and the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardStatus {
    Active {
        /// The listening port (the host's choice for a remote forward on port 0).
        port: u16,
        /// Connections open now.
        open: usize,
        /// Connections since the forward started.
        total: u64,
        /// The last connection that could not be made, as interface text.
        last_error: Option<String>,
    },
    /// The forward ended because of this; nothing is listening any more.
    Failed(ForwardError),
    /// Stopped on purpose.
    Stopped,
}

/// Why a forward could not start, or ended.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ForwardError {
    #[error("Port {0} is already in use on this PC.")]
    PortInUse(u16),
    #[error("This PC would not open port {port} on {address}: {reason}")]
    Bind { address: String, port: u16, reason: String },
    #[error("The host refused to forward port {0}.")]
    Refused(u16),
    #[error("The connection to the host was lost.")]
    ConnectionLost,
}

/// A running forward. Dropping it stops the forward.
pub struct Forward {
    status: watch::Receiver<ForwardStatus>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Forward {
    pub fn status(&self) -> ForwardStatus {
        self.status.borrow().clone()
    }

    /// Follow status changes (`changed().await`).
    pub fn watch(&self) -> watch::Receiver<ForwardStatus> {
        self.status.clone()
    }

    /// Stop listening and close its connections; waits until it has.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Counts connections and publishes the status.
struct Tracker {
    status: watch::Sender<ForwardStatus>,
    port: u16,
    open: AtomicUsize,
    total: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Tracker {
    fn new(port: u16) -> (Arc<Self>, watch::Receiver<ForwardStatus>) {
        let (status, rx) = watch::channel(ForwardStatus::Active { port, open: 0, total: 0, last_error: None });
        let tracker = Arc::new(Self {
            status,
            port,
            open: AtomicUsize::new(0),
            total: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        (tracker, rx)
    }

    fn publish(&self) {
        let active = ForwardStatus::Active {
            port: self.port,
            open: self.open.load(Ordering::SeqCst),
            total: self.total.load(Ordering::SeqCst),
            last_error: self.last_error.lock().unwrap().clone(),
        };
        // An ended forward keeps its final status.
        self.status.send_if_modified(|s| {
            let changed = matches!(s, ForwardStatus::Active { .. }) && *s != active;
            if changed {
                *s = active;
            }
            changed
        });
    }

    /// A connection started; the guard counts it until it ends.
    fn opened(self: &Arc<Self>) -> Open {
        self.open.fetch_add(1, Ordering::SeqCst);
        self.total.fetch_add(1, Ordering::SeqCst);
        self.publish();
        Open(self.clone())
    }

    fn error(&self, text: String) {
        *self.last_error.lock().unwrap() = Some(text);
        self.publish();
    }

    fn end(&self, status: ForwardStatus) {
        self.status.send_replace(status);
    }
}

/// One open connection, counted until dropped.
struct Open(Arc<Tracker>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
        self.0.publish();
    }
}

/// Remote forwards on a session, by the port the host listens on: where
/// each connection goes on this PC. Shared with the session's handler,
/// which receives the host's `forwarded-tcpip` channels.
#[derive(Clone, Default)]
pub(crate) struct Routes(Arc<Mutex<HashMap<u32, Route>>>);

#[derive(Clone)]
struct Route {
    destination: String,
    destination_port: u16,
    tracker: Arc<Tracker>,
    /// Connections being joined, so stopping the forward closes them.
    tasks: Arc<Mutex<JoinSet<()>>>,
}

impl Routes {
    /// Join a channel the host opened for `port` to its destination.
    /// `false` when no forward is listening there.
    pub(crate) fn accept(&self, port: u32, channel: Channel<Msg>) -> bool {
        let Some(route) = self.0.lock().unwrap().get(&port).cloned() else { return false };
        let open = route.tracker.opened();
        route.tasks.lock().unwrap().spawn(async move {
            let _open = open;
            let target = format!("{}:{}", route.destination, route.destination_port);
            match TcpStream::connect((route.destination.as_str(), route.destination_port)).await {
                Ok(mut tcp) => {
                    let _ = tcp.set_nodelay(true);
                    let mut stream = channel.into_stream();
                    let _ = copy_bidirectional(&mut stream, &mut tcp).await;
                }
                Err(e) => {
                    route.tracker.error(format!("{target} on this PC: {}", io_error_text(&e)));
                    let _ = channel.close().await;
                }
            }
        });
        true
    }

    pub(crate) fn has(&self, port: u32) -> bool {
        self.0.lock().unwrap().contains_key(&port)
    }

    fn remove(&self, port: u32) -> Option<Route> {
        self.0.lock().unwrap().remove(&port)
    }
}

fn io_error_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => "nothing is listening there".into(),
        std::io::ErrorKind::TimedOut => "no answer".into(),
        _ => e.to_string(),
    }
}

/// Why the host would not open a `direct-tcpip` channel, as interface text.
fn open_failure_text(failure: &ChannelOpenFailure) -> String {
    match failure {
        ChannelOpenFailure::ConnectFailed => "the host could not connect to it".into(),
        ChannelOpenFailure::AdministrativelyProhibited => "the host does not allow forwarding".into(),
        ChannelOpenFailure::ResourceShortage => "the host is out of resources".into(),
        ChannelOpenFailure::UnknownChannelType => "the host does not support forwarding".into(),
        ChannelOpenFailure::Other { reason, .. } if !reason.trim().is_empty() => reason.trim().to_string(),
        ChannelOpenFailure::Other { code, .. } => format!("refused with code {code}"),
    }
}

/// Open a `direct-tcpip` channel; the error is interface text, and the
/// SOCKS reply code that fits it.
async fn open_direct(handle: &Handle, host: &str, port: u16, peer: SocketAddr) -> Result<Channel<Msg>, (String, u8)> {
    match handle.channel_open_direct_tcpip(host, u32::from(port), peer.ip().to_string(), u32::from(peer.port())).await {
        Ok(channel) => Ok(channel),
        Err(russh::Error::ChannelOpenFailure(f)) => {
            let code = match f {
                ChannelOpenFailure::ConnectFailed => 5,
                ChannelOpenFailure::AdministrativelyProhibited => 2,
                _ => 1,
            };
            Err((format!("{host}:{port}: {}", open_failure_text(&f)), code))
        }
        Err(e) => Err((format!("{host}:{port}: {e}"), 1)),
    }
}

/// Bind a listener on this PC, with interface text for the usual failures.
async fn bind(address: &str, port: u16) -> Result<TcpListener, ForwardError> {
    let address = if address.trim().is_empty() { "127.0.0.1" } else { address.trim() };
    TcpListener::bind((address, port)).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => ForwardError::PortInUse(port),
        // Windows reserves some port ranges (`netsh interface ipv4 show
        // excludedportrange`) and answers with "access denied".
        std::io::ErrorKind::PermissionDenied => ForwardError::Bind {
            address: address.to_string(),
            port,
            reason: "Windows reserves this port, or it needs administrator rights.".into(),
        },
        std::io::ErrorKind::AddrNotAvailable => {
            ForwardError::Bind { address: address.to_string(), port, reason: "this PC has no such address.".into() }
        }
        _ => ForwardError::Bind { address: address.to_string(), port, reason: e.to_string() },
    })
}

/// Start a forward on `handle`'s session.
pub(crate) async fn start(handle: Arc<Handle>, routes: Routes, spec: ForwardSpec) -> Result<Forward, ForwardError> {
    if handle.is_closed() {
        return Err(ForwardError::ConnectionLost);
    }
    let (stop_tx, stop_rx) = oneshot::channel();
    match spec {
        ForwardSpec::Local { bind_address, port, destination, destination_port } => {
            let listener = bind(&bind_address, port).await?;
            let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            let (tracker, status) = Tracker::new(bound);
            let kind = Kind::Local { destination, destination_port };
            let task = tokio::spawn(serve_listener(handle, listener, kind, tracker, stop_rx));
            Ok(Forward { status, stop: Some(stop_tx), task: Some(task) })
        }
        ForwardSpec::Dynamic { bind_address, port } => {
            let listener = bind(&bind_address, port).await?;
            let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            let (tracker, status) = Tracker::new(bound);
            let task = tokio::spawn(serve_listener(handle, listener, Kind::Socks, tracker, stop_rx));
            Ok(Forward { status, stop: Some(stop_tx), task: Some(task) })
        }
        ForwardSpec::Remote { bind_address, port, destination, destination_port } => {
            let address = if bind_address.trim().is_empty() { "127.0.0.1".to_string() } else { bind_address };
            let chosen = match handle.tcpip_forward(address.clone(), u32::from(port)).await {
                // 0 means "the port asked for"; otherwise the host's choice.
                Ok(0) => port,
                Ok(chosen) => u16::try_from(chosen).unwrap_or(port),
                Err(russh::Error::RequestDenied) => return Err(ForwardError::Refused(port)),
                Err(_) => return Err(ForwardError::ConnectionLost),
            };
            let (tracker, status) = Tracker::new(chosen);
            let tasks = Arc::new(Mutex::new(JoinSet::new()));
            let route = Route { destination, destination_port, tracker: tracker.clone(), tasks };
            routes.0.lock().unwrap().insert(u32::from(chosen), route);
            let task = tokio::spawn(serve_remote(handle, routes, address, chosen, tracker, stop_rx));
            Ok(Forward { status, stop: Some(stop_tx), task: Some(task) })
        }
    }
}

enum Kind {
    Local { destination: String, destination_port: u16 },
    Socks,
}

/// Wait for the stop signal (or the `Forward` being dropped) or a lost
/// connection: the status to end with.
async fn until_ended(handle: &Handle, stop: &mut oneshot::Receiver<()>) -> ForwardStatus {
    let mut tick = tokio::time::interval(LOST_CHECK);
    loop {
        tokio::select! {
            _ = &mut *stop => return ForwardStatus::Stopped,
            _ = tick.tick() => {
                if handle.is_closed() {
                    return ForwardStatus::Failed(ForwardError::ConnectionLost);
                }
            }
        }
    }
}

async fn serve_listener(
    handle: Arc<Handle>,
    listener: TcpListener,
    kind: Kind,
    tracker: Arc<Tracker>,
    mut stop: oneshot::Receiver<()>,
) {
    let kind = Arc::new(kind);
    let mut connections = JoinSet::new();
    let ended = {
        let ended = until_ended(&handle, &mut stop);
        tokio::pin!(ended);
        loop {
            tokio::select! {
                status = &mut ended => break status,
                accepted = listener.accept() => {
                    let Ok((tcp, peer)) = accepted else { continue };
                    let _ = tcp.set_nodelay(true);
                    let open = tracker.opened();
                    let (handle, tracker, kind) = (handle.clone(), tracker.clone(), kind.clone());
                    connections.spawn(async move {
                        let _open = open;
                        let result = match &*kind {
                            Kind::Local { destination, destination_port } => {
                                local_connection(&handle, tcp, peer, destination, *destination_port).await
                            }
                            Kind::Socks => socks_connection(&handle, tcp, peer).await,
                        };
                        // Handshake errors are the client's and come back empty.
                        if let Err(text) = result
                            && !text.is_empty()
                        {
                            tracker.error(text);
                        }
                    });
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    };
    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    tracker.end(ended);
}

async fn local_connection(
    handle: &Handle,
    mut tcp: TcpStream,
    peer: SocketAddr,
    destination: &str,
    port: u16,
) -> Result<(), String> {
    let channel = open_direct(handle, destination, port, peer).await.map_err(|(text, _)| text)?;
    let mut stream = channel.into_stream();
    let _ = copy_bidirectional(&mut tcp, &mut stream).await;
    Ok(())
}

/// One SOCKS5 connection (RFC 1928). Errors in the handshake are the
/// client's and not reported; a destination the host cannot reach is.
async fn socks_connection(handle: &Handle, mut tcp: TcpStream, peer: SocketAddr) -> Result<(), String> {
    let quiet = |_| String::new();
    let mut hello = [0u8; 2];
    tcp.read_exact(&mut hello).await.map_err(quiet)?;
    if hello[0] != 5 {
        return Ok(());
    }
    let mut methods = vec![0u8; usize::from(hello[1])];
    tcp.read_exact(&mut methods).await.map_err(quiet)?;
    if !methods.contains(&0) {
        // No acceptable method: only "no authentication" is offered.
        let _ = tcp.write_all(&[5, 0xff]).await;
        return Ok(());
    }
    tcp.write_all(&[5, 0]).await.map_err(quiet)?;
    let mut request = [0u8; 4];
    tcp.read_exact(&mut request).await.map_err(quiet)?;
    let reply = |code: u8| [5, code, 0, 1, 0, 0, 0, 0, 0, 0];
    if request[1] != 1 {
        // Only CONNECT: BIND and UDP ASSOCIATE are not supported.
        let _ = tcp.write_all(&reply(7)).await;
        return Ok(());
    }
    let host = match request[3] {
        1 => {
            let mut a = [0u8; 4];
            tcp.read_exact(&mut a).await.map_err(quiet)?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        4 => {
            let mut a = [0u8; 16];
            tcp.read_exact(&mut a).await.map_err(quiet)?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        3 => {
            let len = usize::from(tcp.read_u8().await.map_err(quiet)?);
            let mut name = vec![0u8; len];
            tcp.read_exact(&mut name).await.map_err(quiet)?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => {
            let _ = tcp.write_all(&reply(8)).await;
            return Ok(());
        }
    };
    let port = tcp.read_u16().await.map_err(quiet)?;
    match open_direct(handle, &host, port, peer).await {
        Ok(channel) => {
            tcp.write_all(&reply(0)).await.map_err(quiet)?;
            let mut stream = channel.into_stream();
            let _ = copy_bidirectional(&mut tcp, &mut stream).await;
            Ok(())
        }
        Err((text, code)) => {
            let _ = tcp.write_all(&reply(code)).await;
            Err(text)
        }
    }
}

async fn serve_remote(
    handle: Arc<Handle>,
    routes: Routes,
    address: String,
    port: u16,
    tracker: Arc<Tracker>,
    mut stop: oneshot::Receiver<()>,
) {
    let ended = until_ended(&handle, &mut stop).await;
    if let Some(route) = routes.remove(u32::from(port)) {
        let mut tasks = std::mem::take(&mut *route.tasks.lock().unwrap());
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    if ended == ForwardStatus::Stopped {
        let _ = handle.cancel_tcpip_forward(address, u32::from(port)).await;
    }
    tracker.end(ended);
}
