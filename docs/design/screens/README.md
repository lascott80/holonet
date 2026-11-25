# Designed screens

Seventeen screens, drawn on the Holonet design canvas: https://claude.ai/artifact/ERcWUAQ2qwDopUveQzZ5NG (owner's account).

The `.dc.html` files here are the canvas sources. They need the canvas runtime and will not render if opened directly in a browser. Read them as a specification: the markup is the structure, the inline styles are the exact sizes and spacing, and colors are written as `var(--token, #fallback)` using the names in `../tokens.json`. The script block at the end of each file holds the sample data and the interactions that were mocked.

Sample data throughout (hosts, addresses, files, terminal output, versions, dates) is invented. Fingerprints and public keys are bracketed placeholders.

| File | Screen | What it shows |
| --- | --- | --- |
| `Main.dc.html` | Hosts | Groups, host cards or a compact list, and a detail panel with addresses, route choice, sign-in, jump host, startup snippet and host key. |
| `NewHost.dc.html` | New host | The host form: label, group, primary and Tailscale addresses, route, sign-in method, jump host, startup snippet, logging, with a live summary and equivalent `ssh` command. |
| `EditHost.dc.html` | Edit host | The same form prefilled, with the delete prompt. |
| `Groups.dc.html` | Groups | Group colors and shared defaults, with the delete prompt. |
| `Terminal.dc.html` | Terminal | Session tabs, split panes, the output color scheme, broadcast input, find, the snippet drawer and the status bar. |
| `HostKey.dc.html` | Host key check | The first-connection prompt and, as a variant, the changed-key warning. |
| `ConnectStates.dc.html` | Connection states | Four panes: connecting, timed out (with "Try Tailscale"), sign-in refused, reconnecting. |
| `Sftp.dc.html` | SFTP | Two panes with breadcrumbs and filters, multi-select with an action bar, drag between panes, the transfer queue, and the permissions, rename, delete and conflict popups. |
| `Keychain.dc.html` | Keychain | Keys, identities, public key copy, import from a file, `~/.ssh` or WSL, and key generation. |
| `KnownHosts.dc.html` | Known hosts | Trusted server keys with search and filters, a detail panel, mismatch handling, forget. |
| `PortForwarding.dc.html` | Port forwarding | Local, remote and dynamic rules with on and off switches, and the new rule form. |
| `Snippets.dc.html` | Snippets | The snippet library and editor, variables, run options, multi-host run. |
| `History.dc.html` | History | Past sessions and saved output where logging is on. |
| `Settings.dc.html` | Settings | The new-release notice, terminal font and appearance with a live preview, vault, connection, SFTP, keyboard, and about and updates. |
| `Palette.dc.html` | Command palette | The palette open over the Hosts screen. |
| `FirstRun.dc.html` | First run | An empty vault with quick connect and OpenSSH config import. |
| `Locked.dc.html` | Vault locked | The Windows Hello unlock screen. |

Shared across screens: the title bar with a pinned vault tab and session tabs, the side navigation (a 56px icon rail on session screens), and the dot on the Settings entry when a newer release exists.
