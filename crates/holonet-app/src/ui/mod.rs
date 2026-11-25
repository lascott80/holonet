//! Shared controls, built from the design tokens and the component guidelines
//! in docs/design/components. Stateless controls live here; the text field and
//! select keep state and have their own modules.

pub mod select;
pub mod slider;
pub mod text_field;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement, ParentElement, Pixels, Rgba,
    SharedString, Stateful, Styled, Svg, Window, div, prelude::*, px, svg,
};

use crate::assets;
use crate::theme::{self, TypeStyles};

pub use select::{Select, SelectEvent};
pub use slider::{Slider, SliderEvent};
pub use text_field::{TextField, TextFieldEvent};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
/// Called with a new checked state.
pub type ToggleHandler = Box<dyn Fn(bool, &mut Window, &mut App) + 'static>;
/// Called with the index of the chosen item.
pub type IndexHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// A stroke icon from assets/icons, tinted with `color`.
pub fn icon(name: &str, size: Pixels, color: impl Into<Hsla>) -> Svg {
    svg().path(assets::icon(name)).size(size).flex_none().text_color(color.into())
}

// ---------------------------------------------------------------- Button --

/// The five button kinds from Button.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// `accent` fill. One per view.
    Primary,
    /// `line-strong` outline. Any other action.
    Secondary,
    /// No border, `ink-muted`. Cancel and dismiss.
    Quiet,
    /// `warn-line` outline, `warn` text. Entry point to a destructive action.
    DestructiveOutline,
    /// `warn` fill. Only as the confirming button in a warning dialog.
    DestructiveFill,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    kind: ButtonKind,
    icon: Option<&'static str>,
    compact: bool,
    header: bool,
    full_width: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            icon: None,
            compact: false,
            header: false,
            full_width: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn primary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, label, ButtonKind::Primary)
    }

    pub fn secondary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, label, ButtonKind::Secondary)
    }

    pub fn quiet(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self::new(id, label, ButtonKind::Quiet)
    }

    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }

    /// `control-compact` height, for toolbars and pane headers.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// `radius-lg`, for the primary action in a page header.
    pub fn header(mut self) -> Self {
        self.header = true;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (bg, fg, border): (Option<Rgba>, Rgba, Option<Rgba>) = match self.kind {
            ButtonKind::Primary => (Some(theme::accent()), theme::on_accent(), None),
            ButtonKind::Secondary => (None, theme::ink(), Some(theme::line_strong())),
            ButtonKind::Quiet => (None, theme::ink_muted(), None),
            ButtonKind::DestructiveOutline => (None, theme::warn(), Some(theme::warn_line())),
            ButtonKind::DestructiveFill => (Some(theme::warn()), theme::on_warn(), None),
        };
        let filled = bg.is_some();
        let kind = self.kind;
        let weight = if filled { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM };
        let mut button = div()
            .id(self.id)
            .h(if self.compact { theme::CONTROL_COMPACT } else { theme::CONTROL })
            .px(if self.compact { theme::SPACE_3 } else { theme::SPACE_4 })
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(theme::SPACE_2)
            .rounded(if self.header { theme::RADIUS_LG } else { theme::RADIUS_MD })
            .border_1()
            .border_color(border.unwrap_or(Rgba { a: 0., ..theme::ink() }))
            .text_body()
            .font_weight(weight)
            .text_color(fg)
            .when_some(bg, |b, bg| b.bg(bg))
            .when(self.full_width, |b| b.w_full())
            .when_some(self.icon, |b, name| b.child(icon(name, px(16.), fg)))
            .child(self.label);
        if self.disabled {
            button = button.opacity(0.45).cursor_not_allowed();
        } else {
            button = button.cursor_pointer().hover(move |s| match kind {
                ButtonKind::Primary | ButtonKind::DestructiveFill => s.opacity(0.9),
                ButtonKind::Secondary => s.bg(theme::raised()),
                ButtonKind::Quiet => s.text_color(theme::ink()).bg(theme::card()),
                ButtonKind::DestructiveOutline => s.bg(theme::warn_surface()),
            });
            if let Some(handler) = self.on_click {
                button = button.on_click(handler);
            }
        }
        button
    }
}

// -------------------------------------------------------------- Checkbox --

/// Checkbox with its label to the right; the row is at least `control` high.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: SharedString,
    checked: bool,
    on_toggle: Option<ToggleHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, checked: bool) -> Self {
        Self { id: id.into(), label: label.into(), checked, on_toggle: None }
    }

    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let checked = self.checked;
        let box_ = div()
            .size(px(18.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_XS)
            .border_1()
            .when(checked, |b| b.bg(theme::accent()).border_color(theme::accent()))
            .when(!checked, |b| b.bg(theme::card()).border_color(theme::line_strong()))
            .when(checked, |b| b.child(icon("check", px(14.), theme::on_accent())));
        let mut row = div()
            .id(self.id)
            .min_h(theme::CONTROL)
            .flex()
            .items_center()
            .gap(theme::SPACE_3)
            .cursor_pointer()
            .text_body()
            .text_color(theme::ink())
            .child(box_)
            .child(self.label);
        if let Some(handler) = self.on_toggle {
            row = row.on_click(move |_, window, cx| handler(!checked, window, cx));
        }
        row
    }
}

// ------------------------------------------------------------- Segmented --

/// Segmented control: equal options in a row; the chosen one is `raised`
/// with an `accent` border (Main and NewHost screens).
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    compact: bool,
    on_change: Option<IndexHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>, options: Vec<SharedString>, selected: usize) -> Self {
        Self { id: id.into(), options, selected, compact: false, on_change: None }
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let height = if self.compact { theme::CONTROL_COMPACT } else { theme::CONTROL };
        let id = self.id.clone();
        div().id(self.id).flex().gap(theme::SPACE_2).children(self.options.into_iter().enumerate().map(|(i, label)| {
            let on = i == self.selected;
            let handler = self.on_change.clone();
            div()
                .id(ElementId::NamedInteger(SharedString::from(format!("{id:?}")), i as u64))
                .flex_1()
                .h(height)
                .px(theme::SPACE_3)
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_MD)
                .border_1()
                .text_body()
                .when(self.compact, |d| d.text_size(px(13.)))
                .when(on, |d| d.bg(theme::raised()).border_color(theme::accent()).text_color(theme::ink()))
                .when(!on, |d| {
                    d.border_color(theme::line())
                        .text_color(theme::ink_muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::ink()).bg(theme::card()))
                })
                .child(label)
                .when_some(handler.filter(|_| !on), |d, handler| {
                    d.on_click(move |_, window, cx| handler(i, window, cx))
                })
        }))
    }
}

// ------------------------------------------------------------------- Tag --

/// Tags from Tag.md. Never buttons.
pub enum Tag {
    /// Group name in its color, outlined, uppercase 11px.
    Group(SharedString, Rgba),
    /// `ink-soft` text, `line-strong` outline ("Tailscale").
    Neutral(SharedString),
    /// Keyboard hint: mono, `ink-muted`, `line-strong` outline.
    Key(SharedString),
}

impl IntoElement for Tag {
    type Element = Div;

    fn into_element(self) -> Self::Element {
        let base = div().flex_none().px(px(6.)).py(px(1.)).rounded(theme::RADIUS_XS).border_1();
        match self {
            Tag::Group(name, color) => {
                base.border_color(color).text_color(color).text_label().child(SharedString::from(name.to_uppercase()))
            }
            Tag::Neutral(text) => base
                .border_color(theme::line_strong())
                .text_color(theme::ink_soft())
                .text_label()
                .child(SharedString::from(text.to_uppercase())),
            Tag::Key(text) => base
                .border_color(theme::line_strong())
                .text_color(theme::ink_muted())
                .font_family(theme::mono())
                .text_size(px(11.))
                .child(text),
        }
    }
}

/// 8px group square before a host name.
pub fn group_square(color: Rgba) -> Div {
    div().size(px(8.)).flex_none().rounded(px(2.)).bg(color)
}

// --------------------------------------------------- Layout and display --

/// Field label above a control (`meta`, `ink-muted`).
pub fn field_label(text: impl Into<SharedString>) -> Div {
    div().text_meta().text_color(theme::ink_muted()).child(text.into())
}

/// Help text below a control; `warn` when it reports a problem.
pub fn field_help(text: impl Into<SharedString>, problem: bool) -> Div {
    div().w_full().text_meta().text_color(if problem { theme::warn() } else { theme::ink_muted() }).child(text.into())
}

/// A labelled field: label, control, optional help line.
pub fn field(label: impl Into<SharedString>, control: impl IntoElement, help: Option<(SharedString, bool)>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::SPACE_2)
        // Full width in a column (so help text wraps at the field's width);
        // in a row, flex_1 sets the size instead.
        .w_full()
        .min_w(px(180.))
        .flex_1()
        .child(field_label(label))
        .child(control)
        .when_some(help, |d, (text, problem)| d.child(field_help(text, problem)))
}

/// Section label ("ALL HOSTS"): `label` style, `ink-muted`.
pub fn section_label(text: &str) -> Div {
    div().text_label().text_color(theme::ink_muted()).child(SharedString::from(text.to_uppercase()))
}

/// Fieldset with a legend, for form sections.
pub fn fieldset(legend: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::SPACE_4)
        .p(theme::SPACE_4)
        .rounded(theme::RADIUS_LG)
        .border_1()
        .border_color(theme::line())
        .child(
            div()
                .font_family(theme::DISPLAY)
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::ink())
                .child(legend.into()),
        )
}

/// Key-value table on `card`, values in mono.
pub fn key_values(rows: Vec<(SharedString, AnyElement)>) -> Div {
    let count = rows.len();
    div().flex().flex_col().bg(theme::card()).rounded(theme::RADIUS_LG).children(rows.into_iter().enumerate().map(
        |(i, (key, value))| {
            div()
                .flex()
                .justify_between()
                .items_center()
                .gap(theme::SPACE_3)
                .px(px(14.))
                .py(theme::SPACE_3)
                .when(i + 1 < count, |d| d.border_b_1().border_color(theme::line_soft()))
                .child(div().flex_none().text_meta().text_color(theme::ink_muted()).child(key))
                .child(div().min_w_0().truncate().text_mono().text_color(theme::ink()).child(value))
        },
    ))
}

/// Settings card: rows inside `card` with `radius-lg`, divided by `line`
/// (SettingRow.md).
pub fn setting_card(rows: Vec<AnyElement>) -> Div {
    let count = rows.len();
    div().flex().flex_col().bg(theme::card()).rounded(theme::RADIUS_LG).children(
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| div().when(i + 1 < count, |d| d.border_b_1().border_color(theme::line())).child(row)),
    )
}

/// One setting: name (and optional consequence line) on the left, control
/// on the right; wraps on a narrow window.
pub fn setting_row(name: impl Into<SharedString>, help: Option<SharedString>, control: impl IntoElement) -> Div {
    setting_row_with(
        name,
        help.map(|h| div().text_meta().text_color(theme::ink_muted()).child(h).into_any_element()),
        control,
    )
}

/// [`setting_row`] with any element as the second line (e.g. a path).
pub fn setting_row_with(name: impl Into<SharedString>, help: Option<AnyElement>, control: impl IntoElement) -> Div {
    div()
        .min_h(theme::CONTROL)
        .px(theme::SPACE_4)
        .py(theme::SPACE_2)
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(theme::SPACE_3)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(220.))
                .overflow_hidden()
                .child(div().text_body().text_color(theme::ink()).child(name.into()))
                .children(help),
        )
        .child(control)
}

/// Code box: `sunken`, mono, for commands and fingerprints.
pub fn code_box(text: impl Into<SharedString>) -> Div {
    div()
        .bg(theme::sunken())
        .rounded(theme::RADIUS_MD)
        .p(theme::SPACE_3)
        .text_mono_small()
        .text_color(theme::ink_soft())
        .child(text.into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CalloutKind {
    Info,
    Success,
    Warning,
}

/// Information, result or warning box.
pub fn callout(kind: CalloutKind) -> Div {
    let base = div().flex().flex_col().gap(theme::SPACE_1).p(px(14.)).rounded(theme::RADIUS_MD).border_1().text_body();
    match kind {
        CalloutKind::Info => base.bg(theme::card()).border_color(theme::line()).text_color(theme::ink_soft()),
        CalloutKind::Success => base.bg(theme::card()).border_color(theme::accent()).text_color(theme::ink_soft()),
        CalloutKind::Warning => {
            base.bg(theme::warn_surface()).border_color(theme::warn()).text_color(theme::warn_ink())
        }
    }
}

// ---------------------------------------------------------------- Dialog --

/// Dialog card from Dialog.md: standard or warning. Place it with
/// [`modal`] to get the backdrop.
pub fn dialog(warning: bool, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::SPACE_4)
        .w_full()
        .max_w(px(460.))
        .p(theme::SPACE_5)
        .rounded(theme::RADIUS_2XL)
        .border_1()
        .shadow(theme::shadow_dialog())
        .when(warning, |d| d.bg(theme::warn_surface()).border_color(theme::warn()))
        .when(!warning, |d| d.bg(theme::card()).border_color(theme::line_strong()))
        .child(div().text_heading().text_color(theme::ink()).child(title.into()))
        .child(
            div()
                .text_body()
                .text_color(if warning { theme::warn_ink() } else { theme::ink_muted() })
                .child(body.into()),
        )
}

/// Buttons row at the bottom of a dialog, right-aligned.
pub fn dialog_buttons(buttons: impl IntoIterator<Item = Button>) -> Div {
    div().flex().justify_end().gap(theme::SPACE_2).pt(theme::SPACE_2).children(buttons)
}

/// Full-window 60% black backdrop with `content` centred, at most
/// `max_width` wide. Clicking the backdrop calls `on_dismiss` (Dialog.md:
/// the backdrop cancels); clicks inside the content do not.
pub fn modal(
    id: impl Into<ElementId>,
    max_width: Pixels,
    content: impl IntoElement,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(theme::SPACE_6)
        .bg(gpui::hsla(0., 0., 0., 0.6))
        .occlude()
        .on_click(move |_, window, cx| on_dismiss(window, cx))
        // Clicks inside the card must not reach the backdrop.
        .child(
            div()
                .id("modal-content")
                .w_full()
                .max_w(max_width)
                .flex()
                .justify_center()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
}
