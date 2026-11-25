# Milestone 3: Sessions, working plan

Written 25 November 2026, after milestone 2 (`docs/milestone-2-plan.md`, results in `docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **tabs, split panes, the connecting, failed and reconnecting states, and the terminal settings including the bundled Nerd Fonts.** It also takes what milestone 2 left over: find in the terminal, automatic reconnect, and a dedicated key-refused state.

Done means:
- You can open several tabs from the new-tab host picker.
- You can split one tab into nested panes, including a second host, and resize them by dragging.
- You can find text in a pane's scrollback.
- Changing the font, size or color scheme in Settings shows on every open terminal at once.
- When a connection drops, the pane reconnects by itself, keeps its scrollback, and says what happened.

## Scope

**In:**
- **Tabs:**
  - the drawn tab strip with state dots (solid when connected, a ring while connecting, `warn` when failed), scrolling sideways when full
  - "+" and Ctrl Shift T open a quick host picker
  - close a tab, next and previous tab
- **Split panes:** Split right and Split down, nested, up to 4 panes per tab.
  - Drag the gap between panes to resize.
  - The active pane has a stronger border.
  - Each pane has its own header (host, group, `shell · cols×rows`).
  - Ctrl Shift D opens the same host at once. A small menu on the Split buttons picks another host.
- **Session header:** host, address, Split right, Split down, Find. It describes the active pane.
- **Status bar:** describes the active pane.
- **Connection states, drawn as in `ConnectStates.dc.html`, inside the pane:**
  - connecting, with log lines, a spinner and Cancel
  - timed out, with Retry and Edit host
  - sign-in refused, with Enter password and Edit host
  - key refused, with Enter passphrase and Edit host
  - reconnecting, with a banner, "Reconnect now" and "Close tab"
- **Automatic reconnect** after sleep, a network change or a dropped connection:
  - 5 attempts, with the scrollback kept
  - the shell restarts, so commands are never replayed (`decisions.md`)
- **Find:**
  - the bar below the header, searching the active pane's scrollback
  - Match case and Regex
  - Previous and next match, a match count, Esc to close
- **Terminal settings:**
  - live preview
  - font cards for the four bundled JetBrainsMono Nerd Font builds, Cascadia Mono, Consolas and "Use another installed font"
  - size, line height and ligatures
  - cursor shape and blink
  - color scheme: Holonet Dark, High contrast or Light
  - scrollback, copy on select, right-click pastes
- **Connection settings:** keep-alive interval, and "Reconnect after sleep or network change".
- **Keyboard:** the read-only shortcut list in Settings, and every shortcut on it that exists by the end of this milestone.
- **Bundled fonts:** JetBrainsMono Nerd Font v3.5.1 as published, with its OFL text and the icon-set notices shipped beside the app.

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| "Try Tailscale", reconnect over Tailscale | 4 |
| "Use a key instead" (links to Keychain) | 4 |
| Broadcast input and its banner | 6 |
| Snippets drawer and the Snippets button | 6 |
| "Open SFTP here", SFTP tabs | 5 |
| Group colors on tabs and panes (hosts stay ungrouped) | 6 |
| Command palette | 7 |
| Save terminal output to History | 7 |
| Restore open sessions on launch | 7 |
| App theme "Follow Windows", Windows accent color | 7 |
| Dragging tabs to reorder them or into a split | Not designed; later, if wanted |

As in milestone 2, no control is shown before its milestone lands.

## Architecture

### `holonet-model`
- `TerminalSettings`:
  - font (bundled build or an installed family name), size, line height, ligatures
  - cursor shape and blink, color scheme
  - scrollback (10,000, 50,000 or unlimited), copy on select, right-click pastes
- `ConnectionSettings`: keep-alive (30 s, 60 s or off) and reconnect on or off.
- Both use `#[serde(default)]`, so milestone 2 vaults open unchanged. Tests cover decoding an old vault.

### `holonet-ssh`
- The keep-alive interval comes from the settings instead of the fixed `KEEPALIVE`.
- The shell's events tell "the shell exited" (exit status, then a close) apart from "the connection was lost" (transport error or keep-alive timeout, with a reason). Only a lost connection reconnects.
- `connect` is already re-entrant. Reconnecting calls it again with the same options, so a changed host key still stops the connection (`decisions.md`).

### `holonet-app`
- **`SessionTab` splits in two:**
  - `Pane`, today's `SessionTab` (one host, one connection, prompts, states, terminal)
  - `Tab`, a tree of panes (`Split { axis, ratio, a, b }` or a leaf), plus the active pane, the find bar and the header
- **`HolonetApp`** keeps a `Vec<Tab>` instead of a list of sessions. Lock, unlock and the Locked screen's session count work as before, counting panes.
- **Find** uses `alacritty_terminal`'s own regex search over the grid. Matches are drawn as background quads under the text.
- **Fonts:** the bundled TTFs are embedded and registered with GPUI's text system at start-up, as Inter and Space Grotesk already are. Fonts are loaded only for the chosen family if start-up time suffers (see phase 0).
- **Reconnect:**
  - A `Pane` that loses its connection keeps its `TerminalView`, prints a separator line and retries 5 times, after 1, 2, 4, 8 and 16 seconds. "Reconnect now" skips the wait.
  - After the fifth failure it shows the failed state, with Retry.
  - Passwords and passphrases typed into a prompt during this session are kept in memory, so reconnecting does not ask again. They are dropped when the pane closes and are never saved unless "Save in the encrypted vault" was ticked.
- **Sleep and network change:** how to detect them is phase 0's question. The keep-alive timeout catches every drop eventually; sleep and network events only make reconnecting faster and the message more precise ("when this PC went to sleep").

### Controls to add
- **Slider** (font size, line height).
- **Segmented control** (cursor shape, color scheme). The gallery's toggle group may already cover it.
- **Font card**, **host picker popup** (search field plus list, built from `select`), **split menu**.
- **Spinner**, small and large.
- Each control is added to `--gallery` with its states.

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **Spikes** *(done; ligatures work with `calt` set explicitly, the Nerd Font default stays, fonts are embedded, the sleep and network APIs register but the live test moves to phase 7)*: (a) ligatures and double-width Nerd icons in the cell grid, (b) detecting sleep, resume and network changes from a GPUI app on Windows ARM64, (c) start-up cost and binary size with 16 embedded font files (about 2.4 MB each) | Notes record each answer. If ligatures cannot be drawn without breaking the grid, the ligature setting is not shown, and the default font becomes the Mono build. |
| 1 | **Model and SSH** *(done)*: the settings structs, keep-alive from settings, lost connection told apart from shell exit | Old vault decodes; WSL tests cover exit compared with a killed connection (`ss -K` in WSL) |
| 2 | **Renderer and fonts** *(done; box-drawing and block characters are also drawn to fill the cell)*: bundled fonts and licence files, font, size and line height from settings, cursor shapes and blink (off under reduced motion), three color schemes, scrollback limit, ligatures if phase 0 allows | Screenshots of each scheme, cursor and font; `--stream-bench` no slower than milestone 1 |
| 3 | **Settings page**: Terminal (live preview, font cards, the controls above), Connection, Keyboard; changes apply to open terminals at once | Change each setting with a session open and see it apply; survives restart |
| 4 | **Tabs**: `Tab` and `Pane` split, the tab strip with state dots and sideways scrolling, new-tab host picker, Ctrl Shift T, Ctrl Shift W, Ctrl Tab and Ctrl Shift Tab, session header and status bar for the active pane | Ten tabs to two hosts, keyboard only; closing tabs in any order; lock and unlock with tabs open |
| 5 | **Split panes**: Split right and down, nesting to 4, the split menu with host picker, drag to resize, active pane, focus by click and keyboard, close pane | Four panes on two hosts, resized; each shell's `stty size` matches its header |
| 6 | **Connection states**: connecting, timed out, sign-in refused, key refused, as drawn, inside panes | Each state reached on purpose against WSL (stopped sshd, wrong password, wrong passphrase, blocked port) |
| 7 | **Reconnect**: drop detection, 5 attempts with backoff, banner, Reconnect now, Close tab, scrollback kept with a separator, typed secrets reused, changed key still stops | Kill the connection, sleep the PC, toggle Wi-Fi: each reconnects; a key swapped during the outage stops |
| 8 | **Find**: bar, Match case, Regex, highlights, current match, previous and next, count text as drawn, Ctrl Shift F, Enter, Shift Enter, Esc | Find in 10,000 lines of `--stream-bench` output stays under one frame per keystroke |
| 9 | **Wrap-up**: notes, `CLAUDE.md`, the done check above from a clean profile, PR | Done check passes |

## Undrawn states I will draw in the design system's style

- **Find:** the current match, which is stronger than the other matches. The mock highlights every match the same way.
- **Panes:**
  - the gap between panes on hover and while dragging, with a resize cursor
  - the split menu ("Same host" and a host list)
  - Split buttons disabled at 4 panes, with the reason in a tooltip
- **New-tab host picker:** results, no results, and an empty vault (which links to New host).
- **Key refused**, as a fifth connection state next to sign-in refused.
- **Reconnect gave up after 5 attempts:** a failed state with the last reason, Retry and Close tab.
- **Shell exited with the connection still up:** milestone 2's ended bar, kept.

## Defaults I will follow unless you say otherwise

- **Split down shortcut:** Ctrl Shift E. Not in `decisions.md`, and Ctrl Shift D is Split right.
- **Moving between panes:** Ctrl Shift ] and Ctrl Shift [. Ctrl Shift W closes the active pane, and the tab when it was the last pane.
- **Previous tab:** Ctrl Shift Tab.
- **Closing a connected tab:** no confirmation, as drawn.
- **Bell:** ignored. The design has no bell setting.
- **New panes:** a split takes half of the pane it splits. Dragging can make a pane no smaller than 20 columns by 5 rows.
- **Tailscale wording:** the connecting state says "Trying <address>" without the Tailscale sentence until milestone 4.
- **Fonts:**
  - The design's default, "JetBrainsMono Nerd Font" (the non-Mono build, with double-width icons), stays the default only if phase 0 shows the grid draws it cleanly. Otherwise the default is the Mono build.
  - Ship Regular, Bold, Italic and Bold Italic of each build. `OFL.txt` and `NOTICE.md` (both copyright lines and the icon-set notices) sit beside the fonts in the repository, are shown in Settings from phase 3, and go in a `licenses/` folder beside `holonet.exe` with release packaging (milestone 7).

## Risks

- **Ligatures in a cell grid.** Each styled run is currently shaped and placed cell by cell, which stops ligatures forming (prototype notes, spike 1). Drawing them means shaping whole runs and checking the result still lands on cell boundaries. Phase 0 decides this before the renderer work.
- **Double-width icons.** The non-Mono Nerd Font draws icons wider than one cell, but terminals treat those code points as one cell wide, so icons overlap the next character. This is the usual trade-off: other terminals let the icon spill into the next cell.
- **Sleep and network events** may need a hidden window or Win32 notification APIs that GPUI does not expose. The keep-alive timeout is the fallback, so reconnecting still happens, only later.
- **Binary size.** 16 font files add about 38 MB. If that is a problem, ship only the selected build's files beside the exe instead of embedding them.
- **Refactoring `SessionTab`.** It is the largest file (about 1,100 lines). Phase 4 moves it into `Pane` without changing behaviour first, then adds `Tab`.
- **Font Logos glyphs** are marked "unlicensed" upstream. Accepted for a personal, unsigned build; revisit if Holonet is ever distributed more widely.

## Owner decisions (25 November 2026)

1. **Split panes:** nested Split right and Split down, up to 4 panes per tab, resized by dragging the gap between panes.
2. **Split host:** Ctrl Shift D opens the same host at once. The Split buttons have a small menu to pick another host.
3. **New tab:** "+" and Ctrl Shift T open a quick searchable host picker. Enter connects in a new tab.
4. **Nerd Font:** bundle JetBrainsMono Nerd Font v3.5.1 as published, with the OFL text and icon-set notices.
