# Holonet

Holonet is a personal SSH and SFTP client for Windows on ARM, in the spirit of Termius, written entirely in Rust. Read this file first, then `docs/build-plan.md`.

Milestones 1 (prototype), 2 (connect), 3 (sessions), 4 (keys and routes) and 5 (files) are done. The app creates and unlocks an encrypted vault, keeps hosts and keys in it, connects by password or key, and checks host keys against Known hosts. Each tab holds up to four split panes, each with its own connection, which shows connecting, failed and reconnecting states and reconnects by itself after a drop. Find searches a pane's scrollback, and the Settings page sets the terminal font (JetBrainsMono Nerd Font is bundled), size, cursor and color scheme. The Keychain generates and imports keys (file, `~/.ssh`, WSL, PuTTY `.ppk`), installs them on hosts, and holds identities. Hosts can have a Tailscale address with an Automatic, Primary or Tailscale route, and a chain of jump hosts. The vault locks after the idle time and when Windows locks, and Windows Hello unlocks it through a key slot in the vault file. An SFTP tab, opened from a host, a terminal's header or the host picker, shows this PC and the host side by side: drag files and folders between them (or from Explorer onto the host), with a transfer queue that pauses, resumes, retries and picks up after a dropped connection, one conflict question per transfer, and rename, delete (to the Recycle Bin on this PC), new folder, copy path and permissions. The next task is milestone 6 below.

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
| `holonet-model` | Hosts, groups, keys, known hosts, terminal and connection settings. No I/O. | Built |
| `holonet-vault` | Encrypted store on disk (Argon2id, XChaCha20-Poly1305, atomic saves with a backup), lock and unlock, and the Windows Hello key slot (the app supplies Hello's signature). | Built |
| `holonet-ssh` | Connecting, host key checks, sign-in (password, keyboard-interactive, keys), PTY shells, keep-alive, telling a lost connection from a shell exit, jump hosts (`direct-tcpip`), key generation, `.ppk` conversion, installing a public key, opening the SFTP subsystem, and local, remote and SOCKS5 port forwards (`forward`). | Built |
| `holonet-net` | Tailscale status over its LocalAPI pipe, route checks, and the route plan (which addresses to try, and how long to wait). | Built |
| `holonet-app` | The GPUI window, shared controls (`ui/`), vault screens with Hello unlock (`hello.rs`), Hosts, host form, Keychain, Known hosts, Settings, tabs and the host picker, split panes, connection states and reconnect, routes and jump chains (`hops.rs`), find, terminal rendering, the SFTP tab (`files_view.rs`, its controls in `ui/files.rs`), and sleep, network and Windows lock events (`platform.rs`). | Built |
| `holonet-files` | The two sides of the SFTP screen without a window: a host's files over SFTP (`remote`) and this PC's (`local`, with drives and the Recycle Bin), as one `Entry` type; transfer planning and conflicts (`plan`), resumable copies (`transfer`) and the queue (`engine`). | Built |
| `holonet-import` | OpenSSH `config` and `known_hosts` parsers. | Planned (milestone 7) |

## Building and testing

- Toolchain pinned in `rust-toolchain.toml`. CI (`.github/workflows/ci.yml`) runs fmt, clippy `-D warnings` and the tests on `windows-11-arm` and `windows-latest`.
- `cargo run -p holonet-app --release` starts the app with the vault at `%APPDATA%\Holonet\vault.holonet`. Set `HOLONET_VAULT` to another file when testing; never test against the owner's real vault.
- Hidden modes: `--gallery` (every control, including the Keychain table, identity cards, import list and side panel, and the SFTP screen's file list, breadcrumbs, selection bar, progress bars and permissions grid), `--showcase [options]` (canned terminal output; options such as `light`, `bar`, `size=N` are listed in `main.rs`), `--fonts`, `--parse-bench`, `--find-bench`, `--stream-bench`.
- Test server: sshd in WSL on port 2222, described in `docs/milestone-1-plan.md`. Its credentials live in `.dev/` (gitignored, never committed). `$env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-ssh` (and `-p holonet-files`, best with `--release` for its large transfers) runs the integration tests against it; `cargo run -p holonet-ssh --example shell -- all` drives it without a window. Both share `crates/holonet-ssh/tests/support/`.
- Route tests: `cargo run -p holonet-net --example ts_relay --release` forwards this PC's Tailscale address to the WSL server, so a host's Tailscale address can point at it; `--example net_check` prints Tailscale's state and route checks.
- Results and lessons from every phase are in `docs/prototype-notes.md`, including GPUI and Taffy pitfalls worth reading before UI work.

## Next task: milestone 6, power features

From `docs/build-plan.md`: port forwarding including dynamic, snippets with variables and multi-host runs, broadcast input, groups and their defaults.

Plan it first as `docs/milestone-6-plan.md`, in phases that each end with a commit and a check, as `docs/milestone-5-plan.md` does, and agree it with the owner before building.

## Things that were not verified

Settled by milestones 1 to 4: GPUI is `gpui` 0.2.2 from crates.io and runs natively on ARM64 with DirectX 11; russh 0.65 builds with `aws-lc-rs`; alacritty_terminal 0.26 renders in a GPUI canvas; JetBrainsMono Nerd Font v3.5.1 is bundled under the SIL OFL (the Font Logos glyphs are unlicensed upstream, a risk the owner accepted for a personal build; see `crates/holonet-app/assets/fonts/jetbrainsmono-nerd/NOTICE.md`). Details are in `docs/prototype-notes.md`. Still open, check current documentation before relying on them:

- russh does not list FIDO key support.

Settled in milestone 5: `russh-sftp` 4.0 with its `raw-string` feature, opened with `Session::open_sftp()`; resumes from an offset both ways; GPUI's Windows drop target delivers Explorer's files as `ExternalPaths`, but GPUI 0.2.2 cannot drag out to Explorer. Details are in `docs/prototype-notes.md`.

Settled in milestone 4: jump hosts tunnel one russh session through another (`channel_open_direct_tcpip`, then `connect_stream`). Windows Hello uses `KeyCredentialManager` (`windows` 0.62) without a window handle; its signatures repeat on this PC, also across app starts, and every prompt opened in front, so the focus workaround was not needed. Details are in `docs/prototype-notes.md`.

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
