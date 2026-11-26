//! The Holonet window: the vault gate (create or unlock), the workspace, and
//! session tabs, each holding one or more panes.
//!
//! Tabs belong to the app, not the workspace, so locking the vault (which
//! drops the workspace and its unlocked vault) leaves them connected. They
//! reappear when the vault is unlocked (decisions.md).
//!
//! The vault locks after the idle time (only input in Holonet counts, not
//! terminal output) and when Windows locks, as Settings says.
//!
//! App shortcuts are actions bound in the "HolonetApp" context. GPUI matches
//! bindings before key-down handlers, so they win over the terminal, and
//! they all use Ctrl Shift (or Ctrl Tab) so shell keys still reach the shell.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, IntoElement, KeyBinding, Pixels, Render, ScrollHandle, SharedString, Window, actions,
    div, prelude::*, px,
};
use holonet_model::{HostId, KnownHostSource, Mismatch, Timestamp, VaultData};
use holonet_ssh::{PresentedKey, classify};
use holonet_vault::UnlockedVault;

use crate::host_picker::{HostPicker, HostPickerEvent};
use crate::session::{KeyCheck, Limits, Pane, PaneMode, Remembered, SessionEvent};
use crate::shell::{TabInfo, TabKind, TitleBar};
use crate::tab::{Axis, Tab, TabEvent};
use crate::terminal::GridSize;
use crate::theme;
use crate::ui::{Button, ButtonKind, dialog, dialog_buttons, icon, modal};
use crate::vault_screens::{CreateVault, LockReason, Locked};
use crate::workspace::{Page, Workspace};

actions!(
    holonet,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        NextPane,
        PreviousPane,
        Find,
        Broadcast,
        OpenPalette,
        AddHost,
        LockNow
    ]
);

const CONTEXT: &str = "HolonetApp";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-d", SplitRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-f", Find, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-b", Broadcast, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-p", OpenPalette, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-n", AddHost, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-l", LockNow, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-e", SplitDown, Some(CONTEXT)),
        // With Shift held, Windows layouts report ] and [ as } and {.
        KeyBinding::new("ctrl-shift-]", NextPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-}", NextPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-}", NextPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-[", PreviousPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-{", PreviousPane, Some(CONTEXT)),
        KeyBinding::new("ctrl-{", PreviousPane, Some(CONTEXT)),
    ]);
}

enum Stage {
    Create(Entity<CreateVault>),
    Locked(Entity<Locked>),
    Open(Entity<Workspace>),
}

pub struct HolonetApp {
    path: PathBuf,
    stage: Stage,
    /// Tokio runtime where SSH work runs.
    runtime: tokio::runtime::Handle,
    tabs: Vec<Entity<Tab>>,
    /// The session tab shown, or `None` for the vault view.
    active: Option<usize>,
    /// The new-tab host picker, while open.
    picker: Option<Entity<HostPicker>>,
    /// The command palette, while open.
    palette: Option<Entity<crate::palette::CommandPalette>>,
    /// An SFTP tab waiting for "Close anyway?" (transfers still running).
    closing: Option<Entity<Tab>>,
    /// Where the "+" button was drawn, to place the picker under it.
    plus_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Horizontal scroll of the tab strip, so the active tab stays in view.
    tab_scroll: ScrollHandle,
    /// A stopped pane whose "Edit host" opened the host form: "Save and
    /// connect" retries there instead of opening another tab.
    edit_origin: Option<gpui::WeakEntity<Pane>>,
    /// Holds keyboard focus when nothing inside does, so app shortcuts
    /// still dispatch (GPUI needs a focused element to start from).
    focus: gpui::FocusHandle,
    /// The last key, click or mouse movement in the window, for idle locking.
    last_input: Rc<Cell<Instant>>,
    /// Port forwarding rules that are switched on, with their background
    /// connections. Here rather than in the vault view, so they stay up
    /// while the vault is locked.
    forwarder: Entity<crate::forwards::Forwarder>,
    /// Each snippet's variable values from its last run, until Holonet
    /// closes (never saved to the vault).
    snippet_values: std::collections::HashMap<holonet_model::SnippetId, std::collections::HashMap<String, String>>,
    /// The session tab last shown, for "Run in active session".
    last_session: Option<gpui::WeakEntity<Tab>>,
    /// The vault's session logs. Kept while locked, so open sessions keep
    /// logging (decisions.md: sessions stay connected while locked).
    logs: Option<holonet_vault::LogStore>,
    /// History changes made while the vault was locked, saved at unlock.
    pending_history: Vec<VaultChange>,
    /// Records a crash left open and records past their kept time are
    /// cleaned up once, at the first unlock.
    history_cleaned: bool,
}

/// A change to the vault waiting to be saved.
type VaultChange = Box<dyn FnOnce(&mut VaultData)>;

/// How often the idle time is checked.
const IDLE_CHECK: Duration = Duration::from_secs(10);

impl HolonetApp {
    pub fn new(path: PathBuf, runtime: tokio::runtime::Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.entity().downgrade();
        let stage = if holonet_vault::exists(&path) {
            let p = path.clone();
            Stage::Locked(cx.new(|cx| Locked::new(app, p, LockReason::Startup, 0, window, cx)))
        } else {
            let p = path.clone();
            Stage::Create(cx.new(|cx| CreateVault::new(app, p, window, cx)))
        };
        // Sleep, wake and network changes for reconnecting, and Windows
        // locking for the vault.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::platform::watch_system(tx);
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = rx.recv().await {
                if this.update_in(cx, |app, window, cx| app.system_event(event, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // The idle lock.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(IDLE_CHECK).await;
                if this.update_in(cx, |app, window, cx| app.check_idle(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (me, forward_runtime) = (cx.entity().downgrade(), runtime.clone());
        let forwarder = cx.new(|_| crate::forwards::Forwarder::new(me, forward_runtime));
        Self {
            forwarder,
            path,
            stage,
            runtime,
            tabs: Vec::new(),
            active: None,
            picker: None,
            closing: None,
            plus_bounds: Rc::new(Cell::new(Bounds::default())),
            tab_scroll: ScrollHandle::new(),
            edit_origin: None,
            focus: cx.focus_handle(),
            last_input: Rc::new(Cell::new(Instant::now())),
            snippet_values: Default::default(),
            last_session: None,
            logs: None,
            palette: None,
            pending_history: Vec::new(),
            history_cleaned: false,
        }
    }

    /// The vault was created or unlocked.
    pub fn opened(&mut self, vault: UnlockedVault, restored_from_backup: bool, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        let forwarder = self.forwarder.clone();
        let logs = vault.logs();
        self.logs = Some(logs.clone());
        let workspace = cx.new(|cx| Workspace::new(app, vault, restored_from_backup, runtime, forwarder, cx));
        // History from before this unlock: changes made while locked, then
        // (once per start) the clean-up.
        let pending = std::mem::take(&mut self.pending_history);
        let clean = !std::mem::replace(&mut self.history_cleaned, true);
        workspace.update(cx, |w, cx| {
            for change in pending {
                w.save(change, cx);
            }
            if clean {
                w.save(move |d| clean_history(d, &logs), cx);
            }
        });
        self.stage = Stage::Open(workspace);
        self.last_input.set(Instant::now());
        cx.notify();
    }

    /// Change History: saved now, or at the next unlock while locked.
    pub fn history_change(&mut self, change: impl FnOnce(&mut VaultData) + 'static, cx: &mut Context<Self>) {
        match self.workspace().cloned() {
            Some(w) => w.update(cx, |w, cx| w.save(change, cx)),
            None => self.pending_history.push(Box::new(change)),
        }
    }

    /// A session connected for the first time: its History record, and its
    /// log when the host saves output. `host` is the pane's resolved copy.
    pub fn start_record(
        &mut self,
        host: &holonet_model::Host,
        kind: holonet_model::SessionKind,
        route: String,
        user: String,
        cx: &mut Context<Self>,
    ) -> (holonet_model::RecordId, Option<crate::history::SessionLog>) {
        // While locked the Settings default cannot be read: only a host's
        // own switch turns logging on.
        let wanted = match self.vault_data(cx) {
            Some(d) => d.host(host.id).map(|h| d.logs_output(h)).unwrap_or_else(|| d.logs_output(host)),
            None => host.log_output.unwrap_or(false),
        };
        let saved = self.is_saved(host.id, cx);
        let mut record = holonet_model::SessionRecord::new(saved.then_some(host.id), host.display_name(), kind, wanted);
        record.route = route;
        record.user = user;
        let log = if record.logged {
            match self.logs.as_ref().map(|l| l.create(record.id)) {
                Some(Ok(writer)) => Some(crate::history::SessionLog::start(writer)),
                _ => {
                    record.logged = false;
                    None
                }
            }
        } else {
            None
        };
        let id = record.id;
        self.history_change(move |d| d.history.push(record), cx);
        (id, log)
    }

    /// A session ended: its record's end, then its log's final size once
    /// the log thread has sealed the rest.
    pub fn end_record(
        &mut self,
        id: holonet_model::RecordId,
        end: holonet_model::SessionEnd,
        log: Option<crate::history::SessionLog>,
        cx: &mut Context<Self>,
    ) {
        let bytes = log.as_ref().map(|l| l.bytes());
        self.history_change(
            move |d| {
                if let Some(r) = d.record_mut(id) {
                    r.ended = Some(Timestamp::now());
                    r.end = Some(end);
                    if let Some(b) = bytes {
                        r.log_bytes = b;
                    }
                }
            },
            cx,
        );
        let Some(log) = log else { return };
        let handle = log.finish();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { handle.join() }).await;
            let bytes = match result {
                Ok(Ok(bytes)) => bytes,
                _ => return,
            };
            let _ = this.update(cx, |app, cx| {
                app.history_change(
                    move |d| {
                        if let Some(r) = d.record_mut(id) {
                            r.log_bytes = bytes;
                        }
                    },
                    cx,
                )
            });
        })
        .detach();
    }

    /// "Lock now".
    pub fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lock_for(LockReason::Manual, window, cx);
    }

    /// Lock after the idle time set in Settings.
    fn check_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mins) = self.vault_data(cx).and_then(|d| d.settings.lock_after_idle_mins) else { return };
        if self.last_input.get().elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
            self.lock_for(LockReason::Idle(mins), window, cx);
        }
    }

    /// Dropping the workspace drops the unlocked vault, which wipes its key
    /// and secrets from memory. Sessions stay connected.
    fn lock_for(&mut self, reason: LockReason, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_unlocked() {
            return;
        }
        let app = cx.entity().downgrade();
        let path = self.path.clone();
        let open = self.tabs.iter().map(|t| t.read(cx).panes().len()).sum();
        for tab in &self.tabs {
            tab.update(cx, |t, cx| t.close_menu(cx));
        }
        self.picker = None;
        self.stage = Stage::Locked(cx.new(|cx| Locked::new(app, path, reason, open, window, cx)));
        self.active = None;
        cx.notify();
    }

    fn workspace(&self) -> Option<&Entity<Workspace>> {
        match &self.stage {
            Stage::Open(w) => Some(w),
            _ => None,
        }
    }

    fn vault_data<'a>(&self, cx: &'a App) -> Option<&'a VaultData> {
        self.workspace().and_then(|w| w.read(cx).vault_data())
    }

    /// A saved host's group, read from the vault each time so renaming or
    /// recoloring a group shows at once. `None` when it has no group, or
    /// while the vault is locked (session tabs are hidden then).
    /// The status bar's text for a host's active forwards.
    pub fn forwards_for(&self, host: holonet_model::HostId, cx: &App) -> Vec<String> {
        self.forwarder.read(cx).active_for(host)
    }

    /// A pane connected to `host`: its rules set to start with the host
    /// start, and its typed secrets serve their background connection.
    pub fn host_connected(
        &mut self,
        host: holonet_model::HostId,
        typed: Vec<(holonet_model::HostId, (bool, holonet_model::Secret))>,
        cx: &mut Context<Self>,
    ) {
        let Some(data) = self.vault_data(cx).cloned() else { return };
        self.forwarder.update(cx, |f, cx| f.host_connected(host, typed, &data, cx));
    }

    /// The group whose "Ask each time" a host's sign-in follows, by name:
    /// its password is asked on every connection and never saved.
    pub fn asks_each_time(&self, host: holonet_model::HostId, cx: &App) -> Option<SharedString> {
        let data = self.vault_data(cx)?;
        let host = data.host(host)?;
        let group = data.host_group(host)?;
        let has_identity = host.identity.and_then(|id| data.identity(id)).is_some();
        (host.from_group.key && !has_identity && group.key == holonet_model::GroupKey::AskEachTime)
            .then(|| group.display_name().to_string().into())
    }

    /// A snippet's values from its last run.
    pub fn snippet_values(&self, id: holonet_model::SnippetId) -> std::collections::HashMap<String, String> {
        self.snippet_values.get(&id).cloned().unwrap_or_default()
    }

    pub fn remember_snippet_values(
        &mut self,
        id: holonet_model::SnippetId,
        values: std::collections::HashMap<String, String>,
    ) {
        self.snippet_values.insert(id, values);
    }

    /// "2 Production hosts" among `hosts`, for broadcast's question.
    pub fn warn_text(&self, hosts: &[HostId], cx: &App) -> Option<String> {
        crate::snippet_run::warn_text(self.vault_data(cx)?, hosts)
    }

    /// The library by name, for a tab's Snippets drawer.
    pub fn snippets(&self, cx: &App) -> Vec<holonet_model::Snippet> {
        let Some(data) = self.vault_data(cx) else { return Vec::new() };
        let mut all = data.snippets.clone();
        all.sort_by_key(|s| s.display_name().to_lowercase());
        all
    }

    /// A host's startup snippet as the vault has it now.
    pub fn startup_snippet(&self, host: holonet_model::HostId, cx: &App) -> Option<holonet_model::Snippet> {
        let data = self.vault_data(cx)?;
        data.host(host)?.startup_snippet.and_then(|id| data.snippet(id)).cloned()
    }

    pub fn group_tag(&self, host: holonet_model::HostId, cx: &App) -> Option<crate::workspace::GroupTag> {
        let data = self.vault_data(cx)?;
        crate::workspace::GroupTag::of(data, data.host(host)?)
    }

    /// Change the vault and save it (no-op while locked).
    pub fn update_vault(&mut self, change: impl FnOnce(&mut VaultData) + 'static, cx: &mut Context<Self>) {
        if let Some(w) = self.workspace().cloned() {
            w.update(cx, |w, cx| w.save(change, cx));
        }
    }

    /// Compare a presented key with Known hosts; `None` while locked.
    /// Check a presented key. `aliases` are the same host's other addresses:
    /// with no entry for this address, a key trusted for one of them is
    /// trusted here too when it matches (and recorded for this address), and
    /// a different one stops the connection as a changed key.
    pub fn check_host_key(
        &mut self,
        key: &PresentedKey,
        aliases: &[String],
        cx: &mut Context<Self>,
    ) -> Option<KeyCheck> {
        let data = self.vault_data(cx)?;
        let found = data.known_host_or_alias(&key.host, key.port, aliases);
        let known = found.map(|(k, _)| k);
        let from_alias = found.is_some_and(|(_, alias)| alias);
        let check = KeyCheck {
            status: classify(known, key),
            policy: data.settings.unknown_host_keys,
            trusted_at: known.map(|k| k.trusted_at),
            from_alias: from_alias.then(|| known.map(|k| k.host.clone())).flatten(),
        };
        if matches!(check.status, holonet_ssh::KeyStatus::Trusted) {
            let (host, port, public) = (key.host.clone(), key.port, key.public_key.clone());
            self.update_vault(
                move |d| {
                    if from_alias {
                        // The same key, now also trusted under this address.
                        d.trust_host_key(&host, port, &public, KnownHostSource::FirstConnection);
                    } else if let Some(k) = d.known_host_mut(&host, port) {
                        k.last_matched_at = Some(Timestamp::now());
                    }
                },
                cx,
            );
        }
        Some(check)
    }

    /// "Trust and connect" or "Replace trusted key".
    pub fn trust_host_key(&mut self, key: &PresentedKey, cx: &mut Context<Self>) {
        let (host, port, public) = (key.host.clone(), key.port, key.public_key.clone());
        self.update_vault(move |d| d.trust_host_key(&host, port, &public, KnownHostSource::FirstConnection), cx);
    }

    /// A changed key was refused: keep it for Known hosts' "Mismatch seen",
    /// on the entry it was compared with (this address, or the host's other
    /// address when this one has none).
    pub fn record_mismatch(&mut self, key: &PresentedKey, entry: Option<&str>, cx: &mut Context<Self>) {
        let host = entry.map(str::to_string).unwrap_or_else(|| key.host.clone());
        let (port, public) = (key.port, key.public_key.clone());
        self.update_vault(
            move |d| {
                if let Some(k) = d.known_host_mut(&host, port) {
                    k.mismatch = Some(Mismatch { presented_key: public, seen_at: Timestamp::now() });
                }
            },
            cx,
        );
    }

    /// Terminal settings changed in Settings: every open terminal follows.
    pub fn apply_terminal_settings(&mut self, settings: &holonet_model::TerminalSettings, cx: &mut Context<Self>) {
        let panes: Vec<Entity<Pane>> = self.tabs.iter().flat_map(|t| t.read(cx).panes()).collect();
        for pane in panes {
            pane.update(cx, |p, cx| p.set_terminal_settings(settings, cx));
        }
    }

    /// SFTP settings changed in Settings: every SFTP tab follows.
    pub fn apply_sftp_settings(&mut self, settings: &holonet_model::SftpSettings, cx: &mut Context<Self>) {
        let panes: Vec<Entity<Pane>> = self.tabs.iter().flat_map(|t| t.read(cx).panes()).collect();
        for pane in panes {
            pane.update(cx, |p, cx| p.set_sftp_settings(settings, cx));
        }
    }

    /// Settings, Connection, "Reconnect after sleep or network change".
    /// While locked the vault cannot be read; reconnecting stays on.
    pub fn reconnect_enabled(&self, cx: &App) -> bool {
        self.vault_data(cx).is_none_or(|d| d.settings.connection.reconnect)
    }

    pub fn is_unlocked(&self) -> bool {
        matches!(self.stage, Stage::Open(_))
    }

    fn system_event(&mut self, event: crate::platform::SystemEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::platform::SystemEvent;
        match event {
            SystemEvent::SessionLocked => {
                if self.vault_data(cx).is_some_and(|d| d.settings.lock_when_windows_locks) {
                    self.lock_for(LockReason::WindowsLocked, window, cx);
                }
                return;
            }
            SystemEvent::SessionUnlocked => return,
            SystemEvent::Suspend | SystemEvent::Resume | SystemEvent::NetworkChanged => {}
        }
        let panes: Vec<Entity<Pane>> = self.tabs.iter().flat_map(|t| t.read(cx).panes()).collect();
        for pane in panes {
            pane.update(cx, |p, cx| p.on_system_event(event, cx));
        }
    }

    /// Saved hosts by name, for a tab's split menu.
    pub fn host_names(&self, cx: &App) -> Vec<(HostId, SharedString)> {
        let Some(data) = self.vault_data(cx) else { return Vec::new() };
        let mut hosts: Vec<(HostId, SharedString)> =
            data.hosts.iter().map(|h| (h.id, SharedString::from(h.display_name().to_string()))).collect();
        hosts.sort_by_key(|(_, n)| n.to_lowercase());
        hosts
    }

    /// A pane connecting to a saved host, `size` cells to start with (the
    /// pane re-measures on its first paint and tells the server).
    fn new_pane(
        &mut self,
        id: HostId,
        size: GridSize,
        opening: (PaneMode, Remembered),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        let data = self.vault_data(cx)?;
        // With its identity's username and sign-in, if it has one.
        let host = data.resolved(data.host(id)?);
        self.new_pane_for(host, size, opening, window, cx)
    }

    /// A pane for a host as given: a saved host already resolved, or a
    /// quick connect's host that is not in the vault.
    fn new_pane_for(
        &mut self,
        host: holonet_model::Host,
        size: GridSize,
        opening: (PaneMode, Remembered),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        let data = self.vault_data(cx)?;
        let key = match &host.auth {
            holonet_model::Auth::Key { key_id, .. } => data.key(*key_id).cloned(),
            holonet_model::Auth::Password { .. } | holonet_model::Auth::Agent => None,
        };
        let app = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        let limits = Limits {
            connect_timeout: Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64),
            keep_alive: data.settings.connection.keep_alive_for(&host),
            terminal: data.settings.terminal.clone(),
            fallback_wait: data.settings.connection.fallback_wait.duration(),
            sftp: data.settings.sftp.clone(),
        };
        let jumps = crate::hops::jumps(data, &host);
        let pane = cx.new(|cx| Pane::new(app, host, key, jumps, runtime, limits, size, opening, window, cx));
        cx.subscribe_in(&pane, window, |this, pane, event: &SessionEvent, window, cx| match event {
            SessionEvent::Close => this.close_pane(pane, window, cx),
            SessionEvent::ReviewKnownHost => {
                let (host, port) = pane.read(cx).review_address();
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.show_known_host(&host, port, cx));
                }
            }
            SessionEvent::EditHost => {
                // The failed hop: a jump host's failure edits the jump host.
                let id = pane.read(cx).host_to_edit();
                this.edit_origin = Some(pane.downgrade());
                // A quick connect has no saved host to edit: save it.
                if !this.is_saved(id, cx) {
                    let host = pane.read(cx).draft_host();
                    this.save_as_host(host, window, cx);
                    return;
                }
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.edit_host(id, window, cx));
                }
            }
        })
        .detach();
        Some(pane)
    }

    /// Open a new tab connected to a saved host, or retry in the stopped pane
    /// the host was edited from.
    pub fn connect(&mut self, id: HostId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.edit_origin.take().and_then(|p| p.upgrade())
            && pane.read(cx).depends_on(id)
            && pane.read(cx).is_stopped()
            && let Some(index) = self.tabs.iter().position(|t| t.read(cx).contains(&pane))
            && let Some(data) = self.vault_data(cx)
            // The pane's own host, with an edited jump host's new details.
            && let Some(host) = data.host(pane.read(cx).host.id).map(|h| data.resolved(h))
        {
            let jumps = crate::hops::jumps(data, &host);
            let key = match &host.auth {
                holonet_model::Auth::Key { key_id, .. } => data.key(*key_id).cloned(),
                holonet_model::Auth::Password { .. } | holonet_model::Auth::Agent => None,
            };
            let limits = Limits {
                connect_timeout: Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64),
                keep_alive: data.settings.connection.keep_alive_for(&host),
                terminal: data.settings.terminal.clone(),
                fallback_wait: data.settings.connection.fallback_wait.duration(),
                sftp: data.settings.sftp.clone(),
            };
            pane.update(cx, |p, cx| p.retry_with(host, key, jumps, limits, cx));
            self.select(Some(index), window, cx);
            return;
        }
        self.open_tab(id, (PaneMode::Terminal, Vec::new()), window, cx);
    }

    /// Open an SFTP tab for a saved host (Open SFTP, Open SFTP here, the
    /// picker). `remembered` carries a terminal's typed secrets.
    pub fn open_files(&mut self, id: HostId, remembered: Remembered, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(id, (PaneMode::Files, remembered), window, cx);
    }

    /// The terminal size a new tab's pane starts with, from the window.
    fn tab_grid(window: &Window) -> GridSize {
        let viewport = window.viewport_size();
        GridSize {
            cols: ((f32::from(viewport.width) - 56. - 16. - 32.) / 7.8).max(20.) as usize,
            rows: ((f32::from(viewport.height) - 44. - 60. - 16. - 40. - 40. - 28.) / 20.8).max(5.) as usize,
        }
    }

    /// "Run on n hosts": a tab with a pane per host, four to a tab, each
    /// typing `text` once its shell opens. Each host signs in its own way;
    /// one that asks for a password asks in its pane.
    pub fn run_on_hosts(&mut self, hosts: &[HostId], text: String, window: &mut Window, cx: &mut Context<Self>) {
        let full = Self::tab_grid(window);
        let half = GridSize { cols: (full.cols / 2).max(20), rows: (full.rows / 2).max(5) };
        for chunk in hosts.chunks(crate::tab::MAX_PANES) {
            let Some(tab) = self.open_tab(chunk[0], (PaneMode::Terminal, Vec::new()), window, cx) else { continue };
            let first = tab.read(cx).active_pane().clone();
            first.update(cx, |p, cx| p.queue_input(text.clone(), cx));
            let mut panes = vec![first];
            for &host in &chunk[1..] {
                let Some(pane) = self.new_pane(host, half, (PaneMode::Terminal, Vec::new()), window, cx) else {
                    continue;
                };
                pane.update(cx, |p, cx| p.queue_input(text.clone(), cx));
                // Two side by side, then each split down: a 2 × 2 grid.
                let (target, axis) = match panes.len() {
                    1 => (panes[0].clone(), Axis::Right),
                    2 => (panes[0].clone(), Axis::Down),
                    _ => (panes[1].clone(), Axis::Down),
                };
                tab.update(cx, |t, cx| t.add_pane(&target, axis, pane.clone(), cx));
                panes.push(pane);
            }
        }
        cx.notify();
    }

    /// The terminal "Run in active session" types into: the active pane
    /// of the session tab shown last, when its shell is open.
    pub fn active_session_name(&self, cx: &App) -> Option<SharedString> {
        let tab = self.last_session.as_ref()?.upgrade()?;
        let pane = tab.read(cx).active_pane().clone();
        let pane = pane.read(cx);
        pane.can_run().then(|| pane.name())
    }

    /// Type `text` into that pane and show its tab.
    pub fn run_in_active_session(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.last_session.as_ref().and_then(|t| t.upgrade()) else { return };
        let Some(index) = self.tabs.iter().position(|t| *t == tab) else { return };
        let pane = tab.read(cx).active_pane().clone();
        pane.update(cx, |p, cx| p.queue_input(text, cx));
        self.select(Some(index), window, cx);
    }

    fn open_tab(
        &mut self,
        id: HostId,
        opening: (PaneMode, Remembered),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Tab>> {
        let size = Self::tab_grid(window);
        let pane = self.new_pane(id, size, opening, window, cx)?;
        Some(self.open_tab_with(pane, window, cx))
    }

    /// Quick connect (FirstRun.dc.html's Connect now, the Hosts search): a
    /// terminal to a host that is not saved. Its host key is still checked.
    pub fn quick_connect(&mut self, host: holonet_model::Host, window: &mut Window, cx: &mut Context<Self>) {
        let size = Self::tab_grid(window);
        if let Some(pane) = self.new_pane_for(host, size, (PaneMode::Terminal, Vec::new()), window, cx) {
            self.open_tab_with(pane, window, cx);
        }
    }

    /// Whether a host is saved in the vault (true while locked: nothing is
    /// offered then anyway).
    pub fn is_saved(&self, id: HostId, cx: &App) -> bool {
        self.vault_data(cx).is_none_or(|d| d.host(id).is_some())
    }

    /// "Save as host": the host form, filled in from a quick connect.
    fn save_as_host(&mut self, host: holonet_model::Host, window: &mut Window, cx: &mut Context<Self>) {
        self.show_vault(window, cx);
        if let Some(w) = self.workspace().cloned() {
            w.update(cx, |w, cx| w.open_form_with(None, Some(host), window, cx));
        }
    }

    fn open_tab_with(&mut self, pane: Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) -> Entity<Tab> {
        let app = cx.entity().downgrade();
        let tab = cx.new(|cx| Tab::new(app, pane, cx));
        // The title bar shows each tab's name and state dot.
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&tab, window, |this, tab, event: &TabEvent, window, cx| match event {
            TabEvent::Split { axis, host } => this.split(tab, *axis, *host, window, cx),
            TabEvent::OpenFiles { host, remembered } => {
                let (host, remembered) = (*host, remembered.clone());
                this.open_files(host, remembered, window, cx);
            }
            TabEvent::SaveAsHost(host) => {
                let host = (**host).clone();
                this.save_as_host(host, window, cx);
            }
            TabEvent::ManageSnippets => {
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.open_snippets(None, cx));
                }
            }
        })
        .detach();
        self.tabs.push(tab.clone());
        self.select(Some(self.tabs.len() - 1), window, cx);
        tab
    }

    /// Split a tab's active pane, with the same host unless another is given.
    fn split(
        &mut self,
        tab: &Entity<Tab>,
        axis: Axis,
        host: Option<HostId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An SFTP tab is one view; it does not split.
        if !tab.read(cx).can_split() || tab.read(cx).is_files(cx) {
            return;
        }
        let active = tab.read(cx).active_pane().clone();
        let (same, size) = {
            let p = active.read(cx);
            (p.host.id, p.grid_size(cx))
        };
        // The new pane takes half of the one it splits.
        let size = match axis {
            Axis::Right => GridSize { cols: (size.cols / 2).max(20), rows: size.rows },
            Axis::Down => GridSize { cols: size.cols, rows: (size.rows / 2).max(5) },
        };
        let opening = (PaneMode::Terminal, Vec::new());
        let pane = match host {
            // A quick connect splits into another pane to the same host.
            None if !self.is_saved(same, cx) => {
                let host = active.read(cx).host.clone();
                self.new_pane_for(host, size, opening, window, cx)
            }
            _ => self.new_pane(host.unwrap_or(same), size, opening, window, cx),
        };
        let Some(pane) = pane else { return };
        tab.update(cx, |t, cx| t.split_active(axis, pane, window, cx));
        cx.notify();
    }

    fn active_tab(&self) -> Option<Entity<Tab>> {
        self.active.and_then(|i| self.tabs.get(i)).cloned()
    }

    /// Transfers that closing this tab would stop (SFTP tabs).
    fn running_transfers(tab: &Entity<Tab>, cx: &App) -> usize {
        tab.read(cx).panes().iter().map(|p| p.read(cx).unfinished_transfers(cx)).sum()
    }

    /// Ask "Close anyway?" when the tab still has transfers; true if asked.
    fn ask_before_closing(&mut self, tab: &Entity<Tab>, cx: &mut Context<Self>) -> bool {
        if Self::running_transfers(tab, cx) == 0 {
            return false;
        }
        self.closing = Some(tab.clone());
        cx.notify();
        true
    }

    /// Close one pane; its tab closes with its last pane.
    fn close_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.read(cx).contains(pane)) else { return };
        // An SFTP tab is its one pane.
        if self.ask_before_closing(&self.tabs[index].clone(), cx) {
            return;
        }
        pane.update(cx, |p, cx| {
            p.stop_transfers(cx);
            p.disconnect(cx)
        });
        let tab = self.tabs[index].clone();
        if tab.update(cx, |t, cx| t.remove(pane, window, cx)) {
            self.close_tab_at(index, window, cx);
        } else {
            tab.update(cx, |t, cx| t.focus(window, cx));
        }
    }

    /// Close a whole tab and every pane in it, asking first while an SFTP
    /// tab still has transfers.
    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(index).cloned()
            && self.ask_before_closing(&tab, cx)
        {
            return;
        }
        self.close_tab_now(index, window, cx);
    }

    fn close_tab_now(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        for pane in tab.read(cx).panes() {
            pane.update(cx, |p, cx| {
                p.stop_transfers(cx);
                p.disconnect(cx)
            });
        }
        let next = match self.active {
            Some(a) if a == index => {
                if self.tabs.is_empty() {
                    None
                } else {
                    Some(index.min(self.tabs.len() - 1))
                }
            }
            Some(a) if a > index => Some(a - 1),
            other => other,
        };
        self.select(next, window, cx);
    }

    fn select(&mut self, tab: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = tab;
        if let Some(t) = tab.and_then(|i| self.tabs.get(i))
            && !t.read(cx).is_files(cx)
        {
            self.last_session = Some(t.downgrade());
        }
        if let Some(i) = tab {
            self.tab_scroll.scroll_to_item(i);
        }
        match tab.and_then(|i| self.tabs.get(i)).cloned() {
            Some(t) => t.update(cx, |t, cx| t.focus(window, cx)),
            // The vault view: take focus back from the hidden terminal, so
            // app shortcuts keep working.
            None => window.focus(&self.focus),
        }
        cx.notify();
    }

    /// Ctrl Tab and Ctrl Shift Tab: cycle through the vault tab and the session tabs.
    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.stage, Stage::Open(_)) {
            return;
        }
        let slots = self.tabs.len() as isize + 1; // slot 0 is the vault
        let current = self.active.map_or(0, |i| i as isize + 1);
        let next = (current + delta).rem_euclid(slots);
        self.select(if next == 0 { None } else { Some(next as usize - 1) }, window, cx);
    }

    /// "+" or Ctrl Shift T: the host picker under the "+" button.
    /// Ctrl Shift P: the command palette, over whatever is shown. Not while
    /// the vault is locked: its entries come from the vault.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::palette::{CommandPalette, PaletteEvent};
        if self.vault_data(cx).is_none() {
            return;
        }
        self.picker = None;
        let entries = self.palette_entries(cx);
        let palette = cx.new(|cx| CommandPalette::new(entries, window, cx));
        cx.subscribe_in(&palette, window, |this, _, event: &PaletteEvent, window, cx| {
            this.palette = None;
            match event {
                PaletteEvent::Run { command, split } => {
                    let (command, split) = (command.clone(), *split);
                    // Deferred: running reads the workspace, which may be mid-update.
                    let app = cx.entity();
                    window.defer(cx, move |window, cx| {
                        app.update(cx, |app, cx| app.run_command(command, split, window, cx))
                    });
                }
                PaletteEvent::Dismiss => this.select(this.active, window, cx),
            }
            cx.notify();
        })
        .detach();
        self.palette = Some(palette);
        cx.notify();
    }

    /// The palette's entries: hosts (the open tab, or connect), snippets
    /// when a terminal can run them, rules, screens and the actions that
    /// work now.
    fn palette_entries(&self, cx: &App) -> Vec<crate::palette::Entry> {
        use crate::palette::{Command, Entry, Screen, Section};
        let Some(data) = self.vault_data(cx) else { return Vec::new() };
        let entry = |section, title: String, hint: &str, command| Entry {
            section,
            title,
            hint: hint.into(),
            keys: None,
            color: None,
            command,
            splits: false,
            recent: 0,
            search_only: false,
        };
        let active = self.active_tab();
        let terminal_tab = active.as_ref().filter(|t| !t.read(cx).is_files(cx));
        let can_split = terminal_tab.is_some_and(|t| t.read(cx).can_split());
        let mut out = Vec::new();
        for host in &data.hosts {
            let tab = self.tabs.iter().position(|t| {
                !t.read(cx).is_files(cx) && t.read(cx).panes().iter().any(|p| p.read(cx).host.id == host.id)
            });
            let (hint, command) = match tab {
                Some(i) => ("Go to session", Command::GoToTab(i)),
                None => ("Connect", Command::Connect(host.id)),
            };
            let color = crate::workspace::GroupTag::of(data, host).map(|t| t.color);
            let recent = host.last_connected_at.map_or(0, |t| t.0) + if tab.is_some() { 1 << 40 } else { 0 };
            out.push(Entry {
                color,
                splits: can_split,
                recent,
                ..entry(Section::Hosts, host.display_name().to_string(), hint, command)
            });
            out.push(Entry {
                color,
                recent,
                search_only: true,
                ..entry(Section::Hosts, host.display_name().to_string(), "Open SFTP", Command::OpenFiles(host.id))
            });
        }
        if self.active_session_name(cx).is_some() {
            for s in &data.snippets {
                out.push(entry(
                    Section::Snippets,
                    s.display_name().to_string(),
                    "Run in active session",
                    Command::RunSnippet(Box::new(s.clone())),
                ));
            }
        }
        for rule in &data.forward_rules {
            let on = self.forwarder.read(cx).is_on(rule.id);
            out.push(entry(
                Section::Forwarding,
                rule.display_name().to_string(),
                if on { "Turn off" } else { "Turn on" },
                Command::Rule(rule.id, !on),
            ));
        }
        for screen in Screen::ALL {
            out.push(entry(Section::GoTo, screen.label().into(), "Screen", Command::Screen(screen)));
        }
        let action = |title: &str, keys: &'static str, command| Entry {
            keys: Some(keys),
            ..entry(Section::Actions, title.into(), "Action", command)
        };
        out.push(action("New host", "Ctrl Shift N", Command::NewHost));
        out.push(action("Lock vault", "Ctrl Shift L", Command::LockVault));
        out.push(action("New tab", "Ctrl Shift T", Command::NewTab));
        if can_split {
            out.push(action("Split right", "Ctrl Shift D", Command::SplitRight));
        }
        if terminal_tab.is_some_and(|t| t.read(cx).panes().len() > 1) {
            out.push(action("Broadcast", "Ctrl Shift B", Command::Broadcast));
        }
        if terminal_tab.is_some() {
            out.push(action("Find in terminal", "Ctrl Shift F", Command::Find));
        }
        out
    }

    /// Run a palette entry. `split` opens a host in a split of the current
    /// terminal tab.
    fn run_command(
        &mut self,
        command: crate::palette::Command,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::palette::{Command, Screen};
        match command {
            Command::GoToTab(i) => self.select(Some(i), window, cx),
            Command::Connect(id) if split => match self.active_tab() {
                Some(tab) => self.split(&tab, Axis::Right, Some(id), window, cx),
                None => self.connect(id, window, cx),
            },
            Command::Connect(id) => self.connect(id, window, cx),
            Command::OpenFiles(id) => self.open_files(id, Vec::new(), window, cx),
            Command::RunSnippet(snippet) => {
                let Some(tab) = self.last_session.as_ref().and_then(|t| t.upgrade()) else { return };
                tab.update(cx, |t, cx| t.run_snippet_here(*snippet, window, cx));
                // Back in its tab, with the keyboard in its pane.
                if let Some(index) = self.tabs.iter().position(|t| *t == tab) {
                    self.select(Some(index), window, cx);
                }
            }
            Command::Rule(id, on) => {
                let Some(data) = self.vault_data(cx).cloned() else { return };
                self.forwarder.update(cx, |f, cx| if on { f.start(id, &data, cx) } else { f.stop(id, cx) });
                // The keyboard goes back where it was.
                self.select(self.active, window, cx);
            }
            Command::Screen(screen) => {
                self.show_vault(window, cx);
                if let Some(w) = self.workspace().cloned() {
                    w.update(cx, |w, cx| match screen {
                        Screen::Snippets => w.open_snippets(None, cx),
                        Screen::History => w.open_history(cx),
                        Screen::Hosts => w.set_page(Page::Hosts, cx),
                        Screen::Groups => w.set_page(Page::Groups, cx),
                        Screen::Keychain => w.set_page(Page::Keychain, cx),
                        Screen::Forwarding => w.set_page(Page::Forwarding, cx),
                        Screen::KnownHosts => w.set_page(Page::KnownHosts, cx),
                        Screen::Settings => w.set_page(Page::Settings, cx),
                    });
                }
            }
            Command::NewHost => self.new_host(window, cx),
            Command::LockVault => self.lock(window, cx),
            Command::NewTab => self.open_picker(window, cx),
            Command::SplitRight => {
                if let Some(tab) = self.active_tab() {
                    self.split(&tab, Axis::Right, None, window, cx);
                }
            }
            Command::Broadcast => {
                if let Some(tab) = self.active_tab() {
                    tab.update(cx, |t, cx| t.toggle_broadcast(cx));
                }
                self.select(self.active, window, cx);
            }
            Command::Find => {
                if let Some(tab) = self.active_tab() {
                    tab.update(cx, |t, cx| {
                        if !t.focus_find(window, cx) {
                            t.toggle_find(window, cx);
                        }
                    });
                }
            }
        }
    }

    /// Ctrl Shift N: the New host form.
    fn new_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace().is_none() {
            return;
        }
        self.show_vault(window, cx);
        if let Some(w) = self.workspace().cloned() {
            w.update(cx, |w, cx| w.open_form(None, window, cx));
        }
    }

    fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault_data(cx) else { return };
        let data = data.clone();
        let picker = cx.new(|cx| HostPicker::new(&data, window, cx));
        cx.subscribe_in(&picker, window, |this, _, event: &HostPickerEvent, window, cx| {
            this.picker = None;
            match event {
                HostPickerEvent::Connect(id) => {
                    let id = *id;
                    // Deferred: connecting reads the workspace, which may be mid-update.
                    let app = cx.entity();
                    window.defer(cx, move |window, cx| app.update(cx, |app, cx| app.connect(id, window, cx)));
                }
                HostPickerEvent::OpenFiles(id) => {
                    let id = *id;
                    let app = cx.entity();
                    window.defer(cx, move |window, cx| {
                        app.update(cx, |app, cx| app.open_files(id, Vec::new(), window, cx))
                    });
                }
                HostPickerEvent::NewHost => {
                    this.show_vault(window, cx);
                    if let Some(w) = this.workspace().cloned() {
                        w.update(cx, |w, cx| w.open_form(None, window, cx));
                    }
                }
                HostPickerEvent::Dismiss => this.select(this.active, window, cx),
            }
            cx.notify();
        })
        .detach();
        self.picker = Some(picker);
        cx.notify();
    }

    fn show_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select(None, window, cx);
    }

    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, glyph: &'static str, active: bool| {
            div()
                .id(id)
                .size(theme::CONTROL)
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_MD)
                .cursor_pointer()
                .when(active, |d| d.bg(theme::raised()))
                .when(!active, |d| d.hover(|s| s.bg(theme::card())))
                .child(icon(glyph, px(18.), if active { theme::accent() } else { theme::ink_muted() }))
        };
        div()
            .w(px(56.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(theme::SPACE_1)
            .py(px(10.))
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::line())
            .child(item("rail-hosts", "hosts", false).on_click(cx.listener(|this, _, window, cx| {
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.set_page(Page::Hosts, cx));
                }
            })))
            .child(item("rail-terminal", "terminal", true))
            .child(div().flex_1())
            .child(item("rail-settings", "settings", false).on_click(cx.listener(|this, _, window, cx| {
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.set_page(Page::Settings, cx));
                }
            })))
    }
}

impl Render for HolonetApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus);
        }
        let unlocked = matches!(self.stage, Stage::Open(_));
        let session = self.active.filter(|_| unlocked).and_then(|i| self.tabs.get(i)).cloned();
        let body = match (&self.stage, &session) {
            (Stage::Open(_), Some(tab)) => div()
                .size_full()
                .flex()
                .child(self.rail(cx))
                .child(div().flex_1().min_w_0().h_full().child(tab.clone()))
                .into_any_element(),
            (Stage::Create(view), _) => view.clone().into_any_element(),
            (Stage::Locked(view), _) => view.clone().into_any_element(),
            (Stage::Open(view), None) => view.clone().into_any_element(),
        };
        // Session tabs show only while unlocked (Locked.dc.html: vault tab only).
        let tabs: Vec<TabInfo> = if unlocked {
            self.tabs
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let t = t.read(cx);
                    let host = t.active_pane().read(cx).host.id;
                    TabInfo {
                        id: SharedString::from(format!("session-{i}")).into(),
                        name: t.name(cx),
                        kind: if t.is_files(cx) { TabKind::Sftp } else { TabKind::Terminal },
                        state: t.state(cx),
                        color: crate::workspace::GroupTag::color_of(self.group_tag(host, cx).as_ref()),
                        active: self.active == Some(i),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut title_bar = TitleBar::new(self.active.is_none() || !unlocked, tabs)
            .scroll(self.tab_scroll.clone())
            .plus_bounds(self.plus_bounds.clone());
        if unlocked {
            title_bar = title_bar
                .on_vault(cx.listener(|this, _, window, cx| this.show_vault(window, cx)))
                .on_tab({
                    let entity = cx.entity();
                    move |i, window, cx| entity.update(cx, |this, cx| this.select(Some(i), window, cx))
                })
                .on_close_tab({
                    let entity = cx.entity();
                    move |i, window, cx| entity.update(cx, |this, cx| this.close_tab_at(i, window, cx))
                })
                // "+": pick a host to connect to.
                .on_new_tab(cx.listener(|this, _, window, cx| this.open_picker(window, cx)));
        }
        let picker = self.picker.clone().map(|picker| {
            let plus = self.plus_bounds.get();
            // A transparent layer over the window closes the picker on an
            // outside click; the picker sits under the "+" button.
            div()
                .id("picker-layer")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.picker = None;
                        this.select(this.active, window, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(plus.origin.x.max(px(8.)))
                        .top(plus.origin.y + plus.size.height + px(6.))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(picker),
                )
        });
        // Palette.dc.html: the palette over a dimmed window, near the top.
        let palette = self.palette.clone().map(|palette| {
            div()
                .id("palette-layer")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(96.))
                .px(px(20.))
                .bg(gpui::hsla(0., 0., 0., 0.6))
                .occlude()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.palette = None;
                        this.select(this.active, window, cx);
                    }),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(palette),
                )
        });
        // "n transfers are still running. Close anyway?"
        let closing = self.closing.clone().filter(|_| unlocked).map(|tab| {
            let n = Self::running_transfers(&tab, cx);
            let title = if n == 1 {
                "1 transfer is still running".to_string()
            } else {
                format!("{n} transfers are still running")
            };
            let keep = cx.listener(|this, _, _, cx| {
                this.closing = None;
                cx.notify();
            });
            modal(
                "close-sftp-tab",
                px(460.),
                dialog(
                    true,
                    title,
                    format!(
                        "Closing {} stops {}, and the queue goes with the tab. Close anyway?",
                        tab.read(cx).name(cx),
                        if n == 1 { "it" } else { "them" }
                    ),
                )
                .child(dialog_buttons([
                    Button::secondary("close-keep", "Keep the tab").on_click(keep),
                    Button::new("close-anyway", "Close tab", ButtonKind::DestructiveFill).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.closing = None;
                            if let Some(index) = this.tabs.iter().position(|t| *t == tab) {
                                this.close_tab_now(index, window, cx);
                            }
                            cx.notify();
                        },
                    )),
                ])),
                {
                    let app = cx.entity().downgrade();
                    move |_, cx| {
                        let _ = app.update(cx, |this, cx| {
                            this.closing = None;
                            cx.notify();
                        });
                    }
                },
            )
        });
        let (keys, clicks, moves) = (self.last_input.clone(), self.last_input.clone(), self.last_input.clone());
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            // Input anywhere in the window keeps the vault unlocked; capture
            // runs before any child can stop the event.
            .capture_key_down(move |_, _, _| keys.set(Instant::now()))
            .capture_any_mouse_down(move |_, _, _| clicks.set(Instant::now()))
            .on_mouse_move(move |_, _, _| moves.set(Instant::now()))
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.open_picker(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(pane) = this.active.and_then(|i| this.tabs.get(i)).map(|t| t.read(cx).active_pane().clone())
                {
                    this.close_pane(&pane, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(1, window, cx)))
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                if let Some(tab) = this.active_tab() {
                    this.split(&tab, Axis::Right, None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                if let Some(tab) = this.active_tab() {
                    this.split(&tab, Axis::Down, None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| {
                if let Some(tab) = this.active_tab() {
                    tab.update(cx, |t, cx| {
                        if !t.focus_find(window, cx) {
                            t.toggle_find(window, cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &Broadcast, _, cx| {
                // Deferred: turning it on reads the vault through this app,
                // which is mid-update here.
                if let Some(tab) = this.active_tab() {
                    cx.defer(move |cx| tab.update(cx, |t, cx| t.toggle_broadcast(cx)));
                }
            }))
            .on_action(cx.listener(|this, _: &NextPane, window, cx| {
                if let Some(tab) = this.active_tab() {
                    tab.update(cx, |t, cx| t.cycle_pane(1, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousPane, window, cx| {
                if let Some(tab) = this.active_tab() {
                    tab.update(cx, |t, cx| t.cycle_pane(-1, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            .on_action(cx.listener(|_, _: &AddHost, window, cx| {
                // Deferred: the form reads the workspace, which may be mid-update.
                let app = cx.entity();
                window.defer(cx, move |window, cx| app.update(cx, |app, cx| app.new_host(window, cx)));
            }))
            .on_action(cx.listener(|this, _: &LockNow, window, cx| this.lock(window, cx)))
            .bg(theme::ground())
            .text_color(theme::ink())
            .font_family(theme::SANS)
            .child(title_bar)
            .child(div().flex_1().min_h_0().relative().child(body))
            .children(picker)
            .children(closing)
            .children(palette)
    }
}

/// Once per start: close records a crash left open (at their log's last
/// write), drop records past Settings' kept time with their logs, and
/// delete log files that belong to no record.
fn clean_history(d: &mut VaultData, logs: &holonet_vault::LogStore) {
    // A log a crash left open never reported its size: count what it kept.
    for r in d.history.iter_mut().filter(|r| r.ended.is_none() && r.logged) {
        if let Ok(text) = logs.read(r.id) {
            r.log_bytes = text.text.len() as u64;
        }
    }
    d.close_open_records(|r| logs.last_write(r.id));
    for old in d.expire_history(Timestamp::now()) {
        let _ = logs.delete(old.id);
    }
    logs.remove_orphans(d.history.iter().filter(|r| r.logged).map(|r| r.id));
}
