# Holonet: Rust build plan

Written 25 November 2026. The live copy, with links to every source checked, is at https://claude.ai/artifact/AFhwmNxYH7WjRngF9QFBTg (owner's account).

## Decision

Holonet is built entirely in Rust with GPUI, because the priority is the speed of the app itself. `aarch64-pc-windows-msvc` is a Tier 1 Rust target with host tools, so the compiler and Cargo run natively on Windows on ARM.

GPUI draws the whole interface on the GPU in one Rust process, so terminal output never crosses into a web view. Zed ships on Windows with a DirectX 11 renderer and an embedded terminal, which is the evidence that the approach works on this platform.

The cost is build time. The 17 designed screens are a visual reference, not code: each is rebuilt in Rust against a framework with little documentation. The design system's tokens and component guidelines are the specification.

Tauri with a web-rendered interface stays as the fallback if the prototype shows GPUI cannot do something the design needs on ARM hardware.

## Core libraries

| Need | Choice | Gap or check |
| --- | --- | --- |
| SSH | `russh` (async, Tokio): password, public key, keyboard-interactive and OpenSSH certificate sign-in; local and remote forwarding; PuTTY key files; Pageant | FIDO keys are not listed. Jump hosts are built by tunnelling one session through another. |
| SFTP | `russh-sftp` | Transfer queue, resume and conflict handling are Holonet's own code. |
| Crypto backend | `aws-lc-rs` or `ring` (russh needs exactly one) | Confirm which builds for ARM64 Windows in the prototype. |
| Terminal | `alacritty_terminal` inside a GPUI view | Ligatures and Nerd Font glyph widths need testing. |
| Dynamic forwarding | Own SOCKS5 listener opening a forwarding channel per connection | Not provided by russh. |
| Windows Hello | The `windows` crate, `UserConsentVerifier` | Desktop apps call it through an interop interface with a window handle. The related key prompt has a reported bug where it opens behind other windows. |
| Tailscale | Tailscale's local API | The existing `tailscale-localapi` crate was last released in December 2023; treat it as a reference and plan a small client of our own. |

## Milestones

1. **Prototype.** A bare window that connects to one host with russh and shows a working terminal, built with GPUI and alacritty_terminal. Measure typing latency and the time to print a large file, and check that Nerd Font glyphs and ligatures render.
2. **Connect.** Vault on disk, hosts list and New host form, password and key sign-in, the host key prompt and Known hosts, one terminal tab.
3. **Sessions.** Tabs, split panes, the connecting, failed and reconnecting states, and the terminal settings including the bundled Nerd Fonts.
4. **Keys and routes.** Keychain with generate and import, Windows Hello unlock and auto-lock, jump hosts, and the primary to Tailscale fallback.
5. **Files.** The SFTP screen: two panes, drag between them, the transfer queue, and the permissions, rename, delete and conflict popups.
6. **Power features.** Port forwarding including dynamic, snippets with variables and multi-host runs, broadcast input, groups and their defaults.
7. **Ship.** History with opt-in logging, the command palette, OpenSSH config import, first run, the new-release notice, and an unsigned build published as a GitHub release.

No dates are attached: the prototype is what makes the rest estimable.

## Risks

- **Interface build time.** Rebuilding 17 screens in code is the largest cost. Build the shared controls (button, field, tab, card, dialog, table row) first, then one screen, before estimating the rest.
- **Windows Hello prompt.** Check that it opens in front of the Holonet window on ARM devices.
- **Crypto backend on ARM64.** Confirm the chosen backend compiles and passes its tests.
- **GPUI outside Zed.** Its API follows Zed's needs and documentation is thin; expect to read Zed's source.

Not verified during planning: iced's and Slint's ARM64 status (both were set aside), and the licences for bundling JetBrainsMono Nerd Font.
