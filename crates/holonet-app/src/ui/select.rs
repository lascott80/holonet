//! Select: a field-styled button that opens a list of options.
//!
//! Styled as Field.md describes for controls (`control` high, `card`, `line`
//! border, `radius-md`, `accent` border when focused). The popup is drawn
//! above everything with GPUI's `deferred` + `anchored`, closes on an outside
//! click or Escape, and the arrow keys move through it.

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton, SharedString, Window, actions,
    anchored, deferred, div, prelude::*, px,
};

use crate::theme::{self, TypeStyles};
use crate::ui::icon;

actions!(select, [Toggle, Close, Next, Previous, Confirm, FocusNext, FocusPrev]);

const CONTEXT: &str = "Select";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", Toggle, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("escape", Close, Some(CONTEXT)),
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Previous, Some(CONTEXT)),
        KeyBinding::new("tab", FocusNext, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrev, Some(CONTEXT)),
    ]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectEvent {
    Changed(usize),
}

pub struct Select {
    focus: FocusHandle,
    options: Vec<SharedString>,
    selected: usize,
    /// Highlighted row while open.
    highlighted: usize,
    open: bool,
    mono: bool,
}

impl EventEmitter<SelectEvent> for Select {}

impl Focusable for Select {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Select {
    pub fn new(cx: &mut Context<Self>, options: Vec<SharedString>, selected: usize) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            focus: cx.focus_handle().tab_stop(true),
            options,
            selected,
            highlighted: selected,
            open: false,
            mono: false,
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_options(&mut self, options: Vec<SharedString>, selected: usize, cx: &mut Context<Self>) {
        self.options = options;
        self.selected = selected.min(self.options.len().saturating_sub(1));
        self.highlighted = self.selected;
        cx.notify();
    }

    pub fn set_selected(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.options.len() && index != self.selected {
            self.selected = index;
            self.highlighted = index;
            cx.emit(SelectEvent::Changed(index));
            cx.notify();
        }
    }

    fn toggle(&mut self, _: &Toggle, _: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        self.highlighted = self.selected;
        cx.notify();
    }

    fn close(&mut self, _: &Close, _: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.set_selected(self.highlighted, cx);
            self.open = false;
            cx.notify();
        } else {
            self.toggle(&Toggle, window, cx);
        }
    }

    fn next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlighted = (self.highlighted + 1).min(self.options.len().saturating_sub(1));
            cx.notify();
        } else {
            self.set_selected((self.selected + 1).min(self.options.len().saturating_sub(1)), cx);
        }
    }

    fn previous(&mut self, _: &Previous, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlighted = self.highlighted.saturating_sub(1);
            cx.notify();
        } else {
            self.set_selected(self.selected.saturating_sub(1), cx);
        }
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
        window.focus_next();
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
        window.focus_prev();
    }
}

impl Render for Select {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let label = self.options.get(self.selected).cloned().unwrap_or_default();
        let mono = self.mono;

        let popup = self.open.then(|| {
            let rows = self.options.iter().cloned().enumerate().map(|(i, option)| {
                let chosen = i == self.selected;
                let lit = i == self.highlighted;
                div()
                    .id(i)
                    .h(px(40.))
                    .px(theme::SPACE_3)
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(theme::RADIUS_SM)
                    .cursor_pointer()
                    .when(lit, |d| d.bg(theme::raised()))
                    .hover(|s| s.bg(theme::raised()))
                    .text_color(if chosen { theme::ink() } else { theme::ink_soft() })
                    .child(option)
                    .when(chosen, |d| d.child(icon("check", px(14.), theme::accent())))
                    .on_click(cx.listener(move |select, _, window, cx| {
                        select.set_selected(i, cx);
                        select.open = false;
                        window.focus(&select.focus);
                        cx.notify();
                    }))
            });
            deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id("select-popup")
                        .mt(theme::SPACE_1)
                        .min_w(px(200.))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p(theme::SPACE_1)
                        .flex()
                        .flex_col()
                        .bg(theme::card())
                        .border_1()
                        .border_color(theme::line_strong())
                        .rounded(theme::RADIUS_MD)
                        .shadow(theme::shadow_hover())
                        .when(mono, |d| d.text_mono())
                        .when(!mono, |d| d.text_body())
                        .occlude()
                        .on_mouse_down_out(cx.listener(|select, _, _, cx| {
                            select.open = false;
                            cx.notify();
                        }))
                        .children(rows),
                ),
            )
            .with_priority(1)
        });

        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .w_full()
            .min_w(px(120.))
            .child(
                div()
                    .id("select-button")
                    .h(theme::CONTROL)
                    .w_full()
                    .pl(theme::SPACE_3)
                    .pr(theme::SPACE_2)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme::SPACE_2)
                    .bg(theme::card())
                    .border_1()
                    .border_color(if focused || self.open { theme::accent() } else { theme::line() })
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .when(mono, |d| d.text_mono())
                    .when(!mono, |d| d.text_body())
                    .text_color(theme::ink())
                    .child(div().flex_1().overflow_hidden().child(label))
                    .child(icon("chevron-down", px(16.), theme::ink_muted()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|select, _, window, cx| {
                            window.focus(&select.focus);
                            select.toggle(&Toggle, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .children(popup)
    }
}
