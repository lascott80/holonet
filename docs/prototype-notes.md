# Milestone 1: prototype notes

Results of the checks listed in `CLAUDE.md`. The working plan is `docs/milestone-1-plan.md`.

Machine: Snapdragon X2 Plus, Windows 11 ARM64, display scale 150%. Rust 1.100.0, `aarch64-pc-windows-msvc`.

## Spike 1: GPUI window on ARM64 (25 Nov 2026), passed

`crates/holonet-app` with `gpui` 0.2.2 from crates.io, default features.

| Check | Result |
| --- | --- |
| Builds natively for `aarch64-pc-windows-msvc` | Yes. First clean debug build about 2 minutes; incremental rebuild about 8 seconds. |
| Window opens and renders (DirectX 11) | Yes, with a native title bar and the design's dark surfaces. |
| Font discovery | `window.text_system().all_font_names()` finds the user-installed "JetBrainsMono Nerd Font Mono". |
| Cell width | 7.80px at 13px (logical pixels). |
| Nerd Font glyph widths | All probes measure exactly one cell: Latin, box drawing, Powerline arrows and rounded caps, folder, git branch, Linux and Rust icons. Expected for the "Mono" build. |
| Ligatures | **Not rendering.** `->`, `!=`, `===` and `<!--` draw as separate characters, although GPUI's DirectWrite backend enables `calt` by default (`platform/windows/direct_write.rs`). Open question for spike 3: the font build, a GPUI shaping detail, or both. The terminal grid draws cell by cell anyway, so ligatures there need runs to be shaped together. |

### GPUI API notes

- `Pixels` has a private inner field: use `f32::from(pixels)`.
- Font metrics: `text_system.resolve_font(&font(family))` gives a `FontId`; `advance(font_id, size, ch)` gives per-glyph widths. `resolve_font` panics only when no fallback resolves.
- No window title is set by default; set `WindowOptions.titlebar` later.
- Build warning from a dependency: `proc-macro-error2` 2.0.1 will be rejected by a future Rust version. It's harmless for now.

## Spike 2: russh without a window (25 Nov 2026), passed

`crates/holonet-ssh` with `russh` 0.65.4 (default features, so `aws-lc-rs` is the only crypto backend; `ring` is not in the tree) and Tokio. Run with:

```
cargo run -p holonet-ssh --example shell --release -- all
```

It reads `.dev/test-server.env` and checks the WSL test server at `127.0.0.1:2222`.

| Check | Result |
| --- | --- |
| Builds for `aarch64-pc-windows-msvc` | Yes, cleanly, with `aws-lc-rs` 1.19. Release build of the example in about 48 seconds. |
| Password sign-in | Passed, 87 to 135 ms to connect and sign in. |
| ed25519 key, RSA 3072 key (`rsa-sha2` chosen by `best_supported_rsa_hash`) | Passed, 69 to 83 ms. |
| ed25519 key with passphrase | Passed, about 160 ms (the extra time is decrypting the key). |
| PTY size and `TERM` | Server sees `40 120 xterm-256color` for a 120×40 request. |
| Resize (`window_change`) | Server sees `25 90` after resizing to 90×25. |
| Unknown host key | Refused with the fingerprint shown. |
| Changed host key | Refused: "HOST KEY CHANGED", with the known_hosts line number. |
| 10 MB of shell output | Arrives in 0.07 to 0.10 s (about 100 to 135 MB/s) over the bounded 256-chunk event channel. SSH alone is not a bottleneck; the terminal parser and renderer will be. |

### Notes for later spikes

- The API the app will use: `Session::connect(ConnectOptions)`, then `session.open_shell(PtySize)` returns a `Shell { input: UnboundedSender<ShellInput>, events: Receiver<ShellEvent> }`. The shell runs on its own Tokio task, so GPUI only needs to drain `events` and push `input`.
- Ubuntu 26.04's prompt emits bracketed-paste mode (`ESC[?2004h/l`) and an `OSC 3008` shell-integration context sequence (systemd) on every prompt. alacritty_terminal must handle or quietly ignore `OSC 3008`; check in spike 3.
- `check_known_hosts_path` reads OpenSSH `known_hosts` files directly, including `[host]:port` entries written by Windows `ssh.exe`.
- Certificates (`PublicKeyOrCertificate::Certificate`) are refused for now; revisit in milestone 4.

## Spike 3: terminal grid in GPUI (25 Nov 2026), passed

`alacritty_terminal` 0.26.0 drawn by a GPUI `canvas` in `crates/holonet-app`, fed canned output (no network). Run `holonet` for the showcase, `holonet --parse-bench` for parsing alone, and `holonet --stream-bench` for parsing plus rendering.

### How it draws

- `terminal.rs` wraps `Term` and the `vte` `Processor`. `snapshot()` turns the visible grid into runs of same-styled cells with resolved colors; it has no GPUI types, so it unit-tests without a window (4 tests).
- `terminal_view.rs` paints background quads for colored runs, then shapes each run with `shape_line(..., force_width = cell width)` and paints it at its own column. Runs holding a wide char or combining marks skip `force_width` and rely on their column alone. Then it paints the cursor (block, beam, underline or hollow).
- Line height is 13px × 1.6 from the design. The grid size follows the window; scrollback is 10,000 lines; the mouse wheel scrolls history.

### Results (release build, 125×28 grid, 150% display scale)

| Check | Result |
| --- | --- |
| 16, 256 and truecolor; bold (bright), italic, bold italic, underline, strikethrough, dim, inverse | All render correctly. |
| Nerd Font icons in `ls`-style output and the prompt | Render at one cell. |
| Wide characters (Japanese, Korean) and emoji | Stay on the grid, two cells each; emoji draw in color. |
| Ubuntu `OSC 3008` and bracketed paste | Ignored cleanly (unit test). |
| Parse only: 10 MB of colored log lines | 71 ms (141 MB/s). |
| Parse and render, 8 ms parse budget per frame | 10 MB fully on screen in 0.12 to 0.13 s at 63 to 68 fps. |
| Per-frame cost | Snapshot about 57 µs, paint about 2.1 ms. Worst frame 22 to 24 ms is the first one, which rasterizes glyphs. |
| Debug build first paint | About 99 ms. Debug builds are not representative. |

Together with spike 2's 100+ MB/s over SSH, a `cat` of a 10 MB log should finish in well under a second with the window responsive throughout. Spike 4 confirms this end to end.

### Open issues

- **Box drawing has gaps between rows.** At 1.6 line height the font's box glyphs do not reach the cell edges, so vertical lines break. Terminals usually draw U+2500 to U+259F (box drawing and block elements), plus Powerline separators, as geometry instead of glyphs. Do the same before the terminal ships (milestone 3).
- **Ligatures**: still not drawn. `force_width` would also pull apart any ligature glyphs. Deciding whether to support ligatures, and how (shape runs freely and only snap glyph clusters), belongs to milestone 3's terminal settings.
- **The 16-color ANSI palette is provisional.** The design defines foreground, background and cursor but no ANSI colors; the 16 used here are built from token hues. They need a design decision.
- **Low-contrast default text on light backgrounds** (palette cells 10 to 14 in the showcase). That's the sample using default foreground on bright backgrounds, not a renderer bug.

## Spike 4: GPUI terminal on a live SSH shell (25 Nov 2026), passed

`holonet --ssh [password|ed25519|rsa|passphrase]` connects to the WSL test server with `holonet_ssh::dev::options` and runs the login shell in the window. Driven with real keyboard and mouse input (SendKeys and `mouse_event`), release build, 150% scale.

### How it is wired

- A two-thread Tokio runtime (`holonet-ssh` threads) owns the SSH session. GPUI keeps the main thread.
- Connecting and opening the shell run on Tokio, and the result returns over a `oneshot` channel. A GPUI task then drains the shell's `mpsc` receiver: tokio's channels work from GPUI's executor without a Tokio reactor.
- Output is batched: each update takes everything already waiting, up to 1 MB, parses it, and calls `notify` once, so GPUI draws at most once per frame however fast output arrives.
- Keystrokes are encoded by `input.rs` (xterm sequences: modifiers on arrows and function keys, DECCKM application cursor, Alt as an ESC prefix, Ctrl letters, AltGr treated as a typed character) and sent on the unbounded input channel.
- Resizes are sent as `window_change` when the grid size changes during render.

### Results

| Check from `CLAUDE.md` | Result |
| --- | --- |
| Input and output | Typing, Enter, Backspace, arrows, Ctrl+C (interrupted `sleep 30`), Escape all reach the shell. Colors and Nerd Font glyphs from `printf` render. |
| Full-screen programs | `top` (inverse header) and `vim` (alternate screen, syntax colors, arrow keys, `/` search, `:q!` restoring the shell screen) work. The character under a block cursor stays readable. |
| Resize | Window resized to 97×20; `stty size` in the shell reports `20 97`. |
| Scrollback | The wheel scrolls 3 lines per notch; Shift Page Up/Down scroll a page; typing snaps back to the bottom. 10,000 lines kept. |
| Selection and copy | Dragging selects exact cells (cols 0 to 5 gave `holote`), triple-click selects a line, and Ctrl Shift C copies. A plain click clears. |
| Paste | Ctrl Shift V pastes; two lines arrived as one bracketed paste (the shell had enabled it) and ran together on Enter. |
| Session end | `exit` shows "shell exited with 0" in amber in the status bar. |
| **Typing latency** | Keystroke sent to first echoed byte back: **0.7 to 1.8 ms** on the WSL server. On-screen delay adds at most one frame. Feels immediate. |
| **`cat` of a 10.4 MB log** | **0.42 s** by the shell's `time`, with the window drawing throughout. Paint about 1 ms per frame during the flood. |

## Milestone 1 summary

All checks in `CLAUDE.md` are done:

| Check | Result |
| --- | --- |
| Builds and runs natively for `aarch64-pc-windows-msvc` | Yes. |
| Typing feels immediate; `cat` of a 10 MB log stays responsive | Yes: echo under 2 ms; 10.4 MB in 0.42 s with the window responsive. |
| Nerd Font glyphs and ligatures render at the right widths | Glyphs yes (every probe exactly one cell). **Ligatures no**; see open issues. |
| Crypto backend for russh on ARM64 Windows | `aws-lc-rs` (russh's default) builds and works; `ring` also builds. Staying with `aws-lc-rs`. |

GPUI, russh and alacritty_terminal work together on ARM64 Windows, so the Tauri fallback is not needed.

### Carried forward

For milestone 3 (sessions and terminal settings) unless noted:

1. **Box drawing and block elements as geometry.** At the design's 1.6 line height, font glyphs leave gaps between rows.
2. **Ligatures.** Decide whether to offer them; it would need run shaping that snaps glyph clusters instead of single glyphs.
3. **ANSI palette and selection color.** Both are provisional here; they need design decisions.
4. **Mouse reporting to programs** (`vim` mouse mode, `tmux`, `htop` clicks). Not wired; the mouse always selects. Shift should bypass reporting when it is added.
5. **Not yet tested**: IME composition (Japanese and Chinese input), dead keys, focus in/out reporting, the kitty keyboard protocol, cursor blink, and window titles from `OSC 0/2`.
6. **Host key prompt** (milestone 2): unknown keys are refused today, not offered as "Trust and connect".
7. **Dev-only paths**: `holonet_ssh::dev` and `.dev/` stand in for the vault; remove them once milestone 2 adds hosts and the vault.
