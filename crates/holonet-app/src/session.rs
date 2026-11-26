//! A pane: one host's connection, from connecting through the host key
//! decision and sign-in to the terminal (Terminal.dc.html, HostKey.dc.html,
//! ConnectStates.dc.html). A [`crate::tab::Tab`] holds one or more panes and
//! draws the session header and status bar for its active pane.
//!
//! The SSH connection runs on Tokio. When russh needs a host key decision or
//! a secret, [`UiPrompts`] sends the request over a channel to this entity and
//! waits for the answer, so the network side and the interface never block
//! each other.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, Render,
    SharedString, Subscription, Transformation, WeakEntity, Window, div, percentage, prelude::*, px,
};
use holonet_model::{Auth, Host, Secret, StoredKey, Timestamp, UnknownHostKeys};
use holonet_ssh::{
    BoxFuture, ConnectError, ConnectOptions, Credential, HostKeyDecision, KeyStatus, PresentedKey, Prompts, PtySize,
    SecretRequest, Session, SessionInfo, Shell,
};
use tokio::sync::{mpsc, oneshot};

use crate::app::HolonetApp;
use crate::host_form::describe_error;
use crate::shell::TabState;
use crate::terminal::GridSize;
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, ButtonKind, Checkbox, TextField, TextFieldEvent, icon};

// ------------------------------------------------------------ the bridge --

enum UiRequest {
    HostKey(PresentedKey, oneshot::Sender<HostKeyDecision>),
    Secret(SecretRequest, oneshot::Sender<Option<Secret>>),
}

/// [`Prompts`] that forwards each question to the session tab.
struct UiPrompts(mpsc::UnboundedSender<UiRequest>);

impl Prompts for UiPrompts {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        let (tx, rx) = oneshot::channel();
        let sent = self.0.send(UiRequest::HostKey(key, tx)).is_ok();
        Box::pin(async move { if sent { rx.await.unwrap_or(HostKeyDecision::Stop) } else { HostKeyDecision::Stop } })
    }

    fn secret(&self, request: SecretRequest) -> BoxFuture<Option<Secret>> {
        let (tx, rx) = oneshot::channel();
        let sent = self.0.send(UiRequest::Secret(request, tx)).is_ok();
        Box::pin(async move { if sent { rx.await.ok().flatten() } else { None } })
    }
}

// ---------------------------------------------------------------- states --

/// What the vault says about a presented key.
pub struct KeyCheck {
    pub status: KeyStatus,
    pub policy: UnknownHostKeys,
    pub trusted_at: Option<Timestamp>,
}

#[derive(Clone, Copy)]
enum Failure {
    /// Could not reach the host (refused, unresolved, network error).
    Network,
    /// The host never answered within the connect timeout.
    TimedOut,
    /// The server refused the password (saved or typed).
    Password,
    /// The server refused the key.
    KeyRefused,
    /// The key's passphrase did not unlock it.
    Passphrase,
    /// "Refuse to connect" stopped an unknown key.
    RefusedUnknown,
    /// Five reconnect attempts failed.
    GaveUp,
    Other,
}

impl Failure {
    /// The pane header's and status bar's word for this state
    /// (ConnectStates.dc.html: "Timed out", "Sign-in refused").
    fn state_word(&self) -> &'static str {
        match self {
            Failure::Network => "Not reachable",
            Failure::TimedOut => "Timed out",
            Failure::Password | Failure::KeyRefused => "Sign-in refused",
            Failure::Passphrase => "Wrong passphrase",
            Failure::RefusedUnknown => "Refused",
            Failure::GaveUp => "Not reconnected",
            Failure::Other => "Not connected",
        }
    }
}

/// The design's spinner: a `line` ring with an accent arc turning on it
/// every 0.9 s (ConnectStates.dc.html). GPUI borders take one color, so the
/// arc is an SVG laid over the ring.
fn ring_spinner(size: gpui::Pixels, border: gpui::Pixels) -> Div {
    div().size(size).flex_none().relative().rounded_full().border(border).border_color(theme::line()).child(
        div().absolute().top(-border).left(-border).child(icon("spinner-arc", size, theme::accent()).with_animation(
            "spin",
            Animation::new(Duration::from_millis(900)).repeat(),
            |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
        )),
    )
}

/// Why a host may not have answered, from its address (ConnectStates.dc.html,
/// "Timed out"). Tailscale advice joins this in milestone 4.
fn unanswered_reason(address: &str) -> String {
    let private = match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => (ip.segments()[0] & 0xfe00) == 0xfc00 || ip.is_unicast_link_local(),
        Err(_) => address.ends_with(".local") || address.ends_with(".lan") || address.ends_with(".home.arpa"),
    };
    if private {
        format!("{address} is a private address, so you may be away from its network.")
    } else {
        "The server may be down, or a firewall may be dropping the connection.".to_string()
    }
}

/// decisions.md: 5 attempts. Waits before each (seconds).
const RECONNECT_BACKOFF: [u64; 5] = [1, 2, 4, 8, 16];

/// A lost connection being re-established (ConnectStates.dc.html,
/// "Reconnecting"). The terminal and its scrollback stay; the shell restarts.
struct Reconnect {
    /// Attempts started so far (1-based once the first one starts).
    attempt: usize,
    /// "Connection lost when this PC went to sleep." and the like.
    cause: SharedString,
    /// When the next attempt starts, while waiting.
    next_at: Option<std::time::Instant>,
    /// The reason the last attempt failed.
    last_error: Option<SharedString>,
}

/// Why a connection most likely dropped, from recent system events.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disturbance {
    Sleep,
    Network,
}

enum State {
    Connecting,
    HostKey {
        key: PresentedKey,
        reply: Option<oneshot::Sender<HostKeyDecision>>,
    },
    KeyChanged {
        presented: PresentedKey,
        trusted_key: String,
        trusted_at: Option<Timestamp>,
        expected: bool,
    },
    Secret {
        request: SecretRequest,
        field: Entity<TextField>,
        save: bool,
        reply: Option<oneshot::Sender<Option<Secret>>>,
    },
    Connected,
    Failed {
        kind: Failure,
        title: SharedString,
        body: SharedString,
    },
    Ended(SharedString),
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The pane should close (Cancel, Disconnect, Close tab).
    Close,
    /// Open the host form for this host.
    EditHost,
    /// Open Known hosts at this host's entry.
    ReviewKnownHost,
}

pub struct Pane {
    app: WeakEntity<HolonetApp>,
    pub host: Host,
    key: Option<StoredKey>,
    runtime: tokio::runtime::Handle,
    limits: Limits,
    state: State,
    log: Vec<(SharedString, bool)>,
    terminal: Entity<TerminalView>,
    session: Option<Arc<Session>>,
    info: SessionInfo,
    /// Ask for the password even if one is saved (after it was refused).
    ask_password: bool,
    /// Ask for the passphrase even if one is saved (after it was wrong).
    ask_passphrase: bool,
    /// Bumped on every attempt, so stale results are ignored.
    attempt: u64,
    /// A typed password (true) or passphrase to save after a successful sign-in.
    pending_save: Option<(bool, Secret)>,
    /// Drawn with the stronger border of the tab's active pane.
    active: bool,
    /// The only pane in its tab: "Close tab" instead of "Close pane".
    alone: bool,
    /// The attempt used a password saved in the vault (for the wording of
    /// "The server refused the saved password").
    used_saved_password: bool,
    /// While reconnecting after a lost connection.
    reconnect: Option<Reconnect>,
    /// Waits for and starts the next reconnect attempt.
    reconnect_task: Option<gpui::Task<()>>,
    /// A password (true) or passphrase typed during this attempt, and the one
    /// that worked, kept in memory so reconnecting does not ask again. Never
    /// saved unless "Save in the encrypted vault" was ticked.
    typed_secret: Option<(bool, Secret)>,
    session_secret: Option<(bool, Secret)>,
    /// The last sleep, wake or network change, and when.
    disturbance: Option<(Disturbance, std::time::Instant)>,
    /// Focus the prompt's field at the next render (it is created without a window).
    focus_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SessionEvent> for Pane {}

/// Settings read when the tab is created.
#[derive(Clone)]
pub struct Limits {
    pub connect_timeout: Duration,
    /// Already combined with the host's own keep-alive switch.
    pub keep_alive: Option<Duration>,
    pub terminal: holonet_model::TerminalSettings,
}

impl Pane {
    /// Settings changed: show them in this tab's terminal.
    pub fn set_terminal_settings(&mut self, settings: &holonet_model::TerminalSettings, cx: &mut Context<Self>) {
        self.limits.terminal = settings.clone();
        self.terminal.update(cx, |t, cx| t.set_settings(settings.clone(), cx));
    }

    #[allow(clippy::too_many_arguments)] // GPUI context plus the connection's inputs
    pub fn new(
        app: WeakEntity<HolonetApp>,
        host: Host,
        key: Option<StoredKey>,
        runtime: tokio::runtime::Handle,
        limits: Limits,
        size: GridSize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = cx.new(|cx| {
            let mut t = TerminalView::new(window, cx);
            t.apply_settings(&limits.terminal, window, cx);
            t.set_embedded(size);
            t
        });
        let subscriptions = vec![cx.subscribe(&terminal, |this: &mut Self, _, event: &TerminalEvent, cx| {
            match event {
                TerminalEvent::Ended(reason) => {
                    this.state = State::Ended(reason.clone().into());
                    this.session = None;
                }
                TerminalEvent::Lost(reason) => this.on_lost(reason.clone(), cx),
            }
            cx.notify();
        })];
        let mut tab = Self {
            app,
            host,
            key,
            runtime,
            limits,
            state: State::Connecting,
            log: Vec::new(),
            terminal,
            session: None,
            info: SessionInfo::default(),
            ask_password: false,
            ask_passphrase: false,
            attempt: 0,
            pending_save: None,
            focus_pending: false,
            active: true,
            alone: true,
            used_saved_password: false,
            reconnect: None,
            reconnect_task: None,
            typed_secret: None,
            session_secret: None,
            disturbance: None,
            _subscriptions: subscriptions,
        };
        tab.start(cx);
        tab
    }

    pub fn name(&self) -> SharedString {
        self.host.display_name().to_string().into()
    }

    pub fn tab_state(&self) -> TabState {
        if self.reconnect.is_some() {
            return TabState::Connecting;
        }
        match self.state {
            State::Connected => TabState::Connected,
            State::Failed { .. } | State::Ended(_) | State::KeyChanged { .. } => TabState::Failed,
            _ => TabState::Connecting,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.state {
            State::Connected | State::Ended(_) => self.terminal.read(cx).focus(window),
            State::Secret { field, .. } => field.read(cx).focus(window),
            _ => {}
        }
    }

    fn note(&mut self, line: impl Into<SharedString>, warn: bool) {
        self.log.push((line.into(), warn));
        if self.log.len() > 8 {
            self.log.remove(0);
        }
    }

    /// "Offering password for root", once the host key is settled.
    fn note_offer(&mut self) {
        let user = if self.host.username.is_empty() { "this user".to_string() } else { self.host.username.clone() };
        let line = match (&self.host.auth, &self.key) {
            (Auth::Key { .. }, Some(key)) => format!("Offering key {} for {user}", key.name),
            _ => format!("Offering password for {user}"),
        };
        self.note(line, false);
    }

    fn target(&self) -> String {
        format!("{}:{}", self.host.address.trim(), self.host.port)
    }

    /// Where the pane sits in its tab (set by [`crate::tab::Tab`]).
    pub fn set_place(&mut self, active: bool, alone: bool, cx: &mut Context<Self>) {
        if (self.active, self.alone) != (active, alone) {
            (self.active, self.alone) = (active, alone);
            cx.notify();
        }
    }

    pub fn grid_size(&self, cx: &gpui::App) -> GridSize {
        self.terminal.read(cx).grid_size()
    }

    fn close_label(&self) -> &'static str {
        if self.alone { "Close tab" } else { "Close pane" }
    }

    /// `user@address:port`, for the session header.
    pub fn target_line(&self) -> SharedString {
        if self.host.username.is_empty() {
            self.target().into()
        } else {
            format!("{}@{}", self.host.username, self.target()).into()
        }
    }

    fn session_secret(&self, password: bool) -> Option<Secret> {
        self.session_secret.as_ref().filter(|(p, _)| *p == password).map(|(_, s)| s.clone())
    }

    /// Sleep, wake or a network change (from the app's system listener).
    pub fn on_system_event(&mut self, event: crate::platform::SystemEvent, cx: &mut Context<Self>) {
        use crate::platform::SystemEvent;
        let kind = match event {
            SystemEvent::Suspend | SystemEvent::Resume => Disturbance::Sleep,
            SystemEvent::NetworkChanged => Disturbance::Network,
        };
        // Sleep explains a drop better than the network changes that follow a wake.
        let keep_sleep = matches!(self.disturbance, Some((Disturbance::Sleep, at)) if at.elapsed().as_secs() < 120)
            && kind == Disturbance::Network;
        if !keep_sleep {
            self.disturbance = Some((kind, std::time::Instant::now()));
        }
        // Waiting to reconnect: the network may be back, so try now.
        if event != SystemEvent::Suspend && self.reconnect.as_ref().is_some_and(|r| r.next_at.is_some()) {
            self.reconnect_now(cx);
        }
    }

    /// The connection dropped under a running shell.
    fn on_lost(&mut self, reason: holonet_ssh::LostReason, cx: &mut Context<Self>) {
        self.session = None;
        let enabled = self.app.upgrade().map(|app| app.read(cx).reconnect_enabled(cx)).unwrap_or(true);
        if !enabled {
            self.state = State::Ended(reason.to_string().into());
            return;
        }
        // A drop is noticed up to three missed keep-alives after it happens
        // (about 90 s at the default), so recent means five minutes.
        let recent = self.disturbance.filter(|(_, at)| at.elapsed().as_secs() < 300).map(|(d, _)| d);
        let cause: SharedString = match (recent, &reason) {
            (Some(Disturbance::Sleep), _) => "Connection lost when this PC went to sleep.".into(),
            (Some(Disturbance::Network), _) => "Connection lost after a network change.".into(),
            (None, holonet_ssh::LostReason::NoResponse) => "The server stopped answering.".into(),
            (None, holonet_ssh::LostReason::ServerDisconnected(_)) => "The server closed the connection.".into(),
            (None, _) => "The connection was lost.".into(),
        };
        self.reconnect = Some(Reconnect { attempt: 0, cause, next_at: None, last_error: None });
        self.state = State::Connecting;
        self.schedule_reconnect(cx);
    }

    /// Wait before the next attempt, or give up after the fifth.
    fn schedule_reconnect(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.reconnect.as_mut() else { return };
        let Some(wait) = RECONNECT_BACKOFF.get(r.attempt).copied() else {
            let last = r.last_error.clone().unwrap_or_else(|| r.cause.clone());
            self.reconnect = None;
            self.reconnect_task = None;
            self.fail(
                Failure::GaveUp,
                format!("Holonet could not reconnect to {}", self.host.display_name()),
                format!("It tried {} times. {last}", RECONNECT_BACKOFF.len()),
            );
            cx.notify();
            return;
        };
        let at = std::time::Instant::now() + Duration::from_secs(wait);
        r.next_at = Some(at);
        // Tick once a second for the countdown, then start the attempt.
        self.reconnect_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let done = this.update(cx, |pane, cx| {
                    if std::time::Instant::now() >= at {
                        pane.reconnect_now(cx);
                        true
                    } else {
                        cx.notify();
                        false
                    }
                });
                if !matches!(done, Ok(false)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// "Reconnect now", a wake or network change, or the wait ran out.
    fn reconnect_now(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.reconnect.as_mut() else { return };
        if r.next_at.is_none() {
            return; // an attempt is already running
        }
        r.attempt += 1;
        r.next_at = None;
        self.reconnect_task = None;
        self.start(cx);
    }

    /// A reconnect attempt failed. Network trouble waits and tries again;
    /// anything else (a refused sign-in) ends reconnecting and shows the
    /// usual failure. Returns true when handled here.
    fn reconnect_failed(&mut self, e: &ConnectError, cx: &mut Context<Self>) -> bool {
        let retry = matches!(
            e,
            ConnectError::Resolve { .. } | ConnectError::Refused | ConnectError::Network(_) | ConnectError::TimedOut(_)
        ) || (matches!(e, ConnectError::HostKeyRejected)
            && self.app.upgrade().is_some_and(|a| !a.read(cx).is_unlocked()));
        if !retry {
            self.reconnect = None;
            self.reconnect_task = None;
            return false;
        }
        let message: SharedString = match e {
            ConnectError::HostKeyRejected => "Unlock the vault so Holonet can check the host key.".into(),
            other => describe_error(other).into(),
        };
        if let Some(r) = self.reconnect.as_mut() {
            r.last_error = Some(message);
        }
        self.state = State::Connecting;
        self.schedule_reconnect(cx);
        true
    }

    /// Whether the pane stopped without a working session (failed or ended).
    pub fn is_stopped(&self) -> bool {
        matches!(self.state, State::Failed { .. } | State::Ended(_))
    }

    /// The host was edited from this pane's failure ("Edit host", then "Save
    /// and connect"): try again here with the new details.
    pub fn retry_with(&mut self, host: Host, key: Option<StoredKey>, limits: Limits, cx: &mut Context<Self>) {
        self.host = host;
        self.key = key;
        self.limits = limits;
        self.ask_password = false;
        self.ask_passphrase = false;
        self.start(cx);
        cx.notify();
    }

    /// Start (or restart) the connection.
    fn start(&mut self, cx: &mut Context<Self>) {
        self.attempt += 1;
        let attempt = self.attempt;
        self.pending_save = None;
        self.typed_secret = None;
        self.state = State::Connecting;
        self.session = None;
        // Each attempt starts a fresh log (ConnectStates.dc.html).
        self.log.clear();
        let address = self.host.address.trim().to_string();
        if address.parse::<std::net::IpAddr>().is_err() {
            self.note(format!("Resolving {address} …"), false);
        }
        self.note(format!("Opening TCP connection to {}", self.target()), false);
        self.used_saved_password = matches!(&self.host.auth, Auth::Password { saved: Some(_) }) && !self.ask_password;

        let credential = match (&self.host.auth, &self.key) {
            (Auth::Password { saved }, _) => Credential::Password(if self.ask_password {
                None
            } else {
                saved.clone().or_else(|| self.session_secret(true))
            }),
            (Auth::Key { passphrase, .. }, Some(key)) => Credential::Key {
                name: key.name.clone(),
                private_key: key.private_key.clone(),
                passphrase: if self.ask_passphrase {
                    None
                } else {
                    passphrase.clone().or_else(|| self.session_secret(false))
                },
            },
            (Auth::Key { .. }, None) => {
                self.fail(
                    Failure::Other,
                    "The key for this host is missing",
                    "It is no longer in the vault. Edit the host to choose or import a key.",
                );
                return;
            }
        };
        // Not read from the app: `start` runs while the app creates this tab.
        let limits = (self.limits.connect_timeout, self.limits.keep_alive);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let opts = ConnectOptions {
            host: self.host.address.trim().to_string(),
            port: self.host.port,
            user: self.host.username.clone(),
            credential,
            connect_timeout: limits.0,
            keep_alive: limits.1,
            prompts: Arc::new(UiPrompts(tx)),
        };
        let size = self.terminal.read(cx).grid_size();
        let (done_tx, done_rx) = oneshot::channel::<Result<(Session, Shell), ConnectError>>();
        self.runtime.spawn(async move {
            let result = async {
                let session = Session::connect(opts).await?;
                let shell = session
                    .open_shell(PtySize::cells(size.cols as u16, size.rows as u16))
                    .await
                    .map_err(|e| ConnectError::Protocol(format!("{e:#}")))?;
                Ok((session, shell))
            }
            .await;
            let _ = done_tx.send(result);
        });

        // Questions from the connection while it runs.
        cx.spawn(async move |this, cx| {
            while let Some(request) = rx.recv().await {
                if this.update(cx, |tab, cx| tab.on_request(request, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // The outcome.
        cx.spawn(async move |this, cx| {
            let result =
                done_rx.await.unwrap_or_else(|_| Err(ConnectError::Protocol("connection task stopped".into())));
            let _ = this.update(cx, |tab, cx| {
                if tab.attempt == attempt {
                    tab.finish(result, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn on_request(&mut self, request: UiRequest, cx: &mut Context<Self>) {
        match request {
            UiRequest::HostKey(key, reply) => {
                let check = self.app.upgrade().and_then(|app| app.update(cx, |app, cx| app.check_host_key(&key, cx)));
                let Some(check) = check else {
                    // Vault locked mid-connect: refuse rather than guess.
                    let _ = reply.send(HostKeyDecision::Stop);
                    return;
                };
                self.note(
                    format!("Server host key type: {}", key.public_key.split_whitespace().next().unwrap_or("")),
                    false,
                );
                match check.status {
                    KeyStatus::Trusted => {
                        self.note("Host key verified against Known hosts", false);
                        self.note_offer();
                        let _ = reply.send(HostKeyDecision::Continue);
                    }
                    KeyStatus::Unknown if check.policy == UnknownHostKeys::Refuse => {
                        let _ = reply.send(HostKeyDecision::Stop);
                        self.fail(
                            Failure::RefusedUnknown,
                            "Holonet refused an unknown host key",
                            "Settings are set to refuse servers whose key is not already in Known hosts.",
                        );
                    }
                    KeyStatus::Unknown => {
                        self.note("Paused: waiting for host key verification", true);
                        self.state = State::HostKey { key, reply: Some(reply) };
                    }
                    KeyStatus::Changed { trusted_key } => {
                        // decisions.md: always stop; never send credentials.
                        let _ = reply.send(HostKeyDecision::Stop);
                        self.note("Stopped: the host key has changed", true);
                        if let Some(app) = self.app.upgrade() {
                            app.update(cx, |app, cx| app.record_mismatch(&key, cx));
                        }
                        self.state = State::KeyChanged {
                            presented: key,
                            trusted_key,
                            trusted_at: check.trusted_at,
                            expected: false,
                        };
                    }
                }
            }
            UiRequest::Secret(request, reply) => {
                let field = cx.new(|cx| TextField::new(cx, "").password());
                self._subscriptions.push(cx.subscribe(&field, |this: &mut Self, _, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Submitted => this.answer_secret(true, cx),
                    TextFieldEvent::Cancelled => this.answer_secret(false, cx),
                    TextFieldEvent::Changed => {}
                }));
                let save = !matches!(request, SecretRequest::Challenge { .. });
                self.state = State::Secret { request, field, save, reply: Some(reply) };
                self.focus_pending = true;
            }
        }
        cx.notify();
    }

    fn answer_host_key(&mut self, decision: &'static str, cx: &mut Context<Self>) {
        let State::HostKey { key, reply } = &mut self.state else { return };
        let key = key.clone();
        let reply = reply.take();
        match decision {
            "trust" => {
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| app.trust_host_key(&key, cx));
                }
                self.note("Trusted: the key is saved in Known hosts", false);
                self.note_offer();
                if let Some(r) = reply {
                    let _ = r.send(HostKeyDecision::Continue);
                }
                self.state = State::Connecting;
            }
            "once" => {
                self.note("Connecting once: the key is not saved", false);
                self.note_offer();
                if let Some(r) = reply {
                    let _ = r.send(HostKeyDecision::Continue);
                }
                self.state = State::Connecting;
            }
            _ => {
                if let Some(r) = reply {
                    let _ = r.send(HostKeyDecision::Stop);
                }
                cx.emit(SessionEvent::Close);
            }
        }
        cx.notify();
    }

    fn answer_secret(&mut self, ok: bool, cx: &mut Context<Self>) {
        let State::Secret { request, field, save, reply } = &mut self.state else { return };
        let value = field.read(cx).text().to_string();
        let reply = reply.take();
        if !ok || value.is_empty() {
            if let Some(r) = reply {
                let _ = r.send(None);
            }
            if !ok {
                cx.emit(SessionEvent::Close);
            }
            return;
        }
        let secret = Secret::new(value);
        let is_password = matches!(request, SecretRequest::Password { .. });
        if !matches!(request, SecretRequest::Challenge { .. }) {
            self.typed_secret = Some((is_password, secret.clone()));
        }
        if *save {
            // Saved only once the server accepts it (see `finish`), so a
            // mistyped password never ends up in the vault.
            let is_password = matches!(request, SecretRequest::Password { .. });
            self.pending_save = Some((is_password, secret.clone()));
        }
        if let Some(r) = reply {
            let _ = r.send(Some(secret));
        }
        self.state = State::Connecting;
        cx.notify();
    }

    fn finish(&mut self, result: Result<(Session, Shell), ConnectError>, cx: &mut Context<Self>) {
        match result {
            Ok((session, shell)) => {
                if let Some(typed) = self.typed_secret.take() {
                    self.session_secret = Some(typed);
                }
                if self.reconnect.take().is_some() {
                    self.reconnect_task = None;
                    let time = jiff::Timestamp::now()
                        .to_zoned_datetime(jiff::tz::TimeZone::system())
                        .strftime("%H:%M")
                        .to_string();
                    let line = format!(
                        "\r\n\x1b[2m-- Reconnected at {time}. The shell restarted; the output above is from the previous session. --\x1b[0m\r\n"
                    );
                    self.terminal.update(cx, |t, _| t.feed(line.as_bytes()));
                }
                self.info = session.info().clone();
                self.note(format!("Signed in ({})", self.info.auth_method), false);
                self.session = Some(Arc::new(session));
                self.ask_password = false;
                self.ask_passphrase = false;
                self.terminal.update(cx, |t, cx| t.attach(shell, cx));
                self.state = State::Connected;
                self.focus_pending = true;
                let id = self.host.id;
                self.host.last_connected_at = Some(Timestamp::now());
                let save = self.pending_save.take();
                if let Some((is_password, secret)) = &save {
                    // Keep this tab's copy of the host in step with the vault.
                    match &mut self.host.auth {
                        Auth::Password { saved } if *is_password => *saved = Some(secret.clone()),
                        Auth::Key { passphrase, .. } if !*is_password => *passphrase = Some(secret.clone()),
                        _ => {}
                    }
                }
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.update_vault(
                            move |d| {
                                if let Some(h) = d.host_mut(id) {
                                    h.last_connected_at = Some(Timestamp::now());
                                    if let Some((is_password, secret)) = save {
                                        match &mut h.auth {
                                            Auth::Password { saved } if is_password => *saved = Some(secret),
                                            Auth::Key { passphrase, .. } if !is_password => *passphrase = Some(secret),
                                            _ => {}
                                        }
                                    }
                                }
                            },
                            cx,
                        )
                    });
                }
            }
            Err(e) => {
                // A host key decision already set its own state; a changed
                // key ends reconnecting too (decisions.md: always stop).
                if matches!(self.state, State::KeyChanged { .. } | State::Failed { .. }) {
                    self.reconnect = None;
                    self.reconnect_task = None;
                    cx.notify();
                    return;
                }
                if self.reconnect.is_some() && self.reconnect_failed(&e, cx) {
                    cx.notify();
                    return;
                }
                let name = self.host.display_name().to_string();
                let user =
                    if self.host.username.is_empty() { "this user".to_string() } else { self.host.username.clone() };
                match &e {
                    ConnectError::Resolve { .. } | ConnectError::Refused | ConnectError::Network(_) => {
                        self.note(describe_error(&e), true);
                        self.fail(Failure::Network, format!("Holonet could not reach {name}"), describe_error(&e))
                    }
                    ConnectError::TimedOut(s) => {
                        self.note(format!("No response after {s} seconds"), true);
                        let body = unanswered_reason(self.host.address.trim());
                        self.fail(Failure::TimedOut, format!("{name} did not answer"), body)
                    }
                    ConnectError::SignInRefused { method, remaining } => {
                        let offered = if remaining.is_empty() { method.to_string() } else { remaining.join(",") };
                        self.note(format!("Server: Permission denied ({offered})"), true);
                        if *method == "publickey" {
                            let key = self.key.as_ref().map(|k| k.name.clone()).unwrap_or_default();
                            self.fail(
                                Failure::KeyRefused,
                                format!("The server refused the key {key}"),
                                format!(
                                    "The key may not be in the server's authorized_keys for {user}, or the server may not allow key sign-in for this user."
                                ),
                            )
                        } else {
                            let title = if self.used_saved_password {
                                "The server refused the saved password"
                            } else {
                                "The server refused the password"
                            };
                            self.fail(
                                Failure::Password,
                                title,
                                format!("The password for {user} may have changed, or the server may no longer allow password sign-in."),
                            )
                        }
                    }
                    ConnectError::WrongPassphrase => {
                        let key = self.key.as_ref().map(|k| k.name.clone()).unwrap_or_default();
                        self.note(format!("The passphrase did not unlock {key}"), true);
                        self.fail(
                            Failure::Passphrase,
                            "The key passphrase is wrong",
                            "Enter the passphrase again. Holonet has not contacted the server with this key.",
                        )
                    }
                    ConnectError::Cancelled | ConnectError::HostKeyRejected => {
                        cx.emit(SessionEvent::Close);
                    }
                    other => {
                        self.note(other.to_string(), true);
                        self.fail(Failure::Other, "The connection failed", other.to_string())
                    }
                }
            }
        }
        cx.notify();
    }

    fn fail(&mut self, kind: Failure, title: impl Into<SharedString>, body: impl Into<SharedString>) {
        self.state = State::Failed { kind, title: title.into(), body: body.into() };
    }

    fn replace_key_and_connect(&mut self, cx: &mut Context<Self>) {
        let State::KeyChanged { presented, .. } = &self.state else { return };
        let key = presented.clone();
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.trust_host_key(&key, cx));
        }
        self.note("The trusted key was replaced", true);
        self.start(cx);
    }

    /// Close the connection (on tab close).
    pub fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            self.runtime.spawn(async move {
                let _ = session.close().await;
            });
        }
    }

    // ---------------------------------------------------------- render --

    fn center_card(&self, content: impl IntoElement) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(18.))
            .gap(theme::SPACE_4)
            .child(
                div().flex().flex_col().text_mono().text_color(theme::ink_dim()).children(
                    self.log
                        .iter()
                        .map(|(line, warn)| div().when(*warn, |d| d.text_color(theme::warn())).child(line.clone())),
                ),
            )
            // Scrolls when a card is taller than the pane (small windows,
            // the expanded changed-key disclosure).
            .child(
                div()
                    .id("session-center")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    // Equal spacers centre the card; they shrink to nothing
                    // when it is taller than the pane, which then scrolls.
                    .child(div().flex_1())
                    // Full width so card text wraps at its real width when its
                    // height is measured (otherwise the card is cut short).
                    .child(
                        div()
                            .flex_none()
                            .w_full()
                            .flex()
                            .items_start()
                            .justify_center()
                            .py(theme::SPACE_4)
                            .child(content),
                    )
                    .child(div().flex_1()),
            )
    }

    fn connecting_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let name = self.name();
        let spinner = ring_spinner(px(28.), px(3.));
        self.center_card(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::SPACE_3)
                .child(spinner)
                .child(
                    div()
                        .text_heading()
                        .text_color(theme::ink())
                        .child(SharedString::from(format!("Connecting to {name}"))),
                )
                .child(
                    div()
                        .text_body()
                        .text_color(theme::ink_muted())
                        .child(SharedString::from(format!("Trying {}.", self.host.address.trim()))),
                )
                .child(Button::secondary("connect-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.attempt += 1;
                    cx.emit(SessionEvent::Close)
                }))),
        )
        .into_any_element()
    }

    /// ConnectStates.dc.html, "Reconnecting": a banner over the old output.
    fn reconnecting_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.reconnect else { return div().into_any_element() };
        let total = RECONNECT_BACKOFF.len();
        let progress = match r.next_at {
            Some(at) => {
                let secs = at.saturating_duration_since(std::time::Instant::now()).as_secs() + 1;
                let tried =
                    if r.attempt == 0 { String::new() } else { format!("Attempt {} of {total} failed. ", r.attempt) };
                format!(" {tried}Trying again in {secs} s.")
            }
            None => format!(" Reconnecting, attempt {} of {total}.", r.attempt),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .px(theme::SPACE_4)
                    .py(theme::SPACE_3)
                    .bg(theme::card())
                    .border_b_1()
                    .border_color(theme::line())
                    .child(ring_spinner(px(16.), px(2.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .text_body()
                            .text_color(theme::ink_soft())
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::ink())
                                            .child(r.cause.clone()),
                                    )
                                    .child(SharedString::from(progress)),
                            )
                            .children(
                                r.last_error.clone().map(|e| div().text_meta().text_color(theme::ink_muted()).child(e)),
                            ),
                    )
                    .child(
                        Button::secondary("reconnect-now", "Reconnect now")
                            .compact()
                            .disabled(r.next_at.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.reconnect_now(cx))),
                    )
                    .child(
                        Button::quiet("reconnect-close", self.close_label())
                            .compact()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Close))),
                    ),
            )
            .child(div().flex_1().min_h_0().opacity(0.75).child(self.terminal.clone()))
            .child(
                div()
                    .flex_none()
                    .px(theme::SPACE_4)
                    .py(theme::SPACE_2)
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .child("Scrollback is kept. The shell session itself restarts after reconnecting."),
            )
            .into_any_element()
    }

    fn host_key_view(&self, key: &PresentedKey, cx: &mut Context<Self>) -> AnyElement {
        let name = self.name();
        let user = if self.host.username.is_empty() { String::new() } else { format!(" · {}", self.host.username) };
        let fingerprint = key.fingerprint.clone();
        let card = div()
            .w_full()
            .max_w(px(560.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(28.))
            .bg(theme::card())
            .border_1()
            .border_color(theme::line_strong())
            .rounded(theme::RADIUS_XL)
            .shadow(theme::shadow_dialog())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(icon("shield-alert", px(28.), theme::accent()))
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::ink())
                            .child("Verify this host before connecting"),
                    ),
            )
            .child(div().text_body().text_color(theme::ink_muted()).child(SharedString::from(format!(
                "This is the first time Holonet has connected to {name}. Check the fingerprint against one you got from the server's administrator or console."
            ))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .p(px(14.))
                    .bg(theme::ground())
                    .border_1()
                    .border_color(theme::line())
                    .rounded(theme::RADIUS_MD)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_meta()
                            .text_color(theme::ink_muted())
                            .child(SharedString::from(format!("{} fingerprint", key.key_type_label())))
                            .child(
                                div()
                                    .id("copy-fingerprint")
                                    .text_color(theme::accent())
                                    .cursor_pointer()
                                    .child("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(fingerprint.clone()))
                                    }),
                            ),
                    )
                    .child(div().text_mono().text_size(px(14.)).text_color(theme::ink()).child(SharedString::from(key.fingerprint.clone())))
                    .child(
                        div()
                            .text_mono_small()
                            .text_color(theme::ink_muted())
                            .child(SharedString::from(format!("{}{user}", self.target()))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::SPACE_2)
                    .child(div().flex_grow().min_w(px(180.)).child(
                        Button::primary("trust", "Trust and connect")
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| this.answer_host_key("trust", cx))),
                    ))
                    .child(div().flex_grow().min_w(px(120.)).child(
                        Button::secondary("once", "Connect once")
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| this.answer_host_key("once", cx))),
                    ))
                    .child(div().flex_grow().min_w(px(100.)).child(
                        Button::quiet("cancel-key", "Cancel")
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| this.answer_host_key("cancel", cx))),
                    )),
            )
            .child(div().text_meta().text_color(theme::ink_muted()).child(
                "Trusting saves the key to Known hosts. Connect once does not save it, so you will be asked again next time.",
            ));
        self.center_card(card).into_any_element()
    }

    fn key_changed_view(
        &self,
        presented: &PresentedKey,
        trusted_key: &str,
        trusted_at: Option<Timestamp>,
        expected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self.name();
        let fp_card = |label: &str, fp: String, meta: String, warn: bool| {
            div()
                .flex_1()
                .min_w(px(220.))
                .flex()
                .flex_col()
                .gap(theme::SPACE_1)
                .p(px(14.))
                .bg(theme::ground())
                .border_1()
                .border_color(if warn { theme::warn() } else { theme::line() })
                .rounded(theme::RADIUS_MD)
                .child(
                    div()
                        .text_meta()
                        .text_color(if warn { theme::warn() } else { theme::ink_muted() })
                        .child(SharedString::from(label.to_string())),
                )
                .child(div().text_mono().text_color(theme::ink()).child(SharedString::from(fp)))
                .child(div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(meta)))
        };
        let trusted_fp = holonet_ssh::fingerprint(trusted_key).unwrap_or_default();
        let saved = trusted_at.map(|t| format!("Saved {}", crate::workspace::format_date(t))).unwrap_or_default();
        let card = div()
            .w_full()
            .max_w(px(600.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(28.))
            .bg(theme::warn_surface())
            .border_1()
            .border_color(theme::warn())
            .rounded(theme::RADIUS_XL)
            .shadow(theme::shadow_dialog())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(icon("warning", px(28.), theme::warn()))
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::ink())
                            .child(SharedString::from(format!("The host key for {name} has changed"))),
                    ),
            )
            .child(div().text_body().text_color(theme::warn_ink()).child(
                "The server may have been rebuilt, or something between you and it may be intercepting the connection. Holonet has not sent your credentials.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(fp_card("Trusted in Known hosts", trusted_fp, saved, false))
                    .child(fp_card(
                        "Presented now",
                        presented.fingerprint.clone(),
                        format!("{} · {}", presented.key_type_label(), self.target()),
                        true,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::SPACE_2)
                    .child(div().flex_grow().min_w(px(160.)).child(
                        Button::new("disconnect", "Disconnect", ButtonKind::DestructiveFill)
                            .full_width()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Close))),
                    ))
                    .child(div().flex_grow().min_w(px(180.)).child(
                        Button::secondary("review-known", "Review in Known hosts")
                            .full_width()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::ReviewKnownHost))),
                    )),
            )
            .child(
                div()
                    .id("expected")
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .py(theme::SPACE_3)
                    .cursor_pointer()
                    .text_body()
                    .text_color(theme::warn_ink())
                    .child(icon(if expected { "chevron-down" } else { "chevron-up" }, px(14.), theme::warn_ink()))
                    .child("I expected this change")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let State::KeyChanged { expected, .. } = &mut this.state {
                            *expected = !*expected;
                        }
                        cx.notify()
                    })),
            )
            .when(expected, |d| {
                d.child(div().text_body().text_color(theme::warn_ink()).child(
                    "Only continue if you have confirmed the new fingerprint with whoever runs the server.",
                ))
                .child(div().flex().child(
                    Button::new("replace-key", "Replace trusted key and connect", ButtonKind::DestructiveOutline)
                        .on_click(cx.listener(|this, _, _, cx| this.replace_key_and_connect(cx))),
                ))
            });
        self.center_card(card).into_any_element()
    }

    fn secret_view(
        &self,
        request: &SecretRequest,
        field: &Entity<TextField>,
        save: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, body, save_label): (SharedString, SharedString, Option<&str>) = match request {
            SecretRequest::Password { user, host } => (
                format!("Enter the password for {user}@{host}").into(),
                "This host has no saved password.".into(),
                Some("Save in the encrypted vault"),
            ),
            SecretRequest::Passphrase { key_name } => (
                format!("Enter the passphrase for {key_name}").into(),
                "The key is protected with a passphrase.".into(),
                Some("Save passphrase in the encrypted vault"),
            ),
            SecretRequest::Challenge { title, instructions, prompt, .. } => (
                if title.is_empty() { "The server asks a question".into() } else { title.clone().into() },
                if instructions.is_empty() {
                    prompt.clone().into()
                } else {
                    format!("{instructions}\n{prompt}").into()
                },
                None,
            ),
        };
        let card = crate::ui::dialog(false, title, body)
            .max_w(px(460.))
            .child(field.clone())
            .when_some(save_label, |d, label| {
                let entity = cx.entity();
                d.child(Checkbox::new("save-secret", label, save).on_toggle(move |v, _, cx| {
                    entity.update(cx, |this, cx| {
                        if let State::Secret { save, .. } = &mut this.state {
                            *save = v;
                        }
                        cx.notify()
                    })
                }))
            })
            .child(crate::ui::dialog_buttons([
                Button::quiet("secret-cancel", "Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.answer_secret(false, cx))),
                Button::primary("secret-ok", "Connect")
                    .on_click(cx.listener(|this, _, _, cx| this.answer_secret(true, cx))),
            ]));
        self.center_card(card).into_any_element()
    }

    fn failed_view(
        &self,
        kind: &Failure,
        title: &SharedString,
        body: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut buttons = div().flex().flex_wrap().justify_center().gap(theme::SPACE_2);
        buttons = match kind {
            Failure::Password => buttons.child(Button::primary("enter-password", "Enter password").on_click(
                cx.listener(|this, _, _, cx| {
                    this.ask_password = true;
                    this.start(cx)
                }),
            )),
            Failure::Passphrase => buttons.child(Button::primary("enter-passphrase", "Enter passphrase").on_click(
                cx.listener(|this, _, _, cx| {
                    this.ask_passphrase = true;
                    this.start(cx)
                }),
            )),
            Failure::RefusedUnknown => buttons,
            Failure::Network | Failure::TimedOut | Failure::KeyRefused | Failure::GaveUp | Failure::Other => {
                buttons.child(Button::primary("retry", "Retry").on_click(cx.listener(|this, _, _, cx| this.start(cx))))
            }
        };
        buttons = buttons
            .when(!matches!(kind, Failure::RefusedUnknown), |d| {
                d.child(
                    Button::secondary("edit-host", "Edit host")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::EditHost))),
                )
            })
            .child(
                Button::quiet("close-tab", self.close_label())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Close))),
            );
        self.center_card(
            div()
                .max_w(px(520.))
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::SPACE_3)
                .child(div().text_heading().text_color(theme::ink()).text_center().child(title.clone()))
                .child(div().text_body().text_color(theme::ink_muted()).text_center().child(body.clone()))
                .child(buttons),
        )
        .into_any_element()
    }

    /// The status bar the tab shows for its active pane.
    pub fn status_bar(&self) -> Div {
        let (word, color): (&str, gpui::Rgba) = match self.state {
            _ if self.reconnect.is_some() => ("Reconnecting", theme::ink_muted()),
            State::Connected => ("Connected", theme::group_mint()),
            State::Ended(_) => ("Ended", theme::warn()),
            State::Failed { kind, .. } => (kind.state_word(), theme::warn()),
            State::KeyChanged { .. } => ("Host key changed", theme::warn()),
            _ => ("Connecting", theme::ink_muted()),
        };
        let auth = match self.info.auth_method.as_str() {
            "publickey" => self.key.as_ref().map(|k| format!("Key {}", k.name)).unwrap_or_else(|| "Key".into()),
            "password" | "keyboard-interactive" => "Password".into(),
            _ => String::new(),
        };
        let mut items: Vec<SharedString> = Vec::new();
        if !self.info.cipher.is_empty() {
            items.push(format!("SSH-2.0 · {}", self.info.cipher).into());
            items.push(self.info.kex.clone().into());
        }
        if !auth.is_empty() {
            items.push(auth.into());
        }
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(20.))
            .gap_y(px(6.))
            .px(px(18.))
            .py(theme::SPACE_2)
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::line())
            .text_mono_small()
            .text_color(theme::ink_muted())
            .child(div().text_color(color).child(word))
            .children(items.into_iter().map(|i| div().child(i)))
            .child(div().flex_1())
            .child("xterm-256color · UTF-8")
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_pending) {
            self.focus(window, cx);
        }
        let color = theme::no_group();
        let size = self.terminal.read(cx).grid_size();
        let state_word: SharedString = match &self.state {
            _ if self.reconnect.is_some() => "Reconnecting".into(),
            State::Connected => format!("{} × {}", size.cols, size.rows).into(),
            State::Connecting | State::HostKey { .. } | State::Secret { .. } => "Connecting".into(),
            State::KeyChanged { .. } => "Host key changed".into(),
            State::Failed { kind, .. } => kind.state_word().into(),
            State::Ended(_) => "Ended".into(),
        };
        let warn_word = matches!(self.state, State::KeyChanged { .. } | State::Failed { .. } | State::Ended(_));

        let content: AnyElement = match &self.state {
            State::Connecting if self.reconnect.is_some() => self.reconnecting_view(cx),
            State::Connecting => self.connecting_view(cx),
            State::HostKey { key, .. } => {
                let key = key.clone();
                self.host_key_view(&key, cx)
            }
            State::KeyChanged { presented, trusted_key, trusted_at, expected } => {
                let (p, t, at, e) = (presented.clone(), trusted_key.clone(), *trusted_at, *expected);
                self.key_changed_view(&p, &t, at, e, cx)
            }
            State::Secret { request, field, save, .. } => {
                let (r, f, s) = (request.clone(), field.clone(), *save);
                self.secret_view(&r, &f, s, cx)
            }
            State::Failed { kind, title, body } => {
                let (title, body) = (title.clone(), body.clone());
                let kind = *kind;
                self.failed_view(&kind, &title, &body, cx)
            }
            State::Connected => self.terminal.clone().into_any_element(),
            State::Ended(reason) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().opacity(0.75).child(self.terminal.clone()))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme::SPACE_3)
                        .px(theme::SPACE_4)
                        .py(theme::SPACE_2)
                        .bg(theme::card())
                        .border_t_1()
                        .border_color(theme::line())
                        .child(
                            div()
                                .flex_1()
                                .text_body()
                                .text_color(theme::ink_soft())
                                .child(SharedString::from(format!("Session ended. {reason}"))),
                        )
                        .child(
                            Button::secondary("reconnect", "Reconnect")
                                .compact()
                                .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                        )
                        .child(
                            Button::quiet("close-ended", self.close_label())
                                .compact()
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Close))),
                        ),
                )
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::terminal())
            .border_1()
            .border_color(if self.active { theme::ink_muted() } else { theme::line() })
            .rounded(theme::RADIUS_MD)
            .overflow_hidden()
            // 2px group-color top edge (GPUI borders are one color).
            .child(div().h(px(2.)).flex_none().bg(color))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(theme::SPACE_3)
                    .py(theme::SPACE_2)
                    .bg(theme::panel())
                    .border_b_1()
                    .border_color(theme::line())
                    .text_meta()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::SPACE_2)
                            .child(crate::ui::group_square(color))
                            .child(div().text_color(theme::ink()).font_weight(FontWeight::MEDIUM).child(self.name()))
                            .child(div().text_color(color).child("No group")),
                    )
                    .child(
                        div()
                            .text_mono_small()
                            .text_color(if warn_word { theme::warn() } else { theme::ink_muted() })
                            .child(state_word),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unanswered_reason_spots_private_addresses() {
        for private in ["192.168.1.51", "10.30.0.9", "172.20.1.1", "169.254.1.1", "fd00::1", "nas.local", "pi.lan"] {
            assert!(unanswered_reason(private).contains("private address"), "{private}");
        }
        for public in ["198.51.100.7", "example.com", "2001:db8::1"] {
            assert!(unanswered_reason(public).contains("may be down"), "{public}");
        }
    }

    #[test]
    fn every_failure_has_a_state_word() {
        let words: Vec<&str> = [
            Failure::Network,
            Failure::TimedOut,
            Failure::Password,
            Failure::KeyRefused,
            Failure::Passphrase,
            Failure::RefusedUnknown,
            Failure::GaveUp,
            Failure::Other,
        ]
        .iter()
        .map(Failure::state_word)
        .collect();
        assert_eq!(words[1], "Timed out");
        assert_eq!(words[2], "Sign-in refused");
        assert!(words.iter().all(|w| !w.is_empty()));
    }
}
