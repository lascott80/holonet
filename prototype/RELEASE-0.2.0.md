# Holonet 0.2.0

The Termius workflow revamp keeps Holonet's gray palette, compact hosts,
JetBrains Mono, groups and single-terminal tabs.

## Included

- Compact library with tags, sorting, list/grid layouts, host actions, group
  launching, and SSH-address entry into a prefilled Save & connect editor.
- Shared group defaults, reusable identities, and real key import/generation.
- Independent sessions for the same host, persistent terminal instances while
  navigating, native clipboard shortcuts/context menu, match navigation, snippet
  insertion, terminal tools, and optional reconnect/selection-copy/right-paste.
- Native local/remote SFTP browsing, upload/download, overwrite decisions,
  two-transfer queue, cancellation, folder creation, rename and permissions.
- Local, remote, and dynamic SOCKS5 forwarding rules, explicit start/stop.
- Known-host management and local connection metadata history.
- macOS development-signed application and Windows ARM64 NSIS installer.

## Run

When ready to switch, quit the current Holonet and launch the new Mac build:

```sh
open "/Users/lascott/Development/Holonet/prototype/release/termius-revamp/mac-arm64/Holonet.app"
```

Windows installer:
`release/windows-arm64/Holonet Setup 0.2.0.exe`.
It is unsigned and has not been runtime-tested on Windows ARM64.

Browser preview: http://192.168.1.113:4173/ (simulated sessions and files).
The preview server stays bound to 0.0.0.0.

## Validation

35 automated tests pass. Native loopback testing verified authentication/trust,
terminal search, local/remote browsing and upload. The application signature and
stable network-privacy UUID were verified. The final signed-package smoke returned `HOLONET_SMOKE_OK` without renderer
errors. Both archives match the tested source/renderer files. Windows payload
architecture is confirmed as ARM64 (PE 0xAA64). Visual QA passed at 1280×800 and
800×700; full evidence and limits are in `design-qa.md`.

## Scope and limits

This is the core personal-client revamp, not complete Termius feature parity.
SFTP does not yet recursively copy folders, resume partial transfers, delete
files or open remote files in external editors. Abrupt termination/disconnection
can leave a uniquely named temporary transfer file. Snippets insert for review;
they do not automatically execute on multiple hosts. Forwarding uses connected
sessions; tunnels do not automatically restart after SSH loss. Reconnecting SSH
creates a new shell, not a restored remote process. Advanced authentication,
jump hosts/proxies, Mosh, cloud/team sync and collaboration are not implemented.
The application icon is still Electron's default.

Existing host metadata and encrypted credential files are preserved. All native
verification used separate temporary profiles; the running installed app was not
replaced or restarted.
