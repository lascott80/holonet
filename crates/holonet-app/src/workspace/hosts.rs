//! The Hosts page (Main.dc.html): header with search and New host, the
//! group cards (which filter the hosts, and lead to Groups), cards or list,
//! and the detail panel. Empty vault: a welcome in FirstRun's layout,
//! without the import cards (milestone 7).

use gpui::{AnyElement, Context, Div, FontWeight, IntoElement, Rgba, SharedString, Window, div, prelude::*, px};
use holonet_model::{Auth, Host, HostId, RoleIcon, Timestamp, VaultData};

use super::groups::{GroupTag, host_count};
use super::{Page, Workspace};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, Segmented, group_square, icon, key_values, section_label};

const CARD_MIN: f32 = 270.;
const GAP: f32 = 12.;
const ASIDE: f32 = 320.;
const NAV: f32 = 220.;

/// "24 Nov 2026" in local time.
pub fn format_date(ts: Timestamp) -> String {
    jiff::Timestamp::from_second(ts.0)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%-d %b %Y").to_string())
        .unwrap_or_default()
}

fn role_icon(icon: RoleIcon) -> &'static str {
    match icon {
        RoleIcon::Server => "role-server",
        RoleIcon::Database => "role-database",
        RoleIcon::Shield => "role-shield",
        RoleIcon::Nas => "role-nas",
        RoleIcon::Chip => "role-chip",
        RoleIcon::Cloud => "role-cloud",
    }
}

/// A host's group color, or the "No group" gray.
fn host_color(data: &VaultData, host: &Host) -> Rgba {
    GroupTag::color_of(GroupTag::of(data, host).as_ref())
}

fn icon_tile(host: &Host, color: Rgba, size: f32, radius: f32, glyph: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bg(theme::with_alpha(color, 0.14))
        .child(icon(role_icon(host.icon), px(glyph), color))
}

fn last_connected(host: &Host) -> SharedString {
    match host.last_connected_at {
        Some(ts) => format!("Last connected {}", format_date(ts)).into(),
        None => "Never connected".into(),
    }
}

/// How a (resolved) host signs in, as the cards and detail panel say it.
fn sign_in(host: &Host, data: &VaultData) -> SharedString {
    let how: SharedString = match &host.auth {
        Auth::Password { saved: Some(_) } => "Password".into(),
        Auth::Password { saved: None } => "Password, asked".into(),
        Auth::Agent => "SSH agent".into(),
        Auth::Key { key_id, .. } => {
            data.key(*key_id).map(|k| k.name.clone()).unwrap_or_else(|| "Missing key".into()).into()
        }
    };
    match host.identity.and_then(|id| data.identity(id)) {
        Some(identity) => format!("{} ({how})", identity.name).into(),
        None => how,
    }
}

fn matches(host: &Host, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    [host.display_name(), host.address.as_str(), host.username.as_str()].iter().any(|s| s.to_lowercase().contains(&q))
        || host.tags.iter().any(|t| t.to_lowercase().contains(&q))
}

impl Workspace {
    pub(super) fn hosts_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault.as_ref().map(|v| v.data().clone()) else {
            return div().into_any_element();
        };
        if data.hosts.is_empty() {
            return self.welcome(cx).into_any_element();
        }
        let query = self.search.read(cx).text().trim().to_string();
        // Shown with their identity's username and sign-in.
        let resolved: Vec<Host> = data.hosts.iter().map(|h| data.resolved(h)).collect();
        // A group filter whose group was deleted shows everything.
        if self.group_filter.is_some_and(|g| data.group(g).is_none()) {
            self.group_filter = None;
        }
        let filter = self.group_filter;
        let mut hosts: Vec<&Host> =
            resolved.iter().filter(|h| matches(h, &query) && filter.is_none_or(|g| h.group == Some(g))).collect();
        hosts.sort_by_key(|h| h.display_name().to_lowercase());
        // Keep a selection while there is something to select.
        if self.selected.is_none_or(|id| !hosts.iter().any(|h| h.id == id)) {
            self.selected = hosts.first().map(|h| h.id);
        }
        let selected = self.selected;

        let width = f32::from(window.viewport_size().width) - NAV - ASIDE - 64.;
        let cols = (((width + GAP) / (CARD_MIN + GAP)).floor() as usize).clamp(1, 6);

        let count = data.hosts.len();
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_grow()
                    .flex_basis(px(160.))
                    .child(div().text_title().text_color(theme::ink()).child("Hosts"))
                    .child(div().text_body().text_color(theme::ink_muted()).child(SharedString::from(
                        match data.groups.len() {
                            0 => host_count(count),
                            1 => format!("{} in 1 group", host_count(count)),
                            n => format!("{} in {n} groups", host_count(count)),
                        },
                    ))),
            )
            // Narrower than drawn, so Import fits on the line with New host.
            .child(div().w(px(260.)).child(self.search.clone()))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(theme::SPACE_2)
                    .child(Button::secondary("import-hosts", "Import").header().on_click(
                        cx.listener(|this, _, _, cx| this.open_import(super::import::ImportKind::Config, cx)),
                    ))
                    .child(
                        Button::primary("new-host", "New host")
                            .header()
                            .icon("plus")
                            .on_click(cx.listener(|this, _, window, cx| this.open_form(None, window, cx))),
                    ),
            );

        let toggle = div().w(px(180.)).child(
            Segmented::new("view", vec!["Cards".into(), "List".into()], usize::from(!self.cards)).compact().on_change(
                {
                    let entity = cx.entity();
                    move |i, _, cx| {
                        entity.update(cx, |w, cx| {
                            w.cards = i == 0;
                            cx.notify()
                        })
                    }
                },
            ),
        );

        let body = if hosts.is_empty() {
            div()
                .py(theme::SPACE_6)
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::SPACE_3)
                .child(div().text_heading().text_color(theme::ink()).child(if query.is_empty() {
                    "No hosts in this group"
                } else {
                    "No hosts match"
                }))
                .child(div().text_body().text_color(theme::ink_muted()).child(SharedString::from(
                    if query.is_empty() {
                        "Put a host in this group from its form, under Group.".to_string()
                    } else {
                        format!("Nothing matches \u{201C}{query}\u{201D}.")
                    },
                )))
                .child(if query.is_empty() {
                    Button::secondary("clear-filter", "Show all hosts").on_click(cx.listener(|this, _, _, cx| {
                        this.group_filter = None;
                        cx.notify()
                    }))
                } else {
                    Button::secondary("clear-search", "Clear search").on_click(cx.listener(|this, _, _, cx| {
                        this.search.update(cx, |f, cx| f.set_text("", cx));
                    }))
                })
                .into_any_element()
        } else if self.cards {
            div()
                .grid()
                .grid_cols(cols as u16)
                .gap(px(GAP))
                .children(hosts.iter().map(|h| self.host_card(h, &data, selected == Some(h.id), cx)))
                .into_any_element()
        } else {
            self.host_list(&hosts, selected, &data, cx).into_any_element()
        };

        let detail =
            selected.and_then(|id| resolved.iter().find(|h| h.id == id)).map(|h| self.host_detail(h, &data, cx));

        div()
            .size_full()
            .flex()
            .child(
                div().id("hosts-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(26.))
                        .child(header)
                        .child(self.group_row(&data, cols, cx))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(theme::SPACE_3)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(theme::SPACE_3)
                                        .child(match filter.and_then(|g| data.group(g)) {
                                            None => section_label("All hosts").into_any_element(),
                                            Some(g) => div()
                                                .flex()
                                                .items_center()
                                                .gap(theme::SPACE_3)
                                                .child(section_label(g.display_name()))
                                                .child(
                                                    div()
                                                        .id("show-all-hosts")
                                                        .text_meta()
                                                        .text_color(theme::accent())
                                                        .cursor_pointer()
                                                        .child("Show all hosts")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.group_filter = None;
                                                            cx.notify()
                                                        })),
                                                )
                                                .into_any_element(),
                                        })
                                        .child(toggle),
                                )
                                .child(body),
                        ),
                ),
            )
            .children(detail)
            .into_any_element()
    }

    fn welcome(&mut self, cx: &mut Context<Self>) -> Div {
        div().size_full().flex().justify_center().px(theme::SPACE_6).py(px(56.)).child(
            div()
                .w_full()
                .max_w(px(760.))
                .flex()
                .flex_col()
                .gap(px(28.))
                .child(icon("globe", px(40.), theme::accent()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_3)
                        .child(
                            div()
                                .font_family(theme::DISPLAY)
                                .text_size(px(34.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::ink())
                                .child("Welcome to Holonet"),
                        )
                        .child(div().text_size(px(16.)).line_height(px(24.)).text_color(theme::ink_muted()).child(
                            "Your vault is set up and empty. Add the first host you want to connect to: an address, a user, and a key or password.",
                        )),
                )
                .child(div().child(
                    Button::primary("first-host", "New host")
                        .header()
                        .icon("plus")
                        .on_click(cx.listener(|this, _, window, cx| this.open_form(None, window, cx))),
                )),
        )
    }

    fn host_card(&self, host: &Host, data: &VaultData, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = host.id;
        let tag = GroupTag::of(data, host);
        let color = GroupTag::color_of(tag.as_ref());
        div()
            .id(SharedString::from(format!("card-{}", host.id.0)))
            .flex()
            .flex_col()
            .gap(theme::SPACE_3)
            .p(theme::SPACE_4)
            .rounded(theme::RADIUS_XL)
            .border_1()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::card_selected()).border_color(theme::accent()))
            .when(!selected, |d| {
                d.bg(theme::card()).border_color(theme::line()).hover(|s| s.shadow(theme::shadow_hover()))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .child(icon_tile(host, color, 40., 10., 20.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_body()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::ink())
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(SharedString::from(host.display_name().to_string())),
                            )
                            .child(
                                div()
                                    .text_mono_small()
                                    .text_color(theme::ink_muted())
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(SharedString::from(host.target())),
                            ),
                    )
                    // Main.dc.html: hosts with a Tailscale address carry the badge.
                    .when(host.tailscale().is_some(), |d| {
                        d.child(div().flex_none().child(crate::ui::Tag::Neutral("Tailscale".into())))
                    }),
            )
            .child(
                div()
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(theme::line())
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_meta()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::SPACE_2)
                            .text_color(theme::ink_muted())
                            .child(group_square(color))
                            .child(GroupTag::name_of(tag.as_ref())),
                    )
                    .child(div().text_color(theme::ink_muted()).child(match host.last_connected_at {
                        Some(ts) => SharedString::from(format_date(ts)),
                        None => "Never connected".into(),
                    })),
            )
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                this.selected = Some(id);
                if e.click_count() >= 2 {
                    this.connect(id, window, cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn host_list(&self, hosts: &[&Host], selected: Option<HostId>, data: &VaultData, cx: &mut Context<Self>) -> Div {
        let columns = |d: Div| d.grid().grid_cols(12).gap(theme::SPACE_3).items_center().px(theme::SPACE_4);
        let header = columns(div().py(px(10.)))
            .text_label()
            .text_color(theme::ink_muted())
            .child(div().col_span(1))
            .child(div().col_span(3).child("HOST"))
            .child(div().col_span(3).child("ADDRESS"))
            .child(div().col_span(2).child("GROUP"))
            .child(div().col_span(2).child("SIGN-IN"))
            .child(div().col_span(1).child("LAST"));
        let rows = hosts.iter().map(|h| {
            let id = h.id;
            let on = selected == Some(id);
            let tag = GroupTag::of(data, h);
            let color = GroupTag::color_of(tag.as_ref());
            columns(div())
                .id(SharedString::from(format!("row-{}", h.id.0)))
                .min_h(theme::CONTROL)
                .border_t_1()
                .border_color(theme::line())
                .cursor_pointer()
                .when(on, |d| d.bg(theme::card_selected()))
                .when(!on, |d| d.hover(|s| s.bg(theme::raised())))
                .child(div().col_span(1).child(icon(role_icon(h.icon), px(16.), color)))
                .child(
                    div()
                        .col_span(3)
                        .min_w_0()
                        .text_body()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .truncate()
                        .child(SharedString::from(h.display_name().to_string())),
                )
                .child(
                    div()
                        .col_span(3)
                        .min_w_0()
                        .text_mono_small()
                        .text_color(theme::ink_soft())
                        .truncate()
                        .child(SharedString::from(h.target())),
                )
                .child(
                    div()
                        .col_span(2)
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child(group_square(color))
                        .child(div().min_w_0().truncate().child(GroupTag::name_of(tag.as_ref()))),
                )
                .child(
                    div()
                        .col_span(2)
                        .min_w_0()
                        .truncate()
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child(sign_in(h, data)),
                )
                .child(div().col_span(1).min_w_0().truncate().text_meta().text_color(theme::ink_muted()).child(
                    match h.last_connected_at {
                        Some(ts) => SharedString::from(format_date(ts)),
                        None => "Never".into(),
                    },
                ))
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    this.selected = Some(id);
                    if e.click_count() >= 2 {
                        this.edit_host(id, window, cx);
                    }
                    cx.notify();
                }))
        });
        div().bg(theme::card()).rounded(theme::RADIUS_LG).child(header).children(rows)
    }

    fn host_detail(&self, host: &Host, data: &VaultData, cx: &mut Context<Self>) -> Div {
        let id = host.id;
        // Trusted under either of the host's addresses (they share trust).
        let others = VaultData::other_addresses(host, host.address.trim());
        let known = data.known_host_or_alias(host.address.trim(), host.port, &others).map(|(k, _)| k);
        let host_key: AnyElement = match known {
            Some(k) => div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_1)
                .child(div().text_mono_small().text_color(theme::ink_soft()).child(SharedString::from(format!(
                    "{} {}",
                    k.key_type_label(),
                    holonet_ssh::fingerprint(&k.public_key).unwrap_or_default()
                ))))
                .child({
                    let (h, p) = (k.host.clone(), k.port);
                    div()
                        .id("to-known-hosts")
                        .text_meta()
                        .text_color(theme::accent())
                        .cursor_pointer()
                        .child("Trusted in Known hosts")
                        .on_click(cx.listener(move |this, _, _, cx| this.show_known_host(&h, p, cx)))
                })
                .into_any_element(),
            None => div()
                .text_meta()
                .text_color(theme::ink_muted())
                .child("Not trusted yet. You verify it on first connect.")
                .into_any_element(),
        };
        let port: SharedString = host.port.to_string().into();
        let username: SharedString =
            if host.username.is_empty() { "Not set".into() } else { host.username.clone().into() };
        div()
            .w(px(ASIDE))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(theme::SPACE_5)
            .py(px(28.))
            .bg(theme::panel())
            .shadow(theme::shadow_panel())
            .child(icon_tile(host, host_color(data, host), 52., 12., 26.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .child(
                        div()
                            .text_panel_title()
                            .text_color(theme::ink())
                            .child(SharedString::from(host.display_name().to_string())),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(last_connected(host))),
            )
            .when(host.tailscale().is_some(), |d| {
                // Main.dc.html: the route, shown when there is a choice. It
                // may come from the group; choosing here makes it the host's own.
                let route = crate::host_form::ROUTES.iter().position(|r| *r == host.route).unwrap_or(0);
                let group_route = data.host(id).zip(data.host_group(host)).filter(|(h, _)| h.from_group.route);
                let help = match group_route {
                    Some((_, g)) => format!(
                        "{} From the {} group.",
                        crate::host_form::route_help(
                            host.route,
                            true,
                            data.settings.connection.fallback_wait.duration().as_secs()
                        ),
                        g.display_name()
                    ),
                    None => crate::host_form::route_help(
                        host.route,
                        true,
                        data.settings.connection.fallback_wait.duration().as_secs(),
                    ),
                };
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_2)
                        .child(section_label("Connect using"))
                        .child(
                            crate::ui::Segmented::new(
                                "detail-route",
                                crate::host_form::ROUTES
                                    .iter()
                                    .map(|r| SharedString::from(crate::host_form::route_label(*r)))
                                    .collect(),
                                route,
                            )
                            .compact()
                            .on_change({
                                let this = cx.weak_entity();
                                move |i, _, cx| {
                                    let route = crate::host_form::ROUTES[i.min(2)];
                                    let _ = this.update(cx, |this, cx| {
                                        this.save(
                                            move |d| {
                                                if let Some(h) = d.host_mut(id) {
                                                    h.route = route;
                                                    h.from_group.route = false;
                                                }
                                            },
                                            cx,
                                        )
                                    });
                                }
                            }),
                        )
                        .child(crate::ui::field_help(help, false)),
                )
            })
            .child(key_values(vec![
                ("Primary address".into(), SharedString::from(host.address.clone()).into_any_element()),
                (
                    "Tailscale address".into(),
                    SharedString::from(host.tailscale().unwrap_or("Not set").to_string()).into_any_element(),
                ),
                ("Port".into(), port.into_any_element()),
                ("Username".into(), username.into_any_element()),
                ("Authentication".into(), sign_in(host, data).into_any_element()),
                // Main.dc.html: the jump host by name, or None.
                (
                    "Jump host".into(),
                    SharedString::from(
                        host.jump_host
                            .map(|id| data.host(id).map_or("Deleted host", |j| j.display_name()))
                            .unwrap_or("None")
                            .to_string(),
                    )
                    .into_any_element(),
                ),
                (
                    "Group".into(),
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .child(group_square(host_color(data, host)))
                        .child(GroupTag::name_of(GroupTag::of(data, host).as_ref()))
                        .into_any_element(),
                ),
            ]))
            .child(div().flex().flex_col().gap(theme::SPACE_2).child(section_label("Host key")).child(host_key))
            .when(!host.tags.is_empty(), |d| {
                d.child(div().flex().flex_col().gap(theme::SPACE_2).child(section_label("Tags")).child(
                    div().text_body().text_color(theme::ink_soft()).child(SharedString::from(host.tags.join(", "))),
                ))
            })
            .child(div().flex_1())
            .child(
                Button::primary("connect-host", "Connect")
                    .full_width()
                    .on_click(cx.listener(move |this, _, window, cx| this.connect(id, window, cx))),
            )
            // Main.dc.html: Open SFTP beside the connect action.
            .child(
                Button::secondary("open-sftp", "Open SFTP")
                    .icon("folder")
                    .full_width()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_files(id, window, cx))),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::SPACE_2)
                    .child(
                        div().flex_1().child(
                            Button::secondary("edit-host", "Edit")
                                .full_width()
                                .on_click(cx.listener(move |this, _, window, cx| this.edit_host(id, window, cx))),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::secondary("duplicate-host", "Duplicate")
                                .full_width()
                                .on_click(cx.listener(move |this, _, _, cx| this.duplicate_host(id, cx))),
                        ),
                    ),
            )
    }

    pub(crate) fn edit_host(&mut self, id: HostId, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.vault.as_ref().and_then(|v| v.data().host(id).cloned());
        if host.is_some() {
            self.open_form(host, window, cx);
        }
    }

    fn duplicate_host(&mut self, id: HostId, cx: &mut Context<Self>) {
        let Some(copy) = self.vault.as_ref().and_then(|v| v.data().host(id)).map(Host::duplicate) else { return };
        let new_id = copy.id;
        self.save(|d| d.hosts.push(copy), cx);
        self.selected = Some(new_id);
        self.page = Page::Hosts;
    }
}

impl Workspace {
    /// Main.dc.html's Groups row: a card per group (its color along the top,
    /// its host count) that filters the hosts, and "Manage groups".
    fn group_row(&self, data: &VaultData, cols: usize, cx: &mut Context<Self>) -> Div {
        let mut groups: Vec<&holonet_model::Group> = data.groups.iter().collect();
        groups.sort_by_key(|g| g.display_name().to_lowercase());
        let filter = self.group_filter;
        let cards: AnyElement = if groups.is_empty() {
            div()
                .text_body()
                .text_color(theme::ink_muted())
                .child("No groups yet. A group gives its hosts a color and shared defaults.")
                .into_any_element()
        } else {
            div()
                .grid()
                // Group cards are narrower than host cards (180px against 270px).
                .grid_cols(((cols * 3).div_ceil(2)).clamp(1, 8) as u16)
                .gap(px(GAP))
                .children(groups.iter().map(|g| {
                    let id = g.id;
                    let on = filter == Some(id);
                    div()
                        .id(SharedString::from(format!("group-card-{}", g.id.0)))
                        .flex()
                        .flex_col()
                        // No overflow_hidden: in a Taffy grid it lets the item
                        // shrink to nothing. The strip rounds its own corners.
                        .rounded(theme::RADIUS_LG)
                        .border_1()
                        .cursor_pointer()
                        .when(on, |d| d.bg(theme::card_selected()).border_color(theme::accent()))
                        .when(!on, |d| {
                            d.bg(theme::card()).border_color(theme::line()).hover(|s| s.shadow(theme::shadow_hover()))
                        })
                        .child(div().h(px(2.)).flex_none().rounded_t(theme::RADIUS_LG).bg(theme::group_color(g.color)))
                        .child(
                            div()
                                .px(theme::SPACE_4)
                                .py(px(13.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .font_family(theme::DISPLAY)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(px(15.))
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Picking the filtered group again shows all hosts.
                            this.group_filter = if this.group_filter == Some(id) { None } else { Some(id) };
                            cx.notify()
                        }))
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div().flex().items_center().justify_between().child(section_label("Groups")).child(
                    div()
                        .id("manage-groups")
                        .text_meta()
                        .text_color(theme::accent())
                        .cursor_pointer()
                        .py(px(6.))
                        .child("Manage groups")
                        .on_click(cx.listener(move |this, _, _, cx| this.open_groups(filter, cx))),
                ),
            )
            .child(cards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_name_address_user_and_tags() {
        let mut h = Host::new("10.0.0.5");
        h.label = "nas-01".into();
        h.username = "admin".into();
        h.tags = vec!["storage".into()];
        for q in ["", "NAS", "10.0.0", "adm", "stor"] {
            assert!(matches(&h, q), "{q}");
        }
        assert!(!matches(&h, "web"));
    }

    #[test]
    fn dates_are_day_month_year() {
        // 2026-11-24 12:00 UTC; any time zone within ±11h keeps the day.
        let s = format_date(Timestamp(1_795_521_600));
        assert!(s.ends_with("Nov 2026"), "{s}");
    }
}
