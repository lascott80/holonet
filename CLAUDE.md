# Holonet

Holonet is a personal SSH and SFTP client for Windows on ARM, in the spirit of Termius, written entirely in Rust. Read this file first, then `docs/build-plan.md`.

Milestones 1 (prototype) and 2 (connect) are done. The app creates and unlocks an encrypted vault, keeps hosts and keys in it, connects by password or key, checks host keys against Known hosts, and runs one terminal per tab. The next task is milestone 3 below.

## What is decided

- **Target:** `aarch64-pc-windows-msvc` (native Windows on ARM). x64 Windows is welcome if it costs nothing, but ARM64 is the one that must work.
- **Interface:** GPUI, the GPU-drawn Rust UI framework from Zed. The owner chose it for the speed of the app. Tauri with a web view is the fallback only if GPUI cannot do something the design needs.
- **SSH:** `russh` on Tokio. **SFTP:** `russh-sftp`.
- **Terminal emulation:** `alacritty_terminal`, rendered inside a GPUI view.
- **Distribution:** unsigned builds for the owner's personal use, published as GitHub releases on this repository. No installer signing, no auto-install. The app only shows a notice when a newer release exists.
- **Not in the first release:** hardware (FIDO) security key sign-in, PuTTY and Termius import.

All behaviour decisions are in `docs/decisions.md`. Treat them as settled; ask before changing one.

## Workspace layout

A Cargo workspace. Everything except `holonet-app` builds and tests without a window.

| Crate | Owns | State |
| --- | --- | --- |
| `holonet-model` | Hosts, groups, keys, known hosts, settings. No I/O. | Built |
| `holonet-vault` | Encrypted store on disk (Argon2id, XChaCha20-Poly1305, atomic saves with a backup), lock and unlock. Windows Hello and key generation come later. | Built |
| `holonet-ssh` | Connecting, host key checks, sign-in (password, keyboard-interactive, keys), PTY shells. Jump hosts, forwarding, SFTP and reconnect come later. | Built |
| `holonet-app` | The GPUI window, shared controls (`ui/`), vault screens, Hosts, host form, Known hosts, session tabs, terminal rendering. | Built |
| `holonet-net` | Choosing between a host's primary and Tailscale address, Tailscale detection, reachability. | Planned (milestone 4) |
| `holonet-import` | OpenSSH `config` and `known_hosts` parsers. | Planned (milestone 7) |

## Building and testing

- Toolchain pinned in `rust-toolchain.toml`. CI (`.github/workflows/ci.yml`) runs fmt, clippy `-D warnings` and the tests on `windows-11-arm` and `windows-latest`.
- `cargo run -p holonet-app --release` starts the app with the vault at `%APPDATA%\Holonet\vault.holonet`. Set `HOLONET_VAULT` to another file when testing; never test against the owner's real vault.
- Hidden modes: `--gallery` (every control), `--showcase`, `--fonts`, `--parse-bench`, `--stream-bench`.
- Test server: sshd in WSL on port 2222, described in `docs/milestone-1-plan.md`. Its credentials live in `.dev/` (gitignored, never committed). `$env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh` runs the integration tests against it; `cargo run -p holonet-ssh --example shell -- all` drives it without a window. Both share `crates/holonet-ssh/tests/support/`.
- Results and lessons from every phase are in `docs/prototype-notes.md`, including GPUI and Taffy pitfalls worth reading before UI work.

## Next task: milestone 3, sessions

From `docs/build-plan.md`: tabs, split panes, the connecting, failed and reconnecting states, and the terminal settings including the bundled Nerd Fonts. Also carried over from milestone 2: find in the terminal, automatic reconnect, and a dedicated key-refused screen.

Plan it first as `docs/milestone-3-plan.md`, in phases that each end with a commit and a check, as `docs/milestone-2-plan.md` does, and agree it with the owner before building.

## Things that were not verified

Settled by milestones 1 and 2: GPUI is `gpui` 0.2.2 from crates.io and runs natively on ARM64 with DirectX 11; russh 0.65 builds with `aws-lc-rs`; alacritty_terminal 0.26 renders in a GPUI canvas. Details are in `docs/prototype-notes.md`. Still open, check current documentation before relying on them:

- The current version and API of `russh-sftp`.
- russh does not list FIDO key support and does not provide jump hosts ready-made; jump hosts are built by tunnelling one session through another.
- Windows Hello from a desktop app needs the `UserConsentVerifier` interop interface with a window handle, and its prompt has a reported focus bug.
- The licence terms for bundling JetBrainsMono Nerd Font.

## Design references

- `docs/design/README.md`: the design system's brand book (voice, surfaces, color rules, layout, icons).
- `docs/design/tokens.json` and `tokens.css`: every color, type style, spacing step, radius and shadow. Use these names in code.
- `docs/design/components/`: guidelines and a reference drawing for eleven components.
- `docs/design/screens/`: the 17 designed screens. `README.md` there lists them. The `.dc.html` files are the canvas sources: they do not open in a browser on their own, but their markup and inline styles are the exact layout specification.
- Live versions (owner's account): design canvas https://claude.ai/artifact/ERcWUAQ2qwDopUveQzZ5NG, design system https://claude.ai/artifact/6nQLN41arw44Bdr9NybuvQ.

## Conventions

- App shortcuts use Ctrl Shift (palette is Ctrl Shift P, new tab Ctrl Shift T, close tab Ctrl Shift W) so that Ctrl K, Ctrl T, Ctrl W and other shell editing keys always reach the terminal.
- Session logging is off by default and opt-in per host.
- Amber (`warn`) is only for warnings and destructive actions. Group colors identify a host's group and are never the only signal.
- Sentence case in all interface text. Buttons name their action.
