//! Choosing how to reach a host: its primary address, its Tailscale address,
//! or both in turn (`decisions.md`, Connection and Tailscale).
//!
//! - [`tailscale`]: whether Tailscale is running on this PC, from its
//!   LocalAPI over a named pipe, as a normal user, without starting a process
//!   (milestone 4 phase 0). `tailscale.exe status --json` is the fallback.
//! - [`route`]: whether this PC has a route to an address, and whether an
//!   address is private, for "skip the wait when the primary address is
//!   private and this PC has no route to it".
//! - [`plan`]: the ordered addresses to try for one connection and how long
//!   to wait on each. A pure function, so the rules are unit tested.
//!
//! No window and no GPUI: everything here tests without one.

pub mod route;
pub mod tailscale;

mod plan;

pub use plan::{Attempt, Plan, PlanNote, PlanProblem, Via, plan};
pub use route::{RouteCheck, is_private, primary_unreachable, route_to};
pub use tailscale::{Tailscale, TailscaleStatus};
