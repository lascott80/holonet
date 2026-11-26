//! A session tab (Terminal.dc.html): the session header and status bar for
//! the active pane, and the panes themselves on the `sunken` gutter.
//!
//! Panes form a tree: a leaf is one pane; a split puts two subtrees side by
//! side (Split right) or one above the other (Split down), sized by a ratio.
//! A tab holds at most [`MAX_PANES`]. The 8px gap between two subtrees is the
//! handle for resizing them.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, EventEmitter, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Render, SharedString, Subscription, WeakEntity, Window, canvas, div,
    prelude::*, px,
};
use holonet_model::HostId;

use crate::app::HolonetApp;
use crate::session::Pane;
use crate::shell::TabState;
use crate::terminal_view::{SearchQuery, SearchStatus};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, icon};
use crate::ui::{Checkbox, TextField, TextFieldEvent};

/// Owner decision 1: up to four panes per tab.
pub const MAX_PANES: usize = 4;
/// Gap between panes (Terminal.dc.html: `gap: 8px`).
const GAP: Pixels = px(8.);
/// Smallest pane while dragging: about 20 columns by 5 rows at the default
/// font, plus the pane header and padding.
const MIN_WIDTH: f32 = 190.;
const MIN_HEIGHT: f32 = 170.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side ("Split right").
    Right,
    /// One above the other ("Split down").
    Down,
}

enum Node {
    Leaf(Entity<Pane>),
    Split(Box<Split>),
}

struct Split {
    axis: Axis,
    /// Share of the space the first subtree takes, 0..1.
    ratio: f32,
    first: Node,
    second: Node,
    /// Where the split was drawn, for turning a drag into a ratio.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Node {
    fn leaves(&self, out: &mut Vec<Entity<Pane>>) {
        match self {
            Node::Leaf(p) => out.push(p.clone()),
            Node::Split(s) => {
                s.first.leaves(out);
                s.second.leaves(out);
            }
        }
    }

    /// Replace the leaf holding `target` with a split of it and `new`.
    fn split(&mut self, target: &Entity<Pane>, axis: Axis, new: Entity<Pane>) -> bool {
        match self {
            Node::Leaf(p) if p == target => {
                let old = std::mem::replace(self, Node::Leaf(new.clone()));
                *self = Node::Split(Box::new(Split {
                    axis,
                    ratio: 0.5,
                    first: old,
                    second: Node::Leaf(new),
                    bounds: Rc::new(Cell::new(Bounds::default())),
                }));
                true
            }
            Node::Leaf(_) => false,
            Node::Split(s) => s.first.split(target, axis, new.clone()) || s.second.split(target, axis, new),
        }
    }

    /// Remove the leaf holding `pane`; its sibling takes the parent's place.
    fn remove(&mut self, pane: &Entity<Pane>) -> bool {
        let Node::Split(s) = self else { return false };
        let keep = if matches!(&s.first, Node::Leaf(p) if p == pane) {
            Some(true)
        } else if matches!(&s.second, Node::Leaf(p) if p == pane) {
            Some(false)
        } else {
            None
        };
        if let Some(keep_second) = keep {
            let placeholder = Node::Split(Box::new(Split {
                axis: Axis::Right,
                ratio: 0.5,
                first: Node::Leaf(pane.clone()),
                second: Node::Leaf(pane.clone()),
                bounds: Rc::new(Cell::new(Bounds::default())),
            }));
            let Node::Split(s) = std::mem::replace(self, placeholder) else { unreachable!() };
            *self = if keep_second { s.second } else { s.first };
            return true;
        }
        s.first.remove(pane) || s.second.remove(pane)
    }

    fn split_at(&mut self, path: &[bool]) -> Option<&mut Split> {
        let Node::Split(s) = self else { return None };
        match path.split_first() {
            None => Some(s),
            Some((false, rest)) => s.first.split_at(rest),
            Some((true, rest)) => s.second.split_at(rest),
        }
    }
}

gpui::actions!(find_bar, [FindNext]);

const FIND_CONTEXT: &str = "FindBar";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new("shift-enter", FindNext, Some(FIND_CONTEXT))]);
}

/// The find bar (Terminal.dc.html): searches the active pane's scrollback.
struct FindBar {
    field: Entity<TextField>,
    match_case: bool,
    regex: bool,
    status: SearchStatus,
    _subscription: Subscription,
}

/// The tab wants another pane (the app creates it: it knows hosts and settings).
pub enum TabEvent {
    Split {
        axis: Axis,
        host: Option<HostId>,
    },
    /// "Open SFTP here": an SFTP tab for the active pane's host, with its
    /// typed secrets.
    OpenFiles {
        host: HostId,
        remembered: crate::session::Remembered,
    },
}

pub struct Tab {
    app: WeakEntity<HolonetApp>,
    root: Node,
    active: Entity<Pane>,
    /// Path (false = first, true = second) of the split being resized.
    drag: Option<Vec<bool>>,
    /// The split menu, while open: its axis and the hosts it offers.
    menu: Option<(Axis, Vec<(HostId, SharedString)>)>,
    find: Option<FindBar>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TabEvent> for Tab {}

impl Tab {
    pub fn new(app: WeakEntity<HolonetApp>, pane: Entity<Pane>, cx: &mut Context<Self>) -> Self {
        // Redraw the header, status bar and title bar when a pane changes.
        let subscriptions = vec![cx.observe(&pane, |_, _, cx| cx.notify())];
        Self {
            app,
            root: Node::Leaf(pane.clone()),
            active: pane,
            drag: None,
            menu: None,
            find: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn panes(&self) -> Vec<Entity<Pane>> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    pub fn active_pane(&self) -> &Entity<Pane> {
        &self.active
    }

    pub fn contains(&self, pane: &Entity<Pane>) -> bool {
        self.panes().contains(pane)
    }

    pub fn can_split(&self) -> bool {
        self.panes().len() < MAX_PANES
    }

    /// Split the active pane and make the new one active.
    pub fn split_active(&mut self, axis: Axis, pane: Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_split() {
            return;
        }
        let target = self.active.clone();
        self._subscriptions.push(cx.observe(&pane, |_, _, cx| cx.notify()));
        self.root.split(&target, axis, pane.clone());
        self.activate(pane, window, cx);
    }

    /// Remove a pane. Returns true when the tab has none left.
    pub fn remove(&mut self, pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let panes = self.panes();
        if panes.len() <= 1 {
            return panes.first().is_none_or(|p| p == pane);
        }
        let index = panes.iter().position(|p| p == pane).unwrap_or(0);
        self.root.remove(pane);
        if &self.active == pane {
            let rest = self.panes();
            let next = rest[index.min(rest.len() - 1)].clone();
            self.activate(next, window, cx);
        } else {
            self.mark_panes(cx);
        }
        cx.notify();
        false
    }

    fn activate(&mut self, pane: Entity<Pane>, window: &mut Window, cx: &mut Context<Self>) {
        // Find follows the active pane.
        if self.find.is_some() && self.active != pane {
            self.active.read(cx).terminal().clone().update(cx, |t, cx| {
                t.set_search(None, cx);
            });
        }
        self.active = pane;
        if self.find.is_some() {
            self.run_find(cx);
        }
        self.mark_panes(cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Tell each pane whether it is active and whether it is alone in the tab.
    fn mark_panes(&self, cx: &mut Context<Self>) {
        let panes = self.panes();
        let alone = panes.len() == 1;
        for p in panes {
            let active = p == self.active;
            p.update(cx, |p, cx| p.set_place(active, alone, cx));
        }
    }

    /// Ctrl Shift ] and Ctrl Shift [: the next or previous pane.
    pub fn cycle_pane(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.panes();
        if panes.len() < 2 {
            return;
        }
        let i = panes.iter().position(|p| p == &self.active).unwrap_or(0) as isize;
        let next = panes[(i + delta).rem_euclid(panes.len() as isize) as usize].clone();
        self.activate(next, window, cx);
    }

    /// The tab's name and dot follow its active pane.
    pub fn name(&self, cx: &App) -> SharedString {
        self.active.read(cx).name()
    }

    /// An SFTP tab: one Files pane, no splits, no Find.
    pub fn is_files(&self, cx: &App) -> bool {
        self.active.read(cx).is_files()
    }

    pub fn state(&self, cx: &App) -> TabState {
        self.active.read(cx).tab_state()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.active.update(cx, |p, cx| p.focus(window, cx));
    }

    /// Ctrl Shift F or the Find button: open the bar (and focus its field),
    /// or close it.
    pub fn toggle_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_files(cx) {
            return;
        }
        if self.find.is_some() {
            self.close_find(window, cx);
            return;
        }
        let field = cx.new(|cx| TextField::new(cx, "Find in scrollback"));
        let sub =
            cx.subscribe_in(&field, window, |this: &mut Self, _, event: &TextFieldEvent, window, cx| match event {
                TextFieldEvent::Changed => this.run_find(cx),
                // Enter goes up to the previous (older) match; Shift Enter down.
                TextFieldEvent::Submitted => this.step_find(-1, cx),
                TextFieldEvent::Cancelled => this.close_find(window, cx),
            });
        field.read(cx).focus(window);
        self.find =
            Some(FindBar { field, match_case: false, regex: false, status: SearchStatus::Empty, _subscription: sub });
        cx.notify();
    }

    /// Ctrl Shift F with the bar open: focus its field again.
    pub fn focus_find(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match &self.find {
            Some(f) if !f.field.read(cx).is_focused(window) => {
                f.field.read(cx).focus(window);
                true
            }
            _ => false,
        }
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.active.read(cx).terminal().clone().update(cx, |t, cx| {
            t.set_search(None, cx);
        });
        self.focus(window, cx);
        cx.notify();
    }

    fn run_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else { return };
        let query = SearchQuery {
            text: find.field.read(cx).text().to_string(),
            match_case: find.match_case,
            regex: find.regex,
        };
        let terminal = self.active.read(cx).terminal().clone();
        find.status = terminal.update(cx, |t, cx| t.set_search(Some(&query), cx));
        cx.notify();
    }

    fn step_find(&mut self, delta: isize, cx: &mut Context<Self>) {
        let terminal = self.active.read(cx).terminal().clone();
        let status = terminal.update(cx, |t, cx| {
            t.search_step(delta, cx);
            t.search_status()
        });
        if let Some(find) = self.find.as_mut() {
            find.status = status;
        }
        cx.notify();
    }

    fn find_bar(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        // Live: new output changes the count without a keystroke.
        let status = match find.status {
            SearchStatus::Invalid | SearchStatus::Empty => find.status,
            _ => self.active.read(cx).terminal().read(cx).search_status(),
        };
        let warn = matches!(status, SearchStatus::Invalid);
        let found = matches!(status, SearchStatus::Found { .. });
        let _ = window;
        let nav = |id: &'static str, glyph: &'static str, delta: isize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(theme::CONTROL_COMPACT)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_MD)
                .border_1()
                .border_color(theme::line())
                .when(found, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(theme::raised()))
                        .on_click(cx.listener(move |this, _, _, cx| this.step_find(delta, cx)))
                })
                .when(!found, |d| d.opacity(0.4))
                .child(icon(glyph, px(16.), theme::ink_muted()))
        };
        Some(
            div()
                .key_context(FIND_CONTEXT)
                .on_action(cx.listener(|this, _: &FindNext, _, cx| this.step_find(1, cx)))
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme::SPACE_3)
                .px(px(18.))
                .py(theme::SPACE_2)
                .bg(theme::panel())
                .border_b_1()
                .border_color(theme::line())
                .child(
                    div()
                        .flex_1()
                        .max_w(px(420.))
                        .min_w(px(220.))
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .child(icon("search", px(16.), theme::ink_muted()))
                        .child(div().flex_1().child(find.field.clone())),
                )
                .child(
                    div()
                        .min_w(px(90.))
                        .text_mono_small()
                        .text_color(if warn { theme::warn() } else { theme::ink_muted() })
                        .child(SharedString::from(status.text())),
                )
                .child(nav("find-prev", "chevron-up", -1, cx))
                .child(nav("find-next", "chevron-down", 1, cx))
                .child(Checkbox::new("find-case", "Match case", find.match_case).on_toggle({
                    let this = cx.weak_entity();
                    move |v, _, cx| {
                        let _ = this.update(cx, |t, cx| {
                            if let Some(f) = t.find.as_mut() {
                                f.match_case = v;
                            }
                            t.run_find(cx);
                        });
                    }
                }))
                .child(Checkbox::new("find-regex", "Regex", find.regex).on_toggle({
                    let this = cx.weak_entity();
                    move |v, _, cx| {
                        let _ = this.update(cx, |t, cx| {
                            if let Some(f) = t.find.as_mut() {
                                f.regex = v;
                            }
                            t.run_find(cx);
                        });
                    }
                }))
                .into_any_element(),
        )
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn open_menu(&mut self, axis: Axis, cx: &mut Context<Self>) {
        let hosts = self.app.upgrade().map(|app| app.read(cx).host_names(cx)).unwrap_or_default();
        self.menu = Some((axis, hosts));
        cx.notify();
    }

    fn choose(&mut self, axis: Axis, host: Option<HostId>, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(TabEvent::Split { axis, host });
        cx.notify();
    }

    fn on_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(path) = self.drag.clone() else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some(split) = self.root.split_at(&path) else { return };
        let b = split.bounds.get();
        let (pos, start, size, min) = match split.axis {
            Axis::Right => (event.position.x, b.origin.x, b.size.width, MIN_WIDTH),
            Axis::Down => (event.position.y, b.origin.y, b.size.height, MIN_HEIGHT),
        };
        let size = f32::from(size);
        if size < 2.0 * min + f32::from(GAP) {
            return;
        }
        let t = f32::from(pos - start) / size;
        let edge = min / size;
        split.ratio = t.clamp(edge, 1.0 - edge);
        cx.notify();
    }

    fn render_node(&self, node: &Node, path: Vec<bool>, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Leaf(pane) => {
                let target = pane.clone();
                div()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    // Clicking anywhere in a pane makes it the active one.
                    .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        if this.active != target {
                            this.activate(target.clone(), window, cx);
                        }
                    }))
                    .child(pane.clone())
                    .into_any_element()
            }
            Node::Split(s) => {
                let bounds = s.bounds.clone();
                let horizontal = s.axis == Axis::Right;
                let dragging = self.drag.as_deref() == Some(&path[..]);
                let mut first_path = path.clone();
                first_path.push(false);
                let mut second_path = path.clone();
                second_path.push(true);
                let handle_path = path.clone();
                let handle = div()
                    .id(SharedString::from(format!("split-{path:?}")))
                    .flex_none()
                    .when(horizontal, |d| d.w(GAP).h_full().cursor(CursorStyle::ResizeLeftRight))
                    .when(!horizontal, |d| d.h(GAP).w_full().cursor(CursorStyle::ResizeUpDown))
                    .flex()
                    .items_center()
                    .justify_center()
                    .group("split-handle")
                    .child(
                        // A line shows on hover and while dragging (undrawn
                        // state, built from the accent token).
                        div()
                            .when(horizontal, |d| d.w(px(2.)).h_full())
                            .when(!horizontal, |d| d.h(px(2.)).w_full())
                            .rounded(px(1.))
                            .when(dragging, |d| d.bg(theme::accent()))
                            .when(!dragging, |d| d.group_hover("split-handle", |s| s.bg(theme::accent_alpha(0.5)))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.drag = Some(handle_path.clone());
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    );
                div()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .relative()
                    .flex()
                    .when(!horizontal, |d| d.flex_col())
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .flex_basis(px(0.))
                            .flex_shrink()
                            .min_w_0()
                            .min_h_0()
                            .map(|d| d.flex_grow_ratio(s.ratio))
                            .child(self.render_node(&s.first, first_path, cx)),
                    )
                    .child(handle)
                    .child(
                        div()
                            .flex_basis(px(0.))
                            .flex_shrink()
                            .min_w_0()
                            .min_h_0()
                            .map(|d| d.flex_grow_ratio(1.0 - s.ratio))
                            .child(self.render_node(&s.second, second_path, cx)),
                    )
                    .into_any_element()
            }
        }
    }

    fn split_control(&self, axis: Axis, can_split: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, label) = match axis {
            Axis::Right => ("split-right", "Split right"),
            Axis::Down => ("split-down", "Split down"),
        };
        let open = matches!(self.menu, Some((a, _)) if a == axis);
        div()
            .relative()
            .flex()
            .child(
                Button::secondary(id, label)
                    .disabled(!can_split)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(axis, None, cx))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-menu")))
                    .ml(px(2.))
                    .w(px(32.))
                    .h(theme::CONTROL)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_MD)
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::card())
                    .when(can_split, |d| {
                        d.cursor_pointer().hover(|s| s.bg(theme::raised())).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if open {
                                    this.menu = None;
                                    cx.notify();
                                } else {
                                    this.open_menu(axis, cx);
                                }
                            },
                        ))
                    })
                    .when(!can_split, |d| d.opacity(0.4))
                    .child(icon("chevron-down", px(14.), theme::ink_muted())),
            )
            .when(open, |d| d.child(self.split_menu(axis, cx)))
    }

    /// "Same host" and the saved hosts (undrawn state: a popup like Select's).
    fn split_menu(&self, axis: Axis, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let hosts = self.menu.as_ref().map(|(_, h)| h.clone()).unwrap_or_default();
        let row = |id: SharedString, label: SharedString, detail: Option<&'static str>| {
            div()
                .id(id)
                .h(px(40.))
                .px(theme::SPACE_3)
                .flex()
                .items_center()
                .justify_between()
                .gap(theme::SPACE_3)
                .rounded(theme::RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(theme::raised()))
                .text_body()
                .text_color(theme::ink())
                .child(label)
                .children(detail.map(|d| div().text_meta().text_color(theme::ink_muted()).child(d)))
        };
        let menu = div()
            .id("split-menu")
            .mt(theme::CONTROL + px(4.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .w(px(260.))
            .p(theme::SPACE_1)
            .flex()
            .flex_col()
            .bg(theme::card())
            .border_1()
            .border_color(theme::line_strong())
            .rounded(theme::RADIUS_MD)
            .shadow(theme::shadow_dialog())
            .occlude()
            .child(
                row(
                    "menu-same".into(),
                    "Same host".into(),
                    Some(if axis == Axis::Right { "Ctrl Shift D" } else { "Ctrl Shift E" }),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.choose(axis, None, cx))),
            )
            .child(div().h(px(1.)).my(theme::SPACE_1).bg(theme::line()))
            .children(hosts.into_iter().enumerate().map(|(i, (id, name))| {
                row(SharedString::from(format!("menu-host-{i}")), name, None)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(axis, Some(id), cx)))
            }));
        // Drawn above the panes (GPUI `deferred`), anchored at the control's
        // right edge and kept inside the window.
        gpui::deferred(gpui::anchored().anchor(gpui::Corner::TopRight).snap_to_window_with_margin(px(8.)).child(menu))
    }
}

/// `flex-grow` from a ratio: GPUI takes the grow factor as f32.
trait FlexGrowRatio {
    fn flex_grow_ratio(self, ratio: f32) -> Self;
}

impl FlexGrowRatio for gpui::Div {
    fn flex_grow_ratio(mut self, ratio: f32) -> Self {
        self.style().flex_grow = Some(ratio.max(0.01));
        self
    }
}

impl Render for Tab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let find_bar = self.find_bar(window, cx);
        let (name, target, status) = {
            let p = self.active.read(cx);
            (p.name(), p.target_line(), p.status_bar(cx))
        };
        let can_split = self.can_split();
        let files = self.is_files(cx);
        let body = self.render_node(&self.root, Vec::new(), cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_drag(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            // Session header: the active pane's host, and the split controls.
            .child(
                div()
                    .px(px(18.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(theme::line())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(theme::SPACE_3)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(theme::DISPLAY)
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::ink())
                                    .child(name),
                            )
                            .child(div().text_mono_small().text_color(theme::ink_muted()).child(target)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme::SPACE_2)
                            .when(!can_split && !files, |d| {
                                d.child(
                                    div()
                                        .text_meta()
                                        .text_color(theme::ink_muted())
                                        .child(format!("A tab holds up to {MAX_PANES} panes.")),
                                )
                            })
                            // Terminal.dc.html: "Open SFTP here" for the active pane's host.
                            .when(!files, |d| {
                                d.child(Button::secondary("open-sftp-here", "Open SFTP here").icon("folder").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let (host, remembered) = {
                                            let p = this.active.read(cx);
                                            (p.host.id, p.remembered())
                                        };
                                        cx.emit(TabEvent::OpenFiles { host, remembered });
                                    }),
                                ))
                            })
                            .when(!files, |d| {
                                d.child(self.split_control(Axis::Right, can_split, cx)).child(self.split_control(
                                    Axis::Down,
                                    can_split,
                                    cx,
                                ))
                            })
                            .when(!files, |d| {
                                d.child({
                                    // A toggle: the accent border while the bar is open.
                                    let on = self.find.is_some();
                                    div()
                                        .id("find-toggle")
                                        .h(theme::CONTROL)
                                        .px(px(14.))
                                        .flex()
                                        .items_center()
                                        .gap(theme::SPACE_2)
                                        .rounded(theme::RADIUS_MD)
                                        .border_1()
                                        .border_color(if on { theme::accent() } else { theme::line() })
                                        .bg(theme::card())
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme::raised()))
                                        .text_body()
                                        .text_color(theme::ink())
                                        .child(icon("search", px(16.), theme::ink_muted()))
                                        .child("Find")
                                        .on_click(cx.listener(|this, _, window, cx| this.toggle_find(window, cx)))
                                })
                            }),
                    ),
            )
            .children(find_bar)
            // Gutter and panes.
            .child(div().flex_1().min_h_0().p(theme::SPACE_2).bg(theme::sunken()).child(body))
            .child(status)
    }
}
