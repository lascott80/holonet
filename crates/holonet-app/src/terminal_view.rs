//! GPUI view that paints a `TerminalModel` snapshot cell by cell, and drives
//! it from an attached SSH shell.
//!
//! Each styled run is shaped on its own and placed at its column, with glyph
//! advances forced to the cell width, so the grid stays aligned whatever the
//! font does. Backgrounds are quads painted before the text.
//!
//! SSH runs on a Tokio runtime on its own threads. Output reaches this view
//! through a tokio channel drained by a GPUI task; keystrokes go back through
//! an unbounded channel. Neither side blocks the other.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    Bounds, ClipboardItem, Context, FocusHandle, Font, FontStyle, FontWeight, Hsla, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollWheelEvent,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};
use holonet_ssh::{PtySize, ShellEvent, ShellInput};
use tokio::sync::mpsc::UnboundedSender;

use crate::input;
use crate::terminal::{GridSize, Snapshot, TerminalModel, palette};

const FONT_SIZE: f32 = 13.0;
/// Line height from the design: 13px at 1.6.
const LINE_HEIGHT: f32 = FONT_SIZE * 1.6;
const PADDING: f32 = 12.0;
const STATUS_HEIGHT: f32 = 28.0;
const SCROLLBACK: usize = 10_000;
/// Most output parsed per update while output floods, so frames keep coming.
const MAX_BATCH_BYTES: usize = 1 << 20;

// Design tokens.
const PANEL: u32 = 0x171717;
const LINE: u32 = 0x2C2C2C;
const INK_DIM: u32 = 0x8A8A8A;
const WARN: u32 = 0xF2A65A;

enum Connection {
    /// Canned output only (spike 3).
    Offline,
    Connected(String),
    Ended(String),
}

pub struct TerminalView {
    model: TerminalModel,
    font: Font,
    cell_width: Pixels,
    line_height: Pixels,
    focus: FocusHandle,
    /// Top-left of the grid in window coordinates, set at paint time.
    grid_origin: Rc<Cell<Point<Pixels>>>,
    scroll_remainder: f32,
    snapshot_us: u128,
    paint_us: Rc<Cell<u128>>,
    stream: Option<StreamBench>,

    connection: Connection,
    input: Option<UnboundedSender<ShellInput>>,
    sent_size: Option<GridSize>,
    /// When the last keystroke was sent, until the first output after it.
    echo_pending: Option<Instant>,
    last_echo_ms: Option<f64>,
    selecting: bool,
    dragged: bool,
    /// Inside a session pane: no status bar, pane padding (Terminal.dc.html).
    embedded: bool,
    /// Grid size measured from the canvas at paint time; applied next render.
    measured: Rc<Cell<Option<GridSize>>>,
}

/// Emitted when the remote shell ends or the connection closes.
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    Ended(String),
}

impl gpui::EventEmitter<TerminalEvent> for TerminalView {}

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
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = window.text_system();
        let family = crate::pick_font(&text.all_font_names());
        let font = font(family);
        let font_id = text.resolve_font(&font);
        let cell_width = text.advance(font_id, px(FONT_SIZE), 'M').map(|s| s.width).unwrap_or(px(FONT_SIZE * 0.6));
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut view = Self {
            model: TerminalModel::new(GridSize { cols: 80, rows: 24 }, SCROLLBACK),
            font,
            cell_width,
            line_height: px(LINE_HEIGHT),
            focus,
            grid_origin: Rc::new(Cell::new(point(px(0.), px(0.)))),
            scroll_remainder: 0.0,
            snapshot_us: 0,
            paint_us: Rc::new(Cell::new(0)),
            stream: None,
            connection: Connection::Offline,
            input: None,
            sent_size: None,
            echo_pending: None,
            last_echo_ms: None,
            selecting: false,
            dragged: false,
            embedded: false,
            measured: Rc::new(Cell::new(None)),
        };
        // Size the grid to the window now, so the PTY opens at the right size.
        let grid = view.grid_for(window.viewport_size());
        view.model.resize(grid);
        view
    }

    /// Draw inside a session pane instead of as the whole window.
    pub fn set_embedded(&mut self, size: GridSize) {
        self.embedded = true;
        self.model.resize(size);
    }

    pub fn grid_size(&self) -> GridSize {
        self.model.size()
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.model.advance(bytes);
    }

    /// Show an open shell: its output is parsed here and keystrokes go to it.
    /// The caller keeps the `Session` alive.
    pub fn attach(&mut self, shell: holonet_ssh::Shell, cx: &mut Context<Self>) {
        let holonet_ssh::Shell { input, mut events } = shell;
        self.input = Some(input);
        self.sent_size = None;
        if matches!(self.connection, Connection::Offline) {
            self.connection = Connection::Connected(String::new());
        }
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                // Take whatever else is already waiting, up to a budget.
                let mut batch = vec![first];
                let mut bytes = 0;
                while bytes < MAX_BATCH_BYTES {
                    match events.try_recv() {
                        Ok(event) => {
                            if let ShellEvent::Output(b) = &event {
                                bytes += b.len();
                            }
                            batch.push(event);
                        }
                        Err(_) => break,
                    }
                }
                if this.update(cx, |view, cx| view.apply(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, batch: Vec<ShellEvent>, cx: &mut Context<Self>) {
        for event in batch {
            match event {
                ShellEvent::Output(bytes) => {
                    if let Some(sent) = self.echo_pending.take() {
                        self.last_echo_ms = Some(sent.elapsed().as_secs_f64() * 1e3);
                    }
                    self.model.advance(&bytes);
                }
                ShellEvent::Exit(code) => {
                    self.connection = Connection::Ended(format!("The shell exited with code {code}."))
                }
                ShellEvent::Closed => {
                    if !matches!(self.connection, Connection::Ended(_)) {
                        self.connection = Connection::Ended("The connection closed.".into());
                    }
                    self.input = None;
                    if let Connection::Ended(reason) = &self.connection {
                        cx.emit(TerminalEvent::Ended(reason.clone()));
                    }
                }
            }
        }
        cx.notify();
    }

    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let Some(input) = &self.input else { return };
        if input.send(ShellInput::Data(bytes)).is_ok() {
            self.echo_pending.get_or_insert_with(Instant::now);
            self.model.scroll_to_bottom();
            self.model.clear_selection();
            cx.notify();
        }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.model.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let bracketed = self.model.mode().contains(TermMode::BRACKETED_PASTE);
            self.send(input::paste(&text, bracketed), cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        // App shortcuts use Ctrl Shift so shell keys always reach the shell.
        if m.control && m.shift && !m.alt {
            match ks.key.as_str() {
                "c" => return self.copy(cx),
                "v" => return self.paste(cx),
                _ => {}
            }
        }
        // Shift Page Up/Down scroll the history instead of going to the shell.
        if m.shift && !m.control && !m.alt {
            let page = self.model.size().rows.saturating_sub(1).max(1) as i32;
            match ks.key.as_str() {
                "pageup" => return self.scroll_lines(page, cx),
                "pagedown" => return self.scroll_lines(-page, cx),
                _ => {}
            }
        }
        let app_cursor = self.model.mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = input::encode(ks, app_cursor) {
            cx.stop_propagation();
            self.send(bytes, cx);
        }
    }

    fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.model.scroll(lines);
        cx.notify();
    }

    fn grid_for(&self, viewport: gpui::Size<Pixels>) -> GridSize {
        let width = f32::from(viewport.width) - 2.0 * PADDING;
        let height = f32::from(viewport.height) - 2.0 * PADDING - STATUS_HEIGHT;
        GridSize {
            cols: (width / f32::from(self.cell_width)).floor().max(2.0) as usize,
            rows: (height / f32::from(self.line_height)).floor().max(1.0) as usize,
        }
    }

    /// Screen cell under a window position, and whether it is the cell's right half.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, bool) {
        let origin = self.grid_origin.get();
        let x = f32::from(position.x - origin.x).max(0.0);
        let y = f32::from(position.y - origin.y).max(0.0);
        let cell = f32::from(self.cell_width);
        let col = (x / cell) as usize;
        let row = (y / f32::from(self.line_height)) as usize;
        (row, col, x - col as f32 * cell > cell / 2.0)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let (row, col, right) = self.cell_at(event.position);
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.model.start_selection(ty, row, col, right);
        self.selecting = true;
        // A double or triple click selects without dragging.
        self.dragged = event.click_count > 1;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            let (row, col, right) = self.cell_at(event.position);
            self.model.update_selection(row, col, right);
            self.dragged = true;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, cx: &mut Context<Self>) {
        self.selecting = false;
        if !self.dragged {
            // A plain click clears the selection.
            self.model.clear_selection();
        }
        cx.notify();
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

    fn status_text(&self, snapshot: &Snapshot) -> (String, bool) {
        if let Some(result) = self.stream.as_ref().and_then(|b| b.result.clone()) {
            return (result, false);
        }
        let (state, warn) = match &self.connection {
            Connection::Offline => ("canned output".to_string(), false),
            Connection::Connected(target) => (target.clone(), false),
            Connection::Ended(why) => (why.clone(), true),
        };
        let scrolled = if snapshot.display_offset > 0 {
            format!("  ·  scrolled up {}", snapshot.display_offset)
        } else {
            String::new()
        };
        let echo = self.last_echo_ms.map(|ms| format!("  ·  echo {ms:.1} ms")).unwrap_or_default();
        (
            format!(
                "{state}  ·  {} × {}  ·  history {}{scrolled}{echo}  ·  snapshot {} µs  ·  paint {} µs",
                snapshot.size.cols,
                snapshot.size.rows,
                snapshot.history,
                self.snapshot_us,
                self.paint_us.get(),
            ),
            warn,
        )
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Size: measured from the canvas at the last paint, or (before the
        // first paint of a full-window view) estimated from the window.
        let grid = match self.measured.get() {
            Some(g) => g,
            None if self.embedded => self.model.size(),
            None => self.grid_for(window.viewport_size()),
        };
        self.model.resize(grid);
        if let Some(input) = &self.input
            && self.sent_size != Some(grid)
        {
            let _ = input.send(ShellInput::Resize(PtySize::cells(grid.cols as u16, grid.rows as u16)));
            self.sent_size = Some(grid);
        }
        self.step_stream(window, cx);

        let started = Instant::now();
        let snapshot = self.model.snapshot();
        self.snapshot_us = started.elapsed().as_micros();
        if let Some(bench) = self.stream.as_mut().filter(|b| b.result.is_none()) {
            bench.snapshot_us_total += self.snapshot_us;
        }
        let (status, warn) = self.status_text(&snapshot);

        let font = self.font.clone();
        let cell_width = self.cell_width;
        let line_height = self.line_height;
        let paint_us = self.paint_us.clone();
        let grid_origin = self.grid_origin.clone();
        let measured = self.measured.clone();
        let current = snapshot.size;
        let embedded = self.embedded;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(palette::BACKGROUND))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| this.on_key_down(event, cx)))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| this.on_scroll(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| this.on_mouse_down(event, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| this.on_mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| this.on_mouse_up(event, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(embedded, |d| d.px(px(16.)).py(px(14.)))
                    .when(!embedded, |d| d.p(px(PADDING)))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                grid_origin.set(bounds.origin);
                                let fits = GridSize {
                                    cols: (f32::from(bounds.size.width) / f32::from(cell_width)).floor().max(2.0)
                                        as usize,
                                    rows: (f32::from(bounds.size.height) / f32::from(line_height)).floor().max(1.0)
                                        as usize,
                                };
                                if fits != current {
                                    // Resize on the next frame (and tell the server).
                                    measured.set(Some(fits));
                                    window.refresh();
                                }
                                let started = Instant::now();
                                paint_grid(bounds, &snapshot, &font, cell_width, line_height, window, cx);
                                paint_us.set(started.elapsed().as_micros());
                            },
                        )
                        .size_full(),
                    ),
            )
            .when(!embedded, |d| {
                d.child(
                    div()
                        .h(px(STATUS_HEIGHT))
                        .px(px(PADDING))
                        .flex()
                        .items_center()
                        .bg(rgb(PANEL))
                        .border_t_1()
                        .border_color(rgb(LINE))
                        .text_color(rgb(if warn { WARN } else { INK_DIM }))
                        .text_size(px(12.))
                        .child(status),
                )
            })
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
    let cell_at =
        |row: usize, col: usize| point(origin.x + cell_width * col as f32, origin.y + line_height * row as f32);

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
                underline: run.underline.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
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
            CursorShape::HollowBlock => window.paint_quad(gpui::outline(
                Bounds::new(at, size(cell_width, line_height)),
                cursor,
                gpui::BorderStyle::Solid,
            )),
            _ => {
                window.paint_quad(fill(Bounds::new(at, size(cell_width, line_height)), cursor));
                if !snap.cursor_char.is_whitespace() {
                    let text: gpui::SharedString = snap.cursor_char.to_string().into();
                    let run = TextRun {
                        len: text.len(),
                        font: base.clone(),
                        color: rgb(palette::BACKGROUND).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(text, font_size, &[run], Some(cell_width));
                    let _ = shaped.paint(at, line_height, window, cx);
                }
            }
        }
    }
}
