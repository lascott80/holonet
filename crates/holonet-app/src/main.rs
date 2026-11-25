//! Holonet.
//!
//!   holonet                 the app: create or unlock the vault, then the workspace.
//!                           HOLONET_VAULT=<file> uses another vault file (testing).
//!   holonet --showcase [options]  terminal grid from canned output; options set
//!                           terminal settings for screenshots: light, contrast, bar,
//!                           underline, noblink, mono, propo, nl, nolig, size=N, lh=N
//!   holonet --gallery       milestone 2 phase 3: every shared control
//!   holonet --fonts         spike 1: font and glyph width check
//!   holonet --font-spike    milestone 3 phase 0: ligatures, icon widths, font load cost
//!   holonet --parse-bench   time parsing 10 MB of colored output, no window
//!   holonet --stream-bench  feed 10 MB through parser and renderer, then quit

mod app;
mod assets;
mod boxdraw;
mod font_check;
mod font_spike;
mod gallery;
mod host_form;
mod host_picker;
mod input;
mod platform;
mod sample;
mod session;
mod tab;
// Design-system modules: parts are used by later phases of milestone 2.
#[allow(dead_code)]
mod shell;
mod terminal;
mod terminal_view;
#[allow(dead_code)]
mod theme;
#[allow(dead_code)]
mod ui;
mod vault_screens;
mod workspace;

use std::time::Instant;

use gpui::{App, Application, Bounds, Pixels, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

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
    FONT_PREFERENCE.iter().find(|name| installed.iter().any(|f| f == *name)).copied().unwrap_or("Consolas")
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

/// Terminal settings from `--showcase` options, for screenshots before the
/// Settings page exists.
fn showcase_settings(args: impl Iterator<Item = String>) -> holonet_model::TerminalSettings {
    use holonet_model::{BundledFont, ColorScheme, CursorShape, TerminalFont};
    let mut s = holonet_model::TerminalSettings::default();
    for arg in args {
        match arg.as_str() {
            "light" => s.scheme = ColorScheme::Light,
            "contrast" => s.scheme = ColorScheme::HighContrast,
            "bar" => s.cursor = CursorShape::Bar,
            "underline" => s.cursor = CursorShape::Underline,
            "noblink" => s.cursor_blink = false,
            "mono" => s.font = TerminalFont::Bundled(BundledFont::NerdFontMono),
            "propo" => s.font = TerminalFont::Bundled(BundledFont::NerdFontPropo),
            "nl" => s.font = TerminalFont::Bundled(BundledFont::NerdFontNl),
            "nolig" => s.ligatures = false,
            other => {
                if let Some(n) = other.strip_prefix("size=").and_then(|n| n.parse().ok()) {
                    s.font_size = n;
                } else if let Some(n) = other.strip_prefix("lh=").and_then(|n| n.parse().ok()) {
                    s.line_height_tenths = n;
                } else if let Some(name) = other.strip_prefix("font=") {
                    s.font = TerminalFont::Installed(name.replace('_', " "));
                } else {
                    eprintln!("holonet: unknown showcase option {other}");
                }
            }
        }
    }
    s
}

fn main() {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--parse-bench") {
        parse_bench();
        return;
    }
    let fonts = arg.as_deref() == Some("--fonts");
    let gallery = arg.as_deref() == Some("--gallery");
    let font_spike = arg.as_deref() == Some("--font-spike");
    let showcase = arg.as_deref() == Some("--showcase");
    // --stream-bench: feed 10 MB a frame at a time, print the timing and quit.
    let stream = arg.as_deref() == Some("--stream-bench");
    // SSH runs on its own Tokio threads; GPUI keeps the main thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("holonet-ssh")
        .enable_all()
        .build()
        .expect("failed to start Tokio runtime");
    let handle = runtime.handle().clone();

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        assets::load_fonts(cx);
        theme::init_fonts(cx);
        ui::text_field::bind_keys(cx);
        ui::select::bind_keys(cx);
        ui::slider::bind_keys(cx);
        app::bind_keys(cx);
        host_picker::bind_keys(cx);
        if font_spike {
            match font_spike::load(cx) {
                Some((n, mb, read, add)) => {
                    println!("registered {n} files, {mb:.1} MB: read {read:.0} ms, add_fonts {add:.0} ms")
                }
                None => println!("HOLONET_SPIKE_FONTS not set: using installed fonts only"),
            }
            let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(1100.), px(900.)), cx);
            let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
            cx.open_window(options, |window, cx| {
                font_spike::report(window);
                cx.new(|_| font_spike::FontSpike)
            })
            .expect("failed to open window");
            cx.activate(true);
            return;
        }
        if gallery {
            let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(1280.), px(860.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Holonet".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(480.))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| cx.new(|cx| gallery::Gallery::new(window, cx)))
                .expect("failed to open window");
            cx.activate(true);
            return;
        }
        if arg.is_none() {
            let Some(path) = std::env::var_os("HOLONET_VAULT").map(Into::into).or_else(holonet_vault::default_path)
            else {
                eprintln!("holonet: no APPDATA folder to keep the vault in");
                cx.quit();
                return;
            };
            let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Holonet".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(520.))),
                ..Default::default()
            };
            let runtime = handle.clone();
            cx.open_window(options, |window, cx| cx.new(|cx| app::HolonetApp::new(path, runtime, window, cx)))
                .expect("failed to open window");
            cx.activate(true);
            return;
        }
        let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(1000.), px(640.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        if fonts {
            cx.open_window(options, |window, cx| cx.new(|_| font_check::FontCheck::new(window)))
                .expect("failed to open window");
        } else {
            cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut view = terminal_view::TerminalView::new(window, cx);
                    if showcase {
                        let settings = showcase_settings(std::env::args().skip(2));
                        view.apply_settings(&settings, window, cx);
                    }
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
