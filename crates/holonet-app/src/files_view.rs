//! The SFTP tab's content (Sftp.dc.html): this PC on the left, the host on
//! the right. Milestone 5 phase 4 lists both sides' starting folders; phase 5
//! adds browsing, selection and dragging, and phase 6 the transfer queue.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use holonet_files::remote::Remote;
use holonet_files::{Entry, Kind, RemotePath, format, local};
use holonet_model::Host;

use crate::theme::{self, TypeStyles};
use crate::ui::icon;

/// Rows drawn before the phase 5 list takes over.
const MAX_ROWS: usize = 400;

enum Listing {
    Loading,
    Ready(Vec<Entry>),
    Failed(SharedString),
}

pub struct FilesView {
    host_name: SharedString,
    runtime: tokio::runtime::Handle,
    remote: Option<Arc<Remote>>,
    /// The host's "Default SFTP folder", tried once on the first connection.
    start_folder: Option<String>,
    remote_dir: Option<RemotePath>,
    remote_list: Listing,
    local_dir: PathBuf,
    local_list: Listing,
}

/// The icon for an entry (Sftp.dc.html's kinds, chosen by extension).
pub fn entry_icon(entry: &Entry) -> (&'static str, gpui::Rgba) {
    match entry.kind {
        Kind::Folder => ("folder", theme::accent()),
        Kind::Link { folder: true, .. } => ("folder", theme::group_violet()),
        Kind::Link { .. } => ("file-link", theme::group_violet()),
        _ => {
            let lower = entry.name.to_lowercase();
            let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            if lower.starts_with(".env") || lower.starts_with(".htpasswd") || ext == "key" || ext == "pem" {
                ("file-lock", theme::ink_muted())
            } else if matches!(ext, "gz" | "tgz" | "zip" | "tar" | "xz" | "bz2" | "7z" | "zst") {
                ("file-archive", theme::ink_soft())
            } else if matches!(
                ext,
                "html" | "css" | "js" | "ts" | "rs" | "py" | "sh" | "ps1" | "go" | "c" | "h" | "cpp" | "java" | "rb"
            ) {
                ("file-code", theme::group_mint())
            } else if matches!(ext, "toml" | "conf" | "yaml" | "yml" | "json" | "ini" | "cfg" | "xml") {
                ("file-config", theme::ink_muted())
            } else if matches!(ext, "md" | "txt" | "log" | "csv") {
                ("file-text", theme::ink_muted())
            } else {
                ("file", theme::ink_muted())
            }
        }
    }
}

impl FilesView {
    pub fn new(host: &Host, runtime: tokio::runtime::Handle, _cx: &mut Context<Self>) -> Self {
        let local_dir =
            host.sftp_local_folder.as_ref().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(local::profile);
        let local_list = match local::list(&local_dir) {
            Ok(mut list) => {
                holonet_files::sort(&mut list);
                Listing::Ready(list)
            }
            Err(e) => Listing::Failed(e.to_string().into()),
        };
        Self {
            host_name: host.display_name().to_string().into(),
            runtime,
            remote: None,
            start_folder: host.sftp_folder.clone().filter(|f| !f.trim().is_empty()),
            remote_dir: None,
            remote_list: Listing::Loading,
            local_dir,
            local_list,
        }
    }

    /// The SFTP connection, or `None` while the tab reconnects. The host's
    /// side keeps its folder across reconnects.
    pub fn set_remote(&mut self, remote: Option<Arc<Remote>>, cx: &mut Context<Self>) {
        self.remote = remote;
        if self.remote.is_some() {
            self.load_remote(cx);
        }
        cx.notify();
    }

    fn load_remote(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.clone() else { return };
        let (dir, start) = (self.remote_dir.clone(), self.start_folder.take());
        self.remote_list = Listing::Loading;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let dir = match (dir, start) {
                (Some(dir), _) => dir,
                // The Default SFTP folder, or home if it cannot be used.
                (None, Some(start)) => {
                    remote.canonicalize(start.trim().as_bytes()).await.unwrap_or_else(|_| remote.home().clone())
                }
                (None, None) => remote.home().clone(),
            };
            let listing = remote.list(&dir).await;
            let _ = tx.send((dir, listing));
        });
        cx.spawn(async move |this, cx| {
            let Ok((dir, listing)) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.remote_dir = Some(dir);
                this.remote_list = match listing {
                    Ok(mut list) => {
                        holonet_files::sort(&mut list);
                        Listing::Ready(list)
                    }
                    Err(e) => Listing::Failed(e.to_string().into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn pane(&self, id: &'static str, title: SharedString, path: SharedString, listing: &Listing) -> impl IntoElement {
        let body = match listing {
            Listing::Loading => div().p(theme::SPACE_3).text_body().text_color(theme::ink_muted()).child("Loading…"),
            Listing::Failed(why) => div().p(theme::SPACE_3).text_body().text_color(theme::warn()).child(why.clone()),
            Listing::Ready(list) if list.is_empty() => {
                div().p(theme::SPACE_3).text_body().text_color(theme::ink_muted()).child("This folder is empty.")
            }
            Listing::Ready(list) => div().flex().flex_col().children(list.iter().take(MAX_ROWS).map(|e| {
                let (name, color) = entry_icon(e);
                div()
                    .h(px(36.))
                    .px(theme::SPACE_3)
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .border_b_1()
                    .border_color(theme::line())
                    .child(icon(name, px(16.), color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_body()
                            .text_color(theme::ink())
                            .child(SharedString::from(e.name.clone())),
                    )
                    .child(div().w(px(90.)).flex_none().text_mono_small().text_color(theme::ink_muted()).child(
                        SharedString::from(if e.kind.opens() { "Folder".to_string() } else { format::size(e.size) }),
                    ))
            })),
        };
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_MD)
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .px(theme::SPACE_3)
                    .py(theme::SPACE_2)
                    .border_b_1()
                    .border_color(theme::line())
                    .bg(theme::panel())
                    .flex()
                    .flex_col()
                    .child(
                        div().text_body().font_weight(gpui::FontWeight::MEDIUM).text_color(theme::ink()).child(title),
                    )
                    .child(div().text_mono_small().text_color(theme::ink_muted()).truncate().child(path)),
            )
            .child(
                div().id(SharedString::from(format!("{id}-rows"))).flex_1().min_h_0().overflow_y_scroll().child(body),
            )
    }
}

impl Render for FilesView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let remote_path: SharedString = self.remote_dir.as_ref().map(|d| d.to_string()).unwrap_or_default().into();
        div()
            .size_full()
            .flex()
            .gap(theme::SPACE_2)
            .p(theme::SPACE_2)
            .child(self.pane(
                "files-local",
                "This PC".into(),
                self.local_dir.display().to_string().into(),
                &self.local_list,
            ))
            .child(self.pane("files-remote", self.host_name.clone(), remote_path, &self.remote_list))
    }
}
