//! OpenSSH `config` into hosts (FirstRun.dc.html: "Hosts, ports, users,
//! jump hosts and identity files").
//!
//! Values are found as `ssh` finds them: for each host name, every block
//! that applies is read in file order, and the first value of each setting
//! wins. So `Host *` at the end fills in what hosts left unset, and `Host *`
//! at the start overrides them, as it does for `ssh`. `IdentityFile`
//! collects every value, as `ssh` tries each.
//!
//! Read: `HostName`, `Port`, `User`, `IdentityFile`, `ProxyJump`,
//! `ForwardAgent` and `Compression`, with `Include` followed. Everything
//! else is reported, never guessed at.

use crate::{JumpSpec, is_pattern, wildcard_match, words};

/// Where a line came from, for messages: the file as the caller named it,
/// and its line number from 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub file: String,
    pub line: usize,
}

/// One host to import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigHost {
    /// The name after `Host`, which becomes the label.
    pub name: String,
    /// `HostName`, or the name itself.
    pub address: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    /// As written (`~/.ssh/id_ed25519`); the caller finds the files.
    pub identity_files: Vec<String>,
    /// `ProxyJump`, first hop first. Empty for none.
    pub jump: Vec<JumpSpec>,
    pub forward_agent: Option<bool>,
    pub compression: Option<bool>,
    /// The first `Host` line that names it.
    pub source: Source,
    /// Settings for this host that were not imported, as sentences.
    pub notes: Vec<String>,
}

/// Something in the file that was not imported, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub what: String,
    pub reason: String,
    pub source: Option<Source>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub hosts: Vec<ConfigHost>,
    pub skipped: Vec<Skipped>,
}

/// A file named by `Include`, read by the caller.
pub struct Included {
    /// How to name it in messages.
    pub name: String,
    /// Its text, or why it could not be read.
    pub text: Result<String, String>,
}

/// `Include` nested deeper than this is a loop (`ssh` uses 16 too).
const MAX_DEPTH: usize = 16;

struct Line {
    words: Vec<String>,
    source: Source,
}

enum Block {
    /// Lines before the first `Host` or `Match`: they apply to every host.
    Global(Vec<Line>),
    Host {
        patterns: Vec<String>,
        lines: Vec<Line>,
    },
    Match,
}

/// Read a config. `include` is called with each `Include` argument (as
/// written, with `~` and wildcards) and returns the files it names, in
/// order; relative names are relative to `~/.ssh`, which is the caller's.
pub fn parse(file: &str, text: &str, include: &mut dyn FnMut(&str) -> Vec<Included>) -> Config {
    let mut skipped = Vec::new();
    let mut lines = Vec::new();
    expand(file, text, include, 0, &mut lines, &mut skipped);

    // Blocks, and host names in the order they first appear.
    let mut blocks = vec![Block::Global(Vec::new())];
    let mut names: Vec<(String, Source)> = Vec::new();
    for line in lines {
        let keyword = line.words[0].to_ascii_lowercase();
        match keyword.as_str() {
            "host" => {
                let patterns: Vec<String> = line.words[1..].to_vec();
                for p in &patterns {
                    if !is_pattern(p) {
                        if !names.iter().any(|(n, _)| n.eq_ignore_ascii_case(p)) {
                            names.push((p.clone(), line.source.clone()));
                        }
                    } else if p != "*" && !p.starts_with('!') {
                        skipped.push(Skipped {
                            what: format!("Host {p}"),
                            reason: "A pattern is not a host, so it is not imported. Its settings are used for the hosts it matches.".into(),
                            source: Some(line.source.clone()),
                        });
                    }
                }
                if patterns.is_empty() {
                    skipped.push(Skipped {
                        what: "Host".into(),
                        reason: "This Host line names no host.".into(),
                        source: Some(line.source.clone()),
                    });
                }
                blocks.push(Block::Host { patterns, lines: Vec::new() });
            }
            "match" => {
                skipped.push(Skipped {
                    what: line.words.join(" "),
                    reason: "Match blocks are not imported, nor the settings in them.".into(),
                    source: Some(line.source.clone()),
                });
                blocks.push(Block::Match);
            }
            // Lines in a Match block are dropped with it.
            _ => {
                if let Some(Block::Global(lines) | Block::Host { lines, .. }) = blocks.last_mut() {
                    lines.push(line);
                }
            }
        }
    }

    // Settings Holonet does not use, named once each for the whole file.
    let mut unused: Vec<String> = Vec::new();
    for block in &blocks {
        if let Block::Global(lines) | Block::Host { lines, .. } = block {
            for line in lines {
                let k = line.words[0].to_ascii_lowercase();
                if !USED.contains(&k.as_str())
                    && !NOTED.iter().any(|(n, _)| *n == k)
                    && !unused.iter().any(|u| u.eq_ignore_ascii_case(&line.words[0]))
                {
                    unused.push(line.words[0].clone());
                }
            }
        }
    }
    if !unused.is_empty() {
        skipped.push(Skipped {
            what: unused.join(", "),
            reason: "Holonet does not use these settings, so they are not imported.".into(),
            source: None,
        });
    }

    let hosts = names.into_iter().map(|(name, source)| resolve(&name, source, &blocks)).collect();
    Config { hosts, skipped }
}

const USED: &[&str] =
    &["hostname", "port", "user", "identityfile", "proxyjump", "forwardagent", "compression", "include"];

/// Settings reported on each host they apply to.
const NOTED: &[(&str, &str)] = &[
    ("proxycommand", "ProxyCommand is not imported. Holonet connects directly or through jump hosts."),
    ("localforward", "Port forwards are not imported. Add them on the Port forwarding page."),
    ("remoteforward", "Port forwards are not imported. Add them on the Port forwarding page."),
    ("dynamicforward", "Port forwards are not imported. Add them on the Port forwarding page."),
    ("certificatefile", "Certificates are not imported."),
];

fn applies(patterns: &[String], name: &str) -> bool {
    let mut hit = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if wildcard_match(neg, name) {
                return false;
            }
        } else if wildcard_match(p, name) {
            hit = true;
        }
    }
    hit
}

fn resolve(name: &str, source: Source, blocks: &[Block]) -> ConfigHost {
    let mut h = ConfigHost {
        name: name.to_string(),
        address: name.to_string(),
        port: None,
        user: None,
        identity_files: Vec::new(),
        jump: Vec::new(),
        forward_agent: None,
        compression: None,
        source,
        notes: Vec::new(),
    };
    let mut seen: Vec<&'static str> = Vec::new();
    let mut first = |k: &'static str| {
        let new = !seen.contains(&k);
        seen.push(k);
        new
    };
    let note = |h: &mut ConfigHost, text: String| {
        if !h.notes.contains(&text) {
            h.notes.push(text);
        }
    };
    for block in blocks {
        let lines = match block {
            Block::Global(lines) => lines,
            Block::Host { patterns, lines } if applies(patterns, name) => lines,
            _ => continue,
        };
        for line in lines {
            let k = line.words[0].to_ascii_lowercase();
            let args = &line.words[1..];
            let value = args.join(" ");
            let at = format!("line {}", line.source.line);
            if args.is_empty() {
                note(&mut h, format!("{} on {at} has no value, so it is not used.", line.words[0]));
                continue;
            }
            match k.as_str() {
                "hostname" if first("hostname") => h.address = expand_host(&value, name),
                "port" if first("port") => match value.parse::<u16>() {
                    Ok(p) if p != 0 => h.port = Some(p),
                    _ => note(&mut h, format!("Port {value} on {at} is not a port from 1 to 65535, so 22 is used.")),
                },
                "user" if first("user") => h.user = Some(value),
                "identityfile" => {
                    if !value.eq_ignore_ascii_case("none") && !h.identity_files.contains(&value) {
                        h.identity_files.push(value);
                    }
                }
                "proxyjump" if first("proxyjump") => match JumpSpec::parse_chain(&value) {
                    Some(chain) => h.jump = chain,
                    None => note(&mut h, format!("ProxyJump {value} on {at} could not be read, so it is not used.")),
                },
                "forwardagent" if first("forwardagent") => match yes_no(&value) {
                    Some(v) => h.forward_agent = Some(v),
                    None => note(&mut h, format!("ForwardAgent {value} on {at} is not yes or no, so it is not used.")),
                },
                "compression" if first("compression") => match yes_no(&value) {
                    Some(v) => h.compression = Some(v),
                    None => note(&mut h, format!("Compression {value} on {at} is not yes or no, so it is not used.")),
                },
                _ => {
                    if let Some((_, text)) = NOTED.iter().find(|(n, _)| *n == k) {
                        note(&mut h, (*text).to_string());
                    }
                }
            }
        }
    }
    h
}

fn yes_no(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

/// `HostName` tokens: `%h` is the name and `%%` a percent sign.
fn expand_host(value: &str, name: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(name),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Lines of `text`, with each `Include` replaced by the files it names.
fn expand(
    file: &str,
    text: &str,
    include: &mut dyn FnMut(&str) -> Vec<Included>,
    depth: usize,
    out: &mut Vec<Line>,
    skipped: &mut Vec<Skipped>,
) {
    for (i, raw) in text.lines().enumerate() {
        let source = Source { file: file.to_string(), line: i + 1 };
        let mut ws = match words(raw) {
            Ok(ws) => ws,
            Err(why) => {
                skipped.push(Skipped { what: raw.trim().to_string(), reason: why.into(), source: Some(source) });
                continue;
            }
        };
        if ws.is_empty() {
            continue;
        }
        // `Keyword=value` and `Keyword = value`.
        if let Some((k, v)) = ws[0].split_once('=') {
            let (k, v) = (k.to_string(), v.to_string());
            ws.remove(0);
            if !v.is_empty() {
                ws.insert(0, v);
            }
            ws.insert(0, k);
        } else if ws.get(1).is_some_and(|w| w == "=") {
            ws.remove(1);
        } else if let Some(v) = ws.get(1).and_then(|w| w.strip_prefix('=')).map(str::to_string) {
            if v.is_empty() {
                ws.remove(1);
            } else {
                ws[1] = v;
            }
        }
        if !ws[0].eq_ignore_ascii_case("include") {
            out.push(Line { words: ws, source });
            continue;
        }
        if depth >= MAX_DEPTH {
            skipped.push(Skipped {
                what: ws.join(" "),
                reason: "Include goes too deep, as if files include each other, so it is not followed.".into(),
                source: Some(source),
            });
            continue;
        }
        for arg in &ws[1..] {
            for inc in include(arg) {
                match inc.text {
                    Ok(text) => expand(&inc.name, &text, include, depth + 1, out, skipped),
                    Err(why) => skipped.push(Skipped {
                        what: format!("Include {}", inc.name),
                        reason: why,
                        source: Some(source.clone()),
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Config {
        parse("config", text, &mut |_| Vec::new())
    }

    fn host<'a>(c: &'a Config, name: &str) -> &'a ConfigHost {
        c.hosts.iter().find(|h| h.name == name).unwrap_or_else(|| panic!("no host {name}"))
    }

    #[test]
    fn hosts_with_their_settings() {
        let c = read(
            "# Work
Host prod-web-01 prod-web-02
    HostName %h.example.com
    User deploy
    IdentityFile ~/.ssh/work_ed25519
    ProxyJump bastion-east

Host bastion-east
    HostName=203.0.113.20
    Port = 2222
    User ops
    ForwardAgent yes
    Compression no

Host nas-01
  hostname 192.168.1.40
  IdentityFile \"~/.ssh/home key\"
",
        );
        assert_eq!(
            c.hosts.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            ["prod-web-01", "prod-web-02", "bastion-east", "nas-01"]
        );
        let web = host(&c, "prod-web-02");
        assert_eq!(web.address, "prod-web-02.example.com");
        assert_eq!((web.user.as_deref(), web.port), (Some("deploy"), None));
        assert_eq!(web.identity_files, ["~/.ssh/work_ed25519"]);
        assert_eq!(web.jump, [JumpSpec { user: None, host: "bastion-east".into(), port: None }]);
        assert_eq!(web.source, Source { file: "config".into(), line: 2 });
        let b = host(&c, "bastion-east");
        assert_eq!(
            (b.address.as_str(), b.port, b.forward_agent, b.compression),
            ("203.0.113.20", Some(2222), Some(true), Some(false))
        );
        assert_eq!(host(&c, "nas-01").identity_files, ["~/.ssh/home key"]);
        assert!(c.skipped.is_empty(), "{:?}", c.skipped);
    }

    #[test]
    fn first_value_wins_as_in_ssh() {
        let c = read(
            "User everyone
Host *
    Port 2200
Host a
    Port 22
    User alice
    IdentityFile ~/.ssh/a
Host b
    HostName b.lan
Host *
    User fallback
    IdentityFile ~/.ssh/default
    Compression yes
",
        );
        let a = host(&c, "a");
        // The global User and the first Host * port come first.
        assert_eq!((a.user.as_deref(), a.port), (Some("everyone"), Some(2200)));
        // IdentityFile collects, in order.
        assert_eq!(a.identity_files, ["~/.ssh/a", "~/.ssh/default"]);
        let b = host(&c, "b");
        assert_eq!((b.address.as_str(), b.compression), ("b.lan", Some(true)));
        // Host * is defaults, not a host.
        assert_eq!(c.hosts.len(), 2);
    }

    #[test]
    fn patterns_and_negation() {
        let c = read(
            "Host *.corp !build.corp
    User corp-user
    ProxyJump gw.corp
Host db.corp build.corp gw.corp
    Port 22
Host gw.corp
    ProxyJump none
",
        );
        assert_eq!(host(&c, "db.corp").user.as_deref(), Some("corp-user"));
        assert_eq!(host(&c, "db.corp").jump.len(), 1);
        // Excluded by !build.corp.
        assert_eq!(host(&c, "build.corp").user, None);
        // The pattern's ProxyJump comes first, even for gw.corp itself:
        // ssh would loop, and the app sees a jump host that is the host.
        assert_eq!(host(&c, "gw.corp").jump[0].host, "gw.corp");
        assert_eq!(c.skipped.len(), 1);
        assert_eq!(c.skipped[0].what, "Host *.corp");
    }

    #[test]
    fn what_is_not_imported_is_said() {
        let c = read(
            "Host a
    ProxyCommand ssh -W %h:%p gw
    LocalForward 5432 db:5432
    ServerAliveInterval 30
    IdentitiesOnly yes
    Port 99999
    ForwardAgent ~/.ssh/agent.sock
    ProxyJump a,,b
Match host b exec \"true\"
    User matched
Host b
    serveraliveinterval 60
    User \"unclosed
",
        );
        let a = host(&c, "a");
        assert_eq!(a.port, None);
        assert_eq!(a.notes.len(), 5, "{:#?}", a.notes);
        assert!(a.notes[0].starts_with("ProxyCommand"));
        assert!(a.notes.iter().any(|n| n.contains("Port 99999 on line 6")));
        // Match and its settings are skipped, not applied to b.
        assert_eq!(host(&c, "b").user, None);
        let whats: Vec<&str> = c.skipped.iter().map(|s| s.what.as_str()).collect();
        assert!(whats.contains(&"Match host b exec true"), "{whats:?}");
        assert!(whats.contains(&"User \"unclosed"));
        // Unused settings are named once, as first written.
        assert!(whats.contains(&"ServerAliveInterval, IdentitiesOnly"), "{whats:?}");
    }

    #[test]
    fn include_is_followed_in_place() {
        let mut asked = Vec::new();
        let c = parse(
            "config",
            "Include conf.d/*
Host top
    Include extra
Host after
",
            &mut |arg| {
                asked.push(arg.to_string());
                match arg {
                    "conf.d/*" => vec![
                        Included { name: "conf.d/lab".into(), text: Ok("Host lab-1\n  User pi\n".into()) },
                        Included {
                            name: "conf.d/broken".into(),
                            text: Err("Holonet could not read this file.".into()),
                        },
                    ],
                    // Inside a Host block, an included file's first lines
                    // belong to that block.
                    "extra" => vec![Included { name: "extra".into(), text: Ok("Port 2022\nHost from-extra\n".into()) }],
                    _ => Vec::new(),
                }
            },
        );
        assert_eq!(asked, ["conf.d/*", "extra"]);
        assert_eq!(
            c.hosts.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            ["lab-1", "top", "from-extra", "after"]
        );
        assert_eq!(host(&c, "lab-1").source, Source { file: "conf.d/lab".into(), line: 1 });
        assert_eq!(host(&c, "top").port, Some(2022));
        assert_eq!(host(&c, "after").port, None);
        assert_eq!(c.skipped[0].what, "Include conf.d/broken");
    }

    #[test]
    fn include_loops_stop() {
        let c = parse("config", "Include self\nHost x\n", &mut |_| {
            vec![Included { name: "self".into(), text: Ok("Include self\n".into()) }]
        });
        assert_eq!(c.hosts.len(), 1);
        assert!(c.skipped.iter().any(|s| s.reason.starts_with("Include goes too deep")));
    }
}
