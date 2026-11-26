//! Port forwarding rules (PortForwarding.dc.html): local, remote and
//! dynamic (SOCKS5) tunnels through a saved host.

use serde::{Deserialize, Serialize};

use crate::{ForwardRuleId, HostId, is_valid_address};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// A port on this PC reaches a service the host can see (`ssh -L`).
    #[default]
    Local,
    /// A port on the host reaches a service on this PC (`ssh -R`).
    Remote,
    /// A SOCKS5 proxy on this PC sends traffic out through the host (`ssh -D`).
    Dynamic,
}

impl ForwardKind {
    pub const ALL: [ForwardKind; 3] = [ForwardKind::Local, ForwardKind::Remote, ForwardKind::Dynamic];

    pub fn label(self) -> &'static str {
        match self {
            ForwardKind::Local => "Local",
            ForwardKind::Remote => "Remote",
            ForwardKind::Dynamic => "Dynamic",
        }
    }

    /// The one-line description under each type on the form.
    pub fn description(self) -> &'static str {
        match self {
            ForwardKind::Local => "A port on this PC reaches a service the host can see.",
            ForwardKind::Remote => "A port on the host reaches a service on this PC.",
            ForwardKind::Dynamic => "A SOCKS5 proxy on this PC sends traffic out through the host.",
        }
    }

    /// The single letter in the status bar ("Forward L 5432 → …").
    pub fn letter(self) -> char {
        match self {
            ForwardKind::Local => 'L',
            ForwardKind::Remote => 'R',
            ForwardKind::Dynamic => 'D',
        }
    }
}

pub const LOOPBACK: &str = "127.0.0.1";

fn loopback() -> String {
    LOOPBACK.to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardRule {
    pub id: ForwardRuleId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: ForwardKind,
    /// The host the tunnel runs through.
    pub host: HostId,
    /// Where the listening port is bound: on this PC for Local and Dynamic,
    /// on the host for Remote.
    #[serde(default = "loopback")]
    pub bind_address: String,
    pub port: u16,
    /// Where connections go: as the host sees it for Local, on this PC for
    /// Remote. Unused for Dynamic.
    #[serde(default)]
    pub destination_host: String,
    #[serde(default)]
    pub destination_port: u16,
    /// "Start when this host connects".
    #[serde(default)]
    pub start_with_host: bool,
}

/// A problem that stops a rule from being saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardProblem {
    PortZero,
    BindAddressInvalid,
    DestinationMissing,
    DestinationInvalid,
    DestinationPortZero,
}

impl ForwardProblem {
    pub fn message(self) -> &'static str {
        match self {
            ForwardProblem::PortZero => "Use a port from 1 to 65535.",
            ForwardProblem::BindAddressInvalid => "Use an IP address, such as 127.0.0.1 or 0.0.0.0.",
            ForwardProblem::DestinationMissing => "Add the destination's address.",
            ForwardProblem::DestinationInvalid => "Use a hostname or an IP address, without spaces or a port.",
            ForwardProblem::DestinationPortZero => "Use a destination port from 1 to 65535.",
        }
    }
}

impl ForwardRule {
    pub fn new(kind: ForwardKind, host: HostId) -> Self {
        Self {
            id: ForwardRuleId::new(),
            name: String::new(),
            kind,
            host,
            bind_address: loopback(),
            port: 0,
            destination_host: String::new(),
            destination_port: 0,
            start_with_host: false,
        }
    }

    /// The name to show: the label, or e.g. "Local 8080".
    pub fn display_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() { format!("{} {}", self.kind.label(), self.port) } else { name.to_string() }
    }

    fn bind(&self) -> &str {
        let bind = self.bind_address.trim();
        if bind.is_empty() { LOOPBACK } else { bind }
    }

    /// Whether only this PC (or, for Remote, only the host) can reach the
    /// listening port.
    pub fn is_loopback(&self) -> bool {
        match self.bind().parse::<std::net::IpAddr>() {
            Ok(ip) => ip.is_loopback(),
            Err(_) => self.bind().eq_ignore_ascii_case("localhost"),
        }
    }

    /// `host:port` of the destination.
    pub fn destination(&self) -> String {
        format!("{}:{}", self.destination_host.trim(), self.destination_port)
    }

    /// The `ssh` flag for the equivalent command, as drawn: `-L 8080:localhost:80`,
    /// `-R 9000:127.0.0.1:3000`, `-D 1080`. A bind address other than
    /// loopback is written in front.
    pub fn ssh_flag(&self) -> String {
        let bind = if self.is_loopback() { String::new() } else { format!("{}:", self.bind()) };
        match self.kind {
            ForwardKind::Local => format!("-L {bind}{}:{}", self.port, self.destination()),
            ForwardKind::Remote => format!("-R {bind}{}:{}", self.port, self.destination()),
            ForwardKind::Dynamic => format!("-D {bind}{}", self.port),
        }
    }

    /// `ssh -N -L 8080:localhost:80 staging-api`.
    pub fn ssh_command(&self, host_name: &str) -> String {
        format!("ssh -N {} {host_name}", self.ssh_flag())
    }

    /// The status bar's text: "Forward L 5432 → db-primary:5432".
    pub fn status_text(&self) -> String {
        match self.kind {
            ForwardKind::Dynamic => format!("Forward D {} (SOCKS5)", self.port),
            kind => format!("Forward {} {} → {}", kind.letter(), self.port, self.destination()),
        }
    }

    /// Problems that stop the rule from being saved, in form order.
    pub fn problems(&self) -> Vec<ForwardProblem> {
        let mut problems = Vec::new();
        if self.bind().parse::<std::net::IpAddr>().is_err() && !self.bind().eq_ignore_ascii_case("localhost") {
            problems.push(ForwardProblem::BindAddressInvalid);
        }
        if self.port == 0 {
            problems.push(ForwardProblem::PortZero);
        }
        if self.kind != ForwardKind::Dynamic {
            let destination = self.destination_host.trim();
            if destination.is_empty() {
                problems.push(ForwardProblem::DestinationMissing);
            } else if !is_valid_address(destination) {
                problems.push(ForwardProblem::DestinationInvalid);
            }
            if self.destination_port == 0 {
                problems.push(ForwardProblem::DestinationPortZero);
            }
        }
        problems
    }
}
