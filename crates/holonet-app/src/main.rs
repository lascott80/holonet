//! Holonet.
//!
//!   holonet                 the app: create or unlock the vault, then the workspace.
//!                           HOLONET_VAULT=<file> uses another vault file (testing).
//!   holonet --showcase      spike 3: terminal grid from canned output
//!   holonet --ssh [mode]    spike 4: shell on the WSL test server; mode is
//!                           password, ed25519 (default), rsa or passphrase
//!   holonet --gallery       milestone 2 phase 3: every shared control
//!   holonet --fonts         spike 1: font and glyph width check
//!   holonet --parse-bench   time parsing 10 MB of colored output, no window
//!   holonet --stream-bench  feed 10 MB through parser and renderer, then quit

mod app;
mod assets;
mod font_check;
mod gallery;
mod input;
mod sample;
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

fn main() {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--parse-bench") {
        parse_bench();
        return;
    }
    let fonts = arg.as_deref() == Some("--fonts");
    let gallery = arg.as_deref() == Some("--gallery");
    let showcase = arg.as_deref() == Some("--showcase");
    // --stream-bench: feed 10 MB a frame at a time, print the timing and quit.
    let stream = arg.as_deref() == Some("--stream-bench");
    let ssh = if arg.as_deref() == Some("--ssh") {
        let mode = std::env::args().nth(2).unwrap_or_else(|| "ed25519".into());
        match holonet_ssh::dev::options(&mode) {
            Ok(opts) => Some(opts),
            Err(err) => {
                eprintln!("holonet: {err:#}");
                std::process::exit(2);
            }
        }
    } else {
        None
    };
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
            cx.open_window(options, |window, cx| cx.new(|cx| app::HolonetApp::new(path, window, cx)))
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
                    if let Some(opts) = ssh.clone() {
                        view.connect(handle.clone(), opts, cx);
                        return view;
                    }
                    let _ = showcase;
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
