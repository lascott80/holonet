//! Milestone 5 phase 0, spike (b): drag and drop in GPUI on Windows. Two
//! lists ("This PC" and "Host"); rows drag from either list to the other, the
//! target outlines while a row is over it, and the Host list also accepts
//! files dropped from Windows Explorer (`ExternalPaths`). Every event is
//! printed and shown in the log at the bottom.
//!
//!   cargo run -p holonet-app --example m5_drag --release

use gpui::{
    App, Application, Bounds, Context, ExternalPaths, IntoElement, ParentElement, Render, SharedString, Styled, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};

/// What a dragged row carries: the side it came from and the names.
#[derive(Clone)]
struct Rows {
    from: &'static str,
    names: Vec<String>,
}

struct Preview(String);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().bg(rgb(0x2a2a2a)).text_color(rgb(0xededed)).rounded_md().child(self.0.clone())
    }
}

struct Spike {
    local: Vec<String>,
    host: Vec<String>,
    log: Vec<String>,
}

impl Spike {
    fn note(&mut self, line: String, cx: &mut Context<Self>) {
        println!("{line}");
        self.log.push(line);
        if self.log.len() > 8 {
            self.log.remove(0);
        }
        cx.notify();
    }

    fn list(&self, side: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let items = if side == "local" { &self.local } else { &self.host };
        let rows = items.iter().enumerate().map(|(i, name)| {
            let value = Rows { from: side, names: vec![name.clone()] };
            div()
                .id(SharedString::from(format!("{side}-{i}")))
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(0x2c2c2c))
                .hover(|s| s.bg(rgb(0x242424)))
                .child(name.clone())
                .on_drag(value, |rows: &Rows, _offset, _window, cx| {
                    let label = rows.names.join(", ");
                    cx.new(|_| Preview(label))
                })
        });
        let entity = cx.entity();
        let entity2 = cx.entity();
        div()
            .id(side)
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgb(0x2c2c2c))
            .rounded_md()
            .child(div().px_3().py_2().text_color(rgb(0xa3a3a3)).child(if side == "local" {
                "This PC"
            } else {
                "Host"
            }))
            .children(rows)
            // Outline while a row from the other side is over this list.
            .drag_over::<Rows>(move |style, rows, _, _| {
                if rows.from != side { style.border_color(rgb(0x5cc8ff)).bg(rgb(0x14202a)) } else { style }
            })
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                if side == "host" { style.border_color(rgb(0x5cc8ff)).bg(rgb(0x14202a)) } else { style }
            })
            .on_drop(move |rows: &Rows, _, cx| {
                entity.update(cx, |this, cx| {
                    if rows.from == side {
                        this.note(format!("dropped {:?} back on {side}: ignored", rows.names), cx);
                        return;
                    }
                    let target = if side == "local" { &mut this.local } else { &mut this.host };
                    target.extend(rows.names.iter().cloned());
                    this.note(format!("row drop: {:?} from {} onto {side}", rows.names, rows.from), cx);
                })
            })
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                entity2.update(cx, |this, cx| {
                    let names: Vec<String> = paths.paths().iter().map(|p| p.display().to_string()).collect();
                    if side == "host" {
                        this.host.extend(names.iter().map(|n| format!("(from Explorer) {n}")));
                    }
                    this.note(format!("Explorer drop onto {side}: {names:?}"), cx);
                })
            })
    }
}

impl Render for Spike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x111111))
            .text_color(rgb(0xededed))
            .child(div().flex_1().min_h_0().flex().gap_3().child(self.list("local", cx)).child(self.list("host", cx)))
            .child(
                div()
                    .h(px(170.))
                    .p_2()
                    .bg(rgb(0x0b0b0b))
                    .text_color(rgb(0xa3a3a3))
                    .children(self.log.iter().map(|l| div().child(SharedString::from(l.clone())))),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| {
                cx.new(|_| Spike {
                    local: ["index.html", "README.md", "assets.tar.gz"].map(String::from).to_vec(),
                    host: ["nginx.conf", "deploy.sh"].map(String::from).to_vec(),
                    log: vec!["Drag rows between the lists, or drop files from Explorer on Host.".into()],
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
