//! The Holonet window: the vault gate (create or unlock), the workspace, and
//! session tabs, each holding one or more panes.
//!
//! Tabs belong to the app, not the workspace, so locking the vault (which
//! drops the workspace and its unlocked vault) leaves them connected. They
//! reappear when the vault is unlocked (decisions.md).
//!
//! App shortcuts are actions bound in the "HolonetApp" context. GPUI matches
//! bindings before key-down handlers, so they win over the terminal, and
//! they all use Ctrl Shift (or Ctrl Tab) so shell keys still reach the shell.

use std::path::PathBuf;
use std::time::Duration;

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
use crate::session::{KeyCheck, Limits, Pane, SessionEvent};
use crate::shell::{TabInfo, TabKind, TitleBar};
use crate::tab::Tab;
use crate::terminal::GridSize;
use crate::theme;
use crate::ui::icon;
use crate::vault_screens::{CreateVault, LockReason, Locked};
use crate::workspace::{Page, Workspace};

actions!(holonet, [NewTab, CloseTab, NextTab, PreviousTab]);

const CONTEXT: &str = "HolonetApp";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(CONTEXT)),
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
    /// Where the "+" button was drawn, to place the picker under it.
    plus_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Horizontal scroll of the tab strip, so the active tab stays in view.
    tab_scroll: ScrollHandle,
    /// Holds keyboard focus when nothing inside does, so app shortcuts
    /// still dispatch (GPUI needs a focused element to start from).
    focus: gpui::FocusHandle,
}

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
        Self {
            path,
            stage,
            runtime,
            tabs: Vec::new(),
            active: None,
            picker: None,
            plus_bounds: Rc::new(Cell::new(Bounds::default())),
            tab_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    /// The vault was created or unlocked.
    pub fn opened(&mut self, vault: UnlockedVault, restored_from_backup: bool, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        self.stage = Stage::Open(cx.new(|cx| Workspace::new(app, vault, restored_from_backup, runtime, cx)));
        cx.notify();
    }

    /// "Lock now": dropping the workspace drops the unlocked vault, which
    /// wipes its key and secrets from memory. Sessions stay connected.
    pub fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let path = self.path.clone();
        let open = self.tabs.iter().map(|t| t.read(cx).panes().len()).sum();
        self.picker = None;
        self.stage = Stage::Locked(cx.new(|cx| Locked::new(app, path, LockReason::Manual, open, window, cx)));
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

    /// Change the vault and save it (no-op while locked).
    pub fn update_vault(&mut self, change: impl FnOnce(&mut VaultData) + 'static, cx: &mut Context<Self>) {
        if let Some(w) = self.workspace().cloned() {
            w.update(cx, |w, cx| w.save(change, cx));
        }
    }

    /// Compare a presented key with Known hosts; `None` while locked.
    pub fn check_host_key(&mut self, key: &PresentedKey, cx: &mut Context<Self>) -> Option<KeyCheck> {
        let data = self.vault_data(cx)?;
        let known = data.known_host(&key.host, key.port);
        let check = KeyCheck {
            status: classify(known, key),
            policy: data.settings.unknown_host_keys,
            trusted_at: known.map(|k| k.trusted_at),
        };
        if matches!(check.status, holonet_ssh::KeyStatus::Trusted) {
            let (host, port) = (key.host.clone(), key.port);
            self.update_vault(
                move |d| {
                    if let Some(k) = d.known_host_mut(&host, port) {
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

    /// A changed key was refused: keep it for Known hosts' "Mismatch seen".
    pub fn record_mismatch(&mut self, key: &PresentedKey, cx: &mut Context<Self>) {
        let (host, port, public) = (key.host.clone(), key.port, key.public_key.clone());
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
        let panes: Vec<Entity<Pane>> = self.tabs.iter().flat_map(|t| t.read(cx).panes().to_vec()).collect();
        for pane in panes {
            pane.update(cx, |p, cx| p.set_terminal_settings(settings, cx));
        }
    }

    /// Open a new tab connected to a saved host.
    pub fn connect(&mut self, id: HostId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault_data(cx) else { return };
        let Some(host) = data.host(id).cloned() else { return };
        let key = match &host.auth {
            holonet_model::Auth::Key { key_id, .. } => data.key(*key_id).cloned(),
            holonet_model::Auth::Password { .. } => None,
        };
        // Estimate the terminal size from the window; the pane corrects it
        // on first paint (and tells the server).
        let viewport = window.viewport_size();
        let size = GridSize {
            cols: ((f32::from(viewport.width) - 56. - 16. - 32.) / 7.8).max(20.) as usize,
            rows: ((f32::from(viewport.height) - 44. - 60. - 16. - 40. - 40. - 28.) / 20.8).max(5.) as usize,
        };
        let app = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        let limits = Limits {
            connect_timeout: Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64),
            keep_alive: data.settings.connection.keep_alive_for(&host),
            terminal: data.settings.terminal.clone(),
        };
        let pane = cx.new(|cx| Pane::new(app, host, key, runtime, limits, size, window, cx));
        cx.subscribe_in(&pane, window, |this, pane, event: &SessionEvent, window, cx| match event {
            SessionEvent::Close => this.close_pane(pane, window, cx),
            SessionEvent::ReviewKnownHost => {
                let (host, port) = {
                    let t = pane.read(cx);
                    (t.host.address.trim().to_string(), t.host.port)
                };
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.show_known_host(&host, port, cx));
                }
            }
            SessionEvent::EditHost => {
                let id = pane.read(cx).host.id;
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.edit_host(id, window, cx));
                }
            }
        })
        .detach();
        let tab = cx.new(|cx| Tab::new(pane, cx));
        // The title bar shows each tab's name and state dot.
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        self.tabs.push(tab);
        self.select(Some(self.tabs.len() - 1), window, cx);
    }

    /// Close one pane; its tab closes with its last pane.
    fn close_pane(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.read(cx).contains(pane)) else { return };
        pane.update(cx, |p, _| p.disconnect());
        let tab = self.tabs[index].clone();
        if tab.update(cx, |t, cx| t.remove(pane, cx)) {
            self.close_tab_at(index, window, cx);
        } else {
            tab.update(cx, |t, cx| t.focus(window, cx));
        }
    }

    /// Close a whole tab and every pane in it.
    fn close_tab_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        for pane in tab.read(cx).panes().to_vec() {
            pane.update(cx, |p, _| p.disconnect());
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
                    TabInfo {
                        id: SharedString::from(format!("session-{i}")).into(),
                        name: t.name(cx),
                        kind: TabKind::Terminal,
                        state: t.state(cx),
                        color: theme::no_group(),
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
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
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
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .bg(theme::ground())
            .text_color(theme::ink())
            .font_family(theme::SANS)
            .child(title_bar)
            .child(div().flex_1().min_h_0().relative().child(body))
            .children(picker)
    }
}
