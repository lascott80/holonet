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
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollWheelEvent, SharedString,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px, rgb, size,
};
use holonet_ssh::{PtySize, ShellEvent, ShellInput};
use tokio::sync::mpsc::UnboundedSender;

use crate::input;
use crate::terminal::{GridSize, Match, Snapshot, TerminalModel};
use holonet_model::TerminalSettings;

/// How long the cursor stays on, and off, while blinking.
const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(530);
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
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
    copy_on_select: bool,
    right_click_pastes: bool,
    /// Blink phase: the cursor is drawn while this is true.
    blink_visible: bool,
    /// Last keystroke or output; the cursor stays solid for a moment after it.
    blink_reset: Instant,
    blink_task: Option<gpui::Task<()>>,
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
    /// The session log, while History saves this terminal's output.
    log_tap: Option<crate::history::LogTap>,
    last_echo_ms: Option<f64>,
    selecting: bool,
    dragged: bool,
    /// Inside a session pane: no status bar, pane padding (Terminal.dc.html).
    embedded: bool,
    /// The Settings preview: never takes focus, draws its cursor as if focused.
    preview: bool,
    /// Settings to apply at the next render (where a window is available).
    pending: Option<TerminalSettings>,
    /// Find in this terminal's scrollback, while the find bar targets it.
    search: Option<Search>,
    /// Grid size measured from the canvas at paint time; applied next render.
    measured: Rc<Cell<Option<GridSize>>>,
}

/// What the find bar asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub match_case: bool,
    pub regex: bool,
}

/// The find bar's count line (Terminal.dc.html).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStatus {
    /// Nothing typed yet.
    Empty,
    /// The text is not a valid regular expression.
    Invalid,
    /// `lines` lines match, `matches` matches in all; `current` is 1-based.
    Found {
        lines: usize,
        matches: usize,
        current: usize,
    },
    NoMatches,
}

impl SearchStatus {
    pub fn text(&self) -> String {
        match self {
            SearchStatus::Empty => "Type to search".into(),
            SearchStatus::Invalid => "Invalid regex".into(),
            SearchStatus::NoMatches => "No matches".into(),
            SearchStatus::Found { lines: 1, .. } => "1 line matches".into(),
            SearchStatus::Found { lines, .. } => format!("{lines} lines match"),
        }
    }
}

struct Search {
    regex: regex::Regex,
    hits: Vec<Match>,
    lines: usize,
    current: Option<usize>,
    /// Model generation the hits were found at, and when.
    generation: u64,
    searched_at: Instant,
    /// Re-runs the search once output has settled.
    refresh: Option<gpui::Task<()>>,
}

/// How often a live search is re-run while output keeps arriving.
const SEARCH_REFRESH: std::time::Duration = std::time::Duration::from_millis(150);

/// Emitted when the remote shell ends or the connection under it is lost.
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    /// The shell exited or its channel closed: nothing to reconnect.
    Ended(String),
    /// The connection was lost under a running shell (milestone 3 phase 7
    /// reconnects). The terminal and its scrollback stay.
    Lost(holonet_ssh::LostReason),
    /// Something the person typed here, for broadcast to the tab's other
    /// panes (each encodes it in its own terminal mode).
    Typed(Typed),
}

/// Input as the person gave it, before a terminal's mode encodes it.
#[derive(Clone, Debug)]
pub enum Typed {
    Key(gpui::Keystroke),
    Paste(String),
    /// A snippet run by hand.
    Text(String),
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
        let view = Self::unfocused(window, cx);
        window.focus(&view.focus);
        view
    }

    /// A non-interactive terminal for the Settings preview.
    pub fn preview(rows: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self::unfocused(window, cx);
        view.preview = true;
        view.set_embedded(GridSize { cols: 80, rows });
        view
    }

    /// Settings changed while this terminal is open: applied at the next frame.
    pub fn set_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        self.pending = Some(settings);
        cx.notify();
    }

    fn unfocused(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let mut view = Self {
            model: TerminalModel::new(GridSize { cols: 80, rows: 24 }, SCROLLBACK),
            font: font("Consolas"),
            font_size: px(13.),
            cell_width: px(7.8),
            line_height: px(20.8),
            copy_on_select: false,
            right_click_pastes: true,
            blink_visible: true,
            blink_reset: Instant::now(),
            blink_task: None,
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
            log_tap: None,
            last_echo_ms: None,
            selecting: false,
            dragged: false,
            embedded: false,
            preview: false,
            pending: None,
            search: None,
            measured: Rc::new(Cell::new(None)),
        };
        view.apply_settings(&TerminalSettings::default(), window, cx);
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

    /// Font, size, line height, ligatures, colors, cursor, history and mouse
    /// behaviour from Settings. Takes effect at the next frame; the grid is
    /// re-measured, so the server hears about any size change.
    pub fn apply_settings(&mut self, settings: &TerminalSettings, window: &mut Window, cx: &mut Context<Self>) {
        let text = window.text_system();
        let wanted = settings.font.family();
        let family: SharedString = if text.all_font_names().iter().any(|f| f == wanted) {
            wanted.to_string().into()
        } else {
            crate::pick_font(&text.all_font_names()).into()
        };
        let mut f = font(family);
        // GPUI hands DirectWrite an empty feature list unless one is set, which
        // turns ligatures off; ask for `calt` either way (prototype notes,
        // milestone 3 phase 0).
        let calt = u32::from(settings.ligatures_on());
        f.features = gpui::FontFeatures(std::sync::Arc::new(vec![("calt".into(), calt)]));
        // Glyphs the font lacks (Nerd Font icons in an installed font) come
        // from the bundled single-width build.
        f.fallbacks =
            Some(gpui::FontFallbacks::from_fonts(vec![holonet_model::BundledFont::NerdFontMono.family().to_string()]));
        let font_size = px(settings.font_size());
        let font_id = text.resolve_font(&f);
        self.cell_width = text.advance(font_id, font_size, 'M').map(|s| s.width).unwrap_or(font_size * 0.6);
        self.line_height = font_size * settings.line_height();
        self.font_size = font_size;
        self.font = f;
        self.copy_on_select = settings.copy_on_select;
        self.right_click_pastes = settings.right_click_pastes;
        self.model.configure(settings);
        self.measured.set(None);
        self.blink_visible = true;
        cx.notify();
    }

    /// Start, change or (with `None`) clear a find. The current match starts
    /// at the most recent one.
    pub fn set_search(&mut self, query: Option<&SearchQuery>, cx: &mut Context<Self>) -> SearchStatus {
        let Some(q) = query.filter(|q| !q.text.is_empty()) else {
            self.search = None;
            self.model.set_hits(Vec::new(), None);
            cx.notify();
            return SearchStatus::Empty;
        };
        let pattern = if q.regex { q.text.clone() } else { regex::escape(&q.text) };
        let pattern = if q.match_case { pattern } else { format!("(?i){pattern}") };
        let Ok(regex) = regex::Regex::new(&pattern) else {
            self.search = None;
            self.model.set_hits(Vec::new(), None);
            cx.notify();
            return SearchStatus::Invalid;
        };
        self.search = Some(Search {
            regex,
            hits: Vec::new(),
            lines: 0,
            current: None,
            generation: u64::MAX,
            searched_at: Instant::now(),
            refresh: None,
        });
        self.run_search(true);
        if let Some(line) = self.search.as_ref().and_then(|s| s.current.map(|i| s.hits[i].line)) {
            self.model.reveal(line);
        }
        cx.notify();
        self.search_status()
    }

    fn run_search(&mut self, fresh: bool) {
        let Some(search) = self.search.as_mut() else { return };
        let old_len = search.hits.len();
        let from_bottom = search.current.map(|c| old_len.saturating_sub(c + 1));
        search.hits = self.model.find(&search.regex);
        search.lines = {
            let mut lines: Vec<i32> = search.hits.iter().map(|m| m.line).collect();
            lines.dedup();
            lines.len()
        };
        // New output pushes matches up: keep the same distance from the bottom.
        search.current = match (fresh, from_bottom) {
            _ if search.hits.is_empty() => None,
            (false, Some(d)) => Some(search.hits.len().saturating_sub(d + 1)),
            _ => Some(search.hits.len() - 1),
        };
        search.generation = self.model.generation();
        search.searched_at = Instant::now();
        self.model.set_hits(search.hits.clone(), search.current);
    }

    /// Move to the previous (-1, older) or next (+1, newer) match.
    pub fn search_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        if search.hits.is_empty() {
            return;
        }
        let n = search.hits.len() as isize;
        let next = (search.current.map_or(n - 1, |c| c as isize) + delta).rem_euclid(n) as usize;
        search.current = Some(next);
        let line = search.hits[next].line;
        self.model.set_hits(search.hits.clone(), Some(next));
        self.model.reveal(line);
        cx.notify();
    }

    pub fn search_status(&self) -> SearchStatus {
        match &self.search {
            None => SearchStatus::Empty,
            Some(s) if s.hits.is_empty() => SearchStatus::NoMatches,
            Some(s) => {
                SearchStatus::Found { lines: s.lines, matches: s.hits.len(), current: s.current.map_or(0, |c| c + 1) }
            }
        }
    }

    /// Re-run a live search after output changed the grid, at most every
    /// [`SEARCH_REFRESH`] so a flood of output stays fast.
    fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let generation = self.model.generation();
        let Some(search) = self.search.as_mut() else { return };
        if search.generation == generation {
            return;
        }
        if search.searched_at.elapsed() >= SEARCH_REFRESH {
            self.run_search(false);
        } else if search.refresh.is_none() {
            search.refresh = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_REFRESH).await;
                let _ = this.update(cx, |view, cx| {
                    if let Some(s) = view.search.as_mut() {
                        s.refresh = None;
                    }
                    cx.notify();
                });
            }));
        }
    }

    pub fn grid_size(&self) -> GridSize {
        self.model.size()
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.model.advance(bytes);
    }

    /// Show an open shell: its output is parsed here and keystrokes go to it.
    /// The caller keeps the `Session` alive.
    /// Send shell output to a session log too, or stop.
    pub fn set_log(&mut self, tap: Option<crate::history::LogTap>) {
        self.log_tap = tap;
    }

    pub fn attach(&mut self, shell: holonet_ssh::Shell, cx: &mut Context<Self>) {
        let holonet_ssh::Shell { input, mut events } = shell;
        self.input = Some(input);
        self.sent_size = None;
        // A new shell after a reconnect is connected again.
        self.connection = Connection::Connected(String::new());
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
                    if let Some(tap) = &self.log_tap {
                        tap.output(&bytes);
                    }
                    self.model.advance(&bytes);
                    self.blink_reset = Instant::now();
                    self.blink_visible = true;
                }
                ShellEvent::Exit(code) => {
                    self.connection = Connection::Ended(format!("The shell exited with code {code}."))
                }
                ShellEvent::Lost(reason) => {
                    self.connection = Connection::Ended(reason.to_string());
                    self.input = None;
                    cx.emit(TerminalEvent::Lost(reason));
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

    /// Type text into the shell as if from the keyboard (snippets).
    /// Returns false when no shell is open.
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.input.is_none() {
            return false;
        }
        self.send(text.as_bytes().to_vec(), cx);
        true
    }

    /// Type a snippet the person ran: like [`Self::type_text`], and
    /// broadcast with the rest of their typing.
    pub fn type_user_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.type_text(text, cx) {
            cx.emit(TerminalEvent::Typed(Typed::Text(text.to_string())));
        }
    }

    /// Input typed in another pane of a broadcasting tab, encoded for this
    /// terminal's own modes (cursor keys, bracketed paste).
    pub fn replay(&mut self, typed: &Typed, cx: &mut Context<Self>) {
        let bytes = match typed {
            Typed::Key(ks) => {
                let app_cursor = self.model.mode().contains(TermMode::APP_CURSOR);
                match input::encode(ks, app_cursor) {
                    Some(bytes) => bytes,
                    None => return,
                }
            }
            Typed::Paste(text) => input::paste(text, self.model.mode().contains(TermMode::BRACKETED_PASTE)),
            Typed::Text(text) => text.as_bytes().to_vec(),
        };
        self.send(bytes, cx);
    }

    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let Some(input) = &self.input else { return };
        if input.send(ShellInput::Data(bytes)).is_ok() {
            self.blink_reset = Instant::now();
            self.blink_visible = true;
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
            if self.input.is_some() {
                cx.emit(TerminalEvent::Typed(Typed::Paste(text)));
            }
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
            if self.input.is_some() {
                cx.emit(TerminalEvent::Typed(Typed::Key(ks.clone())));
            }
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
        let was_selecting = std::mem::replace(&mut self.selecting, false);
        if !self.dragged {
            // A plain click clears the selection.
            self.model.clear_selection();
        } else if was_selecting && self.copy_on_select {
            self.copy(cx);
        }
        cx.notify();
    }

    fn on_right_click(&mut self, cx: &mut Context<Self>) {
        if self.right_click_pastes {
            self.paste(cx);
        }
    }

    /// Run or stop the blink timer to match the current state.
    fn update_blink(&mut self, blinking: bool, focused: bool, cx: &mut Context<Self>) {
        let wanted = blinking && focused && !crate::platform::reduced_motion();
        if !wanted {
            self.blink_task = None;
            self.blink_visible = true;
            return;
        }
        if self.blink_task.is_some() {
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let alive = this.update(cx, |view, cx| {
                    // Stay solid while typing or output is arriving.
                    view.blink_visible =
                        if view.blink_reset.elapsed() < BLINK_INTERVAL { true } else { !view.blink_visible };
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
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
        if let Some(settings) = self.pending.take() {
            self.apply_settings(&settings, window, cx);
        }
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

        self.refresh_search(cx);
        let started = Instant::now();
        let snapshot = self.model.snapshot();
        self.snapshot_us = started.elapsed().as_micros();
        if let Some(bench) = self.stream.as_mut().filter(|b| b.result.is_none()) {
            bench.snapshot_us_total += self.snapshot_us;
        }
        let (status, warn) = self.status_text(&snapshot);
        let focused = self.preview || self.focus.is_focused(window);
        self.update_blink(snapshot.cursor_blinking, focused, cx);
        let cursor_on = self.blink_visible;
        let background = snapshot.palette.background;

        let font = self.font.clone();
        let font_size = self.font_size;
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
            .bg(rgb(background))
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| this.on_right_click(cx)),
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
                                let metrics = Metrics { font_size, cell_width, line_height };
                                paint_grid(bounds, &snapshot, &font, metrics, (cursor_on, focused), window, cx);
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

#[derive(Clone, Copy)]
struct Metrics {
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
}

fn paint_grid(
    bounds: Bounds<Pixels>,
    snap: &Snapshot,
    base: &Font,
    metrics: Metrics,
    (cursor_on, focused): (bool, bool),
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let origin = bounds.origin;
    let Metrics { font_size, cell_width, line_height } = metrics;
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
            // Box-drawing and block characters fill the cell as rectangles;
            // the font draws everything else (crate::boxdraw).
            let one_per_cell = run.cells == run.text.chars().count();
            let mut text = std::borrow::Cow::Borrowed(run.text.as_str());
            if one_per_cell && run.text.chars().any(crate::boxdraw::handles) {
                let light = (f32::from(font_size) / 13.0).round().max(1.0);
                let (cw, lh) = (f32::from(cell_width), f32::from(line_height));
                for (i, ch) in run.text.chars().enumerate().filter(|(_, ch)| crate::boxdraw::handles(*ch)) {
                    let at = cell_at(row, run.col + i);
                    for r in crate::boxdraw::rects(ch, cw, lh, light) {
                        let area = Bounds::new(point(at.x + px(r.x), at.y + px(r.y)), size(px(r.w), px(r.h)));
                        window.paint_quad(fill(area, color.opacity(r.alpha)));
                    }
                }
                text = run.text.chars().map(|ch| if crate::boxdraw::handles(ch) { ' ' } else { ch }).collect();
                if text.trim().is_empty() && !run.underline && !run.strikeout {
                    continue;
                }
            }
            let text_run = TextRun {
                len: text.len(),
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
            let force = one_per_cell.then_some(cell_width);
            let shaped = window.text_system().shape_line(text.into_owned().into(), font_size, &[text_run], force);
            let _ = shaped.paint(cell_at(row, run.col), line_height, window, cx);
        }
    }

    if let Some((row, col, shape)) = snap.cursor.filter(|_| cursor_on || !focused) {
        let at = cell_at(row, col);
        let cursor = rgb(snap.palette.cursor);
        // Without focus the cursor is an outline, so the focused pane stands out.
        let shape = if focused { shape } else { CursorShape::HollowBlock };
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
                        color: rgb(snap.palette.cursor_text).into(),
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
