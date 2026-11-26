//! The Port forwarding page (PortForwarding.dc.html): rule cards with their
//! on/off switches, the three kinds explained, and the rule form, plus the
//! password question a background connection may ask.

use gpui::{AnyElement, Context, Entity, IntoElement, Rgba, SharedString, Subscription, Window, div, prelude::*, px};
use holonet_model::{ForwardKind, ForwardRule, ForwardRuleId, HostId, LOOPBACK, VaultData};
use holonet_ssh::SecretRequest;

use super::{Page, Workspace};
use crate::forwards::{Forwarder, RuleState, host_name};
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, ButtonKind, Checkbox, Segmented, Select, Tag, TextField, TextFieldEvent, code_box, dialog, dialog_buttons,
    field, field_help, modal, section_label, switch,
};

const FORM_W: f32 = 380.;

/// A kind's color (PortForwarding.dc.html: Local cyan, Remote violet,
/// Dynamic mint).
fn kind_color(kind: ForwardKind) -> Rgba {
    match kind {
        ForwardKind::Local => theme::accent(),
        ForwardKind::Remote => theme::group_violet(),
        ForwardKind::Dynamic => theme::group_mint(),
    }
}

/// "This PC 127.0.0.1:5432", the host, then where connections go.
fn route(rule: &ForwardRule, host: &str) -> [String; 3] {
    let bind = if rule.bind_address.trim().is_empty() { LOOPBACK } else { rule.bind_address.trim() };
    match rule.kind {
        ForwardKind::Local => [format!("This PC {bind}:{}", rule.port), host.to_string(), rule.destination()],
        ForwardKind::Remote => {
            [format!("{host} {bind}:{}", rule.port), host.to_string(), format!("This PC {}", rule.destination())]
        }
        ForwardKind::Dynamic => {
            [format!("This PC {bind}:{}", rule.port), host.to_string(), "Any destination (SOCKS5)".to_string()]
        }
    }
}

/// The form's state, and the answer field for a background question.
pub(crate) struct ForwardingUi {
    editing: Option<ForwardRuleId>,
    kind: ForwardKind,
    name: Entity<TextField>,
    /// The vault's hosts, by name.
    host: Entity<Select>,
    host_ids: Vec<HostId>,
    bind: Entity<TextField>,
    port: Entity<TextField>,
    destination: Entity<TextField>,
    destination_port: Entity<TextField>,
    start_with_host: bool,
    show_problems: bool,
    /// The delete prompt is open.
    delete: bool,
    secret: Entity<TextField>,
    /// The question the answer field was last cleared for.
    asked: Option<u64>,
}

impl ForwardingUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let text = |cx: &mut Context<Workspace>, placeholder: &str, mono: bool| {
            cx.new(|cx| {
                let f = TextField::new(cx, placeholder.to_string());
                if mono { f.mono() } else { f }
            })
        };
        Self {
            editing: None,
            kind: ForwardKind::Local,
            name: text(cx, "e.g. Production Postgres", false),
            host: cx.new(|cx| Select::new(cx, vec!["No hosts yet".into()], 0)),
            host_ids: Vec::new(),
            bind: text(cx, LOOPBACK, true),
            port: text(cx, "8080", true),
            destination: text(cx, "localhost", true),
            destination_port: text(cx, "80", true),
            start_with_host: false,
            show_problems: false,
            delete: false,
            secret: cx.new(|cx| TextField::new(cx, "").password()),
            asked: None,
        }
    }

    pub fn subscribe(&self, forwarder: &Entity<Forwarder>, cx: &mut Context<Workspace>) -> Vec<Subscription> {
        let mut subs = vec![
            cx.observe(forwarder, |_, _, cx| cx.notify()),
            cx.observe(&self.host, |_, _, cx| cx.notify()),
            cx.subscribe(&self.secret, |this: &mut Workspace, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submitted => this.answer_ask(true, cx),
                TextFieldEvent::Cancelled => this.answer_ask(false, cx),
                TextFieldEvent::Changed => {}
            }),
        ];
        for f in [&self.name, &self.bind, &self.port, &self.destination, &self.destination_port] {
            subs.push(cx.subscribe(f, |this: &mut Workspace, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submitted => this.save_rule(cx),
                _ => cx.notify(),
            }));
        }
        subs
    }
}

impl Workspace {
    /// Fill the form from a rule, or for a new one.
    fn load_rule(&mut self, data: &VaultData, id: Option<ForwardRuleId>, cx: &mut Context<Self>) {
        let mut hosts: Vec<_> = data.hosts.iter().collect();
        hosts.sort_by_key(|h| h.display_name().to_lowercase());
        let rule = id.and_then(|id| data.forward_rule(id)).cloned();
        let ui = &mut self.fw;
        ui.editing = rule.as_ref().map(|r| r.id);
        ui.show_problems = false;
        ui.delete = false;
        ui.host_ids = hosts.iter().map(|h| h.id).collect();
        let options: Vec<SharedString> = if hosts.is_empty() {
            vec!["No hosts yet".into()]
        } else {
            hosts.iter().map(|h| SharedString::from(h.display_name().to_string())).collect()
        };
        let host_index = rule.as_ref().and_then(|r| ui.host_ids.iter().position(|h| *h == r.host)).unwrap_or(0);
        ui.host.update(cx, |s, cx| s.set_options(options, host_index, cx));
        let r = rule.unwrap_or_else(|| ForwardRule::new(ForwardKind::Local, HostId::new()));
        ui.kind = r.kind;
        ui.start_with_host = r.start_with_host;
        let port = |p: u16| if p == 0 { String::new() } else { p.to_string() };
        ui.name.update(cx, |f, cx| f.set_text(r.name.clone(), cx));
        ui.bind.update(cx, |f, cx| f.set_text(r.bind_address.clone(), cx));
        ui.port.update(cx, |f, cx| f.set_text(port(r.port), cx));
        ui.destination.update(cx, |f, cx| f.set_text(r.destination_host.clone(), cx));
        ui.destination_port.update(cx, |f, cx| f.set_text(port(r.destination_port), cx));
    }

    pub(crate) fn open_forwarding(&mut self, cx: &mut Context<Self>) {
        if let Some(data) = self.vault_data().cloned() {
            self.load_rule(&data, None, cx);
        }
        self.page = Page::Forwarding;
        cx.notify();
    }

    /// The rule the form describes; `None` when there is no host to pick.
    fn edited_rule(&self, cx: &Context<Self>) -> Option<ForwardRule> {
        let ui = &self.fw;
        let host = *ui.host_ids.get(ui.host.read(cx).selected())?;
        let text = |f: &Entity<TextField>| f.read(cx).text().trim().to_string();
        let port = |f: &Entity<TextField>| text(f).parse::<u16>().unwrap_or(0);
        let mut rule = ui
            .editing
            .and_then(|id| self.vault_data().and_then(|d| d.forward_rule(id)).cloned())
            .unwrap_or_else(|| ForwardRule::new(ui.kind, host));
        rule.kind = ui.kind;
        rule.host = host;
        rule.name = text(&ui.name);
        let bind = text(&ui.bind);
        rule.bind_address = if bind.is_empty() { LOOPBACK.to_string() } else { bind };
        rule.port = port(&ui.port);
        rule.destination_host = text(&ui.destination);
        rule.destination_port = port(&ui.destination_port);
        rule.start_with_host = ui.start_with_host;
        Some(rule)
    }

    fn save_rule(&mut self, cx: &mut Context<Self>) {
        self.fw.show_problems = true;
        let Some(rule) = self.edited_rule(cx) else {
            cx.notify();
            return;
        };
        if !rule.problems().is_empty() {
            cx.notify();
            return;
        }
        let id = rule.id;
        self.save(
            move |d| match d.forward_rule_mut(id) {
                Some(existing) => *existing = rule,
                None => d.forward_rules.push(rule),
            },
            cx,
        );
        self.fw.editing = Some(id);
        self.fw.show_problems = false;
        // A running rule starts again with what was saved.
        if let Some(data) = self.vault_data().cloned() {
            self.forwarder.update(cx, |f, cx| f.restart(id, &data, cx));
        }
        cx.notify();
    }

    fn delete_rule(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.fw.editing else { return };
        self.forwarder.update(cx, |f, cx| f.stop(id, cx));
        self.save(move |d| d.forward_rules.retain(|r| r.id != id), cx);
        self.open_forwarding(cx);
    }

    fn toggle_rule(&mut self, id: ForwardRuleId, on: bool, cx: &mut Context<Self>) {
        let Some(data) = self.vault_data().cloned() else { return };
        self.forwarder.update(cx, |f, cx| if on { f.start(id, &data, cx) } else { f.stop(id, cx) });
    }

    fn answer_ask(&mut self, ok: bool, cx: &mut Context<Self>) {
        let Some(id) = self.forwarder.read(cx).ask().map(|a| a.id) else { return };
        let secret = ok
            .then(|| self.fw.secret.read(cx).text().to_string())
            .filter(|s| !s.is_empty())
            .map(holonet_model::Secret::new);
        self.fw.secret.update(cx, |f, cx| f.set_text("", cx));
        self.forwarder.update(cx, |f, cx| f.answer(id, secret, cx));
    }

    pub(super) fn forwarding_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault_data().cloned() else { return div().into_any_element() };
        // The host list follows the vault (a host added since the page opened).
        if self.fw.host_ids.len() != data.hosts.len() {
            let editing = self.fw.editing;
            self.load_rule(&data, editing, cx);
        }
        let mut rules: Vec<&ForwardRule> = data.forward_rules.iter().collect();
        rules.sort_by_key(|r| r.display_name().to_lowercase());

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
                    .child(div().text_title().text_color(theme::ink()).child("Port forwarding"))
                    .child(div().text_body().text_color(theme::ink_muted()).child(
                        "Rules open tunnels through a host. A rule starts its host connection in the background if one is not already open.",
                    )),
            )
            .child(Button::primary("new-rule", "New rule").header().on_click(cx.listener(|this, _, window, cx| {
                this.open_forwarding(cx);
                this.fw.name.read(cx).focus(window);
            })));

        let list: AnyElement = if rules.is_empty() {
            div()
                .p(theme::SPACE_5)
                .rounded(theme::RADIUS_LG)
                .border_1()
                .border_dashed()
                .border_color(theme::line_strong())
                .flex()
                .flex_col()
                .gap(theme::SPACE_2)
                .child(div().text_heading().text_color(theme::ink()).child("No rules yet"))
                .child(div().text_body().text_color(theme::ink_muted()).child(
                    "Describe the first tunnel in the form: its type, the host it runs through, and the ports. Then switch it on here.",
                ))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(theme::SPACE_3)
                .children(rules.iter().map(|r| self.rule_card(&data, r, cx)))
                .into_any_element()
        };

        let kinds = div().grid().grid_cols(3).gap(theme::SPACE_3).children(ForwardKind::ALL.map(|kind| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(theme::SPACE_4)
                .rounded(theme::RADIUS_LG)
                .bg(theme::card())
                .border_1()
                .border_color(theme::line())
                .child(div().flex().child(Tag::Group(kind.label().into(), kind_color(kind))))
                .child(div().text_meta().text_color(theme::ink_muted()).child(kind.description()))
        }));

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("forwarding-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(20.))
                        .child(header)
                        .child(list)
                        .child(kinds),
                ),
            )
            .child(self.rule_form(&data, cx))
            .children(self.rule_delete_dialog(&data, cx))
            .children(self.ask_dialog(window, cx))
            .into_any_element()
    }

    fn rule_card(&self, data: &VaultData, rule: &ForwardRule, cx: &mut Context<Self>) -> AnyElement {
        let id = rule.id;
        let host = host_name(data, rule.host);
        let state = self.forwarder.read(cx).state(id).cloned();
        let on = state.as_ref().is_some_and(RuleState::is_on);
        let selected = self.fw.editing == Some(id);
        let (status, status_color): (String, Rgba) = match &state {
            None => ("Stopped".into(), theme::ink_muted()),
            Some(RuleState::Starting) => ("Starting…".into(), theme::ink_muted()),
            Some(RuleState::NeedsSecret(name)) => (format!("Waiting for the password for {name}"), theme::warn()),
            Some(RuleState::Active { open: 0, .. }) => ("Active".into(), theme::accent()),
            Some(RuleState::Active { open, .. }) => {
                (format!("Active · {open} connection{} open", if *open == 1 { "" } else { "s" }), theme::accent())
            }
            Some(RuleState::Reconnecting { attempt }) => {
                (format!("Reconnecting (attempt {attempt} of 5)…"), theme::warn())
            }
            Some(RuleState::Failed(why)) => (format!("Stopped: {why}"), theme::warn()),
        };
        let last_error = match &state {
            Some(RuleState::Active { last_error: Some(e), .. }) => Some(format!("Last failed connection: {e}")),
            _ => None,
        };
        let note = if rule.start_with_host { format!("Starts with {host}") } else { "Manual start".to_string() };
        let [a, b, c] = route(rule, &host);
        let arrow = || div().text_color(theme::ink_faint()).child("→");
        div()
            .id(SharedString::from(format!("rule-{}", rule.id.0)))
            .flex()
            .flex_col()
            .gap(theme::SPACE_3)
            .p(theme::SPACE_4)
            .rounded(theme::RADIUS_XL)
            .border_1()
            .cursor_pointer()
            .bg(if selected { theme::card_selected() } else { theme::card() })
            .border_color(if selected {
                theme::accent()
            } else if on {
                theme::line_strong()
            } else {
                theme::line()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .child(Tag::Group(rule.kind.label().into(), kind_color(rule.kind)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_body_strong()
                                    .text_color(theme::ink())
                                    .truncate()
                                    .child(SharedString::from(rule.display_name())),
                            )
                            .child(div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(note))),
                    )
                    .child(switch(SharedString::from(format!("rule-switch-{}", rule.id.0)), on, {
                        let this = cx.weak_entity();
                        move |v, _, cx| {
                            let _ = this.update(cx, |this, cx| this.toggle_rule(id, v, cx));
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .text_mono_small()
                    .text_color(theme::ink_soft())
                    .child(SharedString::from(a))
                    .child(arrow())
                    .child(SharedString::from(b))
                    .child(arrow())
                    .child(SharedString::from(c)),
            )
            .child(div().text_meta().text_color(status_color).child(SharedString::from(status)))
            .children(last_error.map(|e| div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(e))))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(data) = this.vault_data().cloned() {
                    this.load_rule(&data, Some(id), cx);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn rule_form(&self, data: &VaultData, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.fw;
        let rule = self.edited_rule(cx);
        let problems: Vec<&'static str> = match &rule {
            Some(r) if ui.show_problems => r.problems().iter().map(|p| p.message()).collect(),
            None if ui.show_problems => vec!["Add a host first: a rule runs through a saved host."],
            _ => Vec::new(),
        };
        let kind_index = ForwardKind::ALL.iter().position(|k| *k == ui.kind).unwrap_or(0);
        let kinds =
            Segmented::new("rule-kind", ForwardKind::ALL.map(|k| SharedString::from(k.label())).to_vec(), kind_index)
                .on_change({
                    let this = cx.weak_entity();
                    move |i, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.fw.kind = ForwardKind::ALL[i.min(2)];
                            cx.notify()
                        });
                    }
                });
        let (port_label, destination_label) = match ui.kind {
            ForwardKind::Local => ("Local port", "Destination from host"),
            ForwardKind::Remote => ("Remote port", "Destination on this PC"),
            ForwardKind::Dynamic => ("SOCKS port", ""),
        };
        let bind_help = match &rule {
            Some(r) if !r.is_loopback() && r.kind == ForwardKind::Remote => Some((
                SharedString::from("Other devices reach this only if the host's SSH server allows it (GatewayPorts)."),
                true,
            )),
            Some(r) if !r.is_loopback() => {
                Some((SharedString::from("Reachable from other devices on your network."), true))
            }
            _ => None,
        };
        let command = rule.as_ref().map(|r| r.ssh_command(&host_name(data, r.host)));
        let side = |a: AnyElement, b: AnyElement| {
            div()
                .flex()
                .gap(theme::SPACE_3)
                .child(div().flex_1().min_w_0().child(a))
                .child(div().w(px(110.)).flex_none().child(b))
        };
        div()
            .id("rule-form")
            .overflow_y_scroll()
            .w(px(FORM_W))
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
                "Edit rule"
            } else {
                "New rule"
            }))
            .child(field("Type", kinds, Some((ui.kind.description().into(), false))))
            .child(field("Label", ui.name.clone(), None))
            .child(field("Through host", ui.host.clone(), None))
            .child(side(
                field("Bind address", ui.bind.clone(), bind_help).into_any_element(),
                field(port_label, ui.port.clone(), None).into_any_element(),
            ))
            .when(ui.kind != ForwardKind::Dynamic, |d| {
                d.child(side(
                    field(destination_label, ui.destination.clone(), None).into_any_element(),
                    field("Port", ui.destination_port.clone(), None).into_any_element(),
                ))
            })
            .children(command.map(|c| {
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(section_label("Equivalent command"))
                    .child(code_box(c))
            }))
            .child(
                Checkbox::new("rule-start-with-host", "Start when this host connects", ui.start_with_host).on_toggle({
                    let this = cx.weak_entity();
                    move |v, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.fw.start_with_host = v;
                            cx.notify()
                        });
                    }
                }),
            )
            .children(problems.into_iter().map(|p| field_help(p, true)))
            .child(div().flex_1())
            .child(
                Button::primary("save-rule", "Save rule")
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| this.save_rule(cx))),
            )
            .when(ui.editing.is_some(), |d| {
                d.child(
                    Button::new("delete-rule", "Delete rule", ButtonKind::DestructiveOutline).full_width().on_click(
                        cx.listener(|this, _, _, cx| {
                            this.fw.delete = true;
                            cx.notify()
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    fn rule_delete_dialog(&self, data: &VaultData, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.fw.editing.filter(|_| self.fw.delete)?;
        let rule = data.forward_rule(id)?;
        let running = self.forwarder.read(cx).is_on(id);
        let close = {
            let this = cx.weak_entity();
            move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |this, cx| {
                    this.fw.delete = false;
                    cx.notify()
                });
            }
        };
        Some(
            modal(
                "delete-rule",
                px(460.),
                dialog(
                    true,
                    format!("Delete {}?", rule.display_name()),
                    if running {
                        "The rule is removed and its forward stops. This cannot be undone."
                    } else {
                        "The rule is removed. This cannot be undone."
                    },
                )
                .child(dialog_buttons([
                    Button::secondary("delete-rule-cancel", "Cancel").on_click({
                        let close = close.clone();
                        move |_, window, cx| close(window, cx)
                    }),
                    Button::new("delete-rule-confirm", "Delete rule", ButtonKind::DestructiveFill)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_rule(cx))),
                ])),
                close,
            )
            .into_any_element(),
        )
    }

    /// A background connection's question: a password, passphrase or code.
    fn ask_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, title, body) = {
            let ask = self.forwarder.read(cx).ask()?;
            let (title, body) = match &ask.request {
                SecretRequest::Password { user, host } => (
                    format!("Enter the password for {user}@{host}"),
                    format!(
                        "A port forwarding rule is connecting to {}. The password is kept until Holonet closes, not saved.",
                        ask.host_name
                    ),
                ),
                SecretRequest::Passphrase { key_name } => (
                    format!("Enter the passphrase for {key_name}"),
                    format!("A port forwarding rule is connecting to {} with this key.", ask.host_name),
                ),
                SecretRequest::Challenge { title, prompt, .. } => (
                    if title.is_empty() { format!("{} asks a question", ask.host_name) } else { title.clone() },
                    prompt.clone(),
                ),
            };
            (ask.id, title, body)
        };
        if self.fw.asked != Some(id) {
            self.fw.asked = Some(id);
            self.fw.secret.update(cx, |f, cx| f.set_text("", cx));
            self.fw.secret.read(cx).focus(window);
        }
        let cancel = {
            let this = cx.weak_entity();
            move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |this, cx| this.answer_ask(false, cx));
            }
        };
        Some(
            modal(
                "forward-ask",
                px(460.),
                dialog(false, title, body).child(self.fw.secret.clone()).child(dialog_buttons([
                    Button::quiet("forward-ask-cancel", "Cancel").on_click({
                        let cancel = cancel.clone();
                        move |_, window, cx| cancel(window, cx)
                    }),
                    Button::primary("forward-ask-ok", "Connect")
                        .on_click(cx.listener(|this, _, _, cx| this.answer_ask(true, cx))),
                ])),
                cancel,
            )
            .into_any_element(),
        )
    }
}
