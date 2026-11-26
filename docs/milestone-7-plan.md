# Milestone 7: Ship, working plan

Written 26 November 2026, after milestone 6 (`docs/milestone-6-plan.md`, results in `docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **History with opt-in logging, the command palette, OpenSSH config import, first run, the new-release notice, and an unsigned build published as a GitHub release.**

It also takes the items earlier milestones moved here:
- **Import and export of `known_hosts`** (milestone 2)
- **Restore open sessions on launch** and **Windows accent color** (milestone 3)
- **Exporting a private key** and **"Vault backup: Export encrypted file"** (milestone 4)
- **Quick fill** on the host form, and palette entries for snippets and rules (milestone 6)
- **The font licences beside `holonet.exe`** (milestone 3)
- **Checking that a downloaded, unsigned build runs on this PC under Smart App Control** (milestone 1)

The designs are:
- `History.dc.html`, `Palette.dc.html` and `FirstRun.dc.html`
- the Updates notice, "Save terminal output to History", "Restore open sessions on launch", "Vault backup" and "About and updates" in `Settings.dc.html`
- Quick fill and "Save terminal output to History" in `NewHost.dc.html`
- the Hosts search ("Find a host, or type ssh user@address") and the Settings dot in `Palette.dc.html`

Done means:
- **History:**
  - Every terminal and SFTP session leaves a record: host, kind, start, length, route, who signed in and how it ended.
  - Output is saved only for hosts with "Save terminal output to History" on, or when the Settings default is on. It is encrypted with the vault's key. SFTP sessions are never logged.
  - The History page filters records, shows a saved log, connects again, exports a log and deletes one. Logs older than the kept time are removed.
- **Command palette:** Ctrl Shift P finds hosts, snippets, port forwarding rules, screens and actions, and runs them. Ctrl Enter opens a host in a split.
- **Import:**
  - OpenSSH `config` from the Windows profile, a WSL distribution or a chosen file becomes hosts, jump hosts and Keychain keys, after a preview. The originals are untouched.
  - `known_hosts` imports into Known hosts and exports from it.
  - Quick fill on the host form takes an `ssh` command or a config entry.
- **First run:** an empty vault shows the Welcome page with quick connect, New host, the three import cards, keys, `known_hosts` and terminal setup. Quick connect also works from the Hosts search.
- **Updates:** Holonet checks for a newer release on start, daily or never. When one exists, Settings shows the notice and the Settings entry shows a dot. Nothing installs.
- **Release:** a tag builds `holonet.exe` for ARM64 (and x64) in CI, with an icon, version details, no console window and the licences, and it is published as a GitHub release. The downloaded build runs on this PC.

## Scope

**In:**

- **History** (`History.dc.html`), the Sessions entry already drawn in the side navigation:
  - **The table:** Host (with its group colour), Started, Length, Route and Log (its size, or "No log"), with "Filter by host" and "No sessions match this filter."
  - **The detail panel:**
    - the host, start, length and route
    - **Connect again**, which opens a new tab
    - **Export log**, which saves the output as a `.txt` file you pick
    - **Saved output**, or "Logging is off for this host, so no output was saved." / "SFTP sessions are not logged."
    - "Signed in as", "Ended" ("Closed by you", "Connection dropped", "Sign-in failed")
    - **Delete this log** (asks first, cannot be undone)
  - **The note:** "Logging is off unless you turn it on for a host; SFTP sessions are never logged. Set the default, and how long logs are kept, in Settings."
- **Logging:**
  - **Host form:** "Save terminal output to History", with the amber warning drawn in Settings ("Logs can contain anything typed or printed, including secrets.").
  - **Settings, Connection:** "Save terminal output to History" (the default for hosts that do not set their own) and **Keep logs for** (not drawn: 7 days, 30 days, 90 days, Forever).
  - **What is saved:** the text the shell prints, as plain text without colour codes. What you type is saved only as the shell echoes it, so a password prompt that does not echo leaves nothing.
  - A reconnect adds a "Reconnected" line to the same log. Broadcast and snippets are ordinary output.
  - The terminal's status bar shows "Logging to History" while it is on.
- **Command palette** (`Palette.dc.html`), Ctrl Shift P and the Hosts page's palette button:
  - **Search hosts, snippets and commands**, with sections:
    - **Hosts:** "Go to session" for a host with an open tab, otherwise "Connect", and an "Open SFTP" entry.
    - **Snippets:** "Run in active session", greyed out with no terminal open.
    - **Port forwarding:** "Turn on" or "Turn off" for each rule.
    - **Go to:** every screen in the side navigation.
    - **Actions:** New host (Ctrl Shift N), Lock vault (Ctrl Shift L), New tab, Split right, Broadcast, Find, and Check for updates.
  - Up and Down move, Enter opens, Ctrl Enter opens a host in a split of the current tab, Esc closes. Results rank exact and prefix matches first, then recent hosts.
  - **New shortcuts:** Ctrl Shift N and Ctrl Shift L, both Ctrl Shift like the others, in the Settings Keyboard list.
- **OpenSSH config import** (`FirstRun.dc.html`'s cards, and an "Import" button on the Hosts page):
  - **Sources:** `%USERPROFILE%\.ssh\config`, `~/.ssh/config` in a WSL distribution you pick (as the Keychain already lists them), or a file you choose.
  - **Read from each `Host` block:** `HostName`, `Port`, `User`, `IdentityFile`, `ProxyJump`, `ForwardAgent` and `Compression`. `Include` is followed. Values are found as `ssh` finds them: the first value of each setting wins, so `Host *` at the end fills in what hosts left unset (phase 2). Patterns with `*` or `?`, `Match` blocks and other keywords are skipped and listed as "Not imported".
  - **The preview:** a list with a checkbox per host (the Keychain's import list), its address, user and jump host, and why any host is skipped. Hosts whose name is already in the vault say "Already in the vault" and are unchecked.
  - **On import:**
    - Each host is copied into the vault without a group.
    - `ProxyJump` becomes a jump host chain when every hop is imported or already in the vault, otherwise the host notes "Jump host not found".
    - `IdentityFile` keys are copied into the Keychain, asking for a passphrase as Keychain import does. A key already in the Keychain by fingerprint is reused.
    - A summary says "Imported 12 hosts and 3 keys. 2 were skipped."
- **`known_hosts`:**
  - **Import** on the Known hosts page and the First run card: from the Windows profile, WSL or a file. Hashed entries (`|1|`) are kept and matched by hash. `@revoked` and `@cert-authority` lines are skipped and counted.
  - **Export** writes the trusted keys as a `known_hosts` file you pick.
  - A key that conflicts with one already trusted is not imported, and the summary says so (`decisions.md`: changed keys are never accepted silently).
- **Quick fill** on the host form (`NewHost.dc.html`):
  - Paste `ssh -J bastion-east deploy@10.30.0.9 -p 22` (with `-p`, `-l`, `-i`, `-J`, `-A`, `-C` and `user@host`), or a `Host` block from a config file.
  - It fills the fields it understands and says which parts it did not use.
- **First run** (`FirstRun.dc.html`), replacing the milestone 2 welcome:
  - "Welcome to Holonet" with **Connect now** (`ssh` and a `user@address` field) and **New host**.
  - **Import what you already have:** the three config cards, "Add your keys" (opens the Keychain's import), "Bring your known_hosts" and "Set up the terminal" (opens Settings, Terminal).
  - "Imports are copied into the encrypted vault. Your original files are left as they are."
  - It shows while the vault has no hosts.
- **Quick connect:**
  - From Connect now, and from the Hosts search when it starts with `ssh ` or holds `user@address`.
  - It opens a terminal tab without saving a host. The host key check and password prompts work as usual.
  - The tab's header offers **Save as host**, which opens the host form filled in.
- **Updates** (`Settings.dc.html`):
  - **The notice** at the top of Settings: "Holonet 0.2.0 is available", "You have 0.1.0. Released on GitHub on 24 Nov 2026.", with "View release on GitHub" (opens the browser) and **Later** (hides it until the next newer release).
  - **The dot** on the Settings entry while a newer release is known and not dismissed.
  - **About and updates:** Version ("0.1.0 · Windows ARM64"), Look for new releases (When Holonet starts, Once a day, Never), Last checked, **Check now**, and the note.
  - **States not drawn:** "Holonet is up to date.", "Holonet could not reach GitHub.", and checking.
- **Settings items left from earlier milestones:**
  - **Restore open sessions on launch:** reopens the tabs and split layout open at the last close, each reconnecting. Off by default.
  - **Use Windows accent color:** the accent follows Windows' accent colour.
  - **Vault backup: Export encrypted file:** a copy of the vault file you place anywhere. It opens with the same password.
- **Keychain:** **Export private key** as an OpenSSH file, with a passphrase you choose (or none, after an amber question).
- **Release:**
  - **The exe:**
    - No console window when opened from Explorer. The hidden modes (`--gallery` and the others) still print when run from a terminal.
    - An app icon drawn from the Holonet mark, and the version, name and copyright in its file details.
  - **The package:** `holonet-0.1.0-windows-arm64.zip` (and `-x64`) holding `holonet.exe`, `licenses/` (the font licences and notices, and Holonet's dependency licences) and a short `README.txt`.
  - **CI:** a `release.yml` workflow that builds both on a `v*` tag and attaches the zips to a draft GitHub release.
  - The owner publishes the draft, or tells me to.

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| App theme "Follow Windows" (needs a light app theme, which is not designed) | Later, if wanted |
| PuTTY and Termius import, hardware security keys | Not in the first release (`CLAUDE.md`) |
| Searching inside saved logs | Later, if wanted |
| Signed builds, an installer, installing updates | Not planned (`decisions.md`) |

No control is shown before its phase lands.

## Architecture

### `holonet-model`
- **`SessionRecord { id, host, host_name, kind: Terminal | Sftp, started, ended, route, user, end: ClosedByYou | Dropped | SignInFailed, log: Option<LogId>, log_bytes }`.** `host_name` is kept so a record still reads after its host is deleted.
- **`Host` gains** `log_output: Option<bool>`, where `None` follows the Settings default.
- **`Settings` gains:**
  - `log_default`, `log_keep` (7, 30, 90 days or forever)
  - `update_check` (Start, Daily, Never), `update_last_checked`, `update_dismissed`
  - `restore_sessions` and the saved layout, `windows_accent`
- **Schema 4,** with a test that decodes a milestone 6 vault.

### `holonet-vault`
- **Logs live beside the vault**, in a `logs` folder next to the vault file, one file per session. Each is encrypted with a key derived from the vault's key (XChaCha20-Poly1305 in appended chunks, so a crash loses at most the last chunk).
- **Why not inside the vault file:** every vault save rewrites the whole file, and logs of hundreds of kilobytes per session would make each save slower. The records, which are small, stay in the vault.
- A locked vault keeps writing open logs. The key stays in memory for that, as the open sessions do (`decisions.md`: sessions stay connected while locked).
- **Backup export** copies the vault file as it is on disk, without logs.

### `holonet-import` (new crate, no I/O beyond reading the text it is given)
- **`config`:** a parser for OpenSSH `config` (keywords case-insensitive, `=` or spaces, quoted values), `Include` resolved through a callback that reads files, and `Host *` defaults applied in order. It returns hosts and the skipped entries with reasons.
- **`known_hosts`:** a parser (plain and hashed names, `[host]:port`, markers) and a writer.
- **`command`:** the Quick fill parser for `ssh` command lines and single `Host` blocks.
- Tests against sample files, including the owner-style layouts the WSL test server has.

### `holonet-app`
- **`history.rs`:** the log writer for each pane (strips escape sequences from the bytes the terminal already receives), records opened and closed with sessions, and the clean-up of old logs on start.
- **`workspace/history.rs`:** the History page.
- **`palette.rs`:** the palette overlay and its sources. Each entry dispatches the same action as its button, so the palette adds no paths of its own.
- **`workspace/import.rs`:** the import preview and summary, shared by First run, the Hosts page and Known hosts.
- **`updates.rs`:** the release check, using `https://api.github.com/repos/lascott80/holonet/releases/latest` and comparing versions. It runs off the UI thread with a 10-second timeout and reads nothing else.
- **`build.rs`:** the icon and version resource (`winresource`, a new build dependency).
- **Controls to add, each in `--gallery`:**
  - the palette row and section header
  - the History table row and log viewer
  - the update notice
  - the first-run import card

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **Spikes** *(done; a CI build runs here, Smart App Control is now off, WinHTTP reads releases, the log writer is faster than the terminal, and release builds have no console)*: (a) build a release exe in CI on `windows-11-arm`, download it here and run it under Smart App Control; (b) the update check against GitHub's releases API (a public repository's, since this one is private until the release); (c) the log writer at `--stream-bench` speed with encryption; (d) `windows_subsystem = "windows"` with the hidden modes still printing in a terminal | Notes record each result. (a) decides whether an unsigned download can be the way this PC gets Holonet |
| 1 | **Model and vault** *(done; the logs folder is named after the vault file, `vault.holonet.logs`)*: records, host and settings fields, schema 4, the log files and their key, backup export | A milestone 6 vault opens; unit tests for log chunks, a torn last chunk, and clean-up by age |
| 2 | **`holonet-import`** *(done; values follow `ssh`'s first-value-wins order, and hashed names were checked against `ssh-keygen -H`)*: config, `known_hosts` and Quick fill parsers | Unit tests cover `Include`, `Host *`, patterns, `ProxyJump` chains, hashed `known_hosts` and command lines |
| 3 | **Logging and History** *(done; checked live, including a crash, a reconnect and an end while locked)*: the host form switch, the Settings default and kept time, the status bar item, the History page | A logged session shows its output, survives a reconnect and exports; a host with logging off shows "No log"; an SFTP tab leaves a record only |
| 4 | **Import** *(done; a WSL test config with a jump host and a key imported and connected; the WSL card was not tried live, as it reads your own WSL user)*: the import preview from the three sources, keys, jump chains, `known_hosts` import and export, Quick fill | A config in the WSL test account with a jump host and a key imports, and the imported host connects |
| 5 | **First run and quick connect:** the Welcome page, Connect now, the Hosts search, Save as host | From a new vault: quick connect, save it as a host, then import |
| 6 | **Command palette** and Ctrl Shift N and L | Every section's entries run; Ctrl Enter splits; the palette works over a terminal |
| 7 | **Updates:** the check, the notice, the dot, About and updates | Against a test release: notice, dot, Later, a newer one shows again; offline says it could not reach GitHub |
| 8 | **Earlier items:** restore open sessions, Windows accent colour, private key export | Close with two tabs and a split, start again: they come back and reconnect; an exported key signs in with OpenSSH in WSL |
| 9 | **Release packaging:** icon, version details, no console, licences, the zips, `release.yml`, and the history check before the repository goes public | A `v0.1.0-rc1` tag makes a draft release whose ARM64 zip, downloaded, runs here |
| 10 | **Wrap-up:** notes, `CLAUDE.md`, gallery, the done check from a clean profile, PR, then the `v0.1.0` release once the PRs are merged | Done check passes; the owner publishes 0.1.0 |

## States to design

The drawings show the populated pages. These are not drawn:
- **History:** no sessions yet, a record whose host was deleted, a log being written (the open session), and the delete question.
- **Keep logs for** in Settings, and the host form's own switch with "from Settings".
- **The palette with no matches** ("Nothing matches. Press Enter to connect to deploy@10.0.0.4" when the text reads as `user@address`).
- **Import:** reading, a file that cannot be read, nothing to import, the preview's skipped rows, a key that needs a passphrase, and the summary.
- **Quick fill:** what it did not use.
- **Updates:** up to date, checking, and could not reach GitHub.
- **Save as host** in a quick-connect tab's header.

## Defaults I will follow unless you say otherwise

- **Every session leaves a record, logging or not,** as `History.dc.html` draws ("No log" rows). Records are kept for the same time as logs.
- **Logs are kept for 30 days** by default.
- **Logs are plain text,** without colours, so exported logs read in any editor.
- **Imported hosts get no group,** and the preview says so. Group them afterwards.
- **Quick connect does not save anything** until Save as host. Its host key is still checked and, with "Trust and connect", saved to Known hosts.
- **Restore open sessions** reopens tabs and splits but never re-runs startup snippets, matching reconnects.
- **The first release is 0.1.0,** as Settings draws. The release title is "Holonet 0.1.0" and the notes list the milestones.
- **The tag goes on `main`** after PRs #1 to #7 are merged, which is the owner's call.

## Risks

- **Smart App Control** *(settled in phase 0)*: it is now off on this PC, and a downloaded build with an internet mark ran. SmartScreen may still warn once on a double-click from Explorer, which phase 9 checks.
- **This repository is private** until the release (owner decision 1). Until then the update check can only be tested against a stand-in release feed, and spike (b) checks that path.
- **OpenSSH config is wide.** Holonet reads the keywords it can act on and lists the rest instead of guessing.
- **Logging cost.** Writing every byte of a busy shell must not slow the terminal. Spike (c) measures it; the writer batches on a background thread.
- **A new build dependency** (`winresource`) for the icon. It needs the Windows SDK's resource compiler, which CI and this PC both have.

## Owner decisions (26 November 2026)

1. **The repository becomes public** before the first release, so the update check reads the public releases page as drawn. In phase 9 I check the whole git history for anything that should not be public (the `.dev` credentials, addresses, personal details) and report it. You then make the repository public, or tell me to.
2. **Logs live beside the vault,** one encrypted file per session in a `logs` folder next to the vault file, under the vault's key. Session records stay inside the vault, and backup export copies the vault file only.
