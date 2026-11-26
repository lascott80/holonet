//! The SFTP screen's controls (Sftp.dc.html), shared by the SFTP tab and
//! `--gallery`: the checkbox square, icon buttons, breadcrumbs, the file
//! list's header and rows, the selection bar, the transfer progress bar and
//! the permissions grid. They draw state they are given; the caller keeps it.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, ParentElement, Rgba, SharedString, Stateful, Styled, Window, div,
    prelude::*, px, relative,
};

use super::{IndexHandler, icon};
use crate::theme::{self, TypeStyles};

/// Column widths (Sftp.dc.html's grid).
pub const CHECK_W: f32 = 28.;
const LOCAL_SIZE_W: f32 = 90.;
const LOCAL_MODIFIED_W: f32 = 140.;
const REMOTE_SIZE_W: f32 = 80.;
const REMOTE_MODIFIED_W: f32 = 110.;
const PERMISSIONS_W: f32 = 100.;
const COLUMN_GAP: f32 = 10.;
const ROW_PAD: f32 = 14.;

/// A 16px checkbox square (the row and header boxes).
pub fn check_box(checked: bool) -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_XS)
        .border_1()
        .when(checked, |b| b.bg(theme::accent()).border_color(theme::accent()))
        .when(!checked, |b| b.bg(theme::card()).border_color(theme::line_strong()))
        .when(checked, |b| b.child(icon("check", px(12.), theme::on_accent())))
}

/// A 36px icon button: a pane's Back, Up, Refresh and New folder, and the
/// small close buttons. Disabled, it draws faint and takes no clicks.
pub fn nav_button(
    id: SharedString,
    name: &'static str,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(theme::CONTROL_COMPACT)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .child(icon(name, px(16.), if enabled { theme::ink_muted() } else { theme::ink_faint() }))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(theme::raised())).on_click(handler))
}

/// Breadcrumbs: each crumb opens its folder, the last is the current one.
/// `id` prefixes the crumbs' ids.
pub fn breadcrumbs(id: &str, crumbs: Vec<SharedString>, on_open: IndexHandler) -> Stateful<Div> {
    let last = crumbs.len().saturating_sub(1);
    div()
        .id(SharedString::from(format!("{id}-crumbs")))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .overflow_x_scroll()
        .text_mono_small()
        .children(crumbs.into_iter().enumerate().map(|(i, label)| {
            let current = i == last;
            let on_open = on_open.clone();
            div()
                .flex()
                .flex_none()
                .items_center()
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-crumb-{i}")))
                        .px(px(6.))
                        .py(theme::SPACE_2)
                        .rounded(theme::RADIUS_SM)
                        .whitespace_nowrap()
                        .text_color(if current { theme::ink() } else { theme::ink_muted() })
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::raised()))
                        .child(label)
                        .on_click(move |_, window, cx| on_open(i, window, cx)),
                )
                .when(!current, |d| d.child(div().text_color(theme::ink_faint()).child("›")))
        }))
}

/// The list's column header. The host's side (`remote`) has narrower Size
/// and Modified columns and a Permissions column. `select_all` is the box
/// that selects every row, left out where rows cannot be selected.
pub fn file_header(remote: bool, select_all: Option<AnyElement>) -> Div {
    let label = |text: &'static str| div().text_label().text_color(theme::ink_muted()).child(text);
    div()
        .flex_none()
        .px(px(ROW_PAD))
        .py(theme::SPACE_2)
        .flex()
        .items_center()
        .gap(px(COLUMN_GAP))
        .border_b_1()
        .border_color(theme::line())
        .child(div().w(px(CHECK_W)).flex_none().children(select_all))
        .child(label("NAME").flex_1().min_w_0())
        .child(label("SIZE").w(px(if remote { REMOTE_SIZE_W } else { LOCAL_SIZE_W })).flex_none())
        .child(label("MODIFIED").w(px(if remote { REMOTE_MODIFIED_W } else { LOCAL_MODIFIED_W })).flex_none())
        .when(remote, |d| d.child(label("PERMISSIONS").w(px(PERMISSIONS_W)).flex_none()))
}

/// What one list row shows.
pub struct FileRow {
    pub glyph: &'static str,
    pub color: Rgba,
    pub name: SharedString,
    /// Empty for folders.
    pub size: SharedString,
    pub modified: SharedString,
    /// `rwxr-xr-x`, on the host's side only.
    pub permissions: Option<SharedString>,
    pub selected: bool,
    /// False at the drives level, where rows only open.
    pub checkable: bool,
}

/// One row of the file list, the height of a control. The caller adds the
/// click, drag and drop handlers.
pub fn file_row(id: impl Into<ElementId>, row: FileRow) -> Stateful<Div> {
    let remote = row.permissions.is_some();
    let selected = row.selected;
    div()
        .id(id.into())
        .w_full()
        .h(theme::CONTROL)
        .px(px(ROW_PAD))
        .flex()
        .items_center()
        .gap(px(COLUMN_GAP))
        .border_b_1()
        .border_color(theme::line_soft())
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::card_selected()))
        .when(!selected, |d| d.hover(|s| s.bg(theme::card())))
        .child(div().w(px(CHECK_W)).flex_none().when(row.checkable, |d| d.child(check_box(selected))))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(COLUMN_GAP))
                .child(icon(row.glyph, px(18.), row.color))
                .child(div().flex_1().min_w_0().truncate().text_body().text_color(theme::ink()).child(row.name)),
        )
        .child(
            div()
                .w(px(if remote { REMOTE_SIZE_W } else { LOCAL_SIZE_W }))
                .flex_none()
                .text_mono_small()
                .text_color(theme::ink_muted())
                .child(row.size),
        )
        .child(
            div()
                .w(px(if remote { REMOTE_MODIFIED_W } else { LOCAL_MODIFIED_W }))
                .flex_none()
                .text_meta()
                .text_color(theme::ink_muted())
                .child(row.modified),
        )
        .children(
            row.permissions.map(|p| {
                div().w(px(PERMISSIONS_W)).flex_none().text_mono_small().text_color(theme::ink_muted()).child(p)
            }),
        )
}

/// The bar over the list while rows are selected: "n items selected", then
/// the actions the caller passes (Upload or Download, Rename, …, clear).
pub fn selection_bar(count: usize, actions: Vec<AnyElement>) -> Div {
    let text = if count == 1 { "1 item selected".to_string() } else { format!("{count} items selected") };
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(theme::SPACE_2)
        .bg(theme::card())
        .border_b_1()
        .border_color(theme::line())
        .child(div().px(px(6.)).text_body_strong().text_color(theme::ink()).child(text))
        .child(div().flex_1())
        .children(actions)
}

/// A transfer's 4px progress bar, filled to `percent` in `color` (accent
/// for uploads, violet for downloads, faint when paused or waiting).
pub fn progress_bar(percent: u8, color: Rgba) -> Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded(px(2.))
        .bg(theme::line())
        .child(div().h(px(4.)).w(relative(percent.min(100) as f32 / 100.)).rounded(px(2.)).bg(color))
}

/// The permissions grid: Read, Write and Run for owner, group and everyone
/// else, with the mode as a number and as `rwxr-x---` under it. `bits` runs
/// owner rwx, group rwx, other rwx; `on_toggle` gets a box's index.
pub fn permissions_grid(bits: [bool; 9], on_toggle: IndexHandler) -> Div {
    let digit = |k: usize| bits[k] as u32 * 4 + bits[k + 1] as u32 * 2 + bits[k + 2] as u32;
    let octal = format!("{}{}{}", digit(0), digit(3), digit(6));
    let symbolic: String =
        bits.iter().enumerate().map(|(i, on)| if *on { b"rwx"[i % 3] as char } else { '-' }).collect();
    let header = |text: &'static str| {
        div().flex_1().flex().justify_center().text_label().text_color(theme::ink_muted()).child(text)
    };
    let rows = ["Owner", "Group", "Everyone else"].into_iter().enumerate().map(|(r, who)| {
        div()
            .w_full()
            .flex()
            .items_center()
            .border_t_1()
            .border_color(theme::line())
            .child(div().flex_none().w(relative(0.3)).text_body().text_color(theme::ink()).child(who))
            .children((0..3).map(|c| {
                let i = r * 3 + c;
                let on_toggle = on_toggle.clone();
                div()
                    .id(("perm-bit", i))
                    .flex_1()
                    .h(theme::CONTROL)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(check_box(bits[i]))
                    .on_click(move |_, window, cx| on_toggle(i, window, cx))
            }))
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(theme::SPACE_4)
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .pb(px(6.))
                        .child(div().flex_none().w(relative(0.3)))
                        .child(header("READ"))
                        .child(header("WRITE"))
                        .child(header("RUN")),
                )
                .children(rows),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(20.))
                .px(px(14.))
                .py(theme::SPACE_3)
                .rounded(theme::RADIUS_MD)
                .border_1()
                .border_color(theme::line())
                .bg(theme::ground())
                .font_family(theme::mono())
                .child(div().text_size(px(20.)).text_color(theme::ink()).child(octal))
                .child(div().text_size(px(14.)).text_color(theme::ink_muted()).child(symbolic)),
        )
}
