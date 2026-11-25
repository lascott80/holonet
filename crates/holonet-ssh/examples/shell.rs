//! Spike 2: sign in to the WSL test server without a window and drive a shell.
//!
//!   cargo run -p holonet-ssh --example shell --release -- <mode|hostkey|all>
//!
//! Modes: password, password-ask, ed25519, rsa, passphrase, passphrase-ask.
//!
//! Reads `.dev/test-server.env` (see docs/milestone-1-plan.md). For each
//! sign-in method it opens a PTY shell, checks the size reaches the server,
//! resizes, then times 10 MB of output.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, bail};
#[path = "../tests/support/mod.rs"]
mod support;

use holonet_ssh::{ConnectError, PtySize, Session, Shell, ShellEvent, ShellInput};
use support::{FileKeyStatus, FilePrompts, MODES, load_env, options_from, root};

/// Send a command and collect output until `marker` appears.
async fn run_until(shell: &mut Shell, command: &str, marker: &str) -> Result<(String, usize)> {
    shell.input.send(ShellInput::Data(command.as_bytes().to_vec()))?;
    let mut text = String::new();
    let mut total = 0usize;
    while let Some(event) = shell.events.recv().await {
        match event {
            ShellEvent::Output(bytes) => {
                total += bytes.len();
                // Keep only the tail so a 10 MB run doesn't build a huge string.
                text.push_str(&String::from_utf8_lossy(&bytes));
                if text.len() > 4096 {
                    let cut = text.len() - 2048;
                    let cut = (cut..text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(cut);
                    text.drain(..cut);
                }
                // Done once the marker's whole line has arrived.
                if let Some(pos) = text.rfind(marker)
                    && text[pos..].contains('\n')
                {
                    return Ok((text, total));
                }
            }
            ShellEvent::Exit(code) => bail!("shell exited with {code}"),
            ShellEvent::Closed => bail!("shell closed"),
        }
    }
    bail!("event stream ended")
}

fn last_line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    // Prompts emit escape sequences on the same line; keep from the marker on.
    let line = text.lines().rev().find(|l| l.contains(needle)).unwrap_or("");
    line[line.find(needle).unwrap_or(0)..].trim()
}

async fn check(mode: &str, env: &HashMap<String, String>) -> Result<()> {
    let opts = options_from(mode, env)?;

    let started = Instant::now();
    let session = Session::connect(opts).await?;
    let connect_ms = started.elapsed().as_millis();
    let info = session.info().clone();
    let mut shell = session.open_shell(PtySize::cells(120, 40)).await?;

    let (out, _) = run_until(&mut shell, "echo SIZE$((0+1)) $(stty size) $TERM\n", "SIZE1 ").await?;
    let size1 = last_line_with(&out, "SIZE1").to_string();

    shell.input.send(ShellInput::Resize(PtySize::cells(90, 25)))?;
    let (out, _) = run_until(&mut shell, "echo SIZE$((0+2)) $(stty size)\n", "SIZE2 ").await?;
    let size2 = last_line_with(&out, "SIZE2").to_string();

    let t = Instant::now();
    let (_, bytes) = run_until(
        &mut shell,
        "head -c 10000000 /dev/zero | tr '\\0' 'x' | fold -w 99; echo; echo DONE-$((40+2))\n",
        "DONE-42",
    )
    .await?;
    let secs = t.elapsed().as_secs_f64();

    shell.input.send(ShellInput::Close)?;
    session.close().await?;
    println!(
        "{mode:<14} ok  {connect_ms} ms via {} ({}, {}, host key {}) | {size1} | resize {size2} | {:.1} MB in {secs:.2}s ({:.0} MB/s)",
        info.auth_method,
        info.kex,
        info.cipher,
        info.host_key_type,
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / secs
    );
    Ok(())
}

/// An unknown key and a changed key must both stop the connection.
async fn check_host_keys(env: &HashMap<String, String>) -> Result<()> {
    let dir = root().join("target/hostkey-check");
    std::fs::create_dir_all(&dir)?;
    let empty = dir.join("empty_known_hosts");
    std::fs::write(&empty, "")?;
    // Same algorithm as the server's ed25519 host key, different key: any ed25519
    // test key works as the impostor recorded in known_hosts.
    let impostor = std::fs::read_to_string(root().join(&env["HOLONET_TEST_KEY_ED25519_PASS"]).with_extension("pub"))?;
    let impostor = impostor.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let changed = dir.join("changed_known_hosts");
    std::fs::write(&changed, format!("[{}]:{} {impostor}\n", env["HOLONET_TEST_HOST"], env["HOLONET_TEST_PORT"]))?;

    for (label, file, expect) in
        [("unknown key", empty, FileKeyStatus::Unknown), ("changed key", changed, FileKeyStatus::Changed)]
    {
        let mut opts = options_from("ed25519", env)?;
        let prompts = FilePrompts::new(file);
        opts.prompts = prompts.clone();
        let status = || prompts.last_status.lock().unwrap().clone();
        match Session::connect(opts).await {
            Ok(_) => bail!("{label}: connected, but it must be refused"),
            Err(ConnectError::HostKeyRejected) if status() == Some(expect.clone()) => {
                println!("{label:<14} ok  refused before sign-in ({expect:?})")
            }
            Err(e) => bail!("{label}: refused for the wrong reason: {e:#} ({:?})", status()),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = load_env()?;
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if mode == "hostkey" || mode == "all" {
        if let Err(e) = check_host_keys(&env).await {
            println!("host keys FAILED: {e:#}");
            std::process::exit(1);
        }
        if mode == "hostkey" {
            return Ok(());
        }
    }
    let modes: Vec<&str> = if mode == "all" { MODES.to_vec() } else { vec![mode.as_str()] };
    let mut failed = false;
    for m in modes {
        if let Err(e) = check(m, &env).await {
            println!("{m:<10} FAILED: {e:#}");
            failed = true;
        }
    }
    if failed {
        std::process::exit(1)
    }
    Ok(())
}
