# Milestone 2: Connect, working plan

Written 25 November 2026, after milestone 1 (`docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **vault on disk, hosts list and New host form, password and key sign-in, the host key prompt and Known hosts, one terminal tab.**

Done means you can install nothing, run `holonet.exe`, create a vault, add a host, connect to it, verify its key, use the terminal, and find the host and its key again after a restart. No `.dev/` files are involved.

## Scope

**In:**
- Vault: create, unlock and lock with a vault password, plus "Lock now".
- Hosts screen with cards and list views, detail panel and empty state.
- New host, Edit host, Duplicate, and the delete prompt with "Also forget its server key in Known hosts".
- Sign-in with a password (saved or asked on connect) or a key file copied into the vault (with its passphrase saved or asked).
- Host key prompt: first connection and changed key.
- Known hosts screen: list, filters, detail, copy, forget.
- One terminal tab built on milestone 1's view, with the drawn tab, pane header and status bar.
- Minimal connection states: connecting, failed, sign-in refused, session ended.
- "Test connection" on the host form.
- CI.

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| Groups and group defaults | 6 (hosts are ungrouped for now) |
| Tailscale address and route | 4 |
| Jump host | 4 |
| Keychain screen and key generation | 4 |
| Windows Hello and auto-lock | 4 |
| SSH agent sign-in | 4 |
| Startup snippets | 6 |
| Logging and History | 7 |
| Quick fill parsing | 7, with OpenSSH import |
| Import and export of `known_hosts` | 7 |
| Multiple tabs, splits and reconnect | 3 |
| SFTP | 5 |

Out-of-scope fields are not shown on the form. They are added when their milestone lands, so no control does nothing.

## Architecture

### `holonet-model` (new, no I/O)

Types: `Host`, `HostId`, `Auth`, `StoredKey`, `KnownHost`, `Settings` and `VaultData`. All are `serde` and carry a schema version.

| Type | Fields in milestone 2 |
| --- | --- |
| `Host` | `id`, `label` (optional; shown as the address), `address` (required), `port` (default 22), `username`, `auth`, `keep_alive` (default on), `tags`, `role_icon` (default server), `created_at`, `last_connected_at` |
| `Auth` | `Password { saved: Option<Secret> }` or `Key { key_id, passphrase: Option<Secret> }` |
| `StoredKey` | `id`, `name`, `algorithm`, the OpenSSH private key text exactly as imported (still encrypted if it had a passphrase), `public_key`, `fingerprint`, `source` path, `imported_at` |
| `KnownHost` | `host`, `port`, `key_type`, `public_key`, `trusted_at`, `last_matched_at`, `source` (FirstConnection or ImportedKnownHosts), `pending_mismatch: Option<{presented_key, seen_at}>` |
| `Settings` (vault part) | `unknown_host_keys` (Ask or Refuse), `connect_timeout` (10 s) |

The `KnownHost` fingerprint is derived from the public key, never stored. The linked host is derived by matching `address:port` against hosts.

### `holonet-vault` (new)

- **File:** `%APPDATA%\Holonet\vault.holonet`, plus `vault.holonet.bak` holding the previous good version.
- **Format:** a small binary header, then the encrypted body.
  - Header: magic `HOLONETV`, format version, and a list of **key slots**.
  - Body: XChaCha20-Poly1305 over `serde_json` of `VaultData`, with the whole header as associated data.
- **Keys:** a random 256-bit data key encrypts the body. Each key slot wraps that data key.
  - Milestone 2 has one slot type: vault password, then Argon2id (salt and parameters stored in the slot), then a key-encryption key, then the wrapped data key.
  - Milestone 4 adds a Windows Hello slot beside it, without re-encrypting anything.
  - Changing the password rewrites one slot.
- **Argon2id parameters:** tune to about 0.5 s on the Snapdragon (start at 64 MiB, t=3, p=1). They're stored per slot so they can be raised later.
- **Saves are atomic:** write `vault.holonet.tmp`, flush, then `ReplaceFileW` onto `vault.holonet`, keeping the old file as `.bak`. Saves happen off the UI thread, and a failed save shows an error without losing the in-memory state.
- **In memory:** the data key and plaintext secrets live in `zeroize` / `secrecy` types. Locking drops and wipes them. Per `decisions.md`, connected sessions keep running while locked.
- **API (sketch):**
  - `Vault::create(path, password)` and `Vault::open(path)` return a `LockedVault`.
  - `LockedVault::unlock(password)` returns an `UnlockedVault`. It fails with `WrongPassword`, `Corrupt` or `NewerFormat`.
  - `UnlockedVault::data()`, `update(|d| ...)` (which saves), `change_password`, and `lock()`.
- **Tests:** round trip; wrong password; tampered header or body is rejected; a newer format version is refused; the `.bak` fallback works when the main file is corrupt; parameters are read back from the slot.

### `holonet-ssh` (extend)

- **Host key verification becomes a callback.** `ConnectOptions` takes a `HostKeyVerifier`; russh's `check_server_key` awaits it.
  - The app's verifier looks the key up in the vault. A match continues silently.
  - An unknown key asks the UI and waits for "Trust and connect", "Connect once" or "Cancel".
  - A changed key stops the connection and records `pending_mismatch`. It never asks to continue, per `decisions.md`.
  - "Replace trusted key and connect" is a fresh connection after the replacement.
- **Sign-in:**
  - Password tries `password`, then `keyboard-interactive` (many servers allow only the second).
  - Keys are decoded from vault bytes with an optional passphrase.
  - A missing password or passphrase is requested through the same callback route as the host key prompt.
- **Typed errors** for the connection states: `Resolve`, `Refused`, `TimedOut` (10 s), `HostKeyRejected`, `SignInRefused { methods }`, `Disconnected`.
- **Session info** for the status bar: negotiated cipher and key exchange, and the sign-in method used.
- **Tests:** unit tests for verifier decisions, plus integration tests against the WSL test server, ignored unless `HOLONET_TEST_SERVER=1`.

### `holonet-app` (grow)

- **App shell:**
  - Title bar: brand, pinned vault tab, one session tab, "+", window buttons. The window is undecorated so the drawn title bar replaces Windows' own; check snapping and dragging on ARM.
  - Side navigation, a router between views, and the icon rail on the session view.
- **Views:**
  - Create vault, Locked (password)
  - Hosts (cards, list, detail, empty)
  - New host / Edit host and the delete dialog
  - Known hosts
  - Session: milestone 1's terminal plus the tab, pane header, status bar, connection states and host key prompt
  - Settings: only the two settings above, plus "Change vault password"
- **Assets:** Inter and Space Grotesk bundled via `add_fonts` (both under the SIL Open Font License). The stroke icons used in the screens are extracted from the `.dc.html` sources as SVG.

### Controls to build first

From the design guidelines and the screens:
- Button in five kinds: primary, secondary, quiet, destructive outline, warn fill
- Text field: single line, mono variant, password mask with show/hide, placeholder, error text
- Select with popup
- Segmented control
- Checkbox
- Tag
- Dialog with backdrop, and the warning variant
- Fieldset with legend
- Key-value table
- List table with header
- Host card
- Side nav item
- Breadcrumb
- Callout / info box

`build-plan.md` names interface build time as the largest risk and says to build the shared controls first, then one screen, before estimating the rest. Phase 3 is that checkpoint.

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **CI**: GitHub Actions runs `cargo build`, `test` and `clippy -D warnings`, and `cargo fmt --check`. The free `windows-11-arm` runner was limited to public repositories when last checked (2025) and this repository is private. Check its availability; otherwise use `windows-latest` (x64) for build, test and lint, and do ARM64 checks locally. | Green on the milestone 2 PR |
| 1 | **Model and vault** crates, headless | Vault tests pass. Unlock time measured on the Snapdragon. |
| 2 | **SSH extensions**: verifier callback, password plus keyboard-interactive, key from bytes, typed errors, session info | Integration tests pass against WSL for every sign-in method, unknown and changed keys, and wrong password |
| 3 | **Controls and shell**: tokens module, fonts, icons, the controls above, title bar and side navigation, plus a hidden `--gallery` view showing every control in every state | Gallery screenshots match the guideline drawings. **Re-estimate the rest here.** |
| 4 | **Vault flow**: create vault (first start), Locked with password unlock, wrong password, "Lock now", change password | Fresh start → create → lock → wrong password → unlock, with data surviving a restart |
| 5 | **Hosts and host form**: Hosts cards, list and detail, empty state, search filter; New and Edit host, Duplicate, delete dialog; key-file import into the vault; Test connection | Add, edit, duplicate and delete hosts, persisted; Test connection reports reachability and the fingerprint |
| 6 | **Session**: one tab with the drawn chrome, connecting and failed states, the host key prompt (both variants), password and passphrase prompts, status bar | Connect to WSL by password and by key, trust the key, reconnect silently; a changed key is stopped and recorded |
| 7 | **Known hosts** screen: table, filters, detail panel, copy fingerprint and public key, forget (with a confirmation), "Review mismatch" | Forgetting a key brings the prompt back on next connect; "Review mismatch" shows the changed-key view |
| 8 | **Wrap-up**: remove `holonet_ssh::dev` and the `--ssh` flag, update `CLAUDE.md` and notes, open the PR | Done check at the top of this file, from a clean `%APPDATA%` |

## Undrawn states I will draw in the design system's style

The screen reports list these as missing. Each one will be built from existing tokens and components and shown in screenshots for review:

- **Create vault (first start):**
  - password and confirm fields
  - a minimum length of 8
  - the warning: "If you forget this password, the vault cannot be recovered."
- **Unlock and lock:**
  - password unlock with a wrong-password error and a short delay after repeated failures
  - "Lock now" in the side navigation's vault card
  - Locked body text for a manual lock
- **Hosts:**
  - no hosts yet: FirstRun's layout, without the import cards until milestone 7
  - no search results
  - nothing selected
- **Prompts:**
  - password on connect, with "Save in the encrypted vault"
  - key passphrase, with "Save passphrase in the encrypted vault"
- **Connection failures:** could not resolve, connection refused, timed out, session ended, refused by "Refuse to connect". The tab gets an amber state dot.
- **Confirmations:** forgetting a key in Known hosts.
- **Test connection:** in progress and failed.

## Defaults I will follow unless you say otherwise

- **Host key prompt placement:** as drawn, centred inside the session pane, not a window-wide modal, so other tabs stay usable later. Button order and sizes follow the HostKey screen where it differs from `Dialog.md`.
- **Fingerprints:** OpenSSH style (`SHA256:` plus unpadded base64), with the key type shown first in every place.
- **Hosts with no group:** an `ink-faint` edge and square, labelled "No group".
- **Settings "Lock vault after":** split into an idle-time select plus a separate "Lock when Windows locks" switch, matching `decisions.md`. Only the select and "Lock now" work before milestone 4.
- **Controls:** 44px where `decisions.md` says so. The drawn 48 and 52px search and quick-connect fields shrink to 44px; title-bar and toolbar controls stay 36px.
- **Off-token values:** the few drawn ones (`#4A4A4A`, `#161616`, the 0.45-alpha tab edge) become named tokens in code.

## Risks

- **Text input in GPUI.** Caret, selection, clipboard, IME and password masking are the costliest single control. Start from GPUI's own `examples/input.rs` (and see decision 1).
- **Undecorated window on Windows ARM.** Dragging, snapping, maximize and the DPI change between monitors need checking early in phase 3.
- **Argon2 cost on ARM.** If it is much faster or slower than expected, tune the parameters in phase 1 rather than later.
- **Losing the vault.** A crash during a save must never corrupt it. The atomic replace and `.bak` handle that, but test it with a killed process mid-save.

## Owner decisions (25 November 2026)

1. **Control library: our own controls on `gpui` 0.2.2.**
   - Start from GPUI's `examples/input.rs`. Read `gpui-component` (Apache-2.0) for approaches, with attribution if any code is adapted.
   - Not chosen: `gpui-component`, which requires `gpui-pre` 0.3.14, a pinned snapshot of Zed's main branch.
   - Revisit if the text field takes more than a few days in phase 3.
2. **Key passphrases: keep the key exactly as imported.** It stays encrypted with its own passphrase. Offer "Save passphrase in the encrypted vault"; otherwise ask on connect.
3. **Vault password: at least 8 characters**, no other rules, no recovery. Daily unlock moves to Windows Hello in milestone 4.
