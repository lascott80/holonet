# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## Approved Holonet decisions

- Electron desktop app for macOS and Windows ARM64; preserve the browser preview bound to 0.0.0.0.
- Fixed neutral gray palette, compact host tiles with collapsible groups and Open group actions. No Workspaces feature.
- Session tabs on top. Host name and connection identity occupy one compact row underneath.
- JetBrains Mono in terminals. One terminal per tab; no terminal splitting. The only dual-pane view is local/remote SFTP.
- Persist metadata and preferences through the isolated Electron bridge. Do not put passwords, passphrases or private keys in the JSON state or browser storage.
- Native milestone one has no live SSH or pretend connected sample hosts. Keep simulation confined to the browser preview.

## Live SSH milestone

Native sessions now use ssh2 and xterm.js for real PTY I/O. The browser remains a simulation. Actual host fingerprints must be approved before authentication; changed keys are blocked. Passwords and private keys are stored only in a separate vault encrypted through Electron's asynchronous OS-backed safeStorage API. Remembering passwords/passphrases is opt-in. Preserve single-terminal tabs and keep native SFTP disabled until implemented.

## macOS local testing

Use `npm run pack:mac:local` for runnable local builds. LAN privacy needs the
local-network usage description plus an Apple-issued code-signing identity.
Do not recommend an unsigned renamed Electron bundle as the reliable LAN build.
Keep the user's active SSH sessions running when building a replacement.
The macOS afterPack hook must assign distinct executable UUIDs before signing;
stock Electron UUIDs are shared across apps and can confuse Local Network privacy.

## Termius reference alignment

The installed Termius desktop app is the behavior/layout reference for the daily
SSH workflow. Preserve Holonet's neutral gray palette, JetBrains Mono, groups,
and no terminal splits. Use one global top tab strip with a Hosts library tab;
hide the sidebar during terminal sessions by default, with a visible toggle.
Hosts use compact two-line cards: single-click selects, double-click or Enter
connects, arrow keys move selection. Keep native window controls in the same
strip. Native SFTP is still disabled until real transfers are implemented.
Observed Termius also displays Bash bracketed-paste active-region highlighting.
Do not disable bracketed paste or strip terminal formatting to mimic its behavior.

## Full Termius workflow revamp

The local deep audit in design/termius-deep-dive.md informed the revamp; that
optional screenshot archive is excluded from Git. The durable decisions are
recorded here. Current functionality includes independent session IDs (multiple sessions per
saved host), mounted terminal instances across navigation, terminal find with
match navigation, optional selection-copy/right-click-paste/Option-as-Meta,
functional native clipboard rebinding, group defaults and reusable identities,
tags/sort/list view, snippets, known-host management, connection metadata history,
native local/remote SFTP with a two-transfer queue and cancellation cleanup, and
local/remote/dynamic forwarding. This supersedes earlier instructions to keep
native SFTP disabled. Browser functionality remains clearly simulated.

Preserve existing version-1 metadata and encrypted credential files. New metadata
fields are additive and optional. Do not run or replace the user's installed app
while validating a new build; use HOLONET_USER_DATA with a disposable profile and
loopback fixtures. Windows ARM64 still needs testing on actual Windows hardware.

## Group removal and disconnect behavior

Removing a populated group moves its direct hosts to the ungrouped Hosts list,
materializing inherited connection settings so credentials/ports remain the same.
Keep subgroups as top-level groups with inherited defaults preserved. Never delete
hosts or disconnect their live sessions as a side effect of deleting a group.
Settings > General > When a session disconnects supports Keep tab open / Close tab.
Close tab takes precedence over automatic reconnect for established sessions;
initial connection failures stay visible. A deliberate shell exit never starts
automatic reconnect. Preserve the existing keep-open default.

## Terminal font measurements

Wait for the bundled JetBrains Mono face through document.fonts.load before
mounting the app/initializing xterm. Always supply a monospace fallback in xterm's
fontFamily, both initially and when applying preferences. Measuring before the
font loads caches a wider fallback W glyph and creates stretched character cells.
Keep font failures nonfatal so saved hosts remain accessible with monospace.

## Terminal graphics

Use xterm's WebGL addon with customGlyphs enabled for continuous Unicode box
borders and block bars, including at larger line heights. Allocate a GPU renderer
only for the visible terminal; hidden terminals keep their buffers using the DOM
renderer. Dispose GPU resources on navigation/unmount. If initialization fails or
the context is lost, retain a usable DOM terminal without reconnecting SSH.
The local Termius reference screenshots are intentionally excluded from Git.
