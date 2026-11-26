//! Milestone 7 spike (b): ask GitHub for a repository's latest release over
//! WinHTTP (Windows' own HTTP client: system proxy, system certificates).
//!
//!   cargo run -p holonet-app --example m7_update -- owner/repo [host]

#[cfg(windows)]
fn main() {
    let repo = std::env::args().nth(1).unwrap_or_else(|| "lascott80/holonet".into());
    let host = std::env::args().nth(2).unwrap_or_else(|| "api.github.com".into());
    let t = std::time::Instant::now();
    match get(&host, &format!("/repos/{repo}/releases/latest")) {
        Ok((status, body)) => {
            let tag = field(&body, "tag_name");
            let published = field(&body, "published_at");
            println!(
                "{repo}: HTTP {status} in {} ms, {} bytes, tag {tag:?}, published {published:?}",
                t.elapsed().as_millis(),
                body.len()
            );
        }
        Err(e) => println!("{repo}: failed after {} ms: {e}", t.elapsed().as_millis()),
    }
}

#[cfg(not(windows))]
fn main() {}

/// The string value of `"name": "..."` in a JSON body (spike only).
fn field(body: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":");
    let rest = body[body.find(&key)? + key.len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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
    let err = |what: &str| format!("{what} failed: error {}", unsafe { GetLastError() });

    // SAFETY: WinHTTP calls with valid, NUL-terminated wide strings that
    // outlive each call, and handles closed by `Handle`.
    unsafe {
        let agent = wide("Holonet");
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(err("WinHttpOpen"));
        }
        // Resolve, connect, send, receive: 10 seconds each.
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 10_000);
        let host = wide(host);
        let conn = Handle(WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(err("WinHttpConnect"));
        }
        let verb = wide("GET");
        let path = wide(path);
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
            if body.len() > 1 << 20 {
                return Err("response larger than 1 MB".into());
            }
        }
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }
}
