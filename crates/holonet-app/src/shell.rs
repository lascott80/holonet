//! App chrome: the title bar (brand, vault tab, session tabs, window buttons)
//! and the side navigation, from the Main, Terminal and KnownHosts screens.
//!
//! The window has no system title bar (`appears_transparent`); the drawn bar
//! is a native drag area and its buttons are native minimize/maximize/close
//! areas, so Windows keeps snapping, double-click maximize and snap layouts.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, IntoElement, Rgba, SharedString, Window, WindowControlArea, div,
    prelude::*, px,
};

use crate::theme::{self, TypeStyles};
use crate::ui::icon;

type Handler = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type NavHandler = std::rc::Rc<dyn Fn(NavEntry, &mut Window, &mut App) + 'static>;
use crate::ui::IndexHandler;

/// Title bar height (Main/Terminal screens: min-height 44).
pub const TITLE_BAR_HEIGHT: gpui::Pixels = px(44.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Terminal,
    Sftp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabState {
    Connected,
    Connecting,
    Failed,
}

/// A session tab as the title bar draws it.
#[derive(Clone)]
pub struct TabInfo {
    pub id: ElementId,
    pub name: SharedString,
    pub kind: TabKind,
    pub state: TabState,
    /// Group color for the top edge (`theme::no_group()` when ungrouped).
    pub color: Rgba,
    pub active: bool,
}

#[derive(IntoElement)]
pub struct TitleBar {
    vault_active: bool,
    tabs: Vec<TabInfo>,
    on_vault: Option<Handler>,
    on_tab: Option<IndexHandler>,
    on_close_tab: Option<IndexHandler>,
    on_new_tab: Option<Handler>,
    /// Scroll position of the tab strip (tabs scroll sideways when full).
    scroll: Option<gpui::ScrollHandle>,
    /// Filled with the "+" button's bounds at paint time.
    plus_bounds: Option<std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
}

impl TitleBar {
    pub fn new(vault_active: bool, tabs: Vec<TabInfo>) -> Self {
        Self {
            vault_active,
            tabs,
            on_vault: None,
            on_tab: None,
            on_close_tab: None,
            on_new_tab: None,
            scroll: None,
            plus_bounds: None,
        }
    }

    pub fn scroll(mut self, handle: gpui::ScrollHandle) -> Self {
        self.scroll = Some(handle);
        self
    }

    pub fn plus_bounds(mut self, cell: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>) -> Self {
        self.plus_bounds = Some(cell);
        self
    }

    pub fn on_vault(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_vault = Some(std::rc::Rc::new(f));
        self
    }

    pub fn on_tab(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_tab = Some(std::rc::Rc::new(f));
        self
    }

    pub fn on_close_tab(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_close_tab = Some(std::rc::Rc::new(f));
        self
    }

    pub fn on_new_tab(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_new_tab = Some(std::rc::Rc::new(f));
        self
    }
}

fn state_dot(state: TabState) -> Div {
    match state {
        TabState::Connected => div().size(px(6.)).rounded_full().bg(theme::ink_soft()),
        TabState::Connecting => div().size(px(7.)).rounded_full().border(px(1.5)).border_color(theme::ink_soft()),
        TabState::Failed => div().size(px(6.)).rounded_full().bg(theme::warn()),
    }
    .flex_none()
}

fn window_button(id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool) -> impl IntoElement {
    div()
        .id(id)
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .hover(move |s| if close { s.bg(gpui::rgb(0xC42B1C)) } else { s.bg(theme::card()) })
        .child(icon(glyph, px(10.), theme::ink_muted()))
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let vault = div()
            .id("vault-tab")
            .w(px(44.))
            .h(px(38.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_t(theme::RADIUS_SM)
            .cursor_pointer()
            .when(self.vault_active, |d| d.bg(theme::ground()).border_1().border_b_0().border_color(theme::line()))
            .when(!self.vault_active, |d| d.hover(|s| s.bg(theme::tab_hover())))
            .child(icon("hosts", px(16.), if self.vault_active { theme::accent() } else { theme::ink_muted() }))
            .when_some(self.on_vault.clone(), |d, f| d.on_click(move |e, w, cx| f(e, w, cx)));

        let tabs = self.tabs.iter().cloned().enumerate().map(|(i, tab)| {
            let edge =
                if tab.active { tab.color } else { theme::with_alpha(tab.color, theme::INACTIVE_TAB_EDGE_ALPHA) };
            let on_tab = self.on_tab.clone();
            let on_close = self.on_close_tab.clone();
            div()
                .id(tab.id.clone())
                .flex_shrink()
                .flex_basis(px(190.))
                .w(px(190.))
                .min_w(px(130.))
                .h(px(38.))
                .pl(theme::SPACE_3)
                .flex()
                .items_center()
                .gap(theme::SPACE_2)
                .rounded_t(theme::RADIUS_SM)
                .border_t_2()
                .border_color(edge)
                .cursor_pointer()
                .text_body()
                .when(tab.active, |d| {
                    d.bg(theme::ground())
                        .border_l_1()
                        .border_r_1()
                        .text_color(theme::ink())
                        .font_weight(gpui::FontWeight::MEDIUM)
                })
                .when(!tab.active, |d| {
                    d.bg(theme::card()).text_color(theme::ink_muted()).hover(|s| s.bg(theme::tab_hover()))
                })
                .child(icon(
                    if tab.kind == TabKind::Sftp { "folder" } else { "terminal" },
                    px(14.),
                    if tab.active { theme::ink() } else { theme::ink_muted() },
                ))
                .child(state_dot(tab.state))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(tab.name.clone()))
                .child(
                    div()
                        .id(SharedString::from(format!("close-{i}")))
                        .size(px(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme::RADIUS_SM)
                        .hover(|s| s.bg(theme::raised()))
                        .child(icon("close", px(9.), theme::ink_muted()))
                        .when_some(on_close, |d, f| {
                            d.on_click(move |_, w, cx| {
                                cx.stop_propagation();
                                f(i, w, cx)
                            })
                        }),
                )
                .when_some(on_tab, |d, f| d.on_click(move |_, w, cx| f(i, w, cx)))
        });

        div()
            .id("title-bar")
            .h(TITLE_BAR_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .gap(theme::SPACE_3)
            .pl(px(14.))
            .bg(theme::raised())
            .border_b_1()
            .border_color(theme::line())
            // Only the brand and the empty space after the tabs drag the
            // window: a drag area over the tabs would turn their clicks into
            // title bar clicks.
            .child(
                div()
                    .id("title-brand")
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .window_control_area(WindowControlArea::Drag)
                    .child(icon("globe", px(18.), theme::accent()))
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(15.))
                            .text_color(theme::ink())
                            .child("Holonet"),
                    ),
            )
            .child(
                div()
                    .flex_shrink()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_end()
                    .gap(px(2.))
                    .child(div().flex_none().pt(px(6.)).h_full().flex().items_end().child(vault))
                    .child(div().flex_none().w(px(1.)).h(px(20.)).mx(px(6.)).mb(px(9.)).bg(theme::tab_divider()))
                    .child(
                        // The vault tab is pinned; session tabs keep at least 130px and scroll sideways when
                        // they no longer fit (Terminal.dc.html: overflow-x auto).
                        div()
                            .id("tab-strip")
                            .flex_shrink()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_end()
                            .gap(px(2.))
                            .pt(px(6.))
                            .overflow_x_scroll()
                            .when_some(self.scroll.as_ref(), |d, h| d.track_scroll(h))
                            .children(tabs),
                    )
                    // "+" only where it does something (milestone 2: no idle controls).
                    .when_some(self.on_new_tab, |d, f| {
                        let plus_bounds = self.plus_bounds.clone();
                        d.child(
                            div()
                                .id("new-tab")
                                .size(px(36.))
                                .mb(px(1.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::RADIUS_SM)
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::card()))
                                .child(icon("plus", px(18.), theme::ink_muted()))
                                .when_some(plus_bounds, |d, cell| {
                                    d.child(
                                        gpui::canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                                            .absolute()
                                            .size_full(),
                                    )
                                })
                                .on_click(move |e, w, cx| f(e, w, cx)),
                        )
                    }),
            )
            .child(div().id("title-drag").flex_1().min_w(px(48.)).h_full().window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .h_full()
                    .flex()
                    .child(window_button("win-min", "win-minimize", WindowControlArea::Min, false))
                    .child(window_button("win-max", "win-maximize", WindowControlArea::Max, false))
                    .child(window_button("win-close", "win-close", WindowControlArea::Close, true)),
            )
    }
}

/// Side navigation entries (Main, EditHost, KnownHosts screens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavEntry {
    Hosts,
    Keychain,
    PortForwarding,
    Snippets,
    KnownHosts,
    Terminal,
    Sftp,
    History,
    Settings,
}

impl NavEntry {
    pub fn label(self) -> &'static str {
        match self {
            NavEntry::Hosts => "Hosts",
            NavEntry::Keychain => "Keychain",
            NavEntry::PortForwarding => "Port forwarding",
            NavEntry::Snippets => "Snippets",
            NavEntry::KnownHosts => "Known hosts",
            NavEntry::Terminal => "Terminal",
            NavEntry::Sftp => "SFTP",
            NavEntry::History => "History",
            NavEntry::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            NavEntry::Hosts => "hosts",
            NavEntry::Keychain => "key",
            NavEntry::PortForwarding => "forwarding",
            NavEntry::Snippets => "snippets",
            NavEntry::KnownHosts => "known-hosts",
            NavEntry::Terminal => "terminal",
            NavEntry::Sftp => "folder",
            NavEntry::History => "history",
            NavEntry::Settings => "settings",
        }
    }

    pub const VAULT: [NavEntry; 5] =
        [NavEntry::Hosts, NavEntry::Keychain, NavEntry::PortForwarding, NavEntry::Snippets, NavEntry::KnownHosts];
    pub const SESSIONS: [NavEntry; 3] = [NavEntry::Terminal, NavEntry::Sftp, NavEntry::History];
}

#[derive(IntoElement)]
pub struct SideNav {
    active: Option<NavEntry>,
    /// Entries that exist in this build; the rest are left out (milestone 2
    /// plan: no control that does nothing).
    available: Vec<NavEntry>,
    update_available: bool,
    vault_status: SharedString,
    on_select: Option<NavHandler>,
    on_lock: Option<Handler>,
}

impl SideNav {
    pub fn new(active: Option<NavEntry>, available: Vec<NavEntry>) -> Self {
        Self {
            active,
            available,
            update_available: false,
            vault_status: "Encrypted, unlocked with vault password".into(),
            on_select: None,
            on_lock: None,
        }
    }

    /// The vault also unlocks with Windows Hello.
    pub fn hello(mut self, yes: bool) -> Self {
        if yes {
            self.vault_status = "Encrypted, unlocks with Windows Hello or the vault password".into();
        }
        self
    }

    pub fn update_available(mut self, yes: bool) -> Self {
        self.update_available = yes;
        self
    }

    pub fn on_select(mut self, f: impl Fn(NavEntry, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }

    pub fn on_lock(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_lock = Some(std::rc::Rc::new(f));
        self
    }

    fn item(&self, entry: NavEntry) -> AnyElement {
        let active = self.active == Some(entry);
        let on_select = self.on_select.clone();
        let dot = entry == NavEntry::Settings && self.update_available;
        div()
            .id(entry.label())
            .relative()
            .px(theme::SPACE_3)
            .h(theme::CONTROL)
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(theme::RADIUS_MD)
            .cursor_pointer()
            .text_body()
            .when(active, |d| d.bg(theme::raised()).text_color(theme::ink()).font_weight(gpui::FontWeight::MEDIUM))
            .when(!active, |d| d.text_color(theme::ink_muted()).hover(|s| s.text_color(theme::ink())))
            .child(icon(entry.icon(), px(16.), if active { theme::accent() } else { theme::ink_muted() }))
            .child(entry.label())
            .when(dot, |d| {
                d.child(div().absolute().top(px(18.)).right(px(12.)).size(px(8.)).rounded_full().bg(theme::accent()))
            })
            .when_some(on_select, |d, f| d.on_click(move |_, w, cx| f(entry, w, cx)))
            .into_any_element()
    }
}

impl RenderOnce for SideNav {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let section = |label: &str, top: gpui::Pixels| {
            div().px(theme::SPACE_3).pt(top).pb(theme::SPACE_2).child(crate::ui::section_label(label))
        };
        let vault: Vec<_> = NavEntry::VAULT.into_iter().filter(|e| self.available.contains(e)).collect();
        let sessions: Vec<_> = NavEntry::SESSIONS.into_iter().filter(|e| self.available.contains(e)).collect();
        let lock = self.on_lock.clone();
        div()
            .w(px(220.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(10.))
            .py(theme::SPACE_4)
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::line())
            .child(section("Vault", px(4.)))
            .children(vault.into_iter().map(|e| self.item(e)))
            .when(!sessions.is_empty(), |d| {
                d.child(section("Sessions", px(20.))).children(sessions.into_iter().map(|e| self.item(e)))
            })
            .child(div().flex_1())
            .when(self.available.contains(&NavEntry::Settings), |d| d.child(self.item(NavEntry::Settings)))
            .child(
                div()
                    .mt(theme::SPACE_3)
                    .mx(theme::SPACE_1)
                    .p(theme::SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .rounded(theme::RADIUS_MD)
                    .border_1()
                    .border_color(theme::line())
                    .child(
                        div()
                            .text_meta()
                            .child(
                                div()
                                    .text_color(theme::ink())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Local vault"),
                            )
                            .child(div().text_color(theme::ink_muted()).child(self.vault_status.clone())),
                    )
                    .when_some(lock, |d, f| {
                        d.child(
                            crate::ui::Button::new("lock-now", "Lock now", crate::ui::ButtonKind::Secondary)
                                .compact()
                                .icon("lock")
                                .full_width()
                                .on_click(move |e, w, cx| f(e, w, cx)),
                        )
                    }),
            )
    }
}
