//! GPUI view that paints a `TerminalModel` snapshot cell by cell.
//!
//! Each styled run is shaped on its own and placed at its column, with glyph
//! advances forced to the cell width, so the grid stays aligned whatever the
//! font does. Backgrounds are quads painted before the text.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    Bounds, Context, Font, FontStyle, FontWeight, Hsla, IntoElement, Pixels, Render, ScrollWheelEvent,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px,
    rgb, size,
};

use crate::terminal::{GridSize, Snapshot, TerminalModel, palette};
use alacritty_terminal::vte::ansi::CursorShape;

const FONT_SIZE: f32 = 13.0;
/// Line height from the design: 13px at 1.6.
const LINE_HEIGHT: f32 = FONT_SIZE * 1.6;
const PADDING: f32 = 12.0;
const STATUS_HEIGHT: f32 = 28.0;
const SCROLLBACK: usize = 10_000;

// Design tokens.
const PANEL: u32 = 0x171717;
const LINE: u32 = 0x2C2C2C;
const INK_DIM: u32 = 0x8A8A8A;

pub struct TerminalView {
    model: TerminalModel,
    font: Font,
    cell_width: Pixels,
    line_height: Pixels,
    scroll_remainder: f32,
    snapshot_us: u128,
    paint_us: Rc<Cell<u128>>,
    stream: Option<StreamBench>,
}

/// Feeds a large buffer a slice per frame, the way output from `cat` of a
/// big file arrives, and times the whole run including rendering.
struct StreamBench {
    data: Vec<u8>,
    offset: usize,
    started: Instant,
    frames: u32,
    worst_frame_us: u128,
    snapshot_us_total: u128,
    paint_us_total: u128,
    last_frame: Instant,
    quit_when_done: bool,
    result: Option<String>,
}

/// Parse budget per frame, so input and painting keep up while output floods.
const STREAM_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);

impl TerminalView {
    pub fn new(window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let text = window.text_system();
        let family = crate::pick_font(&text.all_font_names());
        let font = font(family);
        let font_id = text.resolve_font(&font);
        let cell_width = text
            .advance(font_id, px(FONT_SIZE), 'M')
            .map(|s| s.width)
            .unwrap_or(px(FONT_SIZE * 0.6));
        Self {
            model: TerminalModel::new(GridSize { cols: 80, rows: 24 }, SCROLLBACK),
            font,
            cell_width,
            line_height: px(LINE_HEIGHT),
            scroll_remainder: 0.0,
            snapshot_us: 0,
            paint_us: Rc::new(Cell::new(0)),
            stream: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.model.advance(bytes);
    }

    pub fn stream(&mut self, data: Vec<u8>, quit_when_done: bool) {
        let now = Instant::now();
        self.stream = Some(StreamBench {
            data,
            offset: 0,
            started: now,
            frames: 0,
            worst_frame_us: 0,
            snapshot_us_total: 0,
            paint_us_total: 0,
            last_frame: now,
            quit_when_done,
            result: None,
        });
    }

    /// Advance the stream bench by one frame's worth of parsing.
    fn step_stream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.stream.as_mut() else { return };
        if bench.result.is_some() {
            return;
        }
        let now = Instant::now();
        if bench.frames > 0 {
            bench.worst_frame_us = bench.worst_frame_us.max((now - bench.last_frame).as_micros());
            // Paint time of the frame just finished.
            bench.paint_us_total += self.paint_us.get();
        }
        bench.last_frame = now;
        bench.frames += 1;

        let deadline = now + STREAM_BUDGET;
        while bench.offset < bench.data.len() && Instant::now() < deadline {
            let end = (bench.offset + 16 * 1024).min(bench.data.len());
            self.model.advance(&bench.data[bench.offset..end]);
            bench.offset = end;
        }

        if bench.offset < bench.data.len() {
            window.request_animation_frame();
        } else {
            let secs = bench.started.elapsed().as_secs_f64();
            let result = format!(
                "streamed {:.1} MB in {:.2} s ({:.0} MB/s), {} frames ({:.0} fps), worst frame {:.1} ms, \
                 avg snapshot {} us, avg paint {} us",
                bench.data.len() as f64 / 1e6,
                secs,
                bench.data.len() as f64 / 1e6 / secs,
                bench.frames,
                bench.frames as f64 / secs,
                bench.worst_frame_us as f64 / 1e3,
                bench.snapshot_us_total / bench.frames.max(1) as u128,
                bench.paint_us_total / (bench.frames.max(2) - 1) as u128,
            );
            println!("{result}");
            bench.result = Some(result);
            if bench.quit_when_done {
                cx.quit();
            }
        }
    }

    fn grid_for(&self, viewport: gpui::Size<Pixels>) -> GridSize {
        let width = f32::from(viewport.width) - 2.0 * PADDING;
        let height = f32::from(viewport.height) - 2.0 * PADDING - STATUS_HEIGHT;
        GridSize {
            cols: (width / f32::from(self.cell_width)).floor().max(2.0) as usize,
            rows: (height / f32::from(self.line_height)).floor().max(1.0) as usize,
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line = f32::from(self.line_height);
        self.scroll_remainder += f32::from(event.delta.pixel_delta(self.line_height).y);
        let lines = (self.scroll_remainder / line).trunc();
        if lines != 0.0 {
            self.scroll_remainder -= lines * line;
            self.model.scroll(lines as i32);
            cx.notify();
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.model.resize(self.grid_for(window.viewport_size()));
        self.step_stream(window, cx);

        let started = Instant::now();
        let snapshot = self.model.snapshot();
        self.snapshot_us = started.elapsed().as_micros();
        if let Some(bench) = self.stream.as_mut().filter(|b| b.result.is_none()) {
            bench.snapshot_us_total += self.snapshot_us;
        }

        let status = match self.stream.as_ref().and_then(|b| b.result.clone()) {
            Some(result) => result,
            None => format!(
            "{} × {}  ·  {}  ·  history {} lines{}  ·  snapshot {} µs  ·  last paint {} µs",
            snapshot.size.cols,
            snapshot.size.rows,
            self.font.family,
            snapshot.history,
            if snapshot.display_offset > 0 { format!(" (scrolled up {})", snapshot.display_offset) } else { String::new() },
            self.snapshot_us,
            self.paint_us.get(),
            ),
        };

        let font = self.font.clone();
        let cell_width = self.cell_width;
        let line_height = self.line_height;
        let paint_us = self.paint_us.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(palette::BACKGROUND))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| this.on_scroll(event, cx)))
            .child(
                div().flex_1().p(px(PADDING)).child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            let started = Instant::now();
                            paint_grid(bounds, &snapshot, &font, cell_width, line_height, window, cx);
                            paint_us.set(started.elapsed().as_micros());
                        },
                    )
                    .size_full(),
                ),
            )
            .child(
                div()
                    .h(px(STATUS_HEIGHT))
                    .px(px(PADDING))
                    .flex()
                    .items_center()
                    .bg(rgb(PANEL))
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .text_color(rgb(INK_DIM))
                    .text_size(px(12.))
                    .child(status),
            )
    }
}

fn paint_grid(
    bounds: Bounds<Pixels>,
    snap: &Snapshot,
    base: &Font,
    cell_width: Pixels,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let origin = bounds.origin;
    let font_size = px(FONT_SIZE);
    let cell_at = |row: usize, col: usize| point(origin.x + cell_width * col as f32, origin.y + line_height * row as f32);

    for (row, runs) in snap.lines.iter().enumerate() {
        for run in runs {
            if let Some(bg) = run.bg {
                let area = Bounds::new(cell_at(row, run.col), size(cell_width * run.cells as f32, line_height));
                window.paint_quad(fill(area, rgb(bg)));
            }
        }
        for run in runs {
            if run.text.trim().is_empty() && !run.underline && !run.strikeout {
                continue;
            }
            let color: Hsla = rgb(run.fg).into();
            let text_run = TextRun {
                len: run.text.len(),
                font: Font {
                    weight: if run.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
                    style: if run.italic { FontStyle::Italic } else { FontStyle::Normal },
                    ..base.clone()
                },
                color,
                background_color: None,
                underline: run.underline.then_some(UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false }),
                strikethrough: run.strikeout.then_some(StrikethroughStyle { thickness: px(1.), color: Some(color) }),
            };
            // Force one glyph per cell, except for runs holding a wide char
            // or combining marks, which are placed by their column alone.
            let force = (run.cells == run.text.chars().count()).then_some(cell_width);
            let shaped = window.text_system().shape_line(run.text.clone().into(), font_size, &[text_run], force);
            let _ = shaped.paint(cell_at(row, run.col), line_height, window, cx);
        }
    }

    if let Some((row, col, shape)) = snap.cursor {
        let at = cell_at(row, col);
        let cursor = rgb(palette::CURSOR);
        match shape {
            CursorShape::Beam => window.paint_quad(fill(Bounds::new(at, size(px(2.), line_height)), cursor)),
            CursorShape::Underline => window.paint_quad(fill(
                Bounds::new(point(at.x, at.y + line_height - px(2.)), size(cell_width, px(2.))),
                cursor,
            )),
            CursorShape::HollowBlock => {
                window.paint_quad(gpui::outline(Bounds::new(at, size(cell_width, line_height)), cursor, gpui::BorderStyle::Solid))
            }
            _ => window.paint_quad(fill(Bounds::new(at, size(cell_width, line_height)), cursor)),
        }
    }
}
