//! The popup a snippet opens before it runs (not drawn; milestone 6 plan):
//! one field per `${name}` variable, the command as it will be typed, and
//! the "Ask before running" and warn-group questions. Used over a pane
//! (the drawer and startup snippets) and over the Snippets page (Run on).

use std::collections::HashMap;

use gpui::{
    Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use holonet_model::Snippet;

use crate::theme::{self, TypeStyles};
use crate::ui::{Button, ButtonKind, TextField, TextFieldEvent, code_box, dialog, dialog_buttons, field, modal};

pub enum RunPromptEvent {
    /// Run with these values.
    Run(HashMap<String, String>),
    Cancel,
}

pub struct RunPrompt {
    snippet: Snippet,
    /// "wsl-a", "the active pane" or "3 hosts".
    target: String,
    /// "2 Production hosts": the run reaches hosts in groups marked "warn".
    warn: Option<String>,
    fields: Vec<(String, Entity<TextField>)>,
    problem: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RunPromptEvent> for RunPrompt {}

impl RunPrompt {
    /// Whether running `snippet` asks anything first.
    pub fn needed(snippet: &Snippet, warn: bool) -> bool {
        warn || snippet.confirm || !snippet.variables().is_empty()
    }

    /// `last` holds this snippet's values from its previous run, if any.
    pub fn new(
        snippet: Snippet,
        target: impl Into<String>,
        warn: Option<String>,
        last: &HashMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let fields: Vec<(String, Entity<TextField>)> = snippet
            .variables()
            .into_iter()
            .map(|name| {
                let value = last.get(&name).cloned().unwrap_or_default();
                let f = cx.new(|cx| TextField::new(cx, name.clone()).mono().with_text(value));
                subscriptions.push(cx.subscribe(&f, |this: &mut Self, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Submitted => this.run(cx),
                    TextFieldEvent::Cancelled => cx.emit(RunPromptEvent::Cancel),
                    TextFieldEvent::Changed => {
                        this.problem = None;
                        cx.notify()
                    }
                }));
                (name, f)
            })
            .collect();
        // A remembered value is selected, so typing replaces it.
        for (_, f) in &fields {
            f.update(cx, |f, cx| f.select_all_text(cx));
        }
        if let Some((_, f)) = fields.first() {
            f.read(cx).focus(window);
        }
        Self { snippet, target: target.into(), warn, fields, problem: None, _subscriptions: subscriptions }
    }

    fn values(&self, cx: &Context<Self>) -> HashMap<String, String> {
        self.fields.iter().map(|(name, f)| (name.clone(), f.read(cx).text().to_string())).collect()
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if let Some((name, _)) = self.fields.iter().find(|(_, f)| f.read(cx).text().trim().is_empty()) {
            self.problem = Some(format!("Enter a value for {name}.").into());
            cx.notify();
            return;
        }
        let values = self.values(cx);
        cx.emit(RunPromptEvent::Run(values));
    }
}

impl Render for RunPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.snippet.display_name().to_string();
        let warn = self.warn.is_some();
        let title = if warn || self.snippet.confirm {
            format!("Run {name} on {}?", self.target)
        } else {
            format!("Run {name} on {}", self.target)
        };
        let body = match &self.warn {
            Some(w) => format!("This runs on {w}. Check the command before running it."),
            None if self.fields.is_empty() => format!("This is typed into {}.", self.target),
            None => "Holonet asks for these each time this snippet runs.".to_string(),
        };
        // The command as it will be typed, values filled in.
        let preview = self.snippet.fill(&self.values(cx));
        let run_label = if warn { "Run anyway" } else { "Run" };
        let entity = cx.entity();
        let cancel =
            move |_: &mut Window, cx: &mut gpui::App| entity.update(cx, |_, cx| cx.emit(RunPromptEvent::Cancel));
        modal(
            "snippet-run",
            px(460.),
            dialog(warn, title, body)
                .children(self.fields.iter().map(|(name, f)| field(name.clone(), f.clone(), None).flex_none()))
                .child(code_box(preview))
                .children(self.problem.clone().map(|p| div().text_meta().text_color(theme::warn()).child(p)))
                .child(dialog_buttons([
                    Button::quiet("snippet-run-cancel", "Cancel").on_click({
                        let cancel = cancel.clone();
                        move |_, window, cx| cancel(window, cx)
                    }),
                    if warn {
                        Button::new("snippet-run-ok", run_label, ButtonKind::DestructiveFill)
                    } else {
                        Button::primary("snippet-run-ok", run_label)
                    }
                    .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                ])),
            cancel,
        )
    }
}

/// "2 Production hosts", or "3 hosts in Production and Staging": the
/// hosts a run reaches in groups marked "warn". `None` when there are none.
pub fn warn_text(data: &holonet_model::VaultData, hosts: &[holonet_model::HostId]) -> Option<String> {
    let mut groups: Vec<String> = Vec::new();
    let mut count = 0;
    for host in hosts.iter().filter_map(|h| data.host(*h)) {
        if let Some(g) = data.host_group(host).filter(|g| g.warn) {
            count += 1;
            let name = g.display_name().to_string();
            if !groups.contains(&name) {
                groups.push(name);
            }
        }
    }
    let plural = if count == 1 { "" } else { "s" };
    match groups.as_slice() {
        [] => None,
        [one] => Some(format!("{count} {one} host{plural}")),
        [rest @ .., last] => Some(format!("{count} host{plural} in {} and {last}", rest.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use holonet_model::{Group, Host, VaultData};

    use super::warn_text;

    #[test]
    fn warn_text_counts_hosts_in_warn_groups() {
        let mut data = VaultData::default();
        let mut prod = Group::new("Production");
        prod.warn = true;
        let mut staging = Group::new("Staging");
        staging.warn = true;
        let quiet = Group::new("Lab");
        let mut hosts = Vec::new();
        for (name, group) in [("a", prod.id), ("b", prod.id), ("c", quiet.id), ("d", staging.id)] {
            let mut h = Host::new(name);
            h.group = Some(group);
            hosts.push(h.id);
            data.hosts.push(h);
        }
        data.groups = vec![prod, staging, quiet];
        assert_eq!(warn_text(&data, &hosts[2..3]), None);
        assert_eq!(warn_text(&data, &hosts[..1]).as_deref(), Some("1 Production host"));
        assert_eq!(warn_text(&data, &hosts[..3]).as_deref(), Some("2 Production hosts"));
        assert_eq!(warn_text(&data, &hosts).as_deref(), Some("3 hosts in Production and Staging"));
    }
}
