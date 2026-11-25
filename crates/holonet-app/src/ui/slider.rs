//! Slider: a value on a track, set by clicking or dragging, or with the arrow
//! keys when focused (Settings: font size and line height). The value shows
//! beside the track in mono, as Settings.dc.html draws it.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Render, SharedString, Window, actions, canvas, div, prelude::*, px,
};

use crate::theme::{self, TypeStyles};

actions!(slider, [Increase, Decrease, ToMin, ToMax]);

const CONTEXT: &str = "Slider";
const TRACK_WIDTH: Pixels = px(180.);
const THUMB: Pixels = px(16.);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", Increase, Some(CONTEXT)),
        KeyBinding::new("up", Increase, Some(CONTEXT)),
        KeyBinding::new("left", Decrease, Some(CONTEXT)),
        KeyBinding::new("down", Decrease, Some(CONTEXT)),
        KeyBinding::new("home", ToMin, Some(CONTEXT)),
        KeyBinding::new("end", ToMax, Some(CONTEXT)),
    ]);
}

/// The value changed (on every step while dragging).
pub enum SliderEvent {
    Changed(i32),
}

/// Integer steps from `min` to `max`; `format` turns a step into the label
/// (e.g. 16 shows as "1.6").
pub struct Slider {
    min: i32,
    max: i32,
    value: i32,
    format: fn(i32) -> String,
    label: SharedString,
    focus: FocusHandle,
    dragging: bool,
    /// Track bounds from the last paint, for turning a pointer into a value.
    track: Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<SliderEvent> for Slider {}

impl Slider {
    pub fn new(cx: &mut Context<Self>, min: i32, max: i32, value: i32, format: fn(i32) -> String) -> Self {
        Self {
            min,
            max,
            value: value.clamp(min, max),
            format,
            label: format(value.clamp(min, max)).into(),
            focus: cx.focus_handle().tab_stop(true),
            dragging: false,
            track: Rc::new(Cell::new(Bounds::default())),
        }
    }

    pub fn value(&self) -> i32 {
        self.value
    }

    pub fn set_value(&mut self, value: i32, cx: &mut Context<Self>) {
        let value = value.clamp(self.min, self.max);
        if value != self.value {
            self.value = value;
            self.label = (self.format)(value).into();
            cx.emit(SliderEvent::Changed(value));
            cx.notify();
        }
    }

    fn value_at(&self, x: Pixels) -> i32 {
        let b = self.track.get();
        let width = f32::from(b.size.width).max(1.0);
        let t = (f32::from(x - b.origin.x) / width).clamp(0.0, 1.0);
        self.min + (t * (self.max - self.min) as f32).round() as i32
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.dragging = true;
        self.set_value(self.value_at(event.position.x), cx);
    }

    fn on_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.dragging {
            if event.pressed_button == Some(MouseButton::Left) {
                self.set_value(self.value_at(event.position.x), cx);
            } else {
                self.dragging = false;
            }
        }
    }
}

impl Render for Slider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let t = (self.value - self.min) as f32 / (self.max - self.min).max(1) as f32;
        let track = self.track.clone();
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|s, _: &Increase, _, cx| s.set_value(s.value + 1, cx)))
            .on_action(cx.listener(|s, _: &Decrease, _, cx| s.set_value(s.value - 1, cx)))
            .on_action(cx.listener(|s, _: &ToMin, _, cx| s.set_value(s.min, cx)))
            .on_action(cx.listener(|s, _: &ToMax, _, cx| s.set_value(s.max, cx)))
            .flex()
            .items_center()
            .gap(theme::SPACE_3)
            .child(
                div()
                    .id("slider-track")
                    .w(TRACK_WIDTH)
                    .h(theme::CONTROL)
                    .relative()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|s, e: &MouseDownEvent, w, cx| s.on_down(e, w, cx)))
                    .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, _, cx| s.on_move(e, cx)))
                    .on_mouse_up(MouseButton::Left, cx.listener(|s, _, _, _| s.dragging = false))
                    // Records where the track is, so pointer positions map to values.
                    .child(
                        canvas(move |bounds, _, _| track.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .left_0()
                            .top_0()
                            .size_full(),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(theme::line_strong())
                            .child(div().h_full().w(TRACK_WIDTH * t).rounded(px(2.)).bg(theme::accent())),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(TRACK_WIDTH * t - THUMB / 2.)
                            .size(THUMB)
                            .rounded_full()
                            .bg(theme::ink())
                            .border_2()
                            .border_color(if focused { theme::accent() } else { theme::raised() }),
                    ),
            )
            .child(
                div()
                    .min_w(px(44.))
                    .text_mono_small()
                    .text_color(theme::ink_soft())
                    .flex()
                    .justify_end()
                    .child(self.label.clone()),
            )
    }
}
