# Milestone 6: Power features, working plan

Written 26 November 2026, after milestone 5 (`docs/milestone-5-plan.md`, results in `docs/prototype-notes.md`). Scope from `docs/build-plan.md`: **port forwarding including dynamic, snippets with variables and multi-host runs, broadcast input, groups and their defaults.** It also takes the items earlier milestones moved here: **"SSH agent" sign-in** (its place on the host form went to Identity in milestone 4) and the host form's **Agent forwarding** and **Compression** switches.

The designs are:
- `PortForwarding.dc.html` and `Snippets.dc.html`
- `Groups.dc.html`
- the Broadcast strip and Snippets drawer in `Terminal.dc.html`
- the Group, Startup snippet, Agent forwarding and Compression fields in `NewHost.dc.html`
- the group list in `Main.dc.html`

Done means:
- **Groups:**
  - You can create, recolour and delete groups, put hosts in them, and give a group defaults: key, jump host, route and the broadcast warning.
  - A host uses its group's defaults unless it sets its own, and the host form labels inherited values "from group".
  - Group colours show on host cards, tabs and the top edge of panes.
- **Port forwarding:**
  - Local, remote and dynamic (SOCKS5) rules run through a host and can be switched on and off.
  - A rule opens its host connection in the background if none is open. It can start when the host connects.
  - It survives a dropped connection by reconnecting.
  - The terminal's status bar lists the host's active forwards.
- **Snippets:**
  - A snippet library with `${name}` variables that ask for a value on every run, "Ask before running" and "Press Enter after inserting".
  - Snippets run from the drawer in a terminal, on several hosts at once, and as a host's startup snippet.
- **Broadcast input:** typing goes to every pane in the tab, under an amber strip. Turning it on asks once when a host in a "warn" group is included.
- **Sign-in:** "SSH agent" signs in with the keys in Windows' OpenSSH agent, and a host can forward the agent and use compression.

## Scope

**In:**

- **Groups** (`Groups.dc.html`), opened from "Manage groups" on the Hosts page:
  - A list of group cards, each with its name, host count, key, jump host and route.
  - **The editor:** name, a colour from the drawn six (Cyan, Violet, Mint, Gray, Rose, Lime), and the defaults: Key ("Ask each time" or a key), Jump host, Connect using, and "Warn before broadcast or multi-host snippets".
  - **New group** and **Delete group**, with "Also delete its n hosts" (`decisions.md`). Deleting the hosts keeps their server keys in Known hosts.
- **Groups on the Hosts page** (`Main.dc.html`):
  - The group list in the left column ("All hosts", then each group with its count) filters the hosts.
  - Cards and list rows show the group's colour and name instead of "No group".
- **Host form:**
  - **Group** select.
  - **Inherited values:** Key, Jump host and Connect using show the group's value, labelled "from group" (not drawn), until changed. "Use group default" returns to it.
  - **Startup snippet:** a select from the library (`decisions.md`).
  - **Agent forwarding** and **Compression** switches.
  - **SSH agent** as an Authentication choice, next to SSH key, Password and Identity.
- **Port forwarding** (`PortForwarding.dc.html`), a page in the side navigation:
  - **Rule cards:**
    - The type tag (Local in cyan, Remote in violet, Dynamic in mint) and the name.
    - The note ("Starts with prod-web-01" or "Manual start").
    - The route as drawn: "This PC 127.0.0.1:5432 → prod-web-01 → db-primary:5432".
    - The status ("Active" or "Stopped") and the on/off switch.
  - **New rule:**
    - Type, Label, Through host, Bind address, the port, and the destination (not for Dynamic).
    - The equivalent `ssh -N -L …` command.
    - "Start when this host connects", then Save rule.
  - **Edit and Delete** for each rule (not drawn: the card opens the same form, filled in, with Delete under it).
  - **States not drawn:**
    - Starting
    - "Port 5432 is already in use on this PC"
    - "The host refused the remote forward"
    - Reconnecting
    - The host's sign-in prompts when a rule starts with no open session
- **Forwards in sessions:**
  - The terminal's status bar shows the host's active forwards ("Forward L 5432 → db-primary:5432", as drawn), and "Agent forwarding on" or "off".
  - Forwards stay up while the vault is locked (`decisions.md`).
- **Snippets** (`Snippets.dc.html`), a page in the side navigation:
  - **The library** with a filter: name, command and the run note ("Runs immediately", "Inserts without running", or "Asks before running" in amber).
  - **The editor:**
    - Name and Command, with the note "Holonet asks for service each time this runs. Write a variable as ${name}."
    - "Press Enter after inserting" and "Ask before running".
    - **Run on:** host chips, with "Run in active session", "Run on 1 host" or "Run on n hosts".
    - The startup note ("Startup snippet for staging-api.").
    - Save, Duplicate and Delete.
  - Deleting a snippet that is a host's startup snippet says so and clears it from those hosts.
- **Terminal** (`Terminal.dc.html`):
  - **Snippets drawer:** opened with the header's Snippets button. It lists the library with "Runs in active pane" (or "Runs in both panes" while broadcasting) and "Manage snippets".
  - **Variable prompt** (not drawn): a popup with one field per `${name}`. It remembers the last value per snippet until the app closes.
  - **Ask before running** (not drawn): "Run Restart service on prod-web-01?" with the command shown.
  - **Startup snippet:** sent once, after the shell first opens. It is not sent again after a reconnect, since commands are never replayed (`decisions.md`).
- **Multi-host run:**
  - "Run on n hosts" opens a tab with a pane for each host, up to four per tab, in more tabs past four.
  - Each pane runs the snippet once its shell opens.
  - A host in a "warn" group asks once first: "This runs on 2 Production hosts. Run anyway?".
- **Broadcast input** (`Terminal.dc.html`):
  - **The header's Broadcast button** turns it on for the panes of the current tab.
  - **While on:**
    - The amber strip: "Broadcast is on. Everything you type goes to both panes, including 1 Production host." with Turn off.
    - Amber pane edges.
    - Keys, pastes and snippets go to every connected pane.
  - **Including a "warn" group asks once:** "Broadcast to 2 panes, including 1 Production host?".
  - It turns off when the tab has one pane left, and with Ctrl Shift B (new shortcut, Ctrl Shift like the others).
- **Side navigation:** Port forwarding and Snippets join the vault entries, as drawn.

**Out, and where it goes:**

| Item | Milestone |
| --- | --- |
| "Save terminal output to History" on the host form, and History | 7 |
| "Quick fill" (paste an `ssh` command or config entry) on the host form | 7, with OpenSSH config import |
| Command palette entries for snippets and rules | 7, with the palette |
| Forwarding Unix sockets, X11 forwarding | Later, if wanted |
| Running snippets on hosts without opening panes (background output) | Later, if wanted |

No control is shown before its milestone lands.

## Architecture

### `holonet-model`
- **`Group { id, name, color, key, jump_host, route, warn }`.** The key is `Option<KeyId>`, where `None` means "Ask each time".
- **`Host` gains:**
  - `group: Option<GroupId>`, `startup_snippet: Option<SnippetId>`, `agent_forwarding: bool` and `compression: bool`
  - `Auth::Agent` (added in phase 8 with the agent sign-in, so no unreachable sign-in path exists before then)
- **Inheritable host fields:** route, jump host and key become "own value or from group". The host keeps its own values, and `from_group` marks which ones follow the group. The resolved values come from one function, the existing `VaultData::resolved(host)`, which the connection code and the host form both use.
- **`Snippet { id, name, command, press_enter, confirm }`**, with `variables()` parsing `${name}` (word characters only; `$${x}` writes `${x}` literally).
- **`ForwardRule { id, name, kind: Local | Remote | Dynamic, host, bind_address, port, destination_host, destination_port, start_with_host }`.**
- **Schema 3.** The migration marks route and jump host as "from group" where they hold the defaults (Automatic, none). Keys stay explicit, since a host without a group has no default key. A test decodes a milestone 5 vault.

### `holonet-ssh`
- **`forward` module:**
  - **Local:** a Tokio listener on this PC. Each connection opens `channel_open_direct_tcpip` and copies both ways.
  - **Remote:** `tcpip_forward` on the host. The client handler routes `forwarded-tcpip` channels to the rule's destination on this PC by the bound port.
  - **Dynamic:** a SOCKS5 server on this PC. It supports CONNECT with no authentication, and IPv4, IPv6 or domain names. The host resolves domain names.
  - Each rule reports Starting, Active (with its connection count), Failed (with the reason), and Stopped.
- **Agent:**
  - Sign-in through Windows' OpenSSH agent: `AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")`, offering each of the agent's keys.
  - Agent forwarding: `agent_forward` on the shell channel. The handler bridges the host's agent channels to the same pipe.
- **Compression:** `zlib@openssh.com` first when the host asks for it, otherwise `none` first. russh's `flate2` feature is already on.
- **Startup and run text:** writing a snippet to a shell is ordinary input, with `\r` when "Press Enter after inserting" is on.

### `holonet-app`
- **`forwards.rs`, a background connection per host with active rules:**
  - Shared by that host's rules and kept apart from terminal and SFTP connections.
  - It uses `hops.rs` (route plan, jump hosts, host key check, prompts) and reconnects after a drop as panes do.
  - It reuses secrets typed for that host in a pane.
  - It is closed when the host's last rule stops.
  - Rules with "Start when this host connects" start when a terminal or SFTP tab to that host connects.
  - Prompts from a rule started on the Port forwarding page appear on that page.
- **Pages:** `workspace/groups.rs`, `workspace/forwarding.rs` and `workspace/snippets.rs`.
- **The tab:** the snippet drawer, the variable and confirmation popups, and broadcast, which sends each key and paste to every connected pane's input.
- **Controls to add, each in `--gallery`:**
  - the switch (on/off toggle, drawn on rule cards)
  - colour swatches
  - the type tag (Local, Remote, Dynamic)
  - the rule card
  - host chips (Run on)
  - the amber broadcast strip

## Phases

Each phase ends with a commit and a check.

| # | Phase | Exit check |
| --- | --- | --- |
| 0 | **Spikes** *(done; all three kinds of forward, agent sign-in and forwarding, and broadcast work)*: (a) local, remote and dynamic forwards against the WSL server, with throughput and 50 parallel connections, and `curl --socks5-hostname` through the SOCKS server; (b) sign-in through Windows' OpenSSH agent and agent forwarding (`ssh-add -l` on the host); (c) broadcast to four panes: typing latency and order | Notes record each result |
| 1 | **Model** *(done)*: groups, snippets, rules, the new host fields, inheritance in `resolved()`, schema 3 and its migration | A milestone 5 vault opens; unit tests for inheritance and `${name}` parsing |
| 2 | **Groups** *(done)*: the Groups page, the Hosts page's group list and colours, the host form's Group and "from group" values, colours on tabs and panes | Two groups with different keys and jump hosts; hosts inherit, override and return to the default, and connect with what they show |
| 3 | **Forwarding engine** *(done)* in `holonet-ssh`: local, remote, dynamic, status and errors | WSL tests for each kind, a port already in use, and a refused remote forward |
| 4 | **Port forwarding page and background connections:** cards, the form, switches, start with host, reconnect, the status bar | One rule of each kind used from this PC (a browser through SOCKS); a killed connection comes back with the rule still active |
| 5 | **Snippets page and startup snippets:** library, editor, variables, the host form's Startup snippet | A startup snippet runs once on connect and not after a reconnect |
| 6 | **Snippets in sessions and multi-host runs:** the drawer, the variable and confirmation popups, Run on n hosts, the warn-group question | A snippet with a variable runs in a pane, then on three hosts in one tab |
| 7 | **Broadcast:** the button, the strip, amber edges, the warn question, Ctrl Shift B | Typing and a paste reach all four panes in order; a closed pane drops out |
| 8 | **SSH agent, agent forwarding and compression** | Sign in with a key held only by the agent; `ssh-add -l` on the host lists it; compression is negotiated |
| 9 | **Wrap-up:** notes, `CLAUDE.md`, gallery, the done check from a clean profile, PR | Done check passes |

## States to design

The drawings show the populated pages. These are not drawn:
- **"from group" values** on the host form, and "Use group default".
- **Empty states:** no groups, no rules, no snippets, and a snippet filter with no matches (drawn: "No snippets match this filter.").
- **Rule states:** Starting, a port in use, a refused remote forward, a host that needs a password typed, and Reconnecting.
- **Popups:**
  - the variable prompt
  - "Ask before running"
  - the warn-group questions for broadcast and multi-host runs
  - deleting a snippet in use as a startup snippet
  - deleting a rule
- **The Snippets drawer with no snippets** ("No snippets yet. Manage snippets").
- **"SSH agent" when the agent is not running:** "Windows' OpenSSH agent is not running. Start the ssh-agent service, or choose another sign-in."

## Defaults I will follow unless you say otherwise

- **Rules bind to 127.0.0.1** by default. Another bind address (such as 0.0.0.0) is allowed and labelled "Reachable from other devices on your network" in amber.
- **Rules stop when Holonet closes** and are off at the next start, except those set to start with their host.
- **One background connection per host** carries all of its active rules.
- **The broadcast warning** counts hosts in groups marked "warn" (`decisions.md`), and asks once per time it is turned on.
- **Multi-host runs** use each host's own sign-in. A host that needs a password asks in its pane.
- **Variable values** are remembered per snippet until Holonet closes, never saved to the vault.
- **Group colours** are the six drawn. Gray is the colour of "No group" too, so a gray group is always shown with its name.

## Risks

- **Windows' OpenSSH agent** *(settled in phase 0)*: the owner set the `ssh-agent` service to Manual and started it. A stopped agent gives `NotFound` on the pipe. The agent returns keys without comments, so Holonet names agent keys by type and fingerprint.
- **Remote forwards and `GatewayPorts`.** OpenSSH binds remote forwards to the host's loopback unless `GatewayPorts` allows more. The form says so when the bind address is not loopback.
- **Many connections through one session.** Each forwarded connection is a channel. Spike (a) measures 50 at once.
- **Broadcast and focus.** Input today goes to the focused pane's terminal. Broadcast copies the encoded bytes to the others, so keys that the terminal maps (arrows, Ctrl keys) must be encoded once per pane's mode.
- **Inherited values that the host form shows** must match what connecting uses. A single `resolved()` used by both avoids drift.

## Owner decisions (26 November 2026)

1. **Forward connections:** each host with active rules gets its own background connection, shared by its rules and separate from terminal and SFTP tabs. It reuses secrets typed in a pane, and closing a terminal never stops a forward.
2. **"Run on n hosts"** opens a tab with a pane per host (up to four per tab, more tabs past four) and runs the snippet in each as its shell opens.
3. **SSH agent sign-in and agent forwarding are in.** The owner sets the `ssh-agent` service to Manual and starts it before spike (b); Holonet never changes Windows services.
4. **Broadcast** reaches the panes of the current tab only, as drawn.
