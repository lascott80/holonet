//! Milestone 3 phase 0, spikes (a) and (c): ligatures and icon widths in the
//! cell grid, and the cost of registering the bundled font files.
//!
//!   HOLONET_SPIKE_FONTS=<dir with the extracted TTFs> holonet --font-spike
//!
//! Prints measurements to stdout and draws each build two ways: as GPUI lays
//! the text out naturally, and the way the terminal paints a run (one glyph
//! forced into each cell).

use std::time::Instant;

use gpui::{
    App, Bounds, Context, Font, IntoElement, Pixels, Render, SharedString, TextRun, Window, canvas, div, font,
    prelude::*, px, rgb,
};

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = FONT_SIZE * 1.6;

/// Family names as the TTFs declare them.
pub const FAMILIES: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "JetBrainsMono Nerd Font Propo",
    "JetBrainsMonoNL Nerd Font Mono",
    "JetBrainsMonoNL Nerd Font",
];

/// File name stems of the builds to register (four styles each).
const STEMS: &[&str] = &[
    "JetBrainsMonoNerdFontMono",
    "JetBrainsMonoNerdFont",
    "JetBrainsMonoNerdFontPropo",
    "JetBrainsMonoNLNerdFontMono",
    "JetBrainsMonoNLNerdFont",
];
const STYLES: &[&str] = &["Regular", "Bold", "Italic", "BoldItalic"];

const LIGATURES: &[&str] = &["->", "=>", "!=", "===", "<=", ">=", "<!--", "|>", "::", "www", "abc"];
const ICONS: &[(&str, char)] = &[
    ("Powerline arrow E0B0", '\u{E0B0}'),
    ("Powerline round E0B4", '\u{E0B4}'),
    ("Folder F07B", '\u{F07B}'),
    ("Git branch E725", '\u{E725}'),
    ("Linux F17C", '\u{F17C}'),
    ("Rust E7A8", '\u{E7A8}'),
    ("Ubuntu F31B (Font Logos)", '\u{F31B}'),
];

const SAMPLE: &str = "a -> b => c != d === e <!-- --> |> :: www";
const ICON_SAMPLE: &str = "|\u{E0B6}\u{E0B0}|\u{F07B}|\u{E725}|\u{F17C}|\u{E7A8}|\u{F31B}|x";

/// Register the spike fonts. Returns (files, MB, read ms, register ms).
pub fn load(cx: &mut App) -> Option<(usize, f64, f64, f64)> {
    let dir = std::env::var_os("HOLONET_SPIKE_FONTS")?;
    let dir = std::path::PathBuf::from(dir);
    let t = Instant::now();
    let mut files = Vec::new();
    for stem in STEMS {
        for style in STYLES {
            match std::fs::read(dir.join(format!("{stem}-{style}.ttf"))) {
                Ok(bytes) => files.push(bytes),
                Err(err) => eprintln!("spike: {stem}-{style}.ttf: {err}"),
            }
        }
    }
    let read = t.elapsed().as_secs_f64() * 1e3;
    let count = files.len();
    let mb = files.iter().map(|f| f.len()).sum::<usize>() as f64 / 1e6;
    let t = Instant::now();
    let fonts = files.into_iter().map(std::borrow::Cow::Owned).collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        eprintln!("spike: add_fonts failed: {err:#}");
    }
    Some((count, mb, read, t.elapsed().as_secs_f64() * 1e3))
}

/// The family with `calt` asked for explicitly. GPUI passes an empty
/// DirectWrite typography when no features are set, which may turn the
/// defaults off.
fn lig_font(family: &str) -> Font {
    let mut f = font(SharedString::from(family.to_string()));
    f.features = gpui::FontFeatures(std::sync::Arc::new(vec![("calt".into(), 1)]));
    f
}

fn run_for(family: &str, len: usize) -> TextRun {
    TextRun {
        len,
        font: if std::env::var_os("SPIKE_NO_CALT").is_some() {
            font(SharedString::from(family.to_string()))
        } else {
            lig_font(family)
        },
        color: rgb(0xEDEDED).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// (glyph ids, x positions in cells) of `text` shaped as one run.
fn shape(window: &Window, family: &str, text: &str, force: Option<Pixels>) -> (Vec<String>, Vec<f32>, f32) {
    let ts = window.text_system();
    let font_id = ts.resolve_font(&font(SharedString::from(family.to_string())));
    let cell = ts.advance(font_id, px(FONT_SIZE), 'M').map(|s| f32::from(s.width)).unwrap_or(1.0);
    let line = ts.shape_line(text.to_string().into(), px(FONT_SIZE), &[run_for(family, text.len())], force);
    let mut ids = Vec::new();
    let mut xs = Vec::new();
    for run in &line.runs {
        for g in &run.glyphs {
            ids.push(format!("{:?}", g.id));
            xs.push(f32::from(g.position.x) / cell);
        }
    }
    (ids, xs, cell)
}

pub fn report(window: &Window) {
    let ts = window.text_system();
    let names = ts.all_font_names();
    for family in FAMILIES {
        let installed = names.iter().any(|n| n == family);
        println!("\n== {family} (registered: {installed})");
        if !installed {
            continue;
        }
        let (_, _, cell) = shape(window, family, "M", None);
        println!("cell width {:.3}px", cell);
        println!("{:<6} {:>8} {:>8} {:>10}  positions (cells, natural)", "text", "glyphs", "ligature", "on grid");
        for text in LIGATURES {
            let (whole, xs, _) = shape(window, family, text, None);
            let single: Vec<String> =
                text.chars().flat_map(|c| shape(window, family, &c.to_string(), None).0).collect();
            let ligature = whole != single;
            let on_grid = whole.len() == text.chars().count() && xs.iter().all(|x| (x - x.round()).abs() < 0.02);
            let (forced, _, _) = shape(window, family, text, Some(px(cell)));
            let forced_lig = forced != single;
            println!(
                "{:<6} {:>8} {:>8} {:>10}  {:?}  forced keeps ligature: {}",
                text,
                whole.len(),
                ligature,
                on_grid,
                xs.iter().map(|x| (x * 100.).round() / 100.).collect::<Vec<_>>(),
                forced_lig
            );
        }
        let font_id = ts.resolve_font(&font(SharedString::from(family.to_string())));
        for (label, ch) in ICONS {
            let adv = ts.advance(font_id, px(FONT_SIZE), *ch).map(|s| f32::from(s.width) / cell).ok();
            println!("  {label:<26} advance {:?} cells", adv.map(|a| (a * 100.).round() / 100.));
        }
    }
}

pub struct FontSpike;

impl Render for FontSpike {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div().size_full().flex().flex_col().gap_2().p_4().bg(rgb(0x101010)).text_color(rgb(0xEDEDED));
        for family in FAMILIES {
            let f: SharedString = family.to_string().into();
            col = col
                .child(div().text_size(px(12.)).text_color(rgb(0x8A8A8A)).child(f.clone()))
                .child(div().font_family(f.clone()).text_size(px(FONT_SIZE)).child(format!("natural: {SAMPLE}")))
                .child(div().font_family(f.clone()).text_size(px(FONT_SIZE)).child(format!("natural: {ICON_SAMPLE}")))
                .child(grid_line(f.clone(), format!("forced:  {SAMPLE}")))
                .child(grid_line(f, format!("forced:  {ICON_SAMPLE}")));
        }
        col
    }
}

/// One line painted the terminal's way: one glyph per cell, plus a cell ruler.
fn grid_line(family: SharedString, text: String) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let ts = window.text_system().clone();
            let font: Font = font(family.clone());
            let font_id = ts.resolve_font(&font);
            let cell = ts.advance(font_id, px(FONT_SIZE), 'M').map(|s| s.width).unwrap_or(px(8.));
            for i in 0..text.chars().count() {
                if i % 2 == 0 {
                    let x = bounds.origin.x + cell * i as f32;
                    window.paint_quad(gpui::fill(
                        Bounds::new(gpui::point(x, bounds.origin.y), gpui::size(cell, px(LINE_HEIGHT))),
                        rgb(0x1E1E1E),
                    ));
                }
            }
            let run = run_for(&family, text.len());
            let shaped = ts.shape_line(text.clone().into(), px(FONT_SIZE), &[run], Some(cell));
            let _ = shaped.paint(bounds.origin, px(LINE_HEIGHT), window, cx);
        },
    )
    .w_full()
    .h(px(LINE_HEIGHT))
}
