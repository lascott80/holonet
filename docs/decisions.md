# Holonet: decisions

All of these were decided on 25 November 2026. The live copy, with the original reasoning, is at https://claude.ai/artifact/LhEpxHMFtZeFTQWH1bnpWD (owner's account).

## Connection and Tailscale

| Topic | Decision |
| --- | --- |
| Primary and Tailscale addresses | A host has a required primary address and an optional Tailscale address (a 100.x address or a tailnet name). |
| Default route | Automatic: try the primary address first, then Tailscale. Each host can be set to Primary only or Tailscale only, and the choice can be overridden per connection. |
| Fallback wait | 3 seconds on the primary address, adjustable in Settings. Skip the wait when the primary address is private and this PC has no route to it. |
| Detecting Tailscale | Check that Tailscale is running before offering it. If it is not, say so instead of timing out. |
| Reconnect | On after sleep or a network change: 5 attempts, scrollback kept. The shell restarts, so commands are never replayed. |
| Keep-alive | Every 30 seconds. |

## Vault and security

| Topic | Decision |
| --- | --- |
| Lock | After 15 minutes idle, and when Windows locks. Unlock with Windows Hello, with a vault password as the alternative. |
| Sessions while locked | Stay connected in the background and reappear on unlock. Port forwards stay up too. |
| Unknown host keys | Ask every time. "Trust and connect" saves the key; "Connect once" does not. "Refuse to connect" is a stricter option in Settings. |
| Changed host key | Always stop the connection. Replacing the trusted key sits behind an "I expected this change" disclosure. Never add a setting that accepts changed keys automatically. |
| Key import | Keys are copied into the vault. Sources: a key file anywhere, `~/.ssh` in the Windows profile, and `~/.ssh` in an installed WSL distribution. |
| Hardware security keys | Not in the first release. |

## Hosts, groups and deletion

| Topic | Decision |
| --- | --- |
| Deleting a host | Its trusted server key is kept by default. The delete prompt has an "Also forget its server key" checkbox. |
| Deleting a group | Its hosts are kept and become ungrouped by default. The delete prompt has an "Also delete its hosts" checkbox. |
| Group defaults | A group sets a color and defaults (key, jump host, route, broadcast warning). A host can override any of them. Inherited values are labelled "from group" on the host form (not yet drawn). |
| Startup snippets | Must come from the snippet library. |
| Imports | OpenSSH `config` only, from the Windows profile, a WSL distribution or a chosen file. Imports are copied; originals are untouched. |

## Terminal, snippets and SFTP

| Topic | Decision |
| --- | --- |
| Session logging | Off by default, with a per-host switch and a global default in Settings. Logs are stored in the vault. SFTP sessions are never logged. Owner decision, milestone 7: each log is an encrypted file beside the vault file, under the vault's key, and the session records are inside the vault. |
| Default terminal font | JetBrainsMono Nerd Font at 13px, bundled, once the licences are confirmed. The Mono, Propo and NL builds are also offered, plus Cascadia Mono, Consolas and any installed font. |
| Broadcast input | An amber strip while it is on, counting hosts in "warn" groups. One confirmation is required when a group marked "warn" is included. |
| Snippet variables | `${name}` in a command asks for a value on every run. |
| SFTP conflicts | Ask, with one choice (overwrite, keep both, skip) applied to the whole transfer. |
| SFTP delete | Permanent on the server; Recycle Bin on this PC. |

## Build

| Topic | Decision |
| --- | --- |
| Interface toolkit | GPUI, all Rust, chosen for app speed. Tauri is the fallback. |
| Keyboard shortcuts | App shortcuts use Ctrl Shift everywhere, so shell line-editing keys reach the terminal. Palette: Ctrl Shift P. New tab: Ctrl Shift T. Close tab: Ctrl Shift W. Split right: Ctrl Shift D. Find: Ctrl Shift F. Copy and paste: Ctrl Shift C and V. Next tab: Ctrl Tab. |
| Control sizes | 44px controls, 36px in the title bar and toolbars. Mouse-first. |
| Distribution | Unsigned, personal use, published as GitHub releases on this repository. Windows warns the first time each build runs. |
| Updates | Settings shows a notice, and the Settings navigation entry shows a dot, when a newer GitHub release exists. Nothing installs automatically. Check frequency: on start, daily, or never. |
