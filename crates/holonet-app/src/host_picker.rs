//! New-tab host picker: a small searchable list of saved hosts under the "+"
//! button (milestone 3 owner decision 3). Enter connects the highlighted host
//! in a new tab, Ctrl Enter or a row's SFTP button opens an SFTP tab; Esc
//! closes. The command palette (milestone 7) replaces it.

use gpui::{
    App, Context, Entity, EventEmitter, IntoElement, KeyBinding, Render, SharedString, Subscription, Window, actions,
    div, prelude::*, px,
};
use holonet_model::{HostId, VaultData};

use crate::theme::{self, TypeStyles};
use crate::ui::{Button, TextField, TextFieldEvent};

actions!(host_picker, [Up, Down, OpenFiles]);

const CONTEXT: &str = "HostPicker";
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 8.0;
const ROW_HEIGHT: f32 = 52.0;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("ctrl-enter", OpenFiles, Some(CONTEXT)),
    ]);
}

pub enum HostPickerEvent {
    Connect(HostId),
    /// An SFTP tab for the host.
    OpenFiles(HostId),
    NewHost,
    Dismiss,
}

#[derive(Clone)]
struct Entry {
    id: HostId,
    name: SharedString,
    target: SharedString,
    /// Lower-cased text the search matches against.
    haystack: String,
}

pub struct HostPicker {
    search: Entity<TextField>,
    hosts: Vec<Entry>,
    highlighted: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HostPickerEvent> for HostPicker {}

impl HostPicker {
    pub fn new(data: &VaultData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut hosts: Vec<Entry> = data
            .hosts
            .iter()
            .map(|h| data.resolved(h))
            .map(|h| {
                let target = if h.username.is_empty() {
                    format!("{}:{}", h.address.trim(), h.port)
                } else {
                    format!("{}@{}:{}", h.username, h.address.trim(), h.port)
                };
                let name = h.display_name().to_string();
                let haystack = format!("{name} {target} {}", h.tags.join(" ")).to_lowercase();
                Entry { id: h.id, name: name.into(), target: target.into(), haystack }
            })
            .collect();
        hosts.sort_by_key(|e| e.name.to_lowercase());
        let search = cx.new(|cx| TextField::new(cx, "Connect to a host"));
        search.read(cx).focus(window);
        let subscriptions =
            vec![cx.subscribe(&search, |this: &mut Self, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submitted => this.confirm(cx),
                TextFieldEvent::Cancelled => {
                    if field.read(cx).text().is_empty() {
                        cx.emit(HostPickerEvent::Dismiss);
                    } else {
                        field.update(cx, |f, cx| f.set_text("", cx));
                    }
                }
                _ => {
                    this.highlighted = 0;
                    cx.notify();
                }
            })];
        Self { search, hosts, highlighted: 0, _subscriptions: subscriptions }
    }

    fn matches(&self, cx: &App) -> Vec<Entry> {
        let query = self.search.read(cx).text().trim().to_lowercase();
        self.hosts.iter().filter(|e| query.split_whitespace().all(|word| e.haystack.contains(word))).cloned().collect()
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.matches(cx).get(self.highlighted) {
            cx.emit(HostPickerEvent::Connect(entry.id));
        }
    }

    fn confirm_files(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.matches(cx).get(self.highlighted) {
            cx.emit(HostPickerEvent::OpenFiles(entry.id));
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        if count > 0 {
            self.highlighted = (self.highlighted as isize + delta).rem_euclid(count as isize) as usize;
            cx.notify();
        }
    }
}

impl Render for HostPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        self.highlighted = self.highlighted.min(matches.len().saturating_sub(1));
        let highlighted = self.highlighted;
        let empty_vault = self.hosts.is_empty();

        let body = if empty_vault {
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_3)
                .p(theme::SPACE_3)
                .child(div().text_body().text_color(theme::ink_muted()).child("No saved hosts yet."))
                .child(
                    Button::primary("picker-new-host", "New host")
                        .icon("plus")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HostPickerEvent::NewHost))),
                )
                .into_any_element()
        } else if matches.is_empty() {
            div()
                .p(theme::SPACE_3)
                .text_body()
                .text_color(theme::ink_muted())
                .child("No host matches.")
                .into_any_element()
        } else {
            div()
                .id("picker-list")
                .max_h(px(ROW_HEIGHT * VISIBLE_ROWS))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(matches.into_iter().enumerate().map(|(i, e)| {
                    let id = e.id;
                    div()
                        .id(("picker-host", i))
                        .h(px(ROW_HEIGHT))
                        .flex_none()
                        .px(theme::SPACE_3)
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .rounded(theme::RADIUS_SM)
                        .cursor_pointer()
                        .when(i == highlighted, |d| d.bg(theme::raised()))
                        .hover(|s| s.bg(theme::raised()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_body().text_color(theme::ink()).truncate().child(e.name))
                                .child(
                                    div().text_mono_small().text_color(theme::ink_muted()).truncate().child(e.target),
                                ),
                        )
                        // An SFTP tab instead of a terminal.
                        .child(
                            div()
                                .id(("picker-sftp", i))
                                .flex_none()
                                .px(theme::SPACE_2)
                                .py(theme::SPACE_1)
                                .rounded(theme::RADIUS_SM)
                                .border_1()
                                .border_color(theme::line())
                                .text_meta()
                                .text_color(theme::ink_muted())
                                .hover(|s| s.bg(theme::card()).text_color(theme::ink()))
                                .child("SFTP")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(HostPickerEvent::OpenFiles(id))
                                })),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(HostPickerEvent::Connect(id))))
                }))
                .into_any_element()
        };

        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &OpenFiles, _, cx| this.confirm_files(cx)))
            .w(px(380.))
            .p(theme::SPACE_2)
            .flex()
            .flex_col()
            .gap(theme::SPACE_2)
            .bg(theme::card())
            .border_1()
            .border_color(theme::line_strong())
            .rounded(theme::RADIUS_LG)
            .shadow(theme::shadow_dialog())
            .when(!empty_vault, |d| d.child(self.search.clone()))
            .child(body)
            .when(!empty_vault, |d| {
                d.child(
                    div()
                        .px(theme::SPACE_2)
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child("Enter opens a terminal. Ctrl Enter opens SFTP."),
                )
            })
    }
}
