//! Quick fill on the host form (NewHost.dc.html: "paste an ssh command or a
//! ~/.ssh/config entry").
//!
//! An `ssh` command line fills the address, user, port, key file, jump hosts,
//! agent forwarding and compression. A `Host` block reads as config does.
//! Everything else is named in [`QuickFill::unused`], so the form can say
//! what it did not use.

use crate::config::{self, ConfigHost};
use crate::{JumpSpec, words};

/// What Quick fill found. `None` and empty fields leave the form as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickFill {
    /// From a `Host` block's name.
    pub label: Option<String>,
    pub address: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub jump: Vec<JumpSpec>,
    pub forward_agent: Option<bool>,
    pub compression: Option<bool>,
    /// Parts not used, as written ("-L 5432:db:5432", "uptime").
    pub unused: Vec<String>,
}

/// Why nothing could be filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickFillError {
    Empty,
    /// The command has no `user@address` or address.
    NoAddress,
    /// A quote is not closed.
    Unreadable,
    /// The `Host` block names only patterns.
    NoHost,
}

impl QuickFillError {
    pub fn message(self) -> &'static str {
        match self {
            QuickFillError::Empty => "Paste an ssh command or a config entry first.",
            QuickFillError::NoAddress => "This command has no address to connect to.",
            QuickFillError::Unreadable => "A quote in this text is not closed.",
            QuickFillError::NoHost => "This Host entry names only patterns, not a host.",
        }
    }
}

/// `ssh` options that take a value, besides the ones Quick fill reads.
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

pub fn parse(text: &str) -> Result<QuickFill, QuickFillError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(QuickFillError::Empty);
    }
    let first = text.split_whitespace().next().unwrap_or_default();
    if first.eq_ignore_ascii_case("host") || first.to_ascii_lowercase().starts_with("host=") {
        return from_block(text);
    }
    from_command(text)
}

fn from_block(text: &str) -> Result<QuickFill, QuickFillError> {
    let c = config::parse("Quick fill", text, &mut |_| Vec::new());
    let mut hosts = c.hosts.into_iter();
    let Some(h) = hosts.next() else { return Err(QuickFillError::NoHost) };
    let ConfigHost { name, address, port, user, identity_files, jump, forward_agent, compression, notes, .. } = h;
    let mut unused: Vec<String> = hosts.map(|h| format!("Host {}", h.name)).collect();
    let mut files = identity_files.into_iter();
    let identity_file = files.next();
    unused.extend(files.map(|f| format!("IdentityFile {f}")));
    unused.extend(c.skipped.into_iter().map(|s| s.what));
    unused.extend(notes);
    Ok(QuickFill { label: Some(name), address, user, port, identity_file, jump, forward_agent, compression, unused })
}

fn from_command(text: &str) -> Result<QuickFill, QuickFillError> {
    let mut args = words(text).map_err(|_| QuickFillError::Unreadable)?.into_iter().peekable();
    if args.peek().is_some_and(|a| {
        let a = a.to_ascii_lowercase();
        a == "ssh" || a == "ssh.exe" || a.ends_with("/ssh") || a.ends_with("\\ssh.exe") || a.ends_with("\\ssh")
    }) {
        args.next();
    }
    let mut q = QuickFill::default();
    let mut destination: Option<String> = None;
    let mut port_option: Option<u16> = None;
    let mut user_option: Option<String> = None;
    let mut command = false;
    while let Some(arg) = args.next() {
        if command {
            q.unused.push(arg);
            continue;
        }
        if arg == "--" {
            continue;
        }
        // ssh reads options after the destination too (`ssh host -p 22`).
        // The first other word after it starts the remote command, which
        // Holonet does not run.
        let Some(flags) = arg.strip_prefix('-').filter(|f| !f.is_empty()) else {
            if destination.is_none() {
                destination = Some(arg);
            } else {
                command = true;
                q.unused.push(arg);
            }
            continue;
        };
        // Flags may be grouped (-AC) and a value may be attached (-p2222).
        let chars: Vec<char> = flags.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if WITH_VALUE.contains(c) {
                let attached: String = chars[i + 1..].iter().collect();
                let value = if attached.is_empty() { args.next().unwrap_or_default() } else { attached };
                match c {
                    'p' => match value.parse::<u16>() {
                        Ok(p) if p != 0 => port_option = Some(p),
                        _ => q.unused.push(format!("-p {value}")),
                    },
                    'l' => user_option = Some(value),
                    'i' => q.identity_file = Some(value),
                    'J' => match JumpSpec::parse_chain(&value) {
                        Some(chain) => q.jump = chain,
                        None => q.unused.push(format!("-J {value}")),
                    },
                    'o' => option(&value, &mut q, &mut port_option, &mut user_option),
                    _ => q.unused.push(format!("-{c} {value}")),
                }
                break;
            }
            match c {
                'A' => q.forward_agent = Some(true),
                'a' => q.forward_agent = Some(false),
                'C' => q.compression = Some(true),
                _ => q.unused.push(format!("-{c}")),
            }
            i += 1;
        }
    }
    let Some(destination) = destination else { return Err(QuickFillError::NoAddress) };
    // `user@host`, or `ssh://user@host:port`.
    let (user, host, port) = match destination.strip_prefix("ssh://") {
        Some(uri) => {
            let spec = JumpSpec::parse(uri).ok_or(QuickFillError::NoAddress)?;
            (spec.user, spec.host, spec.port)
        }
        None => match destination.rsplit_once('@') {
            Some((u, h)) if !u.is_empty() && !h.is_empty() => (Some(u.to_string()), h.to_string(), None),
            Some(_) => return Err(QuickFillError::NoAddress),
            None => (None, destination, None),
        },
    };
    q.address = host;
    // `user@` wins over -l, and a URI's port over -p, as in ssh.
    q.user = user.or(user_option);
    q.port = port.or(port_option);
    Ok(q)
}

/// `-o Key=Value` for the settings the host form has.
fn option(value: &str, q: &mut QuickFill, port: &mut Option<u16>, user: &mut Option<String>) {
    let (k, v) = match value.split_once('=') {
        Some((k, v)) => (k.trim(), v.trim()),
        None => match value.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (value, ""),
        },
    };
    let yes = |v: &str| match v.to_ascii_lowercase().as_str() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    };
    let used = match k.to_ascii_lowercase().as_str() {
        "port" => v.parse::<u16>().ok().filter(|p| *p != 0).map(|p| *port = Some(p)).is_some(),
        "user" if !v.is_empty() => {
            *user = Some(v.to_string());
            true
        }
        "identityfile" if !v.is_empty() => {
            q.identity_file = Some(v.to_string());
            true
        }
        "proxyjump" => JumpSpec::parse_chain(v).map(|c| q.jump = c).is_some(),
        "forwardagent" => yes(v).map(|b| q.forward_agent = Some(b)).is_some(),
        "compression" => yes(v).map(|b| q.compression = Some(b)).is_some(),
        _ => false,
    };
    if !used {
        q.unused.push(format!("-o {value}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jump(host: &str) -> JumpSpec {
        JumpSpec { user: None, host: host.into(), port: None }
    }

    #[test]
    fn the_drawn_example() {
        let q = parse("ssh -J bastion-east deploy@10.30.0.9 -p 22").unwrap();
        assert_eq!(q.address, "10.30.0.9");
        assert_eq!((q.user.as_deref(), q.port), (Some("deploy"), Some(22)));
        assert!(q.unused.is_empty());
        assert_eq!(q.jump, [jump("bastion-east")]);
    }

    #[test]
    fn options_in_their_forms() {
        let q = parse(r#"ssh.exe -AC -p2222 -l root -i "C:\Users\Me\.ssh\id ed" -o Compression=no -o "ForwardAgent no" -L 5432:db:5432 -v nas"#)
            .unwrap();
        assert_eq!((q.address.as_str(), q.user.as_deref(), q.port), ("nas", Some("root"), Some(2222)));
        assert_eq!(q.identity_file.as_deref(), Some(r"C:\Users\Me\.ssh\id ed"));
        // Later options win, as written.
        assert_eq!((q.forward_agent, q.compression), (Some(false), Some(false)));
        assert_eq!(q.unused, ["-L 5432:db:5432", "-v"]);
        // user@ wins over -l; a URI's port over -p.
        let q = parse("ssh -l root -p 22 ssh://pi@nas-01:2200").unwrap();
        assert_eq!((q.user.as_deref(), q.port, q.address.as_str()), (Some("pi"), Some(2200), "nas-01"));
        // No "ssh" at all, a jump chain, and a remote command.
        let q = parse("-J a,ops@b:2222 deploy@prod uptime -h").unwrap();
        assert_eq!(q.jump.len(), 2);
        assert_eq!(q.unused, ["uptime", "-h"]);
        let q = parse("ssh -o Port=2022 -o User=ops -o StrictHostKeyChecking=no h").unwrap();
        assert_eq!((q.port, q.user.as_deref()), (Some(2022), Some("ops")));
        assert_eq!(q.unused, ["-o StrictHostKeyChecking=no"]);
    }

    #[test]
    fn what_cannot_be_filled() {
        assert_eq!(parse("  "), Err(QuickFillError::Empty));
        assert_eq!(parse("ssh -p 22"), Err(QuickFillError::NoAddress));
        assert_eq!(parse("ssh @host"), Err(QuickFillError::NoAddress));
        assert_eq!(parse(r#"ssh "deploy@x"#), Err(QuickFillError::Unreadable));
        assert_eq!(parse("Host *.corp\n User x"), Err(QuickFillError::NoHost));
        let q = parse("ssh -p 0 x").unwrap();
        assert_eq!((q.port, q.unused.as_slice()), (None, ["-p 0".to_string()].as_slice()));
    }

    #[test]
    fn a_config_entry() {
        let q = parse(
            "Host staging-api
    HostName 10.30.0.4
    User deploy
    Port 2022
    IdentityFile ~/.ssh/work
    IdentityFile ~/.ssh/old
    ProxyJump bastion-east
    ServerAliveInterval 30
Host other",
        )
        .unwrap();
        assert_eq!(q.label.as_deref(), Some("staging-api"));
        assert_eq!((q.address.as_str(), q.user.as_deref(), q.port), ("10.30.0.4", Some("deploy"), Some(2022)));
        assert_eq!(q.identity_file.as_deref(), Some("~/.ssh/work"));
        assert_eq!(q.jump, [jump("bastion-east")]);
        assert_eq!(q.unused, ["Host other", "IdentityFile ~/.ssh/old", "ServerAliveInterval"]);
        // Host= works too, and the address is the name without HostName.
        assert_eq!(parse("host=pi\n user pi").unwrap().address, "pi");
    }
}
