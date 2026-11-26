//! Connecting to a saved host: the jump hosts in front of it, each hop's
//! sign-in, and the route plan, which applies to the first hop (the one this
//! PC connects to). Shared by panes and Install on host.

use std::time::Duration;

use holonet_model::{Auth, Host, JumpProblem, Route, Secret, StoredKey, VaultData};
use holonet_net::{PlanNote, PlanProblem, Via};
use holonet_ssh::{ConnectError, ConnectOptions, Credential, Session};

use crate::host_form::describe_error;

/// One host on the way, with its identity's sign-in applied.
#[derive(Clone)]
pub struct Hop {
    pub host: Host,
    /// The host's key when it signs in with one (`None` if it is missing).
    pub key: Option<StoredKey>,
    pub keep_alive: Option<Duration>,
}

impl Hop {
    pub fn of(data: &VaultData, host: &Host) -> Hop {
        let host = data.resolved(host);
        let key = host.auth.key_id().and_then(|id| data.key(id)).cloned();
        let keep_alive = data.settings.connection.keep_alive_for(&host);
        Hop { host, key, keep_alive }
    }

    pub fn name(&self) -> &str {
        self.host.display_name()
    }

    /// How this hop signs in: a saved secret, else `remembered` (typed earlier
    /// in this pane), unless asked to ask again. `None`: its key is missing.
    pub fn credential(
        &self,
        ask_password: bool,
        ask_passphrase: bool,
        remembered: impl Fn(bool) -> Option<Secret>,
    ) -> Option<Credential> {
        Some(match (&self.host.auth, &self.key) {
            (Auth::Password { saved }, _) => {
                Credential::Password(if ask_password { None } else { saved.clone().or_else(|| remembered(true)) })
            }
            (Auth::Key { .. }, Some(key)) => Credential::Key {
                name: key.name.clone(),
                private_key: key.private_key.clone(),
                passphrase: if ask_passphrase {
                    None
                } else {
                    key.saved_passphrase.clone().or_else(|| remembered(false))
                },
            },
            (Auth::Key { .. }, None) => return None,
        })
    }
}

/// The jump hosts in front of `host`, first hop first.
pub fn jumps(data: &VaultData, host: &Host) -> Result<Vec<Hop>, JumpProblem> {
    Ok(data.jump_chain(host)?.into_iter().filter_map(|id| data.host(id)).map(|h| Hop::of(data, h)).collect())
}

/// The address a host behind a jump host is reached at: its Tailscale
/// address when its route is Tailscale, otherwise its primary address. The
/// route plan's fallback applies only to the first hop.
pub fn reach_address(host: &Host) -> String {
    match host.effective_route() {
        Route::Tailscale => host.tailscale().unwrap_or(host.address.trim()).to_string(),
        _ => host.address.trim().to_string(),
    }
}

/// Link each hop's options into one chain: `steps` runs first hop first and
/// ends with the target, and each step becomes the next one's jump host.
pub fn link(mut steps: Vec<ConnectOptions>) -> ConnectOptions {
    let mut target = steps.pop().expect("the target's options");
    let mut jump: Option<Box<ConnectOptions>> = None;
    for mut step in steps {
        step.jump = jump.take();
        jump = Some(Box::new(step));
    }
    target.jump = jump;
    target
}

/// The options of the first hop: the one this PC connects to.
#[allow(clippy::unnecessary_unwrap)] // the `if let` form needs Polonius to return the borrow
pub fn first_hop(opts: &mut ConnectOptions) -> &mut ConnectOptions {
    if opts.jump.is_some() {
        return first_hop(opts.jump.as_mut().expect("checked"));
    }
    opts
}

/// The address could not be reached at all, so the next address in the
/// route plan is worth trying.
pub fn unreachable(e: &ConnectError) -> bool {
    matches!(
        e,
        ConnectError::Resolve { .. } | ConnectError::Refused | ConnectError::Network(_) | ConnectError::TimedOut(_)
    )
}

/// Progress of [`connect_by_route`], for the connecting log.
pub enum RouteEvent {
    /// The plan: how many addresses, and its notes.
    Plan { attempts: usize, notes: Vec<PlanNote> },
    /// Starting on an address of the first hop.
    Attempt { via: Via, address: String },
    /// An address did not answer; the next one is tried.
    Unreachable(String),
}

pub enum Routed {
    /// The plan allowed no attempt (Tailscale is needed but off).
    Plan(PlanProblem),
    Connect(Box<Result<Session, ConnectError>>),
}

/// Connect `opts` (a chain from [`link`]), trying the first hop's addresses
/// in the order its route plan gives (decisions.md, Connection and
/// Tailscale). `route_host` is the first hop's host; `jumps` how many jump
/// hosts are in the chain.
pub async fn connect_by_route(
    opts: ConnectOptions,
    route_host: &Host,
    route_override: Option<Route>,
    fallback_wait: Duration,
    jumps: usize,
    mut events: impl FnMut(RouteEvent) + Send,
) -> Routed {
    let tailscale = holonet_net::tailscale::status().await;
    let skip = holonet_net::primary_unreachable(&route_host.address, tailscale.is_running());
    let plan = match holonet_net::plan(route_host, route_override, &tailscale, skip, fallback_wait) {
        Ok(plan) => plan,
        Err(problem) => return Routed::Plan(problem),
    };
    events(RouteEvent::Plan { attempts: plan.attempts.len(), notes: plan.notes.clone() });
    let count = plan.attempts.len();
    for (i, attempt) in plan.attempts.into_iter().enumerate() {
        events(RouteEvent::Attempt { via: attempt.via, address: attempt.address.clone() });
        let mut opts = opts.clone();
        let hop = first_hop(&mut opts);
        hop.host = attempt.address.clone();
        if let Some(wait) = attempt.give_up_after {
            // Only the TCP connection is limited: prompts can take as long
            // as the person needs.
            hop.connect_timeout = wait;
        }
        let result = Session::connect(opts).await;
        if let Err(e) = &result {
            // Only the first hop's own failure to answer moves on.
            let (inner, wrappers) = e.innermost();
            if i + 1 < count && wrappers.len() == jumps && unreachable(inner) {
                events(RouteEvent::Unreachable(match inner {
                    ConnectError::TimedOut(s) => format!("No answer from {} after {s} s", attempt.address),
                    other => format!("{}: {}", attempt.address, describe_error(other)),
                }));
                continue;
            }
        }
        return Routed::Connect(Box::new(result));
    }
    Routed::Connect(Box::new(Err(ConnectError::Protocol("the route plan had no addresses".into()))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NoPrompts;
    impl holonet_ssh::Prompts for NoPrompts {
        fn host_key(&self, _: holonet_ssh::PresentedKey) -> holonet_ssh::BoxFuture<holonet_ssh::HostKeyDecision> {
            Box::pin(async { holonet_ssh::HostKeyDecision::Stop })
        }
        fn secret(&self, _: holonet_ssh::SecretRequest) -> holonet_ssh::BoxFuture<Option<Secret>> {
            Box::pin(async { None })
        }
    }

    fn opts(host: &str) -> ConnectOptions {
        ConnectOptions {
            host: host.into(),
            port: 22,
            user: "u".into(),
            credential: Credential::Password(None),
            connect_timeout: Duration::from_secs(1),
            keep_alive: None,
            prompts: Arc::new(NoPrompts),
            jump: None,
        }
    }

    #[test]
    fn link_nests_first_hop_deepest() {
        let mut chain = link(vec![opts("bastion"), opts("inner"), opts("app")]);
        assert_eq!(chain.host, "app");
        assert_eq!(chain.jump.as_ref().unwrap().host, "inner");
        assert_eq!(chain.jump.as_ref().unwrap().jump.as_ref().unwrap().host, "bastion");
        assert_eq!(first_hop(&mut chain).host, "bastion");
        let mut alone = link(vec![opts("app")]);
        assert!(alone.jump.is_none());
        assert_eq!(first_hop(&mut alone).host, "app");
    }

    #[test]
    fn reach_address_follows_the_route() {
        let mut h = Host::new("10.0.0.5");
        assert_eq!(reach_address(&h), "10.0.0.5");
        h.tailscale_address = Some("100.64.0.9".into());
        assert_eq!(reach_address(&h), "10.0.0.5");
        h.route = Route::Tailscale;
        assert_eq!(reach_address(&h), "100.64.0.9");
    }
}
