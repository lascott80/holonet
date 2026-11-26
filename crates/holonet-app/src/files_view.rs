//! The SFTP tab's content (Sftp.dc.html): this PC on the left, the host on
//! the right, and the transfers strip under them.
//!
//! Each side ([`FilePane`]) has a place (the drives of "This PC", a folder on
//! this PC, or a folder on the host), Back, Up and Refresh, breadcrumbs, a
//! filter, and a list drawn with `uniform_list` so large folders stay smooth.
//! Clicking a row toggles it, Shift click selects a range, the header box
//! selects every shown row, and a double click opens a folder (or a link to
//! one). A selection shows the action bar: Upload or Download, and Copy path.
//!
//! Rows drag to the other side (the selection, when the row is part of it),
//! onto the pane (its current folder) or onto a folder row (that folder).
//! The host's side also takes files dropped from Explorer. Transfers go to
//! the tab's [`Engine`]; the queue panel and the conflict popup come in
//! milestone 5 phase 6, and until then "Ask" skips names already there.
//! Rename, delete, permissions and new folder come in phase 7.

use std::any::Any;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EntityId, ExternalPaths, FontWeight, IntoElement,
    MouseButton, MouseDownEvent, Render, Rgba, ScrollStrategy, SharedString, Stateful, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use holonet_files::engine::{Engine, EngineEvent, JobState};
use holonet_files::plan::{self, Choice};
use holonet_files::remote::Remote;
use holonet_files::{Entry, FileError, Kind, RemotePath, format, local};
use holonet_model::{Host, HostId, OnConflict, SftpSettings};

use crate::app::HolonetApp;
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, TextField, TextFieldEvent, icon};

/// Column widths (Sftp.dc.html's grid).
const CHECK_W: f32 = 28.;
const LOCAL_SIZE_W: f32 = 90.;
const LOCAL_MODIFIED_W: f32 = 140.;
const REMOTE_SIZE_W: f32 = 80.;
const REMOTE_MODIFIED_W: f32 = 110.;
const PERMISSIONS_W: f32 = 100.;
const COLUMN_GAP: f32 = 10.;
const ROW_PAD: f32 = 14.;

/// How long after a transfer finishes the panes refresh (several finishing
/// together refresh once).
const REFRESH_AFTER: Duration = Duration::from_millis(400);

/// Breadcrumbs shown after the first when a path is long.
const CRUMBS_KEPT: usize = 3;

/// A second click this soon after the first opens the first click's row.
const DOUBLE_CLICK: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

impl Side {
    fn other(self) -> Side {
        match self {
            Side::Local => Side::Remote,
            Side::Remote => Side::Local,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Side::Local => "local",
            Side::Remote => "remote",
        }
    }
}

/// Where a side is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    /// "This PC": the drives.
    Drives,
    Local(PathBuf),
    Remote(RemotePath),
}

impl Place {
    /// The place an entry of this one opens.
    fn child(&self, entry: &Entry) -> Place {
        match self {
            // A drive's entry carries its root ("C:\") as its raw name.
            Place::Drives => Place::Local(PathBuf::from(String::from_utf8_lossy(&entry.raw_name).into_owned())),
            Place::Local(dir) => Place::Local(dir.join(&entry.name)),
            Place::Remote(dir) => Place::Remote(dir.join(&entry.raw_name)),
        }
    }

    /// One folder up; a drive's root goes up to "This PC".
    fn parent(&self) -> Option<Place> {
        match self {
            Place::Drives => None,
            Place::Local(dir) => Some(dir.parent().map(|p| Place::Local(p.to_path_buf())).unwrap_or(Place::Drives)),
            Place::Remote(dir) => dir.parent().map(Place::Remote),
        }
    }

    fn display(&self) -> String {
        match self {
            Place::Drives => "This PC".into(),
            Place::Local(dir) => dir.display().to_string(),
            Place::Remote(dir) => dir.to_string(),
        }
    }

    /// Breadcrumbs: "This PC › C: › Users › …" or "/ › home › …".
    fn crumbs(&self) -> Vec<(String, Place)> {
        match self {
            Place::Drives => vec![("This PC".into(), Place::Drives)],
            Place::Local(dir) => {
                let mut out = vec![("This PC".to_string(), Place::Drives)];
                let mut parts: Vec<&Path> = dir.ancestors().collect();
                parts.reverse();
                for part in parts {
                    let label = match part.file_name() {
                        Some(name) => name.to_string_lossy().into_owned(),
                        None => part.display().to_string().trim_end_matches('\\').to_string(),
                    };
                    out.push((label, Place::Local(part.to_path_buf())));
                }
                out
            }
            Place::Remote(dir) => dir.components().into_iter().map(|(label, p)| (label, Place::Remote(p))).collect(),
        }
    }

    /// A folder files can be put in (not "This PC" itself).
    fn is_folder(&self) -> bool {
        !matches!(self, Place::Drives)
    }
}

enum Listing {
    Loading,
    Ready(Vec<Entry>),
    Failed(SharedString),
}

/// How a load moves through the history.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// To a new place; the old one goes on the Back list.
    Push,
    /// Back: nothing is pushed.
    Back,
    /// The same place again, keeping the list shown until the new one comes.
    Quiet,
}

/// One side of the screen.
struct FilePane {
    place: Option<Place>,
    back: Vec<Place>,
    listing: Listing,
    /// Indices into the listing that the filter and hidden files let through.
    visible: Vec<usize>,
    filter: Entity<TextField>,
    /// Selected rows by raw name.
    selected: HashSet<Vec<u8>>,
    /// The last row clicked, for Shift click.
    anchor: Option<Vec<u8>>,
    scroll: UniformListScrollHandle,
    /// The latest load; older answers are dropped.
    seq: u64,
}

impl FilePane {
    fn entries(&self) -> &[Entry] {
        match &self.listing {
            Listing::Ready(list) => list,
            _ => &[],
        }
    }

    fn at_drives(&self) -> bool {
        self.place == Some(Place::Drives)
    }

    /// The selected entries, in list order.
    fn selection(&self) -> Vec<&Entry> {
        self.entries().iter().filter(|e| self.selected.contains(&e.raw_name)).collect()
    }
}

/// What a dragged row carries.
#[derive(Clone)]
pub struct DraggedFiles {
    view: EntityId,
    from: Side,
    items: Items,
    label: SharedString,
}

#[derive(Clone)]
enum Items {
    Local(Vec<PathBuf>),
    Remote(Vec<RemotePath>),
}

/// The chip under the pointer while dragging.
struct DragChip(SharedString);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(theme::SPACE_3)
            .py(theme::SPACE_1)
            .bg(theme::raised())
            .border_1()
            .border_color(theme::line_strong())
            .rounded(theme::RADIUS_SM)
            .text_body()
            .text_color(theme::ink())
            .child(self.0.clone())
    }
}

pub struct FilesView {
    app: WeakEntity<HolonetApp>,
    host_id: HostId,
    host_name: SharedString,
    runtime: tokio::runtime::Handle,
    remote: Option<Arc<Remote>>,
    /// The host's "Default SFTP folder", tried once on the first connection.
    start_folder: Option<String>,
    settings: SftpSettings,
    /// The local folder last saved on the host (`Host::sftp_local_folder`).
    saved_local: Option<PathBuf>,
    local: FilePane,
    remote_side: FilePane,
    engine: Engine,
    summary: SharedString,
    queued_any: bool,
    done_count: usize,
    refresh_pending: bool,
    /// The last thing to report (skipped items, a copied path, a failure).
    note: Option<SharedString>,
    /// The side a row drag started from, while it lasts.
    dragging: Option<Side>,
    /// The row a click last toggled. The action bar appearing moves the
    /// rows, so a double click opens this row, not the one now under the
    /// pointer.
    last_click: Option<(Side, Vec<u8>, Instant)>,
    _subscriptions: Vec<Subscription>,
}

/// The icon for an entry (Sftp.dc.html's kinds, chosen by extension).
pub fn entry_icon(entry: &Entry) -> (&'static str, Rgba) {
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

/// "24 Nov 2026", or "24 Nov 2026 16:02", in local time.
fn format_modified(seconds: Option<i64>, with_time: bool) -> String {
    let Some(seconds) = seconds else { return String::new() };
    let pattern = if with_time { "%-d %b %Y %H:%M" } else { "%-d %b %Y" };
    jiff::Timestamp::from_second(seconds)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime(pattern).to_string())
        .unwrap_or_default()
}

/// The drives as entries: the name is the label, the raw name the root.
fn drive_entries() -> Vec<Entry> {
    local::drives()
        .into_iter()
        .map(|d| Entry {
            name: d.display(),
            raw_name: d.root.to_string_lossy().into_owned().into_bytes(),
            kind: Kind::Folder,
            size: 0,
            modified: None,
            mode: None,
            uid: None,
            gid: None,
            hidden: false,
        })
        .collect()
}

/// A 16px checkbox square (Sftp.dc.html's row and header boxes).
fn check_box(checked: bool) -> gpui::Div {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_XS)
        .border_1()
        .when(checked, |b| b.bg(theme::accent()).border_color(theme::accent()))
        .when(!checked, |b| b.bg(theme::card()).border_color(theme::line_strong()))
        .when(checked, |b| b.child(icon("check", px(12.), theme::on_accent())))
}

/// A 36px icon button in a pane's navigation row.
fn nav_button(
    id: SharedString,
    name: &'static str,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(theme::CONTROL_COMPACT)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .child(icon(name, px(16.), if enabled { theme::ink_muted() } else { theme::ink_faint() }))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(theme::raised())).on_click(handler))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

impl FilesView {
    pub fn new(
        host: &Host,
        runtime: tokio::runtime::Handle,
        app: WeakEntity<HolonetApp>,
        settings: SftpSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let mut side = |side: Side, cx: &mut Context<Self>| {
            let filter = cx.new(|cx| TextField::new(cx, "Filter").compact().leading_icon("search"));
            subscriptions.push(cx.subscribe(&filter, move |this: &mut Self, _, event: &TextFieldEvent, cx| {
                if *event == TextFieldEvent::Changed {
                    this.refilter(side, cx);
                    cx.notify();
                }
            }));
            FilePane {
                place: None,
                back: Vec::new(),
                listing: Listing::Loading,
                visible: Vec::new(),
                filter,
                selected: HashSet::new(),
                anchor: None,
                scroll: UniformListScrollHandle::new(),
                seq: 0,
            }
        };
        let local_side = side(Side::Local, cx);
        let remote_side = side(Side::Remote, cx);

        // The engine reports on a channel; the view redraws from snapshots.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine::new(runtime.clone(), settings.at_once.count(), tx);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                if this.update(cx, |this, cx| this.engine_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let saved_local = host.sftp_local_folder.as_ref().map(PathBuf::from);
        let start_local = saved_local.clone().filter(|p| p.is_dir()).unwrap_or_else(local::profile);
        let mut view = Self {
            app,
            host_id: host.id,
            host_name: host.display_name().to_string().into(),
            runtime,
            remote: None,
            start_folder: host.sftp_folder.clone().filter(|f| !f.trim().is_empty()),
            settings,
            saved_local,
            local: local_side,
            remote_side,
            engine,
            summary: SharedString::default(),
            queued_any: false,
            done_count: 0,
            refresh_pending: false,
            note: None,
            dragging: None,
            last_click: None,
            _subscriptions: subscriptions,
        };
        view.load(Side::Local, Place::Local(start_local), Nav::Push, cx);
        view
    }

    fn side(&self, side: Side) -> &FilePane {
        match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote_side,
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut FilePane {
        match side {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote_side,
        }
    }

    /// Settings, SFTP changed.
    pub fn set_settings(&mut self, settings: SftpSettings, cx: &mut Context<Self>) {
        self.engine.set_at_once(settings.at_once.count());
        self.settings = settings;
        self.refilter(Side::Local, cx);
        self.refilter(Side::Remote, cx);
        cx.notify();
    }

    /// The SFTP connection, or `None` while the tab reconnects. The host's
    /// side keeps its folder across reconnects, and interrupted transfers
    /// continue on the new connection.
    pub fn set_remote(&mut self, remote: Option<Arc<Remote>>, cx: &mut Context<Self>) {
        self.remote = remote.clone();
        self.engine.set_remote(remote.clone());
        let Some(remote) = remote else {
            cx.notify();
            return;
        };
        match self.remote_side.place.clone() {
            Some(place) => self.load(Side::Remote, place, Nav::Quiet, cx),
            None => {
                // The Default SFTP folder, or home if it cannot be used.
                let start = self.start_folder.take();
                let pane = &mut self.remote_side;
                pane.seq += 1;
                let seq = pane.seq;
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.runtime.spawn(async move {
                    let dir = match start {
                        Some(start) => {
                            remote.canonicalize(start.trim().as_bytes()).await.unwrap_or_else(|_| remote.home().clone())
                        }
                        None => remote.home().clone(),
                    };
                    let listing = remote.list(&dir).await;
                    let _ = tx.send((dir, listing));
                });
                cx.spawn(async move |this, cx| {
                    let Ok((dir, listing)) = rx.await else { return };
                    let _ = this.update(cx, |this, cx| {
                        this.loaded(Side::Remote, seq, Place::Remote(dir), Nav::Push, listing, cx)
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------ loading --

    fn load(&mut self, side: Side, place: Place, nav: Nav, cx: &mut Context<Self>) {
        let remote = self.remote.clone();
        if matches!(place, Place::Remote(_)) && remote.is_none() {
            return;
        }
        let pane = self.side_mut(side);
        pane.seq += 1;
        let seq = pane.seq;
        if nav != Nav::Quiet {
            pane.listing = Listing::Loading;
            pane.visible.clear();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        match place.clone() {
            Place::Drives => {
                self.runtime.spawn_blocking(move || {
                    let _ = tx.send(Ok(drive_entries()));
                });
            }
            Place::Local(dir) => {
                self.runtime.spawn_blocking(move || {
                    let _ = tx.send(local::list(&dir));
                });
            }
            Place::Remote(dir) => {
                let remote = remote.expect("checked above");
                self.runtime.spawn(async move {
                    let _ = tx.send(remote.list(&dir).await);
                });
            }
        }
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| this.loaded(side, seq, place, nav, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn loaded(
        &mut self,
        side: Side,
        seq: u64,
        place: Place,
        nav: Nav,
        result: Result<Vec<Entry>, FileError>,
        cx: &mut Context<Self>,
    ) {
        let pane = self.side_mut(side);
        if pane.seq != seq {
            return;
        }
        let moved = pane.place.as_ref() != Some(&place);
        if moved {
            if nav == Nav::Push
                && let Some(old) = pane.place.take()
            {
                pane.back.push(old);
            }
            pane.selected.clear();
            pane.anchor = None;
            pane.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        let ok = result.is_ok();
        pane.listing = match result {
            Ok(mut list) => {
                // Drives stay in letter order.
                if place != Place::Drives {
                    holonet_files::sort(&mut list);
                }
                Listing::Ready(list)
            }
            Err(e) => Listing::Failed(e.to_string().into()),
        };
        pane.place = Some(place.clone());
        if moved {
            let filter = pane.filter.clone();
            filter.update(cx, |f, cx| f.set_text("", cx));
        }
        self.refilter(side, cx);
        if let (true, Place::Local(dir)) = (ok, &place) {
            self.remember_local(dir, cx);
        }
        cx.notify();
    }

    /// Recompute the shown rows; a selection the filter hides is dropped.
    fn refilter(&mut self, side: Side, cx: &mut Context<Self>) {
        let show_hidden = self.settings.show_hidden;
        let pane = self.side_mut(side);
        let query = pane.filter.read(cx).text().trim().to_lowercase();
        let visible: Vec<usize> = pane
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| (show_hidden || !e.hidden) && (query.is_empty() || e.name.to_lowercase().contains(&query)))
            .map(|(i, _)| i)
            .collect();
        let shown: HashSet<&Vec<u8>> = visible.iter().map(|i| &pane.entries()[*i].raw_name).collect();
        let selected: HashSet<Vec<u8>> = pane.selected.iter().filter(|n| shown.contains(n)).cloned().collect();
        pane.selected = selected;
        pane.visible = visible;
    }

    /// Keep the local folder on the host, so the next SFTP tab starts there.
    fn remember_local(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.saved_local.as_deref() == Some(dir) {
            return;
        }
        self.saved_local = Some(dir.to_path_buf());
        let (id, folder) = (self.host_id, dir.display().to_string());
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.update_vault(
                    move |d| {
                        if let Some(h) = d.host_mut(id) {
                            h.sftp_local_folder = Some(folder);
                        }
                    },
                    cx,
                )
            });
        }
    }

    fn open(&mut self, side: Side, place: Place, cx: &mut Context<Self>) {
        self.load(side, place, Nav::Push, cx);
    }

    fn go_back(&mut self, side: Side, cx: &mut Context<Self>) {
        if let Some(place) = self.side_mut(side).back.pop() {
            self.load(side, place, Nav::Back, cx);
        }
    }

    fn go_up(&mut self, side: Side, cx: &mut Context<Self>) {
        if let Some(place) = self.side(side).place.as_ref().and_then(Place::parent) {
            self.load(side, place, Nav::Push, cx);
        }
    }

    fn refresh(&mut self, side: Side, cx: &mut Context<Self>) {
        if let Some(place) = self.side(side).place.clone() {
            self.load(side, place, Nav::Quiet, cx);
        }
    }

    // ---------------------------------------------------------- selection --

    fn row_click(&mut self, side: Side, row: usize, event: &ClickEvent, cx: &mut Context<Self>) {
        let pane = self.side_mut(side);
        let Some(&index) = pane.visible.get(row) else { return };
        let entry = pane.entries()[index].clone();
        // "This PC" has no selection: a click opens the drive.
        if pane.at_drives() && event.click_count() == 1 {
            if let Some(place) = pane.place.as_ref().map(|p| p.child(&entry)) {
                self.open(side, place, cx);
            }
            return;
        }
        // The pane's mouse down handles the second click ([`Self::double_click`]).
        if event.click_count() >= 2 {
            return;
        }
        self.last_click = Some((side, entry.raw_name.clone(), Instant::now()));
        let pane = self.side_mut(side);
        let anchor_row =
            pane.anchor.as_ref().and_then(|a| pane.visible.iter().position(|i| &pane.entries()[*i].raw_name == a));
        match anchor_row {
            Some(from) if event.modifiers().shift => {
                let (lo, hi) = if from <= row { (from, row) } else { (row, from) };
                let names: Vec<Vec<u8>> =
                    pane.visible[lo..=hi].iter().map(|i| pane.entries()[*i].raw_name.clone()).collect();
                pane.selected.extend(names);
            }
            _ => {
                if !pane.selected.remove(&entry.raw_name) {
                    pane.selected.insert(entry.raw_name.clone());
                }
                pane.anchor = Some(entry.raw_name);
            }
        }
        cx.notify();
    }

    /// A second click anywhere in the pane: open the row the first click hit,
    /// if it is a folder or a link to one.
    fn double_click(&mut self, side: Side, cx: &mut Context<Self>) {
        let Some((clicked, name, at)) = self.last_click.take() else { return };
        if clicked != side || at.elapsed() > DOUBLE_CLICK {
            return;
        }
        let pane = self.side(side);
        let Some(entry) = pane.entries().iter().find(|e| e.raw_name == name) else { return };
        if !entry.kind.opens() {
            return;
        }
        if let Some(place) = pane.place.as_ref().map(|p| p.child(entry)) {
            self.open(side, place, cx);
        }
    }

    fn toggle_all(&mut self, side: Side, cx: &mut Context<Self>) {
        let pane = self.side_mut(side);
        let all = !pane.visible.is_empty() && pane.selected.len() == pane.visible.len();
        if all {
            pane.selected.clear();
        } else {
            let names: Vec<Vec<u8>> = pane.visible.iter().map(|i| pane.entries()[*i].raw_name.clone()).collect();
            pane.selected = names.into_iter().collect();
        }
        cx.notify();
    }

    fn clear_selection(&mut self, side: Side, cx: &mut Context<Self>) {
        let pane = self.side_mut(side);
        pane.selected.clear();
        pane.anchor = None;
        cx.notify();
    }

    /// The items of these entries, at this side's place.
    fn items_of(&self, side: Side, entries: &[&Entry]) -> Option<Items> {
        match self.side(side).place.as_ref()? {
            Place::Local(dir) => Some(Items::Local(entries.iter().map(|e| dir.join(&e.name)).collect())),
            Place::Remote(dir) => Some(Items::Remote(entries.iter().map(|e| dir.join(&e.raw_name)).collect())),
            Place::Drives => None,
        }
    }

    fn copy_paths(&mut self, side: Side, cx: &mut Context<Self>) {
        let entries = self.side(side).selection();
        let text = match self.items_of(side, &entries) {
            Some(Items::Local(paths)) => paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            Some(Items::Remote(paths)) => paths.iter().map(|p| p.to_string()).collect(),
            None => return,
        };
        let n = text.len();
        cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n")));
        self.note = Some(format!("Copied {}.", plural(n, "path", "paths")).into());
        cx.notify();
    }

    // ---------------------------------------------------------- transfers --

    /// Whether a drop or send onto `side` can land.
    fn can_receive(&self, side: Side) -> bool {
        self.remote.is_some() && self.side(side).place.as_ref().is_some_and(Place::is_folder)
    }

    /// The action bar's Upload or Download: the selection to the other
    /// side's current folder.
    fn send(&mut self, from: Side, cx: &mut Context<Self>) {
        let to = from.other();
        let (Some(dest), Some(items)) =
            (self.side(to).place.clone(), self.items_of(from, &self.side(from).selection()))
        else {
            return;
        };
        self.clear_selection(from, cx);
        self.transfer(items, dest, cx);
    }

    /// Plan, create folders and queue. Names already at the destination
    /// follow Settings' "When a file already exists"; Ask skips them until
    /// the conflict popup (milestone 5 phase 6).
    fn transfer(&mut self, items: Items, dest: Place, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.clone() else { return };
        let choice = match self.settings.on_conflict {
            OnConflict::Ask => Choice::Skip,
            OnConflict::Overwrite => Choice::Overwrite,
            OnConflict::KeepBoth => Choice::KeepBoth,
        };
        let engine = self.engine.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let planned = match (items, &dest) {
                (Items::Local(paths), Place::Remote(dir)) => plan::plan_upload(&remote, &paths, dir).await,
                (Items::Remote(paths), Place::Local(dir)) => plan::plan_download(&remote, &paths, dir).await,
                _ => return,
            };
            let outcome = async {
                let plan = planned?.resolve(choice);
                plan.create_folders(&remote).await?;
                let files = plan.jobs();
                let queued = files.len();
                engine.add(files);
                Ok::<_, FileError>((queued, plan.skipped.clone()))
            }
            .await;
            let _ = tx.send(outcome);
        });
        cx.spawn(async move |this, cx| {
            let Ok(outcome) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.note = match outcome {
                    Ok((_, skipped)) if !skipped.is_empty() => {
                        let (name, why) = &skipped[0];
                        Some(match skipped.len() {
                            1 => format!("Skipped {name}: {why}."),
                            n => format!("Skipped {n} items, such as {name}: {why}."),
                        })
                    }
                    Ok((0, _)) => Some("Nothing to transfer: the folders are empty.".into()),
                    Ok(_) => None,
                    Err(e) => Some(format!("The transfer could not start: {e}")),
                }
                .map(SharedString::from);
                this.queued_any = true;
                this.summary = this.engine.summary().into();
                cx.notify();
            });
        })
        .detach();
    }

    fn engine_event(&mut self, _event: EngineEvent, cx: &mut Context<Self>) {
        self.summary = self.engine.summary().into();
        let done = self.engine.snapshot().iter().filter(|j| matches!(j.state, JobState::Done)).count();
        if done > self.done_count && !self.refresh_pending {
            // New files on either side: show them.
            self.refresh_pending = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(REFRESH_AFTER).await;
                let _ = this.update(cx, |this, cx| {
                    this.refresh_pending = false;
                    this.refresh(Side::Local, cx);
                    this.refresh(Side::Remote, cx);
                });
            })
            .detach();
        }
        self.done_count = done;
        cx.notify();
    }

    fn accepts(&self, side: Side, value: &dyn Any, view: EntityId) -> bool {
        if let Some(d) = value.downcast_ref::<DraggedFiles>() {
            return d.view == view && d.from != side && self.can_receive(side);
        }
        side == Side::Remote && value.downcast_ref::<ExternalPaths>().is_some() && self.can_receive(side)
    }

    // ------------------------------------------------------------- render --

    fn rows(&mut self, side: Side, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let view = cx.entity_id();
        let entity = cx.entity();
        let pane = self.side(side);
        let Some(place) = pane.place.clone() else { return Vec::new() };
        let remote = side == Side::Remote;
        let drives = pane.at_drives();
        let receive = self.can_receive(side);
        let mut out = Vec::new();
        for row in range {
            let Some(&index) = pane.visible.get(row) else { continue };
            let entry = &pane.entries()[index];
            let selected = pane.selected.contains(&entry.raw_name);
            let (glyph, color) = if drives { ("folder", theme::accent()) } else { entry_icon(entry) };
            let size = if entry.kind.opens() { String::new() } else { format::size(entry.size) };
            let mut el = div()
                .id((if remote { "remote-row" } else { "local-row" }, row))
                .w_full()
                .h(theme::CONTROL)
                .px(px(ROW_PAD))
                .flex()
                .items_center()
                .gap(px(COLUMN_GAP))
                .border_b_1()
                .border_color(theme::line_soft())
                .cursor_pointer()
                .when(selected, |d| d.bg(theme::card_selected()))
                .when(!selected, |d| d.hover(|s| s.bg(theme::card())))
                .child(div().w(px(CHECK_W)).flex_none().when(!drives, |d| d.child(check_box(selected))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(COLUMN_GAP))
                        .child(icon(glyph, px(18.), color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_body()
                                .text_color(theme::ink())
                                .child(SharedString::from(entry.name.clone())),
                        ),
                )
                .child(
                    div()
                        .w(px(if remote { REMOTE_SIZE_W } else { LOCAL_SIZE_W }))
                        .flex_none()
                        .text_mono_small()
                        .text_color(theme::ink_muted())
                        .child(SharedString::from(size)),
                )
                .child(
                    div()
                        .w(px(if remote { REMOTE_MODIFIED_W } else { LOCAL_MODIFIED_W }))
                        .flex_none()
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child(SharedString::from(format_modified(entry.modified, !remote))),
                )
                .when(remote, |d| {
                    d.child(
                        div()
                            .w(px(PERMISSIONS_W))
                            .flex_none()
                            .text_mono_small()
                            .text_color(theme::ink_muted())
                            .child(SharedString::from(entry.symbolic_mode().unwrap_or_default())),
                    )
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| this.row_click(side, row, event, cx)));
            // Drag the selection when this row is part of it.
            if !drives {
                let dragged: Vec<&Entry> = if selected { pane.selection() } else { vec![entry] };
                if let Some(items) = self.items_of(side, &dragged) {
                    let label: SharedString = if dragged.len() == 1 {
                        dragged[0].name.clone().into()
                    } else {
                        format!("{} items", dragged.len()).into()
                    };
                    let value = DraggedFiles { view, from: side, items, label };
                    let entity = entity.clone();
                    el = el.on_drag(value, move |d: &DraggedFiles, _, _, cx| {
                        let from = d.from;
                        entity.update(cx, |this, cx| {
                            this.dragging = Some(from);
                            cx.notify();
                        });
                        let label = d.label.clone();
                        cx.new(|_| DragChip(label))
                    });
                }
            }
            // A folder row takes drops into itself.
            if entry.kind.opens() && receive {
                let dest = place.child(entry);
                let (dest_a, dest_b) = (dest.clone(), dest);
                let accepts = entity.clone();
                el = el
                    .can_drop(move |value, _, cx| accepts.read(cx).accepts(side, value, view))
                    .drag_over::<DraggedFiles>(|s, _, _, _| s.bg(theme::accent_alpha(0.12)))
                    .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(theme::accent_alpha(0.12)))
                    .on_drop(cx.listener(move |this, d: &DraggedFiles, _, cx| {
                        this.dragging = None;
                        this.clear_selection(d.from, cx);
                        this.transfer(d.items.clone(), dest_a.clone(), cx);
                    }))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.transfer(Items::Local(paths.paths().to_vec()), dest_b.clone(), cx);
                    }));
            }
            out.push(el.into_any_element());
        }
        out
    }

    fn header_row(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.side(side);
        let remote = side == Side::Remote;
        let all = !pane.visible.is_empty() && pane.selected.len() == pane.visible.len();
        let label = |text: &'static str| div().text_label().text_color(theme::ink_muted()).child(text);
        div()
            .flex_none()
            .px(px(ROW_PAD))
            .py(theme::SPACE_2)
            .flex()
            .items_center()
            .gap(px(COLUMN_GAP))
            .border_b_1()
            .border_color(theme::line())
            .child(div().w(px(CHECK_W)).flex_none().when(!pane.at_drives(), |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("{}-all", side.tag())))
                        .cursor_pointer()
                        .child(check_box(all))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_all(side, cx))),
                )
            }))
            .child(label("NAME").flex_1().min_w_0())
            .child(label("SIZE").w(px(if remote { REMOTE_SIZE_W } else { LOCAL_SIZE_W })).flex_none())
            .child(label("MODIFIED").w(px(if remote { REMOTE_MODIFIED_W } else { LOCAL_MODIFIED_W })).flex_none())
            .when(remote, |d| d.child(label("PERMISSIONS").w(px(PERMISSIONS_W)).flex_none()))
    }

    fn action_bar(&self, side: Side, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let count = self.side(side).selected.len();
        if count == 0 {
            return None;
        }
        let tag = side.tag();
        let (send_label, can_send) = match side {
            Side::Local => ("Upload →", self.can_receive(Side::Remote)),
            Side::Remote => ("← Download", self.can_receive(Side::Local)),
        };
        Some(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(theme::SPACE_2)
                .bg(theme::card())
                .border_b_1()
                .border_color(theme::line())
                .child(div().px(px(6.)).text_body_strong().text_color(theme::ink()).child(plural(
                    count,
                    "item selected",
                    "items selected",
                )))
                .child(div().flex_1())
                .child(
                    Button::primary(SharedString::from(format!("{tag}-send")), send_label)
                        .compact()
                        .disabled(!can_send)
                        .on_click(cx.listener(move |this, _, _, cx| this.send(side, cx))),
                )
                .child(
                    Button::secondary(SharedString::from(format!("{tag}-copy-path")), "Copy path")
                        .compact()
                        .on_click(cx.listener(move |this, _, _, cx| this.copy_paths(side, cx))),
                )
                .child(nav_button(
                    format!("{tag}-clear").into(),
                    "close",
                    true,
                    cx.listener(move |this, _, _, cx| this.clear_selection(side, cx)),
                )),
        )
    }

    fn pane(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.side(side);
        let tag = side.tag();
        let remote = side == Side::Remote;
        let view = cx.entity_id();
        let title: SharedString = if remote { self.host_name.clone() } else { "This PC".into() };
        let top = if remote { theme::no_group() } else { theme::ink_muted() };
        // Long paths keep the first crumb and the last few; "…" opens the
        // folder just before those.
        let mut crumbs = pane.place.as_ref().map(Place::crumbs).unwrap_or_default();
        if crumbs.len() > CRUMBS_KEPT + 2 {
            let tail = crumbs.split_off(crumbs.len() - CRUMBS_KEPT);
            let before = crumbs.pop().map(|(_, p)| p).expect("long enough");
            crumbs.truncate(1);
            crumbs.push(("…".into(), before));
            crumbs.extend(tail);
        }
        let last = crumbs.len().saturating_sub(1);
        let can_up = pane.place.as_ref().and_then(Place::parent).is_some();
        let connected = !remote || self.remote.is_some();

        // A row drag from the other side: "Drop to upload into …". It floats
        // over the bottom of the pane: moving the rows mid-drag would move
        // the folder rows out from under the pointer.
        let target = self.dragging == Some(side.other()) && cx.has_active_drag() && self.can_receive(side);
        let banner = target.then(|| {
            let place = pane.place.as_ref().map(Place::display).unwrap_or_default();
            div()
                .absolute()
                .left(theme::SPACE_2)
                .right(theme::SPACE_2)
                .bottom(theme::SPACE_2)
                .p(px(14.))
                .border_1()
                .border_dashed()
                .border_color(theme::accent())
                .rounded(theme::RADIUS_MD)
                .bg(theme::card())
                .shadow(theme::shadow_panel())
                .text_body_strong()
                .text_color(theme::accent())
                .flex()
                .justify_center()
                .child(if remote {
                    format!("Drop to upload into {place}")
                } else {
                    format!("Drop to download into {place}")
                })
        });

        let body: AnyElement = match &pane.listing {
            _ if pane.place.is_none() && !connected => self.message("Connecting…", false),
            Listing::Loading => self.message("Loading…", false),
            Listing::Failed(why) => self.message(why.clone(), true),
            Listing::Ready(list) if list.is_empty() => self.message("This folder is empty.", false),
            Listing::Ready(_) if pane.visible.is_empty() => {
                let query = pane.filter.read(cx).text().trim().to_string();
                if query.is_empty() {
                    self.message("Only hidden files are here. Settings, SFTP shows them.", false)
                } else {
                    self.message(format!("Nothing here matches \"{query}\"."), false)
                }
            }
            Listing::Ready(_) => uniform_list(
                SharedString::from(format!("{tag}-list")),
                pane.visible.len(),
                cx.processor(move |this, range: Range<usize>, _, cx| this.rows(side, range, cx)),
            )
            .track_scroll(pane.scroll.clone())
            .flex_1()
            .min_h_0()
            .into_any_element(),
        };

        let accepts = cx.entity();
        div()
            .id(SharedString::from(format!("{tag}-pane")))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::ground())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_MD)
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.double_click(side, cx);
                    }
                }),
            )
            .can_drop(move |value, _, cx| accepts.read(cx).accepts(side, value, view))
            .drag_over::<DraggedFiles>(|s, _, _, _| s.border_dashed().border_color(theme::accent()))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_dashed().border_color(theme::accent()))
            .on_drop(cx.listener(move |this, d: &DraggedFiles, _, cx| {
                this.dragging = None;
                this.clear_selection(d.from, cx);
                if let Some(dest) = this.side(side).place.clone() {
                    this.transfer(d.items.clone(), dest, cx);
                }
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                if let Some(dest) = this.side(side).place.clone() {
                    this.transfer(Items::Local(paths.paths().to_vec()), dest, cx);
                }
            }))
            // The side's color along the top (the host's group color).
            .child(div().flex_none().h(px(2.)).bg(top))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_3)
                    .px(px(14.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::ink())
                            .truncate()
                            .child(title),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(200.)).flex_shrink().min_w(px(120.)).child(pane.filter.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_1)
                    .px(px(10.))
                    .pb(theme::SPACE_2)
                    .border_b_1()
                    .border_color(theme::line())
                    .child(nav_button(
                        format!("{tag}-back").into(),
                        "chevron-left",
                        !pane.back.is_empty(),
                        cx.listener(move |this, _, _, cx| this.go_back(side, cx)),
                    ))
                    .child(nav_button(
                        format!("{tag}-up").into(),
                        "chevron-up",
                        can_up,
                        cx.listener(move |this, _, _, cx| this.go_up(side, cx)),
                    ))
                    .child(nav_button(
                        format!("{tag}-refresh").into(),
                        "refresh",
                        pane.place.is_some() && connected,
                        cx.listener(move |this, _, _, cx| this.refresh(side, cx)),
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!("{tag}-crumbs")))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_x_scroll()
                            .text_mono_small()
                            .children(crumbs.into_iter().enumerate().map(|(i, (label, place))| {
                                let current = i == last;
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("{tag}-crumb-{i}")))
                                            .px(px(6.))
                                            .py(theme::SPACE_2)
                                            .rounded(theme::RADIUS_SM)
                                            .whitespace_nowrap()
                                            .text_color(if current { theme::ink() } else { theme::ink_muted() })
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme::raised()))
                                            .child(label)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.side(side).place.as_ref() != Some(&place) {
                                                    this.open(side, place.clone(), cx);
                                                }
                                            })),
                                    )
                                    .when(!current, |d| d.child(div().text_color(theme::ink_faint()).child("›")))
                            })),
                    ),
            )
            .children(self.action_bar(side, cx))
            .child(self.header_row(side, cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .children(banner)
    }

    fn message(&self, text: impl Into<SharedString>, problem: bool) -> AnyElement {
        div()
            .p(px(18.))
            .text_body()
            .text_color(if problem { theme::warn() } else { theme::ink_muted() })
            .child(text.into())
            .into_any_element()
    }

    /// The bottom strip: "Transfers" and the queue summary. The list of
    /// transfers with their controls comes in phase 6.
    fn transfers(&self) -> impl IntoElement {
        let summary: SharedString = if self.queued_any {
            self.summary.clone()
        } else {
            "Nothing in the queue. Drag files between the panes to start a transfer.".into()
        };
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .px(px(18.))
            .py(px(14.))
            .border_t_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .child(
                div()
                    .font_family(theme::DISPLAY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .text_color(theme::ink())
                    .child("Transfers"),
            )
            .child(div().text_meta().text_color(theme::ink_muted()).child(summary))
            .child(div().flex_1())
            .children(self.note.clone().map(|n| div().text_meta().text_color(theme::ink_soft()).child(n)))
    }
}

impl Render for FilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.dragging = None;
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(theme::SPACE_2)
                    .p(theme::SPACE_2)
                    .bg(theme::sunken())
                    .child(self.pane(Side::Local, cx))
                    .child(self.pane(Side::Remote, cx)),
            )
            .child(self.transfers())
    }
}
