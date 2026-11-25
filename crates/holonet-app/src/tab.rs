//! A session tab (Terminal.dc.html): the session header and status bar for
//! the active pane, and the panes themselves on the `sunken` gutter.
//! Milestone 3 phase 4 holds one pane per tab; phase 5 adds splits.

use gpui::{Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};

use crate::session::Pane;
use crate::shell::TabState;
use crate::theme::{self, TypeStyles};

pub struct Tab {
    panes: Vec<Entity<Pane>>,
    active: usize,
    _subscriptions: Vec<Subscription>,
}

impl Tab {
    pub fn new(pane: Entity<Pane>, cx: &mut Context<Self>) -> Self {
        // Redraw the header, status bar and title bar when a pane changes.
        let subscriptions = vec![cx.observe(&pane, |_, _, cx| cx.notify())];
        Self { panes: vec![pane], active: 0, _subscriptions: subscriptions }
    }

    pub fn panes(&self) -> &[Entity<Pane>] {
        &self.panes
    }

    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.panes[self.active.min(self.panes.len() - 1)]
    }

    pub fn contains(&self, pane: &Entity<Pane>) -> bool {
        self.panes.contains(pane)
    }

    /// Remove a pane. Returns true when the tab has none left.
    pub fn remove(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) -> bool {
        self.panes.retain(|p| p != pane);
        self.active = self.active.min(self.panes.len().saturating_sub(1));
        cx.notify();
        self.panes.is_empty()
    }

    /// The tab's name and dot follow its active pane.
    pub fn name(&self, cx: &gpui::App) -> SharedString {
        self.active_pane().read(cx).name()
    }

    pub fn state(&self, cx: &gpui::App) -> TabState {
        self.active_pane().read(cx).tab_state()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane().update(cx, |p, cx| p.focus(window, cx));
    }
}

impl Render for Tab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_pane().clone();
        let (name, target, status) = {
            let p = active.read(cx);
            (p.name(), p.target_line(), p.status_bar())
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            // Session header: the active pane's host.
            .child(
                div()
                    .px(px(18.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(theme::line())
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::ink())
                            .child(name),
                    )
                    .child(div().text_mono_small().text_color(theme::ink_muted()).child(target)),
            )
            // Gutter and panes.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(theme::SPACE_2)
                    .bg(theme::sunken())
                    .flex()
                    .gap(theme::SPACE_2)
                    .children(self.panes.iter().map(|p| div().flex_1().min_w_0().h_full().child(p.clone()))),
            )
            .child(status)
    }
}
