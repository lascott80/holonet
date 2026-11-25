//! `holonet --gallery`: every shared control in every state, inside the app
//! shell, for checking against docs/design (milestone 2, phase 3).

use gpui::{AnyElement, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};

use crate::shell::{NavEntry, SideNav, TabInfo, TabKind, TabState, TitleBar};
use crate::theme::{self, TypeStyles};
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
    route: usize,
    auth: usize,
    keep_alive: bool,
    forward_agent: bool,
    forget_key: bool,
    dialog: OpenDialog,
    nav: NavEntry,
    active_tab: Option<usize>,
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

        let subscriptions = vec![
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
            route: 0,
            auth: 0,
            keep_alive: true,
            forward_agent: false,
            forget_key: false,
            dialog: OpenDialog::None,
            nav: NavEntry::Hosts,
            active_tab: None,
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
