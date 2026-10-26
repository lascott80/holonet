# Holonet 0.2.2 — terminal font spacing

Fixed stretched terminal character spacing on first connection. xterm previously
cached the width of a fallback font before the bundled JetBrains Mono face loaded.
Startup now waits for that font before mounting terminals. Both initial font
options and preference updates use a quoted JetBrains Mono family with a
monospace fallback. A font-loading failure does not block access to saved hosts.

Verification: reproduced the stretched layout in an isolated native session,
then verified normal spacing after a fresh app launch with the fix. All 47 tests
pass, including font-load ordering, failed font loading, and metadata errors.
The LAN preview loads without console errors. The user's existing epcot session
was refreshed without reconnecting, and its original 14px setting was restored.

Signed macOS build:

```sh
open "/Users/lascott/Development/Holonet/prototype/release/font-metrics/mac-arm64/Holonet.app"
```

The currently connected session can stay open; use the new build when switching
applications next time. Windows ARM64 installer:
`release/windows-arm64/Holonet Setup 0.2.2.exe` (unsigned; Windows runtime testing
remains outstanding).
