//! The command palette (Palette.dc.html), Ctrl Shift P anywhere: hosts,
//! snippets, port forwarding rules, screens and actions, found by typing.
//!
//! The app builds the entries when the palette opens (from the vault, the
//! open tabs and the running rules) and runs the chosen one; the palette
//! only filters, ranks and picks. Entries that cannot run now (a snippet
//! with no terminal open, Split right with no terminal tab) are left out.

use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, IntoElement, KeyBinding, Render, Rgba, SharedString, Subscription,
    Window, actions, div, prelude::*, px,
};
use holonet_model::{ForwardRuleId, HostId, Snippet};

use crate::theme::{self, TypeStyles};
use crate::ui::{TextField, TextFieldEvent, icon};

actions!(command_palette, [Up, Down, OpenSplit]);

const CONTEXT: &str = "CommandPalette";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("ctrl-enter", OpenSplit, Some(CONTEXT)),
    ]);
}

/// The drawn sections, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Hosts,
    Snippets,
    Forwarding,
    GoTo,
    Actions,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Hosts => "HOSTS",
            Section::Snippets => "SNIPPETS",
            Section::Forwarding => "PORT FORWARDING",
            Section::GoTo => "GO TO",
            Section::Actions => "ACTIONS",
        }
    }

    /// Entries shown with nothing typed; typing shows every match.
    fn idle_limit(self) -> usize {
        match self {
            Section::Hosts => 6,
            Section::Snippets | Section::Forwarding => 4,
            Section::GoTo | Section::Actions => usize::MAX,
        }
    }
}

/// A screen in the side navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Hosts,
    Groups,
    Keychain,
    Forwarding,
    Snippets,
    KnownHosts,
    History,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 8] = [
        Screen::Hosts,
        Screen::Groups,
        Screen::Keychain,
        Screen::Forwarding,
        Screen::Snippets,
        Screen::KnownHosts,
        Screen::History,
        Screen::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Screen::Hosts => "Hosts",
            Screen::Groups => "Groups",
            Screen::Keychain => "Keychain",
            Screen::Forwarding => "Port forwarding",
            Screen::Snippets => "Snippets",
            Screen::KnownHosts => "Known hosts",
            Screen::History => "History",
            Screen::Settings => "Settings",
        }
    }
}

/// What an entry does when chosen.
#[derive(Clone, Debug)]
pub enum Command {
    /// The tab already open to the host.
    GoToTab(usize),
    Connect(HostId),
    OpenFiles(HostId),
    RunSnippet(Box<Snippet>),
    /// Switch a rule on (true) or off.
    Rule(ForwardRuleId, bool),
    Screen(Screen),
    NewHost,
    LockVault,
    NewTab,
    SplitRight,
    Broadcast,
    Find,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub section: Section,
    pub title: String,
    /// What it does: "Connect", "Go to session", "Screen".
    pub hint: String,
    /// Its shortcut, shown in place of the hint's chip.
    pub keys: Option<&'static str>,
    /// A host's group colour.
    pub color: Option<Rgba>,
    pub command: Command,
    /// Ctrl Enter opens it in a split of the current tab.
    pub splits: bool,
    /// Higher comes first among equal matches (a host's last connection).
    pub recent: i64,
    /// Shown only while searching (each host's Open SFTP).
    pub search_only: bool,
}

/// How well `query` matches `text`: 0 exact, 1 prefix, 2 a word's start,
/// 3 anywhere, 4 its letters in order. `None` is no match.
pub fn score(query: &str, text: &str) -> Option<u8> {
    let (q, t) = (query.trim().to_lowercase(), text.to_lowercase());
    if q.is_empty() {
        return Some(0);
    }
    if t == q {
        return Some(0);
    }
    if t.starts_with(&q) {
        return Some(1);
    }
    if t.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q)) {
        return Some(2);
    }
    if t.contains(&q) {
        return Some(3);
    }
    let mut rest = t.chars();
    q.chars().filter(|c| !c.is_whitespace()).all(|c| rest.any(|t| t == c)).then_some(4)
}

/// The entries to show for `query`, as indexes, in section order and best
/// match first within each section.
pub fn rank(entries: &[Entry], query: &str) -> Vec<usize> {
    let searching = !query.trim().is_empty();
    let mut hits: Vec<(Section, u8, i64, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| searching || !e.search_only)
        .filter_map(|(i, e)| score(query, &e.title).map(|s| (e.section, s, e.recent, i)))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)));
    let mut out = Vec::new();
    let mut count = 0;
    let mut section = None;
    for (s, _, _, i) in hits {
        if section != Some(s) {
            section = Some(s);
            count = 0;
        }
        if searching || count < s.idle_limit() {
            out.push(i);
            count += 1;
        }
    }
    out
}

pub enum PaletteEvent {
    Run { command: Command, split: bool },
    Dismiss,
}

pub struct CommandPalette {
    field: Entity<TextField>,
    entries: Vec<Entry>,
    shown: Vec<usize>,
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(entries: Vec<Entry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| TextField::new(cx, "Search hosts, snippets and commands"));
        field.read(cx).focus(window);
        let subscriptions = vec![cx.subscribe(&field, |this: &mut Self, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Changed => {
                this.refilter(cx);
            }
            TextFieldEvent::Submitted => this.choose(None, false, cx),
            TextFieldEvent::Cancelled => cx.emit(PaletteEvent::Dismiss),
        })];
        let shown = rank(&entries, "");
        Self { field, entries, shown, selected: 0, _subscriptions: subscriptions }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.field.read(cx).text().to_string();
        self.shown = rank(&self.entries, &query);
        self.selected = 0;
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let n = self.shown.len() as isize;
        self.selected = (self.selected as isize + by).rem_euclid(n) as usize;
        cx.notify();
    }

    /// Run the selected entry (or `at`), in a split when asked and it can.
    fn choose(&mut self, at: Option<usize>, split: bool, cx: &mut Context<Self>) {
        let Some(&i) = self.shown.get(at.unwrap_or(self.selected)) else { return };
        let entry = &self.entries[i];
        cx.emit(PaletteEvent::Run { command: entry.command.clone(), split: split && entry.splits });
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut section = None;
        for (pos, &i) in self.shown.iter().enumerate() {
            let e = &self.entries[i];
            if section != Some(e.section) {
                section = Some(e.section);
                rows.push(crate::ui::ship::palette_section(e.section.label()).into_any_element());
            }
            let on = pos == self.selected;
            let hint = match e.keys {
                Some(keys) => SharedString::from(format!("{} · {keys}", e.hint)),
                None => SharedString::from(e.hint.clone()),
            };
            rows.push(
                crate::ui::ship::palette_row(
                    SharedString::from(format!("palette-{i}")),
                    on,
                    e.color,
                    SharedString::from(e.title.clone()),
                    hint,
                )
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                    let split = ev.modifiers().control;
                    this.choose(Some(pos), split, cx)
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px(theme::SPACE_4)
                    .py(theme::SPACE_5)
                    .text_body()
                    .text_color(theme::ink_muted())
                    .child("Nothing matches. Try a host's name, a snippet or a screen.")
                    .into_any_element(),
            );
        }
        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &OpenSplit, _, cx| this.choose(None, true, cx)))
            .w_full()
            .max_w(px(640.))
            .flex()
            .flex_col()
            .bg(theme::card())
            .border_1()
            .border_color(theme::line_strong())
            .rounded(px(14.))
            .shadow(theme::shadow_dialog())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .px(theme::SPACE_4)
                    .py(theme::SPACE_2)
                    .border_b_1()
                    .border_color(theme::line())
                    .child(icon("search", px(16.), theme::ink_muted()))
                    .child(div().flex_1().min_w_0().child(self.field.clone()))
                    .child(crate::ui::ship::key_chip("Esc")),
            )
            .child(div().id("palette-rows").max_h(px(460.)).overflow_y_scroll().pb(theme::SPACE_2).children(rows))
            .child(
                div()
                    .px(theme::SPACE_4)
                    .py(px(10.))
                    .border_t_1()
                    .border_color(theme::line())
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .font_weight(FontWeight::NORMAL)
                    .child("Up and Down to move · Enter to open · Ctrl Enter to open in a split"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(section: Section, title: &str, recent: i64) -> Entry {
        Entry {
            section,
            title: title.into(),
            hint: String::new(),
            keys: None,
            color: None,
            command: Command::NewHost,
            splits: false,
            recent,
            search_only: false,
        }
    }

    #[test]
    fn scores_order_matches() {
        assert_eq!(score("nas-01", "NAS-01"), Some(0));
        assert_eq!(score("prod", "prod-web-01"), Some(1));
        assert_eq!(score("web", "prod-web-01"), Some(2));
        assert_eq!(score("eb-0", "prod-web-01"), Some(3));
        assert_eq!(score("pw1", "prod-web-01"), Some(4));
        assert_eq!(score("xyz", "prod-web-01"), None);
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn ranking_keeps_sections_and_puts_best_first() {
        let mut entries = vec![
            entry(Section::Actions, "New host", 0),
            entry(Section::Hosts, "prod-web-02", 10),
            entry(Section::Hosts, "web", 0),
            entry(Section::Hosts, "prod-web-01", 20),
            entry(Section::GoTo, "Settings", 0),
        ];
        let mut sftp = entry(Section::Hosts, "web", 0);
        sftp.search_only = true;
        entries.push(sftp);
        let titles = |q: &str| rank(&entries, q).into_iter().map(|i| entries[i].title.clone()).collect::<Vec<_>>();
        // Exact first, then by word start with recent first; sections in order.
        assert_eq!(titles("web"), ["web", "web", "prod-web-01", "prod-web-02"]);
        // Nothing typed: recent hosts first, search-only rows hidden.
        assert_eq!(titles(""), ["prod-web-01", "prod-web-02", "web", "Settings", "New host"]);
        assert_eq!(titles("set"), ["Settings"]);
    }

    #[test]
    fn idle_lists_are_capped() {
        let entries: Vec<Entry> = (0..10).map(|i| entry(Section::Hosts, &format!("h{i}"), i)).collect();
        assert_eq!(rank(&entries, "").len(), 6);
        assert_eq!(rank(&entries, "h").len(), 10);
    }
}
