//! Milestone 4 controls (Keychain.dc.html), shared by the Keychain page and
//! `--gallery`: the table with a selected row, identity cards with the
//! dashed "new" card, the multi-select list for importing keys, and the
//! side panel.

use gpui::{
    AnyElement, App, ClickEvent, Div, FontWeight, IntoElement, SharedString, Stateful, Window, div, prelude::*, px,
};

use super::{Checkbox, icon};
use crate::theme::{self, TypeStyles};

/// A table column: its upper-case label and a fixed width, or `None` to
/// share the remaining width.
pub struct TableColumn {
    pub label: &'static str,
    pub width: Option<f32>,
}

/// A row's click handler.
pub type RowClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One row: its cells (one per column), whether it is selected, and what a
/// click does.
pub struct TableRow {
    pub id: SharedString,
    pub cells: Vec<AnyElement>,
    pub selected: bool,
    pub on_click: Option<RowClick>,
}

fn cell(width: Option<f32>, child: impl IntoElement) -> Div {
    match width {
        Some(w) => div().w(px(w)).flex_none().child(child),
        None => div().flex_grow().flex_basis(px(0.)).min_w_0().child(child),
    }
}

/// The Keys table (Keychain.dc.html): a header row, then rows separated by
/// lines; the selected row is `raised`, the others highlight on hover.
pub fn table(columns: &[TableColumn], rows: Vec<TableRow>) -> Div {
    let line = |d: Div| d.flex().items_center().gap(theme::SPACE_3).px(theme::SPACE_4);
    let header = line(div().py(px(10.)))
        .text_label()
        .text_color(theme::ink_muted())
        .border_b_1()
        .border_color(theme::line())
        .children(columns.iter().map(|c| cell(c.width, c.label)));
    let count = rows.len();
    let lines = rows.into_iter().enumerate().map(|(i, row)| {
        let mut cells = row.cells.into_iter();
        let mut d = line(div())
            .id(row.id)
            .py(px(14.))
            .when(i + 1 < count, |d| d.border_b_1().border_color(theme::line()))
            .cursor_pointer()
            .when(row.selected, |d| d.bg(theme::raised()))
            .when(!row.selected, |d| d.hover(|s| s.bg(theme::tab_hover())))
            .children(columns.iter().map(|c| cell(c.width, cells.next().unwrap_or_else(|| div().into_any_element()))));
        if let Some(f) = row.on_click {
            d = d.on_click(f);
        }
        d
    });
    div()
        .flex()
        .flex_col()
        .bg(theme::card())
        .border_1()
        .border_color(theme::line())
        .rounded(theme::RADIUS_LG)
        .overflow_hidden()
        .min_w(px(560.))
        .child(header)
        .children(lines)
}

/// A name over a mono line, for a table's first column.
pub fn name_cell(name: impl Into<SharedString>, detail: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(div().text_body().font_weight(FontWeight::MEDIUM).text_color(theme::ink()).truncate().child(name.into()))
        .child(div().text_mono_small().text_color(theme::ink_muted()).truncate().child(detail.into()))
        .into_any_element()
}

fn card_frame(id: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .flex_grow()
        .flex_basis(px(220.))
        .min_w(px(220.))
        .max_w(px(360.))
        .px(theme::SPACE_4)
        .py(px(14.))
        .rounded(theme::RADIUS_LG)
        .border_1()
        .cursor_pointer()
}

/// An identity card: name, how it signs in, and who and how many hosts.
pub fn identity_card(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    how: impl Into<SharedString>,
    meta: impl Into<SharedString>,
) -> Stateful<Div> {
    card_frame(id.into())
        .bg(theme::card())
        .border_color(theme::line())
        .hover(|s| s.bg(theme::tab_hover()))
        .flex()
        .flex_col()
        .gap(theme::SPACE_1)
        .child(div().text_body().font_weight(FontWeight::MEDIUM).text_color(theme::ink()).truncate().child(name.into()))
        .child(div().text_meta().text_color(theme::ink_muted()).truncate().child(how.into()))
        .child(div().text_meta().text_color(theme::ink_faint()).truncate().child(meta.into()))
}

/// The dashed card that adds one ("New identity").
pub fn add_card(id: impl Into<SharedString>, label: &'static str) -> Stateful<Div> {
    card_frame(id.into())
        .border_color(theme::line_strong())
        .border_dashed()
        .hover(|s| s.bg(theme::card()))
        .flex()
        .items_center()
        .gap(theme::SPACE_2)
        .text_body()
        .text_color(theme::ink_muted())
        .child(icon("plus", px(16.), theme::ink_muted()))
        .child(label)
}

/// A row of the multi-select list: a checkbox, a title and a detail line.
/// A disabled row is dimmed and its checkbox does nothing.
pub fn pick_row(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_start()
        .gap(theme::SPACE_3)
        .py(theme::SPACE_2)
        .child(Checkbox::new(id.into(), "", checked).on_toggle(move |v, window, cx| {
            if enabled {
                on_toggle(v, window, cx)
            }
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .when(!enabled, |d| d.opacity(0.6))
                .child(div().text_body().text_color(theme::ink()).child(title.into()))
                .child(div().text_meta().text_color(theme::ink_muted()).child(detail.into())),
        )
}

/// The right-hand side panel (Keychain.dc.html, Generate key): `panel`
/// with a left line and shadow, full height, a heading on top.
pub fn side_panel(width: f32, title: &'static str) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .gap(px(18.))
        .px(theme::SPACE_5)
        .py(px(28.))
        .bg(theme::panel())
        .border_l_1()
        .border_color(theme::line())
        .shadow(theme::shadow_panel())
        .child(div().text_heading().text_color(theme::ink()).child(title))
}
