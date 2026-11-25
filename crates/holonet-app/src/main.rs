//! Holonet prototype (milestone 1).
//!
//!   holonet                 spike 3: terminal grid from canned output
//!   holonet --fonts         spike 1: font and glyph width check
//!   holonet --parse-bench   time parsing 10 MB of colored output, no window
//!   holonet --stream-bench  feed 10 MB through parser and renderer, then quit

mod font_check;
mod sample;
mod terminal;
mod terminal_view;

use std::time::Instant;

use gpui::{App, Application, Bounds, Pixels, WindowBounds, WindowOptions, prelude::*, px, size};

use terminal::{GridSize, TerminalModel};

/// Preferred terminal fonts, most wanted first (docs/decisions.md).
pub const FONT_PREFERENCE: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "JetBrainsMonoNL Nerd Font Mono",
    "Cascadia Mono",
    "Consolas",
];

/// First preferred font that is installed.
pub fn pick_font(installed: &[String]) -> &'static str {
    FONT_PREFERENCE
        .iter()
        .find(|name| installed.iter().any(|f| f == *name))
        .copied()
        .unwrap_or("Consolas")
}

fn parse_bench() {
    let data = sample::bulk(10_000_000);
    let mut model = TerminalModel::new(GridSize { cols: 120, rows: 40 }, 10_000);
    let t = Instant::now();
    for chunk in data.chunks(32 * 1024) {
        model.advance(chunk);
    }
    let parse = t.elapsed();
    let t = Instant::now();
    let snap = model.snapshot();
    let snapshot = t.elapsed();
    println!(
        "parsed {:.1} MB in {:.0} ms ({:.0} MB/s); snapshot of {} lines in {} us; history {} lines",
        data.len() as f64 / 1e6,
        parse.as_secs_f64() * 1e3,
        data.len() as f64 / 1e6 / parse.as_secs_f64(),
        snap.lines.len(),
        snapshot.as_micros(),
        snap.history
    );
}

fn main() {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--parse-bench") {
        parse_bench();
        return;
    }
    let fonts = arg.as_deref() == Some("--fonts");
    // --stream-bench: feed 10 MB a frame at a time, print the timing and quit.
    let stream = arg.as_deref() == Some("--stream-bench");

    Application::new().run(move |cx: &mut App| {
        let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(1000.), px(640.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        if fonts {
            cx.open_window(options, |window, cx| cx.new(|_| font_check::FontCheck::new(window)))
                .expect("failed to open window");
        } else {
            cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut view = terminal_view::TerminalView::new(window, cx);
                    view.feed(&sample::showcase());
                    if stream {
                        view.stream(sample::bulk(10_000_000), true);
                    }
                    view
                })
            })
            .expect("failed to open window");
        }
        cx.activate(true);
    });
}
