//! Which addresses to try for one connection, in order, and how long to give
//! each (`decisions.md`, Connection and Tailscale; NewHost.dc.html "Connect
//! using").

use std::time::Duration;

use holonet_model::{Host, Route};

use crate::tailscale::Tailscale;

/// Which of a host's addresses an attempt uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Primary,
    Tailscale,
}

/// One address to try.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub address: String,
    pub via: Via,
    /// Move on to the next attempt if this one has not connected after this
    /// long. `None` on the last attempt, which uses the connect timeout.
    pub give_up_after: Option<Duration>,
}

/// Something the connecting pane should say about the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanNote {
    /// Automatic went straight to Tailscale: the primary address is private
    /// and this PC has no route to it.
    PrimarySkipped,
    /// Automatic cannot fall back to Tailscale, for this reason.
    NoFallback(&'static str),
    /// Tailscale knows the device at the Tailscale address and says it is
    /// offline. Holonet still tries: the status can lag.
    PeerOffline,
}

/// Why no attempt can be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanProblem {
    /// The route is Tailscale, and Tailscale cannot be used, for this reason.
    TailscaleUnavailable(&'static str),
    /// Tailscale was asked for (for example "Try Tailscale") but the host has
    /// no Tailscale address.
    NoTailscaleAddress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub attempts: Vec<Attempt>,
    pub notes: Vec<PlanNote>,
}

/// Plan one connection.
///
/// - `route_override`: a per-connection choice (`decisions.md`: "the choice
///   can be overridden per connection"), such as "Try Tailscale".
/// - `primary_unreachable`: from [`crate::route::primary_unreachable`].
/// - `fallback_wait`: Settings, Connection (3 s by default).
pub fn plan(
    host: &Host,
    route_override: Option<Route>,
    tailscale: &Tailscale,
    primary_unreachable: bool,
    fallback_wait: Duration,
) -> Result<Plan, PlanProblem> {
    let primary = host.address.trim().to_string();
    let route = match route_override {
        Some(Route::Tailscale) if host.tailscale().is_none() => return Err(PlanProblem::NoTailscaleAddress),
        Some(route) => route,
        None => host.effective_route(),
    };
    let attempt = |address: &str, via, give_up_after| Attempt { address: address.to_string(), via, give_up_after };
    let mut notes = Vec::new();
    let offline = |address: &str| match tailscale {
        Tailscale::Running(s) => s.peer(address).is_some_and(|p| !p.online),
        _ => false,
    };
    let attempts = match (route, host.tailscale()) {
        (Route::Primary, _) | (Route::Automatic, None) => vec![attempt(&primary, Via::Primary, None)],
        (Route::Tailscale, None) => return Err(PlanProblem::NoTailscaleAddress),
        (Route::Tailscale, Some(ts)) => {
            if let Some(problem) = tailscale.problem() {
                return Err(PlanProblem::TailscaleUnavailable(problem));
            }
            if offline(ts) {
                notes.push(PlanNote::PeerOffline);
            }
            vec![attempt(ts, Via::Tailscale, None)]
        }
        (Route::Automatic, Some(ts)) => match tailscale.problem() {
            Some(problem) => {
                notes.push(PlanNote::NoFallback(problem));
                vec![attempt(&primary, Via::Primary, None)]
            }
            None => {
                if offline(ts) {
                    notes.push(PlanNote::PeerOffline);
                }
                if primary_unreachable {
                    notes.push(PlanNote::PrimarySkipped);
                    vec![attempt(ts, Via::Tailscale, None)]
                } else {
                    vec![attempt(&primary, Via::Primary, Some(fallback_wait)), attempt(ts, Via::Tailscale, None)]
                }
            }
        },
    };
    Ok(Plan { attempts, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tailscale::{Peer, TailscaleStatus};

    const WAIT: Duration = Duration::from_secs(3);

    fn host(tailscale: Option<&str>, route: Route) -> Host {
        let mut h = Host::new("192.168.1.51");
        h.tailscale_address = tailscale.map(String::from);
        h.route = route;
        h
    }

    fn running(online: bool) -> Tailscale {
        Tailscale::Running(TailscaleStatus {
            own_ips: vec!["100.64.0.10".parse().unwrap()],
            magic_dns_suffix: "tail1234.ts.net".into(),
            peers: vec![Peer {
                host_name: "pi-cluster-1".into(),
                dns_name: "pi-cluster-1.tail1234.ts.net".into(),
                ips: vec!["100.64.0.21".parse().unwrap()],
                online,
            }],
        })
    }

    fn addresses(p: &Plan) -> Vec<(&str, Via, Option<Duration>)> {
        p.attempts.iter().map(|a| (a.address.as_str(), a.via, a.give_up_after)).collect()
    }

    #[test]
    fn automatic_tries_primary_then_tailscale_after_the_wait() {
        let p = plan(&host(Some("100.64.0.21"), Route::Automatic), None, &running(true), false, WAIT).unwrap();
        assert_eq!(addresses(&p), [("192.168.1.51", Via::Primary, Some(WAIT)), ("100.64.0.21", Via::Tailscale, None)]);
        assert!(p.notes.is_empty());
    }

    #[test]
    fn automatic_skips_an_unreachable_private_primary() {
        let p = plan(&host(Some("100.64.0.21"), Route::Automatic), None, &running(true), true, WAIT).unwrap();
        assert_eq!(addresses(&p), [("100.64.0.21", Via::Tailscale, None)]);
        assert_eq!(p.notes, [PlanNote::PrimarySkipped]);
    }

    #[test]
    fn automatic_without_tailscale_running_says_so_and_uses_primary() {
        let stopped = Tailscale::NotConnected { state: "Stopped".into() };
        let p = plan(&host(Some("100.64.0.21"), Route::Automatic), None, &stopped, true, WAIT).unwrap();
        assert_eq!(addresses(&p), [("192.168.1.51", Via::Primary, None)]);
        assert!(matches!(p.notes[..], [PlanNote::NoFallback(text)] if text.contains("switched off")));
    }

    #[test]
    fn no_tailscale_address_means_primary_only() {
        for route in [Route::Automatic, Route::Primary, Route::Tailscale] {
            let p = plan(&host(None, route), None, &running(true), true, WAIT).unwrap();
            assert_eq!(addresses(&p), [("192.168.1.51", Via::Primary, None)], "{route:?}");
        }
    }

    #[test]
    fn primary_route_never_uses_tailscale() {
        let p = plan(&host(Some("100.64.0.21"), Route::Primary), None, &running(true), true, WAIT).unwrap();
        assert_eq!(addresses(&p), [("192.168.1.51", Via::Primary, None)]);
    }

    #[test]
    fn tailscale_route_needs_tailscale() {
        let h = host(Some("pi-cluster-1"), Route::Tailscale);
        let p = plan(&h, None, &running(false), false, WAIT).unwrap();
        assert_eq!(addresses(&p), [("pi-cluster-1", Via::Tailscale, None)]);
        assert_eq!(p.notes, [PlanNote::PeerOffline]);
        let err = plan(&h, None, &Tailscale::NotRunning, false, WAIT).unwrap_err();
        assert_eq!(err, PlanProblem::TailscaleUnavailable("Tailscale is not running on this PC."));
    }

    #[test]
    fn try_tailscale_overrides_a_primary_host() {
        let h = host(Some("100.64.0.21"), Route::Primary);
        let p = plan(&h, Some(Route::Tailscale), &running(true), false, WAIT).unwrap();
        assert_eq!(addresses(&p), [("100.64.0.21", Via::Tailscale, None)]);
        let no_ts = host(None, Route::Primary);
        assert_eq!(
            plan(&no_ts, Some(Route::Tailscale), &running(true), false, WAIT),
            Err(PlanProblem::NoTailscaleAddress)
        );
    }
}
