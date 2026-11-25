//! The Holonet window: the vault gate (create or unlock), the workspace, and
//! session tabs.
//!
//! Sessions belong to the app, not the workspace, so locking the vault (which
//! drops the workspace and its unlocked vault) leaves them connected. They
//! reappear when the vault is unlocked (decisions.md).

use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use holonet_model::{HostId, KnownHostSource, Mismatch, Timestamp, VaultData};
use holonet_ssh::{PresentedKey, classify};
use holonet_vault::UnlockedVault;

use crate::session::{KeyCheck, SessionEvent, SessionTab};
use crate::shell::{TabInfo, TabKind, TitleBar};
use crate::terminal::GridSize;
use crate::theme;
use crate::ui::icon;
use crate::vault_screens::{CreateVault, LockReason, Locked};
use crate::workspace::{Page, Workspace};

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
    sessions: Vec<Entity<SessionTab>>,
    /// The session tab shown, or `None` for the vault view.
    active: Option<usize>,
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
        Self { path, stage, runtime, sessions: Vec::new(), active: None }
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
        let open = self.sessions.len();
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

    /// Open a session tab for a saved host.
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
        let timeout = Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64);
        let tab = cx.new(|cx| SessionTab::new(app, host, key, runtime, timeout, size, window, cx));
        cx.subscribe_in(&tab, window, |this, tab, event: &SessionEvent, window, cx| match event {
            SessionEvent::Close => this.close_session(tab, window, cx),
            SessionEvent::EditHost => {
                let id = tab.read(cx).host.id;
                this.show_vault(window, cx);
                if let Some(w) = this.workspace().cloned() {
                    w.update(cx, |w, cx| w.edit_host(id, window, cx));
                }
            }
        })
        .detach();
        self.sessions.push(tab);
        self.select(Some(self.sessions.len() - 1), window, cx);
    }

    fn close_session(&mut self, tab: &Entity<SessionTab>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.sessions.iter().position(|t| t == tab) else { return };
        let tab = self.sessions.remove(index);
        tab.update(cx, |t, _| t.disconnect());
        let next = match self.active {
            Some(a) if a == index => {
                if self.sessions.is_empty() {
                    None
                } else {
                    Some(index.min(self.sessions.len() - 1))
                }
            }
            Some(a) if a > index => Some(a - 1),
            other => other,
        };
        self.select(next, window, cx);
    }

    fn select(&mut self, tab: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = tab;
        if let Some(t) = tab.and_then(|i| self.sessions.get(i)).cloned() {
            t.update(cx, |t, cx| t.focus(window, cx));
        }
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unlocked = matches!(self.stage, Stage::Open(_));
        let session = self.active.filter(|_| unlocked).and_then(|i| self.sessions.get(i)).cloned();
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
            self.sessions
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let t = t.read(cx);
                    TabInfo {
                        id: SharedString::from(format!("session-{i}")).into(),
                        name: t.name(),
                        kind: TabKind::Terminal,
                        state: t.tab_state(),
                        color: theme::no_group(),
                        active: self.active == Some(i),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut title_bar = TitleBar::new(self.active.is_none() || !unlocked, tabs);
        if unlocked {
            title_bar = title_bar
                .on_vault(cx.listener(|this, _, window, cx| this.show_vault(window, cx)))
                .on_tab({
                    let entity = cx.entity();
                    move |i, window, cx| entity.update(cx, |this, cx| this.select(Some(i), window, cx))
                })
                .on_close_tab({
                    let entity = cx.entity();
                    move |i, window, cx| {
                        entity.update(cx, |this, cx| {
                            if let Some(tab) = this.sessions.get(i).cloned() {
                                this.close_session(&tab, window, cx);
                            }
                        })
                    }
                })
                // "+": pick a host to connect to.
                .on_new_tab(cx.listener(|this, _, window, cx| {
                    this.show_vault(window, cx);
                    if let Some(w) = this.workspace().cloned() {
                        w.update(cx, |w, cx| w.set_page(Page::Hosts, cx));
                    }
                }));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .text_color(theme::ink())
            .font_family(theme::SANS)
            .child(title_bar)
            .child(div().flex_1().min_h_0().relative().child(body))
    }
}
