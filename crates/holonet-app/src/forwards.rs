//! Port forwarding rules while Holonet runs (milestone 6, phase 4).
//!
//! The [`Forwarder`] keeps one background connection per host with rules
//! switched on (owner decision 1): shared by that host's rules, separate
//! from terminal and SFTP tabs, closed when its last rule stops. It lives in
//! the app, not the vault view, so forwards stay up while the vault is
//! locked (`decisions.md`).
//!
//! Connecting follows a pane's path (route plan, jump hosts, each hop's
//! sign-in) with two differences, since nobody may be watching:
//! - **Host keys:** only a key already trusted in Known hosts is accepted.
//!   An unknown key stops with "Connect to … once in a terminal to verify
//!   its host key"; a changed key stops and is recorded, as in a pane.
//!   While the vault is locked, a reconnect accepts only the key this
//!   connection already accepted.
//! - **Secrets:** a saved one, else one typed in a pane or on the Port
//!   forwarding page since Holonet started, else a question on that page.
//!
//! A lost connection is retried 5 times (after 1, 2, 4, 8 and 16 s), and the
//! rules come back on the new connection.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, WeakEntity};
use holonet_model::{ForwardKind, ForwardRule, ForwardRuleId, Host, HostId, Secret, VaultData};
use holonet_net::PlanProblem;
use holonet_ssh::{
    BoxFuture, ConnectError, ConnectOptions, Forward, ForwardSpec, ForwardStatus, HostKeyDecision, KeyStatus,
    PresentedKey, Prompts, SecretRequest, Session,
};
use tokio::sync::{mpsc, oneshot};

use crate::app::HolonetApp;
use crate::hops::{self, Hop, Routed};

/// Waits before each reconnect attempt (`decisions.md`: 5 attempts).
const RECONNECT_DELAYS: [u64; 5] = [1, 2, 4, 8, 16];

/// A rule's state, for its card and the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleState {
    /// Connecting to the host, or binding the port.
    Starting,
    /// Waiting for a password or passphrase on the Port forwarding page.
    NeedsSecret(String),
    Active {
        port: u16,
        open: usize,
        total: u64,
        last_error: Option<String>,
    },
    /// The connection was lost; trying again.
    Reconnecting {
        attempt: usize,
    },
    /// Not running, and why. The switch shows off.
    Failed(String),
}

impl RuleState {
    /// The switch is on.
    pub fn is_on(&self) -> bool {
        !matches!(self, RuleState::Failed(_))
    }
}

struct Run {
    rule: ForwardRule,
    state: RuleState,
    /// The running forward; dropping it stops the forward.
    forward: Option<Forward>,
}

enum LinkState {
    Connecting,
    Up(Arc<Session>),
    /// Between reconnect attempts.
    Waiting,
}

/// A host's background connection.
struct Link {
    /// Every hop in order, jump hosts first, the host last.
    hops: Vec<Hop>,
    connect_timeout: Duration,
    fallback_wait: Duration,
    state: LinkState,
    /// Reconnect attempts made since the connection was lost.
    attempt: usize,
    /// Bumped on every attempt, so a stale result is ignored.
    generation: u64,
    /// Each hop's host key accepted on this connection.
    accepted: HashMap<usize, String>,
    /// Why the last attempt stopped, when Holonet stopped it.
    stop_reason: Option<String>,
}

/// A question for the Port forwarding page.
pub struct SecretAsk {
    pub id: u64,
    /// The host the question is for (a jump host's own name, when it is one).
    pub host_name: String,
    pub request: SecretRequest,
    /// The host whose background connection asks.
    link: HostId,
    /// The hop the answer is for, kept under its host.
    hop_host: HostId,
    reply: Option<oneshot::Sender<Option<Secret>>>,
}

enum Request {
    HostKey { host: HostId, hop: usize, key: PresentedKey, reply: oneshot::Sender<HostKeyDecision> },
    Secret { host: HostId, hop: usize, request: SecretRequest, reply: oneshot::Sender<Option<Secret>> },
}

struct BackgroundPrompts {
    tx: mpsc::UnboundedSender<Request>,
    host: HostId,
    hop: usize,
}

impl Prompts for BackgroundPrompts {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        let (reply, rx) = oneshot::channel();
        let sent = self.tx.send(Request::HostKey { host: self.host, hop: self.hop, key, reply }).is_ok();
        Box::pin(async move { if sent { rx.await.unwrap_or(HostKeyDecision::Stop) } else { HostKeyDecision::Stop } })
    }

    fn secret(&self, request: SecretRequest) -> BoxFuture<Option<Secret>> {
        let (reply, rx) = oneshot::channel();
        let sent = self.tx.send(Request::Secret { host: self.host, hop: self.hop, request, reply }).is_ok();
        Box::pin(async move { if sent { rx.await.ok().flatten() } else { None } })
    }
}

pub struct Forwarder {
    app: WeakEntity<HolonetApp>,
    runtime: tokio::runtime::Handle,
    runs: HashMap<ForwardRuleId, Run>,
    links: HashMap<HostId, Link>,
    /// Passwords (true) and passphrases typed since Holonet started, by the
    /// host they are for. Never saved.
    typed: HashMap<HostId, (bool, Secret)>,
    /// Questions waiting on the Port forwarding page, oldest first.
    asks: VecDeque<SecretAsk>,
    next_ask: u64,
}

impl Forwarder {
    pub fn new(app: WeakEntity<HolonetApp>, runtime: tokio::runtime::Handle) -> Self {
        Self {
            app,
            runtime,
            runs: HashMap::new(),
            links: HashMap::new(),
            typed: HashMap::new(),
            asks: VecDeque::new(),
            next_ask: 0,
        }
    }

    pub fn state(&self, rule: ForwardRuleId) -> Option<&RuleState> {
        self.runs.get(&rule).map(|r| &r.state)
    }

    pub fn is_on(&self, rule: ForwardRuleId) -> bool {
        self.state(rule).is_some_and(RuleState::is_on)
    }

    /// The status bar's text for a host's active forwards.
    pub fn active_for(&self, host: HostId) -> Vec<String> {
        let mut texts: Vec<String> = self
            .runs
            .values()
            .filter(|r| r.rule.host == host && matches!(r.state, RuleState::Active { .. }))
            .map(|r| r.rule.status_text())
            .collect();
        texts.sort();
        texts
    }

    /// The oldest question for the Port forwarding page.
    pub fn ask(&self) -> Option<&SecretAsk> {
        self.asks.front()
    }

    /// Answer a question; `None` cancels the connection that asked.
    pub fn answer(&mut self, id: u64, secret: Option<Secret>, cx: &mut Context<Self>) {
        let Some(index) = self.asks.iter().position(|a| a.id == id) else { return };
        let mut ask = self.asks.remove(index).expect("found");
        if let Some(secret) = &secret {
            let password = matches!(ask.request, SecretRequest::Password { .. });
            self.typed.insert(ask.hop_host, (password, secret.clone()));
        }
        if let Some(reply) = ask.reply.take() {
            let _ = reply.send(secret);
        }
        for id in self.host_runs(ask.link) {
            self.set_state(id, RuleState::Starting, cx);
        }
        cx.notify();
    }

    /// A pane connected to `host`: keep its typed secrets for background
    /// connections, and start the host's rules set to start with it.
    pub fn host_connected(
        &mut self,
        host: HostId,
        typed: Vec<(HostId, (bool, Secret))>,
        data: &VaultData,
        cx: &mut Context<Self>,
    ) {
        self.typed.extend(typed);
        let rules: Vec<ForwardRuleId> =
            data.host_rules(host).filter(|r| r.start_with_host && !self.is_on(r.id)).map(|r| r.id).collect();
        for rule in rules {
            self.start(rule, data, cx);
        }
    }

    /// Switch a rule on.
    pub fn start(&mut self, id: ForwardRuleId, data: &VaultData, cx: &mut Context<Self>) {
        if self.is_on(id) {
            return;
        }
        let Some(rule) = data.forward_rule(id).cloned() else { return };
        let host_id = rule.host;
        self.runs.insert(id, Run { rule, state: RuleState::Starting, forward: None });
        cx.notify();
        match self.links.get(&host_id).map(|l| &l.state) {
            Some(LinkState::Up(_)) => self.start_forward(id, cx),
            // It starts when the connection comes up.
            Some(LinkState::Connecting | LinkState::Waiting) => {}
            None => {
                let Some(host) = data.host(host_id) else {
                    self.fail_rule(id, "Its host was deleted.".into(), cx);
                    return;
                };
                let host = data.resolved(host);
                let mut chain = match hops::jumps(data, &host) {
                    Ok(chain) => chain,
                    Err(problem) => {
                        self.fail_rule(id, problem.message().into(), cx);
                        return;
                    }
                };
                chain.push(Hop::of(data, &host));
                let link = Link {
                    hops: chain,
                    connect_timeout: Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64),
                    fallback_wait: data.settings.connection.fallback_wait.duration(),
                    state: LinkState::Connecting,
                    attempt: 0,
                    generation: 0,
                    accepted: HashMap::new(),
                    stop_reason: None,
                };
                self.links.insert(host_id, link);
                self.connect(host_id, cx);
            }
        }
    }

    /// Switch a rule off (also clears a failed one).
    pub fn stop(&mut self, id: ForwardRuleId, cx: &mut Context<Self>) {
        let Some(run) = self.runs.remove(&id) else { return };
        drop(run.forward);
        self.close_if_unused(run.rule.host, cx);
        cx.notify();
    }

    /// A running rule was edited: start it again with its new settings.
    pub fn restart(&mut self, id: ForwardRuleId, data: &VaultData, cx: &mut Context<Self>) {
        if self.is_on(id) {
            self.stop(id, cx);
            self.start(id, data, cx);
        }
    }

    fn host_runs(&self, host: HostId) -> Vec<ForwardRuleId> {
        self.runs.iter().filter(|(_, r)| r.rule.host == host && r.state.is_on()).map(|(id, _)| *id).collect()
    }

    /// Close a host's connection once none of its rules is on.
    fn close_if_unused(&mut self, host: HostId, cx: &mut Context<Self>) {
        if !self.host_runs(host).is_empty() {
            return;
        }
        if let Some(link) = self.links.remove(&host) {
            if let LinkState::Up(session) = link.state {
                self.runtime.spawn(async move {
                    let _ = session.close().await;
                });
            }
            // Questions from its connection have nobody to answer them now.
            self.asks.retain(|a| a.link != host);
        }
        cx.notify();
    }

    fn set_state(&mut self, id: ForwardRuleId, state: RuleState, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.get_mut(&id) {
            run.state = state;
            cx.notify();
        }
    }

    fn fail_rule(&mut self, id: ForwardRuleId, why: String, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.get_mut(&id) {
            run.forward = None;
            run.state = RuleState::Failed(why);
            let host = run.rule.host;
            self.close_if_unused(host, cx);
        }
        cx.notify();
    }

    fn fail_host(&mut self, host: HostId, why: String, cx: &mut Context<Self>) {
        for id in self.host_runs(host) {
            if let Some(run) = self.runs.get_mut(&id) {
                run.forward = None;
                run.state = RuleState::Failed(why.clone());
            }
        }
        self.close_if_unused(host, cx);
    }

    /// Start (or retry) a host's background connection.
    fn connect(&mut self, host_id: HostId, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let Some(link) = self.links.get_mut(&host_id) else { return };
        link.generation += 1;
        link.state = LinkState::Connecting;
        link.stop_reason = None;
        let generation = link.generation;
        let mut steps = Vec::new();
        for (index, hop) in link.hops.iter().enumerate() {
            let typed = self.typed.get(&hop.host.id).cloned();
            let Some(credential) =
                hop.credential(false, false, |password| typed.clone().filter(|(p, _)| *p == password).map(|(_, s)| s))
            else {
                let name = hop.name().to_string();
                self.fail_host(host_id, format!("The key for {name} is missing from the vault."), cx);
                return;
            };
            steps.push(ConnectOptions {
                host: hops::reach_address(&hop.host),
                port: hop.host.port,
                user: hop.host.username.clone(),
                credential,
                connect_timeout: link.connect_timeout,
                keep_alive: hop.keep_alive,
                prompts: Arc::new(BackgroundPrompts { tx: tx.clone(), host: host_id, hop: index }),
                // No shell here, so nothing to forward the agent to.
                agent_forwarding: false,
                compression: hop.host.compression,
                jump: None,
            });
        }
        let opts = hops::link(steps);
        let route_host = link.hops[0].host.clone();
        let (wait, jumps) = (link.fallback_wait, link.hops.len() - 1);
        let (done_tx, done_rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let routed = hops::connect_by_route(opts, &route_host, None, wait, jumps, |_| {}).await;
            let _ = done_tx.send(routed);
        });
        cx.spawn(async move |this, cx| {
            while let Some(request) = rx.recv().await {
                if this.update(cx, |f, cx| f.on_request(request, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let routed = done_rx.await.unwrap_or_else(|_| {
                Routed::Connect(Box::new(Err(ConnectError::Protocol("the connection task stopped".into()))))
            });
            let _ = this.update(cx, |f, cx| f.connected(host_id, generation, routed, cx));
        })
        .detach();
        cx.notify();
    }

    fn on_request(&mut self, request: Request, cx: &mut Context<Self>) {
        match request {
            Request::HostKey { host, hop, key, reply } => {
                let decision = self.decide_host_key(host, hop, &key, cx);
                let _ = reply.send(decision);
            }
            Request::Secret { host, hop, request, reply } => {
                let Some(link) = self.links.get(&host) else {
                    let _ = reply.send(None);
                    return;
                };
                let hop_host = link.hops[hop].host.id;
                let host_name = link.hops[hop].name().to_string();
                for id in self.host_runs(host) {
                    self.set_state(id, RuleState::NeedsSecret(host_name.clone()), cx);
                }
                self.next_ask += 1;
                self.asks.push_back(SecretAsk {
                    id: self.next_ask,
                    host_name,
                    request,
                    link: host,
                    hop_host,
                    reply: Some(reply),
                });
                cx.notify();
            }
        }
    }

    fn decide_host_key(
        &mut self,
        host: HostId,
        hop: usize,
        key: &PresentedKey,
        cx: &mut Context<Self>,
    ) -> HostKeyDecision {
        let Some(link) = self.links.get(&host) else { return HostKeyDecision::Stop };
        let hop_host = link.hops[hop].host.clone();
        let name = hop_host.display_name().to_string();
        let accepted_before = link.accepted.get(&hop).is_some_and(|k| *k == key.public_key);
        let aliases = VaultData::other_addresses(&hop_host, &key.host);
        let check = self.app.upgrade().and_then(|app| app.update(cx, |app, cx| app.check_host_key(key, &aliases, cx)));
        let (decision, reason) = match check.map(|c| (c.status, c.from_alias)) {
            Some((KeyStatus::Trusted, _)) => (HostKeyDecision::Continue, None),
            // Locked: the key this connection already accepted is still good.
            None if accepted_before => (HostKeyDecision::Continue, None),
            None => (
                HostKeyDecision::Stop,
                Some(format!(
                    "The vault is locked, so the host key of {name} cannot be checked. Unlock it to reconnect."
                )),
            ),
            Some((KeyStatus::Unknown, _)) => (
                HostKeyDecision::Stop,
                Some(format!("Connect to {name} once in a terminal to verify its host key, then switch this on.")),
            ),
            Some((KeyStatus::Changed { .. }, alias)) => {
                // decisions.md: always stop, and keep the key for review.
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| app.record_mismatch(key, alias.as_deref(), cx));
                }
                (HostKeyDecision::Stop, Some(format!("The host key of {name} has changed. Review it in Known hosts.")))
            }
        };
        if let Some(link) = self.links.get_mut(&host) {
            if decision == HostKeyDecision::Continue {
                link.accepted.insert(hop, key.public_key.clone());
            }
            link.stop_reason = reason;
        }
        decision
    }

    fn connected(&mut self, host: HostId, generation: u64, routed: Routed, cx: &mut Context<Self>) {
        let Some(link) = self.links.get_mut(&host) else {
            // Switched off while connecting.
            if let Routed::Connect(result) = routed
                && let Ok(session) = *result
            {
                self.runtime.spawn(async move {
                    let _ = session.close().await;
                });
            }
            return;
        };
        if link.generation != generation {
            return;
        }
        let name = link.hops.last().map(|h| h.name().to_string()).unwrap_or_default();
        let result = match routed {
            Routed::Plan(problem) => Err(match problem {
                PlanProblem::TailscaleUnavailable(why) => why.to_string(),
                PlanProblem::NoTailscaleAddress => format!("{name} has no Tailscale address."),
            }),
            Routed::Connect(result) => match *result {
                Ok(session) => Ok(session),
                Err(ConnectError::HostKeyRejected) if link.stop_reason.is_some() => {
                    Err(link.stop_reason.clone().unwrap_or_default())
                }
                Err(ConnectError::Cancelled) => Err("Cancelled: the password was not entered.".into()),
                Err(e) => {
                    // A refused sign-in: forget what was typed for these hops.
                    let (inner, jumps) = e.innermost();
                    // The hop that refused is reached through every jump host named.
                    let hop = jumps.len();
                    let refused = matches!(inner, ConnectError::SignInRefused { .. } | ConnectError::WrongPassphrase);
                    if refused {
                        for hop in &link.hops {
                            self.typed.remove(&hop.host.id);
                        }
                    }
                    let hop_name = link.hops.get(hop).map(|h| h.name().to_string()).unwrap_or_else(|| name.clone());
                    Err(match inner {
                        ConnectError::SignInRefused { method: "publickey", .. } => {
                            format!("{hop_name} refused the key. Check that it is in the host's authorized_keys.")
                        }
                        ConnectError::SignInRefused { .. } => {
                            format!("{hop_name} refused the password. Switch the rule on to type it again.")
                        }
                        ConnectError::WrongPassphrase => {
                            "The passphrase did not unlock the key. Switch the rule on to type it again.".into()
                        }
                        _ => crate::host_form::describe_error(&e),
                    })
                }
            },
        };
        match result {
            Ok(session) => {
                let session = Arc::new(session);
                link.state = LinkState::Up(session.clone());
                link.attempt = 0;
                for id in self.host_runs(host) {
                    self.start_forward(id, cx);
                }
                // Watch for the connection being lost.
                let (lost_tx, lost_rx) = oneshot::channel();
                self.runtime.spawn(async move {
                    let _ = lost_tx.send(session.closed().await);
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Some(_)) = lost_rx.await {
                        let _ = this.update(cx, |f, cx| f.lost(host, generation, cx));
                    }
                })
                .detach();
            }
            // A reconnect keeps trying while attempts remain.
            Err(why) if link.attempt > 0 && link.attempt < RECONNECT_DELAYS.len() => {
                link.stop_reason = Some(why);
                self.schedule_reconnect(host, cx);
            }
            Err(why) if link.attempt > 0 => {
                let attempts = link.attempt;
                self.fail_host(host, format!("{name} did not reconnect after {attempts} attempts. {why}"), cx);
            }
            Err(why) => self.fail_host(host, why, cx),
        }
        cx.notify();
    }

    /// The connection under running rules was lost.
    fn lost(&mut self, host: HostId, generation: u64, cx: &mut Context<Self>) {
        let Some(link) = self.links.get_mut(&host) else { return };
        if link.generation != generation {
            return;
        }
        link.attempt = 0;
        self.schedule_reconnect(host, cx);
    }

    fn schedule_reconnect(&mut self, host: HostId, cx: &mut Context<Self>) {
        let Some(link) = self.links.get_mut(&host) else { return };
        let delay = Duration::from_secs(RECONNECT_DELAYS[link.attempt.min(RECONNECT_DELAYS.len() - 1)]);
        link.attempt += 1;
        link.state = LinkState::Waiting;
        let (attempt, generation) = (link.attempt, link.generation);
        for id in self.host_runs(host) {
            if let Some(run) = self.runs.get_mut(&id) {
                run.forward = None;
                run.state = RuleState::Reconnecting { attempt };
            }
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |f, cx| {
                if f.links.get(&host).is_some_and(|l| l.generation == generation) {
                    f.connect(host, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn start_forward(&mut self, id: ForwardRuleId, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get_mut(&id) else { return };
        let Some(LinkState::Up(session)) = self.links.get(&run.rule.host).map(|l| &l.state) else { return };
        let session = session.clone();
        run.state = RuleState::Starting;
        let spec = spec_of(&run.rule);
        let (tx, rx) = oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(session.forward(spec).await);
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |f, cx| f.forward_started(id, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn forward_started(
        &mut self,
        id: ForwardRuleId,
        result: Result<Forward, holonet_ssh::ForwardError>,
        cx: &mut Context<Self>,
    ) {
        // Switched off meanwhile: dropping the forward stops it.
        let Some(run) = self.runs.get_mut(&id) else { return };
        match result {
            Ok(forward) => {
                let mut watch = forward.watch();
                run.state = state_of(&forward.status()).unwrap_or(RuleState::Starting);
                run.forward = Some(forward);
                cx.spawn(async move |this, cx| {
                    while watch.changed().await.is_ok() {
                        let status = watch.borrow().clone();
                        let Some(state) = state_of(&status) else { break };
                        if this.update(cx, |f, cx| f.set_state(id, state, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                cx.notify();
            }
            Err(e) => self.fail_rule(id, e.to_string(), cx),
        }
    }
}

/// The page's state for a forward's status; `None` once it has ended (the
/// connection's own watcher handles a lost connection).
fn state_of(status: &ForwardStatus) -> Option<RuleState> {
    match status {
        ForwardStatus::Active { port, open, total, last_error } => {
            Some(RuleState::Active { port: *port, open: *open, total: *total, last_error: last_error.clone() })
        }
        ForwardStatus::Failed(_) | ForwardStatus::Stopped => None,
    }
}

fn spec_of(rule: &ForwardRule) -> ForwardSpec {
    let bind_address = rule.bind_address.trim().to_string();
    match rule.kind {
        ForwardKind::Local => ForwardSpec::Local {
            bind_address,
            port: rule.port,
            destination: rule.destination_host.trim().to_string(),
            destination_port: rule.destination_port,
        },
        ForwardKind::Remote => ForwardSpec::Remote {
            bind_address,
            port: rule.port,
            destination: rule.destination_host.trim().to_string(),
            destination_port: rule.destination_port,
        },
        ForwardKind::Dynamic => ForwardSpec::Dynamic { bind_address, port: rule.port },
    }
}

/// A host's display name, for texts.
pub fn host_name(data: &VaultData, host: HostId) -> String {
    data.host(host).map(Host::display_name).unwrap_or("Deleted host").to_string()
}
