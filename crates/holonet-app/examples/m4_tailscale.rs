//! Milestone 4 phase 0, spike (d): read Tailscale's status from its LocalAPI
//! over the named pipe, as a normal (not elevated) user, without starting
//! `tailscale.exe`. Prints only the state, this PC's own Tailscale addresses
//! and a peer count; peer names stay private.
//!
//!   cargo run -p holonet-app --example m4_tailscale --release

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    // winbase.h: SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION.
    const SQOS: u32 = 0x0010_0000 | (1 << 16);
    let pipe = r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled";
    for attempt in ["with identification", "without impersonation flags"] {
        let t = Instant::now();
        let mut options = ClientOptions::new();
        if attempt.starts_with("with ") {
            options.security_qos_flags(SQOS);
        }
        let mut client = match options.open(pipe) {
            Ok(c) => c,
            Err(e) => {
                println!("{attempt}: pipe not available ({e}): Tailscale is not running");
                continue;
            }
        };
        let request = "GET /localapi/v0/status HTTP/1.1\r\nHost: local-tailscaled.sock\r\nSec-Tailscale: localapi\r\nConnection: close\r\n\r\n";
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let _ = client.read_to_end(&mut response).await;
        let elapsed = t.elapsed();
        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        println!("{attempt}: {}", head.lines().next().unwrap_or(""));
        let (Some(start), Some(end)) = (body.find('{'), body.rfind('}')) else { continue };
        let v: serde_json::Value = match serde_json::from_str(&body[start..=end]) {
            Ok(v) => v,
            Err(e) => {
                println!("  body is not one JSON object ({e}); chunked encoding needs decoding");
                continue;
            }
        };
        let own = v["Self"]["TailscaleIPs"].as_array().map_or(0, |a| a.len());
        println!(
            "  BackendState {}, {own} own addresses, {} peers",
            v["BackendState"],
            v["Peer"].as_object().map_or(0, |p| p.len())
        );
        println!("  round trip {:.1} ms, {} bytes", elapsed.as_secs_f64() * 1e3, response.len());
    }
}

#[cfg(not(windows))]
fn main() {}
