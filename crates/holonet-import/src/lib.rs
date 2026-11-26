//! Readers for what people already have from OpenSSH (`decisions.md`:
//! imports are copied; originals are untouched).
//!
//! - [`config`]: `~/.ssh/config` into hosts, with what was not imported.
//! - [`known_hosts`]: `known_hosts` lines, hashed names, and a writer for
//!   export.
//! - [`command`]: Quick fill, an `ssh` command line or one `Host` block.
//!
//! No I/O: callers read the files and pass the text in.

pub mod command;
pub mod config;
pub mod known_hosts;

/// A jump host as `ProxyJump` and `-J` write it: `[user@]host[:port]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl JumpSpec {
    /// One hop: `host`, `user@host`, `host:port`, `[::1]:22`, or
    /// `ssh://user@host:port`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix("ssh://").unwrap_or(text);
        let (user, rest) = match text.rsplit_once('@') {
            Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
            Some(_) => return None,
            None => (None, text),
        };
        let (host, port) = split_host_port(rest)?;
        Some(Self { user, host, port })
    }

    /// A comma-separated chain, first hop first. `none` is no chain.
    pub fn parse_chain(text: &str) -> Option<Vec<Self>> {
        if text.trim().eq_ignore_ascii_case("none") {
            return Some(Vec::new());
        }
        text.split(',').map(Self::parse).collect()
    }
}

/// `host`, `host:port` or `[v6]:port`. A bare IPv6 address has no port.
fn split_host_port(text: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p.parse().ok().filter(|p| *p != 0)?),
            None if after.is_empty() => None,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }
    match text.split_once(':') {
        Some((host, port)) if !port.contains(':') => {
            let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
            (!host.is_empty()).then(|| (host.to_string(), Some(port)))
        }
        // More than one colon: an IPv6 address without brackets.
        Some(_) => Some((text.to_string(), None)),
        None => (!text.is_empty()).then(|| (text.to_string(), None)),
    }
}

/// OpenSSH's `match_pattern`: `*` is any run, `?` any one character,
/// compared without case as host names are.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether a name has wildcards, so it names many hosts and is not one.
pub fn is_pattern(name: &str) -> bool {
    name.contains(['*', '?', '!'])
}

/// Split a line into words as OpenSSH does: spaces separate, double quotes
/// group, and an unquoted `#` at the start of a word ends the line.
pub(crate) fn words(line: &str) -> Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None if c == '#' && !in_word => break,
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote is not closed.");
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_specs() {
        let j = |s: &str| JumpSpec::parse(s);
        assert_eq!(j("bastion-east"), Some(JumpSpec { user: None, host: "bastion-east".into(), port: None }));
        assert_eq!(
            j("ops@203.0.113.20:2222"),
            Some(JumpSpec { user: Some("ops".into()), host: "203.0.113.20".into(), port: Some(2222) })
        );
        assert_eq!(j("[fd00::1]:22").unwrap().host, "fd00::1");
        assert_eq!(j("fd00::1").unwrap(), JumpSpec { user: None, host: "fd00::1".into(), port: None });
        assert_eq!(j("ssh://pi@nas:2200").unwrap().port, Some(2200));
        assert_eq!(j("host:0"), None);
        assert_eq!(j("@host"), None);
        assert_eq!(j(""), None);
        let chain = JumpSpec::parse_chain("a,ops@b:2222").unwrap();
        assert_eq!(chain.iter().map(|h| h.host.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(JumpSpec::parse_chain("None"), Some(vec![]));
        assert_eq!(JumpSpec::parse_chain("a,,b"), None);
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("prod-*", "PROD-web-01"));
        assert!(wildcard_match("web-0?", "web-07"));
        assert!(!wildcard_match("web-0?", "web-007"));
        assert!(wildcard_match("*.example.com", "a.b.example.com"));
        assert!(!wildcard_match("*.example.com", "example.com"));
        assert!(wildcard_match("a*b*c", "aXXbYYc") && !wildcard_match("a*b*c", "aXXbYY"));
        assert!(is_pattern("prod-*") && is_pattern("!x") && !is_pattern("prod-web-01"));
    }

    #[test]
    fn splitting_words() {
        assert_eq!(
            words(r#"  IdentityFile "C:\Users\Me\.ssh\id ed" # mine"#).unwrap(),
            ["IdentityFile", r"C:\Users\Me\.ssh\id ed"]
        );
        assert_eq!(words("Host a#b c").unwrap(), ["Host", "a#b", "c"]);
        assert!(words(r#"User "x"#).is_err());
        assert!(words("   # only a comment").unwrap().is_empty());
    }
}
