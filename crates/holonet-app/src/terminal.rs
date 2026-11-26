//! Terminal state: an alacritty_terminal `Term` plus its escape-sequence
//! parser, and a snapshot of the visible grid as styled runs ready to paint.
//! Has no GPUI types except colors, so it can be timed without a window.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb};

/// Terminal size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Terminal colors for one scheme (Settings, Terminal, "Color scheme").
///
/// Foreground, background and the accent and green hues come from
/// Settings.dc.html. The design draws no full ANSI palette, so the other
/// entries are built from those hues; the tests below check their contrast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    /// Text drawn on top of a block cursor.
    pub cursor_text: u32,
    pub selection: u32,
    /// Find: every match, and the current one (undrawn state, from the accent).
    pub find_hit: u32,
    pub find_current: u32,
    pub ansi: [u32; 16],
}

impl Palette {
    /// "Holonet Dark": the design tokens (TerminalText.md).
    pub const DARK: Palette = Palette {
        foreground: 0xCFCFCF, // ink-soft
        background: 0x101010, // terminal
        cursor: 0xEDEDED,     // ink
        cursor_text: 0x101010,
        selection: 0x274758, // accent at about 30% over terminal
        find_hit: 0x1C2D36,  // accent at 16% (Terminal.dc.html's hit lines)
        find_current: 0x2B5568,
        ansi: [
            0x2A2A2A, 0xF0776C, 0x7FD6B5, 0xF2A65A, 0x5CC8FF, 0xB9A5FF, 0x6FD3D6, 0xCFCFCF, 0x8A8A8A, 0xFF9A90,
            0xA3EBCF, 0xFFC285, 0x8FDBFF, 0xD3C6FF, 0x9AE6E8, 0xEDEDED,
        ],
    };

    /// "High contrast": black and white, brighter hues.
    pub const HIGH_CONTRAST: Palette = Palette {
        foreground: 0xFFFFFF,
        background: 0x000000,
        cursor: 0xFFFFFF,
        cursor_text: 0x000000,
        selection: 0x1F4F66,
        find_hit: 0x142229,
        find_current: 0x2E5A6E,
        ansi: [
            0x3A3A3A, 0xFF8A80, 0x9FF0CF, 0xFFD27A, 0x7AD7FF, 0xD2C2FF, 0x8CF0F2, 0xE6E6E6, 0xBDBDBD, 0xFFB0A8,
            0xC4FFE6, 0xFFE3AA, 0xB0E8FF, 0xE6DCFF, 0xB8FAFB, 0xFFFFFF,
        ],
    };

    /// "Light": dark text on near white. The "white" entries are dark grays so
    /// programs that print in white stay readable.
    pub const LIGHT: Palette = Palette {
        foreground: 0x1F1F1F,
        background: 0xFAFAFA,
        cursor: 0x1F1F1F,
        cursor_text: 0xFAFAFA,
        selection: 0xC5E1F0,
        find_hit: 0xE2EDF2,
        find_current: 0xF5D9A8,
        ansi: [
            0x1F1F1F, 0xB3261E, 0x1B7A55, 0x8A5A00, 0x0B6A99, 0x7442C8, 0x0E6F75, 0x5F5F5F, 0x6B6B6B, 0xA8201A,
            0x18694A, 0x7A4F00, 0x095A82, 0x6538B0, 0x0B5F64, 0x3A3A3A,
        ],
    };

    pub fn for_scheme(scheme: holonet_model::ColorScheme) -> &'static Palette {
        match scheme {
            holonet_model::ColorScheme::HolonetDark => &Self::DARK,
            holonet_model::ColorScheme::HighContrast => &Self::HIGH_CONTRAST,
            holonet_model::ColorScheme::Light => &Self::LIGHT,
        }
    }

    /// Dimmed text (SGR 2): a third of the way toward the background.
    fn dim(&self, c: u32) -> u32 {
        let mix = |shift: u32| {
            let (a, b) = ((c >> shift) & 0xFF, (self.background >> shift) & 0xFF);
            ((a * 2 + b) / 3) << shift
        };
        mix(16) | mix(8) | mix(0)
    }

    fn named(&self, name: NamedColor) -> u32 {
        let i = name as usize;
        match name {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.dim(self.foreground),
            _ if i < 16 => self.ansi[i],
            // Dim variants of the 8 base colors.
            _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                self.dim(self.ansi[i - NamedColor::DimBlack as usize])
            }
            _ => self.foreground,
        }
    }

    /// xterm 256-color table, with the first 16 from this palette.
    fn indexed(&self, i: u8) -> u32 {
        if i < 16 { self.ansi[i as usize] } else { xterm_256(i) }
    }
}

/// One run of cells with the same style, starting at `col`.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub col: usize,
    /// Number of grid cells the run covers (a wide char covers two).
    pub cells: usize,
    pub text: String,
    pub fg: u32,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

pub struct Snapshot {
    /// Grid size the snapshot was taken at.
    pub size: GridSize,
    pub lines: Vec<Vec<Run>>,
    /// Cursor cell, if it is visible on screen.
    pub cursor: Option<(usize, usize, CursorShape)>,
    /// Character under the cursor, redrawn on top of a block cursor.
    pub cursor_char: char,
    pub display_offset: usize,
    pub history: usize,
    pub palette: &'static Palette,
    /// The cursor should blink (the settings default, or as a program asked).
    pub cursor_blinking: bool,
}

pub struct TerminalModel {
    term: Term<VoidListener>,
    parser: Processor,
    size: GridSize,
    palette: &'static Palette,
    /// Bumped on every change to the grid, so search results know when they
    /// are stale.
    generation: u64,
    /// Find results to highlight, sorted top to bottom, and the current one.
    hits: Vec<Match>,
    current_hit: Option<usize>,
}

/// One find match: a grid line (negative in the history) and a column range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: i32,
    pub start: usize,
    /// Exclusive.
    pub end: usize,
}

impl TerminalModel {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let config = Config { scrolling_history: scrollback, ..Default::default() };
        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            size,
            palette: &Palette::DARK,
            generation: 0,
            hits: Vec::new(),
            current_hit: None,
        }
    }

    /// Apply terminal settings: colors, history length and the default cursor
    /// (programs can still change the cursor with DECSCUSR).
    pub fn configure(&mut self, settings: &holonet_model::TerminalSettings) {
        self.palette = Palette::for_scheme(settings.scheme);
        let shape = match settings.cursor {
            holonet_model::CursorShape::Block => CursorShape::Block,
            holonet_model::CursorShape::Bar => CursorShape::Beam,
            holonet_model::CursorShape::Underline => CursorShape::Underline,
        };
        let config = Config {
            scrolling_history: settings.scrollback.lines().unwrap_or(usize::MAX),
            default_cursor_style: CursorStyle { shape, blinking: settings.cursor_blink },
            ..Default::default()
        };
        self.term.set_options(config);
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Every match of `re` in the history and the screen, top to bottom.
    /// Lines are matched one at a time, as their cells read; a match never
    /// spans a wrapped line.
    pub fn find(&self, re: &regex::Regex) -> Vec<Match> {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let cols = self.size.cols;
        let mut text = String::with_capacity(cols * 2);
        // Byte offset in `text` -> column, for each character pushed.
        let mut col_at: Vec<(usize, usize)> = Vec::with_capacity(cols);
        let mut out = Vec::new();
        for line in top..self.size.rows as i32 {
            let row = &grid[Line(line)];
            text.clear();
            col_at.clear();
            for col in 0..cols {
                let cell = &row[Column(col)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                col_at.push((text.len(), col));
                text.push(cell.c);
                if let Some(zw) = cell.zerowidth() {
                    text.extend(zw);
                }
            }
            let trimmed = text.trim_end().len();
            if trimmed == 0 {
                continue;
            }
            for m in re.find_iter(&text[..trimmed]) {
                if m.start() == m.end() {
                    continue;
                }
                let column = |byte: usize| match col_at.binary_search_by_key(&byte, |(b, _)| *b) {
                    Ok(i) => col_at[i].1,
                    Err(i) => col_at.get(i).map_or(cols, |(_, c)| *c),
                };
                let start = column(m.start());
                // The last character's column, plus its width.
                let last = col_at.iter().rev().find(|(b, _)| *b < m.end()).map_or(start, |(_, c)| *c);
                let wide = row[Column(last)].flags.contains(Flags::WIDE_CHAR);
                out.push(Match { line, start, end: last + if wide { 2 } else { 1 } });
            }
        }
        out
    }

    /// Matches to highlight in snapshots, and which is current.
    pub fn set_hits(&mut self, hits: Vec<Match>, current: Option<usize>) {
        self.hits = hits;
        self.current_hit = current;
    }

    /// Scroll so a grid line is on screen, a few lines from the top when the
    /// view has to move.
    pub fn reveal(&mut self, line: i32) {
        let offset = self.term.grid().display_offset() as i32;
        let rows = self.size.rows as i32;
        let first = -offset;
        if line >= first && line < first + rows {
            return;
        }
        let history = self.term.grid().history_size() as i32;
        // A little context above the match, less in a short pane.
        let margin = (rows / 3).min(3);
        let wanted = (-(line - margin)).clamp(0, history);
        self.term.scroll_display(Scroll::Delta(wanted - offset));
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.generation = self.generation.wrapping_add(1);
        self.parser.advance(&mut self.term, bytes);
    }

    #[allow(dead_code)] // used by tests; the view reads size from snapshots
    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn resize(&mut self, size: GridSize) {
        if size != self.size && size.cols > 1 && size.rows > 0 {
            self.generation = self.generation.wrapping_add(1);
            self.size = size;
            self.term.resize(size);
        }
    }

    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Grid point for a cell on screen, accounting for scrollback position.
    fn grid_point(&self, row: usize, col: usize) -> Point {
        let row = row.min(self.size.rows.saturating_sub(1));
        let col = col.min(self.size.cols.saturating_sub(1));
        viewport_to_point(self.term.grid().display_offset(), Point::new(row, Column(col)))
    }

    /// Start a selection at a screen cell. `right_half` is which half of the
    /// cell the pointer is in, so a selection can start between characters.
    pub fn start_selection(&mut self, ty: SelectionType, row: usize, col: usize, right_half: bool) {
        let side = if right_half { Side::Right } else { Side::Left };
        self.term.selection = Some(Selection::new(ty, self.grid_point(row, col), side));
    }

    pub fn update_selection(&mut self, row: usize, col: usize, right_half: bool) {
        let point = self.grid_point(row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    pub fn snapshot(&self) -> Snapshot {
        let content = self.term.renderable_content();
        let colors = content.colors;
        let p = self.palette;
        let resolve = |color: Color, bold: bool| -> u32 {
            let from_rgb = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            match color {
                Color::Spec(rgb) => from_rgb(rgb),
                Color::Indexed(i) => colors[i as usize].map(from_rgb).unwrap_or_else(|| p.indexed(i)),
                Color::Named(name) => {
                    // Bold text in the first 8 colors uses the bright variant.
                    let name = if bold { name.to_bright() } else { name };
                    colors[name].map(from_rgb).unwrap_or_else(|| p.named(name))
                }
            }
        };

        let display_offset = content.display_offset;
        let selection = content.selection;
        let mut lines: Vec<Vec<Run>> = vec![Vec::new(); self.size.rows];
        for indexed in content.display_iter {
            let cell = indexed.cell;
            let row = (indexed.point.line.0 + display_offset as i32) as usize;
            if row >= lines.len() || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let flags = cell.flags;
            let bold = flags.contains(Flags::BOLD);
            let mut fg = resolve(cell.fg, bold);
            let mut bg = resolve(cell.bg, false);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = p.dim(fg);
            }
            let hit = self.hit_at(indexed.point);
            let bg = if selection.is_some_and(|s| s.contains(indexed.point)) {
                Some(p.selection)
            } else if let Some(current) = hit {
                Some(if current { p.find_current } else { p.find_hit })
            } else {
                (bg != p.background).then_some(bg)
            };
            let ch = if flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
            let wide = flags.contains(Flags::WIDE_CHAR);
            let run = Run {
                col: indexed.point.column.0,
                cells: if wide { 2 } else { 1 },
                text: String::new(),
                fg,
                bg,
                bold,
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.contains(Flags::STRIKEOUT),
            };

            let line = &mut lines[row];
            let joins = !wide
                && line.last().is_some_and(|last: &Run| {
                    last.cells == last.text.chars().count() // no wide chars in it
                        && last.col + last.cells == run.col
                        && same_style(last, &run)
                });
            if joins {
                let last = line.last_mut().unwrap();
                last.text.push(ch);
                last.cells += 1;
            } else {
                let mut run = run;
                run.text.push(ch);
                if let Some(zw) = cell.zerowidth() {
                    run.text.extend(zw);
                }
                line.push(run);
            }
        }

        let cursor = content.cursor;
        let cursor_char = self.term.grid()[cursor.point].c;
        let cursor = (content.mode.contains(TermMode::SHOW_CURSOR) && cursor.shape != CursorShape::Hidden)
            .then(|| {
                let row = cursor.point.line.0 + display_offset as i32;
                (row >= 0 && (row as usize) < self.size.rows).then_some((
                    row as usize,
                    cursor.point.column.0,
                    cursor.shape,
                ))
            })
            .flatten();

        Snapshot {
            size: self.size,
            lines,
            cursor,
            cursor_char,
            display_offset,
            history: self.term.grid().history_size(),
            palette: p,
            cursor_blinking: self.term.cursor_style().blinking,
        }
    }

    /// Whether a grid point is in a find match, and whether that match is the
    /// current one.
    fn hit_at(&self, point: Point) -> Option<bool> {
        if self.hits.is_empty() {
            return None;
        }
        let line = point.line.0;
        let first = self.hits.partition_point(|m| m.line < line);
        self.hits[first..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.line == line)
            .find(|(_, m)| (m.start..m.end).contains(&point.column.0))
            .map(|(i, _)| self.current_hit == Some(first + i))
    }

    /// Text of the visible screen, for tests and debugging.
    #[allow(dead_code)]
    pub fn screen_text(&self) -> String {
        let grid = self.term.grid();
        (0..self.size.rows)
            .map(|r| {
                let row = &grid[Line(r as i32)];
                (0..self.size.cols).map(|c| row[Column(c)].c).collect::<String>().trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[allow(dead_code)]
    pub fn cell_char(&self, row: usize, col: usize) -> char {
        self.term.grid()[Point::new(Line(row as i32), Column(col))].c
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strikeout == b.strikeout
}

/// xterm 256-color table above the first 16.
fn xterm_256(i: u8) -> u32 {
    match i {
        0..=15 => 0,
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v as u32 };
            level(i / 36) << 16 | level((i / 6) % 6) << 8 | level(i % 6)
        }
        232..=255 => {
            let v = 8 + 10 * (i as u32 - 232);
            v << 16 | v << 8 | v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_runs() {
        let mut t = TerminalModel::new(GridSize { cols: 20, rows: 3 }, 100);
        t.advance(b"ab\x1b[31mcd\x1b[0m\x1b[1;34mef\x1b[0m");
        let s = t.snapshot();
        let runs = &s.lines[0];
        assert_eq!(runs[0].text, "ab");
        assert_eq!(runs[0].fg, Palette::DARK.foreground);
        assert_eq!((runs[1].text.as_str(), runs[1].fg), ("cd", Palette::DARK.ansi[1]));
        // Bold blue becomes bright blue.
        assert_eq!((runs[2].text.as_str(), runs[2].fg, runs[2].bold), ("ef", Palette::DARK.ansi[12], true));
        assert_eq!(s.cursor.map(|c| (c.0, c.1)), Some((0, 6)));
    }

    #[test]
    fn wide_chars_take_two_cells() {
        let mut t = TerminalModel::new(GridSize { cols: 20, rows: 2 }, 100);
        t.advance("a日b".as_bytes());
        let runs = &t.snapshot().lines[0];
        let wide = runs.iter().find(|r| r.text == "日").unwrap();
        assert_eq!((wide.col, wide.cells), (1, 2));
        let b = runs.iter().find(|r| r.text.starts_with('b')).unwrap();
        assert_eq!(b.col, 3);
    }

    #[test]
    fn osc_3008_and_bracketed_paste_are_ignored() {
        // What Ubuntu 26.04's prompt sends (see docs/prototype-notes.md).
        let mut t = TerminalModel::new(GridSize { cols: 40, rows: 2 }, 100);
        t.advance(b"\x1b[?2004h\x1b]3008;start=abc;type=shell;user=holotest\x1b\\$ ls\x1b[?2004l");
        assert_eq!(t.screen_text().lines().next().unwrap(), "$ ls");
    }

    #[test]
    fn selection_text_and_highlight() {
        let mut t = TerminalModel::new(GridSize { cols: 20, rows: 3 }, 100);
        t.advance(b"hello world\r\nsecond line");
        t.start_selection(SelectionType::Simple, 0, 6, false);
        t.update_selection(1, 5, true);
        assert_eq!(t.selection_text().as_deref(), Some("world\nsecond"));
        let s = t.snapshot();
        let highlighted: String =
            s.lines[0].iter().filter(|r| r.bg == Some(Palette::DARK.selection)).map(|r| r.text.as_str()).collect();
        assert!(highlighted.starts_with("world"));
        t.clear_selection();
        assert_eq!(t.selection_text(), None);
    }

    #[test]
    fn selection_follows_scrollback() {
        let mut t = TerminalModel::new(GridSize { cols: 10, rows: 2 }, 100);
        t.advance(b"one\r\ntwo\r\nthree\r\nfour");
        t.scroll(2); // screen now shows "one", "two"
        t.start_selection(SelectionType::Simple, 0, 0, false);
        t.update_selection(0, 2, true);
        assert_eq!(t.selection_text().as_deref(), Some("one"));
    }

    #[test]
    fn scrollback_and_resize() {
        let mut t = TerminalModel::new(GridSize { cols: 10, rows: 3 }, 100);
        for i in 0..10 {
            t.advance(format!("line{i}\r\n").as_bytes());
        }
        let s = t.snapshot();
        assert!(s.history >= 7);
        t.scroll(2);
        assert_eq!(t.snapshot().display_offset, 2);
        t.resize(GridSize { cols: 20, rows: 5 });
        assert_eq!(t.size(), GridSize { cols: 20, rows: 5 });
    }

    fn luminance(c: u32) -> f64 {
        let ch = |shift: u32| {
            let v = ((c >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn palettes_are_readable() {
        for (name, p) in [("dark", Palette::DARK), ("high contrast", Palette::HIGH_CONTRAST), ("light", Palette::LIGHT)]
        {
            assert!(contrast(p.foreground, p.background) >= 7.0, "{name}: foreground");
            assert!(contrast(p.foreground, p.selection) >= 4.5, "{name}: text on selection");
            assert!(contrast(p.cursor_text, p.cursor) >= 4.5, "{name}: text on cursor");
            for (i, c) in p.ansi.iter().enumerate() {
                // Black is a background color; bright black is for de-emphasised text.
                let min = match i {
                    0 => continue,
                    8 => 3.0,
                    _ => 4.5,
                };
                let ratio = contrast(*c, p.background);
                assert!(ratio >= min, "{name}: ANSI {i} {c:06X} has {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn configure_changes_colors_cursor_and_history() {
        let mut m = TerminalModel::new(GridSize { cols: 10, rows: 2 }, 100);
        let settings = holonet_model::TerminalSettings {
            scheme: holonet_model::ColorScheme::Light,
            cursor: holonet_model::CursorShape::Bar,
            cursor_blink: false,
            scrollback: holonet_model::Scrollback::Lines50k,
            ..Default::default()
        };
        m.configure(&settings);
        m.advance(b"\x1b[31mred\x1b[0m plain");
        let snap = m.snapshot();
        assert_eq!(snap.palette, &Palette::LIGHT);
        assert_eq!(snap.lines[0][0].fg, Palette::LIGHT.ansi[1]);
        assert_eq!(snap.cursor.map(|c| c.2), Some(CursorShape::Beam));
        assert!(!snap.cursor_blinking);
        // A program can still ask for a blinking block (DECSCUSR 1).
        m.advance(b"\x1b[1 q");
        let snap = m.snapshot();
        assert_eq!(snap.cursor.map(|c| c.2), Some(CursorShape::Block));
        assert!(snap.cursor_blinking);
    }

    #[test]
    fn find_plain_regex_case_and_history() {
        let mut m = TerminalModel::new(GridSize { cols: 20, rows: 3 }, 100);
        m.advance(b"alpha Beta\r\nnothing\r\nbeta gamma beta\r\nlast line\r\n");
        // "alpha Beta" has scrolled into the history.
        let insensitive = regex::Regex::new("(?i)beta").unwrap();
        let hits = m.find(&insensitive);
        assert_eq!(hits.len(), 3);
        assert!(hits[0].line < 0, "first hit is in the history");
        assert_eq!((hits[0].start, hits[0].end), (6, 10));
        assert_eq!((hits[2].start, hits[2].end), (11, 15));
        assert_eq!(m.find(&regex::Regex::new("beta").unwrap()).len(), 2);
        assert_eq!(m.find(&regex::Regex::new(r"g\w+").unwrap()).len(), 1);
    }

    #[test]
    fn find_counts_wide_characters_in_columns() {
        let mut m = TerminalModel::new(GridSize { cols: 20, rows: 2 }, 10);
        m.advance("日本 ok".as_bytes());
        let hits = m.find(&regex::Regex::new("本").unwrap());
        assert_eq!((hits[0].start, hits[0].end), (2, 4));
        let hits = m.find(&regex::Regex::new("ok").unwrap());
        assert_eq!((hits[0].start, hits[0].end), (5, 7));
    }

    #[test]
    fn hits_are_highlighted_and_revealed() {
        let mut m = TerminalModel::new(GridSize { cols: 10, rows: 2 }, 100);
        for i in 0..20 {
            m.advance(format!("line {i}\r\n").as_bytes());
        }
        let hits = m.find(&regex::Regex::new("line 3$").unwrap());
        assert_eq!(hits.len(), 1);
        m.set_hits(hits.clone(), Some(0));
        m.reveal(hits[0].line);
        let snap = m.snapshot();
        let row = snap.lines.iter().position(|r| r.iter().any(|run| run.text.contains("line 3"))).unwrap();
        assert!(snap.lines[row].iter().any(|r| r.bg == Some(Palette::DARK.find_current)));
    }

    #[test]
    fn find_colors_keep_text_readable() {
        for p in [Palette::DARK, Palette::HIGH_CONTRAST, Palette::LIGHT] {
            assert!(contrast(p.foreground, p.find_hit) >= 4.5);
            assert!(contrast(p.foreground, p.find_current) >= 4.5);
        }
    }
}
