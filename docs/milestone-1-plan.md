# Milestone 1: prototype plan

Written 25 November 2026. Goal and acceptance checks are in `CLAUDE.md` ("Next task"). This file is the working plan for getting there.

## Facts checked today

| Item | Finding |
| --- | --- |
| Dev machine | Snapdragon X2 Plus, native ARM64 Windows 11. Builds and tests run on the target hardware, no cross-compiling. |
| Toolchain | Installed 25 Nov 2026: Rust 1.100.0 (host `aarch64-pc-windows-msvc`), Visual Studio Build Tools 2026 with ARM64 C++ tools and Windows SDK, LLVM/clang 23.1.6 (ARM64; `C:\Program Files\LLVM\bin` added to the user PATH by hand), CMake 4.4.4 (ARM64). |
| Crypto backends | `aws-lc-rs` 1.19 and `ring` 0.17 both compile and run natively on ARM64, and give identical SHA-256 results. Stay on russh's default, `aws-lc-rs`, unless the russh build itself shows a problem. |
| Smart App Control | On. Builds under `%TEMP%` are blocked: Cargo build scripts there fail with os error 4551. Builds under `C:\Local_Dev` run fine. Keep `target/` inside the repo. A downloaded, unsigned release `.exe` may also be blocked on this PC, which bears on the "unsigned GitHub releases" decision; check it in milestone 7. |
| Font | JetBrainsMono Nerd Font (Mono build) is installed in the user font folder. |
| GPUI | Published on crates.io (`gpui` 0.2.2, 30 Oct 2026). A plain crates.io dependency, not a git one. Features include `windows-manifest`. |
| russh | 0.65.4 (21 Nov 2026), needs Rust 1.89 or newer. Default features: `flate2`, `aws-lc-rs`, `rsa`. `ring` is the opt-in alternative. |
| russh-sftp | 4.0.0 (21 Nov 2026). Not needed until milestone 5. |
| alacritty_terminal | 0.26.0 (April 2026). |
| gpui-component | 0.11.0 (longbridge, 23 Nov 2026): a ready-made GPUI control set. Worth evaluating before milestone 2's shared controls; not a decision. |

Still not verified: the Nerd Font bundling licence.

## Step 0: machine setup

1. Rust via rustup, host `aarch64-pc-windows-msvc`, stable channel.
2. Visual Studio Build Tools with the C++ workload, **ARM64 build tools** and a Windows 11 SDK.
3. LLVM (clang) and CMake. Both C crypto backends need a C toolchain on ARM64 Windows, and clang is the usual requirement. Confirm against each crate's build docs when the first build runs.

Exit check: `cargo new` hello-world builds and `rustc -vV` reports `host: aarch64-pc-windows-msvc`.

## Step 1: workspace skeleton

Create the Cargo workspace with only the two crates this milestone needs. The other four arrive in milestone 2.

- `crates/holonet-ssh`: connect, authenticate (password or key file), open a PTY and shell channel, resize, and stream bytes both ways. No window. Comes with a small CLI example that pipes the shell to stdout, so SSH is proven apart from the interface.
- `crates/holonet-app`: the GPUI binary.

Pin `rust-toolchain.toml` to a stable version of at least 1.89.

## Step 2: four spikes, each a risk gate

Each spike stands alone, so a failure points at one library.

1. **GPUI window on ARM64.** *Passed 25 Nov 2026; see `docs/prototype-notes.md`.* An empty window with text drawn in JetBrainsMono Nerd Font. This proves the DirectX 11 renderer and font loading. If it fails, stop and re-assess against the Tauri fallback before doing anything else.
2. **russh headless.** *Passed 25 Nov 2026; see `docs/prototype-notes.md`.* The `holonet-ssh` CLI example connects, signs in with a password and with a key, and runs a shell. Try `aws-lc-rs` (the default) first, then `ring`; record which one builds.
3. **Terminal grid in GPUI.** *Passed 25 Nov 2026; see `docs/prototype-notes.md`.* `alacritty_terminal::Term` fed from a local byte source (canned ANSI output, no network). Render the grid cell by cell with colors, cursor and Nerd Font glyphs.
4. **Wire together.** *Passed 25 Nov 2026; see `docs/prototype-notes.md`.* SSH channel bytes go into the terminal parser, and keystrokes are encoded and sent to the channel. Run SSH on a Tokio runtime and send updates into GPUI through a channel, so the network never blocks the UI thread.

## Step 3: terminal behaviour

*All five done in spike 4 (25 Nov 2026).*

Complete these in order:

1. Input: printable keys, Enter, Backspace, arrows, Ctrl combinations, all sent to the shell.
2. Resize: window size changes the grid size, which sends `window_change` to the server.
3. Scrollback: mouse wheel and Shift Page Up/Down.
4. Selection with the mouse, and copy with Ctrl Shift C (paste with Ctrl Shift V), per the shortcut convention.
5. Font fallback: JetBrainsMono Nerd Font if installed, otherwise Cascadia Mono, then Consolas.

## Step 4: measure and write `docs/prototype-notes.md`

*Done; see the milestone summary in `docs/prototype-notes.md`.*

- Native `aarch64-pc-windows-msvc` build runs.
- Typing latency: does it feel immediate? Add a frame-time readout if it is unclear.
- `cat` of a 10 MB log: time to finish, and whether the window stays responsive during output.
- Nerd Font icons and ligatures render at the right cell widths (test with a prompt that uses Powerline glyphs, and with `eza --icons` or similar).
- The chosen crypto backend and any build quirks.
- GPUI API notes for later milestones: what was easy, what needed Zed's source.

## Reference material

- Zed's `terminal` and `terminal_view` crates are the closest example of alacritty_terminal inside GPUI. They are GPL-3.0: read them to learn the approach, but do not copy code into Holonet.
- russh's `examples/` folder has a client with a PTY.

## Test server

`sshd` in WSL (Ubuntu-26.04), reachable from Windows at `127.0.0.1:2222` through WSL's localhost forwarding.

- Only `holotest` and `holotest-kbd` may sign in. `holotest` can use a password, a public key or keyboard-interactive; `holotest-kbd` (same password) is limited to keyboard-interactive by a `Match User` block, to test the fallback.
- Config: `/etc/ssh/sshd_config.d/10-holonet-test.conf` and the socket override `/etc/systemd/system/ssh.socket.d/port.conf`.
- Credentials live in `.dev/` (gitignored): `test-server.env` holds the password and key passphrase, plus three keys: `holotest_ed25519` (no passphrase), `holotest_rsa` (no passphrase, 3072-bit) and `holotest_ed25519_pass` (passphrase). `.dev/known_hosts` holds the server's key.
- The server runs only while the WSL distribution is running. If a connection is refused, start it with `wsl -d Ubuntu-26.04 -u root -- systemctl start ssh.socket`.
