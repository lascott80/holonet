//! Milestone 7's controls, shared by their pages and the gallery: History's
//! rows and saved output (History.dc.html), the command palette's rows
//! (Palette.dc.html), the new-release notice (Settings.dc.html) and First
//! run's cards.

use gpui::{Div, ElementId, FontWeight, IntoElement, Rgba, SharedString, Stateful, div, prelude::*, px};

use crate::theme::{self, TypeStyles};

// ------------------------------------------------------------- History --

/// History's columns (History.dc.html: minmax(0, 1.3fr) 150px 90px 100px
/// 80px).
fn history_columns(d: Div) -> Div {
    d.flex().items_center().gap(theme::SPACE_3).px(theme::SPACE_4)
}

fn history_cell(w: f32) -> Div {
    div().w(px(w)).flex_none().min_w_0().truncate()
}

pub fn history_header() -> Div {
    history_columns(div().py(px(10.)))
        .text_label()
        .text_color(theme::ink_muted())
        .border_b_1()
        .border_color(theme::line())
        .child(div().flex_1().min_w_0().child("HOST"))
        .child(history_cell(150.).child("STARTED"))
        .child(history_cell(90.).child("LENGTH"))
        .child(history_cell(100.).child("ROUTE"))
        .child(history_cell(80.).child("LOG"))
}

/// One session in History's table.
pub struct HistoryRow {
    pub selected: bool,
    /// The host's group color, or none.
    pub color: Option<Rgba>,
    pub host: SharedString,
    /// "Terminal" or "SFTP".
    pub kind: &'static str,
    pub started: SharedString,
    /// "42 min", or "Open".
    pub length: SharedString,
    pub route: SharedString,
    /// "1.2 MB", "No log" or "Writing".
    pub log: SharedString,
}

pub fn history_row(id: impl Into<ElementId>, r: HistoryRow) -> Stateful<Div> {
    history_columns(div())
        .id(id)
        .min_h(px(48.))
        .border_b_1()
        .border_color(theme::line())
        .cursor_pointer()
        .when(r.selected, |d| d.bg(theme::card_selected()))
        .when(!r.selected, |d| d.hover(|s| s.bg(theme::raised())))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(super::icon("role-server", px(16.), r.color.unwrap_or_else(theme::no_group)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_body()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(r.host),
                )
                .child(div().flex_none().text_meta().text_color(theme::ink_muted()).child(r.kind)),
        )
        .child(history_cell(150.).text_meta().text_color(theme::ink_muted()).child(r.started))
        .child(history_cell(90.).text_mono_small().text_color(theme::ink_muted()).child(r.length))
        .child(history_cell(100.).text_meta().text_color(theme::ink_muted()).child(r.route))
        .child(history_cell(80.).text_mono_small().text_color(theme::ink_muted()).child(r.log))
}

/// A record's saved output: `(line, note)` pairs, where Holonet's own
/// notes are dim and wrap, and saved lines keep their layout.
pub fn log_output(id: impl Into<ElementId>, lines: Vec<(SharedString, bool)>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(240.))
        .overflow_scroll()
        .bg(theme::terminal())
        .border_1()
        .border_color(theme::line())
        .rounded(theme::RADIUS_MD)
        .px(px(14.))
        .py(theme::SPACE_3)
        .text_mono_small()
        .children(lines.into_iter().map(|(t, note)| {
            div()
                .when(!note, |d| d.whitespace_nowrap())
                .text_color(if note { theme::ink_dim() } else { theme::ink_soft() })
                .child(t)
        }))
}

// ------------------------------------------------------------- Palette --

/// A key in a small box: "Enter", "Esc".
pub fn key_chip(text: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(2.))
        .border_1()
        .border_color(theme::line_strong())
        .rounded(px(4.))
        .text_mono_small()
        .text_color(theme::ink_muted())
        .child(text)
}

/// A section's label in the palette: "HOSTS", "SNIPPETS".
pub fn palette_section(label: &'static str) -> Div {
    div().px(theme::SPACE_4).pt(theme::SPACE_3).pb(px(6.)).text_label().text_color(theme::ink_muted()).child(label)
}

/// One palette entry; the selected one shows Enter.
pub fn palette_row(
    id: impl Into<ElementId>,
    selected: bool,
    color: Option<Rgba>,
    title: SharedString,
    hint: SharedString,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(44.))
        .px(theme::SPACE_4)
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::raised()))
        .when(!selected, |d| d.hover(|s| s.bg(theme::card_selected())))
        .children(color.map(|c| div().flex_none().size(px(8.)).rounded(px(2.)).bg(c)))
        .child(div().flex_1().min_w_0().truncate().text_body().text_color(theme::ink()).child(title))
        .child(div().flex_none().text_meta().text_color(theme::ink_muted()).child(hint))
        .when(selected, |d| d.child(key_chip("Enter")))
}

// ------------------------------------------------------------- Updates --

/// Settings' notice of a newer release, with its two buttons.
pub fn update_card(
    version: &str,
    current: &str,
    released: &str,
    view: impl IntoElement,
    later: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(theme::SPACE_4)
        .gap_y(theme::SPACE_3)
        .px(px(18.))
        .py(theme::SPACE_4)
        .bg(theme::card())
        .border_1()
        .border_color(theme::accent())
        .rounded(theme::RADIUS_XL)
        .child(super::icon("download", px(20.), theme::accent()))
        .child(
            div()
                .flex_1()
                .min_w(px(280.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_family(theme::DISPLAY)
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::ink())
                        .child(SharedString::from(format!("Holonet {version} is available"))),
                )
                .child(
                    div()
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child(SharedString::from(format!("You have {current}. Released on GitHub on {released}."))),
                ),
        )
        .child(view)
        .child(later)
}

// ----------------------------------------------------------- First run --

/// An import choice on First run: name, where it reads from, what it does.
pub fn import_card(
    id: impl Into<ElementId>,
    name: &'static str,
    place: &'static str,
    about: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(18.))
        .bg(theme::card())
        .border_1()
        .border_color(theme::line())
        .rounded(theme::RADIUS_XL)
        .cursor_pointer()
        .hover(|s| s.shadow(theme::shadow_hover()))
        .child(
            div()
                .font_family(theme::DISPLAY)
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::ink())
                .child(name),
        )
        .child(div().text_mono_small().text_color(theme::ink_muted()).child(place))
        .child(div().text_meta().text_color(theme::ink_muted()).child(about))
}

/// A next step on First run: an outlined card that fills on hover.
pub fn first_run_link(id: impl Into<ElementId>, title: &'static str, about: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .min_w_0()
        .flex()
        .flex_col()
        .gap(theme::SPACE_1)
        .px(px(18.))
        .py(px(16.))
        .border_1()
        .border_color(theme::line())
        .rounded(theme::RADIUS_XL)
        .cursor_pointer()
        .hover(|s| s.bg(theme::card()))
        .child(div().text_body().font_weight(FontWeight::MEDIUM).text_color(theme::ink()).child(title))
        .child(div().text_meta().text_color(theme::ink_muted()).child(about))
}
