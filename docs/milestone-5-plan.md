# Milestone 5: Files, working plan

Written 25 November 2026, after milestone 4 (`docs/milestone-4-plan.md`, results in `docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **the SFTP screen: two panes, drag between them, the transfer queue, and the permissions, rename, delete and conflict popups.** The design is `docs/design/screens/Sftp.dc.html`, with the SFTP rows of `Settings.dc.html`, "Open SFTP" in `Main.dc.html` and "Open SFTP here" in `Terminal.dc.html`.

Done means:
- From a host, or from a terminal connected to it, you can open an SFTP tab and browse this PC and the host side by side.
- Dragging files and folders between the panes (or from Explorer onto the host's pane) queues transfers. The queue shows progress, rate and time left, runs several at once, and can pause, resume and retry.
- A transfer interrupted by a dropped connection resumes where it stopped once the tab reconnects.
- A transfer that meets existing files asks once, with overwrite, keep both or skip, for the whole transfer.
- You can rename, delete (permanently on the host, to the Recycle Bin on this PC) and copy the path of files on either side, and change permissions on the host.

## Scope

**In:**
- **SFTP tab** (`Sftp.dc.html`): a tab of its own kind (the folder icon already in the tab strip), one host per tab.
  - Opened with **Open SFTP** in the host detail, **Open SFTP here** in a terminal's header, and the SFTP choice in the new-tab picker.
  - It connects as a terminal pane does: route plan, jump hosts, host key check, prompts, the connecting, failed and reconnecting states. "Open SFTP here" reuses the pane's typed secrets, so it does not ask again.
- **Two panes:**
  - **This PC**: drives at the top ("This PC"), then folders. Hidden files follow the Settings switch. It starts in the folder last used for this host, otherwise the Windows profile folder.
  - **The host**: it starts in the host's "Default SFTP folder" (a new host form field, drawn in `NewHost.dc.html`), otherwise the sign-in's home folder. It shows permissions, and symbolic links with their own icon; opening a link to a folder enters it.
  - Breadcrumbs, a filter box, columns Name, Size, Modified (and Permissions on the host), folders first. Double-click opens a folder.
  - **New folder** and **Refresh** in each pane's header (owner decision; not drawn). New folder asks for a name in a popup like Rename's.
  - Multi-select with checkboxes, Ctrl and Shift click, and select all. An action bar appears for a selection: **Upload →** or **← Download**, Rename, Permissions (host only), Copy path, Delete.
- **Drag:**
  - Between the panes, in either direction, with the drop target outlined as drawn ("Drop to upload into /mnt/tank/www/site").
  - From Windows Explorer onto the host's pane. GPUI's Windows drop target delivers the paths.
  - Folders transfer with their contents.
- **Transfer queue** (the bottom panel): each transfer shows name, route ("This PC → nas-01"), a progress bar coloured by direction, percent, rate and time left.
  - **Pause, Resume, Retry** and remove. Settings' "Transfers at once" (2, 4 or 8) sets how many run.
  - A summary ("2 active · 1 queued"), the empty state, and "Show finished (n)".
  - **Resume:** a paused, failed or interrupted transfer continues from the bytes already written.
- **Popups**, as drawn:
  - **Conflict:** "n files already exist", Overwrite, Keep both (adds " (1)"), Skip, Cancel all. One choice for the whole transfer (`decisions.md`). Settings' "When a file already exists" can make it Overwrite or Keep both without asking.
  - **Rename:** validated (no slash, name taken).
  - **Delete:** "Files deleted over SFTP do not go to a recycle bin. This cannot be undone." on the host; "These are moved to the Recycle Bin on this PC." locally.
  - **Permissions:** the 3 × 3 grid, octal and symbolic, applied to one or several items.
- **Settings, SFTP:** When a file already exists (Ask, Overwrite, Keep both), Transfers at once (2, 4, 8), Show hidden files.
- **Sessions while locked:** SFTP tabs stay connected and transfers keep running while the vault is locked (`decisions.md`).

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| Dragging from Holonet out to Explorer | Not possible with GPUI 0.2.2 (no drag source on Windows); Download covers it |
| Opening or editing a host's file in a Windows app | Later, if wanted |
| Sync or compare folders | Later, if wanted |
| SFTP over port forwards, and the "Sessions: SFTP" side-navigation entry | 6, with the session list |
| SFTP history entries | 7, with History (SFTP sessions are never logged, `decisions.md`) |

No control is shown before its milestone lands.

## Architecture

### `holonet-model`
- `Host` gains `sftp_folder: Option<String>`.
- `Settings` gains `sftp: SftpSettings { on_conflict: Ask | Overwrite | KeepBoth, at_once: 2 | 4 | 8, show_hidden: bool }`, defaulting to Ask, 2 and shown, as `Settings.dc.html` draws them.
- `Host` also gains `sftp_local_folder: Option<String>`, the folder on this PC last used for it (on the host rather than a separate map, so it goes when the host does).
- Everything uses `#[serde(default)]`; a test decodes a milestone 4 vault.

### `holonet-ssh`
- `Session::open_sftp()`: a channel with the `sftp` subsystem, wrapped in **`russh-sftp` 4.0.0** (`SftpSession::new(channel.into_stream())`).
  - That release works over any stream, is tested against russh 0.65.1, and pipelines up to 16 reads and writes in flight.
  - Its `File` implements `AsyncRead`, `AsyncWrite` and `AsyncSeek`, which resume needs.
- Errors map to interface text ("Permission denied", "No such file", "The connection was lost").

### `holonet-files` (new, no window)
- **Remote side:** list (with `symlink_metadata`, and following links), stat, rename, remove (recursive for folders), mkdir, chmod, home and canonical paths. Non-UTF-8 names are shown escaped and kept as raw bytes (`russh-sftp`'s `raw-string`).
- **Local side:** drives, listing with the hidden attribute, rename, delete to the Recycle Bin (`IFileOperation` with undo), and the profile folder.
- **Transfer engine**, Holonet's own (`build-plan.md`):
  - A queue that expands folders into files and runs N at once.
  - Pause, resume, retry and cancel, with progress events (bytes, rate, time left).
  - The conflict check before a transfer starts.
  - **Resume:** downloads write to `name.holonet-part` and are renamed when complete. A resumed download seeks to the part file's length. A resumed upload asks the host for the file's size and continues from there.
  - **Integrity:** sizes must match at the end. An interrupted transfer is resumed only if the source has not changed (same size and modification time).
- Tests: unit tests for the queue (a fake file system), and WSL integration tests for transfers, resume after a killed connection, conflicts and permissions.

### `holonet-app`
- **Tabs** hold either terminal panes or an SFTP view, so the tab strip, the rail and the shortcuts work for both.
- **`sftp/`:** the view, the two file panes, the queue panel and the popups. The connection reuses `hops.rs` and the pane's connecting, failed and reconnecting states.
- **Controls to add**, each added to `--gallery`: breadcrumbs, the file list (rows with icons, checkboxes and a drop-target state), the action bar, a progress bar, and the permissions grid.

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **Spikes** *(done; russh-sftp outpaces OpenSSH's `sftp` here, resume works, Explorer drops arrive)*: (a) `russh-sftp` 4.0 against the WSL server: list, a 500 MB upload and download with throughput compared with OpenSSH's `sftp`, seek and resume, chmod, symlinks, a non-UTF-8 name; (b) GPUI drag between two lists, and a file dropped from Explorer; (c) Recycle Bin delete; (d) drives and hidden files | Notes record each result; throughput sets the target below |
| 1 | **Model and settings** *(done)*: the new fields, the host form's "Default SFTP folder", Settings' SFTP section | A milestone 4 vault opens; Settings saves the three SFTP rows |
| 2 | **`holonet-files` operations** *(done)*: remote and local listing, rename, delete, mkdir, chmod | WSL tests for each, and the Recycle Bin on this PC |
| 3 | **Transfer engine** *(done)*: queue, folders, at-once limit, conflicts, pause, resume, retry, progress | Tests, including resume after killing the connection mid-file |
| 4 | **SFTP tab** *(done)*: tab kind, Open SFTP from the host detail, the terminal header and the picker, connecting and reconnecting | Open SFTP to the WSL server and through a jump host; drop and reconnect |
| 5 | **File panes:** listing, breadcrumbs, filter, selection, links, hidden files, copy path, drag between panes and from Explorer | Browse both sides; drag queues a transfer |
| 6 | **Queue panel and conflicts:** progress, pause, resume, retry, finished items, the conflict popup and its Settings default | A folder of mixed files both ways with conflicts; pause and resume; a dropped connection resumes |
| 7 | **Rename, delete, permissions and new folder popups**, and Refresh | Each on both sides; delete lands in the Recycle Bin locally |
| 8 | **Wrap-up:** notes, `CLAUDE.md`, gallery, the done check from a clean profile, PR | Done check passes |

## States to design

The drawings show the populated screen and its popups. These are not drawn:
- The SFTP tab while connecting, failed and reconnecting: the terminal pane's states, inside the SFTP tab.
- An empty folder, a folder that cannot be read ("Permission denied"), and a filter with no matches.
- A transfer that cannot resume because the source changed ("… changed since the transfer started. Start it again?").
- Drag onto a folder row (drop into that folder) versus onto the pane (the current folder).
- Transfers still running when the tab is closed: "n transfers are still running. Close anyway?"

## Defaults I will follow unless you say otherwise

- **Delete to the Recycle Bin locally** and permanently on the host, after the drawn confirmation (`decisions.md`).
- **Keep both** names the incoming file `name (1).ext`, then `(2)` and so on.
- **Folder transfers** follow links on this PC, and copy links on the host as links when the host supports it.
- **Permissions** set the mode only, never owner or group.
- **The local pane starts** in the folder last used for that host, otherwise the profile folder.
- **Sorting:** folders first, then by name. The column headers are not clickable, as drawn.

## Risks

- **Throughput** *(settled in phase 0)*: `russh-sftp` 4.0 with its defaults moved 500 MB at 284 MB/s up and 167 MB/s down over loopback, against 221 and 63 MB/s for Windows' OpenSSH `sftp`.
- **Symbolic links on OpenSSH servers.** OpenSSH takes `SSH_FXP_SYMLINK`'s two paths in reverse order. Holonet must pass them target first when the server is OpenSSH (phase 0's first run made two stray links on the test server, since removed).
- **Non-UTF-8 file names** on hosts. The `raw-string` feature keeps the bytes; the interface shows them escaped and transfers them unchanged.
- **GPUI drag and drop.** Dragging between views inside the app and dropping files from Explorer both appear supported on Windows; phase 0 confirms them. If dropping from Explorer fails, Upload is the fallback.
- **Large folders.** Listing thousands of entries must stay smooth; the file list draws only visible rows (`uniform_list`).

## Owner decisions (25 November 2026)

1. **Extras not drawn:** dropping files from Explorer onto the host's pane, **New folder** and **Refresh** are in scope.
2. **Connection:** each SFTP tab has its own connection, through the host's route and jump hosts. "Open SFTP here" reuses the terminal pane's typed secrets so it does not ask again. A dropped terminal does not stop transfers, and the reverse.
