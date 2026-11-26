//! Terminal state: an alacritty_terminal `Term` plus its escape-sequence
//! parser, and a snapshot of the visible grid as styled runs ready to paint.
//! Has no GPUI types except colors, so it can be timed without a window.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};

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

/// "Holonet Dark". Foreground, background and cursor come from the design
/// tokens (docs/design/components/TerminalText.md). The design has no ANSI
/// palette yet, so these 16 colors are provisional, built from the token hues.
pub mod palette {
    pub const FOREGROUND: u32 = 0xCFCFCF; // ink-soft
    pub const BACKGROUND: u32 = 0x101010; // terminal
    pub const CURSOR: u32 = 0xEDEDED; // ink
    /// `accent` at about 30% over `terminal`. Provisional, like the ANSI set.
    pub const SELECTION: u32 = 0x274758;
    pub const ANSI: [u32; 16] = [
        0x2A2A2A, 0xF0776C, 0x7FD6B5, 0xF2A65A, 0x5CC8FF, 0xB9A5FF, 0x6FD3D6, 0xCFCFCF,
        0x666666, 0xFF9A90, 0xA3EBCF, 0xFFC285, 0x8FDBFF, 0xD3C6FF, 0x9AE6E8, 0xEDEDED,
    ];
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
}

pub struct TerminalModel {
    term: Term<VoidListener>,
    parser: Processor,
    size: GridSize,
}

impl TerminalModel {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let config = Config { scrolling_history: scrollback, ..Default::default() };
        Self { term: Term::new(config, &size, VoidListener), parser: Processor::new(), size }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    #[allow(dead_code)] // used by tests; the view reads size from snapshots
    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn resize(&mut self, size: GridSize) {
        if size != self.size && size.cols > 1 && size.rows > 0 {
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
        let resolve = |color: Color, bold: bool| -> u32 {
            let from_rgb = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
            match color {
                Color::Spec(rgb) => from_rgb(rgb),
                Color::Indexed(i) => colors[i as usize].map(from_rgb).unwrap_or_else(|| indexed(i)),
                Color::Named(name) => {
                    // Bold text in the first 8 colors uses the bright variant.
                    let name = if bold { name.to_bright() } else { name };
                    colors[name].map(from_rgb).unwrap_or_else(|| named(name))
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
                fg = dim(fg);
            }
            let bg = if selection.is_some_and(|s| s.contains(indexed.point)) {
                Some(palette::SELECTION)
            } else {
                (bg != palette::BACKGROUND).then_some(bg)
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
        let cursor =(content.mode.contains(TermMode::SHOW_CURSOR) && cursor.shape != CursorShape::Hidden)
            .then(|| {
                let row = cursor.point.line.0 + display_offset as i32;
                (row >= 0 && (row as usize) < self.size.rows)
                    .then_some((row as usize, cursor.point.column.0, cursor.shape))
            })
            .flatten();

        Snapshot {
            size: self.size,
            lines,
            cursor,
            cursor_char,
            display_offset,
            history: self.term.grid().history_size(),
        }
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

fn dim(c: u32) -> u32 {
    let f = |shift: u32| (((c >> shift) & 0xFF) * 2 / 3) << shift;
    f(16) | f(8) | f(0)
}

fn named(name: NamedColor) -> u32 {
    let i = name as usize;
    match name {
        NamedColor::Foreground | NamedColor::BrightForeground => palette::FOREGROUND,
        NamedColor::Background => palette::BACKGROUND,
        NamedColor::Cursor => palette::CURSOR,
        NamedColor::DimForeground => dim(palette::FOREGROUND),
        _ if i < 16 => palette::ANSI[i],
        // Dim variants of the 8 base colors.
        _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
            dim(palette::ANSI[i - NamedColor::DimBlack as usize])
        }
        _ => palette::FOREGROUND,
    }
}

/// xterm 256-color table.
fn indexed(i: u8) -> u32 {
    match i {
        0..=15 => palette::ANSI[i as usize],
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
        assert_eq!(runs[0].fg, palette::FOREGROUND);
        assert_eq!((runs[1].text.as_str(), runs[1].fg), ("cd", palette::ANSI[1]));
        // Bold blue becomes bright blue.
        assert_eq!((runs[2].text.as_str(), runs[2].fg, runs[2].bold), ("ef", palette::ANSI[12], true));
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
        let highlighted: String = s.lines[0]
            .iter()
            .filter(|r| r.bg == Some(palette::SELECTION))
            .map(|r| r.text.as_str())
            .collect();
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
}
