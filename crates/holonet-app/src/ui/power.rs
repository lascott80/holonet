//! Milestone 6's controls, shared by the pages that use them and
//! `--gallery`: the group colour swatch (Groups.dc.html), the rule type tag
//! and rule card (PortForwarding.dc.html), the host chip of "Run on"
//! (Snippets.dc.html) and the amber broadcast strip (Terminal.dc.html).
//! The on/off switch is `ui::switch`. They draw the state they are given;
//! the caller keeps it and adds the clicks.

use gpui::{App, Div, ElementId, Rgba, SharedString, Stateful, Window, div, prelude::*, px};
use holonet_model::ForwardKind;

use super::{Button, ButtonKind, Tag, group_square, icon, switch};
use crate::theme::{self, TypeStyles};

/// A 44px colour swatch; the chosen one has an `ink` ring.
pub fn swatch(id: impl Into<ElementId>, color: Rgba, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(theme::CONTROL)
        .rounded(theme::RADIUS_LG)
        .bg(theme::card())
        .border_2()
        .border_color(if on { theme::ink() } else { theme::line() })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(div().size(px(22.)).rounded(px(6.)).bg(color))
}

/// A rule type's colour: Local cyan, Remote violet, Dynamic mint.
pub fn kind_color(kind: ForwardKind) -> Rgba {
    match kind {
        ForwardKind::Local => theme::accent(),
        ForwardKind::Remote => theme::group_violet(),
        ForwardKind::Dynamic => theme::group_mint(),
    }
}

/// "LOCAL", "REMOTE" or "DYNAMIC" in its colour.
pub fn kind_tag(kind: ForwardKind) -> Tag {
    Tag::Group(kind.label().into(), kind_color(kind))
}

/// What a rule card shows.
pub struct RuleCard {
    pub kind: ForwardKind,
    pub name: SharedString,
    /// "Starts with prod-web-01" or "Manual start".
    pub note: SharedString,
    /// Where connections enter, the host, where they go.
    pub route: [SharedString; 3],
    pub status: SharedString,
    pub status_color: Rgba,
    /// A line under the status ("Last failed connection: …").
    pub detail: Option<SharedString>,
    pub on: bool,
    pub selected: bool,
}

/// A rule card with its switch. The caller adds the card's own click.
pub fn rule_card(
    id: impl Into<ElementId>,
    switch_id: impl Into<ElementId>,
    card: RuleCard,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let arrow = || div().text_color(theme::ink_faint()).child("→");
    let [a, b, c] = card.route;
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(theme::SPACE_3)
        .p(theme::SPACE_4)
        .rounded(theme::RADIUS_XL)
        .border_1()
        .cursor_pointer()
        .bg(if card.selected { theme::card_selected() } else { theme::card() })
        .border_color(if card.selected {
            theme::accent()
        } else if card.on {
            theme::line_strong()
        } else {
            theme::line()
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme::SPACE_3)
                .child(kind_tag(card.kind))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_body_strong().text_color(theme::ink()).truncate().child(card.name))
                        .child(div().text_meta().text_color(theme::ink_muted()).child(card.note)),
                )
                .child(switch(switch_id, card.on, on_toggle)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme::SPACE_2)
                .text_mono_small()
                .text_color(theme::ink_soft())
                .child(a)
                .child(arrow())
                .child(b)
                .child(arrow())
                .child(c),
        )
        .child(div().text_meta().text_color(card.status_color).child(card.status))
        .children(card.detail.map(|d| div().text_meta().text_color(theme::ink_muted()).child(d)))
}

/// A "Run on" host chip: its group colour and name; chosen ones are
/// raised with an accent border.
pub fn host_chip(id: impl Into<ElementId>, name: impl Into<SharedString>, color: Rgba, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(theme::CONTROL)
        .px(theme::SPACE_3)
        .flex()
        .items_center()
        .gap(theme::SPACE_2)
        .rounded(px(22.))
        .border_1()
        .border_color(if on { theme::accent() } else { theme::line() })
        .bg(if on { theme::raised() } else { theme::card() })
        .cursor_pointer()
        .text_body()
        .text_color(theme::ink())
        .child(group_square(color))
        .child(name.into())
}

/// The amber strip under a broadcasting tab's header: "Broadcast is on."
/// and what it reaches, with Turn off.
pub fn broadcast_strip(
    reach: impl Into<SharedString>,
    on_turn_off: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme::SPACE_3)
        .px(px(18.))
        .py(theme::SPACE_2)
        .bg(theme::warn_surface())
        .border_b_1()
        .border_color(theme::warn())
        .child(icon("warning", px(16.), theme::warn()))
        .child(
            div().flex_1().min_w(px(240.)).text_body().text_color(theme::warn_ink()).child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(4.))
                    .child(
                        div()
                            .text_color(theme::warn())
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Broadcast is on."),
                    )
                    .child(reach.into()),
            ),
        )
        .child(Button::new("broadcast-off", "Turn off", ButtonKind::DestructiveFill).compact().on_click(on_turn_off))
}
