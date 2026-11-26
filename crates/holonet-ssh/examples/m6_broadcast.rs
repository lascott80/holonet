//! Milestone 6 phase 0, spike (c): broadcast input to four shells, as four
//! panes of a tab would have, each on its own connection to the WSL test
//! server.
//!
//!   cargo run -p holonet-ssh --example m6_broadcast --release
//!
//! Each shell runs `cat` in raw mode (`stty raw -echo; cat`), so every byte
//! typed comes straight back. 400 keystrokes are sent to all four, one every
//! 15 ms like fast typing, each a single input message per shell, sent in the
//! same loop. For each shell the spike checks that what came back is exactly
//! what was sent, in order, and records the echo time per keystroke. Then a
//! 64 KB paste goes to all four at once, also checked byte for byte.

use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use holonet_ssh::{PtySize, Session, Shell, ShellEvent, ShellInput};
use tokio::sync::mpsc;

#[path = "../tests/support/mod.rs"]
mod support;

const PANES: usize = 4;
const KEYS: usize = 400;
const KEY_GAP: Duration = Duration::from_millis(15);
const PASTE: usize = 64 * 1024;
const READY: &str = "HOLONET_READY";

/// Wait until `marker` arrives on a shell, dropping everything before it.
async fn wait_for(shell: &mut Shell, marker: &str) -> Result<()> {
    let mut seen = String::new();
    while let Some(event) = shell.events.recv().await {
        if let ShellEvent::Output(bytes) = event {
            seen.push_str(&String::from_utf8_lossy(&bytes));
            if seen.contains(marker) {
                return Ok(());
            }
        }
    }
    anyhow::bail!("shell ended before {marker}")
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = support::load_env()?;
    let mut sessions = Vec::new();
    let mut shells = Vec::new();
    for _ in 0..PANES {
        let session = Session::connect(support::options_from("password", &env)?).await?;
        let mut shell = session.open_shell(PtySize::cells(120, 40)).await?;
        // Raw, no echo from the terminal: `cat` sends back exactly what it
        // reads. The quotes keep the echoed command line from matching.
        shell.input.send(ShellInput::Data(b"stty raw -echo; echo HOLONET_\"\"READY_; exec cat\n".to_vec()))?;
        wait_for(&mut shell, &format!("{READY}_")).await?;
        sessions.push(session);
        shells.push(shell);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Anything printed after the marker (the newline) is dropped.
    for shell in &mut shells {
        while shell.events.try_recv().is_ok() {}
    }
    println!("{PANES} shells ready, each on its own connection");

    // Each shell's output goes to a collector that timestamps every byte.
    let (senders, readers): (Vec<_>, Vec<_>) = shells.into_iter().map(|s| (s.input, s.events)).unzip();
    let mut collectors = Vec::new();
    for mut events in readers {
        let (tx, rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
        collectors.push(rx);
        tokio::spawn(async move {
            while let Some(ShellEvent::Output(bytes)) = events.recv().await {
                if tx.send((Instant::now(), bytes)).is_err() {
                    break;
                }
            }
        });
    }

    // Typing: printable bytes that cycle, so order mistakes show.
    let keys: Vec<u8> = (0..KEYS).map(|i| b'a' + (i % 26) as u8).collect();
    let mut sent_at = Vec::with_capacity(KEYS);
    let start = Instant::now();
    for &key in &keys {
        sent_at.push(Instant::now());
        for input in &senders {
            input.send(ShellInput::Data(vec![key]))?;
        }
        tokio::time::sleep(KEY_GAP).await;
    }
    let typing = start.elapsed();
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("typed {KEYS} keys to all {PANES} in {:.1} s", typing.as_secs_f64());
    for (pane, rx) in collectors.iter_mut().enumerate() {
        let mut got = Vec::new();
        let mut lat = Vec::new();
        while let Ok((at, bytes)) = rx.try_recv() {
            for b in bytes {
                if let Some(sent) = sent_at.get(got.len()) {
                    lat.push(at.duration_since(*sent).as_secs_f64() * 1e3);
                }
                got.push(b);
            }
        }
        ensure!(
            got == keys,
            "pane {pane}: {} bytes back, first difference at {:?}",
            got.len(),
            got.iter().zip(&keys).position(|(a, b)| a != b)
        );
        lat.sort_by(f64::total_cmp);
        let pick = |q: f64| lat[((lat.len() - 1) as f64 * q) as usize];
        println!(
            "  pane {pane}: all {KEYS} back in order; echo median {:.1} ms, 95th {:.1} ms, worst {:.1} ms",
            pick(0.5),
            pick(0.95),
            pick(1.0)
        );
    }

    // A paste: one large message per pane.
    let paste: Vec<u8> = (0..PASTE).map(|i| b'0' + (i % 10) as u8).collect();
    let start = Instant::now();
    for input in &senders {
        input.send(ShellInput::Data(paste.clone()))?;
    }
    for (pane, rx) in collectors.iter_mut().enumerate() {
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while got.len() < PASTE && Instant::now() < deadline {
            if let Ok(Some((_, bytes))) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                got.extend(bytes);
            }
        }
        ensure!(got == paste, "pane {pane}: paste came back as {} bytes, not matching", got.len());
        println!("  pane {pane}: 64 KB paste back intact after {:.0} ms", start.elapsed().as_secs_f64() * 1e3);
    }

    for input in &senders {
        let _ = input.send(ShellInput::Close);
    }
    for session in sessions {
        let _ = session.close().await;
    }
    Ok(())
}
