# Milestone 4: Keys and routes, working plan

Written 25 November 2026, after milestone 3 (`docs/milestone-3-plan.md`, results in `docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **the keychain with generate and import, Windows Hello unlock and auto-lock, jump hosts, and the primary to Tailscale fallback.** By owner decision it also takes two drawn Keychain extras: **Install on host** and **Identities**.

Done means:
- You can generate an Ed25519 key in the Keychain, install it on a host, and sign in with it.
- You can import keys from a file, from `~/.ssh` in the Windows profile, and from `~/.ssh` in a WSL distribution.
- A host with a Tailscale address connects over Tailscale when its primary address does not answer, and the pane says which route it used.
- A host behind a jump host connects through it, and both host keys are checked.
- The vault locks after the idle time and when Windows locks, and Windows Hello unlocks it.

## Scope

**In:**
- **Keychain** (`Keychain.dc.html`), a new side-navigation entry:
  - the Keys table: name and fingerprint, type, "Protected by", and the number of hosts that use the key
  - "Import key file", "Import from ~/.ssh", "Import from WSL". Keys are copied into the vault (`decisions.md`). OpenSSH, PEM and PuTTY `.ppk` files are read.
  - "Generate key": name, type (Ed25519, RSA 4096, ECDSA P-256), optional passphrase, "Unlock with Windows Hello"
  - the public key panel with Copy and **Install on host**
  - rename and delete. Deleting a key that hosts use asks first and names them.
- **Identities:** named sign-ins (a username plus a key or a saved password) that many hosts can share. Shown as cards in the Keychain as drawn, with create, edit and delete.
- **Routes** (`NewHost.dc.html`, `Main.dc.html`, `ConnectStates.dc.html`):
  - a "Tailscale address (optional)" field, and "Connect using": Automatic, Primary or Tailscale, with the drawn help text
  - Automatic tries the primary address for the fallback wait (3 s, adjustable in Settings), then Tailscale. The wait is skipped when the primary address is private and this PC has no route to it (`decisions.md`).
  - Tailscale is checked before it is offered. When it is not running, the pane says so instead of timing out.
  - the connecting, timed-out ("Try Tailscale") and reconnecting wording as drawn, and "Connected over Tailscale" in the status bar and host detail
  - the "Tailscale" badge on host cards
- **Jump hosts:** a "Jump host" select of saved hosts on the host form, `-J` in the equivalent command, and the Jump host row in the host detail. The jump host uses its own sign-in, route and host key. A jump host may have its own jump host; loops are refused when saving.
- **Lock and unlock** (`Locked.dc.html`, Settings, Vault and security):
  - lock after the idle time, and when Windows locks
  - "Unlock with Windows Hello", with "Use vault password instead"
  - the Settings switch "Unlock with Windows Hello" and the lock settings
  - sessions stay connected while locked and reappear on unlock, as in milestone 2

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| "SSH agent" sign-in and "Add to Windows OpenSSH agent" | 6 |
| Group defaults for key, jump host and route | 6 |
| Exporting a private key | 7 |
| "Vault backup: Export encrypted file" | 7 |
| Quick fill from an `ssh` command or `~/.ssh/config` entry | 7, with the config import |
| OpenSSH certificates | Later, if wanted |
| Hardware (FIDO) security keys | Not in the first release (`decisions.md`) |

As before, no control is shown before its milestone lands.

## Architecture

### `holonet-model`
- `Host` gains `tailscale_address: Option<String>`, `route: Route` (Automatic, Primary or Tailscale), `jump_host: Option<HostId>` and `identity: Option<IdentityId>`. When `identity` is set, the username and sign-in come from the identity.
- `Identity { id, name, username, auth: Auth }`.
- `StoredKey` gains `comment`, and its saved passphrase moves here from each host (`saved_passphrase: Option<Secret>`), because a passphrase belongs to the key. Old vaults are migrated on open: a passphrase saved on any host moves to its key.
- `Settings` gains `lock_when_windows_locks` (on), `hello_unlock` (on when Hello is available), and `connection.fallback_wait` (3 s).
- `VaultData` gains `identities`. Everything uses `#[serde(default)]`, and a test decodes a milestone 3 vault.
- `jump_chain(host)` returns the hops in order and refuses a loop.

### `holonet-vault`
- A **Windows Hello key slot** beside the password slot. Phase 0 showed Hello signatures repeat on this PC, so this is the design:
  - Holonet creates a Hello key credential named for the vault, signs a fixed random challenge stored in the slot, and uses the SHA-256 of the signature to wrap the data key (the KeePassXC approach).
  - Turning Hello off removes the slot. The password slot always stays.
- **If signatures ever stop repeating** (the owner's fallback): no slot in the file. The vault password unlocks once per app start, and the data key, wrapped under a Hello-derived key, stays in memory, so every unlock after an auto-lock uses Hello. This is how KeePassXC does it.

### `holonet-ssh`
- **Key generation:** Ed25519, ECDSA P-256 and RSA 4096 with `ssh-key` (re-exported by russh), encrypted with bcrypt-pbkdf when a passphrase is given. Runs off the UI thread.
- **Key import:** `decode_secret_key` already reads OpenSSH, PEM and PPK. `.ppk` keys are stored converted to OpenSSH format, keeping their passphrase.
- **Jump hosts:** connect to the first hop, open a `direct-tcpip` channel to the next one, and run the next session over it with `connect_stream`. Each hop's host key is checked against its own address. The outer sessions stay open for as long as the inner one, and they close in reverse order.
- **Install on host:** over an existing sign-in, run one command that creates `~/.ssh` (mode 700) and appends the public key to `authorized_keys` (mode 600) unless it is already there.
- **Connect to an address:** `connect` takes the address chosen by the route, so the host key check uses the address actually connected to.

### `holonet-net` (new)
- **Tailscale status** from the LocalAPI on `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled` (`GET /localapi/v0/status`, opened with identification-level impersonation). Falls back to `tailscale.exe status --json`. Reports running or not, and this PC's own 100.x addresses.
- **Route planning:** for each connect, the ordered list of addresses to try and how long to wait on each. `GetBestRoute2` decides whether a private primary address has a route.
- No window and no GPUI, so it tests without one.

### `holonet-app`
- **Keychain screen** (`workspace/keychain.rs`), with the Generate side panel, import pickers and the Install on host popup.
- **`platform.rs`** gains:
  - Windows lock and unlock: a hidden message-only window on its own thread, registered with `WTSRegisterSessionNotification`, sending events into the same channel as sleep and network changes
  - Windows Hello: create, sign, and bring the prompt to the front
- **Idle time** counts Holonet's own keyboard and mouse input, not the whole PC's.
- **Panes** connect through the route plan and the jump chain. Prompts for a jump host (its key, password or passphrase) appear in the same pane, naming that host.

### Controls to add
- **Table** for the Keys list, with a selected row.
- **Side panel** for Generate key.
- **Identity card**.
- **Multi-select list** for importing several keys from `~/.ssh` or WSL at once.
- Each is added to `--gallery` with its states.

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **Spikes** *(done; Hello signatures repeat, so the vault gets a Hello key slot; WSL cannot be reached on this PC's Tailscale address)*: (a) Windows Hello from GPUI: create a key credential, sign the same challenge twice and compare, and check that the prompt opens in front on ARM; (b) Windows lock and unlock events from a message-only window; (c) RSA 4096 generation time, debug and release; (d) Tailscale LocalAPI over the pipe from Rust; (e) a jump host through WSL sshd to itself; (f) whether WSL sshd answers on this PC's Tailscale address | Notes record each result; Hello's outcome picks the vault design above |
| 1 | **Model and vault** *(done)*: the new fields, identities, passphrase migration, Hello slot or in-memory wrap, lock settings | A milestone 3 vault opens and migrates; unit tests cover jump loops and the Hello slot round trip (with a fake signer) |
| 2 | **SSH** *(done; PuTTY files checked against PuTTY 0.83's own puttygen)*: key generation, `.ppk` import, jump-host connect, install public key, connect to a chosen address | WSL tests: sign in with each generated key type, through a jump host, and after install on host |
| 3 | **`holonet-net`** *(done)*: Tailscale status, route planning, the no-route check | Unit tests for planning; live check with Tailscale running and stopped |
| 4 | **Keychain** *(done)*: table, generate, three imports, copy, rename, delete, install on host, side navigation | Generate, install and sign in end to end; import from a file, `~/.ssh` and WSL |
| 5 | **Identities** *(done)*: cards, create, edit, delete, choosing one on the host form | Two hosts share an identity; editing it changes both |
| 6 | **Routes** *(done; tested through a relay on this PC's Tailscale address)*: host form, detail and cards, the connecting, timed-out, reconnecting and Tailscale-not-running states, the fallback wait setting | Unreachable primary falls back to Tailscale; Primary-only shows "Try Tailscale"; Tailscale stopped is reported |
| 7 | **Jump hosts in panes:** host form select, `-J` command, prompts per hop, reconnect through the chain | Connect through a jump host from a clean Known hosts: two key prompts, then a shell; drop and reconnect |
| 8 | **Lock and Windows Hello:** idle and Windows-lock locking, the Locked screen, Hello unlock and the password fallback, the Settings switch | Idle lock, Windows lock (Win L) and Hello unlock, each with a session open that survives |
| 9 | **Wrap-up:** notes, `CLAUDE.md`, the done check above from a clean profile, PR | Done check passes |

## Undrawn states I will draw in the design system's style

- **Keychain:**
  - an empty vault ("No keys yet", with the import and generate actions)
  - import pickers listing the keys found, with ones already in the vault marked and unreadable files explained
  - the passphrase prompt for an encrypted key when it is imported
  - "Copied" feedback, rename, and the delete prompt naming the hosts and identities that use the key
  - Install on host: pick a host, then progress, done, or the error
  - Generate in progress (RSA can take a few seconds)
- **Identities:** the create and edit form, the empty state, and the delete prompt. On the host form, "Identity" is an Authentication choice, in the place of the "SSH agent" segment, which waits for milestone 6.
- **Routes:**
  - Tailscale not running, as a failed state with the reason and Retry
  - both routes failed under Automatic, naming both addresses
  - the "Fallback wait" row in Settings, Connection (2, 3, 5 or 10 seconds)
- **Jump hosts:** connecting through a jump host ("Connecting to bastion-east, then app-01"), and which hop a failure or prompt belongs to.
- **Lock:**
  - the vault password field on the Locked screen
  - Hello cancelled or failed, and Hello not set up on this PC (the Settings switch is disabled with the reason)
  - turning Hello on (asks for the vault password first)
  - the Locked text for each reason: idle time, Windows locked, or Lock now

## Defaults I will follow unless you say otherwise

- **Lock settings:** an idle-time select (15 minutes, 1 hour, Never) and a separate "Lock when Windows locks" switch, as `decisions.md` says and the milestone 2 plan already chose. The design merges them into one select, which cannot do both.
- **Fallback:** Automatic falls back to Tailscale by itself after the wait, as the design's help text says. "Try Tailscale" in the timed-out pane is for Primary-only hosts that have a Tailscale address, which is how the drawn example reads.
- **Timed out:** the fallback wait applies to the primary address under Automatic. The last address tried still uses the connect timeout (10 s).
- **Equivalent command:** shows the primary address under Automatic, as drawn, and `-J` with the jump host's label.
- **"Protected by":** "No passphrase" (`warn`), "Passphrase", or "Passphrase + Windows Hello" when the passphrase is saved in the vault and the vault unlocks with Hello. "Unlock with Windows Hello" on Generate means "save the passphrase in the vault".
- **RSA:** 4096 bits only, as drawn.
- **Generated key comment:** `holonet@<computer name>`, editable on the key afterwards.
- **WSL import:** lists installed distributions with `wsl.exe -l -q` and reads `~/.ssh` through `\\wsl.localhost\<distro>\home\<user>\.ssh`. Nothing in WSL is changed.
- **Install on host:** never removes or rewrites existing lines in `authorized_keys`.
- **Idle:** only input in Holonet counts. Output arriving in a terminal does not keep the vault unlocked.

## Risks

- **Hello signatures.** Windows documents the key as RSA-PSS, which is usually randomized. If two signatures differ, the Hello slot is impossible, and the in-memory approach is the fallback (owner decision).
- **Hello prompt focus.** It is reported to open behind the app window. KeePassXC and Chromium find the "Credential Dialog Xaml Host" window and bring it forward; phase 0 checks whether that is needed on ARM.
- **`windows` crate APIs.** The newer `...ForWindowAsync` Hello calls need Windows SDK 26100 metadata. If the crate lacks them, use the plain calls and the focus workaround.
- **RSA generation** in pure Rust may take many seconds in debug builds. Generation runs off the UI thread, and the RSA crates can be optimised in the dev profile.
- **Testing Tailscale.** This PC has Tailscale 1.104 (native ARM64), but the test server is WSL on the same PC. Phase 0 checks whether WSL sshd can be reached over this PC's Tailscale address; if not, the fallback is tested with a Tailscale address that points at another device you name.
- **Jump host reconnect.** A drop on the jump host ends every session through it. Reconnect restarts the whole chain from the first hop.

## Owner decisions (25 November 2026)

1. **Windows Hello:** phase 0 tests whether Hello signatures repeat. If they do, add a Hello key slot to the vault file, so Hello unlocks even straight after starting the app. If not, the vault password unlocks once per app start and Hello unlocks after every auto-lock while Holonet runs.
2. **Keychain extras:** milestone 4 also builds Install on host and Identities. The SSH agent and key export wait for later milestones.
