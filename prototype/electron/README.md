# Holonet desktop runtime

The main process owns SSH connections, local files, SFTP transfers, forwarding,
credential encryption, trust records, connection history, and settings. The
renderer is isolated and sandboxed with Node integration disabled. Privileged
calls require the owning window's exact main frame and renderer URL. Navigation,
popups, and permission requests are blocked, and production assets use a local
CSP. The preload exposes named operations, not arbitrary IPC or shell execution.

`store.mjs` validates metadata with `shared/state.mjs`, writes privately through
an atomic rename, and preserves the previous valid state as a backup. Damaged or
future-version data is not silently replaced. The version-1 format now has
optional fields for stable group IDs/defaults, identities, snippets, forwarding
rules, tags, and separate session IDs. Existing hosts and credentials migrate
without replacing their IDs. App close waits for pending metadata/vault writes.

## Terminal and authentication

`ssh.mjs` owns ssh2 connections and remote PTYs. Saved host IDs identify reusable
connection metadata; session IDs identify independent connections and terminal
instances. Raw server keys are verified before authentication; changed keys are
blocked. Keepalives, PTY resizing, bounded initial output snapshots, and renderer
output acknowledgements remain active. xterm instances stay mounted across
library/settings/SFTP navigation, preserving terminal state and viewport.

`secrets.mjs` keeps credentials separate from metadata and encrypts them through
Electron's asynchronous OS-backed safeStorage API. There is no plaintext fallback.
Host passwords are scoped to saved host ID/address/port/user. A deliberately
assigned reusable identity shares its saved password by identity ID and username;
server fingerprint checks still happen before authentication. Remembering a
password or key passphrase is opt-in and occurs only after authentication.
Private keys remain encrypted at rest. `keys.mjs` implements real ED25519/RSA
key generation, private-key import, and public-key retrieval.

Native Copy/Paste/Select All menu accelerators follow saved shortcut settings and
route to the focused xterm instance. Form controls retain ordinary editing.
Paste uses `Terminal.paste()` so bracketed paste and line endings are preserved.
Default macOS shortcuts are Cmd+C/V/A; Windows uses Ctrl+Shift+C/V/A, leaving
Ctrl+C available to the shell. Selection-copy, right-click-paste, Option-as-Meta,
terminal find with match navigation, font/cursor settings, and optional reconnect
are implemented. General settings can keep disconnected tabs or close them;
initial connection errors stay visible. Intentional shell exits never trigger
automatic reconnect, and close-on-disconnect takes priority. Reconnect creates a new remote shell; it cannot restore a lost
remote process. Shell history/transcripts are not captured.

## File and administration tools

`tools.mjs` implements direct native local-file browsing (no localhost SSH), remote
SFTP directory listing, folder creation, rename, permissions, upload/download,
overwrite confirmation, progress, a two-transfer queue, and cancellation. Data
first writes to uniquely named temporary files. Failed/cancelled transfers remove
those partial files when the process and connection remain available. Abrupt app
termination or a lost server connection can leave a `.holonet-<id>` temporary
file; there is no automatic resume. File deletion and removal of empty directories require confirmation. Directory recursion and external editing are not implemented in this version.

Local and remote TCP forwarding and dynamic SOCKS5 CONNECT forwarding use ssh2
channels with explicit start/stop and connection cleanup. Bind defaults to
127.0.0.1. Existing connected sessions can be reused; otherwise the user connects
the rule's host and starts the rule. Active tunnels are not automatically restarted
after SSH loss. Known Hosts exposes real trust records and explicit removal; a
changed key must be independently verified before removing the old record.
Connection history stores only local connection metadata, capped at 500 entries.
Snippets are inserted through the terminal paste pipeline for review before running.

The browser preview uses simulated sessions/files/transfers and never receives
these privileged capabilities. Jump hosts, SSH agents, certificates, FIDO2,
keyboard-interactive authentication, Mosh, cloud sync, and split terminals are
outside this build. Unsupported jump-host settings are not offered as working.

## Build and test

- `npm test` runs storage, clipboard, real loopback SSH/SFTP/tunnel, platform-identity,
  and Sites packaging regressions.
- `npm run build` preserves the existing Sites-compatible build output.
- `npm run pack:mac:local` makes a signed local macOS ARM64 development app in
  `release/signed-local/mac-arm64/Holonet.app`.
- `npm run dist:windows` produces a Windows ARM64 NSIS installer in
  `release/windows-arm64/`. Cross-builds omit optional host-platform native SSH
  acceleration and use Node's built-in crypto fallback. The installer is unsigned
  unless a Windows signing configuration is supplied. Test it on Windows ARM64
  before treating that platform as verified.

macOS local builds require an existing Apple Development signing identity, the
local-network usage description, and bundle-specific Mach-O UUIDs assigned by
`afterPack` before signing. This preserves the app identity used for Local Network
privacy. Development packaging does not upload or notarize the app. Preserve the
user's running sessions while building replacements; verify in an isolated
`HOLONET_USER_DATA` profile and with disposable loopback servers.

## Terminal graphics

Startup waits for the bundled terminal font before measuring character cells.
The visible terminal uses the WebGL addon with custom glyphs to join box-drawing
borders and block graphics across rows. Hidden terminals release GPU resources
while keeping their buffers and SSH sessions mounted. Initialization failure or
context loss falls back to the DOM renderer; ordinary text stays usable, though
font-rendered borders can show gaps at larger line heights.
