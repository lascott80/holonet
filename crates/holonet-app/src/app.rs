//! The Holonet window: the vault gate (create or unlock), then the workspace.

use std::path::PathBuf;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*};
use holonet_vault::UnlockedVault;

use crate::shell::TitleBar;
use crate::theme;
use crate::vault_screens::{CreateVault, LockReason, Locked};
use crate::workspace::Workspace;

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
}

impl HolonetApp {
    pub fn new(path: PathBuf, runtime: tokio::runtime::Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.entity().downgrade();
        let stage = if holonet_vault::exists(&path) {
            let p = path.clone();
            Stage::Locked(cx.new(|cx| Locked::new(app, p, LockReason::Startup, window, cx)))
        } else {
            let p = path.clone();
            Stage::Create(cx.new(|cx| CreateVault::new(app, p, window, cx)))
        };
        Self { path, stage, runtime }
    }

    /// The vault was created or unlocked.
    pub fn opened(&mut self, vault: UnlockedVault, restored_from_backup: bool, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let runtime = self.runtime.clone();
        self.stage = Stage::Open(cx.new(|cx| Workspace::new(app, vault, restored_from_backup, runtime, cx)));
        cx.notify();
    }

    /// "Lock now": dropping the workspace drops the unlocked vault, which
    /// wipes its key and secrets from memory.
    pub fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let path = self.path.clone();
        self.stage = Stage::Locked(cx.new(|cx| Locked::new(app, path, LockReason::Manual, window, cx)));
        cx.notify();
    }
}

impl Render for HolonetApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.stage {
            Stage::Create(view) => view.clone().into_any_element(),
            Stage::Locked(view) => view.clone().into_any_element(),
            Stage::Open(view) => view.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .text_color(theme::ink())
            .font_family(theme::SANS)
            .child(TitleBar::new(true, Vec::new()))
            .child(div().flex_1().min_h_0().relative().child(body))
    }
}
