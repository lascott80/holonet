# Holonet 0.2.3 — continuous terminal graphics

Terminal box-drawing borders and block bars now fill their cells using xterm's
WebGL renderer and custom glyphs. This removes the gaps between vertical table
borders at larger line heights while preserving JetBrains Mono and row spacing.
Only the visible terminal holds a GPU renderer; navigation preserves its buffer
and SSH connection. GPU initialization failure or context loss retains the DOM
renderer so the terminal remains usable (font-rendered borders may have gaps).

Verified single/rounded/double borders and block shading in a disposable native
SSH session at 14px and line height 1.4, including returning through Hosts and
reopening a saved tab after app restart. Renderer regression tests cover resource
release, initialization failure, and context loss. Windows ARM64 runtime testing
remains outstanding.

Signed local macOS app: `release/terminal-graphics/mac-arm64/Holonet.app`.
Windows ARM64 installer: `release/windows-arm64/Holonet Setup 0.2.3.exe`.
The macOS build is development-signed and not notarized; the Windows installer is
unsigned. Existing sessions can stay open until switching to the new build.
