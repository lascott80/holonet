//! The Snippets page (Snippets.dc.html): the library as cards with a
//! filter, the editor on the right, and the delete prompt. Running a
//! snippet from here ("Run on") comes with the drawer in phase 6.

use gpui::{AnyElement, Context, Entity, IntoElement, SharedString, Subscription, Window, div, prelude::*, px};
use holonet_model::{Snippet, SnippetId, VaultData};

use super::{Page, Workspace};
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, ButtonKind, Checkbox, TextField, TextFieldEvent, dialog, dialog_buttons, field, field_help, modal,
};

const EDITOR_W: f32 = 380.;
/// Snippets.dc.html: cards at least 260px wide, 12px apart.
const CARD_MIN_W: f32 = 260.;

/// The editor's state: the snippet being edited (`None` for a new one).
pub(crate) struct SnippetsUi {
    editing: Option<SnippetId>,
    filter: Entity<TextField>,
    name: Entity<TextField>,
    command: Entity<TextField>,
    press_enter: bool,
    confirm: bool,
    /// The delete prompt is open.
    delete: bool,
    problem: Option<SharedString>,
}

impl SnippetsUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            editing: None,
            filter: cx.new(|cx| TextField::new(cx, "Filter by name or command").leading_icon("search")),
            name: cx.new(|cx| TextField::new(cx, "e.g. Restart service")),
            command: cx.new(|cx| TextField::new(cx, "sudo systemctl restart ${service}").mono()),
            press_enter: true,
            confirm: false,
            delete: false,
            problem: None,
        }
    }

    pub fn subscribe(&self, cx: &mut Context<Workspace>) -> Vec<Subscription> {
        let mut subs =
            vec![cx.subscribe(&self.filter, |_: &mut Workspace, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Cancelled => field.update(cx, |f, cx| f.set_text("", cx)),
                _ => cx.notify(),
            })];
        for f in [&self.name, &self.command] {
            subs.push(cx.subscribe(f, |this: &mut Workspace, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submitted => this.save_snippet(cx),
                TextFieldEvent::Changed => {
                    this.sn.problem = None;
                    cx.notify()
                }
                TextFieldEvent::Cancelled => {}
            }));
        }
        subs
    }
}

/// "Startup snippet for staging-api.", "… for a, b and c.", or not used.
pub fn startup_note(data: &VaultData, id: SnippetId) -> String {
    let mut names: Vec<String> =
        data.startup_users(id).iter().filter_map(|h| data.host(*h)).map(|h| h.display_name().to_string()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    match names.as_slice() {
        [] => "Not used as a startup snippet.".into(),
        [one] => format!("Startup snippet for {one}."),
        [rest @ .., last] => format!("Startup snippet for {} and {last}.", rest.join(", ")),
    }
}

/// The editor's note under Command (Snippets.dc.html).
fn variable_note(snippet: &Snippet) -> String {
    let names = snippet.variables();
    if names.is_empty() {
        "No variables. Write ${name} to be asked for a value each time this runs.".into()
    } else {
        format!("Holonet asks for {} each time this runs. Write a variable as ${{name}}.", names.join(", "))
    }
}

impl Workspace {
    /// Open the Snippets page with a snippet in the editor (the first one,
    /// or a new snippet when there are none).
    pub(crate) fn open_snippets(&mut self, select: Option<SnippetId>, cx: &mut Context<Self>) {
        let Some(data) = self.vault_data().cloned() else { return };
        let pick = select.or_else(|| sorted(&data).first().map(|s| s.id));
        self.load_snippet(&data, pick, cx);
        self.page = Page::Snippets;
        cx.notify();
    }

    /// Fill the editor from a snippet, or empty for a new one.
    fn load_snippet(&mut self, data: &VaultData, id: Option<SnippetId>, cx: &mut Context<Self>) {
        let snippet = id.and_then(|id| data.snippet(id)).cloned().unwrap_or_else(|| Snippet::new("", ""));
        let ui = &mut self.sn;
        ui.editing = id.filter(|id| data.snippet(*id).is_some());
        ui.press_enter = snippet.press_enter;
        ui.confirm = snippet.confirm;
        ui.delete = false;
        ui.name.update(cx, |f, cx| f.set_text(snippet.name.clone(), cx));
        ui.command.update(cx, |f, cx| f.set_text(snippet.command.clone(), cx));
        ui.problem = None;
    }

    /// The snippet the editor describes.
    fn edited_snippet(&self, cx: &Context<Self>) -> Snippet {
        let ui = &self.sn;
        let mut snippet = ui
            .editing
            .and_then(|id| self.vault_data().and_then(|d| d.snippet(id)).cloned())
            .unwrap_or_else(|| Snippet::new("", ""));
        snippet.name = ui.name.read(cx).text().trim().to_string();
        snippet.command = ui.command.read(cx).text().trim().to_string();
        snippet.press_enter = ui.press_enter;
        snippet.confirm = ui.confirm;
        snippet
    }

    fn save_snippet(&mut self, cx: &mut Context<Self>) {
        let snippet = self.edited_snippet(cx);
        let problem = if snippet.name.is_empty() {
            Some("Give the snippet a name.")
        } else if snippet.command.is_empty() {
            Some("Write the command to run.")
        } else {
            None
        };
        if let Some(p) = problem {
            self.sn.problem = Some(p.into());
            cx.notify();
            return;
        }
        let id = snippet.id;
        self.save(
            move |d| match d.snippet_mut(id) {
                Some(existing) => *existing = snippet,
                None => d.snippets.push(snippet),
            },
            cx,
        );
        // The card appearing or changing is the confirmation.
        self.sn.editing = Some(id);
        self.sn.problem = None;
        cx.notify();
    }

    /// Save what the editor shows as a new snippet, "… copy", and edit it.
    fn duplicate_snippet(&mut self, cx: &mut Context<Self>) {
        let edited = self.edited_snippet(cx);
        let mut copy = Snippet::new(format!("{} copy", edited.display_name()), edited.command.clone());
        copy.press_enter = edited.press_enter;
        copy.confirm = edited.confirm;
        let id = copy.id;
        self.save(move |d| d.snippets.push(copy), cx);
        self.open_snippets(Some(id), cx);
    }

    fn delete_snippet(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sn.editing else { return };
        self.save(move |d| drop(d.delete_snippet(id)), cx);
        self.open_snippets(None, cx);
    }

    pub(super) fn snippets_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault_data().cloned() else { return div().into_any_element() };
        let all = sorted(&data);
        let query = self.sn.filter.read(cx).text().trim().to_lowercase();
        let shown: Vec<&Snippet> = all
            .iter()
            .copied()
            .filter(|s| query.is_empty() || format!("{} {}", s.name, s.command).to_lowercase().contains(&query))
            .collect();

        let header = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(theme::SPACE_3)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(240.))
                    .gap(theme::SPACE_1)
                    .child(div().text_title().text_color(theme::ink()).child("Snippets"))
                    .child(div().text_body().text_color(theme::ink_muted()).child(
                        "Saved commands you can run in any session, on several hosts at once, or when a host connects.",
                    )),
            )
            .child(Button::primary("new-snippet", "New snippet").header().on_click(cx.listener(
                |this, _, window, cx| {
                    if let Some(data) = this.vault_data().cloned() {
                        this.load_snippet(&data, None, cx);
                        this.sn.name.read(cx).focus(window);
                        cx.notify();
                    }
                },
            )));

        let empty_box = |title: Option<&'static str>, body: &'static str| {
            div()
                .p(theme::SPACE_5)
                .rounded(theme::RADIUS_LG)
                .border_1()
                .border_dashed()
                .border_color(theme::line_strong())
                .flex()
                .flex_col()
                .gap(theme::SPACE_2)
                .children(title.map(|t| div().text_heading().text_color(theme::ink()).child(t)))
                .child(div().text_body().text_color(theme::ink_muted()).child(body))
                .into_any_element()
        };
        let list: AnyElement = if all.is_empty() {
            empty_box(
                Some("No snippets yet"),
                "Write the first one in the editor: a name and the command, with ${name} for anything that changes between runs. Then Save.",
            )
        } else if shown.is_empty() {
            div()
                .py(px(28.))
                .flex()
                .justify_center()
                .text_body()
                .text_color(theme::ink_muted())
                .child("No snippets match this filter.")
                .into_any_element()
        } else {
            // Snippets.dc.html: as many 260px columns as fit beside the editor.
            // The side navigation is 220 wide, as on the Hosts page.
            let room = f32::from(window.viewport_size().width) - 220. - EDITOR_W - 64.;
            let cols = ((room + 12.) / (CARD_MIN_W + 12.)).floor().clamp(1., 4.) as u16;
            div()
                .grid()
                .grid_cols(cols)
                .gap(px(12.))
                .children(shown.iter().map(|s| self.snippet_card(s, cx)))
                .into_any_element()
        };

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("snippets-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(header)
                        .when(!all.is_empty(), |d| d.child(self.sn.filter.clone()))
                        .child(list),
                ),
            )
            .child(self.snippet_editor(&data, cx))
            .children(self.snippet_delete_dialog(&data, cx))
            .into_any_element()
    }

    fn snippet_card(&self, s: &Snippet, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id;
        let selected = self.sn.editing == Some(id);
        div()
            .id(SharedString::from(format!("snippet-{}", id.0)))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme::SPACE_2)
            .p(px(14.))
            .rounded(theme::RADIUS_LG)
            .border_1()
            .cursor_pointer()
            .bg(if selected { theme::raised() } else { theme::card() })
            .border_color(if selected { theme::accent() } else { theme::line() })
            .hover(|d| d.border_color(theme::line_strong()))
            .child(div().text_body_strong().text_color(theme::ink()).truncate().child(s.display_name().to_string()))
            .child(
                div()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(theme::RADIUS_MD)
                    .bg(theme::ground())
                    .border_1()
                    .border_color(theme::line())
                    // A flex row gives the text a definite width to cut at;
                    // as a block child it measured as empty and showed only "…".
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_mono_small()
                            .text_color(theme::ink_soft())
                            .truncate()
                            .child(s.command.replace(['\r', '\n'], " ")),
                    ),
            )
            .child(
                div()
                    .text_meta()
                    .text_color(if s.confirm { theme::warn() } else { theme::ink_muted() })
                    .child(s.run_note()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(data) = this.vault_data().cloned() {
                    this.load_snippet(&data, Some(id), cx);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn snippet_editor(&self, data: &VaultData, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.sn;
        let edited = self.edited_snippet(cx);
        let toggle = |id: &'static str, label: &'static str, value: bool, set: fn(&mut SnippetsUi, bool)| {
            let this = cx.weak_entity();
            Checkbox::new(id, label, value).on_toggle(move |v, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    set(&mut this.sn, v);
                    cx.notify()
                });
            })
        };
        div()
            .id("snippet-editor")
            .overflow_y_scroll()
            .w(px(EDITOR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(theme::SPACE_4)
            .px(theme::SPACE_5)
            .py(px(28.))
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::line())
            .shadow(theme::shadow_panel())
            .child(div().text_panel_title().text_color(theme::ink()).child(if ui.editing.is_some() {
                "Edit snippet"
            } else {
                "New snippet"
            }))
            .child(field("Name", ui.name.clone(), None).flex_none())
            .child(field("Command", ui.command.clone(), Some((variable_note(&edited).into(), false))).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(toggle("snippet-enter", "Press Enter after inserting", ui.press_enter, |u, v| {
                        u.press_enter = v
                    }))
                    .child(toggle("snippet-confirm", "Ask before running", ui.confirm, |u, v| u.confirm = v)),
            )
            .children(ui.editing.map(|id| {
                div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(startup_note(data, id)))
            }))
            .children(ui.problem.clone().map(|p| field_help(p, true)))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(theme::SPACE_2)
                    .child(
                        div().flex_1().child(
                            Button::secondary("save-snippet", "Save")
                                .full_width()
                                .on_click(cx.listener(|this, _, _, cx| this.save_snippet(cx))),
                        ),
                    )
                    .when(ui.editing.is_some(), |d| {
                        d.child(
                            div().flex_1().child(
                                Button::secondary("duplicate-snippet", "Duplicate")
                                    .full_width()
                                    .on_click(cx.listener(|this, _, _, cx| this.duplicate_snippet(cx))),
                            ),
                        )
                        .child(
                            div().flex_1().child(
                                Button::new("delete-snippet", "Delete", ButtonKind::DestructiveOutline)
                                    .full_width()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sn.delete = true;
                                        cx.notify()
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn snippet_delete_dialog(&self, data: &VaultData, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.sn.editing.filter(|_| self.sn.delete)?;
        let snippet = data.snippet(id)?;
        let users = data.startup_users(id);
        let body = if users.is_empty() {
            "The snippet is removed from the library. This cannot be undone.".to_string()
        } else {
            let note = startup_note(data, id);
            let note = note.trim_end_matches('.');
            format!(
                "It is the {}. {} will no longer run a startup snippet. This cannot be undone.",
                note.replacen("Startup snippet", "startup snippet", 1),
                if users.len() == 1 { "That host" } else { "Those hosts" }
            )
        };
        let close = {
            let this = cx.weak_entity();
            move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |this, cx| {
                    this.sn.delete = false;
                    cx.notify()
                });
            }
        };
        Some(
            modal(
                "delete-snippet",
                px(460.),
                dialog(true, format!("Delete {}?", snippet.display_name()), body).child(dialog_buttons([
                    Button::secondary("delete-snippet-cancel", "Cancel").on_click({
                        let close = close.clone();
                        move |_, window, cx| close(window, cx)
                    }),
                    Button::new("delete-snippet-confirm", "Delete snippet", ButtonKind::DestructiveFill)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_snippet(cx))),
                ])),
                close,
            )
            .into_any_element(),
        )
    }
}

/// The library by name.
fn sorted(data: &VaultData) -> Vec<&Snippet> {
    let mut all: Vec<&Snippet> = data.snippets.iter().collect();
    all.sort_by_key(|s| s.display_name().to_lowercase());
    all
}

#[cfg(test)]
mod tests {
    use holonet_model::{Host, Snippet, VaultData};

    use super::startup_note;

    #[test]
    fn startup_note_names_the_hosts() {
        let mut data = VaultData::default();
        let snippet = Snippet::new("Tail app log", "journalctl -u app -f");
        let id = snippet.id;
        data.snippets.push(snippet);
        assert_eq!(startup_note(&data, id), "Not used as a startup snippet.");
        for name in ["staging-api", "prod-web-02", "prod-web-01"] {
            let mut host = Host::new(name);
            host.label = name.into();
            host.startup_snippet = Some(id);
            data.hosts.push(host);
            if name == "staging-api" {
                assert_eq!(startup_note(&data, id), "Startup snippet for staging-api.");
            }
        }
        assert_eq!(startup_note(&data, id), "Startup snippet for prod-web-01, prod-web-02 and staging-api.");
    }
}
