# Holonet

A personal Electron SSH and SFTP client for macOS and Windows ARM64. The interface uses a compact gray host library, groups, top session tabs, and JetBrains Mono. Each tab contains one terminal; SFTP has local and remote file panes.

## Features

- Live SSH with password or private-key authentication, independent sessions per host, PTY resizing, terminal search, native clipboard shortcuts, and right-click actions.
- Continuous box-drawing borders and block graphics through xterm's WebGL renderer, with a DOM fallback when GPU rendering is unavailable.
- Host groups with inherited connection defaults, reusable identities, tags, sorting, and list/card views. Removing a group keeps its hosts.
- Optional reconnect and close-tab-on-disconnect behavior. Intentional shell exits do not reconnect.
- Native local/remote SFTP browsing, upload/download, rename, folder creation, permissions, progress, overwrite confirmation, and a cancellable two-transfer queue.
- Local/remote TCP and dynamic SOCKS5 forwarding, snippets, known-host management, and connection metadata history.
- First-use server fingerprint approval and changed-key blocking. Credentials are stored separately from metadata in an OS-encrypted vault; remembering passwords and passphrases is opt-in.

The browser preview uses simulated SSH and file operations. Jump hosts, SSH agents, certificates, FIDO2, keyboard-interactive/MFA, Mosh, cloud sync, recursive SFTP transfers, transfer resume, and external file editing are outside this version. Windows ARM64 runtime testing remains outstanding.

## Development

Requires Node.js and npm, plus an OS supported by Electron.

```sh
npm ci
npm run desktop
```

For the browser preview:

```sh
npm run dev -- --host 0.0.0.0 --port 4173 --strictPort
```

Visit `http://localhost:4173` or the machine's LAN address on port 4173. For desktop development against that server, use `npm run desktop:dev`.

Set `HOLONET_USER_DATA` to an absolute disposable directory for isolated desktop tests. Normal runs use Electron's per-user application-data directory. Non-secret metadata is validated and atomically saved in `holonet/state.json`, with a previous-version backup. Credentials and trusted host keys are stored separately. Local profiles, credentials, generated builds, and reference screenshots are excluded from Git.

## Build and verification

```sh
npm test
npm run build
npm run desktop:smoke
npm run pack:mac:local
npm run dist:windows
```

The macOS local packaging command requires an existing Apple Development signing identity and produces `release/signed-local/mac-arm64/Holonet.app`. The packaging hook assigns a stable app-specific executable UUID before signing to preserve the identity used by macOS Local Network privacy. Development builds are not notarized.

The Windows command cross-builds an ARM64 NSIS installer in `release/windows-arm64/`. It omits optional platform-native SSH acceleration and uses Node crypto. The installer is unsigned unless signing is configured; validate it on Windows ARM64 before relying on that platform.

Tests cover storage, credential exclusion, clipboard routing, font loading, renderer lifecycle/fallback, real loopback SSH/SFTP/tunnels, disconnect behavior, group inheritance, macOS identity, and Sites-compatible packaging. Native smoke testing checks that the bundled renderer loads through the isolated bridge and saves initial state.

See [desktop runtime details](electron/README.md) and [project decisions](AGENTS.md). Keep active SSH sessions running while testing replacement builds with disposable profiles.

For a disposable local performance baseline, run `npm run benchmark` from the project root. It measures a 1,000-entry local folder, loopback SSH setup and terminal output, a loopback SFTP upload, and Node memory. Results vary by machine and do not include real-network performance. Run `npm run benchmark:renderer` for isolated Electron startup, renderer working-set, and JavaScript-heap measurements with zero and ten restored tabs.

Run `npm run test:desktop-ui` for an isolated Electron UI test using a temporary profile and a loopback server. It exercises saved identity selection, first-use host trust, history clearing, and the default restart behavior; it never uses the installed application's profile.
