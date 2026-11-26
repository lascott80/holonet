//! The new-release check (`decisions.md`: a notice in Settings and a dot on
//! its navigation entry; nothing installs). It asks GitHub's API for this
//! repository's latest release through WinHTTP, Windows' own HTTP client
//! (milestone 7 phase 0, spike b), and reads nothing else.
//!
//! For testing only: `HOLONET_UPDATE_REPO` (`owner/name`) asks another
//! repository, and `HOLONET_UPDATE_HOST` another host (an unreachable one
//! shows the offline message).

use holonet_model::{Timestamp, UpdateSettings};

/// Where releases are published (`decisions.md`).
pub const REPO: &str = "lascott80/holonet";

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn repo() -> String {
    std::env::var("HOLONET_UPDATE_REPO").ok().filter(|r| r.contains('/')).unwrap_or_else(|| REPO.to_string())
}

/// A release's page, for "View release on GitHub".
pub fn release_page(tag: &str) -> String {
    format!("https://github.com/{}/releases/tag/{tag}", repo())
}

/// "github.com/lascott80/holonet" for the Settings note.
pub fn repo_label() -> String {
    format!("github.com/{}", repo())
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckResult {
    /// A release newer than this build: its tag and when it was published.
    Newer {
        tag: String,
        published: Timestamp,
    },
    /// This build is the newest, or nothing is published yet.
    UpToDate,
    Failed(String),
}

/// `v0.2.0`, `0.2.0` or `0.2` as numbers. A pre-release suffix (`-rc1`)
/// is ignored, since `releases/latest` never returns pre-releases.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let t = text.trim().trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

/// Whether a release tag is newer than `current`.
pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// The release to show in Settings' notice: the newest found, if newer
/// than this build and not hidden with Later.
pub fn notice(u: &UpdateSettings) -> Option<(String, Timestamp)> {
    let (tag, published) = u.latest.clone()?;
    (is_newer(&tag, CURRENT) && u.dismissed.as_deref() != Some(tag.as_str())).then_some((tag, published))
}

/// Read GitHub's answer for `releases/latest`.
fn read_release(status: u32, body: &str) -> CheckResult {
    match status {
        200 => {}
        // No release published (or the repository is not public).
        404 => return CheckResult::UpToDate,
        403 | 429 => return CheckResult::Failed("GitHub is limiting requests right now. Try again later.".into()),
        other => return CheckResult::Failed(format!("GitHub answered with an error ({other}).")),
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return CheckResult::Failed("GitHub's answer could not be read.".into());
    };
    let Some(tag) = json.get("tag_name").and_then(|t| t.as_str()) else {
        return CheckResult::Failed("GitHub's answer had no release tag.".into());
    };
    let published = json
        .get("published_at")
        .and_then(|p| p.as_str())
        .and_then(|p| p.parse::<jiff::Timestamp>().ok())
        .map(|t| Timestamp(t.as_second()))
        .unwrap_or_else(Timestamp::now);
    if is_newer(tag, CURRENT) { CheckResult::Newer { tag: tag.to_string(), published } } else { CheckResult::UpToDate }
}

/// Ask GitHub. Blocks for up to about 10 seconds per step: run it off the
/// UI thread.
pub fn check() -> CheckResult {
    let host = std::env::var("HOLONET_UPDATE_HOST").unwrap_or_else(|_| "api.github.com".into());
    match get(&host, &format!("/repos/{}/releases/latest", repo())) {
        Ok((status, body)) => read_release(status, &body),
        Err(_) => CheckResult::Failed("Holonet could not reach GitHub.".into()),
    }
}

/// "Today at 13:40", or "24 Nov 2026 at 13:40".
pub fn when(t: Timestamp) -> String {
    let tz = jiff::tz::TimeZone::system();
    let Ok(at) = jiff::Timestamp::from_second(t.0).map(|t| t.to_zoned_datetime(tz.clone())) else {
        return String::new();
    };
    let today = jiff::Timestamp::now().to_zoned_datetime(tz).date();
    if at.date() == today {
        at.strftime("Today at %H:%M").to_string()
    } else {
        at.strftime("%-d %b %Y at %H:%M").to_string()
    }
}

#[cfg(windows)]
fn get(host: &str, path: &str) -> Result<(u32, String), String> {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Networking::WinHttp::*;

    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a handle WinHTTP returned, closed once.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // SAFETY: reads the calling thread's last error.
    let err = |what: &str| format!("{what} failed: error {}", unsafe { GetLastError() });

    // SAFETY: WinHTTP calls with NUL-terminated wide strings that outlive
    // each call, and handles closed by `Handle`.
    unsafe {
        // A generic agent: nothing about this PC or its owner is sent.
        let agent = wide("Holonet");
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(err("WinHttpOpen"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 10_000);
        let host = wide(host);
        let conn = Handle(WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(err("WinHttpConnect"));
        }
        let (verb, path) = (wide("GET"), wide(path));
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(err("WinHttpOpenRequest"));
        }
        let headers = wide("Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28");
        WinHttpAddRequestHeaders(req.0, headers.as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD);
        if WinHttpSendRequest(req.0, null(), 0, null(), 0, 0, 0) == 0 {
            return Err(err("WinHttpSendRequest"));
        }
        if WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
            return Err(err("WinHttpReceiveResponse"));
        }
        let mut status: u32 = 0;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut len,
            std::ptr::null_mut(),
        );
        let mut body = Vec::new();
        loop {
            let mut buf = [0u8; 16 * 1024];
            let mut read = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read) == 0 {
                return Err(err("WinHttpReadData"));
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > 2 << 20 {
                return Err("response larger than 2 MB".into());
            }
        }
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }
}

#[cfg(not(windows))]
fn get(_: &str, _: &str) -> Result<(u32, String), String> {
    Err("not on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.30.2"), Some((1, 30, 2)));
        assert_eq!(parse_version("v0.2"), Some((0, 2, 0)));
        assert_eq!(parse_version("v0.1.0-rc1"), Some((0, 1, 0)));
        assert_eq!(parse_version("nightly"), None);
        assert!(is_newer("v0.2.0", "0.1.0") && is_newer("v0.1.1", "0.1.0") && is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0") && !is_newer("v0.0.9", "0.1.0") && !is_newer("latest", "0.1.0"));
    }

    #[test]
    fn answers_read() {
        let body = r#"{"tag_name":"v99.0.0","published_at":"2026-11-24T17:43:29Z","draft":false}"#;
        assert_eq!(
            read_release(200, body),
            CheckResult::Newer { tag: "v99.0.0".into(), published: Timestamp(1795542209) }
        );
        assert_eq!(read_release(200, r#"{"tag_name":"v0.0.1"}"#), CheckResult::UpToDate);
        assert_eq!(read_release(404, "{}"), CheckResult::UpToDate);
        assert!(matches!(read_release(403, ""), CheckResult::Failed(m) if m.contains("limiting")));
        assert!(matches!(read_release(200, "not json"), CheckResult::Failed(_)));
    }

    #[test]
    fn the_notice_follows_later() {
        let mut u = UpdateSettings { latest: Some(("v99.0.0".into(), Timestamp(0))), ..Default::default() };
        assert_eq!(notice(&u).map(|n| n.0), Some("v99.0.0".into()));
        u.dismissed = Some("v99.0.0".into());
        assert_eq!(notice(&u), None);
        // A newer release shows again.
        u.latest = Some(("v99.1.0".into(), Timestamp(0)));
        assert!(notice(&u).is_some());
        // Not newer than this build: no notice.
        u.latest = Some((format!("v{CURRENT}"), Timestamp(0)));
        assert_eq!(notice(&u), None);
    }
}
