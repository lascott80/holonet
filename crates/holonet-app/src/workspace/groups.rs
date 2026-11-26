//! The Groups page (Groups.dc.html), opened from "Manage groups" on the
//! Hosts page: group cards on the left, the editor on the right (name,
//! color, and the defaults its hosts take), and the delete prompt.

use gpui::{AnyElement, Context, Entity, IntoElement, Rgba, SharedString, Window, div, prelude::*, px};
use holonet_model::{Group, GroupColor, GroupId, GroupKey, Host, HostId, Route, VaultData};

use super::{Page, Workspace};
use crate::host_form::{ROUTES, route_label};
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, ButtonKind, Checkbox, Select, TextField, TextFieldEvent, dialog, dialog_buttons, field, modal,
    section_label,
};

/// Wide enough for the six 44px swatches in one row.
const EDITOR_W: f32 = 360.;

/// A host's group as shown on cards, tabs and pane headers.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupTag {
    pub name: SharedString,
    pub color: Rgba,
}

impl GroupTag {
    /// The host's group, or `None` when it has none.
    pub fn of(data: &VaultData, host: &Host) -> Option<Self> {
        data.host_group(host)
            .map(|g| GroupTag { name: g.display_name().to_string().into(), color: theme::group_color(g.color) })
    }

    /// The color for a host: its group's, or the "No group" gray.
    pub fn color_of(tag: Option<&GroupTag>) -> Rgba {
        tag.map_or_else(theme::no_group, |t| t.color)
    }

    /// The name for a host: its group's, or "No group".
    pub fn name_of(tag: Option<&GroupTag>) -> SharedString {
        tag.map_or_else(|| "No group".into(), |t| t.name.clone())
    }
}

/// "4 hosts", "1 host".
pub fn host_count(n: usize) -> String {
    format!("{n} host{}", if n == 1 { "" } else { "s" })
}

/// The group's key as the cards and host form say it.
pub fn key_text(data: &VaultData, key: GroupKey) -> String {
    match key {
        GroupKey::AskEachTime => "Ask each time".into(),
        GroupKey::Key(id) => data.key(id).map_or_else(|| "Deleted key".to_string(), |k| k.name.clone()),
    }
}

/// The group's jump host by name, or "None".
pub fn jump_text(data: &VaultData, jump: Option<HostId>) -> String {
    match jump {
        None => "None".into(),
        Some(id) => data.host(id).map_or_else(|| "Deleted host".to_string(), |h| h.display_name().to_string()),
    }
}

/// The route as Groups.dc.html writes it on cards.
pub fn route_text(route: Route) -> &'static str {
    match route {
        Route::Automatic => "Automatic, Tailscale fallback",
        Route::Primary => "Primary",
        Route::Tailscale => "Tailscale",
    }
}

/// The editor's state: the group being edited (`None` for a new one) and
/// its fields.
pub(crate) struct GroupsUi {
    editing: Option<GroupId>,
    name: Entity<TextField>,
    color: GroupColor,
    /// "Ask each time", then the vault's keys.
    key: Entity<Select>,
    key_ids: Vec<holonet_model::KeyId>,
    /// "None", then the vault's hosts.
    jump: Entity<Select>,
    jump_ids: Vec<HostId>,
    route: Entity<Select>,
    warn: bool,
    /// The delete prompt, with "Also delete its hosts".
    delete: Option<bool>,
    problem: Option<SharedString>,
}

impl GroupsUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let name = cx.new(|cx| TextField::new(cx, "e.g. Production"));
        let key = cx.new(|cx| Select::new(cx, vec!["Ask each time".into()], 0));
        let jump = cx.new(|cx| Select::new(cx, vec!["None".into()], 0));
        let route =
            cx.new(|cx| Select::new(cx, ROUTES.iter().map(|r| SharedString::from(route_label(*r))).collect(), 0));
        Self {
            editing: None,
            name,
            color: GroupColor::default(),
            key,
            key_ids: Vec::new(),
            jump,
            jump_ids: Vec::new(),
            route,
            warn: false,
            delete: None,
            problem: None,
        }
    }

    pub fn subscribe(&self, cx: &mut Context<Workspace>) -> Vec<gpui::Subscription> {
        vec![cx.subscribe(&self.name, |this: &mut Workspace, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submitted => this.save_group(cx),
            TextFieldEvent::Changed => {
                this.groups_ui.problem = None;
                cx.notify()
            }
            TextFieldEvent::Cancelled => {}
        })]
    }
}

impl Workspace {
    /// Open the Groups page with a group in the editor (the first one, or a
    /// new group when there are none).
    pub(crate) fn open_groups(&mut self, select: Option<GroupId>, cx: &mut Context<Self>) {
        let Some(data) = self.vault_data().cloned() else { return };
        let mut groups: Vec<&Group> = data.groups.iter().collect();
        groups.sort_by_key(|g| g.display_name().to_lowercase());
        let pick = select.or_else(|| groups.first().map(|g| g.id));
        self.load_group(&data, pick, cx);
        self.page = Page::Groups;
        cx.notify();
    }

    /// Fill the editor from a group, or empty for a new one.
    fn load_group(&mut self, data: &VaultData, id: Option<GroupId>, cx: &mut Context<Self>) {
        let group = id.and_then(|id| data.group(id)).cloned().unwrap_or_else(|| Group::new(""));
        let ui = &mut self.groups_ui;
        ui.editing = id.filter(|id| data.group(*id).is_some());
        ui.color = group.color;
        ui.warn = group.warn;
        ui.delete = None;
        ui.problem = None;
        ui.name.update(cx, |f, cx| f.set_text(group.name.clone(), cx));
        let mut keys: Vec<_> = data.keys.iter().collect();
        keys.sort_by_key(|k| k.name.to_lowercase());
        ui.key_ids = keys.iter().map(|k| k.id).collect();
        let key_index = match group.key {
            GroupKey::AskEachTime => 0,
            GroupKey::Key(id) => ui.key_ids.iter().position(|k| *k == id).map_or(0, |i| i + 1),
        };
        let key_options = std::iter::once(SharedString::from("Ask each time"))
            .chain(keys.iter().map(|k| SharedString::from(k.name.clone())))
            .collect();
        ui.key.update(cx, |s, cx| s.set_options(key_options, key_index, cx));
        let mut hosts: Vec<&Host> = data.hosts.iter().collect();
        hosts.sort_by_key(|h| h.display_name().to_lowercase());
        ui.jump_ids = hosts.iter().map(|h| h.id).collect();
        let jump_index = group.jump_host.and_then(|j| ui.jump_ids.iter().position(|h| *h == j)).map_or(0, |i| i + 1);
        let jump_options = std::iter::once(SharedString::from("None"))
            .chain(hosts.iter().map(|h| SharedString::from(h.display_name().to_string())))
            .collect();
        ui.jump.update(cx, |s, cx| s.set_options(jump_options, jump_index, cx));
        let route_index = ROUTES.iter().position(|r| *r == group.route).unwrap_or(0);
        ui.route.update(cx, |s, cx| s.set_selected(route_index, cx));
    }

    /// The group the editor describes.
    fn edited_group(&self, cx: &Context<Self>) -> Group {
        let ui = &self.groups_ui;
        let mut group = ui
            .editing
            .and_then(|id| self.vault_data().and_then(|d| d.group(id)).cloned())
            .unwrap_or_else(|| Group::new(""));
        group.name = ui.name.read(cx).text().trim().to_string();
        group.color = ui.color;
        group.key = match ui.key.read(cx).selected().checked_sub(1).and_then(|i| ui.key_ids.get(i)) {
            Some(id) => GroupKey::Key(*id),
            None => GroupKey::AskEachTime,
        };
        group.jump_host = ui.jump.read(cx).selected().checked_sub(1).and_then(|i| ui.jump_ids.get(i)).copied();
        group.route = ROUTES[ui.route.read(cx).selected().min(2)];
        group.warn = ui.warn;
        group
    }

    fn save_group(&mut self, cx: &mut Context<Self>) {
        let group = self.edited_group(cx);
        if group.name.is_empty() {
            self.groups_ui.problem = Some("Give the group a name.".into());
            cx.notify();
            return;
        }
        let taken = self.vault_data().is_some_and(|d| {
            d.groups.iter().any(|g| g.id != group.id && g.name.trim().eq_ignore_ascii_case(&group.name))
        });
        if taken {
            self.groups_ui.problem = Some("Another group has this name.".into());
            cx.notify();
            return;
        }
        let id = group.id;
        self.save(
            move |d| match d.group_mut(id) {
                Some(existing) => *existing = group,
                None => d.groups.push(group),
            },
            cx,
        );
        // The card appearing or changing is the confirmation; a notice
        // would push the editor's buttons down.
        self.groups_ui.editing = Some(id);
        self.groups_ui.problem = None;
        cx.notify();
    }

    fn delete_group(&mut self, also_hosts: bool, cx: &mut Context<Self>) {
        let Some(id) = self.groups_ui.editing else { return };
        self.save(move |d| drop(d.delete_group(id, also_hosts)), cx);
        if self.group_filter == Some(id) {
            self.group_filter = None;
        }
        self.groups_ui.delete = None;
        self.open_groups(None, cx);
    }

    pub(super) fn groups_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault_data().cloned() else { return div().into_any_element() };
        let mut groups: Vec<&Group> = data.groups.iter().collect();
        groups.sort_by_key(|g| g.display_name().to_lowercase());
        let editing = self.groups_ui.editing;

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
                    .child(
                        div()
                            .flex()
                            .gap(theme::SPACE_2)
                            .text_meta()
                            .child(
                                div()
                                    .id("groups-crumb-hosts")
                                    .text_color(theme::accent())
                                    .cursor_pointer()
                                    .child("Hosts")
                                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Hosts, cx))),
                            )
                            .child(div().text_color(theme::ink_muted()).child("/ Groups")),
                    )
                    .child(div().text_title().text_color(theme::ink()).child("Groups"))
                    .child(div().text_body().text_color(theme::ink_muted()).child(
                        "A group gives its hosts a color and shared defaults. A host can override any default.",
                    )),
            )
            .child(Button::primary("new-group", "New group").header().on_click(cx.listener(|this, _, window, cx| {
                if let Some(data) = this.vault_data().cloned() {
                    this.load_group(&data, None, cx);
                    this.groups_ui.name.read(cx).focus(window);
                    cx.notify();
                }
            })));

        let list: AnyElement = if groups.is_empty() {
            div()
                .p(theme::SPACE_5)
                .rounded(theme::RADIUS_LG)
                .border_1()
                .border_dashed()
                .border_color(theme::line_strong())
                .flex()
                .flex_col()
                .gap(theme::SPACE_2)
                .child(div().text_heading().text_color(theme::ink()).child("No groups yet"))
                .child(div().text_body().text_color(theme::ink_muted()).child(
                    "Name the first group in the editor, choose its color and defaults, then Save group. Put hosts in it from the host form.",
                ))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_3)
                .children(groups.iter().map(|g| self.group_card(&data, g, editing == Some(g.id), cx)))
                .into_any_element()
        };

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("groups-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div().px(theme::SPACE_6).py(px(28.)).flex().flex_col().gap(px(20.)).child(header).child(list),
                ),
            )
            .child(self.group_editor(&data, cx))
            .children(self.group_delete_dialog(&data, cx))
            .into_any_element()
    }

    fn group_card(&self, data: &VaultData, g: &Group, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = g.id;
        let value = |label: &'static str, text: String| {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .text_meta()
                .text_color(theme::ink_muted())
                .child(label)
                // Wraps rather than cutting "Automatic, Tailscale fallback" short.
                .child(div().text_body().text_color(theme::ink()).child(SharedString::from(text)))
        };
        // GPUI has one border color per element, so the group's 3px left
        // edge (Groups.dc.html) is a strip inside the clipped card.
        let content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(24.))
            .gap_y(px(10.))
            .pl(px(15.))
            .pr(px(18.))
            .py(theme::SPACE_4)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_grow()
                    .flex_basis(px(180.))
                    .min_w_0()
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(17.))
                            .text_color(theme::ink())
                            .truncate()
                            .child(SharedString::from(g.display_name().to_string())),
                    )
                    .child(
                        div()
                            .text_meta()
                            .text_color(theme::ink_muted())
                            .child(SharedString::from(host_count(data.group_hosts(g.id).len()))),
                    ),
            )
            .child(
                div()
                    .flex_grow()
                    .flex_basis(px(320.))
                    .grid()
                    .grid_cols(3)
                    .gap(theme::SPACE_3)
                    .child(value("Key", key_text(data, g.key)))
                    .child(value("Jump host", jump_text(data, g.jump_host)))
                    .child(value("Connect using", route_text(g.route).to_string())),
            );
        div()
            .id(SharedString::from(format!("group-{}", g.id.0)))
            .flex()
            .rounded(theme::RADIUS_XL)
            .border_1()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::card_selected()).border_color(theme::accent()))
            .when(!selected, |d| {
                d.bg(theme::card()).border_color(theme::line()).hover(|s| s.shadow(theme::shadow_hover()))
            })
            .child(div().w(px(3.)).flex_none().bg(theme::group_color(g.color)))
            .child(content)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(data) = this.vault_data().cloned() {
                    this.load_group(&data, Some(id), cx);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn group_editor(&self, data: &VaultData, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.groups_ui;
        let current = ui.color;
        let swatches = GroupColor::ALL.map(|c| {
            crate::ui::power::swatch(
                SharedString::from(format!("swatch-{}", c.label())),
                theme::group_color(c),
                c == current,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.groups_ui.color = c;
                cx.notify();
            }))
        });
        let hosts = ui.editing.map_or(0, |id| data.group_hosts(id).len());
        div()
            .id("group-editor")
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
                "Edit group"
            } else {
                "New group"
            }))
            .child(field("Name", ui.name.clone(), ui.problem.clone().map(|p| (p, true))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(crate::ui::field_label("Color"))
                    .child(div().flex().flex_wrap().gap(theme::SPACE_2).children(swatches))
                    .child(crate::ui::field_help(
                        "Used on host cards, tabs and the top edge of terminal and SFTP panes.",
                        false,
                    )),
            )
            .child(div().pt(theme::SPACE_1).child(section_label("Defaults for hosts in this group")))
            .child(field("Key", ui.key.clone(), None))
            .child(field("Jump host", ui.jump.clone(), None))
            .child(field("Connect using", ui.route.clone(), None))
            .child(Checkbox::new("group-warn", "Warn before broadcast or multi-host snippets", ui.warn).on_toggle({
                let entity = cx.entity();
                move |v, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.groups_ui.warn = v;
                        cx.notify()
                    })
                }
            }))
            .child(div().flex_1())
            .child(
                Button::primary("save-group", "Save group")
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| this.save_group(cx))),
            )
            .when(ui.editing.is_some(), |d| {
                d.child(
                    Button::new("delete-group", "Delete group", ButtonKind::DestructiveOutline).full_width().on_click(
                        cx.listener(|this, _, _, cx| {
                            this.groups_ui.delete = Some(false);
                            cx.notify()
                        }),
                    ),
                )
            })
            .when(ui.editing.is_none() && !data.groups.is_empty(), |d| {
                d.child(Button::quiet("cancel-group", "Cancel").full_width().on_click(cx.listener(|this, _, _, cx| {
                    this.open_groups(None, cx);
                })))
            })
            .when(hosts > 0, |d| {
                d.child(crate::ui::field_help(
                    format!("{} in this group. A host can override any default on its form.", host_count(hosts)),
                    false,
                ))
            })
            .into_any_element()
    }

    fn group_delete_dialog(&self, data: &VaultData, cx: &mut Context<Self>) -> Option<AnyElement> {
        let also = self.groups_ui.delete?;
        let id = self.groups_ui.editing?;
        let group = data.group(id)?;
        let hosts = data.group_hosts(id).len();
        let (note, label) = if also {
            ("The group and its hosts are removed. Their server keys stay in Known hosts.", "Delete group and hosts")
        } else if hosts > 0 {
            ("Its hosts are kept and become ungrouped.", "Delete group")
        } else {
            ("The group has no hosts.", "Delete group")
        };
        let close = {
            let entity = cx.entity();
            move |_: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |this, cx| {
                    this.groups_ui.delete = None;
                    cx.notify()
                })
            }
        };
        Some(
            modal(
                "delete-group",
                px(460.),
                dialog(true, format!("Delete the {} group?", group.display_name()), note)
                    .when(hosts > 0, |d| {
                        d.child(
                            Checkbox::new("group-also-hosts", format!("Also delete its {}", host_count(hosts)), also)
                                .on_toggle({
                                    let entity = cx.entity();
                                    move |v, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.groups_ui.delete = Some(v);
                                            cx.notify()
                                        })
                                    }
                                }),
                        )
                    })
                    .child(dialog_buttons([
                        Button::secondary("delete-group-cancel", "Cancel").on_click({
                            let close = close.clone();
                            move |_, window, cx| close(window, cx)
                        }),
                        Button::new("delete-group-confirm", label, ButtonKind::DestructiveFill)
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_group(also, cx))),
                    ])),
                close,
            )
            .into_any_element(),
        )
    }
}
