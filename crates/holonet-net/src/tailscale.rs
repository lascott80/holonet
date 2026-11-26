//! Is Tailscale running on this PC, and what does it know?
//!
//! Asked of `tailscaled`'s LocalAPI over its named pipe
//! (`\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled`,
//! `GET /localapi/v0/status`). A normal user may ask; it answers in under a
//! millisecond (milestone 4 phase 0). Tokio's named-pipe client is used
//! because `std::fs` could not open that path. If the pipe answers badly,
//! `tailscale.exe status --json` is asked instead.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// How long to wait for Tailscale before treating it as not answering.
const TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(windows)]
const PIPE: &str = r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled";

/// Tailscale on this PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tailscale {
    /// No Tailscale client is installed.
    NotInstalled,
    /// Installed, but its service is not running (or not answering).
    NotRunning,
    /// Running but not connected to a tailnet: `state` is Tailscale's own
    /// word, such as "Stopped" (switched off) or "NeedsLogin".
    NotConnected {
        state: String,
    },
    Running(TailscaleStatus),
}

impl Tailscale {
    pub fn is_running(&self) -> bool {
        matches!(self, Tailscale::Running(_))
    }

    /// Why Tailscale cannot be used, in the interface's words, or `None`
    /// when it can.
    pub fn problem(&self) -> Option<&'static str> {
        match self {
            Tailscale::Running(_) => None,
            Tailscale::NotInstalled => Some("Tailscale is not installed on this PC."),
            Tailscale::NotRunning => Some("Tailscale is not running on this PC."),
            Tailscale::NotConnected { state } if state == "NeedsLogin" => {
                Some("Tailscale on this PC is signed out. Sign in to Tailscale, then try again.")
            }
            Tailscale::NotConnected { .. } => Some("Tailscale on this PC is switched off. Connect it, then try again."),
        }
    }
}

/// What the LocalAPI reports, trimmed to what Holonet uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TailscaleStatus {
    /// This PC's own Tailscale addresses.
    pub own_ips: Vec<IpAddr>,
    /// e.g. "tail1234.ts.net", for recognising tailnet names.
    pub magic_dns_suffix: String,
    pub peers: Vec<Peer>,
}

/// Another device on the tailnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub host_name: String,
    /// Full MagicDNS name without the trailing dot, e.g. "nas.tail1234.ts.net".
    pub dns_name: String,
    pub ips: Vec<IpAddr>,
    pub online: bool,
}

impl TailscaleStatus {
    /// The peer a host's Tailscale address names: a 100.x address, the full
    /// MagicDNS name, or the short name.
    pub fn peer(&self, address: &str) -> Option<&Peer> {
        let address = address.trim().trim_end_matches('.').to_ascii_lowercase();
        if let Ok(ip) = address.parse::<IpAddr>() {
            return self.peers.iter().find(|p| p.ips.contains(&ip));
        }
        self.peers.iter().find(|p| {
            let dns = p.dns_name.to_ascii_lowercase();
            dns == address
                || dns.split('.').next() == Some(address.as_str())
                || p.host_name.eq_ignore_ascii_case(&address)
        })
    }

    /// Whether `address` is one of this PC's own Tailscale addresses.
    pub fn is_own(&self, address: &str) -> bool {
        address.trim().parse::<IpAddr>().is_ok_and(|ip| self.own_ips.contains(&ip))
    }
}

/// Parse the LocalAPI's (or `tailscale status --json`'s) status document.
pub fn parse_status(json: &str) -> Option<Tailscale> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let state = v["BackendState"].as_str()?.to_string();
    if state != "Running" {
        return Some(Tailscale::NotConnected { state });
    }
    let ips = |v: &serde_json::Value| -> Vec<IpAddr> {
        v.as_array().map(|a| a.iter().filter_map(|ip| ip.as_str()?.parse().ok()).collect()).unwrap_or_default()
    };
    let peers = v["Peer"]
        .as_object()
        .map(|peers| {
            peers
                .values()
                .map(|p| Peer {
                    host_name: p["HostName"].as_str().unwrap_or_default().to_string(),
                    dns_name: p["DNSName"].as_str().unwrap_or_default().trim_end_matches('.').to_string(),
                    ips: ips(&p["TailscaleIPs"]),
                    online: p["Online"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Tailscale::Running(TailscaleStatus {
        own_ips: ips(&v["Self"]["TailscaleIPs"]),
        magic_dns_suffix: v["MagicDNSSuffix"].as_str().unwrap_or_default().to_string(),
        peers,
    }))
}

/// Split an HTTP/1.1 response into its status code and body, undoing
/// chunked transfer encoding.
fn parse_http(response: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&response[..split]).ok()?;
    let body = &response[split + 4..];
    let code = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if !chunked {
        return Some((code, body.to_vec()));
    }
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&rest[..line_end]).ok()?;
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Some((code, out));
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

/// Where the Tailscale client would be installed.
fn tailscale_exe() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into());
    base.join("Tailscale").join("tailscale.exe")
}

/// Ask Tailscale for its status. Never takes longer than a few seconds.
pub async fn status() -> Tailscale {
    #[cfg(windows)]
    {
        match tokio::time::timeout(TIMEOUT, local_api()).await {
            Ok(Ok(Some(t))) => return t,
            // No pipe: the service is not running (or not installed).
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return if tailscale_exe().exists() { Tailscale::NotRunning } else { Tailscale::NotInstalled };
            }
            // The pipe exists but the answer was not usable: ask the CLI.
            _ => {}
        }
    }
    cli().await
}

#[cfg(windows)]
async fn local_api() -> std::io::Result<Option<Tailscale>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    let mut client = ClientOptions::new().open(PIPE)?;
    let request = "GET /localapi/v0/status HTTP/1.1\r\nHost: local-tailscaled.sock\r\nSec-Tailscale: localapi\r\nConnection: close\r\n\r\n";
    client.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    client.read_to_end(&mut response).await?;
    Ok(parse_http(&response)
        .filter(|(code, _)| *code == 200)
        .and_then(|(_, body)| parse_status(std::str::from_utf8(&body).ok()?)))
}

async fn cli() -> Tailscale {
    let exe = tailscale_exe();
    if !exe.exists() {
        return Tailscale::NotInstalled;
    }
    let mut command = tokio::process::Command::new(exe);
    command.args(["status", "--json"]).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    match tokio::time::timeout(TIMEOUT, command.output()).await {
        Ok(Ok(out)) => parse_status(&String::from_utf8_lossy(&out.stdout)).unwrap_or(Tailscale::NotRunning),
        _ => Tailscale::NotRunning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = r#"{
        "BackendState": "Running",
        "MagicDNSSuffix": "tail1234.ts.net",
        "Self": {"TailscaleIPs": ["100.64.0.10", "fd7a:115c:a1e0::a"]},
        "Peer": {
            "nodekey:1": {"HostName": "nas-01", "DNSName": "nas-01.tail1234.ts.net.",
                          "TailscaleIPs": ["100.64.0.21"], "Online": true},
            "nodekey:2": {"HostName": "pi", "DNSName": "pi.tail1234.ts.net.",
                          "TailscaleIPs": ["100.64.0.30"], "Online": false}
        }
    }"#;

    #[test]
    fn status_is_parsed_and_peers_found_by_ip_and_name() {
        let Some(Tailscale::Running(s)) = parse_status(RUNNING) else { panic!("running") };
        assert_eq!(s.own_ips.len(), 2);
        assert!(s.is_own("100.64.0.10") && !s.is_own("100.64.0.21"));
        assert_eq!(s.peer("100.64.0.21").unwrap().host_name, "nas-01");
        assert_eq!(s.peer("NAS-01.tail1234.ts.net.").unwrap().host_name, "nas-01");
        assert_eq!(s.peer("nas-01").unwrap().host_name, "nas-01");
        assert!(!s.peer("pi").unwrap().online);
        assert!(s.peer("100.64.0.99").is_none() && s.peer("printer").is_none());
    }

    #[test]
    fn stopped_and_signed_out_states() {
        let stopped = parse_status(r#"{"BackendState":"Stopped"}"#).unwrap();
        assert_eq!(stopped, Tailscale::NotConnected { state: "Stopped".into() });
        assert!(stopped.problem().unwrap().contains("switched off"));
        let login = parse_status(r#"{"BackendState":"NeedsLogin"}"#).unwrap();
        assert!(login.problem().unwrap().contains("signed out"));
        assert!(parse_status("not json").is_none());
        assert!(Tailscale::NotRunning.problem().is_some() && !Tailscale::NotInstalled.is_running());
    }

    #[test]
    fn http_plain_and_chunked() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}";
        assert_eq!(parse_http(plain), Some((200, b"{\"a\":1}".to_vec())));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_http(chunked), Some((200, b"{\"a\":1}".to_vec())));
        let denied = b"HTTP/1.1 401 Unauthorized\r\n\r\nnope";
        assert_eq!(parse_http(denied).unwrap().0, 401);
        assert!(parse_http(b"garbage").is_none());
    }
}
