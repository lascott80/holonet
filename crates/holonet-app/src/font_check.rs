//! Spike 1: draw text in the terminal font and measure glyph cell widths.

use gpui::{Context, FontWeight, IntoElement, Render, SharedString, Window, div, font, prelude::*, px, rgb};

const FONT_SIZE: f32 = 13.0;

// Design tokens (docs/design/tokens.json).
const GROUND: u32 = 0x121212;
const TERMINAL: u32 = 0x101010;
const LINE: u32 = 0x2C2C2C;
const INK: u32 = 0xEDEDED;

/// Characters whose advance must equal one terminal cell.
const WIDTH_PROBES: &[(&str, char)] = &[
    ("Latin M", 'M'),
    ("Latin i", 'i'),
    ("Box drawing", '─'),
    ("Powerline arrow", '\u{E0B0}'),
    ("Powerline rounded", '\u{E0B4}'),
    ("Folder icon", '\u{F07B}'),
    ("Git branch", '\u{E725}'),
    ("Linux penguin", '\u{F17C}'),
    ("Rust logo", '\u{E7A8}'),
];

struct WidthRow {
    label: &'static str,
    ch: char,
    advance: Option<f32>,
}

pub struct FontCheck {
    family: SharedString,
    cell_width: f32,
    rows: Vec<WidthRow>,
}

impl FontCheck {
    pub fn new(window: &mut Window) -> Self {
        let text = window.text_system();
        let family = crate::pick_font(&text.all_font_names());
        let font_id = text.resolve_font(&font(family));
        let size = px(FONT_SIZE);
        let measure = |ch: char| text.advance(font_id, size, ch).ok().map(|s| f32::from(s.width));
        let cell_width = measure('M').unwrap_or(0.0);
        let rows = WIDTH_PROBES
            .iter()
            .map(|&(label, ch)| WidthRow { label, ch, advance: measure(ch) })
            .collect();
        Self { family: family.into(), cell_width, rows }
    }
}

impl Render for FontCheck {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mono = self.family.clone();
        let cell = self.cell_width;
        let line = |text: String| div().child(text);

        let mut sample = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_4()
            .bg(rgb(TERMINAL))
            .border_1()
            .border_color(rgb(LINE))
            .font_family(mono.clone())
            .text_size(px(FONT_SIZE))
            .child(line(format!("Font: {}  cell width: {:.2}px", mono, cell)))
            .child(line(" \u{E0B6}\u{E0B0} lascott@holonet \u{E0B0} \u{F07B} ~/src \u{E0B0} \u{E725} main \u{E0B4}".into()))
            .child(line("Ligatures: -> => != === <= >= :: |> www <!-- -->".into()))
            .child(line("Grid:      |0123456789|ABCDEFGHIJ|".into()))
            .child(line("Icons:     |\u{F07B}\u{E725}\u{F17C}\u{E7A8}\u{E0B0}\u{E0B4}─│┼█|".into()));

        for row in &self.rows {
            let verdict = match row.advance {
                None => "missing glyph".to_string(),
                Some(w) if (w - cell).abs() < 0.01 => format!("{w:.2}px  one cell"),
                Some(w) => format!("{w:.2}px  WRONG WIDTH ({:.2} cells)", w / cell),
            };
            sample = sample.child(line(format!("{:<18} {}  {}", row.label, row.ch, verdict)));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(GROUND))
            .text_color(rgb(INK))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Holonet prototype: GPUI text check"))
            .child(sample)
    }
}
