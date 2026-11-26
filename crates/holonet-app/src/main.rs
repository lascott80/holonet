//! Holonet.
//!
//!   holonet                 the app: create or unlock the vault, then the workspace.
//!                           HOLONET_VAULT=<file> uses another vault file (testing).
//!   holonet --showcase [options]  terminal grid from canned output; options set
//!                           terminal settings for screenshots: light, contrast, bar,
//!                           underline, noblink, mono, propo, nl, nolig, size=N, lh=N
//!   holonet --gallery       milestone 2 phase 3: every shared control
//!   holonet --fonts         spike 1: font and glyph width check
//!   holonet --parse-bench   time parsing 10 MB of colored output, no window
//!   holonet --find-bench    time find over a full 10,000-line history, no window
//!   holonet --stream-bench  feed 10 MB through parser and renderer, then quit
//!
//! Release builds are Windows apps, so no console opens with them. The modes
//! above attach to the console of the terminal they were started from.

// No console window for the release app; debug builds keep theirs for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod boxdraw;
mod files_view;
mod font_check;
mod forwards;
mod gallery;
mod hello;
mod history;
mod hops;
mod host_form;
mod host_picker;
mod input;
mod key_import;
mod platform;
mod sample;
mod session;
mod snippet_run;
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

/// Find over a full history, as the find bar runs it on each keystroke.
fn find_bench() {
    let mut model = TerminalModel::new(GridSize { cols: 150, rows: 40 }, 10_000);
    for chunk in sample::bulk(10_000_000).chunks(32 * 1024) {
        model.advance(chunk);
    }
    for (label, pattern) in [
        ("plain, every line", "(?i)status=200"),
        ("plain, few lines", "(?i)items/996 "),
        ("plain, no match", "(?i)nothing-here"),
        ("regex, every line", r"took=\d{3}ms"),
        ("case-sensitive", "INFO"),
    ] {
        let re = regex::Regex::new(pattern).unwrap();
        let t = Instant::now();
        let mut hits = Vec::new();
        for _ in 0..10 {
            hits = model.find(&re);
        }
        let per = t.elapsed().as_secs_f64() * 1e3 / 10.0;
        let t = Instant::now();
        model.set_hits(hits.clone(), hits.len().checked_sub(1));
        let _ = model.snapshot();
        let snap = t.elapsed().as_micros();
        println!("{label:<20} {:>6} matches  find {per:.2} ms  snapshot with highlights {snap} us", hits.len());
    }
}

/// Print to the terminal a release build was started from, when there is one.
/// Output that is piped or redirected already has its handles and is left
/// alone.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    // SAFETY: plain Win32 calls with no pointers.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    let arg = std::env::args().nth(1);
    #[cfg(windows)]
    if arg.is_some() {
        attach_console();
    }
    if arg.as_deref() == Some("--find-bench") {
        find_bench();
        return;
    }
    if arg.as_deref() == Some("--parse-bench") {
        parse_bench();
        return;
    }
    let fonts = arg.as_deref() == Some("--fonts");
    let gallery = arg.as_deref() == Some("--gallery");
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
        tab::bind_keys(cx);
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
