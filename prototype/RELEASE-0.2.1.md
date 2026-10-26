# Holonet 0.2.1

- Removing a group moves its direct hosts to the main Hosts list. Their effective
  connection settings are preserved, including inherited username, port, identity
  and key choice. Live sessions and saved credentials are not deleted.
- Child groups are retained as top-level groups, preserving inherited defaults.
  Conflicting promoted names receive the former parent's name as a suffix.
- Settings > General > When a session disconnects now offers Keep tab open or
  Close tab. The existing keep-open default is preserved. Close tab applies to
  established sessions, including `exit`, and overrides automatic reconnect.
  Initial connection failures remain visible for diagnosis. Intentional shell
  exits never start automatic reconnect.

All 44 regression tests pass. The signed macOS package passes signature and
startup smoke checks. Both platform archives match the tested source and build.

Native loopback verification: connected through the UI, typed exit with both
close-on-disconnect and autoreconnect enabled, and observed the tab disappear
without a confirmation prompt or reconnect. Browser verification also covered
keep-open behavior, group deletion with inherited settings, and preservation of
other tabs. Temporary test metadata was removed and preview preferences restored.

Launch the new signed macOS build after quitting the current app:

```sh
open "/Users/lascott/Development/Holonet/prototype/release/session-behavior/mac-arm64/Holonet.app"
```

Windows ARM64 installer: `release/windows-arm64/Holonet Setup 0.2.1.exe`.
Windows runtime testing remains outstanding; the installer is unsigned.
