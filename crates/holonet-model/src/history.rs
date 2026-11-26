//! Session history (History.dc.html): a record for every terminal and SFTP
//! session, and whether its output was saved.
//!
//! The output itself is not here: `holonet-vault` keeps each log as an
//! encrypted file beside the vault, named by the record's id. Records are
//! small, so they live in the vault with everything else.

use serde::{Deserialize, Serialize};

use crate::{HostId, RecordId, Timestamp};

/// What kind of tab the session was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Terminal,
    /// Never logged (`decisions.md`).
    Sftp,
}

impl SessionKind {
    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Terminal => "Terminal",
            SessionKind::Sftp => "SFTP",
        }
    }
}

/// How a session ended, as History's "Ended" reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    ClosedByYou,
    /// The connection was lost and not brought back.
    Dropped,
    SignInFailed,
}

impl SessionEnd {
    pub fn label(self) -> &'static str {
        match self {
            SessionEnd::ClosedByYou => "Closed by you",
            SessionEnd::Dropped => "Connection dropped",
            SessionEnd::SignInFailed => "Sign-in failed",
        }
    }
}

/// One session. Written when the session starts and again when it ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: RecordId,
    /// The saved host, if it still exists. `None` for a quick connect.
    #[serde(default)]
    pub host: Option<HostId>,
    /// The host's name when the session started, so the record still reads
    /// after the host is renamed or deleted.
    pub host_name: String,
    #[serde(default)]
    pub kind: SessionKind,
    pub started: Timestamp,
    /// `None` while the session is open (or if Holonet closed without
    /// ending it).
    #[serde(default)]
    pub ended: Option<Timestamp>,
    /// As History's Route column reads: "Primary", "Tailscale", or the jump
    /// host's name.
    #[serde(default)]
    pub route: String,
    /// "Signed in as".
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub end: Option<SessionEnd>,
    /// Output was saved to a log named by `id`.
    #[serde(default)]
    pub logged: bool,
    /// Size of the saved output, in bytes of plain text.
    #[serde(default)]
    pub log_bytes: u64,
}

impl SessionRecord {
    pub fn new(host: Option<HostId>, host_name: impl Into<String>, kind: SessionKind, logged: bool) -> Self {
        Self {
            id: RecordId::new(),
            host,
            host_name: host_name.into(),
            kind,
            started: Timestamp::now(),
            ended: None,
            route: String::new(),
            user: String::new(),
            end: None,
            // SFTP sessions are never logged (`decisions.md`).
            logged: logged && kind == SessionKind::Terminal,
            log_bytes: 0,
        }
    }

    /// History's Length column: "38 min", "1 h 12 min", "Under a minute".
    /// An open session counts up to `now`.
    pub fn length(&self, now: Timestamp) -> String {
        let secs = (self.ended.unwrap_or(now).0 - self.started.0).max(0);
        let mins = secs / 60;
        match mins {
            0 => "Under a minute".into(),
            1..=59 => format!("{mins} min"),
            _ if mins % 60 == 0 => format!("{} h", mins / 60),
            _ => format!("{} h {} min", mins / 60, mins % 60),
        }
    }

    /// History's Log column: "84 KB", "No log".
    pub fn log_size(&self) -> String {
        if !self.logged {
            return "No log".into();
        }
        match self.log_bytes {
            0..1_000 => format!("{} bytes", self.log_bytes),
            1_000..1_000_000 => format!("{} KB", self.log_bytes.div_ceil(1_000)),
            _ => format!("{:.1} MB", self.log_bytes as f64 / 1e6),
        }
    }
}

/// How long History keeps records and logs (Settings, Connection; not
/// drawn). Counted from when a session ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKeep {
    Days7,
    #[default]
    Days30,
    Days90,
    Forever,
}

impl LogKeep {
    pub fn label(self) -> &'static str {
        match self {
            LogKeep::Days7 => "7 days",
            LogKeep::Days30 => "30 days",
            LogKeep::Days90 => "90 days",
            LogKeep::Forever => "Forever",
        }
    }

    /// Seconds a record is kept after it ends; `None` keeps it.
    pub fn max_age_secs(self) -> Option<i64> {
        let days = match self {
            LogKeep::Days7 => 7,
            LogKeep::Days30 => 30,
            LogKeep::Days90 => 90,
            LogKeep::Forever => return None,
        };
        Some(days * 24 * 60 * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(started: i64, ended: Option<i64>) -> SessionRecord {
        let mut r = SessionRecord::new(None, "h", SessionKind::Terminal, true);
        r.started = Timestamp(started);
        r.ended = ended.map(Timestamp);
        r
    }

    #[test]
    fn length_and_size_read_as_drawn() {
        assert_eq!(at(0, Some(38 * 60 + 20)).length(Timestamp(0)), "38 min");
        assert_eq!(at(0, Some(72 * 60)).length(Timestamp(0)), "1 h 12 min");
        assert_eq!(at(0, Some(120 * 60)).length(Timestamp(0)), "2 h");
        assert_eq!(at(0, Some(30)).length(Timestamp(0)), "Under a minute");
        // Open: counts up to now.
        assert_eq!(at(0, None).length(Timestamp(9 * 60)), "9 min");
        let mut r = at(0, None);
        r.log_bytes = 84_000;
        assert_eq!(r.log_size(), "84 KB");
        r.log_bytes = 412_300;
        assert_eq!(r.log_size(), "413 KB");
        r.log_bytes = 2_450_000;
        assert_eq!(r.log_size(), "2.5 MB");
        r.logged = false;
        assert_eq!(r.log_size(), "No log");
    }

    #[test]
    fn sftp_is_never_logged() {
        assert!(!SessionRecord::new(None, "nas-01", SessionKind::Sftp, true).logged);
        assert!(SessionRecord::new(None, "nas-01", SessionKind::Terminal, true).logged);
    }
}
