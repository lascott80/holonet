# Holonet

Holonet is a personal SSH and SFTP client for Windows on ARM, in the spirit of Termius, written entirely in Rust. This file is the handoff from the design phase. Read it first, then `docs/build-plan.md`.

The repository holds no code yet. The design, the decisions and the build plan are finished; the next task is milestone 1 below.

## What is decided

- **Target:** `aarch64-pc-windows-msvc` (native Windows on ARM). x64 Windows is welcome if it costs nothing, but ARM64 is the one that must work.
- **Interface:** GPUI, the GPU-drawn Rust UI framework from Zed. The owner chose it for the speed of the app. Tauri with a web view is the fallback only if GPUI cannot do something the design needs.
- **SSH:** `russh` on Tokio. **SFTP:** `russh-sftp`.
- **Terminal emulation:** `alacritty_terminal`, rendered inside a GPUI view.
- **Distribution:** unsigned builds for the owner's personal use, published as GitHub releases on this repository. No installer signing, no auto-install. The app only shows a notice when a newer release exists.
- **Not in the first release:** hardware (FIDO) security key sign-in, PuTTY and Termius import.

All behaviour decisions are in `docs/decisions.md`. Treat them as settled; ask before changing one.

## Workspace layout (planned)

A Cargo workspace of six crates. Everything except `holonet-app` must build and test without a window.

| Crate | Owns |
| --- | --- |
| `holonet-model` | Hosts, groups, snippets, port forwarding rules, settings. No I/O. |
| `holonet-vault` | Encrypted store on disk, lock and unlock, Windows Hello, key generation and import. |
| `holonet-net` | Choosing between a host's primary and Tailscale address, Tailscale detection, reachability. |
| `holonet-ssh` | Sessions, sign-in, host key checks and known hosts, jump hosts, port forwarding, SFTP and its transfer queue, reconnect. |
| `holonet-import` | OpenSSH `config` and `known_hosts` parsers (Windows profile, a WSL distribution, or a chosen file). |
| `holonet-app` | The GPUI window, shared controls, terminal rendering. |

## Next task: milestone 1, the prototype

Goal: prove GPUI, russh and alacritty_terminal work together on ARM hardware before any designed screen is rebuilt.

Build one bare window that:

1. Takes a host, port, user and password or key path (hard-coded or from arguments is fine).
2. Connects with russh, requests a PTY and a shell.
3. Shows a working terminal: input, output, resize, scrollback, selection and copy.
4. Renders with JetBrainsMono Nerd Font if installed, otherwise a system monospace font.

Then check and write the results into `docs/prototype-notes.md`:

- It builds and runs natively for `aarch64-pc-windows-msvc`.
- Typing feels immediate; printing a large file (for example `cat` of a 10 MB log) stays responsive.
- Nerd Font icon glyphs and ligatures render at the right widths.
- Which crypto backend for russh (`aws-lc-rs` or `ring`) builds cleanly on ARM64 Windows.

Do not build the vault, host list or any designed screen in this milestone.

## Things that were not verified

The design phase did not write or run any Rust. Before relying on any of these, check current documentation:

- How GPUI is consumed outside Zed (crate or git dependency), its current API, and its Windows ARM64 status. The plan rests on Zed shipping on Windows with a DirectX 11 renderer.
- Current versions and APIs of `russh`, `russh-sftp` and `alacritty_terminal`.
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
