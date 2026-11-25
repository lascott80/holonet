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
7. **Dev-only paths**: `holonet_ssh::dev` and `.dev/` stand in for the vault; remove them once milestone 2 adds hosts and the vault. *(Done in milestone 2 phase 8: now test support only.)*

## Milestone 2, phases 0 and 1 (25 Nov 2026)

**CI** (`.github/workflows/ci.yml`): `cargo fmt --check`, `clippy -D warnings` and tests on `windows-latest` (x64), plus the same on `windows-11-arm` marked experimental until we know whether this private repository can use that runner. `rustfmt.toml` sets a 120-column width.

**`holonet-model`**: hosts, stored keys, known hosts and settings as serde data; `Secret` wipes itself and is redacted in `Debug`; address and port validation; delete-host keeps the server key unless asked and no other host shares it. 6 tests.

**`holonet-vault`**: the file format in the crate docs (magic, version, JSON header with key slots, XChaCha20-Poly1305 body with the header as associated data). 12 tests: round trip, nothing readable on disk, wrong password, short password, no overwrite, tampering with body or header, newer format refused, backup fallback (missing, unreadable, or damaged main file), password change, slot parameters.

| Measurement (Snapdragon X2 Plus, release) | Result |
| --- | --- |
| Argon2id unlock, 19 MiB t=2 (OWASP minimum) | 12 ms |
| 64 MiB t=3 (planned default) | 78 ms |
| 128 MiB t=3 | 154 ms |
| **256 MiB t=3 (chosen default)** | **326 ms** |
| 512 MiB t=2 | 520 ms |
| `p=4` vs `p=1` at 64 MiB | No faster (the `argon2` crate runs lanes serially without its `parallel` feature) |
| Crash check: 60 kills at random moments during back-to-back saves | Vault opened with the right password every time: 57 from the main file, 3 from the backup (killed between the two renames), 0 damaged |

The planned 64 MiB default unlocked far faster than the 0.5 s budget, so the default is 256 MiB, 3 passes. Parameters are stored per slot, so they can be raised later without migration.

## Milestone 2, phase 2: SSH extensions (25 Nov 2026)

`holonet-ssh` now has `hostkey` and `connect` modules, and `Session::connect` returns a typed `ConnectError`.

- **`Prompts` trait**: the interface's side of a connection. `host_key(PresentedKey)` returns Continue or Stop, and is awaited inside russh's `check_server_key`, before any credential is sent. `secret(SecretRequest)` asks for a password, passphrase or keyboard-interactive challenge that is not saved (`None` cancels).
- **`classify(known, presented)`**: Trusted, Unknown or Changed against a `KnownHost`. Comments are ignored. The app applies `decisions.md`: Changed always stops; Unknown asks, or stops under "Refuse to connect".
- **`Credential`**: `Password(Option<Secret>)` or `Key { name, private_key, passphrase }`. Keys are decoded from the vault's text with the saved passphrase, or one asked on connect.
- **Password sign-in falls back to keyboard-interactive** when the server lists it. Hidden "password" prompts get the password once; any other question is asked of the person.
- **Own TCP connect** (`lookup_host`, then `TcpStream` with the timeout, then `connect_stream`) so failures are typed: `Resolve`, `Refused`, `TimedOut`, `Network`, `HostKeyRejected`, `SignInRefused { method, remaining }`, `Cancelled`, `WrongPassphrase`, `BadKey`, `Protocol`.
- **`SessionInfo`** for the status bar: key exchange and cipher (from the `kex_done` hook), host key type and key, and the sign-in method used.
- **`dev::FilePrompts`**: trusts an OpenSSH `known_hosts` file and answers secrets from a script, for the example, the tests and `holonet --ssh`.

The test server gained a second user, `holotest-kbd`, limited by a `Match User` block to keyboard-interactive only (same password, `HOLONET_TEST_KBD_USER` in `.dev/test-server.env`).

| Check against the WSL server | Result |
| --- | --- |
| Six sign-in modes: password saved or asked, ed25519, RSA, passphrase saved or asked | All connect (72 to 157 ms). The negotiated key exchange is **`mlkem768x25519-sha256`** (post-quantum hybrid) with `chacha20-poly1305@openssh.com`. |
| Unknown key, changed key | Both stopped as `HostKeyRejected`, and no secret was requested before the decision |
| Wrong password | `SignInRefused`, listing `publickey` among the remaining methods |
| Keyboard-interactive-only user | Signs in through the fallback |
| Wrong passphrase / cancelled prompt | `WrongPassphrase` / `Cancelled` |
| Closed port, unresolvable name, non-routable address with a 1 s timeout | `Refused`, `Resolve`, `TimedOut` (within 3 s) |

Tests: 3 unit tests for `hostkey`, plus 6 integration tests in `tests/wsl.rs` that skip unless `HOLONET_TEST_SERVER=1`, so CI passes without the server.

## Milestone 2, phase 3: controls and shell (25 Nov 2026)

`holonet --gallery` shows every shared control in every state inside the app shell. Checked with real keyboard and mouse input on the Snapdragon (release build, 150% scale).

**Assets.** Inter 4.1 (Regular, Medium, SemiBold) and Space Grotesk 2.0.0 (Regular, Medium, Bold) are bundled under SIL OFL 1.1 (`assets/fonts`, about 1.6 MB). Space Grotesk ships no static SemiBold; weight 600 resolves to Bold, and titles look right. 34 stroke icons were extracted from the screen sources into `assets/icons`; `eye` and `eye-off` were added for password fields. The interface mono family resolves at startup: "JetBrains Mono" is not installed here, so the Nerd Font build is used.

**Modules.**
- `theme.rs`: every token from `tokens.json` under its own name, type styles as `Styled` extension methods, and the few off-token values from the screens, named.
- `ui`: `Button` (five kinds, compact, header, icon, disabled), `TextField`, `Select`, `Segmented`, `Checkbox`, `Tag` (group, neutral, key), group square, field with label and help, fieldset, key-value table, code box, callouts, dialog (standard and warning) and modal backdrop.
- `shell.rs`: `TitleBar` and `SideNav`.

**`TextField`** is adapted from GPUI's `examples/input.rs` (Apache-2.0, attributed in the file), keeping its IME support, grapheme-aware editing and clipboard handling. It adds horizontal scrolling that keeps the caret in view, a password mask with show/hide (copy and cut disabled for passwords), the mono variant, a placeholder and problem border, Tab/Shift-Tab focus movement, and Enter/Escape events. It took well under a day, so owner decision 1 (own controls on `gpui` 0.2.2) stands.

| Checked interactively | Result |
| --- | --- |
| Typing, Tab through Label → Group → Address → Port, Ctrl+A then retype | Works. Fields and selects need `focus_handle().tab_stop(true)` or `focus_next` skips them. |
| Select: arrow keys while focused, click to open, popup flips up near the window edge | Works. |
| Password: masked bullets, reveal toggle | Works. |
| Segmented, checkboxes, dialog checkbox | Toggle on click. |
| Warning dialog: layout matches Dialog.md; a backdrop click cancels | Works. The backdrop covers the body only, so the window buttons stay usable. |
| Title bar: tab click selects; double-click on the empty area maximizes and restores; window buttons | Works after a fix: the first version made the whole bar a drag area, which swallowed tab clicks. Only the brand and a spacer after "+" are drag areas now. |
| Events: text field `Changed` per keystroke, select `Changed(i)` | Delivered through `cx.subscribe`. |

**Known gaps** (small, fix when the screens use them):
- Tab names clip without an ellipsis.
- The select popup is not matched to the field's width.
- Buttons have no keyboard focus ring.
- The caret does not blink.
- Letter-spacing and uppercase transforms are not available in GPUI; section labels pass uppercase text.

**CI**: the first run on `milestone-2` passed on both jobs, including `windows-11-arm`, so the hosted ARM64 runner works for this private repository. That job is no longer experimental.

### Re-estimate (milestone 2 plan, phase 3 checkpoint)

Measured against the work phase 3 took, the remaining phases come to about four times phase 3:

| Phase | Relative size | Why |
| --- | --- | --- |
| 4 Vault flow | 0.5 | Two small screens on finished controls and a finished vault crate |
| 5 Hosts and host form | 1.5 | Two drawn screens, cards and list views, key-file import, Test connection |
| 6 Session tab | 1.25 | Wiring prompts across the Tokio and GPUI threads, the host key prompt, connection states, status bar |
| 7 Known hosts | 0.75 | One screen with a table, filters and a detail panel |
| 8 Wrap-up | 0.25 | Removing the dev paths, docs, PR |

The largest risk left is phase 6's prompt round-trip: a russh callback waiting on a GPUI dialog. Nothing found so far changes the plan.

## Milestone 2, phase 4: vault flow (25 Nov 2026)

`holonet` with no arguments is now the app. It shows Create vault on first start, Locked once a vault exists, then the workspace. `HOLONET_VAULT=<file>` points it at another vault for testing. The canned terminal demo moved to `--showcase`.

**Screens.**
- *Locked* follows Locked.dc.html (card, icon tile, title, body), with the vault password field in place of Windows Hello. It has two body texts: one at startup, one after "Lock now".
- *Create vault* is not drawn; it uses the same card. It has password and confirm fields, the 8-character minimum, and a warning that the vault cannot be recovered.
- *Workspace*: side navigation (Hosts, Settings, the vault card with Lock now) and a Hosts empty state (phase 5 replaces it).
- *Settings → Vault and security*, following SettingRow.md:
  - "Unknown host keys" (Ask every time / Refuse to connect), saved in the vault
  - "Change password", a dialog asking for the current password, the new one and a confirmation
  - "Vault file", with "Show in folder"

**Behaviour.** Argon2 runs on GPUI's background executor, so the window never freezes during unlock or a password change. Wrong passwords: no delay for the first three, then 1, 2, 4 … seconds (at most 30), with a countdown on the button. Lock now drops the workspace, which drops the unlocked vault and wipes its key. Password fields are cleared after use. A notice says when the vault was opened from its backup, and another when a save fails (the change is kept in memory).

**Checked with real input against a throwaway vault:**
- Too-short and mismatched passwords are refused with field messages.
- Create, then the workspace opens.
- A setting changed, then a restart, then unlock: the setting persisted.
- Lock now shows the manual-lock text.
- Five wrong passwords: the countdown shows, and Enter during it is ignored.
- Change password: a wrong current password is refused. After the change, the old password fails and the new one works.
- A damaged main file opens from the backup, with the notice.

**Bug found and fixed in `holonet-vault`.** Right after a password change, the backup (the previous save) still opened only with the old password. A damaged main file at that moment would have needed the old password. `change_password` now saves twice, so the backup uses the new password too. A test covers it (13 vault tests).

### GPUI layout notes (taffy)

- A child sized `relative(1.)` (100%) of a parent whose width comes from flex layout can collapse. The text field's text area measured 18 px. Make such children flex items (`flex_grow 1`, `flex_basis 0`) instead.
- A `div` in a column needs `w_full()` for its text to wrap at the column's width. Without it, wrapping is computed at the intrinsic width ("Try / again in 3 s.").
- A plain (non-flex) wrapper shrink-wraps flex content. Use `flex().flex_col()` wrappers in forms.
- Text without spaces never wraps, so long paths are shortened in the middle (`short_path`).

## Milestone 2, phase 5: Hosts and the host form (25 Nov 2026)

**Hosts page** (Main.dc.html):
- Header with the host count, "Find a host" search (label, address, user and tags; Escape clears it) and New host.
- "All hosts" with a Cards/List toggle. Cards use a grid whose column count follows the window width (GPUI grids take a fixed count).
- The detail panel shows the role icon tile, last connected, address, port, user, sign-in, host key (fingerprint if trusted, otherwise "Not trusted yet") and tags, with Edit and Duplicate. Double-clicking a card or row edits the host.
- An empty vault shows a welcome in FirstRun's layout, without the import cards (milestone 7). A search with no results shows "No hosts match" with Clear search.

**Host form** (NewHost/EditHost.dc.html), with the milestone 2 fields:
- General: label, primary address (required), port, tags.
- Credentials: username; SSH key or Password. A key comes from a select of vault keys plus "Import key file…". An encrypted key adds a passphrase field with "Save passphrase in the encrypted vault". Password adds "Save in the encrypted vault".
- Connection: keep connection alive.
- The aside shows the live title, the equivalent `ssh` command, a summary, problems (after the first Save), Test connection, and Save / Delete host / Cancel. On a narrow window it stacks below the form.
- The delete dialog has "Also forget its server key in Known hosts".
- "Save and connect" and the detail panel's Connect arrive in phase 6.

**`holonet-ssh` additions:**
- `inspect_private_key` reads OpenSSH keys' public half and encryption flag without the passphrase, and decodes unencrypted PEM/PKCS#8. It refuses encrypted PEM (with a `ssh-keygen -p` hint) and public keys.
- `probe(host, port, timeout)` runs TCP and the key exchange, captures the host key and stops before sign-in. This is "Test connection".
- Tests: 2 unit tests for key inspection (the second reads the `.dev` keys when present), plus a `probe` integration test against WSL.

**Checked with real input against a throwaway vault and the WSL server:**
- New host from the welcome screen, with live updates to the title and equivalent command.
- Test connection: "Reachable. Host key not yet trusted" with the server's real fingerprint.
- Import of the passphrase key through the Windows file dialog: recognised as encrypted Ed25519, passphrase saved.
- Save, then the card and detail panel.
- Edit (label added; Enter saves), Duplicate (selected afterwards), List view, search.
- Restart and unlock: both hosts and the key persisted.
- Delete through the warning dialog.
- Validation: `root@pi` refused with the address message.

**Fixed during the pass:**
- The key-value values overflowed the panel; they now shrink and clip.
- The list's Address column was cut so `:2222` read as `:22`; it is wider now.
- The Import button stretched to full width.

**Known gaps:**
- GPUI 0.2.2 clips truncated text but draws no "…", even with `text_ellipsis`.
- Every host shows "No group" until milestone 6.
- The role icon has no chooser yet (the design draws none); every host is a server.

## Milestone 2, phase 6: session tabs (25 Nov 2026)

Connect from the Hosts detail panel, by double-clicking a card or row, or with "Save and connect" on the form. Each connection opens a session tab (`session.rs`).

**How it fits together.**
- `SessionTab` owns the `Session` and an embedded `TerminalView`. The SSH work runs on Tokio.
- `UiPrompts` implements `holonet_ssh::Prompts` by sending each question (host key, password, passphrase, challenge) over a channel to the tab, and awaits the reply on a `oneshot`. Neither side blocks.
- Sessions belong to `HolonetApp`, not the workspace, so "Lock now" (which drops the workspace and its vault) leaves them connected. The Locked screen says how many are open, and they reappear on unlock (decisions.md).
- The title bar lists session tabs (state dot: ring while connecting, solid when connected, amber when failed or ended). "+" goes to Hosts, × disconnects. The session view has the 56 px icon rail (Hosts, Terminal, Settings).
- The terminal view gained `attach(shell)`, an embedded mode, and sizing from its own canvas (measured at paint, applied next frame, sent to the server).

**States drawn** (HostKey.dc.html, ConnectStates.dc.html, Terminal.dc.html):
- Connecting: spinner, a log above it, and Cancel.
- First-connection host key prompt: Trust and connect / Connect once / Cancel, with the fingerprint and Copy.
- Changed key: trusted and presented fingerprints side by side and Disconnect. "I expected this change" opens "Replace trusted key and connect".
- Secret prompts: password, passphrase or challenge, with "Save in the encrypted vault".
- Failures: could not reach, did not answer, password refused (Enter password), key refused, wrong passphrase (Enter passphrase), refused by "Refuse to connect". Each offers Retry or Edit host and Close tab.
- Session ended: the terminal dims, with Reconnect and Close tab.
- Status bar: state, cipher, key exchange, sign-in; "xterm-256color · UTF-8".

**Host key rules (decisions.md).**
- A trusted key connects silently and updates "last matched".
- An unknown key asks, or is refused under "Refuse to connect".
- A changed key always stops the connection before credentials. The presented key is recorded as `mismatch` for Known hosts, and replacing it starts a fresh connection.

**Checked with real input against the WSL server and a temporary second sshd** (port 2223, swapped between two throwaway host keys):

| Check | Result |
| --- | --- |
| First connect | Prompt with the server's real fingerprint; Trust and connect; passphrase key from the vault; terminal 150×28, which matches `stty size` |
| Lock with the session open, then unlock | The Locked screen lists 1 open session; a `sleep 4; echo` started before locking had printed when the tab came back |
| `exit` | Session ended bar, then Reconnect with no prompt (key trusted) |
| Password host, asked on connect | Trust; wrong password; "The server refused the password" (about 6 s: PAM delays failures, and the keyboard-interactive fallback tries once more); Enter password; connected; password saved only after success |
| Host key swapped on the server | The changed-key view stops the connection; Replace trusted key and connect works and uses the saved password |
| "Refuse to connect" with an unknown key | Refused before credentials, with the explanation |
| Cancel at the prompt | The tab closes and Hosts shows again |

**Bugs found and fixed during the pass:**
1. **Crash on Connect.** The new tab read the app while the app was creating it (GPUI double-lease panic). The connect timeout is now passed in.
2. **A mistyped password was saved** with "Save in the encrypted vault" ticked. Typed secrets are now saved only after the server accepts them.
3. **Prompt fields and the terminal did not take focus**: they were created without a window. Focus is now applied at the next render.
4. **Tall prompt cards were clipped.** The prompt area now scrolls, with spacers keeping cards centred.
5. **Singular wording on the Locked screen.**

**Not in milestone 2:** split panes, find, broadcast, snippets, automatic reconnect (milestone 3), and a dedicated key-refused screen variant.

## Milestone 2, phase 7: Known hosts (25 Nov 2026)

**Built.**
- Known hosts in the side navigation, between Hosts and Settings.
- Table: host, key type, fingerprint, trusted on, status. Filter pills: All, Saved hosts, No saved host, Needs review. Search matches name, address, fingerprint or key type.
- Detail panel: a mismatch callout with "Review mismatch", the trusted fingerprint, trusted on, last matched, source, a link to the saved host, Copy fingerprint, Copy public key and "Forget this key" (with a confirmation).
- Review dialog: the trusted and presented fingerprints side by side. "Keep the trusted key" clears the mismatch. "I expected this change" leads to "Replace trusted key"; the next connection then uses the new key.
- Links in: "Trusted in Known hosts" on the Hosts detail, and "Review in Known hosts" on a session's changed-key view, which unlocks to Known hosts with that entry selected.

**Checked with real input against the WSL server and a temporary second sshd** (port 2223, with its host key swapped):

| Check | Result |
| --- | --- |
| Changed key, then Review in Known hosts | Known hosts opened with the entry selected and "Mismatch seen" |
| Review, then Keep the trusted key | Mismatch cleared; "Needs review" shows nothing |
| Forget, with confirmation | Entry removed; the next connect shows the first-connection prompt |

**Fixed during the pass:** the copy buttons overflowed the 320 px panel (now stacked), and the changed-key card was narrower than its wrapper (the wrapper now takes the full width).

## Milestone 2, phase 8: wrap-up (25 Nov 2026)

**Removed from the app.** `holonet_ssh::dev` is gone from the library: its `.dev/test-server.env` reader and `FilePrompts` moved to `crates/holonet-ssh/tests/support/`, shared by the integration tests and the `shell` example. `holonet --ssh` and the terminal view's own connect path are removed; session tabs are the only way to connect.

**Done check from a clean profile**: release build, `APPDATA` pointed at an empty folder, no `HOLONET_VAULT`, no `.dev/` files used by the app.

| Step | Result |
| --- | --- |
| First start | Create your vault; vault created at `%APPDATA%\Holonet\vault.holonet` |
| Add a host | Welcome screen, New host, password host on the WSL server, Save and connect |
| Verify its key | First-connection prompt with the real fingerprint; Trust and connect |
| Use the terminal | `echo $(whoami) $(stty size)` printed `holotest 28 150`, matching the 150 × 28 tab |
| Restart | Unlock; Hosts lists the host with its key; Known hosts lists the trusted key with its saved host |
| Connect again | No prompts: trusted key, saved password |

Clippy, the unit tests and the WSL integration tests (7) pass.

**Milestone 2 is done.**

## Milestone 3, phase 0: spikes (25 Nov 2026)

JetBrainsMono Nerd Font v3.5.1 (`JetBrainsMono.tar.xz`, sha256 checked against the release) was extracted to a scratch folder for these tests. Nothing was installed in Windows.

### (a) Ligatures and icon widths in the cell grid: ligatures work, with one fix

`holonet --font-spike` (with `HOLONET_SPIKE_FONTS` set to the extracted files) shapes test strings in each build and paints them the way the terminal does, one glyph forced into each cell.

**Why milestone 1 saw no ligatures:**
- GPUI 0.2.2's DirectWrite backend (`platform/windows/direct_write.rs`) always calls `SetTypography`. When a font has no features set, the typography it passes is empty.
- An empty typography turns off DirectWrite's default `calt`, so no ligatures form anywhere in GPUI text.
- Asking for `calt` explicitly fixes it: `font.features = FontFeatures(vec![("calt", 1)])`.

**Results with `calt` explicit:**

| Build | `->` `=>` `!=` `===` `<=` `>=` `<!--` `\|>` `::` | On the grid |
| --- | --- | --- |
| JetBrainsMono Nerd Font, Mono and Propo | Ligatures form (glyph ids differ from single characters) | Yes: one glyph per character, every glyph at a whole cell, with or without forced width |
| JetBrainsMonoNL Nerd Font and NL Mono | No ligatures, as intended | Yes |

JetBrains Mono's ligatures replace each character with a same-width piece, so forcing glyphs into cells keeps them intact. A ligature forms only within one styled run, which is what other terminals do.

**Icon widths** are measured in cells, at 13 px with a cell of 7.80 px:

| Build | Powerline, folder, git branch, Linux, Rust, Ubuntu (Font Logos) |
| --- | --- |
| Mono, NL Mono | All exactly 1 cell |
| Nerd Font (non-Mono) | Advance 1 cell; the icon is drawn larger and spills into the next cell |
| Propo | Advances from 0.96 to 1.56 cells; in the grid it is placed per cell and spills like the non-Mono build |

**Decisions:**
- The design's default, "JetBrainsMono Nerd Font" (non-Mono), stays the default.
- The ligatures setting is shown. The renderer sets `calt` to 1 or 0 from it.
- Wide icons overlap the next cell, as in Windows Terminal and WezTerm. The Mono build is the choice for strict alignment, as its card says.

### (b) Sleep, resume and network changes: the APIs register without a window; real events not yet tested

`cargo run -p holonet-app --example power_spike --release` registers three Win32 callbacks, none of which need a window. This is good news for GPUI, which does not expose window messages.

| API | Result on Windows 11 ARM64 |
| --- | --- |
| `PowerRegisterSuspendResumeNotification` (`DEVICE_NOTIFY_CALLBACK`) | Returns 0 (registered) |
| `NotifyIpInterfaceChange` (`AF_UNSPEC`) | Returns 0 |
| `NotifyNetworkConnectivityHintChange` | Returns 0, and delivers the initial hint at once (level 3: internet) |

The live test was skipped: Wi-Fi off and on, then sleep and wake. It moves to phase 7's exit check.

The spike also logs whenever the wall clock and the monotonic clock jump apart, as a second sign of sleep. Phase 7 records which of these fire, in which order, and how soon after waking. The keep-alive timeout remains the fallback that catches every drop.

### (c) Embedding the fonts: cheap at run time, larger download

- **The 16 files to bundle:** Regular, Bold, Italic and Bold Italic of the Nerd Font, Mono, Propo and NL builds, offered as the four "Bundled" cards. They total 39.0 MB, or 21.6 MB zipped.
- **Registration:** GPUI's `add_fonts` took **4 ms** for 20 files (51 MB). Reading them from disk took 10 to 66 ms; embedding skips that.
- **Size:** today's `holonet.exe` is 18.5 MB, or 7.9 MB zipped. With the fonts embedded, the exe is about 58 MB and the release zip about 30 MB.

**Decision:** embed them with `include_bytes!`, as Inter and Space Grotesk already are. The cost is download size only, and a single-file exe is simpler to run unsigned. If size ever matters, the fallback is a `fonts/` folder beside the exe, loaded at start.

### Left in the tree for later phases
- `holonet --font-spike` (`src/font_spike.rs`) stays as a regression check until phase 2 embeds the fonts. It is removed in phase 9.
- `examples/power_spike.rs` and its `windows-sys` dev-dependency stay for phase 7's live test.

## Milestone 3, phase 1: settings model and lost connections (25 Nov 2026)

**`holonet-model`.**
- `Settings` gains `terminal: TerminalSettings` and `connection: ConnectionSettings`. Both use `#[serde(default)]`, so milestone 2 vaults open unchanged: the throwaway vault from milestone 2 unlocked and connected as before.
- `TerminalSettings` holds:
  - `font` (`Bundled(NerdFont | NerdFontMono | NerdFontPropo | NerdFontNl)` or `Installed(name)`)
  - `font_size` (10 to 20) and `line_height_tenths` (10 to 18), clamped when read
  - `ligatures`, `cursor` (Block, Bar or Underline), `cursor_blink`
  - `scheme` (Holonet Dark, High contrast or Light), `scrollback` (10,000, 50,000 or unlimited)
  - `copy_on_select`, `right_click_pastes`
- The defaults match `Settings.dc.html`. `ligatures_on()` is false for the NL build, whatever the switch says.
- `ConnectionSettings` holds `keep_alive` (30 s, 60 s or off) and `reconnect` (on). `keep_alive_for(host)` honours the host form's own keep-alive switch.
- 3 new tests: a settings block as milestone 2 wrote it, partial and clamped values, and the keep-alive combination.

**`holonet-ssh`.**
- `ConnectOptions::keep_alive` is now `Option<Duration>`, taken from Settings. Three unanswered keep-alives end the connection.
- A shell ends in one of two ways:
  - `Exit(code)` followed by `Closed`, or `Closed` alone: the shell ended, so the connection was not lost.
  - `Lost(LostReason)`: the channel ended with no exit status, EOF or close.
- The reason comes from russh's `Handler::disconnected` hook:
  - `ServerDisconnected(message)`
  - `NoResponse` (keep-alive or inactivity timeout)
  - `Network(detail)` (reset, or closed without a goodbye)
  - `Other`
- Only `Lost` will reconnect, in phase 7. Until then the pane shows it as an ended session with the reason.
- `Session::lost_reason()` exposes the same reason.

**New WSL tests** (10 in all now):

| Test | Result |
| --- | --- |
| `exit 3` in the shell | `Exit(3)`, then `Closed`; no lost reason |
| `kill -9 $PPID` (kills this connection's sshd process, no SSH goodbye) | `Lost(Network("it closed without warning"))` at once |
| Keep-alive every 1 s through a local proxy that stops forwarding but keeps the socket open | `Lost(NoResponse)` 2 to 8 s after the freeze |

The app shows the killed connection as "Session ended. The network connection failed: it closed without warning". Phases 6 and 7 replace this with the designed wording.
