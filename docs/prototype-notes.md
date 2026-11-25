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
