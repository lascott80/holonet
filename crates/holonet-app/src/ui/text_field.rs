//! Single-line text field.
//!
//! Adapted from GPUI's `examples/input.rs` (Zed Industries, Apache-2.0):
//! the action set, UTF-16 conversions and `EntityInputHandler` (IME) follow it.
//! Holonet adds horizontal scrolling that keeps the caret in view, a password
//! mask with a show/hide toggle, the mono variant, placeholder and problem
//! styling from Field.md, Tab/Shift-Tab focus movement, and Enter/Escape
//! events for forms.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{self, TypeStyles};

actions!(
    text_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectToHome,
        SelectToEnd,
        Paste,
        Cut,
        Copy,
        Submit,
        Cancel,
        FocusNext,
        FocusPrev,
    ]
);

const CONTEXT: &str = "TextField";
const MASK: char = '\u{2022}';

/// Register the field's key bindings. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEnd, Some(CONTEXT)),
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("tab", FocusNext, Some(CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrev, Some(CONTEXT)),
    ]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextFieldEvent {
    Changed,
    /// Enter was pressed.
    Submitted,
    /// Escape was pressed.
    Cancelled,
}

pub struct TextField {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    mono: bool,
    password: bool,
    revealed: bool,
    problem: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_x: Pixels,
    is_selecting: bool,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextField {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            content: String::new(),
            placeholder: placeholder.into(),
            mono: false,
            password: false,
            revealed: false,
            problem: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_x: px(0.),
            is_selecting: false,
        }
    }

    /// Mono family, for addresses, ports, paths and commands (Field.md).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Mask the content, with a show/hide toggle.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.marked_range = None;
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    /// Show the field as having a problem (`warn` border).
    pub fn set_problem(&mut self, problem: bool, cx: &mut Context<Self>) {
        if self.problem != problem {
            self.problem = problem;
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn masked(&self) -> bool {
        self.password && !self.revealed
    }

    // ------------------------------------------------------------ actions --

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_to_home(&mut self, _: &SelectToHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_to_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Single line: line breaks become spaces, surrounding space is trimmed.
            let text = text.trim().replace(['\r', '\n'], " ");
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.password && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.password && !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextFieldEvent::Submitted);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextFieldEvent::Cancelled);
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, _: &mut Context<Self>) {
        window.focus_next();
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, _: &mut Context<Self>) {
        window.focus_prev();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.click_count >= 2 {
            self.select_all(&SelectAll, window, cx);
        } else if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    // ------------------------------------------------------------ helpers --

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    /// Text as drawn: the content, or one mask character per character.
    fn display_text(&self) -> String {
        if self.masked() { MASK.to_string().repeat(self.content.chars().count()) } else { self.content.clone() }
    }

    /// Content byte offset to display byte offset.
    fn content_to_display(&self, offset: usize) -> usize {
        if self.masked() { masked_offset(&self.content, offset) } else { offset }
    }

    /// Display byte offset to content byte offset.
    fn display_to_content(&self, offset: usize) -> usize {
        if self.masked() { unmasked_offset(&self.content, offset) } else { offset.min(self.content.len()) }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        self.display_to_content(line.closest_index_for_x(position.x - bounds.left() + self.scroll_x))
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(idx, _)| (idx < offset).then_some(idx)).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // Single line only.
        let new_text = new_text.replace(['\r', '\n'], "");
        self.content = self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.marked_range = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let x = |offset| bounds.left() + last_layout.x_for_index(self.content_to_display(offset)) - self.scroll_x;
        Some(Bounds::from_corners(point(x(range.start), bounds.top()), point(x(range.end), bounds.bottom())))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let display_index = last_layout.index_for_x(line_point.x + self.scroll_x)?;
        Some(self.offset_to_utf16(self.display_to_content(display_index)))
    }
}

struct TextElement {
    field: Entity<TextField>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Grow to fill the field as a flex item. A percentage width collapses
        // when the parent's width comes from flex layout (measured 18px).
        let mut style = Style {
            flex_grow: 1.,
            flex_shrink: 1.,
            flex_basis: gpui::Length::Definite(px(0.).into()),
            ..Default::default()
        };
        style.min_size.width = px(0.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let field = self.field.read(cx);
        let style = window.text_style();
        let empty = field.content.is_empty();
        let display: SharedString = if empty { field.placeholder.clone() } else { field.display_text().into() };
        let color = if empty { theme::ink_dim().into() } else { style.color };

        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match field.marked_range.as_ref().filter(|_| !empty && !field.masked()) {
            Some(marked) => vec![
                TextRun { len: marked.start, ..run.clone() },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: display.len() - marked.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display, font_size, &runs, None);

        // Keep the caret in view.
        let cursor_x = if empty { px(0.) } else { line.x_for_index(field.content_to_display(field.cursor_offset())) };
        let width = bounds.size.width;
        let mut scroll_x = field.scroll_x;
        if cursor_x - scroll_x > width - px(2.) {
            scroll_x = cursor_x - width + px(2.);
        }
        if cursor_x < scroll_x {
            scroll_x = cursor_x;
        }
        scroll_x = scroll_x.min((line.width - width + px(2.)).max(px(0.))).max(px(0.));

        let left = bounds.left() - scroll_x;
        let (selection, cursor) = if field.selected_range.is_empty() || empty {
            let caret = fill(
                Bounds::new(point(left + cursor_x, bounds.top()), size(px(1.5), bounds.bottom() - bounds.top())),
                theme::ink(),
            );
            (None, Some(caret))
        } else {
            let start = line.x_for_index(field.content_to_display(field.selected_range.start));
            let end = line.x_for_index(field.content_to_display(field.selected_range.end));
            let quad = fill(
                Bounds::from_corners(point(left + start, bounds.top()), point(left + end, bounds.bottom())),
                theme::accent_alpha(0.3),
            );
            (Some(quad), None)
        };
        PrepaintState { line: Some(line), cursor, selection, scroll_x }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        let scroll_x = prepaint.scroll_x;
        let line = prepaint.line.take().unwrap();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            let _ = line.paint(point(bounds.left() - scroll_x, bounds.top()), window.line_height(), window, cx);
            if focus.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.field.update(cx, |field, _cx| {
            field.last_layout = Some(line);
            field.last_bounds = Some(bounds);
            field.scroll_x = scroll_x;
        });
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let border = if focused {
            theme::accent()
        } else if self.problem {
            theme::warn()
        } else {
            theme::line()
        };
        let revealed = self.revealed;
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_to_home))
            .on_action(cx.listener(Self::select_to_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .h(theme::CONTROL)
            .w_full()
            .min_w(px(80.))
            .pl(theme::SPACE_3)
            .pr(if self.password { theme::SPACE_1 } else { theme::SPACE_3 })
            .flex()
            .items_center()
            .gap(theme::SPACE_2)
            .bg(theme::card())
            .border_1()
            .border_color(border)
            .rounded(theme::RADIUS_MD)
            .when(self.mono, |d| d.text_mono())
            .when(!self.mono, |d| d.text_body())
            .text_color(theme::ink())
            .child(div().flex().flex_1().min_w_0().overflow_hidden().child(TextElement { field: cx.entity() }))
            .when(self.password, |d| {
                d.child(
                    div()
                        .id("reveal")
                        .size(px(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme::RADIUS_SM)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::raised()))
                        .child(crate::ui::icon(if revealed { "eye-off" } else { "eye" }, px(16.), theme::ink_muted()))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|field, _, _, cx| {
                            field.revealed = !field.revealed;
                            cx.notify();
                        })),
                )
            })
    }
}

/// Byte offset in `content` to byte offset in its masked form.
fn masked_offset(content: &str, offset: usize) -> usize {
    content[..offset].chars().count() * MASK.len_utf8()
}

/// Byte offset in the masked form back to a byte offset in `content`.
fn unmasked_offset(content: &str, offset: usize) -> usize {
    let chars = offset / MASK.len_utf8();
    content.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(content.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_mask_maps_offsets_by_character() {
        let content = "pä55";
        // 'ä' is two bytes in the content, one mask char (3 bytes) on screen.
        assert_eq!(masked_offset(content, 3), 2 * MASK.len_utf8());
        assert_eq!(unmasked_offset(content, 2 * MASK.len_utf8()), 3);
        assert_eq!(unmasked_offset(content, 99), content.len());
        assert_eq!(masked_offset(content, content.len()), 4 * MASK.len_utf8());
    }
}
