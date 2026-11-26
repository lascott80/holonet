//! A pane: one host's connection, from connecting through the host key
//! decision and sign-in to the terminal (Terminal.dc.html, HostKey.dc.html,
//! ConnectStates.dc.html). A [`crate::tab::Tab`] holds one or more panes and
//! draws the session header and status bar for its active pane.
//!
//! The SSH connection runs on Tokio. When russh needs a host key decision or
//! a secret, [`UiPrompts`] sends the request over a channel to this entity and
//! waits for the answer, so the network side and the interface never block
//! each other.
//!
//! A pane in Files mode ([`PaneMode::Files`]) connects the same way, then
//! opens SFTP instead of a shell and shows a [`FilesView`]; a watcher on the
//! session notices a dropped connection, since there is no shell to.
//!
//! A host behind jump hosts connects through them ([`crate::hops`]). Each hop
//! has its own prompts, sign-in state and host key check, and a failure says
//! which hop it belongs to.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, Entity, EventEmitter, FontWeight, IntoElement, Render,
    SharedString, Subscription, Transformation, WeakEntity, Window, div, percentage, prelude::*, px,
};
use holonet_model::{Auth, Host, JumpProblem, Route, Secret, StoredKey, Timestamp, UnknownHostKeys};
use holonet_net::{PlanNote, PlanProblem, Via};
use holonet_ssh::{
    BoxFuture, ConnectError, ConnectOptions, HostKeyDecision, KeyStatus, PresentedKey, Prompts, PtySize, SecretRequest,
    Session, SessionInfo, Shell,
};
use tokio::sync::{mpsc, oneshot};

use crate::app::HolonetApp;
use crate::files_view::FilesView;
use crate::hops::{self, Hop, RouteEvent, Routed};
use crate::host_form::describe_error;
use crate::shell::TabState;
use crate::snippet_run::{RunPrompt, RunPromptEvent};
use crate::terminal::GridSize;
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, ButtonKind, Checkbox, TextField, TextFieldEvent, icon};

// ------------------------------------------------------------ the bridge --

/// Requests carry the hop they belong to: an index into the pane's jump
/// hosts, or their count for the pane's own host.
enum UiRequest {
    HostKey(usize, PresentedKey, oneshot::Sender<HostKeyDecision>),
    Secret(usize, SecretRequest, oneshot::Sender<Option<Secret>>),
    /// The route plan for this attempt: how many addresses, and its notes.
    Plan {
        attempts: usize,
        notes: Vec<PlanNote>,
    },
    /// Starting on an address.
    Attempt {
        via: Via,
        address: String,
    },
    /// A line for the connecting log.
    Note(String, bool),
}

/// How a connection attempt ended, from the connection task.
enum Done {
    /// The route plan allowed no attempt (Tailscale is needed but off).
    Plan(PlanProblem),
    Connect(Box<Result<(Session, Opened), ConnectError>>),
}

/// What a signed-in session opened for the pane.
enum Opened {
    Shell(Shell),
    Files(holonet_files::remote::Remote),
}

/// A terminal, or the SFTP screen (milestone 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneMode {
    Terminal,
    Files,
}

/// Each hop's typed secret, kept from another pane ("Open SFTP here").
pub type Remembered = Vec<Option<(bool, Secret)>>;

/// "the jump host x" at the start of a sentence.
fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// A sentence continued after "but": its first letter lowered, except
/// when it starts with a name ("Tailscale …").
fn lower_first(text: &str) -> String {
    match text.split_once(' ') {
        Some(("Tailscale", _)) => text.to_string(),
        _ => {
            let mut chars = text.chars();
            chars.next().map(|c| c.to_lowercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    }
}

/// [`Prompts`] that forwards each question to the session tab, naming the
/// hop it comes from.
struct UiPrompts {
    tx: mpsc::UnboundedSender<UiRequest>,
    hop: usize,
}

impl Prompts for UiPrompts {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        let (tx, rx) = oneshot::channel();
        let sent = self.tx.send(UiRequest::HostKey(self.hop, key, tx)).is_ok();
        Box::pin(async move { if sent { rx.await.unwrap_or(HostKeyDecision::Stop) } else { HostKeyDecision::Stop } })
    }

    fn secret(&self, request: SecretRequest) -> BoxFuture<Option<Secret>> {
        let (tx, rx) = oneshot::channel();
        let sent = self.tx.send(UiRequest::Secret(self.hop, request, tx)).is_ok();
        Box::pin(async move { if sent { rx.await.ok().flatten() } else { None } })
    }
}

/// One hop's sign-in state in a pane.
#[derive(Default)]
struct SignIn {
    /// Ask for the password even if one is saved (after it was refused).
    ask_password: bool,
    /// Ask for the passphrase even if one is saved (after it was wrong).
    ask_passphrase: bool,
    /// A typed password (true) or passphrase to save after a successful sign-in.
    pending_save: Option<(bool, Secret)>,
    /// The attempt used a password saved in the vault (for the wording of
    /// "The server refused the saved password").
    used_saved_password: bool,
    /// A password (true) or passphrase typed during this attempt, and the one
    /// that worked, kept in memory so reconnecting does not ask again. Never
    /// saved unless "Save in the encrypted vault" was ticked.
    typed: Option<(bool, Secret)>,
    remembered: Option<(bool, Secret)>,
}

impl SignIn {
    fn remembered(&self, password: bool) -> Option<Secret> {
        self.remembered.as_ref().filter(|(p, _)| *p == password).map(|(_, s)| s.clone())
    }
}

// ---------------------------------------------------------------- states --

/// What the vault says about a presented key.
pub struct KeyCheck {
    pub status: KeyStatus,
    pub policy: UnknownHostKeys,
    pub trusted_at: Option<Timestamp>,
    /// The trusted entry is the host's other address (this one has none).
    pub from_alias: Option<String>,
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
    /// The host connects over Tailscale, which cannot be used now.
    TailscaleOff,
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
            Failure::TailscaleOff => "Tailscale off",
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
    /// The host needs Tailscale, which is off: waiting for it does not use
    /// up attempts. Its network change starts the next one at once.
    waiting_for_tailscale: bool,
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
        hop: usize,
        key: PresentedKey,
        reply: Option<oneshot::Sender<HostKeyDecision>>,
    },
    KeyChanged {
        hop: usize,
        presented: PresentedKey,
        trusted_key: String,
        trusted_at: Option<Timestamp>,
        expected: bool,
    },
    Secret {
        hop: usize,
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
    /// With its identity applied (`VaultData::resolved`).
    pub host: Host,
    /// A per-connection route ("Try Tailscale"); `None` uses the host's.
    route_override: Option<Route>,
    /// The address being tried or connected to, and how.
    via: Option<Via>,
    address: String,
    /// This attempt's route plan: how many addresses, its notes, and the
    /// addresses tried so far.
    plan_len: usize,
    plan_notes: Vec<PlanNote>,
    tried: Vec<(Via, String)>,
    key: Option<StoredKey>,
    mode: PaneMode,
    /// The SFTP screen, made at the first connection and kept across
    /// reconnects (it keeps its folders).
    files: Option<Entity<FilesView>>,
    /// Waits for the session to end (Files mode).
    lost_watch: Option<tokio::task::JoinHandle<()>>,
    /// The jump hosts in front of this host, first hop first, or why the
    /// chain cannot be used.
    jumps: Vec<Hop>,
    jump_problem: Option<JumpProblem>,
    /// Each hop's sign-in state: the jump hosts' in order, then this host's.
    signs: Vec<SignIn>,
    /// The hop the current failure belongs to.
    failed_hop: usize,
    runtime: tokio::runtime::Handle,
    limits: Limits,
    state: State,
    log: Vec<(SharedString, bool)>,
    terminal: Entity<TerminalView>,
    session: Option<Arc<Session>>,
    info: SessionInfo,
    /// Bumped on every attempt, so stale results are ignored.
    attempt: u64,
    /// Drawn with the stronger border of the tab's active pane.
    active: bool,
    /// The only pane in its tab: "Close tab" instead of "Close pane".
    alone: bool,
    /// While reconnecting after a lost connection.
    reconnect: Option<Reconnect>,
    /// Waits for and starts the next reconnect attempt.
    reconnect_task: Option<gpui::Task<()>>,
    /// The host's startup snippet has had its one chance: it is sent after
    /// the shell first opens and never replayed (decisions.md).
    startup_done: bool,
    /// Filled snippets from a multi-host run, typed once the shell opens.
    queued: Vec<String>,
    /// A snippet waiting for its run prompt, and the prompt while open.
    ask: Option<(holonet_model::Snippet, bool)>,
    /// The tab is broadcasting (amber edges).
    broadcasting: bool,
    prompt: Option<(Entity<RunPrompt>, Subscription)>,
    /// The last sleep, wake or network change, and when.
    disturbance: Option<(Disturbance, std::time::Instant)>,
    /// Focus the prompt's field at the next render (it is created without a window).
    focus_pending: bool,
    /// This session's History record, from its first connection until it
    /// ends (closed, the shell exits, or reconnecting gives up).
    record: Option<holonet_model::RecordId>,
    /// Its saved output, when the host logs.
    history_log: Option<crate::history::SessionLog>,
    /// Held while no field or terminal in the pane can take focus
    /// (connecting, a card, a failure), so app shortcuts such as Ctrl Shift
    /// T still dispatch from inside the pane.
    focus_handle: gpui::FocusHandle,
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
    /// How long Automatic waits on the primary address (Settings, Connection).
    pub fallback_wait: Duration,
    /// Settings, SFTP (Files mode).
    pub sftp: holonet_model::SftpSettings,
}

impl Pane {
    /// Settings changed: show them in this tab's terminal.
    pub fn set_terminal_settings(&mut self, settings: &holonet_model::TerminalSettings, cx: &mut Context<Self>) {
        self.limits.terminal = settings.clone();
        self.terminal.update(cx, |t, cx| t.set_settings(settings.clone(), cx));
    }

    /// Settings, SFTP changed: the files view follows.
    pub fn set_sftp_settings(&mut self, settings: &holonet_model::SftpSettings, cx: &mut Context<Self>) {
        self.limits.sftp = settings.clone();
        if let Some(files) = self.files.clone() {
            files.update(cx, |f, cx| f.set_settings(settings.clone(), cx));
        }
    }

    #[allow(clippy::too_many_arguments)] // GPUI context plus the connection's inputs
    pub fn new(
        app: WeakEntity<HolonetApp>,
        host: Host,
        key: Option<StoredKey>,
        jumps: Result<Vec<Hop>, JumpProblem>,
        runtime: tokio::runtime::Handle,
        limits: Limits,
        size: GridSize,
        opening: (PaneMode, Remembered),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mode, remembered) = opening;
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
                    // Typing exit ends it as closed by you; a channel the
                    // server closed counts as dropped.
                    let end = if reason.starts_with("The shell exited") {
                        holonet_model::SessionEnd::ClosedByYou
                    } else {
                        holonet_model::SessionEnd::Dropped
                    };
                    this.end_record(end, cx);
                }
                TerminalEvent::Lost(reason) => this.on_lost(reason.clone(), cx),
                // The tab listens for these itself (broadcast).
                TerminalEvent::Typed(_) => return,
            }
            cx.notify();
        })];
        let (jumps, jump_problem) = match jumps {
            Ok(jumps) => (jumps, None),
            Err(problem) => (Vec::new(), Some(problem)),
        };
        let mut tab = Self {
            app,
            host,
            route_override: None,
            via: None,
            // Set by `start`.
            address: String::new(),
            plan_len: 0,
            plan_notes: Vec::new(),
            tried: Vec::new(),
            key,
            mode,
            files: None,
            lost_watch: None,
            // A secret typed in the pane this one was opened from.
            signs: (0..=jumps.len())
                .map(|i| SignIn { remembered: remembered.get(i).cloned().flatten(), ..SignIn::default() })
                .collect(),
            failed_hop: jumps.len(),
            jumps,
            jump_problem,
            runtime,
            limits,
            state: State::Connecting,
            log: Vec::new(),
            terminal,
            session: None,
            info: SessionInfo::default(),
            attempt: 0,
            focus_pending: false,
            record: None,
            history_log: None,
            focus_handle: cx.focus_handle(),
            active: true,
            alone: true,
            reconnect: None,
            reconnect_task: None,
            startup_done: false,
            queued: Vec::new(),
            ask: None,
            broadcasting: false,
            prompt: None,
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

    /// Transfers in this pane's SFTP view that closing it would stop.
    pub fn unfinished_transfers(&self, cx: &gpui::App) -> usize {
        self.files.as_ref().map_or(0, |f| f.read(cx).unfinished())
    }

    /// Closing: stop this pane's transfers (before [`Self::disconnect`]).
    pub fn stop_transfers(&mut self, cx: &mut Context<Self>) {
        if let Some(files) = self.files.clone() {
            files.update(cx, |f, _| f.stop_all());
        }
    }

    pub fn is_files(&self) -> bool {
        self.mode == PaneMode::Files
    }

    /// Each hop's typed secret, for a pane opened from this one.
    pub fn remembered(&self) -> Remembered {
        self.signs.iter().map(|s| s.remembered.clone()).collect()
    }

    /// The SFTP view, or the terminal: what the pane shows once connected.
    fn body(&self) -> AnyElement {
        match &self.files {
            Some(files) if self.mode == PaneMode::Files => files.clone().into_any_element(),
            _ => self.terminal.clone().into_any_element(),
        }
    }

    /// Notice the connection dropping (Files mode has no shell to say so).
    fn watch_lost(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else { return };
        if let Some(old) = self.lost_watch.take() {
            old.abort();
        }
        let (tx, rx) = oneshot::channel();
        self.lost_watch = Some(self.runtime.spawn(async move {
            if let Some(reason) = session.closed().await {
                let _ = tx.send(reason);
            }
        }));
        let attempt = self.attempt;
        cx.spawn(async move |this, cx| {
            let Ok(reason) = rx.await else { return };
            let _ = this.update(cx, |pane, cx| {
                if pane.attempt != attempt {
                    return;
                }
                if let Some(files) = pane.files.clone() {
                    files.update(cx, |f, cx| f.set_remote(None, cx));
                }
                pane.on_lost(reason, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.state {
            State::Connected | State::Ended(_) if self.mode == PaneMode::Files => window.focus(&self.focus_handle),
            State::Connected | State::Ended(_) => self.terminal.read(cx).focus(window),
            State::Secret { field, .. } => field.read(cx).focus(window),
            _ => window.focus(&self.focus_handle),
        }
    }

    fn note(&mut self, line: impl Into<SharedString>, warn: bool) {
        self.log.push((line.into(), warn));
        // Two more lines for each jump host's key check and sign-in.
        if self.log.len() > 8 + 2 * self.jumps.len() {
            self.log.remove(0);
        }
    }

    /// "Offering password for root", once the host key is settled.
    fn note_offer(&mut self, hop: usize) {
        let host = self.hop_host(hop);
        let user = if host.username.is_empty() { "this user".to_string() } else { host.username.clone() };
        let line = match (&host.auth, self.hop_key(hop)) {
            (Auth::Key { .. }, Some(key)) => format!("Offering key {} for {user}", key.name),
            (Auth::Agent, _) => format!("Offering the agent's keys for {user}"),
            _ => format!("Offering password for {user}"),
        };
        let line = format!("{}{line}", self.hop_prefix(hop));
        self.note(line, false);
    }

    // -------------------------------------------------------------- hops --

    /// The hop index of this pane's own host (after its jump hosts).
    fn target_hop(&self) -> usize {
        self.jumps.len()
    }

    fn is_jump(&self, hop: usize) -> bool {
        hop < self.jumps.len()
    }

    fn hop_host(&self, hop: usize) -> &Host {
        self.jumps.get(hop).map(|j| &j.host).unwrap_or(&self.host)
    }

    fn hop_key(&self, hop: usize) -> Option<&StoredKey> {
        match self.jumps.get(hop) {
            Some(j) => j.key.as_ref(),
            None => self.key.as_ref(),
        }
    }

    fn hop_name(&self, hop: usize) -> String {
        self.hop_host(hop).display_name().to_string()
    }

    /// "the jump host bastion-east" or "app-01".
    fn hop_title(&self, hop: usize) -> String {
        if self.is_jump(hop) { format!("the jump host {}", self.hop_name(hop)) } else { self.hop_name(hop) }
    }

    /// "bastion-east: " before a log line, when there are several hops.
    fn hop_prefix(&self, hop: usize) -> String {
        if self.jumps.is_empty() { String::new() } else { format!("{}: ", self.hop_name(hop)) }
    }

    /// The host the route plan applies to: the first jump host, or this one.
    fn route_host(&self) -> &Host {
        self.hop_host(0)
    }

    /// The pane's host was edited, or one of its jump hosts.
    pub fn depends_on(&self, id: holonet_model::HostId) -> bool {
        self.host.id == id || self.jumps.iter().any(|j| j.host.id == id)
    }

    /// The host to open from "Edit host": the hop that failed.
    pub fn host_to_edit(&self) -> holonet_model::HostId {
        match &self.state {
            State::Failed { .. } => self.hop_host(self.failed_hop).id,
            _ => self.host.id,
        }
    }

    /// The Known hosts entry to review: the address whose key changed.
    pub fn review_address(&self) -> (String, u16) {
        match &self.state {
            State::KeyChanged { presented, .. } => (presented.host.clone(), presented.port),
            _ => (self.host.address.trim().to_string(), self.host.port),
        }
    }

    /// The address a connection starts with: the Tailscale address when the
    /// route (or "Try Tailscale") is Tailscale, otherwise the primary one.
    /// Behind jump hosts, the address they reach this host at.
    fn initial_address(&self) -> String {
        if !self.jumps.is_empty() {
            return hops::reach_address(&self.host);
        }
        match self.route_override.unwrap_or(self.host.effective_route()) {
            Route::Tailscale => self.host.tailscale().unwrap_or(self.host.address.trim()).to_string(),
            _ => self.host.address.trim().to_string(),
        }
    }

    /// The address being used (primary or Tailscale) and port.
    fn target(&self) -> String {
        format!("{}:{}", self.address, self.host.port)
    }

    /// Where the pane sits in its tab (set by [`crate::tab::Tab`]).
    pub fn set_place(&mut self, active: bool, alone: bool, cx: &mut Context<Self>) {
        if (self.active, self.alone) != (active, alone) {
            (self.active, self.alone) = (active, alone);
            cx.notify();
        }
    }

    pub fn terminal(&self) -> &Entity<TerminalView> {
        &self.terminal
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

    /// Sleep, wake or a network change (from the app's system listener).
    pub fn on_system_event(&mut self, event: crate::platform::SystemEvent, cx: &mut Context<Self>) {
        use crate::platform::SystemEvent;
        let kind = match event {
            SystemEvent::Suspend | SystemEvent::Resume => Disturbance::Sleep,
            SystemEvent::NetworkChanged => Disturbance::Network,
            // Windows locking does not touch connections.
            SystemEvent::SessionLocked | SystemEvent::SessionUnlocked => return,
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
            self.end_record(holonet_model::SessionEnd::Dropped, cx);
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
            (None, holonet_ssh::LostReason::JumpHost { host, .. }) => {
                let name = self
                    .jumps
                    .iter()
                    .find(|j| j.host.address.trim() == host || j.host.tailscale() == Some(host.as_str()))
                    .map(|j| j.name().to_string())
                    .unwrap_or_else(|| host.clone());
                format!("The connection to the jump host {name} was lost.").into()
            }
            (None, _) => "The connection was lost.".into(),
        };
        self.reconnect =
            Some(Reconnect { attempt: 0, cause, next_at: None, last_error: None, waiting_for_tailscale: false });
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
            self.end_record(holonet_model::SessionEnd::Dropped, cx);
            self.fail(
                Failure::GaveUp,
                format!("Holonet could not reconnect to {}", self.host.display_name()),
                format!("It tried {} times. {last}", RECONNECT_BACKOFF.len()),
            );
            cx.notify();
            return;
        };
        // Waiting for Tailscale: its network change starts the next attempt,
        // so only check now and then.
        let wait = if r.waiting_for_tailscale { 16 } else { wait };
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
        r.waiting_for_tailscale = false;
        self.reconnect_task = None;
        self.start(cx);
    }

    /// A reconnect attempt failed. Network trouble waits and tries again;
    /// anything else (a refused sign-in) ends reconnecting and shows the
    /// usual failure. Returns true when handled here.
    fn reconnect_failed(&mut self, e: &ConnectError, cx: &mut Context<Self>) -> bool {
        // A jump host's error is judged by what happened at that hop, and
        // the banner names that hop.
        let (e, wrappers) = e.innermost();
        let prefix = self.hop_prefix(self.jumps.len().saturating_sub(wrappers.len()));
        let retry = hops::unreachable(e)
            || matches!(e, ConnectError::JumpCouldNotReach { .. })
            || (matches!(e, ConnectError::HostKeyRejected)
                && self.app.upgrade().is_some_and(|a| !a.read(cx).is_unlocked()));
        if !retry {
            self.reconnect = None;
            self.reconnect_task = None;
            return false;
        }
        let message: SharedString = match e {
            ConnectError::HostKeyRejected => "Unlock the vault so Holonet can check the host key.".into(),
            other => format!("{prefix}{}", describe_error(other)).into(),
        };
        if let Some(r) = self.reconnect.as_mut() {
            r.last_error = Some(message);
        }
        self.state = State::Connecting;
        self.schedule_reconnect(cx);
        true
    }

    /// History's Route column: the jump hosts, else the address used.
    fn route_label(&self) -> String {
        if !self.jumps.is_empty() {
            return self.jumps.iter().map(|j| j.name()).collect::<Vec<_>>().join(", ");
        }
        match self.via {
            Some(Via::Tailscale) => "Tailscale".into(),
            _ => "Primary".into(),
        }
    }

    /// The session is over: end its record and seal its log. Deferred, as
    /// this can run while the app is mid-update (closing a tab).
    fn end_record(&mut self, end: holonet_model::SessionEnd, cx: &mut Context<Self>) {
        let Some(id) = self.record.take() else { return };
        let log = self.history_log.take();
        if log.is_some() {
            self.terminal.update(cx, |t, _| t.set_log(None));
        }
        let app = self.app.clone();
        cx.defer(move |cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| app.end_record(id, end, log, cx));
            }
        });
    }

    /// Whether the pane stopped without a working session (failed or ended).
    pub fn is_stopped(&self) -> bool {
        matches!(self.state, State::Failed { .. } | State::Ended(_))
    }

    /// The host was edited from this pane's failure ("Edit host", then "Save
    /// and connect"): try again here with the new details.
    pub fn retry_with(
        &mut self,
        host: Host,
        key: Option<StoredKey>,
        jumps: Result<Vec<Hop>, JumpProblem>,
        limits: Limits,
        cx: &mut Context<Self>,
    ) {
        self.host = host;
        self.route_override = None;
        self.key = key;
        (self.jumps, self.jump_problem) = match jumps {
            Ok(jumps) => (jumps, None),
            Err(problem) => (Vec::new(), Some(problem)),
        };
        self.signs = (0..=self.jumps.len()).map(|_| SignIn::default()).collect();
        self.limits = limits;
        self.start(cx);
        cx.notify();
    }

    /// Start (or restart) the connection, through the jump hosts if any.
    fn start(&mut self, cx: &mut Context<Self>) {
        self.attempt += 1;
        let attempt = self.attempt;
        for sign in &mut self.signs {
            sign.pending_save = None;
            sign.typed = None;
        }
        self.failed_hop = self.target_hop();
        self.state = State::Connecting;
        self.session = None;
        // Each attempt starts a fresh log (ConnectStates.dc.html).
        self.log.clear();
        self.tried.clear();
        self.plan_notes.clear();
        self.plan_len = 0;
        // Until the plan names one: the address this route starts with.
        // `via` keeps the last attempt's, so a reconnect over Tailscale still
        // says so.
        self.address = self.initial_address();
        if let Some(problem) = self.jump_problem {
            let name = self.host.display_name().to_string();
            let title = match problem {
                JumpProblem::Loop => format!("The jump hosts for {name} lead back to it"),
                JumpProblem::Missing => format!("A jump host for {name} was deleted"),
            };
            self.fail(Failure::Other, title, "Edit the host to choose another jump host, or None.");
            return;
        }

        // Not read from the app: `start` runs while the app creates this tab.
        let (connect_timeout, keep_alive) = (self.limits.connect_timeout, self.limits.keep_alive);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let target = Hop { host: self.host.clone(), key: self.key.clone(), keep_alive };
        let mut steps = Vec::new();
        let mut missing = None;
        for (hop, step) in self.jumps.iter().chain(std::iter::once(&target)).enumerate() {
            let sign = &mut self.signs[hop];
            sign.used_saved_password =
                matches!(&step.host.auth, Auth::Password { saved: Some(_) }) && !sign.ask_password;
            let sign = &*sign;
            let Some(credential) = step.credential(sign.ask_password, sign.ask_passphrase, |p| sign.remembered(p))
            else {
                let (title, body) = if hop == self.jumps.len() {
                    (
                        "The key for this host is missing".to_string(),
                        "It is no longer in the vault. Edit the host to choose or import a key.".to_string(),
                    )
                } else {
                    let name = step.name();
                    (
                        format!("The key for the jump host {name} is missing"),
                        format!("It is no longer in the vault. Edit {name} to choose or import a key."),
                    )
                };
                missing = Some((hop, title, body));
                break;
            };
            steps.push(ConnectOptions {
                // The first hop's address comes from the route plan.
                host: hops::reach_address(&step.host),
                port: step.host.port,
                user: step.host.username.clone(),
                credential,
                connect_timeout,
                keep_alive: step.keep_alive,
                prompts: Arc::new(UiPrompts { tx: tx.clone(), hop }),
                // Only this pane's own host gets the agent (`ssh -A` on the last hop).
                agent_forwarding: hop == self.jumps.len() && step.host.agent_forwarding,
                compression: step.host.compression,
                jump: None,
            });
        }
        if let Some((hop, title, body)) = missing {
            self.failed_hop = hop;
            self.fail(Failure::Other, title, body);
            return;
        }
        let opts = hops::link(steps);
        let size = self.terminal.read(cx).grid_size();
        let (done_tx, done_rx) = oneshot::channel::<Done>();
        let route_host = self.route_host().clone();
        let mode = self.mode;
        let (route_override, fallback_wait, jumps) = (self.route_override, self.limits.fallback_wait, self.jumps.len());
        let ui = tx;
        self.runtime.spawn(async move {
            let routed = hops::connect_by_route(opts, &route_host, route_override, fallback_wait, jumps, |event| {
                let _ = ui.send(match event {
                    RouteEvent::Plan { attempts, notes } => UiRequest::Plan { attempts, notes },
                    RouteEvent::Attempt { via, address } => UiRequest::Attempt { via, address },
                    RouteEvent::Unreachable(line) => UiRequest::Note(line, true),
                });
            })
            .await;
            let done = match routed {
                Routed::Plan(problem) => Done::Plan(problem),
                Routed::Connect(result) => match *result {
                    Err(e) => Done::Connect(Box::new(Err(e))),
                    Ok(session) if mode == PaneMode::Files => {
                        let files = holonet_files::remote::Remote::open(&session).await;
                        Done::Connect(Box::new(match files {
                            Ok(remote) => Ok((session, Opened::Files(remote))),
                            Err(e) => {
                                let _ = session.close().await;
                                Err(ConnectError::Protocol(e.to_string()))
                            }
                        }))
                    }
                    Ok(session) => {
                        let shell = session.open_shell(PtySize::cells(size.cols as u16, size.rows as u16)).await;
                        Done::Connect(Box::new(match shell {
                            Ok(shell) => Ok((session, Opened::Shell(shell))),
                            Err(e) => {
                                let _ = session.close().await;
                                Err(ConnectError::Protocol(format!("{e:#}")))
                            }
                        }))
                    }
                },
            };
            let _ = done_tx.send(done);
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
            let done = done_rx.await.unwrap_or_else(|_| {
                Done::Connect(Box::new(Err(ConnectError::Protocol("connection task stopped".into()))))
            });
            let _ = this.update(cx, |tab, cx| {
                if tab.attempt == attempt {
                    match done {
                        Done::Plan(problem) => tab.plan_failed(problem, cx),
                        Done::Connect(result) => tab.finish(*result, cx),
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn on_request(&mut self, request: UiRequest, cx: &mut Context<Self>) {
        match request {
            UiRequest::Plan { attempts, notes } => {
                self.plan_len = attempts;
                if notes.contains(&PlanNote::PrimarySkipped) {
                    let line = format!(
                        "{}{} is a private address with no route from this network",
                        self.hop_prefix(0),
                        self.route_host().address.trim()
                    );
                    self.note(line, false);
                }
                self.plan_notes = notes;
            }
            UiRequest::Attempt { via, address } => {
                self.via = Some(via);
                // Behind jump hosts the header keeps this host's own address.
                if self.jumps.is_empty() {
                    self.address = address.clone();
                }
                self.tried.push((via, address.clone()));
                let prefix = self.hop_prefix(0);
                if address.parse::<std::net::IpAddr>().is_err() {
                    self.note(format!("{prefix}Resolving {address} …"), false);
                }
                let over = if via == Via::Tailscale { " over Tailscale" } else { "" };
                let port = self.route_host().port;
                self.note(format!("{prefix}Opening TCP connection to {address}:{port}{over}"), false);
            }
            UiRequest::Note(line, warn) => self.note(line, warn),
            UiRequest::HostKey(hop, key, reply) => {
                let prefix = self.hop_prefix(hop);
                let aliases = holonet_model::VaultData::other_addresses(self.hop_host(hop), &key.host);
                let check =
                    self.app.upgrade().and_then(|app| app.update(cx, |app, cx| app.check_host_key(&key, &aliases, cx)));
                let Some(check) = check else {
                    // Vault locked mid-connect: refuse rather than guess.
                    let _ = reply.send(HostKeyDecision::Stop);
                    return;
                };
                self.note(
                    format!("{prefix}Server host key type: {}", key.public_key.split_whitespace().next().unwrap_or("")),
                    false,
                );
                match check.status {
                    KeyStatus::Trusted => {
                        match &check.from_alias {
                            Some(other) => self.note(
                                format!(
                                    "{prefix}Host key matches the one trusted for {other}; trusted for this address too"
                                ),
                                false,
                            ),
                            None => self.note(format!("{prefix}Host key verified against Known hosts"), false),
                        }
                        self.note_offer(hop);
                        let _ = reply.send(HostKeyDecision::Continue);
                    }
                    KeyStatus::Unknown if check.policy == UnknownHostKeys::Refuse => {
                        let _ = reply.send(HostKeyDecision::Stop);
                        self.failed_hop = hop;
                        let title = if self.is_jump(hop) {
                            format!("Holonet refused the unknown host key of {}", self.hop_title(hop))
                        } else {
                            "Holonet refused an unknown host key".to_string()
                        };
                        self.fail(
                            Failure::RefusedUnknown,
                            title,
                            "Settings are set to refuse servers whose key is not already in Known hosts.",
                        );
                    }
                    KeyStatus::Unknown => {
                        self.note(format!("{prefix}Paused: waiting for host key verification"), true);
                        self.state = State::HostKey { hop, key, reply: Some(reply) };
                    }
                    KeyStatus::Changed { trusted_key } => {
                        // decisions.md: always stop; never send credentials.
                        let _ = reply.send(HostKeyDecision::Stop);
                        self.note(format!("{prefix}Stopped: the host key has changed"), true);
                        if let Some(app) = self.app.upgrade() {
                            let entry = check.from_alias.clone();
                            app.update(cx, |app, cx| app.record_mismatch(&key, entry.as_deref(), cx));
                        }
                        self.state = State::KeyChanged {
                            hop,
                            presented: key,
                            trusted_key,
                            trusted_at: check.trusted_at,
                            expected: false,
                        };
                    }
                }
            }
            UiRequest::Secret(hop, request, reply) => {
                let field = cx.new(|cx| TextField::new(cx, "").password());
                self._subscriptions.push(cx.subscribe(&field, |this: &mut Self, _, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Submitted => this.answer_secret(true, cx),
                    TextFieldEvent::Cancelled => this.answer_secret(false, cx),
                    TextFieldEvent::Changed => {}
                }));
                // A group that asks each time never has its password saved.
                let asks = matches!(request, SecretRequest::Password { .. }) && self.group_asks(hop, cx).is_some();
                let save = !matches!(request, SecretRequest::Challenge { .. }) && !asks;
                self.state = State::Secret { hop, request, field, save, reply: Some(reply) };
                self.focus_pending = true;
            }
        }
        cx.notify();
    }

    fn answer_host_key(&mut self, decision: &'static str, cx: &mut Context<Self>) {
        let State::HostKey { hop, key, reply } = &mut self.state else { return };
        let hop = *hop;
        let key = key.clone();
        let reply = reply.take();
        match decision {
            "trust" => {
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| app.trust_host_key(&key, cx));
                }
                self.note(format!("{}Trusted: the key is saved in Known hosts", self.hop_prefix(hop)), false);
                self.note_offer(hop);
                if let Some(r) = reply {
                    let _ = r.send(HostKeyDecision::Continue);
                }
                self.state = State::Connecting;
            }
            "once" => {
                self.note(format!("{}Connecting once: the key is not saved", self.hop_prefix(hop)), false);
                self.note_offer(hop);
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
        let State::Secret { hop, request, field, save, reply } = &mut self.state else { return };
        let hop = *hop;
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
        let (is_challenge, save) = (matches!(request, SecretRequest::Challenge { .. }), *save);
        if !is_challenge {
            self.signs[hop].typed = Some((is_password, secret.clone()));
        }
        if save {
            // Saved only once the server accepts it (see `finish`), so a
            // mistyped password never ends up in the vault.
            self.signs[hop].pending_save = Some((is_password, secret.clone()));
        }
        if let Some(r) = reply {
            let _ = r.send(Some(secret));
        }
        self.state = State::Connecting;
        cx.notify();
    }

    /// Send the host's startup snippet, once per pane: not after a
    /// reconnect, since commands are never replayed (decisions.md).
    fn run_startup_snippet(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.startup_done, true) {
            return;
        }
        if let Some(snippet) = self.app.upgrade().and_then(|app| app.read(cx).startup_snippet(self.host.id, cx)) {
            self.run_snippet(snippet, false, cx);
        }
        // Then what a multi-host run asked this pane to type.
        for text in std::mem::take(&mut self.queued) {
            self.terminal.update(cx, |t, cx| t.type_text(&text, cx));
        }
    }

    /// The pane's shell is open and takes input.
    pub fn can_run(&self) -> bool {
        self.mode == PaneMode::Terminal && matches!(self.state, State::Connected) && self.reconnect.is_none()
    }

    /// Run a snippet here: at once, or after the run prompt when it has
    /// variables or asks before running.
    /// `broadcast`: the person ran it in a broadcasting tab, so it goes
    /// to the other panes too (startup snippets never do).
    pub fn run_snippet(&mut self, snippet: holonet_model::Snippet, broadcast: bool, cx: &mut Context<Self>) {
        if RunPrompt::needed(&snippet, false) {
            // The prompt is built on the next render, which has the window
            // to focus its first field.
            self.prompt = None;
            self.ask = Some((snippet, broadcast));
            cx.notify();
            return;
        }
        let text = snippet.input(&Default::default());
        self.type_snippet(&text, broadcast, cx);
    }

    fn type_snippet(&mut self, text: &str, broadcast: bool, cx: &mut Context<Self>) {
        self.terminal.update(cx, |t, cx| {
            if broadcast {
                t.type_user_text(text, cx)
            } else {
                t.type_text(text, cx);
            }
        });
    }

    /// The tab is broadcasting: amber edges.
    pub fn set_broadcast(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.broadcasting != on {
            self.broadcasting = on;
            cx.notify();
        }
    }

    /// Type `text` (a filled snippet) now, or once the shell first opens.
    pub fn queue_input(&mut self, text: String, cx: &mut Context<Self>) {
        if self.startup_done && self.can_run() {
            self.terminal.update(cx, |t, cx| t.type_text(&text, cx));
            self.focus_pending = true;
        } else {
            self.queued.push(text);
        }
    }

    /// Build the run prompt asked for by [`Self::run_snippet`].
    fn open_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((snippet, broadcast)) = self.ask.take() else { return };
        let last = self.app.upgrade().map(|app| app.read(cx).snippet_values(snippet.id)).unwrap_or_default();
        let target = self.name().to_string();
        let run = snippet.clone();
        let prompt = cx.new(|cx| RunPrompt::new(snippet, target, None, &last, window, cx));
        let sub = cx.subscribe(&prompt, move |this: &mut Self, _, event: &RunPromptEvent, cx| {
            if let RunPromptEvent::Run(values) = event {
                if let Some(app) = this.app.upgrade() {
                    let values = values.clone();
                    app.update(cx, |app, _| app.remember_snippet_values(run.id, values));
                }
                let text = run.input(values);
                this.type_snippet(&text, broadcast, cx);
            }
            this.prompt = None;
            this.focus_pending = true;
            cx.notify();
        });
        self.prompt = Some((prompt, sub));
    }

    fn finish(&mut self, result: Result<(Session, Opened), ConnectError>, cx: &mut Context<Self>) {
        match result {
            Ok((session, opened)) => {
                let reconnected = self.reconnect.take().is_some();
                if reconnected {
                    self.reconnect_task = None;
                }
                if reconnected && self.mode == PaneMode::Terminal {
                    let time = jiff::Timestamp::now()
                        .to_zoned_datetime(jiff::tz::TimeZone::system())
                        .strftime("%H:%M")
                        .to_string();
                    let line = format!(
                        "\r\n\x1b[2m-- Reconnected at {time}. The shell restarted; the output above is from the previous session. --\x1b[0m\r\n"
                    );
                    self.terminal.update(cx, |t, _| t.feed(line.as_bytes()));
                    if let Some(log) = &self.history_log {
                        log.note(format!("-- Reconnected at {time}. The shell restarted. --"));
                    }
                }
                self.info = session.info().clone();
                self.note(
                    format!("{}Signed in ({})", self.hop_prefix(self.target_hop()), self.info.auth_method),
                    false,
                );
                self.session = Some(Arc::new(session));
                match opened {
                    Opened::Shell(shell) => {
                        self.terminal.update(cx, |t, cx| t.attach(shell, cx));
                        self.run_startup_snippet(cx);
                    }
                    Opened::Files(remote) => {
                        let files = match self.files.clone() {
                            Some(files) => files,
                            None => {
                                let (host, runtime) = (self.host.clone(), self.runtime.clone());
                                let (app, settings) = (self.app.clone(), self.limits.sftp.clone());
                                let files = cx.new(|cx| FilesView::new(&host, runtime, app, settings, cx));
                                self.files = Some(files.clone());
                                files
                            }
                        };
                        files.update(cx, |f, cx| f.set_remote(Some(Arc::new(remote)), cx));
                        self.watch_lost(cx);
                    }
                }
                self.state = State::Connected;
                // Only the tab's active pane takes the keyboard: a multi-host
                // run's panes connect in any order.
                self.focus_pending = self.active;
                // Each hop's typed secret is remembered for reconnecting, and
                // saved when asked.
                let mut saves = Vec::new();
                for (hop, sign) in self.signs.iter_mut().enumerate() {
                    if let Some(typed) = sign.typed.take() {
                        sign.remembered = Some(typed);
                    }
                    sign.ask_password = false;
                    sign.ask_passphrase = false;
                    if let Some((is_password, secret)) = sign.pending_save.take() {
                        saves.push((hop, is_password, secret));
                    }
                }
                let mut stored = Vec::new();
                for (hop, is_password, secret) in saves {
                    // Keep this tab's copies of the hosts and keys in step with the vault.
                    let (auth, key) = match self.jumps.get_mut(hop) {
                        Some(j) => (&mut j.host.auth, &mut j.key),
                        None => (&mut self.host.auth, &mut self.key),
                    };
                    match (auth, key) {
                        (Auth::Password { saved }, _) if is_password => *saved = Some(secret.clone()),
                        (Auth::Key { .. }, Some(key)) if !is_password => key.saved_passphrase = Some(secret.clone()),
                        _ => {}
                    }
                    stored.push((self.hop_host(hop).id, self.hop_key(hop).map(|k| k.id), is_password, secret));
                }
                let id = self.host.id;
                self.host.last_connected_at = Some(Timestamp::now());
                // Each hop's typed secret, for the host's background connection.
                let typed: Vec<(holonet_model::HostId, (bool, Secret))> = (0..self.signs.len())
                    .filter_map(|hop| self.signs[hop].remembered.clone().map(|r| (self.hop_host(hop).id, r)))
                    .collect();
                // The first connection of a session opens its History record.
                let new_record = self.record.is_none().then(|| {
                    let kind = match self.mode {
                        PaneMode::Files => holonet_model::SessionKind::Sftp,
                        PaneMode::Terminal => holonet_model::SessionKind::Terminal,
                    };
                    (self.host.clone(), kind, self.route_label(), self.host.username.clone())
                });
                if let Some(app) = self.app.upgrade() {
                    if let Some((host, kind, route, user)) = new_record {
                        let (id, log) = app.update(cx, |app, cx| app.start_record(&host, kind, route, user, cx));
                        self.record = Some(id);
                        if let Some(log) = &log {
                            let tap = log.tap();
                            self.terminal.update(cx, |t, _| t.set_log(Some(tap)));
                        }
                        self.history_log = log;
                    }
                    app.update(cx, |app, cx| {
                        // Rules set to start with this host start now.
                        app.host_connected(id, typed, cx);
                        app.update_vault(
                            move |d| {
                                if let Some(h) = d.host_mut(id) {
                                    h.last_connected_at = Some(Timestamp::now());
                                }
                                for (host, key, is_password, secret) in stored {
                                    if is_password {
                                        // A password belongs to the host's identity when it has one.
                                        if let Some(Auth::Password { saved }) = d.sign_in_auth_mut(host) {
                                            *saved = Some(secret);
                                        }
                                    } else if let Some(key) = key.and_then(|k| d.key_mut(k)) {
                                        key.saved_passphrase = Some(secret);
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
                    self.end_record(holonet_model::SessionEnd::Dropped, cx);
                    cx.notify();
                    return;
                }
                if self.reconnect.is_some() && self.reconnect_failed(&e, cx) {
                    cx.notify();
                    return;
                }
                // A reconnect that cannot go on (a refused sign-in) ends the
                // session that dropped.
                self.end_record(holonet_model::SessionEnd::Dropped, cx);
                // Which hop failed: each jump host wraps the error once.
                let (e, wrappers) = e.innermost();
                let hop = self.jumps.len().saturating_sub(wrappers.len());
                self.failed_hop = hop;
                let name = self.hop_name(hop);
                let title_name = self.hop_title(hop);
                let jump = self.is_jump(hop);
                let host = self.hop_host(hop);
                let user = if host.username.is_empty() { "this user".to_string() } else { host.username.clone() };
                let prefix = self.hop_prefix(hop);
                let key_name = self.hop_key(hop).map(|k| k.name.clone()).unwrap_or_default();
                let server = if jump { title_name.clone() } else { "the server".to_string() };
                match e {
                    ConnectError::Resolve { .. } | ConnectError::Refused | ConnectError::Network(_) => {
                        self.note(format!("{prefix}{}", describe_error(e)), true);
                        self.fail(Failure::Network, format!("Holonet could not reach {title_name}"), describe_error(e))
                    }
                    ConnectError::TimedOut(s) => {
                        self.note(format!("{prefix}No response after {s} seconds"), true);
                        let body = if hop == 0 {
                            self.unanswered_body()
                        } else {
                            format!(
                                "{} got no answer from {name} within {s} seconds. {name} may be down.",
                                self.hop_name(hop - 1)
                            )
                        };
                        self.fail(Failure::TimedOut, format!("{} did not answer", upper_first(&title_name)), body)
                    }
                    ConnectError::JumpCouldNotReach { reason, .. } => {
                        // Raised while reaching this hop, by the jump host before it.
                        let via = self.hop_name(hop.saturating_sub(1));
                        self.note(format!("{via}: could not reach {name}: {reason}"), true);
                        self.fail(
                            Failure::Network,
                            format!("{via} could not reach {name}"),
                            format!("The jump host {via} answered, but {reason}."),
                        )
                    }
                    ConnectError::SignInRefused { method, remaining } => {
                        let offered = if remaining.is_empty() { method.to_string() } else { remaining.join(",") };
                        self.note(format!("{prefix}Server: Permission denied ({offered})"), true);
                        if *method == "publickey" && matches!(self.hop_host(hop).auth, Auth::Agent) {
                            self.fail(
                                Failure::KeyRefused,
                                format!("{} refused the agent's keys", upper_first(&server)),
                                format!(
                                    "None of the keys in Windows' OpenSSH agent are in the server's authorized_keys for {user}. Add the right key to the agent with ssh-add, or choose another sign-in."
                                ),
                            )
                        } else if *method == "publickey" {
                            self.fail(
                                Failure::KeyRefused,
                                format!("{} refused the key {key_name}", upper_first(&server)),
                                format!(
                                    "The key may not be in the server's authorized_keys for {user}, or the server may not allow key sign-in for this user."
                                ),
                            )
                        } else {
                            let saved = if self.signs[hop].used_saved_password { "saved " } else { "" };
                            self.fail(
                                Failure::Password,
                                format!("{} refused the {saved}password", upper_first(&server)),
                                format!("The password for {user} may have changed, or the server may no longer allow password sign-in."),
                            )
                        }
                    }
                    ConnectError::WrongPassphrase => {
                        self.note(format!("{prefix}The passphrase did not unlock {key_name}"), true);
                        self.fail(
                            Failure::Passphrase,
                            "The key passphrase is wrong",
                            format!("Enter the passphrase for {key_name} again. Holonet has not contacted the server with this key."),
                        )
                    }
                    ConnectError::AgentUnavailable | ConnectError::AgentEmpty | ConnectError::AgentFailed(_) => {
                        self.note(format!("{prefix}{e}"), true);
                        let (title, body) = match e {
                            ConnectError::AgentUnavailable => (
                                "Windows' OpenSSH agent is not running",
                                "Start the ssh-agent service in Windows, then retry, or edit the host to choose another sign-in.".to_string(),
                            ),
                            ConnectError::AgentEmpty => (
                                "The SSH agent holds no keys",
                                "Add a key to Windows' OpenSSH agent with ssh-add, then retry, or edit the host to choose another sign-in.".to_string(),
                            ),
                            _ => ("The SSH agent could not sign in", describe_error(e)),
                        };
                        self.fail(Failure::KeyRefused, title, body)
                    }
                    ConnectError::Cancelled | ConnectError::HostKeyRejected => {
                        cx.emit(SessionEvent::Close);
                    }
                    other => {
                        self.note(format!("{prefix}{other}"), true);
                        let title = if jump {
                            format!("The connection to {title_name} failed")
                        } else {
                            "The connection failed".to_string()
                        };
                        self.fail(Failure::Other, title, other.to_string())
                    }
                }
            }
        }
        cx.notify();
    }

    /// Why the host did not answer, with what its routes mean
    /// (ConnectStates.dc.html, "Timed out").
    fn unanswered_body(&self) -> String {
        let route_host = self.route_host();
        let tailscale = route_host.tailscale().map(str::to_string);
        let tried_tailscale = self.tried.iter().any(|(v, _)| *v == Via::Tailscale);
        let tried_primary = self.tried.iter().any(|(v, _)| *v == Via::Primary);
        let offline = self.plan_notes.contains(&PlanNote::PeerOffline);
        let no_fallback = self.plan_notes.iter().find_map(|n| match n {
            PlanNote::NoFallback(why) => Some(*why),
            _ => None,
        });
        match (tried_primary, tried_tailscale, tailscale) {
            (true, true, Some(ts)) => format!(
                "Neither {} nor its Tailscale address {ts} answered. The server may be down.",
                route_host.address.trim()
            ),
            (_, true, Some(ts)) => {
                let mut text = format!("{ts} did not answer over Tailscale. The device may be offline or asleep.");
                if offline {
                    text.push_str(" Tailscale lists it as offline.");
                }
                text
            }
            (_, _, Some(_)) => {
                let mut text = unanswered_reason(route_host.address.trim());
                match no_fallback {
                    Some(why) => {
                        text.push_str(&format!(" This host also has a Tailscale address, but {}", lower_first(why)))
                    }
                    None => text.push_str(" This host also has a Tailscale address."),
                }
                text
            }
            _ => unanswered_reason(route_host.address.trim()),
        }
    }

    /// "Try Tailscale" applies: the host has a Tailscale address this
    /// attempt did not use.
    fn can_try_tailscale(&self) -> Option<String> {
        // The route plan is the first hop's: only its failure can use it.
        if self.failed_hop != 0 {
            return None;
        }
        let ts = self.route_host().tailscale()?;
        // Not when Tailscale is known to be off: the failure already says so.
        let off = self.plan_notes.iter().any(|n| matches!(n, PlanNote::NoFallback(_)));
        (!off && !self.tried.iter().any(|(v, _)| *v == Via::Tailscale)).then(|| ts.to_string())
    }

    /// The route plan allowed no attempt.
    fn plan_failed(&mut self, problem: PlanProblem, cx: &mut Context<Self>) {
        self.failed_hop = 0;
        let name = upper_first(&self.hop_title(0));
        let reason = match &problem {
            PlanProblem::TailscaleUnavailable(why) => why.to_string(),
            PlanProblem::NoTailscaleAddress => "This host has no Tailscale address.".to_string(),
        };
        self.note(reason.clone(), true);
        // Tailscale may come back (after a wake, say): reconnecting keeps trying.
        if self.reconnect.is_some() && matches!(problem, PlanProblem::TailscaleUnavailable(_)) {
            if let Some(r) = self.reconnect.as_mut() {
                r.last_error = Some(reason.into());
                // Waiting for Tailscale is not a failed attempt.
                r.attempt = r.attempt.saturating_sub(1);
                r.waiting_for_tailscale = true;
            }
            self.state = State::Connecting;
            self.schedule_reconnect(cx);
            return;
        }
        self.reconnect = None;
        self.reconnect_task = None;
        match problem {
            PlanProblem::TailscaleUnavailable(_) => self.fail(
                Failure::TailscaleOff,
                format!("{name} connects over Tailscale"),
                format!("{reason} Or edit the host to connect another way."),
            ),
            PlanProblem::NoTailscaleAddress => {
                self.fail(Failure::Other, "No Tailscale address", "Edit the host to add its Tailscale address.")
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
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.end_record(holonet_model::SessionEnd::ClosedByYou, cx);
        if let Some(watch) = self.lost_watch.take() {
            watch.abort();
        }
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
                .child(div().text_body().text_color(theme::ink_muted()).child(SharedString::from(self.trying_text())))
                .child(Button::secondary("connect-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.attempt += 1;
                    cx.emit(SessionEvent::Close)
                }))),
        )
        .into_any_element()
    }

    /// The connecting card's line (ConnectStates.dc.html).
    fn trying_text(&self) -> String {
        let route_host = self.route_host();
        let ts = route_host.tailscale().unwrap_or_default();
        if !self.jumps.is_empty() {
            // "Connecting to bastion-east, then app-01" (milestone 4 plan).
            let over = if self.via == Some(Via::Tailscale) { " over Tailscale" } else { "" };
            let mut names: Vec<String> = self.jumps.iter().map(|j| j.name().to_string()).collect();
            names[0].push_str(over);
            names.push(self.host.display_name().to_string());
            return format!("Connecting to {}.", names.join(", then "));
        }
        match self.via {
            Some(Via::Tailscale) if self.plan_notes.contains(&PlanNote::PrimarySkipped) => format!(
                "{} is not reachable from this network, so Holonet is using Tailscale ({ts}).",
                route_host.address.trim()
            ),
            Some(Via::Tailscale) if self.tried.len() > 1 => {
                format!("The primary address did not answer. Trying Tailscale ({ts}).")
            }
            Some(Via::Tailscale) => format!("Trying {ts} over Tailscale."),
            Some(Via::Primary) if self.plan_len > 1 => {
                "Trying the primary address. Tailscale is next if it does not answer.".to_string()
            }
            _ => format!("Trying {}.", self.address),
        }
    }

    /// ConnectStates.dc.html, "Reconnecting": a banner over the old output.
    fn reconnecting_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.reconnect else { return div().into_any_element() };
        let total = RECONNECT_BACKOFF.len();
        let progress = match r.next_at {
            Some(_) if r.waiting_for_tailscale => " Waiting for Tailscale on this PC.".to_string(),
            Some(at) => {
                let secs = at.saturating_duration_since(std::time::Instant::now()).as_secs() + 1;
                let tried =
                    if r.attempt == 0 { String::new() } else { format!("Attempt {} of {total} failed. ", r.attempt) };
                format!(" {tried}Trying again in {secs} s.")
            }
            None if self.via == Some(Via::Tailscale) => {
                format!(" Reconnecting over Tailscale, attempt {} of {total}.", r.attempt)
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
            .child(div().flex_1().min_h_0().opacity(0.75).child(self.body()))
            .child(
                div()
                    .flex_none()
                    .px(theme::SPACE_4)
                    .py(theme::SPACE_2)
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .child(if self.mode == PaneMode::Files {
                        "Both folders are kept. Transfers continue after reconnecting."
                    } else {
                        "Scrollback is kept. The shell session itself restarts after reconnecting."
                    }),
            )
            .into_any_element()
    }

    fn host_key_view(&self, hop: usize, key: &PresentedKey, cx: &mut Context<Self>) -> AnyElement {
        let name = self.hop_title(hop);
        let host = self.hop_host(hop);
        let user = if host.username.is_empty() { String::new() } else { format!(" · {}", host.username) };
        let address = format!("{}:{}", key.host, key.port);
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
                            .child(SharedString::from(format!("{address}{user}"))),
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
        hop: usize,
        presented: &PresentedKey,
        trusted_key: &str,
        trusted_at: Option<Timestamp>,
        expected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self.hop_title(hop);
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
                        format!("{} · {}:{}", presented.key_type_label(), presented.host, presented.port),
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

    /// The group whose "Ask each time" this hop's sign-in follows.
    fn group_asks(&self, hop: usize, cx: &gpui::App) -> Option<SharedString> {
        let host = self.hop_host(hop).id;
        self.app.upgrade().and_then(|app| app.read(cx).asks_each_time(host, cx))
    }

    fn secret_view(
        &self,
        hop: usize,
        request: &SecretRequest,
        field: &Entity<TextField>,
        save: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, body, save_label): (SharedString, SharedString, Option<&str>) = match request {
            SecretRequest::Password { user, host } => match self.group_asks(hop, cx) {
                Some(group) => (
                    format!("Enter the password for {user}@{host}").into(),
                    format!("The {group} group asks for the password each time, so it is not saved.").into(),
                    None,
                ),
                None => (
                    format!("Enter the password for {user}@{host}").into(),
                    "This host has no saved password.".into(),
                    Some("Save in the encrypted vault"),
                ),
            },
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
        // A jump host's prompt names it (milestone 4 plan).
        let body: SharedString = if self.is_jump(hop) {
            let body = body.replace("This host has", "It has");
            format!("{} is the jump host for {}. {body}", self.hop_name(hop), self.name()).into()
        } else {
            body
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
                    let hop = this.failed_hop;
                    this.signs[hop].ask_password = true;
                    this.start(cx)
                }),
            )),
            Failure::Passphrase => buttons.child(Button::primary("enter-passphrase", "Enter passphrase").on_click(
                cx.listener(|this, _, _, cx| {
                    let hop = this.failed_hop;
                    this.signs[hop].ask_passphrase = true;
                    this.start(cx)
                }),
            )),
            Failure::RefusedUnknown => buttons,
            Failure::Network | Failure::TimedOut if self.can_try_tailscale().is_some() => {
                let ts = self.can_try_tailscale().unwrap_or_default();
                buttons
                    .child(Button::primary("try-tailscale", format!("Try Tailscale ({ts})")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.route_override = Some(Route::Tailscale);
                            this.start(cx)
                        },
                    )))
                    .child(Button::secondary("retry", "Retry").on_click(cx.listener(|this, _, _, cx| {
                        this.route_override = None;
                        this.start(cx)
                    })))
            }
            Failure::Network
            | Failure::TimedOut
            | Failure::KeyRefused
            | Failure::GaveUp
            | Failure::TailscaleOff
            | Failure::Other => {
                buttons.child(Button::primary("retry", "Retry").on_click(cx.listener(|this, _, _, cx| this.start(cx))))
            }
        };
        buttons = buttons
            .when(!matches!(kind, Failure::RefusedUnknown), |d| {
                // A jump host's failure edits that host.
                let label = if self.is_jump(self.failed_hop) {
                    format!("Edit {}", self.hop_name(self.failed_hop))
                } else {
                    "Edit host".to_string()
                };
                d.child(
                    Button::secondary("edit-host", label)
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
    pub fn status_bar(&self, cx: &gpui::App) -> Div {
        let (word, color): (&str, gpui::Rgba) = match self.state {
            _ if self.reconnect.is_some() => ("Reconnecting", theme::ink_muted()),
            State::Connected if self.via == Some(Via::Tailscale) => ("Connected over Tailscale", theme::group_mint()),
            State::Connected => ("Connected", theme::group_mint()),
            State::Ended(_) => ("Ended", theme::warn()),
            State::Failed { kind, .. } => (kind.state_word(), theme::warn()),
            State::KeyChanged { .. } => ("Host key changed", theme::warn()),
            _ => ("Connecting", theme::ink_muted()),
        };
        let auth = match self.info.auth_method.as_str() {
            "publickey" => self.key.as_ref().map(|k| format!("Key {}", k.name)).unwrap_or_else(|| "Key".into()),
            "password" | "keyboard-interactive" => "Password".into(),
            "agent" => "SSH agent".into(),
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
        if !self.jumps.is_empty() && matches!(self.state, State::Connected) {
            let names: Vec<&str> = self.jumps.iter().map(|j| j.name()).collect();
            items.push(format!("Through {}", names.join(", ")).into());
        }
        if self.history_log.is_some() {
            items.push("Logging to History".into());
        }
        if matches!(self.state, State::Connected) && self.mode == PaneMode::Terminal {
            if !matches!(self.info.compression.as_str(), "" | "none") {
                items.push("Compressed".into());
            }
            // Terminal.dc.html: "Agent forwarding off".
            items.push(if self.host.agent_forwarding { "Agent forwarding on" } else { "Agent forwarding off" }.into());
        }
        // The host's running port forwards (Terminal.dc.html: "Forward L 5432 → db-primary:5432").
        if matches!(self.state, State::Connected)
            && let Some(app) = self.app.upgrade()
        {
            items.extend(app.read(cx).forwards_for(self.host.id, cx).into_iter().map(SharedString::from));
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
            .child(if self.mode == PaneMode::Files { "SFTP" } else { "xterm-256color · UTF-8" })
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.ask.is_some() {
            self.open_prompt(window, cx);
        }
        if std::mem::take(&mut self.focus_pending) && self.prompt.is_none() {
            self.focus(window, cx);
        }
        // A terminal that is no longer drawn (the pane failed, or shows a
        // card) cannot dispatch keys: hand its focus to the pane.
        let terminal_shown = matches!(self.state, State::Connected | State::Ended(_)) || self.reconnect.is_some();
        if !terminal_shown && self.terminal.read(cx).is_focused(window) {
            window.focus(&self.focus_handle);
        }
        let tag = self.app.upgrade().and_then(|app| app.read(cx).group_tag(self.host.id, cx));
        let color = crate::workspace::GroupTag::color_of(tag.as_ref());
        let size = self.terminal.read(cx).grid_size();
        let state_word: SharedString = match &self.state {
            _ if self.reconnect.is_some() => "Reconnecting".into(),
            State::Connected if self.mode == PaneMode::Files => "Connected".into(),
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
            State::HostKey { hop, key, .. } => {
                let (hop, key) = (*hop, key.clone());
                self.host_key_view(hop, &key, cx)
            }
            State::KeyChanged { hop, presented, trusted_key, trusted_at, expected } => {
                let (h, p, t, at, e) = (*hop, presented.clone(), trusted_key.clone(), *trusted_at, *expected);
                self.key_changed_view(h, &p, &t, at, e, cx)
            }
            State::Secret { hop, request, field, save, .. } => {
                let (h, r, f, s) = (*hop, request.clone(), field.clone(), *save);
                self.secret_view(h, &r, &f, s, cx)
            }
            State::Failed { kind, title, body } => {
                let (title, body) = (title.clone(), body.clone());
                let kind = *kind;
                self.failed_view(&kind, &title, &body, cx)
            }
            State::Connected => self.body(),
            State::Ended(reason) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().opacity(0.75).child(self.body()))
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
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::terminal())
            .border_1()
            .border_color(if self.broadcasting {
                theme::warn()
            } else if self.active {
                theme::ink_muted()
            } else {
                theme::line()
            })
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
                            .child(div().text_color(color).child(crate::workspace::GroupTag::name_of(tag.as_ref()))),
                    )
                    .child(
                        div()
                            .text_mono_small()
                            .text_color(if warn_word { theme::warn() } else { theme::ink_muted() })
                            .child(state_word),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(content)
                    .children(self.prompt.as_ref().map(|(p, _)| p.clone())),
            )
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
