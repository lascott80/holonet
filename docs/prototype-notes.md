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
- `holonet --font-spike` (`src/font_spike.rs`) stayed as a regression check until phase 2 embedded the fonts. Removed in phase 9.
- `examples/power_spike.rs` and its `windows-sys` dev-dependency stayed for phase 7's live test. Removed in phase 9.

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

## Milestone 3, phase 2: renderer and fonts (25 Nov 2026)

**Bundled fonts.**
- JetBrainsMono Nerd Font v3.5.1 lives in `crates/holonet-app/assets/fonts/jetbrainsmono-nerd/`. It holds Regular, Bold, Italic and Bold Italic of the Nerd Font, Mono, Propo and NL builds (16 files, 39 MB), plus `OFL.txt` and `NOTICE.md` (copyrights and the icon-set licences).
- The fonts are embedded and registered at start-up with Inter and Space Grotesk.
- `holonet.exe` grew from 18.5 MB to 56.7 MB.

**The terminal view follows `TerminalSettings`** (`TerminalView::apply_settings`):
- **Font, size and line height.** The cell width is measured from the chosen font. An installed font that is missing falls back to the preference list.
- **Ligatures.** `calt` is set to 1 or 0 explicitly; the phase 0 finding is that leaving it unset turns ligatures off. They are always off for the NL build.
- **Cursor:**
  - The default shape (block, bar or underline) and blink pass to alacritty's `default_cursor_style`, so programs can still change them with DECSCUSR (tested).
  - Blinking runs at 530 ms only while the pane has focus. It stays solid for a moment after typing or output.
  - It stops when Windows' "Animation effects" is off (`SPI_GETCLIENTAREAANIMATION`, `src/platform.rs`).
  - Without focus, the cursor is drawn as an outline.
- **Color schemes.** `Palette` holds Holonet Dark, High contrast and Light.
  - The design gives only the background, foreground, dim, accent and green for each scheme, so the 16 ANSI colors are built from those hues.
  - A test checks contrast against the background: 7:1 for the foreground, 4.5:1 for every ANSI color except black and bright black (3:1), and 4.5:1 for text on the selection and on the cursor.
  - In Light, "white" is a dark gray so programs that print white stay readable.
  - Dim text is mixed a third of the way toward the background, which works on light backgrounds too.
- **Scrollback.** 10,000 lines, 50,000 lines, or unlimited (no cap; alacritty grows the history as output arrives).
- **Copy on select** (on mouse up after a drag) and **right-click pastes**.
- Session tabs take these settings from the vault. `holonet --showcase` accepts options for screenshots before the Settings page exists: `light`, `contrast`, `bar`, `underline`, `noblink`, `mono`, `propo`, `nl`, `nolig`, `size=N`, `lh=N` and `font=Name_With_Underscores`.

**Box drawing (new, `src/boxdraw.rs`).**
- At the design's 1.6 line height, font glyphs for `│ ─ ┌` stop short of the row above and below, so table and `tmux` borders broke between rows.
- These are now drawn as rectangles that fill the cell, as Alacritty, WezTerm and Windows Terminal do. That covers U+2500 to U+257F (light, heavy, double; rounded corners drawn square) and the block elements U+2580 to U+259F (halves, eighths, quadrants, shades).
- Dashed lines and diagonals stay with the font.
- The segment table is generated from the Unicode names, and tests check its edges, weights and blocks.

**Checks.**
- Screenshots: each scheme, block, bar and underline cursors, the Nerd Font, Mono and NL builds, size 16 with line height 1.2, and a live WSL session printing box drawing, shades and ligatures.
- `--stream-bench` (10 MB), three runs each:

| | Throughput | Frames | Average paint |
| --- | --- | --- | --- |
| Before this phase | 72 to 85 MB/s | 64 to 71 fps | 1.9 to 2.1 ms |
| After | 79 to 81 MB/s | 64 to 71 fps | 2.3 to 2.5 ms |

- Paint costs about 0.4 ms more per frame (Nerd Font with `calt` shaping, plus the box check), still far inside a 16 ms frame. Throughput is unchanged.
- 22 app tests pass, 4 of them new (palettes, configure, box drawing).

**Notes.**
- Programs that choose 24-bit colors themselves keep them in every scheme. A prompt themed for dark backgrounds can look pale in Light; that is the program's color, not the palette's.
- The font licences are in the repository now. Showing them in Settings comes with phase 3, and shipping them beside the exe with release packaging in milestone 7.

## Milestone 3, phase 3: Settings page (25 Nov 2026)

The Settings page is rebuilt from `Settings.dc.html` in `workspace/settings.rs`. Section chips (Terminal, Vault and security, Connection, Keyboard, About) scroll to their section with GPUI's `ScrollHandle::scroll_to_top_of_item`.

**Terminal.**
- **Live preview.** A real `TerminalView` in a preview mode that never takes focus, fed the design's sample with palette colors plus a Nerd Font prompt line. It shows the actual font, ligatures, scheme and cursor. The header shows "font · size".
- **Font cards**, two equal columns, each drawn in its own font:
  - the four bundled builds, marked "Bundled"
  - Cascadia Mono and Consolas when installed, marked "System"
  - the installed font chosen, if it is another one
- **"Use another installed font"** opens a searchable list of every installed family. GPUI cannot tell monospaced fonts apart, so the dialog says that.
- **Installed fonts fall back to the bundled Mono build** for glyphs they lack, so Nerd Font prompt icons still draw in, say, Courier New.
- **Controls:**
  - Font size slider (10 to 20 px) and line height slider (1.0 to 1.8)
  - Font ligatures, off and disabled for the NL build, with the design's note
  - Cursor shape and Color scheme as segmented controls
  - Blinking cursor
  - Scrollback (10,000 lines, 50,000 lines or Unlimited)
  - Copy on select and Right-click pastes, as rows where the whole row toggles
- **New control: `ui::Slider`.** Click or drag on the track, or use the arrow keys, Home and End when focused. It shows the value in mono and has an accent focus ring, and it is in `--gallery`.

**Vault and security:** unchanged rows.

**Connection:** Keep-alive interval (applies to new connections). "Reconnect after sleep or network change" joins this section in phase 7, when reconnecting exists.

**Keyboard:** the design's read-only list, showing only shortcuts that work today (Copy and Paste). Each joins the list in the phase that adds it.

**About:** the version, and the bundled fonts with "View licences". That dialog shows `NOTICE.md` and the three OFL texts, compiled into the exe.

**Changes apply at once.**
- Each change saves to the vault and updates the preview.
- `HolonetApp::apply_terminal_settings` passes the change to every open session tab (deferred, so the workspace and the app are never updated at the same time). Each `TerminalView` applies it at its next frame, where a window is available, and re-measures its grid, so the server hears about the new size.

**Checked by hand against WSL, with a session open:**

| Change | Result |
| --- | --- |
| Mono build, then 16 px | Preview and the open session switched; the session resized to 122×23 |
| Light, then Bar cursor | Session redrawn light at once, with ANSI colors from the Light palette |
| About chip | Scrolled to the end of the page |
| View licences | Dialog with the notices and OFL texts; Close |
| "cour" in the installed font list | Courier New; picked; its own card appears, selected |
| Restart and unlock | Courier New, 16 px, Light and Bar still set |
| Courier New with Nerd icons in the preview | Icons drawn from the bundled fallback (empty boxes before the fallback was added) |
| Slider keys in the gallery | Right, End and Left step and clamp |

Afterwards the throwaway vault was set back to the defaults.

**Not on the page yet**, because those features don't exist: Appearance (milestone 7), SFTP (5), Updates (7), and the update notice.

## Milestone 3, phase 4: tabs (25 Nov 2026)

**Structure.**
- `SessionTab` is now `Pane` (`session.rs`): one host's connection, prompts, states and terminal, drawn as the pane card (group edge, pane header, content).
- The new `Tab` (`tab.rs`) holds the panes, one each until phase 5. It draws the session header and status bar for its active pane, and observes its panes so the header, status bar and title-bar dot follow them.
- `HolonetApp` keeps a list of tabs and observes each. Closing a pane closes its tab when it was the last pane. The Locked screen counts panes.

**Tab strip.**
- The vault tab is pinned. Session tabs shrink to 130 px, then scroll sideways (Terminal.dc.html: `overflow-x: auto`). The active tab is scrolled into view whenever it changes.
- Dots, as Tab.md specifies: solid when connected, a ring while connecting, `warn` when failed or ended.
- "+" stays just after the strip.

**New-tab host picker** (`host_picker.rs`, owner decision 3).
- "+" or Ctrl Shift T opens a 380 px popup under "+", at the position recorded when "+" was painted. It holds a search field and the saved hosts, sorted by name, each with `user@address:port`.
- Typing filters on the name, address, user and tags. Up and Down move the highlight, Enter connects in a new tab, and Esc clears the search and then closes.
- An outside click closes it.
- With no saved hosts it says so and offers "New host", which opens the host form.

**Shortcuts.**
- Ctrl Shift T (new tab), Ctrl Shift W (close the active pane, which is the tab for now), Ctrl Tab and Ctrl Shift Tab (next and previous, through the vault tab and the session tabs).
- They are GPUI actions bound in the `HolonetApp` context. GPUI matches bindings before key-down handlers, so they win over the terminal while every other key still reaches the shell.
- Settings, Keyboard now lists them.

**Bugs found and fixed while testing:**
1. **Ctrl Shift T did nothing right after unlocking.** Nothing had keyboard focus, so GPUI had no element to dispatch the binding from. The app root now has a focus handle and takes focus when nothing else holds it.
2. **Ctrl Tab did nothing on the vault view.** Focus stayed on the hidden terminal of the tab just left. Selecting the vault view now focuses the app root.
3. **The vault tab scrolled out of view** with many tabs. It now sits outside the scrolling part, as the design pins it.

**Checked with real input against WSL** (two hosts: `wsl-test` by key, and `localhost`, trusted on first connect, with its passphrase saved):

| Check | Result |
| --- | --- |
| Ten tabs from the keyboard (Ctrl Shift T, type, Enter), alternating hosts | All connected; strip scrolled to the newest; vault tab still visible |
| Labelled each shell `TAB-n`, then Ctrl Tab through all of them | Correct order; wraps through the vault tab |
| Ctrl Tab to tab 3, Ctrl Shift W | TAB-3 closed; TAB-4 active and focused |
| ✕ on tab 1 | Closed; TAB-4 stayed active |
| Ctrl Shift Tab | TAB-2 |
| Lock now | "8 sessions are open"; after unlocking all 8 tabs are back |
| Ctrl Shift W eight times, then Ctrl Shift T | All closed, the vault view shows, the picker opens |

## Milestone 3, phase 5: split panes (25 Nov 2026)

**Pane tree** (`tab.rs`).
- A tab holds a tree. A leaf is one pane. A split puts two subtrees side by side (Split right) or one above the other (Split down), sized by a ratio.
- A tab holds at most four panes (owner decision 1). At four, the Split buttons are disabled and the header says "A tab holds up to 4 panes."
- Splitting replaces the active leaf with a split of it and the new pane. The new pane starts at half the size, takes focus and becomes active.
- Removing a leaf puts its sibling in the parent's place.

**Layout.**
- Panes sit on the `sunken` gutter with an 8 px gap (Terminal.dc.html). Each split's halves use `flex-grow` set to the ratio with a zero basis; Taffy collapses percentage widths inside flex parents.
- The active pane has the `ink-muted` border, and the others `line`. Panes without focus draw an outline cursor.

**Resizing.**
- The 8 px gap between two subtrees is the handle, with a resize cursor and a 2 px line: half-strength accent on hover, full accent while dragging (an undrawn state).
- Each split records its bounds when painted, so a drag maps straight to a ratio.
- Panes stay at least 190 px wide or 170 px tall: about 20 columns by 5 rows at the default font, plus the pane header and padding.
- Every terminal re-measures after a drag and sends its new size to the server.

**Controls.**
- The session header has **Split right** and **Split down** (44 px, as drawn), each with a chevron menu: "Same host" (with its shortcut) and every saved host (owner decision 2).
- The menu is drawn with `deferred` plus `anchored`, so it appears above the panes. It closes on an outside click.
- Shortcuts:
  - Ctrl Shift D: split right, same host
  - Ctrl Shift E: split down, same host
  - Ctrl Shift ] and Ctrl Shift [: next and previous pane
  - Ctrl Shift W: close the active pane, and the tab with its last pane
- With Shift held, Windows reports `]` and `[` as `}` and `{`, so both spellings are bound. Found during testing: the first build only bound `ctrl-shift-]`, and nothing happened.
- Clicking anywhere in a pane makes it active (`capture_any_mouse_down`, so the terminal still gets the click).
- A pane's "Close tab" button reads "Close pane" when the tab holds more than one pane.
- The header and status bar follow the active pane. The tab's name and dot follow it too.
- Settings, Keyboard lists the new shortcuts.

**Checked with real input against WSL:**

| Check | Result |
| --- | --- |
| wsl-test, then Ctrl Shift D, Ctrl Shift E, then the Split right menu choosing localhost | Four panes, two hosts; the header shows localhost; Split disabled with the message |
| `stty size` in each pane, moving with Ctrl Shift ] | 28 72, 12 72, 12 33, 12 33, matching each header |
| Dragged the main gap from x 1012 to 700 | Left pane 45×28, the right column 99×12 and 46×12 twice; `stty size` matched all four |
| Ctrl Shift W on the active pane | It closed; its sibling took the space; Split enabled again |
| `exit` in another pane | Ended bar with "Close pane"; clicking it closed the pane |
| Ctrl Shift W twice more | The tab closed; the vault view came back |
| Split down, then a click in the top pane, then typing | The text went to the top pane |

## Milestone 3, phase 6: connection states (25 Nov 2026)

The pane's states now follow `ConnectStates.dc.html`. Each has its log lines at the top, a centred message, buttons, and a matching word in the pane header and status bar.

**Log lines.**
- Each attempt starts a fresh log: "Resolving <name> …" (names only), then "Opening TCP connection to <address:port>".
- Then the host key lines, then "Offering password for <user>" or "Offering key <name> for <user>" once the key is settled.
- Failures add a warn line, such as "No response after 10 seconds", "Server: Permission denied (publickey,password,keyboard-interactive)", "The passphrase did not unlock <key>", or the network reason.

**States** (header word in brackets):

| State | Title | Body | Buttons |
| --- | --- | --- | --- |
| Connecting (Connecting) | Connecting to <name> | Trying <address>. (Tailscale wording waits for milestone 4) | Cancel |
| Not reachable | Holonet could not reach <name> | The reason (refused, unresolved, network error) | Retry, Edit host |
| Timed out | <name> did not answer | "<address> is a private address, so you may be away from its network.", or "The server may be down…" for public addresses | Retry, Edit host |
| Sign-in refused (password) | The server refused the saved password (or "the password" when typed) | The password may have changed… | Enter password, Edit host |
| Sign-in refused (key) | The server refused the key <name> | Not in authorized_keys for <user>, or key sign-in not allowed | Retry, Edit host |
| Wrong passphrase | The key passphrase is wrong | Enter it again; the server was not contacted with this key | Enter passphrase, Edit host |

- Every failure also has a quiet "Close tab", or "Close pane" in a split. The design draws none, but a pane needs a way out besides the tab's ✕.
- The spinner is the design's: a 3 px `line` ring with an accent arc turning on it every 0.9 s. GPUI borders take one color, so the arc is an SVG over the ring.
- New: after **Edit host** from a stopped pane, **Save and connect** retries in that pane instead of opening another tab. Testing showed failed tabs piling up without this.
- Two unit tests: the private-address wording, and that every failure has a header word.

**Reached on purpose against WSL:**

| How | Result |
| --- | --- |
| A saved host on port 2223 (nothing listening) | Not reachable: "The host refused the connection. Check the port, and that SSH is running." |
| Edit host on that pane, address 10.255.255.1 | Ring spinner and "Trying 10.255.255.1.", then after 10 s Timed out with the private-address text |
| New host with a key, then a wrong passphrase at the prompt | Wrong passphrase |
| Edit host to password sign-in, then a wrong password at the prompt | Sign-in refused: "The server refused the password", with the server's method list |
| Edit host to the keyboard-interactive-only user, with the key | Sign-in refused: "The server refused the key holotest_ed25519_pass" |
| Edit host back to `holotest`, Save and connect | Retried in the same pane and connected; still one tab |

Not reached by hand: "the saved password" wording. That path depends only on whether the attempt used the vault's password, and the typed-password case above shows the shared code.

## Milestone 3, phase 7: automatic reconnect (25 Nov 2026)

**Behaviour** (decisions.md: on, 5 attempts, scrollback kept, the shell restarts so nothing is replayed).
- The terminal now reports `TerminalEvent::Lost(reason)` separately from `Ended`. The pane keeps its `TerminalView`, so the scrollback stays.
- The pane shows the "Reconnecting" banner from ConnectStates.dc.html above the old output, which is dimmed to 75%:
  - the cause in bold, then "Reconnecting, attempt n of 5." or "Attempt n of 5 failed. Trying again in n s."
  - the last attempt's error underneath
  - **Reconnect now** (enabled while waiting) and **Close tab** / **Close pane**
  - the design's footnote, "Scrollback is kept. The shell session itself restarts after reconnecting."
- Attempts wait 1, 2, 4, 8 and 16 s. Only network failures retry: refused, unresolved, network error or timed out. A refused sign-in shows the usual failure.
- **A changed host key during the outage stops reconnecting** with the changed-key view, as on any connection (decisions.md: always stop).
- While the vault is locked, a key cannot be checked, so the attempt waits and says "Unlock the vault so Holonet can check the host key."
- After five failures: "Holonet could not reconnect to <name>. It tried 5 times. <last reason>", with Retry, Edit host and Close.
- On success the terminal gets a dim separator line, "-- Reconnected at HH:MM. The shell restarted; the output above is from the previous session. --", then the new shell.
- **Typed secrets.** A password or passphrase typed at a prompt is kept in the pane's memory once it works, and used again when reconnecting. It is dropped with the pane, and saved to the vault only if "Save in the encrypted vault" was ticked.
- **Cause.**
  - `platform::watch_system` registers the three Win32 callbacks from phase 0 (`PowerRegisterSuspendResumeNotification`, `NotifyIpInterfaceChange`, `NotifyNetworkConnectivityHintChange`) and passes the events to every pane.
  - A drop within five minutes of a sleep or wake reads "Connection lost when this PC went to sleep."; after a network change, "Connection lost after a network change."
  - Otherwise the text comes from the lost reason: "The server stopped answering." or "The connection was lost."
  - A wake or network change while waiting starts the next attempt at once.
- **Settings, Connection:** "Reconnect after sleep or network change", on by default. Off, a drop ends the session as before.

**Checked against WSL:**

| Check | Result |
| --- | --- |
| `kill -9 $PPID` in a live shell (drops the TCP connection) | Banner, then reconnected after about 1 s; old output and the separator line in scrollback; `ps` shows a single new login shell |
| `systemctl stop ssh.socket`, drop, `start` after about 5 s | Attempts 1 and 2 refused, with "The host refused the connection…" under the banner; attempt 3 reconnected |
| Socket kept stopped | Five attempts (Windows takes about 2 s to report a refused localhost connection, so most of the time an attempt is in flight), then "could not reconnect… It tried 5 times." Retry after starting the socket reconnected |
| Temporary sshd on 2223 with throwaway key A, trusted; restarted with key B; drop | Reconnect stopped at "The host key for blackhole has changed", showing key A as trusted and key B as presented; no credentials sent. The temporary sshd and both keys were removed afterwards |
| Password typed at the prompt with Save unticked, then a drop | Reconnected without asking; the separator confirms it was a reconnect |
| Reconnect switched off, then a drop | "Session ended" bar with Reconnect and Close, as before; switched back on afterwards |

**Live sleep and Wi-Fi test** (the owner toggled Wi-Fi, then slept the PC for about 40 s, with the phase 0 logger and Holonet both running):
- Wi-Fi off and on: interface parameter changes, and connectivity hints going from 3 to 2 and back to 3.
- Sleep: `PBT_APMSUSPEND`, then on wake `PBT_APMRESUMEAUTOMATIC` and `PBT_APMRESUMESUSPEND` within 1 ms, followed by a burst of interface changes.
- The monotonic clock (`Instant`, which uses QPC) counted through sleep: wall and monotonic both read 39.4 s. A clock-gap check could not detect sleep, so the power notification is the signal to use.
- The open sessions are to WSL on this PC, so the connection survived sleep: typing after wake worked at once. **A drop after sleep from a remote server was not tested** (there is none in this setup). The code path is the same as the tests above, with the cause set by the power event.

## Milestone 3, phase 8: find in the terminal (25 Nov 2026)

**Behaviour** (Terminal.dc.html's find bar).
- **Opening and closing:**
  - Ctrl Shift F or the header's **Find** button opens a bar under the tab header. The button has an accent border while the bar is open.
  - Ctrl Shift F with the bar open puts focus back in its field.
  - Esc or the button closes the bar and clears the highlights. The view stays where it was scrolled.
- **Searching:**
  - The search covers the active pane's history and screen, one line at a time, so a match never spans a wrapped line.
  - It is plain text and ignores case by default. **Match case** and **Regex** (the `regex` crate) change that.
  - An invalid regex shows "Invalid regex" in amber and clears the highlights.
- **Count:** "Type to search", "No matches", "1 line matches" or "N lines match", as drawn.
- **Moving between matches:**
  - Every match gets a faint accent background and the current match a stronger one. Both pass 4.5:1 with the foreground in all three schemes, and a test checks this.
  - The first current match is the newest. Enter and the up button go to the previous (older) match; Shift Enter and the down button go to the next. Both wrap around.
  - When the current match is off screen, the view scrolls so it sits a few lines from the top (at most 3, and a third of the pane in short panes).
- **Live updates:** new output re-runs the search, at most every 150 ms, and the current match keeps its distance from the bottom.
- **Panes:** the bar follows the active pane. Clicking another pane clears the old pane's highlights and searches the new one.

**`--find-bench`** (10,000-line history at 150×40, from `sample::bulk`; release build, ARM64):

| Query | Matches | Find | Snapshot with highlights |
| --- | --- | --- | --- |
| plain, every line | 10,039 | 6.95 ms | 0.43 ms |
| plain, few lines | 11 | 7.92 ms | 0.22 ms |
| plain, no match | 0 | 6.57 ms | 0.22 ms |
| regex `took=\d{3}ms` | 6,039 | 8.98 ms | 0.40 ms |
| case-sensitive | 10,039 | 7.44 ms | 0.36 ms |

Every keystroke's search plus a frame stays under 16 ms. The time is spent reading cells into a string, so it barely depends on the query.

**Checked against WSL** (`seq 1 3000 | sed 's/^/Row /'`):

| Check | Result |
| --- | --- |
| "row 15" | "111 lines match" (Row 15, 150–159, 1500–1599); Row 1599 current |
| Enter twice, then Shift Enter | Current moved to 1597, then back to 1598 |
| Match case on | "No matches" |
| Regex on, `Row (2\d` | "Invalid regex" in amber |
| `Row (2\d00)$` | "10 lines match"; the view jumped from the bottom to Row 2900 |
| Esc | Bar closed, highlights gone |
| Split right, "Row 2999" in the new pane | "No matches"; clicking the first pane gave "1 line matches" with Row 2999 highlighted |

## Milestone 3, phase 9: wrap-up (25 Nov 2026)

**Removed.** The phase 0 spikes are gone: `holonet --font-spike` (`src/font_spike.rs`), `examples/power_spike.rs`, and the `windows-sys` dev-dependency. The live code from them is `assets.rs` (embedded fonts) and `platform::watch_system`.

**Done check from a clean profile**: release build, a new vault file in an empty folder (the vault holds every setting, so this is a clean profile), the WSL test server.

| Done means | Result |
| --- | --- |
| Several tabs from the new-tab host picker | Created the vault, added hosts `wsl-a` and `wsl-b`. "+" opened the picker with both; `wsl-a` showed the first-connection key prompt, Trust and connect. Ctrl Shift T, "wsl-b", Enter opened a second tab with no prompt (same server key) |
| Nested panes with a second host, resized by dragging | In the `wsl-b` tab, the Split right menu picked `wsl-a`, then Split down nested a third pane. Dragging the divider changed the panes from 72 to 96 and 49 columns |
| Find in a pane's scrollback | `seq 1 500`, Ctrl Shift F, "42": "15 lines match" (42, 142, 242, 342, 420–429, 442); Enter moved to the older match |
| Font, size and scheme apply to every open terminal at once | Settings: Cascadia Mono, 17 px, Light. All four terminals in both tabs changed at once |
| A dropped connection reconnects, keeps its scrollback and says what happened | `kill -9 $PPID`: "The connection was lost. Reconnecting, attempt 1 of 5." over the dimmed output, then connected. Find for "before-drop" found it above "-- Reconnected at 19:18. The shell restarted; …" |

fmt, clippy `-D warnings`, the unit tests and the WSL integration tests pass.

**Milestone 3 is done.** Carried forward, not tested here: a drop after sleep from a remote server (phase 7).

## Milestone 4, phase 0: spikes (25 Nov 2026)

The spike programs stay in the tree until the phases that replace them land, then go in phase 9:
- `crates/holonet-ssh/examples/m4_keygen.rs`, `m4_jump.rs`
- `crates/holonet-app/examples/m4_tailscale.rs`, `m4_lock.rs`, `m4_hello.rs`

### (a) Windows Hello: signatures repeat, so the vault can have a Hello key slot
`m4_hello` opens an ordinary window as a stand-in for Holonet's. It then creates a Hello key credential with `KeyCredentialManager::RequestCreateAsync` (`windows` 0.62, plain calls without a window handle), signs the same 32-byte challenge twice, compares the signatures, and deletes the credential.

| Step | Result |
| --- | --- |
| `IsSupportedAsync` | true |
| Create | Success after 4.4 s (the owner's Hello check) |
| Sign 1 | Success after 0.5 s with no new prompt: Hello reused the check just made |
| Sign 2 | Success after 3.3 s, with a prompt |
| Signatures | 256 bytes each (RSA 2048), **identical** |
| Prompt placement | All three prompts opened in front, without the "Credential Dialog Xaml Host" workaround |

- Microsoft's documentation describes these signatures as RSA-PSS, which would normally differ every time. On this PC they repeat, which matches KeePassXC's experience. So the **Hello key slot** design from the plan goes ahead: wrap the data key under SHA-256 of the signature over a random challenge stored in the slot.
- Phase 1 tests the slot with a fake signer. Phase 8 confirms live that a signature made in a new process, after `OpenAsync`, matches the one made when the slot was created. If it ever does not, the password slot still opens the vault, and Hello is set up again.
- The focus workaround is kept in reserve and not built unless phase 8 shows a prompt behind the window.

### (b) Windows lock and unlock: a message-only window hears them
`m4_lock` runs its own thread with a message loop. It registers two windows with `WTSRegisterSessionNotification`: a message-only window (`HWND_MESSAGE`) and a hidden ordinary one. The owner pressed Win L and unlocked about 10 s later:

```
28.9 s  message-only  WTS_SESSION_LOCK (0x7)
28.9 s  hidden window WTS_SESSION_LOCK (0x7)
39.3 s  message-only  WTS_SESSION_UNLOCK (0x8)
39.3 s  hidden window WTS_SESSION_UNLOCK (0x8)
```

The message-only window is enough. `platform::watch_system` gains it in phase 8, on a thread of its own, feeding the same channel as sleep and network changes.

### (c) Key generation: fast in release, slow RSA in debug
`m4_keygen` generates each type with `ssh-key` 0.7.0-rc.12 (re-exported by russh 0.65.4, with `rand` 0.10). It encrypts each key with a passphrase (bcrypt-pbkdf, AES-256-CTR), writes it as OpenSSH text, and reads it back with `decode_secret_key`. The round trip matched every time.

| Key | Generate, release | Generate, debug | Encrypt and encode, release | Decode, release |
| --- | --- | --- | --- | --- |
| Ed25519 | 0.1 ms | 0.4 ms | 95 ms | 88 ms |
| ECDSA P-256 | 0.4 ms | 4.7 ms | 89 ms | 88 ms |
| RSA 4096 (3 runs) | 205–409 ms | 3.4–14.6 s | 86–88 ms | 87–105 ms |

- RSA generation time varies a lot, because it is a prime search. Generation runs off the UI thread with a progress state. The RSA crates get `opt-level = 3` in the dev profile so debug builds stay usable.
- About 90 ms of every encrypt or decode is bcrypt-pbkdf's work factor. That cost falls on each connect with a passphrase-protected key, which is acceptable.

### (d) Tailscale LocalAPI: works over the pipe, no admin
`m4_tailscale` opens `\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled` and sends `GET /localapi/v0/status` (headers `Host: local-tailscaled.sock`, `Sec-Tailscale: localapi`).

- **With Tokio's named-pipe client** it got `200 OK`, BackendState "Running", this PC's own addresses and the peer count. The round trip took 0.3–0.8 ms for 21 KB, as a normal user, with or without the identification-level impersonation flag.
- **`std::fs::OpenOptions`** failed on the same path with "path not found" (os error 3), with or without the flag. `holonet-net` uses Tokio's client.
- Tailscale 1.104.7 is installed and native ARM64. When Tailscale is stopped the pipe is missing, which reads as "not running"; phase 3 checks that live.

### (e) Jump host: `direct-tcpip` plus `connect_stream` works
`m4_jump` signs in to the WSL server, opens `channel_open_direct_tcpip` from it to `127.0.0.1:2222`, and runs a second `client::connect_stream` session over `Channel::into_stream()`. Both host keys are checked against `.dev/known_hosts`, each with its own address.

| Case | Result |
| --- | --- |
| Through the jump host to itself | Both keys matched; signed in to the jump host in 222 ms, then the target in 454 ms. `$SSH_CONNECTION` differed between the two sessions |
| Closing | Disconnecting the inner session left the outer one open, then the outer one closed |
| Target port closed | "Failed to open channel (ConnectFailed)" from the jump host. Phase 7 words this as "bastion could not reach app-01:22" |
| Target name not in Known hosts (`localhost`) | "Unknown server key": stopped, nothing trusted |

### (f) WSL on this PC's Tailscale address: not reachable
A TCP connection to this PC's own Tailscale IPv4 address on port 2222 was refused. WSL runs in NAT mode, and only `localhost` is forwarded into it. So the WSL test server cannot stand in for "a host on the tailnet" without a change. How phase 6 tests the fallback is an open question for the owner, recorded in the plan once decided.

## Milestone 4, phase 1: model and vault (25 Nov 2026)

**Model** (`holonet-model`, schema 2):
- **`Host`** gains:
  - `tailscale_address` and `route` (Automatic, Primary or Tailscale). `effective_route()` is Primary when no Tailscale address is set, as the host form forces.
  - `jump_host`, a saved host.
  - `identity`, which replaces the host's own username and sign-in when set (`VaultData::sign_in`).
  - A Tailscale address must pass the same check as the primary address.
- **`ssh_command_via(jump)`** builds the drawn `ssh -J bastion-east deploy@10.30.0.9`. The target is the Tailscale address only under the Tailscale route.
- **`Identity`** holds a name, a username and an `Auth`. `VaultData.identities` stores them.
- **`StoredKey`** gains `comment` and `saved_passphrase`.
- **`Settings`** gains `lock_when_windows_locks` (on) and `hello_unlock` (off until a Hello slot exists). `ConnectionSettings.fallback_wait` offers 2, 3, 5 or 10 s, default 3.
- **`VaultData::jump_chain`** lists the hops in order. It refuses a loop, including a host as its own jump host, and reports a deleted hop.
- **`key_users`** returns the hosts (directly or through an identity) and the identities that use a key, for the Keychain's Hosts column and the delete prompt.

**Migration:**
- Schema 1 saved a key's passphrase on each host (`Auth::Key.passphrase`). That field is still read, as `legacy_passphrase`, but never written.
- `VaultData::migrate` moves the passphrase onto the key; when hosts disagree, the first one wins. Unlock runs it, and the next save writes schema 2.
- The host form now saves the passphrase on the key, and a passphrase typed at a prompt and saved goes to the key too.

**Vault** (`holonet-vault`):
- A **`hello` slot** holds the Hello credential's name, a random 32-byte challenge, and the data key wrapped with XChaCha20-Poly1305 under SHA-256 of Hello's signature. The vault crate never calls Windows; the app passes the signature in.
- `LockedVault::hello()` returns the credential and challenge to sign, and `unlock_with_hello(signature)` opens the vault.
- `UnlockedVault::set_hello` adds or replaces the slot, and `remove_hello` removes it. Both save twice so the backup matches.
- Changing the password now replaces only the password slot, so it keeps the Hello slot.
- Files with a Hello slot are written as **format 2**, so a milestone 3 build says "saved by a newer version" instead of failing on the unknown slot. Without one, files stay format 1.

**Checks:**
- Unit tests, using a fake signer that returns the same bytes for the same challenge:
  - the Hello round trip, including that another device's signature is refused and that the backup matches
  - the password change keeping Hello
  - removing Hello falling back to format 1
  - schema 1 data migrating on unlock
  - jump chains and loops
  - routes and `-J`
  - identity sign-in and key users
  - a schema 1 vault's JSON migrating, with the passphrase written only once
- Live, on a copy of milestone 3's test vault: unlocked, connected `wsl-test` with its passphrase-protected key and no prompt, closed, reopened and connected again with no prompt.
- fmt, clippy, the unit tests (28, 13, 5, 10 and 16) and the WSL tests (10) pass.

## Milestone 4, phase 2: SSH (25 Nov 2026)

**Key generation** (`holonet_ssh::generate_key`, `KeyKind`):
- Ed25519, RSA 4096 and ECDSA P-256, with the comment set. With a passphrase, the key is encrypted the OpenSSH way (bcrypt-pbkdf, AES-256-CTR).
- `KeyKind` carries the Generate panel's labels and notes as drawn.
- The workspace optimises `rsa`, `crypto-bigint`, `crypto-primes`, `bcrypt-pbkdf`, `blowfish`, `argon2` and `blake2` even in dev builds. In the WSL test (debug build), generation took 0.3 ms for Ed25519, 5 ms for ECDSA and 0.3 s for RSA 4096. In phase 0's unoptimised debug build, RSA 4096 took 3–15 s.

**PuTTY keys** (`.ppk` v2 and v3):
- `inspect_private_key` reads a `.ppk` file's header and public lines without the passphrase: type, comment, public key and whether it is encrypted.
- `convert_ppk` decodes the key, with its passphrase when it has one, and returns it in OpenSSH format, encrypted again with the same passphrase. Holonet stores that.
- **Checked against PuTTY 0.83's own `puttygen`** (`putty-tools`, installed in the WSL test distribution with the owner's approval). Windows PuTTYgen has no command-line conversion. puttygen made 40 keys, each with its own OpenSSH export as the reference: Ed25519 ×12, ECDSA P-256 ×4 and RSA 3072 ×4, each in PPK v2 and v3, encrypted and not.
  - All 39 checkable keys converted to exactly the same private key, with the same passphrase.
  - The 40th was skipped because ssh-key can't read puttygen's OpenSSH export of it, although OpenSSH's `ssh-keygen` reads it. Its `.ppk` still converted.
- Four of these throwaway keys are fixtures in `crates/holonet-ssh/tests/fixtures/`, for the unit test. They were never installed anywhere.
- `examples/m4_ppk.rs` repeats the check over a folder of puttygen files.
- My first unit test wrote its own `.ppk` and failed for about half of Ed25519 keys. The writer was wrong (it encoded the private value with an extra sign byte), not the reader. The real files replaced it.
- **Known limit, upstream:** ssh-key 0.7.0-rc.12 refused one valid ECDSA key in OpenSSH format, puttygen's export above, that `ssh-keygen` accepts. ECDSA private keys imported from OpenSSH files may occasionally fail with "not a private key Holonet can read". Recorded for the Keychain's import error text in phase 4.

**Jump hosts** (`ConnectOptions::jump`):
- A jump host is a full `ConnectOptions` of its own, with its own sign-in, prompts, keep-alive and its own jump host. `connect` signs in to it first, opens `direct-tcpip` to the target (within the connect timeout), then runs the handshake and sign-in over that channel.
- The target's host key is checked against the address the jump host sees.
- `Session` keeps its jump host session open (`jump_host()`). `close` closes the target, then the jump hosts.
- **Errors:**
  - A failure at a jump host is `ConnectError::JumpHost { host, error }`. `innermost()` gives the real error and the hops it passed through.
  - A jump host that cannot reach the target is `JumpCouldNotReach { jump, target, reason }`, for example "the connection was refused, or nothing answered".
- **Lost connections:** a shell notices a dropped jump host as `LostReason::JumpHost { host, reason }` ("The connection to the jump host 127.0.0.1 was lost. …"). The jump host's reason is preferred, because it is the cause; the shell waits up to 0.2 s for it after the target's own reason.

**Install on host** (`Session::install_public_key`):
- One remote command creates `~/.ssh` (700) and `authorized_keys` (600) if needed, then appends the line unless the same type and key is already there, whatever the comment. It adds a newline first if the file lacks one.
- Existing lines are never changed.
- It returns `Added` or `AlreadyPresent`, or the server's error text.
- `Session::exec` runs a command and collects its output and exit status.

**Connecting to a chosen address:** `ConnectOptions.host` is already whatever address the caller chooses, and the host key check uses it. That means a host reached over Tailscale checks its key under the Tailscale address, which is a separate Known hosts entry. Phase 6 decides how a host's two addresses share a trusted key.

**WSL tests** (13, all pass):

| Test | Result |
| --- | --- |
| Generate, install, sign in | Ed25519, Ed25519 with a passphrase, ECDSA P-256 and RSA 4096: each installed (`Added`), installed again (`AlreadyPresent`), then signed in by public key. `~/.ssh` stays 700 and `authorized_keys` 600. The test's keys are removed afterwards; none are left |
| Jump host to itself | Separate `$SSH_CONNECTION` on each hop. The target signed in with a key and the jump host with a password. Closing the target closed the jump host |
| Jump host, closed target port | `JumpCouldNotReach`: "the connection was refused, or nothing answered" |
| Jump host's key unknown | Stopped at the first hop: `JumpHost { error: HostKeyRejected }`, with no credentials sent |
| Target's key unknown under its name | `HostKeyRejected` |
| Jump host's connection killed under a running target shell | `ShellEvent::Lost(LostReason::JumpHost { .. })` |

**sshd's per-source penalties.** OpenSSH 10 drops new connections from an address for a while after failed sign-ins (the log shows "srclimit_penalise … penalty: failed authentication"). My first version of the key test also tried each key before installing it. Those deliberate failures, together with the wrong-password tests running in parallel, got 127.0.0.1 penalised for 18 s, and the next connection was dropped ("Disconnected"). A freshly generated key cannot already be authorized, so that check was removed. Signing in with it proves the install.

fmt, clippy, and the unit tests (28, 13, 7, 13 and 16) pass. The WSL tests (13) pass.

## Milestone 4, phase 3: `holonet-net` (25 Nov 2026)

A new crate with no window, so it tests without one.

**Tailscale status** (`tailscale::status()`):
- Asks the LocalAPI over its named pipe with Tokio's client and reads the HTTP answer, including chunked bodies.
- If the pipe answers badly, it runs `tailscale.exe status --json` with no console window. It never waits more than 3 s.
- **Result:**
  - `NotInstalled`
  - `NotRunning`: no pipe, though the client is installed
  - `NotConnected { state }`: for example "Stopped" when switched off, or "NeedsLogin"
  - `Running(status)`: this PC's own addresses, the MagicDNS suffix, and peers with their names, addresses and whether they are online
- `problem()` gives the sentence a pane shows when Tailscale cannot be used: "Tailscale on this PC is switched off. Connect it, then try again."
- `TailscaleStatus::peer(address)` finds the device a Tailscale address names: a 100.x address, the full MagicDNS name, or the short name.

**Route check** (`route::route_to`, `primary_unreachable`):
- `GetBestRoute2` says which route Windows would use: a specific route (with its prefix length), only the default route, or none.
- `primary_unreachable(address, tailscale_running)` is true when the primary address is a private IP and only the default route (or none) leads to it. That is the `decisions.md` case for skipping the fallback wait.
- **Private** means RFC 1918, link-local, IPv6 unique-local and link-local, plus Tailscale's 100.64.0.0/10 only while Tailscale is not running.
- Hostnames are never skipped: resolving them could itself take time, and many resolve to public addresses.

**Planning** (`plan()`), a pure function:

| Route | Tailscale | Attempts |
| --- | --- | --- |
| Automatic, with a Tailscale address | Running | Primary for the fallback wait, then Tailscale |
| Automatic, primary private and unroutable | Running | Tailscale only, with the note `PrimarySkipped` |
| Automatic | Not usable | Primary only, with the note `NoFallback(reason)` |
| Primary, or no Tailscale address | Any | Primary only |
| Tailscale | Running | Tailscale only; `PeerOffline` if Tailscale lists that device as offline (still tried) |
| Tailscale | Not usable | Error `TailscaleUnavailable(reason)`, so the pane says why instead of timing out |
| Override "Try Tailscale" on a Primary host | Running | Tailscale only. With no Tailscale address: `NoTailscaleAddress` |

The last attempt has no `give_up_after`; it uses the connect timeout (10 s).

**Live check** (`examples/net_check.rs`, release build). The owner switched Tailscale off and then on again from its tray icon.

| Address | Tailscale on | Tailscale off |
| --- | --- | --- |
| Status | Running, 15 peers, in 0.6–1.2 ms | `NotConnected { "Stopped" }`, in 0.5 ms, "…switched off. Connect it, then try again." |
| 127.0.0.1 | Specific /32, waits | Same |
| A private address on this PC's own network | Specific /24, waits | Same |
| 10.255.255.1, 172.31.255.1 (private, not local) | Default route only, **skips** | Same |
| fd00::1 | No route, **skips** | Same |
| 100.100.100.100 (Tailscale's service address) | Specific /32, waits | Default route only, **skips** |
| 100.127.255.254 (tailnet range, no device) | Default route only, waits: not private while Tailscale runs | **Skips** |
| 192.0.2.1, 8.8.8.8 (public) | Default route only, waits | Same |
| This PC's own Tailscale address | Specific /32 | Not listed: no status while off |

- Each route check took under 0.15 ms, so it can run before every connect.
- Tailscale installs a /32 route per device, not one for the whole 100.64.0.0/10. So a 100.x address with no device behind it has only the default route even while Tailscale runs. The plan still tries it under the Tailscale route, and `peer()` can say no such device is known.

fmt, clippy, and the unit tests (28, 13, 12, 7, 13 and 16) pass.

## Milestone 4, phase 4: Keychain (25 Nov 2026)

**The page** (`workspace/keychain.rs`, Keychain.dc.html), a new side-navigation entry between Hosts and Known hosts:
- **Header:** "Keychain" with Import key file, Import from ~/.ssh and Import from WSL.
- **Keys table:**
  - Name with its fingerprint underneath.
  - Type: "Ed25519", "ECDSA P-256", "RSA 4096" or "RSA 3072", with RSA's size read from the public key.
  - Protected by: "No passphrase" in `warn`; "Passphrase"; "Passphrase, saved in vault"; or "Passphrase + Windows Hello" once Hello unlock is on, which is phase 8.
  - Hosts: the count from `key_users`, including hosts that use the key through an identity.
  - When there are no keys: "No keys yet".
- **Public key of the selected key:**
  - The full line with its comment, with Copy (which reads "Copied" for 2 s) and Install on host.
  - Where the key came from, and which hosts sign in with it.
  - Rename, and Delete key in `warn` outline.
- **Generate key panel** (320 px, always shown as drawn): Name, Type and Passphrase.
  - Type is drawn as a 2-column grid, so `Segmented` gained `.columns(n)`.
  - "Unlock with Windows Hello" saves the passphrase in the vault.
  - Generation runs off the UI thread and shows "Generating…". The comment is `holonet@<computer name>`, and the new key is selected.

**Imports** (`key_import.rs`, shared with the host form's "Import key file…"):
- **File:** the native picker. A PuTTY key with a passphrase opens "Enter the passphrase for …"; a wrong one says "The passphrase did not unlock this key."
- **~/.ssh:** `%USERPROFILE%\.ssh`.
- **WSL:**
  - `wsl.exe -l -q` lists the distributions (its output is UTF-16), leaving out Docker's. Each one's `$HOME` is asked for, and `\wsl.localhost\<distro><home>\.ssh` is read.
  - The source is recorded as "Ubuntu-26.04: ~/.ssh/<name>".
- **The list dialog:**
  - `.pub`, `known_hosts`, `authorized_keys` and `config` files are skipped, and so are files that are not keys.
  - Keys already in the vault are shown, unticked.
  - PuTTY keys with a passphrase are shown but point to Import key file, which can ask for it.
  - Importable keys start ticked.
- **Rules:** keys already in the vault (same public key) are never added twice. PuTTY keys are stored converted to OpenSSH, keeping their passphrase. Files are only read.
- **Fixed:** the host form's import used to store a `.ppk` file as it was; it now converts it. A PuTTY key with a passphrase is sent to the Keychain.

**Install on host:**
- A dialog picks a host and signs in the way that host is set up (its password or key, or its identity's).
- When the host's password or key passphrase isn't saved, a field asks for it.
- Only a host key already trusted in Known hosts is accepted. A new or changed key stops before signing in, with "Connect to it once from Hosts" or "Check it in Known hosts".
- The result shows in the dialog:
  - "Installed m4-test-ed25519 on wsl-a. To sign in with it, edit wsl-a and choose this key."
  - Or "… already accepts …", or the server's error.
- For now it connects straight to the primary address. Routes and jump hosts join it in phases 6 and 7.

**Host form fixes found while testing:**
- **Passphrase cleared by accident.** Editing a host whose "Save passphrase" box started unticked would have cleared the key's saved passphrase on save, even when nothing was touched. That came from moving the passphrase to the key in phase 1. Now the form shows "This key's passphrase is saved in the vault, so it unlocks without asking." with **Forget saved passphrase**. It clears the passphrase only when you choose that.
- **Fields stretching in a full-height column.** `field()` has `flex_1`, so in the Generate panel the fields grew to share the panel's height. The panel's fields are `flex_none`.

**Checked against WSL** (a copy of the clean-profile test vault):

| Check | Result |
| --- | --- |
| Generate Ed25519 "m4-test-ed25519" with a passphrase, Hello box ticked | Listed, selected, "Passphrase, saved in vault", Hosts 0 |
| Install on host → wsl-a (saved password) | "Installed m4-test-ed25519 on wsl-a. To sign in with it, edit wsl-a and choose this key." |
| Edit wsl-b → SSH key → m4-test-ed25519 → Save and connect | Connected; the status bar shows "Key m4-test-ed25519", with no prompt (the passphrase saved on the key) |
| Keychain afterwards | Hosts 1, "Signs in to wsl-b." |
| Import from ~/.ssh (a throwaway key, with the owner's approval) | Listed ticked; imported, with the full path as its source |
| Import from WSL | Ubuntu-26.04 listed the throwaway ECDSA key and the owner's own `id_ed25519`, both ticked. The owner's key was unticked; only the throwaway one was imported ("No passphrase" in `warn`) |
| Import key file: a PuTTY RSA key with a passphrase | Passphrase dialog. A wrong passphrase was refused; the right one imported "RSA 3072 · Passphrase" |
| Rename | putty-rsa-v2-enc-1.ppk → putty-rsa-3072 |
| Delete m4-test-ed25519 | "wsl-b signs in with it, and will need another key or a password. Servers that accept it are not changed." Deleted |
| Copy | "Copied" for 2 s; the clipboard held the public key line |

**Afterwards:**
- Both throwaway keys were deleted, along with the Windows `.ssh` folder created for the test. The owner's WSL `~/.ssh` is back to its original five files.
- The installed test key was removed from the test server's `authorized_keys`.

**Not done:**
- The gallery has not gained the new pieces: the grid `Segmented`, the keys table and the pick list. Recorded for the wrap-up.
- Workspace notices stay until dismissed, so an old "Imported …" message can sit above later actions, as on other pages.

fmt, clippy, and the unit tests (32 in the app) pass.

## Milestone 4, phase 5: Identities (25 Nov 2026)

An identity is a name, a username and a sign-in (a key, or a password that is saved or asked on connect), shared by several hosts.

**Keychain:**
- The Identities section sits between Keys and the public key, as drawn.
- **Cards:**
  - the name
  - the sign-in: "Key work-ed25519", "Password, stored in vault" or "Password, asked on connect"
  - a faint line, "User deploy · 2 hosts"
- A dashed **New identity** card is always last. With no identities, a line explains what they are.
- **The dialog (undrawn):** Name, Username, and Authentication (SSH key with a key list, or Password with "Save in the encrypted vault"). It shows Add identity or Save identity, and Delete identity when editing. Each field is required, with a message.
- **Delete identity** names the hosts that use it: "wsl-a and wsl-b use it. They keep its username and sign-in as their own, so they still connect." `VaultData::delete_identity` copies them onto each host.
- Deleting a key now also names the identities that use it.

**Host form:** Authentication offers SSH key, Password and **Identity**, the last once the vault has one. It takes the place the design gives "SSH agent", which comes in milestone 6.
- With Identity chosen, the Username field is hidden. An identity list and a line take its place: "Signs in as holotest with a password stored in the vault. Edit the identity in the Keychain; every host that uses it changes too."
- The summary shows "Identity holotest-shared", and the equivalent command uses the identity's username.
- The host's own sign-in is kept underneath, unchanged.

**Using it everywhere** (`VaultData::resolved`):
- Panes, the host cards and list, the detail panel and the new-tab picker all work on the host with its identity's username and sign-in.
- The detail panel says "holotest-shared (Password)".
- A password typed at a prompt and saved goes to the identity (`sign_in_auth_mut`), so every host using it gets it.
- Install on host already used `sign_in` (phase 4).

**Checked against WSL** (the phase 4 test vault):

| Check | Result |
| --- | --- |
| New identity "holotest-shared": holotest, password saved | Card: "Password, stored in vault · User holotest · no hosts" |
| wsl-a → Identity; wsl-b → Identity, Save and connect | wsl-b connected with the identity's password. The card reads "2 hosts" |
| Edit the identity's username to `nobody-test` | Both host cards show `nobody-test@127.0.0.1:2222`. Connecting wsl-a offered a password for nobody-test and was refused |
| Username back to holotest | wsl-a connected |
| Delete the identity | Both hosts show holotest and Password as their own, with nothing missing |

**Fixed on the way:**
- The Hosts detail panel used the stored host rather than the resolved one, so it showed the host's old username and "Missing key" from wsl-b's deleted key. It now uses the resolved host.
- `HostForm::new` takes the vault data instead of separate lists. Clippy's argument limit prompted it.

**Noted, older behaviour:** a host whose key was deleted opens in the form with the first key in the vault selected, without saying its own key is gone. Saving would quietly switch it to that key. Recorded for the wrap-up.

fmt, clippy, and the unit tests (32, 14, 12, 7, 13 and 16) pass.

## Milestone 4, phase 6: routes (25 Nov 2026)

**Owner decisions:**
1. Test the fallback through a **test relay**: `crates/holonet-net/examples/ts_relay.rs` listens on this PC's own Tailscale IPv4 address and forwards to the WSL server. No firewall prompt appeared.
2. **A host's addresses share trust when the key matches.**
   - With no Known hosts entry for the address connected to, the entry for the same host's other address on that port is used.
   - The same key is trusted, and recorded for this address too. The log reads "Host key matches the one trusted for 127.0.0.1; trusted for this address too".
   - A different key stops the connection as a changed key, with the mismatch recorded on the entry it was compared with.

**Panes follow the route plan** (`holonet_net::plan` before each attempt and reconnect):
- The connection task asks Tailscale for its status, checks whether the primary address has a route, plans, and tries each address in turn.
- The fallback wait limits only the TCP connection, so a prompt can take as long as it needs.
- An unreachable address moves on to the next one and logs why ("No answer from 192.0.2.1 after 3 s"). Any other failure, such as a refused sign-in, stops there.
- **Connecting card**, as drawn:
  - "Trying the primary address. Tailscale is next if it does not answer."
  - "Trying 100.x over Tailscale."
  - "… is not reachable from this network, so Holonet is using Tailscale (100.x)."
  - "The primary address did not answer. Trying Tailscale (100.x)."
- **Status bar:** "Connected over Tailscale". The session header shows the address actually used.
- **Timed out**, as drawn:
  - "… This host also has a Tailscale address." with **Try Tailscale (100.x)**, which overrides the route for this connection, and Retry.
  - If Tailscale is off: "…, but Tailscale on this PC is switched off. Connect it, then try again.", with no Try Tailscale button.
  - If both addresses failed: "Neither … nor its Tailscale address … answered."
- **Tailscale off** (a new failure, "Tailscale off") for a host that must use Tailscale: "ts-route connects over Tailscale", "Tailscale on this PC is switched off. Connect it, then try again. Or edit the host to connect another way.", with Retry and Edit host.
- **Reconnecting** says "over Tailscale" when the last route was Tailscale. A host that needs Tailscale while it is off **waits instead of using up attempts**: "Waiting for Tailscale on this PC." It checks every 16 s, and Tailscale's own network change starts the next attempt at once.

**Screens:**
- **Host form:** "Tailscale address (optional)", checked like the primary address. Below it, "Connect using" (Automatic, Primary, Tailscale) with the drawn help text, which names the Settings fallback wait. Without a Tailscale address only the help line shows ("Add a Tailscale address…"). The summary gains Tailscale address and Connect using, and the equivalent command uses the Tailscale address under the Tailscale route.
- **Hosts:**
  - Cards carry the TAILSCALE tag.
  - The detail panel shows "Connect using" (saved per host as soon as it changes) and the Tailscale address row.
  - Its Host key section finds trust under either address.
- **Settings, Connection:** "Tailscale fallback wait" (2, 3, 5 or 10 seconds).

**Checked live** (the phase 4 test vault, four seeded hosts from `examples/m4_routes_seed.rs`, relay running):

| Host | Setup | Result |
| --- | --- | --- |
| ts-route | Primary 127.0.0.1 (trusted), route Tailscale | Connected over Tailscale with no key prompt; trust carried over from 127.0.0.1 |
| ts-fallback | Primary 192.0.2.1 (public, never answers), Automatic | "Trying the primary address. Tailscale is next…", then connected over Tailscale after the 3 s wait |
| ts-skip | Primary 10.255.255.1 (private, no route), Automatic | Connected over Tailscale in under 1.2 s, with no wait |
| ts-primary | Primary 192.0.2.1, route Primary | Timed out after 10 s with "Try Tailscale (100.x)", which connected over Tailscale |
| Tailscale off (the owner switched it off), new ts-route tab | | "Tailscale off" state, as above |
| Tailscale off, new ts-fallback tab | | Timed out on the primary; the message says Tailscale is switched off |
| Tailscale off under four open Tailscale sessions | First build | **They used up all 5 attempts within seconds and gave up**: Tailscale switching off is itself a network change, which starts attempts at once. Fixed by waiting for Tailscale |
| Tailscale off and on again with the fix | | ts-route showed "Waiting for Tailscale on this PC." and reconnected by itself when Tailscale came back. ts-fallback, under Automatic, spent attempts on its unreachable primary, then reconnected over Tailscale within its 5 |

**Fixed on the way:**
- The session header showed the primary address between attempts. It now shows the address the route starts with.
- The reconnect banner lost "over Tailscale" between attempts.

**Not checked:**
- The Settings "Tailscale fallback wait" row was not screenshotted. Its select saves like the keep-alive select next to it.
- Once, Ctrl Shift T from a pane in the "Tailscale off" state did not open the host picker; "+" did. Not reproduced or investigated yet; recorded for the wrap-up.
- An Automatic host whose primary address is truly down while Tailscale is off still uses its attempts. That is reasonable, since the primary address may come back, but it differs from a Tailscale-only host.

The relay and seeder stay in the tree with the other milestone 4 test programs until the wrap-up.

fmt, clippy, and the unit tests (32, 15, 12, 7, 13 and 16) pass.

## Milestone 4, phase 7: jump hosts in panes (25 Nov 2026)

**Shared connection code** (`crates/holonet-app/src/hops.rs`), used by panes and Install on host:
- `Hop` holds a host with its identity applied, its key and its keep-alive.
- `jumps()` returns the jump hosts in front of a host, first hop first.
- `link()` nests each hop's `ConnectOptions` as the next one's jump host.
- `connect_by_route()` runs the route plan for the **first hop**, the only one this PC connects to.
  - Hosts further along are reached at their primary address, or at their Tailscale address when their route is Tailscale. They get no fallback.
  - Only the first hop's own failure to answer moves on to its next address.

**Panes:**
- **Per hop:** each hop has its own prompts, sign-in state (ask again, typed and remembered secrets, pending save) and host key check, with shared trust across that host's own addresses.
  - Log lines are prefixed with the hop's name ("jump-a: Host key verified against Known hosts").
  - Prompts name the hop:
    - "This is the first time Holonet has connected to the jump host jump-a."
    - "jump-a is the jump host for behind-a. It has no saved password."
  - A typed secret saved from a jump host's prompt goes to that host's (or its identity's) entry.
- **Failures** belong to the hop that failed: each jump host wraps an error once, so the depth gives the hop.
  - Titles name it: "Holonet could not reach the jump host jump-a", "The jump host jump-a refused the key …", "jump-a could not reach behind-a".
  - "Edit host" becomes "Edit jump-a" and opens that host. "Save and connect" there retries the pane it came from, because the pane checks whether it uses that host.
  - "Try Tailscale" applies only to a first-hop failure.
  - A deleted jump host or a loop fails before connecting, with "Edit the host to choose another jump host, or None."
- **Display:**
  - The connecting card reads "Connecting to jump-a, then behind-a." ("jump-a over Tailscale" when it applies).
  - The header keeps the host's own address.
  - The status bar adds "Through jump-a".
- **Reconnect** restarts the whole chain from the first hop:
  - A lost jump host reads "The connection to the jump host jump-a was lost."
  - A failed attempt's error line names the hop ("jump-a: The host refused the connection…").

**Screens:**
- **Host form, Connection:**
  - A "Jump host" select: None, then the other saved hosts by name.
  - Hosts that already go through this one are left out, so a loop cannot be chosen.
  - The summary gains "Jump host", and the equivalent command gains `-J jump-a`.
  - With a jump host set, **Test connection tests the first hop** and says so ("This tested the jump host jump-a. The host itself is checked when you connect through it."), because this PC usually cannot reach the host directly.
- **Host detail:** a "Jump host" row ("None", the name, or "Deleted host").
- **Install on host** connects the way panes do, through the jump hosts and the route plan. Every hop's host key must already be trusted (under either of its addresses).
  - A failure names the hop.
  - A jump host whose password or passphrase is not saved stops with "Connect to … once from Hosts and save it".

**Checked live** (the phase 4 test vault; `examples/m4_jump_seed.rs` added jump-a at 127.0.0.1:2222, and behind-a at localhost:2222 through jump-a, and cleared Known hosts; from WSL, localhost is the same server under another Known hosts name):

| Check | Result |
| --- | --- |
| Connect behind-a from a clean Known hosts | Prompt for the jump host jump-a (127.0.0.1:2222), trusted. Prompt for behind-a (localhost:2222), trusted. Shell; status bar "Through jump-a" |
| `kill -9` the jump host's `sshd-session` (the one without a terminal) | Reconnected through both hops within a second, with no prompts; "-- Reconnected at 21:04 …" in scrollback |
| Same with sshd's listener stopped, then started | "The connection to the jump host jump-a was lost. Attempt 1 of 5 failed. Trying again in 2 s.", "The host refused the connection…"; reconnected by itself once the listener was back |
| jump-a's port set to 2223 | "Holonet could not reach the jump host jump-a" with Retry, **Edit jump-a**, Close tab. Edit jump-a, port back to 2222, Save and connect: the same pane connected |
| Host form for behind-a | Command `ssh -J jump-a holotest@localhost -p 2222`; summary "Jump host jump-a"; Test connection tested jump-a. jump-a's own select leaves out behind-a |
| jump-a's saved password removed, new behind-a tab | "Enter the password for holotest@127.0.0.1", naming jump-a as the jump host for behind-a. Typed with Save ticked: connected, and jump-a shows "Password, saved" again |
| Install on host: holonet-test-import on behind-a | "Installed holonet-test-import on behind-a." sshd logged two password sign-ins back to back (jump-a, then behind-a through it). The added line was removed from the test account's `authorized_keys` afterwards |

**Not checked:**
- The hop name on a reconnect attempt's error line was added after the live run. It is one line and is covered by clippy only.
- A jump host over Tailscale (the first hop's route plan) was not run live. It uses the same code as phase 6, with the first hop's host.
- Chains of more than one jump host are covered by the `link` unit test, not live.

fmt, clippy, and the unit tests (34, 15, 12, 7, 13 and 16) pass.

## Milestone 4, phase 8: lock and Windows Hello (25 Nov 2026)

**Windows Hello** (`crates/holonet-app/src/hello.rs`, the `windows` 0.62 calls from phase 0's spike):
- `create_and_sign` creates the credential, then signs through a freshly opened one, as unlocking does.
- `sign`, `delete`, `available`.
- `HelloError`: Cancelled, Missing, Unavailable, Failed. Each has interface text.
- Every call blocks on Windows, so callers run it on GPUI's background executor.
- Credential names are `holonet-vault-<12 hex>`, stored in the vault's Hello slot.

**Locking** (`app.rs`):
- **Idle:** the window's root captures key-down and mouse-down, and listens for mouse moves, recording the time. Only input in Holonet counts: terminal output does not.
  - A check every 10 s locks after Settings' "Lock vault after" (15 minutes, 1 hour, Never).
- **Windows lock:** `platform::watch_system` gains the message-only window from phase 0, on its own thread (`WTSRegisterSessionNotification`). It sends `SessionLocked` and `SessionUnlocked` into the same channel as sleep and network changes.
  - "Lock when Windows locks" (on by default) locks the vault on `SessionLocked`.
  - Panes ignore these events.
- Locking drops the workspace as "Lock now" does, so sessions stay connected.

**Locked screen** (Locked.dc.html):
- It reads the vault file's header (no password needed) for a Hello slot.
  - With one: **Unlock with Windows Hello** ("Waiting for Windows Hello…" while the prompt is open) and **Use vault password instead**.
  - The password view then offers "Use Windows Hello instead".
  - Without one: the password field, as before.
- The body names the reason:
  - "Holonet locked after 15 minutes idle." (or "1 hour", or "1 minute")
  - "Holonet locked when Windows locked."
  - "You locked the vault."
  - At startup: "Unlock to open your hosts, keys and passwords." (with Hello)
- Hello failures show under the button:
  - "Windows Hello was cancelled."
  - A missing credential, or a key that does not open the vault, both say to use the vault password and turn Hello on again.

**Settings, Vault and security:**
- **Unlock with Windows Hello:**
  - Disabled with "Windows Hello is not set up on this PC. Add a PIN, fingerprint or face in Windows Settings, Accounts, Sign-in options." when `IsSupportedAsync` says no. It reads "Checking…" while Windows answers.
  - **Turning it on** opens "Turn on Windows Hello", which asks for the vault password. Holonet checks the password (Argon2, off the UI thread, holding the vault as the password change does), has Hello create its key and sign a new challenge, and adds the slot. The file becomes format 2.
  - **Turning it off** removes the slot and deletes the credential. The file goes back to format 1.
- **Lock vault after** (select) and **Lock when Windows locks** (switch). The owner's plan kept these separate, where the design merges them into one select.
- The side navigation's vault card reads "Encrypted, unlocks with Windows Hello or the vault password" when the vault has a Hello slot.

**Fixed on the way: the Settings page layout.**
- Sections were sized to their content, not to `w_full` with `max_w(1080)`: Taffy did not apply them inside the scroll column.
  - So the Connection card was wider than the others (since phase 6's long help line).
  - Sections were measured too short: the Connection heading overlapped the Vault file row.
- `page_block` now takes a definite width (the window less the side navigation and padding, at most 1080), and the card and row wrappers are flex columns. All cards now share one width, with no overlap.

**Checked live** (the phase 4 test vault):

| Check | Result |
| --- | --- |
| Idle lock (set to 1 minute with `examples/m4_idle_seed.rs`), a session printing the time every 5 s | Locked about 60 s after the last keystroke with "Holonet locked after 1 minute idle." and "1 session is open…"; the output did not keep it unlocked. After the password unlock the loop had kept printing, with no reconnect |
| Turn on Windows Hello (the owner completed the prompt) | "Windows Hello now unlocks the vault. The vault password still works too."; the vault file became format 2 with the credential in its slot |
| Win L with a behind-a session open (the owner) | Holonet showed the Locked screen after Windows unlocked, and Hello opened it. The session was the same login, with no reconnect |
| New process, Hello at startup (the owner) | Cancelling showed the cancelled message; the second try opened the vault. **This confirms the signature repeats across processes** (phase 0 had checked one process only) |
| Lock now, then Use vault password instead | The password view with "Use Windows Hello instead"; the password unlocked |
| Turn off Windows Hello | "Windows Hello no longer unlocks the vault. Use the vault password."; file back to format 1, no Hello slot |

**Not checked:**
- Hello not set up on this PC (the disabled switch): this PC has Hello. The wording is in `hello::UNAVAILABLE`.
- A missing credential, or a key that does not open the vault: not run live.
- That the credential was deleted from Windows after turning Hello off: not confirmed directly. The vault no longer refers to it.
- The prompt focus workaround was not needed; every prompt opened in front.

fmt, clippy, and the unit tests (35, 15, 12, 7, 13 and 16) pass.

## Milestone 4, phase 9: wrap-up (25 Nov 2026)

**Removed:**
- The phase 0 spikes: `holonet-app` examples `m4_hello`, `m4_lock`, `m4_tailscale`, and `holonet-ssh` examples `m4_jump`, `m4_keygen`, `m4_ppk` (the `.ppk` fixtures and their tests cover what `m4_ppk` checked).
- The test seeders `m4_routes_seed`, `m4_jump_seed`, `m4_idle_seed`.
- `holonet-app`'s dev-dependencies, which only the spikes used.
- **Kept:** `holonet-net`'s `ts_relay` (the relay for testing routes) and `net_check`, both listed in `CLAUDE.md`.

**Fixed:**
- **A host whose key was deleted** opened in the form with the first key selected, and saving would quietly switch it to that key.
  - The Key select now starts on "Deleted key: choose another".
  - The field says "The key this host used was deleted. Choose another key, import one, or sign in with a password.", and saving is blocked until a key is chosen.
  - The summary says "Deleted key".
- **App shortcuts from a pane with nothing to focus** (connecting, a card, a failure): a pane now has its own focus handle on its root.
  - It takes focus in those states, and when its terminal stops being drawn while focused.
  - So Ctrl Shift T and the others dispatch from inside the pane. A focused terminal in a hidden tab, or one that is no longer drawn, could not dispatch them; this is the likely cause of phase 6's one-off "Ctrl Shift T did nothing".
  - After the fix: a connected wsl-a tab, then a ts-skip tab failing with "Not reachable", then Ctrl Shift T opened the host picker. The original failure was never reproduced, so this checks the fix, not the cause.

**Gallery:**
- The Keychain controls moved into `ui/lists.rs`, which the Keychain page uses too: `table` with `TableColumn`/`TableRow` and `name_cell`, `identity_card` and `add_card`, `pick_row`, and `side_panel`.
- `--gallery` has a Keychain section:
  - a three-row table, with the row clicked selected and a "No passphrase" row in `warn`
  - two identity cards and the dashed "New identity" card
  - the import list with a checked, an unchecked and a disabled row
  - the Generate key side panel

**Done check, from a clean profile** (a new throwaway vault, the WSL test server):

| Done item | Result |
| --- | --- |
| Generate an Ed25519 key, install it on a host, sign in with it | Generated `done-ed25519` with a passphrase saved in the vault. Added done-a (127.0.0.1:2222, password), connected once to trust its key, then Install on host: "Installed done-ed25519 on done-a." Switched done-a to the key: connected, status bar "Key done-ed25519". The line was removed from the test account's `authorized_keys` afterwards |
| Import keys from a file, `~/.ssh`, and WSL | File: `.dev/holotest_rsa` imported ("RSA 3072, No passphrase"). `~/.ssh`: the picker said "No private keys in …\.ssh" (this profile has none now; phase 4 imported from it). WSL: the picker listed the Ubuntu distribution's `id_ed25519`, and was cancelled, because that is the owner's real key |
| Tailscale fallback, and the pane says which route | done-ts (primary 192.0.2.1, this PC's Tailscale address through `ts_relay`, Automatic): "No answer from 192.0.2.1 after 3 s", "Opening TCP connection to …:2222 over Tailscale", then "Connected over Tailscale" |
| Through a jump host, both host keys checked | done-behind (localhost, jump host done-a): "done-a: Host key verified against Known hosts", then the prompt for done-behind's own key. Connected, "Through done-a" |
| The vault locks after the idle time and when Windows locks, and Hello unlocks it | Lock now with four sessions: "4 sessions are open…"; the password unlocked and the sessions were still connected. Idle lock, Windows lock and Hello unlock (also from a fresh start) were checked live in phase 8, with the owner, on this build's code |

fmt, clippy, the unit tests (35, 15, 12, 7, 13 and 16) and the WSL integration tests (13) pass.

**Milestone 4 is done.**

Carried forward, not tested:
- Hello on a PC without Hello set up (the disabled switch).
- A jump host reached over Tailscale.
- Chains of more than one jump host, live.
- An identity whose key was deleted opens its form with the first key selected. This is the same issue fixed above for hosts; recorded for milestone 5.

## Milestone 5, phase 0: spikes (25 Nov 2026)

The plan is `docs/milestone-5-plan.md`. **Owner decisions:** dropping files from Explorer, New folder and Refresh are in scope, and each SFTP tab has its own connection (reusing a terminal's typed secrets for "Open SFTP here").

The spike programs stay until phase 8: `crates/holonet-ssh/examples/m5_sftp.rs`, `crates/holonet-app/examples/m5_drag.rs`, `m5_local.rs`. `holonet-ssh` now depends on `russh-sftp` 4.0.0 with its `raw-string` feature, and `Session::open_sftp()` opens the subsystem (`request_subsystem(true, "sftp")`, then `SftpSession::new(channel.into_stream())`).

### (a) `russh-sftp` 4.0.0 against the WSL server
- **Version:** 4.0.0 is current. It is independent of russh (any stream), tested against russh 0.65.1, uses `jiff` 1.0 like Holonet, and pipelines 16 reads and 16 writes by default (`Config`).
- **Without `raw-string`,** paths and names are `String`s, so a non-UTF-8 name cannot round-trip. With it, `SftpString` holds the bytes.

| Check | Result |
| --- | --- |
| Open the subsystem | 40–300 ms (the first open after connecting is the slow one) |
| List the home folder | 1–3 ms; attributes carry size, mode (type and permission bits) and mtime. **No user or group names** (SFTP v3 attributes only have numeric ids, and `russh-sftp`'s attributes left them empty here) |
| 500 MB upload (`File`, 256 KB writes) | 1.8 s, **284 MB/s** (100 MB: 377 MB/s) |
| 500 MB download | 3.0 s, **167 MB/s** (100 MB: 114 MB/s); contents matched |
| Windows' OpenSSH `sftp` (`sftp.exe -b`), same 500 MB, same link | put 221 MB/s, get 63 MB/s (each including sign-in) |
| Resumed upload: a third, closed, reopened with `WRITE` only, seek to the remote size, the rest | Matched |
| Resumed download: half to a local file, then seek the remote file and append | Matched |
| `set_metadata` permissions 0o640 | 664 → 640 |
| Symbolic links | `symlink_metadata` says link; `metadata` follows (file or folder); `read_link` gives the target; `read_dir` reports links as links (not their targets' type), so the pane must `stat` a link to know whether it opens as a folder |
| **`symlink` argument order** | OpenSSH's server takes `SSH_FXP_SYMLINK`'s paths **reversed** (a long-known OpenSSH quirk). Spec order created `~/small.txt` and `~/sub` as links on the test server; they were removed. Passing the target first worked |
| `read_dir` of 5,000 files | 87 ms |
| Non-UTF-8 name (`caf\xe9.txt`, made by the shell) | Listed with its raw bytes, shown lossily as "caf�.txt", and `stat` by the raw bytes works |

Everything the spike made was removed from the server and this PC.

### (b) GPUI drag and drop on Windows
`m5_drag` shows two lists. Rows use `on_drag(value, preview)`, and the lists use `drag_over::<T>` for the outline and `on_drop::<T>`.

| Check | Result |
| --- | --- |
| Row dragged from one list to the other (real mouse input) | Dropped, both directions; the preview follows the pointer and the target list outlines while the row is over it |
| Row dropped back on its own list | Delivered, and ignored by the handler (the handler has to check the source) |
| **Files and a folder dragged from Explorer onto "Host" (the owner)** | `on_drop::<ExternalPaths>` received three full paths (the folder as one path), and the list outlined during the drag. Short 8.3 names arrive as long paths |
| Dragging out of the app to Explorer | Not tried: GPUI 0.2.2 has no drag source on Windows (no `DoDragDrop`) |

### (c) Recycle Bin
`SHFileOperationW` with `FO_DELETE | FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI` moved a file and a folder with a file inside to the Recycle Bin in 75 ms. Both left the folder, and both appeared in the Recycle Bin (`Shell.Application` namespace 10). The two test items were then removed from the Recycle Bin, and nothing else was touched. `windows-sys` gains `Win32_Storage_FileSystem` and `Win32_UI_Shell`.

### (d) Drives and hidden files
- `GetLogicalDrives` with `GetDriveTypeW` and `GetVolumeInformationW` listed `C:\` as fixed, "Local Disk", in 0.1 ms.
- The profile folder holds 28 shown entries and 19 with the hidden or system attribute (`AppData` and its old junctions among them).
- 16 entries start with a dot but have no hidden attribute (`.cargo`, `.ssh` and the like). **The local pane treats only the attribute as hidden** (Windows' own meaning), and the host's pane treats dot-names as hidden.

fmt, clippy and the unit tests pass.

## Milestone 5, phase 1: model and settings (25 Nov 2026)

- **`holonet-model`:**
  - `Host` gains `sftp_folder` ("Default SFTP folder"; `None` means the home folder) and `sftp_local_folder` (the folder on this PC last used for the host).
    - The plan had a separate map for the second. On the host it goes when the host is deleted, and duplicates with it.
  - `Settings` gains `sftp: SftpSettings`:
    - `on_conflict` (Ask, Overwrite, Keep both)
    - `at_once` (2, 4, 8, with `count()`)
    - `show_hidden`
    - The defaults are Ask, 2 and on, the values `Settings.dc.html` shows.
  - A test loads a milestone 4 vault (schema 2, no SFTP fields) with these defaults and round-trips it.
- **Host form, Connection:** "Default SFTP folder" (mono, placeholder "Home directory") beside Jump host, as `NewHost.dc.html` draws them. Empty saves `None`.
- **Settings:**
  - A new SFTP section with its own chip: "When a file already exists" and "Transfers at once" selects, and "Show hidden files".
  - The help lines say what each does; the hidden-files line says hidden means the attribute on this PC and dot-names on hosts.

**Checked live** (the phase 4 test vault):
- The section showed Ask, 2 and ticked.
- Changed to Keep both, 8 and unticked, and set behind-a's Default SFTP folder to `/tmp`.
- After restarting Holonet, all four had kept their new values.

fmt, clippy, and the unit tests (35, 16, 12, 7, 13 and 16) pass.

## Milestone 5, phase 2: file operations (25 Nov 2026)

**New crate `holonet-files`** (no window; depends on `holonet-ssh`):
- `Entry`:
  - name to show, and the raw name bytes
  - `Kind`: Folder, File, `Link { folder, broken }`, Other
  - size, modified, permission bits, uid, gid, hidden
  - `symbolic_mode()` gives `-rwxr-x---`
- `sort` puts folders and links to folders first, then names ignoring case.
- `RemotePath` is bytes: `join`, `parent`, `file_name`, and `components` for breadcrumbs. Its `Display` is lossy.
- `FileError` covers NotFound, PermissionDenied, AlreadyExists, Lost, TimedOut, and Other. Each has interface text, and each maps from `russh-sftp`'s status codes and from `std::io`.

**`remote::Remote`** (over `Session::open_sftp`):
- `home` (canonical `.`) and `canonicalize` (relative paths start at home)
- `list`: a link is `stat`ed to learn whether it opens as a folder or is broken
- `entry`, `exists`
- `rename` and `create_folder`: both refuse to replace something
- `remove`: recursive, never following links, returns the count
- `set_mode`: permission bits only
- `symlink` and `read_link`
- **Symlink order:** OpenSSH-style servers are recognised by the `statvfs@openssh.com` extension (`fs_info` returns `Some`), and get the target first.
- **Hidden** on a host means a leading dot.

**`local`:**
- `drives()` (with `Drive::display()`, as in "Local Disk (C:)") and `profile()`
- `list`, `entry`, `rename`, `create_folder`
- `recycle(paths)`: `SHFileOperationW` with undo, no Windows confirmation, and an error if anything was aborted
- **Hidden** on this PC means the hidden or system attribute.

**Tests:**
- **Unit (7):**
  - sorting
  - remote paths, including non-UTF-8 bytes and breadcrumbs
  - symbolic modes
  - local list, rename, create and AlreadyExists
  - hidden follows the attribute, not the dot
  - drives include `C:\`
  - Recycle Bin, gated by `HOLONET_TEST_RECYCLE=1` because it puts two items there. It ran once and passed, and its two items were then removed from the Recycle Bin.
- **WSL (5),** `cargo test -p holonet-files --test wsl`:
  - kinds, links to a folder, a file and nowhere, hidden files, sizes and modes
  - folder create, rename and AlreadyExists, and recursive remove (7 items, with a link to `/etc` removed and not followed)
  - `set_mode`, and a link made in the server's order, with the home folder checked for stray links (none)
  - a non-UTF-8 name listed by its bytes and renamed by them
  - NotFound, PermissionDenied (`/root`), and relative canonicalisation
  - All passed on the first run. Each test uses its own folder and removes it, and the test account's home folder was left as it was.

fmt, clippy, and the unit tests pass (`holonet-files` 7, the others as before).

## Milestone 5, phase 3: transfer engine (25 Nov 2026)

New modules in `holonet-files`, still without a window.

**`format`:** sizes, rates and time left as drawn ("940 B", "48.2 MB", "4.2 MB/s", "4 s left", "3 min left").

**`plan`:** `plan_upload` and `plan_download` expand the selected items into the folders to create and the files to copy.
- **Conflicts** are the selected items whose names already exist at the destination; `conflicts()` lists them for the popup. `resolve(choice)` applies the choice to all of them:
  - **Overwrite** replaces files and merges folders.
  - **Keep both** takes the first free name: "index (2).html", "site (1)", ".bashrc (1)".
  - **Skip** drops those items and lists them as skipped.
- **Links:** a link to a file is copied as the file; a link to a folder is skipped with its reason. There are no loops, and Windows cannot make the link anyway.
- **Names Windows cannot use** are skipped on download with their reason: reserved characters, a trailing dot or space, or `CON`/`COM1`-style names.
- `create_folders` makes them parents first, reusing existing ones. `jobs()` gives the files to queue.

**`transfer`:** `upload` and `download` copy one file in 256 KB chunks, checking a pause or cancel flag between chunks.
- **Downloads** write to `name.holonet-part` and rename it over the target when complete.
- **Resume:**
  - A resumed download appends to its part file.
  - A resumed upload continues from the host's size.
  - Either happens only if the source still matches the `Stamp` (size and modification time) taken when the copy first started. Otherwise it fails with "The source changed since the transfer started."
- **At the end:** the sizes must match, and the modification time is carried over.
- **Telling a refusal from a lost connection:** SFTP file handles report both as plain I/O errors. `classify` asks the server for `realpath(".")` with a 5 s limit. No answer means the connection went; an answer means a refusal, shown with the server's message.

**`engine`:**
- `Queue` is the plain state:
  - start order up to the limit
  - the summary ("2 active · 1 paused · 2 queued", "1 waiting to reconnect · 1 failed", "Idle")
  - a rate smoothed over samples at least 0.25 s apart, and "64% · 4.2 MB/s · 4 s left"
- `Engine` runs it on Tokio:
  - `add`, `pause`, `resume`, `retry` (from nothing), `remove` (cancels an active copy and deletes its part file), `clear_finished`, `set_at_once`
  - `set_remote`; `snapshot` and `summary` for the interface
  - events: `Changed`, at most every 100 ms per active copy, and `ConnectionLost`
- **A copy that loses the connection is Interrupted, not failed.** The engine then stops starting copies until `set_remote` gives it a new connection, which queues interrupted copies again.

**Tests:**
- **Unit (8 new, 15 in the crate):** formatting, Keep both names, Windows-unsafe names, conflict resolution paths, queue order and summary, the rate and progress text, part paths.
- **WSL** (`cargo test -p holonet-files --release --test transfer`, 3 tests):

| Test | Result |
| --- | --- |
| A folder (with an empty folder and a nested one) and a file uploaded where both already exist | Conflicts listed; Keep both made `site (1)` and `notes (1).txt`; Overwrite merged `site` (kept `old.txt`) and replaced `notes.txt`; Skip sent nothing and listed two skipped; the folder downloaded back matched byte for byte, empty folder included, no part file left |
| Pause and resume a 192 MB upload | Paused part-way ("1 paused"), resumed, and matched |
| Change the source while paused | Failed with "The source changed since the transfer started."; Retry started over and matched the new contents |
| **Kill the connection mid-download** (`kill -9` the connection's own sshd process from a second connection) | Interrupted, `ConnectionLost`, "1 waiting to reconnect". The part file kept the bytes so far; handed the second connection, the copy resumed from exactly those bytes and matched. Three runs killed it at 26, 40 and 53 MB of 96 MB |

- **A race in the first version of the pause test:** a 64 MB upload finishes in about 0.2 s, so it sometimes completed before the pause landed. Two runs failed this way when the tests ran in parallel. The files are now larger (192 MB and 128 MB), the pause comes at the first progress report, and three full runs then passed. The failed runs left two temp folders here, which were removed. The server's home folder was clean after every run.

fmt, clippy, and the unit tests (`holonet-files` 15, the others as before) pass.

## Milestone 5, phase 4: the SFTP tab (25 Nov 2026)

An SFTP tab is a tab with one pane in Files mode. It reuses the terminal pane's whole connection path: routes, jump hosts, host key checks, sign-in prompts, the failed states and reconnect.

**`session.rs`:**
- `Pane::new` takes a `PaneMode` (Terminal or Files) and the remembered secrets.
- After sign-in, a Files pane opens SFTP (`Remote::open`) instead of a shell.
- The first connection makes a `FilesView`. Reconnects hand the same view the new `Remote`, so both sides keep their folders.
- **There is no shell to report a dropped connection,** so `watch_lost` waits on the new `Session::closed()` in `holonet-ssh`. That polls `is_closed()` every 250 ms and returns the lost reason, or `None` for a close on purpose. On a loss, the view drops its `Remote` and the pane runs the usual `on_lost` and reconnect.
- A Files pane shows "Connected" where a terminal shows its size, and "SFTP" in the status bar in place of the terminal type.

**`app.rs` and `tab.rs`:**
- Tab creation moved into `open_tab(id, (mode, remembered))`. `connect` opens a terminal; `open_files` opens SFTP.
- `Tab::is_files` gives the title bar its folder icon (`TabKind::Sftp`).
- It hides Split and Find, and Split, Find and their shortcuts do nothing in an SFTP tab.

**Entry points:**
- The host detail has "Open SFTP" under Connect.
- A terminal tab's header has "Open SFTP here". It opens the host the active pane shows, with that pane's typed secrets for each hop, so nothing is asked again.
- In the host picker, each row has an SFTP button, and Ctrl Enter opens the highlighted host. A hint under the list says so.

**`files_view.rs`** (phase 4 version):
- Two panes: "This PC" and the host.
  - This PC starts in the host's last local folder or the user profile.
  - The host starts in its "Default SFTP folder" (through `realpath`, falling back to home if it cannot be used) or home.
- Each pane has a header and plain rows with an icon, a name and a size.
- `entry_icon` picks the icon for each kind and extension. Seven new file icons from the same Lucide set: file, link, archive, code, text, config and lock.
- Browsing, selection and dragging come in phase 5.

**Check (WSL server, release build):**

| Test | Result |
| --- | --- |
| Open SFTP from behind-a's detail (through jump-a, Default SFTP folder `/tmp`) | Connected; status bar "Through jump-a … SFTP"; the host side listed `/tmp` |
| Drop the connection: `kill -9` both hops' sshd session processes | Reconnected by itself within about a second, with new server processes; still in `/tmp` |
| "Open SFTP here" from a wsl-a terminal | A second tab with the folder icon opened without asking for the password; home listed |
| Picker: Ctrl Shift T, type "wsl-b", Ctrl Enter | An SFTP tab for wsl-b |

- **Killing only the `[priv]` sshd processes** left the unprivileged session processes running, and the connection stayed up. Killing the `holotest` ones dropped it.
- **The reconnecting state was too brief to capture** here; it is the milestone 3 view with the files view under it.

fmt, clippy and all workspace tests pass.

## Milestone 5, phase 5: file panes (25 Nov 2026)

`files_view.rs` now draws Sftp.dc.html's two panes and a first transfers strip.

**Each side:**
- **Places:**
  - "This PC" lists the drives; opening one goes to its root.
  - A folder on this PC, or a folder on the host.
- **Header and navigation:**
  - The header has the title and a Filter box. `TextField` gained `compact()` (36 px) and `leading_icon()` for it.
  - Back, Up and Refresh. Refresh is in phase 7's list, but costs nothing here.
  - Breadcrumbs. A long path keeps the first crumb and the last three; "…" opens the folder before those.
  - The Bookmarks button drawn there has no milestone yet, so it is left out.
- **Columns:** Name, Size, Modified ("24 Nov 2026 16:02" here, "24 Nov 2026" on the host), and Permissions on the host. Folders come first.
- **The list:** a `uniform_list`, so only visible rows are drawn. Listings load off the UI thread: this PC through `spawn_blocking`, the host through SFTP. An older answer is dropped when a newer load has started.
- **States:**
  - Loading.
  - The server's message in amber ("Permission denied.").
  - "This folder is empty."
  - "Nothing here matches "…"".
  - A folder of only hidden files says so and points at Settings.
- **Hidden files:** Settings' switch applies to both sides, including open tabs: `HolonetApp::apply_sftp_settings` reaches each pane, as terminal settings already do. Transfers at once applies the same way.
- **Links:** a link to a folder opens like a folder (violet folder icon). Other links show the link icon.
- **The local folder** is saved on the host (`sftp_local_folder`) when it changes, so the next SFTP tab for that host starts there. Checked across app restarts.

**Selection:**
- A click toggles a row, Shift click adds the range from the last click, and the header box selects or clears every shown row. A filter drops selected rows it hides.
- The action bar shows the count, then Upload → or ← Download to the other side's current folder, Copy path (one path per line), and Clear.
- Rename, Permissions and Delete come in phase 7, so they are not shown yet.
- **Double click opens the row the first click hit.** The first click selects the row, the action bar appears, and the rows move down by its height. Using the row now under the pointer, a test double click on `holonet-p5` opened `.ssh`, the row above. The pane's mouse-down handler now opens the remembered row instead.

**Drag:**
- A row drags the selection when it is part of it, otherwise itself. The chip under the pointer shows the name or "n items".
- **Drop targets:**
  - Dropping on the other pane uses its current folder.
  - Dropping on a folder row uses that folder; the row highlights.
  - The host's pane also takes `ExternalPaths` from Explorer.
- A target outlines in dashed accent, and "Drop to upload into /…" floats over the bottom of the pane. The first version put that banner above the list, which moved the rows mid-drag, so a drop meant for a folder row landed on the pane.
- **Transfers:** plan (`plan_upload` or `plan_download`), apply Settings' conflict choice, create folders, queue on the tab's `Engine`.
  - Until phase 6's conflict popup, "Ask" skips names that already exist.
  - Skipped items and failures show in the transfers strip ("Skipped 2 items, such as link-to-folder: Links to folders are not copied.").
  - Both panes refresh 0.4 s after transfers finish.
- **The transfers strip** shows "Transfers", the engine's summary, or the drawn empty text. Phase 6 adds the rows and their controls.

**Reconnecting:** an SFTP tab's reconnect footnote now reads "Both folders are kept. Transfers continue after reconnecting." The host side keeps its listing, and Refresh and drops are disabled until the connection is back.

**Check (WSL server, release build, throwaway folders `%LOCALAPPDATA%\Temp\holonet-p5` and `~/holonet-p5`, both removed afterwards):**

| Test | Result |
| --- | --- |
| Browse with the filter and double clicks to the test folder, with hidden files switched on in Settings while the tab was open | Both sides followed the switch; AppData, then Local, Temp and the folder opened |
| Restart the app and open SFTP again | This PC started in the test folder |
| Drag a folder (with a subfolder) and a file to the host's pane | Banner and outline shown; `site/`, `site/css/a.css`, `site/index.html` and `deploy.sh` arrived; the host's list refreshed |
| Drag a host file onto the local `site` folder row | Landed in `site\notes.txt` |
| A name that exists, with Settings on Keep both | `notes (1).txt` on the host |
| ← Download of six items: an empty folder, a folder link, a broken link, a folder and two files that exist here | `existing\` (empty), `site (1)\` with its contents, `deploy (1).sh` and `notes (1).txt`; "Skipped 2 items, such as link-to-folder: …" |
| Open `link-to-folder` on the host, then Back | Entered the link's folder, then returned |
| Shift click from the first row to the last, then Copy path | Six rows selected, the header box checked, and the clipboard held the six host paths |
| "This PC" crumb | Local Disk (C:) listed; Up disabled |
| Kill the connection's server processes | Reconnecting banner over both panes with their folders kept; reconnected in about a second |
| A file dragged from another app (a WinForms `DoDragDrop` with a file list, Explorer's format) onto the host's pane | Dashed outline while over it; the file arrived on the host |

fmt, clippy and all workspace tests pass.

## Milestone 5, phase 6: queue panel and conflicts (25 Nov 2026)

**The queue panel** (Sftp.dc.html, TransferRow.md) replaces phase 5's strip.
- **Header:** "Transfers", the engine's summary ("1 active", "1 paused", "1 waiting to reconnect", "1 failed", "Idle"), the last note, and "Show finished (n)" / "Hide finished (n)" once anything has finished.
- **Rows** are a `uniform_list`, so a folder of thousands of files stays smooth. Up to 4.5 rows show before the list scrolls. Each row has the name and route ("This PC → wsl-a"), a 4 px bar and its text, and a remove button.
- **Each state:**
  - Active: accent bar for uploads, violet for downloads; "64% · 4.2 MB/s · 4 s left"; Pause.
  - Paused: "Paused at n%"; Resume.
  - Queued: "Queued".
  - Interrupted: "Waiting to reconnect at n%". It resumes by itself when the tab reconnects.
  - Failed: amber bar and "Failed: …"; Retry.
  - Finished: "Uploaded to wsl-a · 3.0 GB" or "Downloaded to This PC · 9 B".
- The view keeps a snapshot of the engine's jobs, taken on each engine event, so drawing does not lock the queue.

**Conflicts:**
- A transfer is planned first. If any top-level names already exist and Settings says Ask, the plan waits in the conflict popup.
- **The popup:** "mixed already exists" or "n items already exist", "Uploading to /… on wsl-a." or "Downloading to this PC.", the list of names, then Overwrite, Keep both, Skip and Cancel all, and the drawn help line. The backdrop cancels.
- **Overwrite and Keep both in Settings** go straight to the queue, as phase 5 did.
- The popup is placed inside the SFTP tab, so other tabs stay usable.
- **Layout:** the card needs a definite width (460 px). With `w_full` inside the modal, Taffy sized the wrapped text and the `flex_1` buttons from their content: the note ran out of the card and the buttons overlapped. This is milestone 4's Settings lesson again. The design's wrapping row of four buttons became three in a row with Cancel all under them.

**Check (WSL server, release build, Settings set to Ask; throwaway folders `%LOCALAPPDATA%\Temp\holonet-p6` and `~/holonet-p6`, removed afterwards):**
- **The test folder:** `mixed/` had five small files, `sub/deep.txt` and a 3 GB `big.bin`.
- **The host's copy:** an older `small1.txt` and a `remote-only.txt`.

| Test | Result |
| --- | --- |
| Drag `mixed` to the host | Popup "mixed already exists", "Uploading to /home/holotest/holonet-p6 on wsl-a."; Overwrite merged: `small1.txt` replaced, `remote-only.txt` kept, `sub/` and the rest added. big.bin went at about 450 MB/s and finished before Pause could be clicked |
| Drag the host's `mixed` back to this PC | Popup "Downloading to this PC."; Keep both made `mixed (1)` with all nine items |
| Pause during that download | "Paused at 10%"; Resume continued (about 210–350 MB/s) |
| Kill the connection's server processes mid-download | "1 waiting to reconnect", "Waiting to reconnect at 30%"; the tab reconnected and the copy carried on from there to the end. 3,221,225,472 bytes and no part file left |
| Show finished | Fifteen finished rows with direction and size; Hide finished folds them |
| Pause a download, append to the host's file, Resume | "Failed: The source changed since the transfer started." in amber; Retry started over and fetched 3,221,225,480 bytes |

**Still to come:** closing an SFTP tab with transfers still running should ask "n transfers are still running. Close anyway?" (milestone 5 plan, "States to design"). It is added to phase 7.

fmt, clippy and all workspace tests pass.
