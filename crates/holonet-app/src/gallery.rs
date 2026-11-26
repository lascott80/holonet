//! `holonet --gallery`: every shared control in every state, inside the app
//! shell, for checking against docs/design (milestone 2, phase 3; the
//! Keychain controls from milestone 4, the SFTP screen's from milestone 5,
//! milestone 6's switch, swatches, rule cards, host chips and broadcast
//! strip, and milestone 7's History rows and saved output, palette rows,
//! update notice and First run cards).

use gpui::{AnyElement, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};

use crate::shell::{NavEntry, SideNav, TabInfo, TabKind, TabState, TitleBar};
use crate::theme::{self, TypeStyles};
use crate::ui::files::{
    FileRow, breadcrumbs, check_box, file_header, file_row, nav_button, permissions_grid, progress_bar, selection_bar,
};
use crate::ui::power::{RuleCard, broadcast_strip, host_chip, kind_tag, rule_card, swatch};
use crate::ui::ship::{
    HistoryRow, first_run_link, history_header, history_row, import_card, key_chip, log_output, palette_row,
    palette_section, update_card,
};
use crate::ui::{
    Button, ButtonKind, CalloutKind, Checkbox, Segmented, Select, SelectEvent, Tag, TextField, TextFieldEvent, callout,
    code_box, dialog, dialog_buttons, field, fieldset, group_square, key_values, modal, section_label,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenDialog {
    None,
    Standard,
    Warning,
}

pub struct Gallery {
    label: Entity<TextField>,
    address: Entity<TextField>,
    port: Entity<TextField>,
    password: Entity<TextField>,
    problem: Entity<TextField>,
    long: Entity<TextField>,
    group: Entity<Select>,
    timeout: Entity<Select>,
    slider: Entity<crate::ui::Slider>,
    route: usize,
    auth: usize,
    keep_alive: bool,
    forward_agent: bool,
    forget_key: bool,
    dialog: OpenDialog,
    nav: NavEntry,
    active_tab: Option<usize>,
    /// Keychain section: the selected table row and the import picks.
    kc_row: usize,
    picks: [bool; 2],
    /// Files section: the selected rows and the permissions grid.
    file_picks: [bool; 5],
    mode_bits: [bool; 9],
    /// Power section: the two rule switches, the chosen swatch, the chips.
    rules_on: [bool; 2],
    swatch: usize,
    chips: [bool; 4],
    /// Milestone 7 section: the selected History row and palette row.
    hist_row: usize,
    palette_at: usize,
    log: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let label = cx.new(|cx| TextField::new(cx, "Defaults to the address"));
        let address = cx.new(|cx| TextField::new(cx, "Hostname or IP address").mono());
        let port = cx.new(|cx| TextField::new(cx, "22").mono().with_text("22"));
        let password = cx.new(|cx| TextField::new(cx, "Leave empty to be asked on connect").password());
        let problem = cx.new(|cx| {
            let mut f = TextField::new(cx, "Hostname or IP address").mono().with_text("root@pi.local");
            f.set_problem(true, cx);
            f
        });
        let long = cx.new(|cx| {
            TextField::new(cx, "").mono().with_text(
                "ssh -J bastion-east deploy@staging-worker-02.internal.example.com -p 2222 -o ServerAliveInterval=30",
            )
        });
        let group = cx.new(|cx| {
            Select::new(
                cx,
                ["No group", "Production", "Staging", "Home lab", "Clients"].map(SharedString::from).to_vec(),
                0,
            )
        });
        let timeout = cx
            .new(|cx| Select::new(cx, ["15 minutes idle", "1 hour idle", "Never"].map(SharedString::from).to_vec(), 0));

        let slider = cx.new(|cx| crate::ui::Slider::new(cx, 10, 20, 13, |v| format!("{v}px")));
        let subscriptions = vec![
            cx.subscribe(&slider, |this: &mut Self, _, event: &crate::ui::SliderEvent, cx| {
                let crate::ui::SliderEvent::Changed(v) = *event;
                this.note(format!("slider {v}"), cx);
            }),
            cx.subscribe(&address, |this: &mut Self, field, event: &TextFieldEvent, cx| {
                let text = field.read(cx).text().to_string();
                this.note(format!("address {event:?}: {text}"), cx);
            }),
            cx.subscribe(&group, |this: &mut Self, _, event: &SelectEvent, cx| {
                this.note(format!("group {event:?}"), cx);
            }),
        ];
        label.read(cx).focus(window);
        Self {
            label,
            address,
            port,
            password,
            problem,
            long,
            group,
            timeout,
            slider,
            route: 0,
            auth: 0,
            keep_alive: true,
            forward_agent: false,
            forget_key: false,
            dialog: OpenDialog::None,
            nav: NavEntry::Hosts,
            active_tab: None,
            kc_row: 0,
            picks: [true, false],
            file_picks: [false, true, true, false, false],
            mode_bits: [true, true, true, true, false, true, false, false, false],
            rules_on: [true, false],
            swatch: 1,
            chips: [true, true, false, false],
            hist_row: 0,
            palette_at: 0,
            log: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn note(&mut self, line: String, cx: &mut Context<Self>) {
        self.log.push(line);
        if self.log.len() > 4 {
            self.log.remove(0);
        }
        cx.notify();
    }

    fn section(title: &str, body: impl IntoElement) -> AnyElement {
        div().flex().flex_col().gap(theme::SPACE_3).child(section_label(title)).child(body).into_any_element()
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = vec![
            TabInfo {
                id: "tab-a".into(),
                name: "prod-web-01".into(),
                kind: TabKind::Terminal,
                state: TabState::Connected,
                color: theme::accent(),
                active: self.active_tab == Some(0),
            },
            TabInfo {
                id: "tab-b".into(),
                name: "staging-worker-02".into(),
                kind: TabKind::Terminal,
                state: TabState::Connecting,
                color: theme::group_violet(),
                active: self.active_tab == Some(1),
            },
            TabInfo {
                id: "tab-c".into(),
                name: "nas-01".into(),
                kind: TabKind::Sftp,
                state: TabState::Failed,
                color: theme::no_group(),
                active: self.active_tab == Some(2),
            },
        ];

        let buttons = div()
            .flex()
            .flex_wrap()
            .gap(theme::SPACE_2)
            .items_center()
            .child(Button::primary("b-primary", "Save and connect"))
            .child(Button::secondary("b-secondary", "Test connection"))
            .child(Button::quiet("b-quiet", "Cancel"))
            .child(Button::new("b-dout", "Delete host", ButtonKind::DestructiveOutline))
            .child(Button::new("b-dfill", "Delete host", ButtonKind::DestructiveFill))
            .child(Button::primary("b-header", "New host").header().icon("plus"))
            .child(Button::secondary("b-compact", "Find").compact().icon("search"))
            .child(Button::primary("b-disabled", "Save").disabled(true));

        let fields = fieldset("General")
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::SPACE_4)
                    .child(field("Label", self.label.clone(), None))
                    .child(field("Group", self.group.clone(), None)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::SPACE_4)
                    .child(div().flex_grow().min_w(px(240.)).child(field(
                        "Primary address (required)",
                        self.address.clone(),
                        None,
                    )))
                    .child(div().w(px(110.)).child(field("Port", self.port.clone(), None))),
            )
            .child(field(
                "Primary address (required)",
                self.problem.clone(),
                Some(("Use a hostname or an IP address, without spaces or a user name.".into(), true)),
            ))
            .child(field(
                "Password",
                self.password.clone(),
                Some(("Leave empty to be asked on connect.".into(), false)),
            ))
            .child(field("Long value (scrolls to keep the caret visible)", self.long.clone(), None))
            .child(field("Lock vault after", self.timeout.clone(), None));

        let entity = cx.entity();
        let choices = div()
            .flex()
            .flex_col()
            .gap(theme::SPACE_3)
            .child(field(
                "Connect using",
                Segmented::new(
                    "route",
                    ["Automatic", "Primary", "Tailscale"].map(SharedString::from).to_vec(),
                    self.route,
                )
                .on_change({
                    let entity = entity.clone();
                    move |i, _, cx| {
                        entity.update(cx, |g, cx| {
                            g.route = i;
                            cx.notify()
                        })
                    }
                }),
                None,
            ))
            .child(field("Font size (slider: drag, click, arrow keys)", self.slider.clone(), None))
            .child(field(
                "Authentication",
                Segmented::new("auth", ["SSH key", "Password"].map(SharedString::from).to_vec(), self.auth).on_change(
                    {
                        let entity = entity.clone();
                        move |i, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.auth = i;
                                cx.notify()
                            })
                        }
                    },
                ),
                None,
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(28.))
                    .child(Checkbox::new("keep-alive", "Keep connection alive", self.keep_alive).on_toggle({
                        let entity = entity.clone();
                        move |v, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.keep_alive = v;
                                cx.notify()
                            })
                        }
                    }))
                    .child(Checkbox::new("agent", "Agent forwarding", self.forward_agent).on_toggle({
                        let entity = entity.clone();
                        move |v, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.forward_agent = v;
                                cx.notify()
                            })
                        }
                    })),
            );

        let tags = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::SPACE_3)
            .child(Tag::Group("Production".into(), theme::accent()))
            .child(Tag::Group("Staging".into(), theme::group_violet()))
            .child(Tag::Neutral("Tailscale".into()))
            .child(Tag::Key("Ctrl Shift P".into()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .child(group_square(theme::group_mint()))
                    .child(div().text_body().text_color(theme::ink()).child("Home lab")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .child(group_square(theme::no_group()))
                    .child(div().text_body().text_color(theme::ink_muted()).child("No group")),
            );

        let display = div()
            .flex()
            .flex_wrap()
            .gap(theme::SPACE_4)
            .child(div().w(px(320.)).flex().flex_col().gap(theme::SPACE_3).child(key_values(vec![
                ("Primary address".into(), "192.168.1.40".into_any_element()),
                ("Port".into(), "22".into_any_element()),
                ("Username".into(), "admin".into_any_element()),
                ("Authentication".into(), "home-rsa-4096".into_any_element()),
            ])))
            .child(
                div()
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_3)
                    .child(code_box("ssh admin@192.168.1.40 -p 2222"))
                    .child(
                        callout(CalloutKind::Info)
                            .child("Holonet will offer the keys loaded in the Windows OpenSSH agent."),
                    )
                    .child(
                        callout(CalloutKind::Success)
                            .child(div().text_color(theme::ink()).child("Reachable. Host key not yet trusted"))
                            .child(div().text_mono_small().child("ED25519 SHA256:[FINGERPRINT]")),
                    )
                    .child(callout(CalloutKind::Warning).child("Add an address to test or save this host.")),
            );

        let open = |kind: OpenDialog| {
            let entity = entity.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |g, cx| {
                    g.dialog = kind;
                    cx.notify()
                })
            }
        };
        let dialogs = div()
            .flex()
            .gap(theme::SPACE_2)
            .child(Button::secondary("open-standard", "Open standard dialog").on_click(open(OpenDialog::Standard)))
            .child(
                Button::new("open-warning", "Open warning dialog", ButtonKind::DestructiveOutline)
                    .on_click(open(OpenDialog::Warning)),
            );

        let typography = div()
            .flex()
            .flex_col()
            .gap(theme::SPACE_2)
            .child(div().text_title().text_color(theme::ink()).child("Hosts (title, Space Grotesk 600)"))
            .child(div().text_panel_title().text_color(theme::ink()).child("nas-01 (panel title)"))
            .child(div().text_heading().text_color(theme::ink()).child("Verify this host before connecting (heading)"))
            .child(
                div()
                    .text_body()
                    .text_color(theme::ink_soft())
                    .child("Server keys you have trusted. (body, Inter 400)"),
            )
            .child(div().text_body_strong().text_color(theme::ink()).child("prod-web-01 (body strong, Inter 500)"))
            .child(div().text_meta().text_color(theme::ink_muted()).child("Last connected 24 Nov 2026 (meta)"))
            .child(div().text_mono().text_color(theme::ink_soft()).child("deploy@10.20.0.11:22 (mono)"))
            .child(div().text_mono_small().text_color(theme::ink_soft()).child("SHA256:[FINGERPRINT] (mono small)"));

        let keychain = {
            use crate::ui::{TableColumn, TableRow, add_card, identity_card, name_cell, pick_row, side_panel, table};
            let columns = [
                TableColumn { label: "NAME", width: None },
                TableColumn { label: "TYPE", width: Some(110.) },
                TableColumn { label: "PROTECTED BY", width: None },
                TableColumn { label: "HOSTS", width: Some(80.) },
            ];
            let sample = [
                ("work-ed25519", "Ed25519", "Passphrase + Windows Hello", false, "4"),
                ("home-rsa-4096", "RSA 4096", "Passphrase", false, "2"),
                ("old-laptop", "ECDSA P-256", "No passphrase", true, "0"),
            ];
            let rows = sample
                .iter()
                .enumerate()
                .map(|(i, (name, kind, protection, warn, hosts))| {
                    let entity = entity.clone();
                    let text = |t: &'static str, warn: bool| {
                        div()
                            .text_body()
                            .text_color(if warn { theme::warn() } else { theme::ink() })
                            .child(t)
                            .into_any_element()
                    };
                    TableRow {
                        id: SharedString::from(format!("g-key-{i}")),
                        cells: vec![
                            name_cell(*name, "SHA256:[FINGERPRINT]"),
                            div().text_mono_small().text_color(theme::ink()).child(*kind).into_any_element(),
                            text(protection, *warn),
                            text(hosts, false),
                        ],
                        selected: self.kc_row == i,
                        on_click: Some(Box::new(move |_, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.kc_row = i;
                                g.note(format!("key row {i}"), cx);
                            })
                        })),
                    }
                })
                .collect();
            let pick = |i: usize, title: &'static str, detail: &'static str, enabled: bool| {
                let entity = entity.clone();
                let checked = self.picks.get(i).copied().unwrap_or(false);
                pick_row(format!("g-pick-{i}"), title, detail, checked, enabled, move |v, _, cx| {
                    entity.update(cx, |g, cx| {
                        if let Some(p) = g.picks.get_mut(i) {
                            *p = v;
                        }
                        g.note(format!("pick {i}: {v}"), cx);
                    })
                })
            };
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_4)
                .child(table(&columns, rows))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme::SPACE_3)
                        .child(identity_card("g-id-1", "deploy", "Key work-ed25519", "User deploy · 4 hosts"))
                        .child(identity_card(
                            "g-id-2",
                            "home admin",
                            "Password, asked on connect",
                            "User admin · no hosts",
                        ))
                        .child(add_card("g-id-new", "New identity")),
                )
                .child(
                    div()
                        .p(theme::SPACE_4)
                        .bg(theme::card())
                        .rounded(theme::RADIUS_LG)
                        .child(pick(0, "~/.ssh · id_ed25519", "Ed25519 · passphrase", true))
                        .child(pick(1, "~/.ssh · id_rsa", "RSA 3072 · no passphrase", true))
                        .child(pick(2, "~/.ssh · work.ppk", "Already in the vault", false)),
                )
                .child(
                    div().h(px(220.)).flex().justify_end().bg(theme::ground()).child(
                        side_panel(320., "Generate key")
                            .child(div().text_body().text_color(theme::ink_muted()).child("Panel content goes here.")),
                    ),
                )
        };

        let files = {
            let rows: [(&'static str, gpui::Rgba, &str, &str, &str, &str); 5] = [
                ("folder", theme::accent(), "assets", "", "22 Nov 2026", "rwxr-xr-x"),
                ("folder", theme::group_violet(), "current", "", "21 Nov 2026", "rwxrwxrwx"),
                ("file-code", theme::group_mint(), "index.html", "18.4 KB", "24 Nov 2026", "rw-r--r--"),
                ("file-archive", theme::ink_soft(), "backup-2026-11.tar.gz", "1.2 GB", "1 Nov 2026", "rw-r-----"),
                ("file-lock", theme::ink_muted(), ".env", "312 B", "12 Oct 2026", "rw-------"),
            ];
            let count = self.file_picks.iter().filter(|p| **p).count();
            let all = count == rows.len();
            let crumbs = ["/", "…", "www", "site", "releases"].map(SharedString::from).to_vec();
            let on_crumb: crate::ui::IndexHandler = {
                let entity = entity.clone();
                std::rc::Rc::new(move |i, _, cx| entity.update(cx, |g, cx| g.note(format!("crumb {i}"), cx)))
            };
            let action = |id: &'static str, label: &'static str| {
                let entity = entity.clone();
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                    entity.update(cx, |g, cx| g.note(format!("{id}: {label}"), cx))
                }
            };
            let actions = vec![
                Button::primary("g-download", "← Download")
                    .compact()
                    .on_click(action("g-download", "Download"))
                    .into_any_element(),
                Button::secondary("g-rename", "Rename")
                    .compact()
                    .disabled(count != 1)
                    .on_click(action("g-rename", "Rename"))
                    .into_any_element(),
                Button::secondary("g-permissions", "Permissions")
                    .compact()
                    .on_click(action("g-permissions", "Permissions"))
                    .into_any_element(),
                Button::secondary("g-copy-path", "Copy path")
                    .compact()
                    .on_click(action("g-copy-path", "Copy path"))
                    .into_any_element(),
                Button::new("g-delete", "Delete", ButtonKind::DestructiveOutline)
                    .compact()
                    .on_click(action("g-delete", "Delete"))
                    .into_any_element(),
                nav_button("g-clear".into(), "close", true, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |g, cx| {
                            g.file_picks = [false; 5];
                            cx.notify()
                        })
                    }
                })
                .into_any_element(),
            ];
            let select_all = div()
                .id("g-files-all")
                .cursor_pointer()
                .child(check_box(all))
                .on_click({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |g, cx| {
                            g.file_picks = [!all; 5];
                            cx.notify()
                        })
                    }
                })
                .into_any_element();
            let list = rows.iter().enumerate().map(|(i, (glyph, color, name, size, modified, mode))| {
                let entity = entity.clone();
                file_row(
                    ("g-file", i),
                    FileRow {
                        glyph,
                        color: *color,
                        name: SharedString::from(name.to_string()),
                        size: SharedString::from(size.to_string()),
                        modified: SharedString::from(modified.to_string()),
                        permissions: Some(SharedString::from(mode.to_string())),
                        selected: self.file_picks[i],
                        checkable: true,
                    },
                )
                .on_click(move |_, _, cx| {
                    entity.update(cx, |g, cx| {
                        g.file_picks[i] = !g.file_picks[i];
                        cx.notify()
                    })
                })
            });
            let pane = div()
                .flex()
                .flex_col()
                .bg(theme::ground())
                .border_1()
                .border_color(theme::line())
                .rounded(theme::RADIUS_MD)
                .overflow_hidden()
                .child(div().h(px(2.)).bg(theme::group_violet()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_1)
                        .px(px(10.))
                        .py(theme::SPACE_2)
                        .border_b_1()
                        .border_color(theme::line())
                        .child(nav_button("g-back".into(), "chevron-left", true, action("g-back", "Back")))
                        .child(nav_button("g-up".into(), "chevron-up", true, action("g-up", "Up")))
                        .child(nav_button("g-refresh".into(), "refresh", false, |_, _, _| {}))
                        .child(nav_button(
                            "g-new-folder".into(),
                            "folder-plus",
                            true,
                            action("g-new-folder", "New folder"),
                        ))
                        .child(breadcrumbs("g", crumbs, on_crumb)),
                )
                .when(count > 0, |d| d.child(selection_bar(count, actions)))
                .child(file_header(true, Some(select_all)))
                .children(list)
                .child(
                    div()
                        .m(theme::SPACE_2)
                        .p(px(14.))
                        .border_1()
                        .border_dashed()
                        .border_color(theme::accent())
                        .rounded(theme::RADIUS_MD)
                        .bg(theme::card())
                        .text_body_strong()
                        .text_color(theme::accent())
                        .flex()
                        .justify_center()
                        .child("Drop to upload into /srv/www/site/releases"),
                );
            let bar = |label: &'static str, percent: u8, color: gpui::Rgba| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(progress_bar(percent, color))
                    .child(div().text_meta().text_color(theme::ink_muted()).child(label))
            };
            let bars = div()
                .grid()
                .grid_cols(2)
                .gap(theme::SPACE_4)
                .p(theme::SPACE_4)
                .bg(theme::card())
                .rounded(theme::RADIUS_LG)
                .child(bar("Upload 62% · 48 MB/s · 12 s left", 62, theme::accent()))
                .child(bar("Download 35% · 21 MB/s · 1 min left", 35, theme::group_violet()))
                .child(bar("Paused at 40%", 40, theme::ink_faint()))
                .child(bar("Waiting to reconnect at 88%", 88, theme::ink_faint()));
            let on_bit: crate::ui::IndexHandler = {
                let entity = entity.clone();
                std::rc::Rc::new(move |i, _, cx| {
                    entity.update(cx, |g, cx| {
                        g.mode_bits[i] = !g.mode_bits[i];
                        cx.notify()
                    })
                })
            };
            div().flex().flex_col().gap(theme::SPACE_4).child(pane).child(bars).child(
                div()
                    .w(px(460.))
                    .p(theme::SPACE_5)
                    .bg(theme::card())
                    .rounded(theme::RADIUS_2XL)
                    .border_1()
                    .border_color(theme::line_strong())
                    .child(permissions_grid(self.mode_bits, on_bit)),
            )
        };

        // ------------------------------------------------------------ power --
        let power = {
            let toggle_rule = |i: usize| {
                let entity = entity.clone();
                move |v: bool, _: &mut Window, cx: &mut gpui::App| {
                    entity.update(cx, |g, cx| {
                        g.rules_on[i] = v;
                        g.note(format!("rule {i} switch: {v}"), cx);
                    })
                }
            };
            let cards = div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_3)
                .child(rule_card(
                    "g-rule-0",
                    "g-rule-switch-0",
                    RuleCard {
                        kind: holonet_model::ForwardKind::Local,
                        name: "Production Postgres".into(),
                        note: "Starts with db-primary".into(),
                        route: ["This PC 127.0.0.1:5432".into(), "db-primary".into(), "localhost:5432".into()],
                        status: if self.rules_on[0] { "Active · 2 connections open" } else { "Stopped" }.into(),
                        status_color: if self.rules_on[0] { theme::accent() } else { theme::ink_muted() },
                        detail: None,
                        on: self.rules_on[0],
                        selected: true,
                    },
                    toggle_rule(0),
                ))
                .child(rule_card(
                    "g-rule-1",
                    "g-rule-switch-1",
                    RuleCard {
                        kind: holonet_model::ForwardKind::Dynamic,
                        name: "Browse via bastion".into(),
                        note: "Manual start".into(),
                        route: [
                            "This PC 127.0.0.1:1080".into(),
                            "bastion-east".into(),
                            "Any destination (SOCKS5)".into(),
                        ],
                        status: if self.rules_on[1] {
                            "Reconnecting (attempt 2 of 5)…"
                        } else {
                            "Stopped: Port 1080 is already in use on this PC."
                        }
                        .into(),
                        status_color: theme::warn(),
                        detail: None,
                        on: self.rules_on[1],
                        selected: false,
                    },
                    toggle_rule(1),
                ));
            let swatches = holonet_model::GroupColor::ALL.iter().enumerate().map(|(i, c)| {
                swatch(("g-swatch", i), theme::group_color(*c), self.swatch == i).on_click({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |g, cx| {
                            g.swatch = i;
                            cx.notify()
                        })
                    }
                })
            });
            let chip_hosts = [
                ("prod-web-01", theme::accent()),
                ("prod-web-02", theme::accent()),
                ("staging-api", theme::group_violet()),
                ("nas-01", theme::group_mint()),
            ];
            let chips = chip_hosts.iter().enumerate().map(|(i, (name, color))| {
                host_chip(("g-chip", i), *name, *color, self.chips[i]).on_click({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |g, cx| {
                            g.chips[i] = !g.chips[i];
                            cx.notify()
                        })
                    }
                })
            });
            let label = |t: &'static str| div().text_meta().text_color(theme::ink_muted()).child(t);
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_4)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_3)
                        .child(kind_tag(holonet_model::ForwardKind::Local))
                        .child(kind_tag(holonet_model::ForwardKind::Remote))
                        .child(kind_tag(holonet_model::ForwardKind::Dynamic))
                        .child(label("Rule types; the switches below toggle")),
                )
                .child(cards)
                .child(div().flex().flex_wrap().gap(theme::SPACE_2).children(swatches))
                .child(div().flex().flex_wrap().gap(px(6.)).children(chips))
                .child(broadcast_strip("Everything you type goes to both panes, including 1 Production host.", {
                    let entity = entity.clone();
                    move |_, _, cx| entity.update(cx, |g, cx| g.note("broadcast: Turn off".into(), cx))
                }))
        };

        let ship = {
            let rows = [
                (
                    "prod-web-02",
                    Some(theme::group_violet()),
                    "Terminal",
                    "24 Nov 2026, 15:12",
                    "42 min",
                    "Primary",
                    "1.2 MB",
                ),
                ("nas-01", Some(theme::group_mint()), "SFTP", "24 Nov 2026, 09:03", "6 min", "Tailscale", "No log"),
                ("deploy@10.0.0.4", None, "Terminal", "Today, 13:40", "Open", "Primary", "Writing"),
            ];
            let table = div()
                .bg(theme::card())
                .border_1()
                .border_color(theme::line())
                .rounded(theme::RADIUS_LG)
                .child(history_header())
                .children(rows.into_iter().enumerate().map(|(i, (host, color, kind, started, length, route, log))| {
                    history_row(
                        SharedString::from(format!("g-hist-{i}")),
                        HistoryRow {
                            selected: self.hist_row == i,
                            color,
                            host: host.into(),
                            kind,
                            started: started.into(),
                            length: length.into(),
                            route: route.into(),
                            log: log.into(),
                        },
                    )
                    .on_click(cx.listener(move |g, _, _, cx| {
                        g.hist_row = i;
                        g.note(format!("history row {i}"), cx);
                    }))
                }));
            let output = log_output(
                "g-hist-output",
                vec![
                    ("Showing the last 400 of 1,204 lines. Export log for all of them.".into(), true),
                    ("deploy@prod-web-02:~$ systemctl status nginx".into(), false),
                    ("● nginx.service - A high performance web server and a reverse proxy server".into(), false),
                    ("     Active: active (running) since Mon 2026-11-24 15:12:40 UTC; 42min ago".into(), false),
                    (crate::history::FULL_SCREEN_NOTE.into(), true),
                ],
            )
            .h(px(150.))
            .min_h(px(150.));
            let entries = [
                ("HOSTS", Some(theme::group_violet()), "prod-web-02", "Go to session"),
                ("HOSTS", Some(theme::group_mint()), "nas-01", "Connect"),
                ("SNIPPETS", None, "Disk usage", "Run in active session"),
                ("ACTIONS", None, "New host", "Action · Ctrl Shift N"),
            ];
            let mut palette = div()
                .max_w(px(640.))
                .flex()
                .flex_col()
                .bg(theme::card())
                .border_1()
                .border_color(theme::line_strong())
                .rounded(px(14.))
                .overflow_hidden()
                .pb(theme::SPACE_2);
            let mut last = "";
            for (i, (section, color, title, hint)) in entries.into_iter().enumerate() {
                if section != last {
                    palette = palette.child(palette_section(section));
                    last = section;
                }
                palette = palette.child(
                    palette_row(
                        SharedString::from(format!("g-pal-{i}")),
                        self.palette_at == i,
                        color,
                        title.into(),
                        hint.into(),
                    )
                    .on_click(cx.listener(move |g, _, _, cx| {
                        g.palette_at = i;
                        g.note(format!("palette row {i}"), cx);
                    })),
                );
            }
            let update = update_card(
                "0.2.0",
                crate::updates::CURRENT,
                "2 Dec 2026",
                Button::primary("g-update-view", "View release on GitHub")
                    .on_click(cx.listener(|g, _, _, cx| g.note("update: View release on GitHub".into(), cx))),
                Button::secondary("g-update-later", "Later")
                    .on_click(cx.listener(|g, _, _, cx| g.note("update: Later".into(), cx))),
            );
            let first_run = div()
                .grid()
                .grid_cols(3)
                .gap(px(12.))
                .child(
                    import_card(
                        "g-first-0",
                        "Windows OpenSSH",
                        "~/.ssh/config",
                        "Hosts, users, ports, keys and jump hosts.",
                    )
                    .on_click(cx.listener(|g, _, _, cx| g.note("first run: import card".into(), cx))),
                )
                .child(
                    first_run_link(
                        "g-first-1",
                        "Bring your known_hosts",
                        "Keep the server keys you have already trusted.",
                    )
                    .on_click(cx.listener(|g, _, _, cx| g.note("first run: link".into(), cx))),
                )
                .child(div().flex().items_center().gap(theme::SPACE_2).child(key_chip("Enter")).child(key_chip("Esc")));
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_4)
                .child(table)
                .child(output)
                .child(palette)
                .child(update)
                .child(first_run)
        };

        let log = div()
            .text_mono_small()
            .text_color(theme::ink_dim())
            .children(self.log.iter().map(|l| div().child(SharedString::from(l.clone()))));

        let content = div().id("gallery-content").flex_1().h_full().overflow_y_scroll().bg(theme::ground()).child(
            div()
                .px(theme::SPACE_6)
                .py(px(28.))
                .flex()
                .flex_col()
                .gap(theme::SPACE_5)
                .max_w(px(980.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_1)
                        .child(div().text_title().text_color(theme::ink()).child("Controls"))
                        .child(
                            div()
                                .text_body()
                                .text_color(theme::ink_muted())
                                .child("Every shared control in every state, built from docs/design tokens."),
                        ),
                )
                .child(Self::section("Type", typography))
                .child(Self::section("Buttons", buttons))
                .child(Self::section("Fields", fields))
                .child(Self::section("Choices", choices))
                .child(Self::section("Tags", tags))
                .child(Self::section("Display", display))
                .child(Self::section("Dialogs", dialogs))
                .child(Self::section("Keychain", keychain))
                .child(Self::section("Files", files))
                .child(Self::section("Power features", power))
                .child(Self::section("History, palette, updates and first run", ship))
                .child(Self::section("Events", log)),
        );

        let dialog_layer = match self.dialog {
            OpenDialog::None => None,
            OpenDialog::Standard => Some(modal(
                "dialog-standard",
                px(460.),
                dialog(false, "Change vault password", "You will use the new password the next time you unlock.")
                    .child(dialog_buttons([
                        Button::quiet("d-cancel", "Cancel").on_click(open(OpenDialog::None)),
                        Button::primary("d-ok", "Change password").on_click(open(OpenDialog::None)),
                    ])),
                {
                    let entity = entity.clone();
                    move |_, cx| {
                        entity.update(cx, |g, cx| {
                            g.dialog = OpenDialog::None;
                            cx.notify()
                        })
                    }
                },
            )),
            OpenDialog::Warning => Some(modal(
                "dialog-warning",
                px(460.),
                dialog(true, "Delete nas-01?", "The host is removed from the vault. This cannot be undone.")
                    .child(
                        Checkbox::new("forget", "Also forget its server key in Known hosts", self.forget_key)
                            .on_toggle({
                                let entity = entity.clone();
                                move |v, _, cx| {
                                    entity.update(cx, |g, cx| {
                                        g.forget_key = v;
                                        cx.notify()
                                    })
                                }
                            }),
                    )
                    .child(dialog_buttons([
                        Button::secondary("w-cancel", "Cancel").on_click(open(OpenDialog::None)),
                        Button::new("w-delete", "Delete host", ButtonKind::DestructiveFill)
                            .on_click(open(OpenDialog::None)),
                    ])),
                {
                    let entity = entity.clone();
                    move |_, cx| {
                        entity.update(cx, |g, cx| {
                            g.dialog = OpenDialog::None;
                            cx.notify()
                        })
                    }
                },
            )),
        };

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .text_color(theme::ink())
            .font_family(theme::SANS)
            .child(
                TitleBar::new(self.active_tab.is_none(), tabs)
                    .on_vault({
                        let entity = entity.clone();
                        move |_, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.active_tab = None;
                                cx.notify()
                            })
                        }
                    })
                    .on_tab({
                        let entity = entity.clone();
                        move |i, _, cx| {
                            entity.update(cx, |g, cx| {
                                g.active_tab = Some(i);
                                cx.notify()
                            })
                        }
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // Dialogs cover the body, not the title bar, so the window
                    // buttons keep working while one is open.
                    .relative()
                    .flex()
                    .child(
                        SideNav::new(
                            Some(self.nav),
                            vec![NavEntry::Hosts, NavEntry::KnownHosts, NavEntry::Terminal, NavEntry::Settings],
                        )
                        .update_available(true)
                        .on_select({
                            let entity = entity.clone();
                            move |e, _, cx| {
                                entity.update(cx, |g, cx| {
                                    g.nav = e;
                                    cx.notify()
                                })
                            }
                        })
                        .on_lock({
                            let entity = entity.clone();
                            move |_, _, cx| entity.update(cx, |g, cx| g.note("Lock now clicked".into(), cx))
                        }),
                    )
                    .child(content)
                    .children(dialog_layer),
            )
    }
}
