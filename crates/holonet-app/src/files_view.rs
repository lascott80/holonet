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
//! the tab's [`Engine`], shown in the queue panel under the panes with
//! Pause, Resume, Retry and remove, and the finished ones on request. A
//! transfer that meets names already there asks once, in the conflict
//! popup, unless Settings chose Overwrite or Keep both.
//! The action bar's Rename, Permissions (on the host) and Delete, and each
//! pane's New folder, open popups; Delete sends this PC's files to the
//! Recycle Bin and deletes the host's for good, after saying so.

use std::any::Any;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Entity, EntityId, ExternalPaths, FontWeight, IntoElement,
    MouseButton, MouseDownEvent, Render, Rgba, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use holonet_files::engine::{Engine, EngineEvent, Job, JobId, JobState};
use holonet_files::plan::{self, Choice, Direction, Plan};
use holonet_files::remote::Remote;
use holonet_files::{Entry, FileError, Kind, RemotePath, format, local};
use holonet_model::{Host, HostId, OnConflict, SftpSettings};

use crate::app::HolonetApp;
use crate::theme::{self, TypeStyles};
use crate::ui::files::{
    FileRow, breadcrumbs, check_box, file_header, file_row, nav_button, permissions_grid, progress_bar, selection_bar,
};
use crate::ui::{Button, ButtonKind, IndexHandler, TextField, TextFieldEvent, modal};

/// How long after a transfer finishes the panes refresh (several finishing
/// together refresh once).
const REFRESH_AFTER: Duration = Duration::from_millis(400);

/// A queue row's height, and how many show before the panel scrolls.
const QUEUE_ROW_H: f32 = 58.;
const QUEUE_ROWS_SHOWN: f32 = 4.5;

/// The conflict popup's width (Sftp.dc.html's dialogs).
const CONFLICT_W: f32 = 460.;

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

/// An open popup: rename, new folder, delete or permissions.
struct Dialog {
    side: Side,
    kind: DialogKind,
    /// The host's or this PC's answer when it refused.
    error: Option<SharedString>,
    /// Waiting for that answer; the buttons are disabled.
    busy: bool,
}

enum DialogKind {
    /// Rename `old`, or New folder when `None`.
    Name {
        old: Option<Entry>,
        field: Entity<TextField>,
        _subscription: Subscription,
    },
    Delete {
        entries: Vec<Entry>,
    },
    /// Read, write and run for owner, group and everyone else.
    Permissions {
        entries: Vec<Entry>,
        bits: [bool; 9],
    },
}

/// What a confirmed popup does, run off the UI thread.
enum Op {
    LocalRename(PathBuf, PathBuf),
    RemoteRename(RemotePath, RemotePath),
    LocalFolder(PathBuf),
    RemoteFolder(RemotePath),
    /// To the Recycle Bin (`decisions.md`).
    LocalRecycle(Vec<PathBuf>),
    /// Permanently, folders with their contents.
    RemoteRemove(Vec<RemotePath>),
    /// Each path's new mode: the grid, keeping setuid, setgid and sticky.
    RemoteMode(Vec<(RemotePath, u32)>),
}

impl Op {
    fn remote(&self) -> bool {
        matches!(self, Op::RemoteRename(..) | Op::RemoteFolder(_) | Op::RemoteRemove(_) | Op::RemoteMode(_))
    }

    /// Run it; the text is the note to show afterwards.
    async fn run(self, remote: Option<Arc<Remote>>) -> Result<Option<String>, FileError> {
        let blocking = |f: Box<dyn FnOnce() -> Result<Option<String>, FileError> + Send>| async move {
            tokio::task::spawn_blocking(f).await.map_err(|e| FileError::Other(e.to_string()))?
        };
        match self {
            Op::LocalRename(from, to) => blocking(Box::new(move || local::rename(&from, &to).map(|_| None))).await,
            Op::LocalFolder(path) => blocking(Box::new(move || local::create_folder(&path).map(|_| None))).await,
            Op::LocalRecycle(paths) => {
                let n = paths.len();
                blocking(Box::new(move || {
                    local::recycle(&paths)
                        .map(|_| Some(format!("Moved {} to the Recycle Bin.", plural(n, "item", "items"))))
                }))
                .await
            }
            Op::RemoteRename(from, to) => remote.expect("checked by the caller").rename(&from, &to).await.map(|_| None),
            Op::RemoteFolder(path) => remote.expect("checked by the caller").create_folder(&path).await.map(|_| None),
            Op::RemoteRemove(paths) => {
                let remote = remote.expect("checked by the caller");
                let mut removed = 0;
                for path in &paths {
                    removed += remote.remove(path).await?;
                }
                Ok(Some(format!("Deleted {}.", plural(removed, "file or folder", "files and folders"))))
            }
            Op::RemoteMode(changes) => {
                let remote = remote.expect("checked by the caller");
                for (path, mode) in &changes {
                    remote.set_mode(path, *mode).await?;
                }
                Ok(Some(format!("Changed the permissions of {}.", plural(changes.len(), "item", "items"))))
            }
        }
    }
}

/// A planned transfer waiting for the conflict popup's answer.
struct Pending {
    plan: Plan,
    /// "Uploading to /srv/www on nas-01." or "Downloading to this PC."
    note: String,
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
    /// The engine's jobs as last reported, for the queue panel.
    jobs: Vec<Job>,
    summary: SharedString,
    done_count: usize,
    show_finished: bool,
    queue_scroll: UniformListScrollHandle,
    /// The conflict popup's transfer.
    pending: Option<Pending>,
    /// Rename, New folder, Delete or Permissions, while open.
    dialog: Option<Dialog>,
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
            jobs: Vec::new(),
            summary: "Idle".into(),
            done_count: 0,
            show_finished: false,
            queue_scroll: UniformListScrollHandle::new(),
            pending: None,
            dialog: None,
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

    /// Plan a transfer. Names already at the destination follow Settings'
    /// "When a file already exists": Ask opens the conflict popup, the others
    /// go straight to the queue.
    fn transfer(&mut self, items: Items, dest: Place, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.clone() else { return };
        let note = match &dest {
            Place::Remote(dir) => format!("Uploading to {dir} on {}.", self.host_name),
            _ => "Downloading to this PC.".to_string(),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let planned = match (items, &dest) {
                (Items::Local(paths), Place::Remote(dir)) => plan::plan_upload(&remote, &paths, dir).await,
                (Items::Remote(paths), Place::Local(dir)) => plan::plan_download(&remote, &paths, dir).await,
                _ => return,
            };
            let _ = tx.send(planned);
        });
        cx.spawn(async move |this, cx| {
            let Ok(planned) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match planned {
                    Err(e) => this.note = Some(format!("The transfer could not start: {e}").into()),
                    Ok(plan) => {
                        let choice = match this.settings.on_conflict {
                            _ if plan.conflicts().is_empty() => Some(Choice::Skip),
                            OnConflict::Ask => None,
                            OnConflict::Overwrite => Some(Choice::Overwrite),
                            OnConflict::KeepBoth => Some(Choice::KeepBoth),
                        };
                        match choice {
                            Some(choice) => this.start(plan, choice, cx),
                            None => this.pending = Some(Pending { plan, note }),
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The conflict popup's answer: `None` is Cancel all.
    fn resolve_pending(&mut self, choice: Option<Choice>, cx: &mut Context<Self>) {
        if let (Some(pending), Some(choice)) = (self.pending.take(), choice) {
            self.start(pending.plan, choice, cx);
        }
        self.pending = None;
        cx.notify();
    }

    /// Apply the conflict choice, create the folders and queue the files.
    fn start(&mut self, plan: Plan, choice: Choice, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.clone() else {
            self.note = Some("The transfer could not start: the connection was lost.".into());
            return;
        };
        let engine = self.engine.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let outcome = async {
                let plan = plan.resolve(choice);
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
                // New folders show at once; files as they finish.
                this.refresh(Side::Local, cx);
                this.refresh(Side::Remote, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn engine_event(&mut self, _event: EngineEvent, cx: &mut Context<Self>) {
        self.jobs = self.engine.snapshot();
        self.summary = self.engine.summary().into();
        let done = self.jobs.iter().filter(|j| j.state == JobState::Done).count();
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

    /// Pause, Resume or Retry, by the job's state.
    fn toggle_job(&mut self, id: JobId, cx: &mut Context<Self>) {
        let Some(state) = self.jobs.iter().find(|j| j.id == id).map(|j| j.state.clone()) else { return };
        match state {
            JobState::Active => self.engine.pause(id),
            JobState::Paused => self.engine.resume(id),
            JobState::Failed(_) => self.engine.retry(id),
            _ => {}
        }
        cx.notify();
    }

    // ------------------------------------------------------------ dialogs --

    /// Where a side is, for dialog titles: "This PC" or the host's name.
    fn side_name(&self, side: Side) -> SharedString {
        match side {
            Side::Local => "This PC".into(),
            Side::Remote => self.host_name.clone(),
        }
    }

    /// Whether this side can change files now (a folder, and for the host a
    /// connection).
    fn can_edit(&self, side: Side) -> bool {
        (side == Side::Local || self.remote.is_some()) && self.side(side).place.as_ref().is_some_and(Place::is_folder)
    }

    /// Rename (one selected item) or New folder: a name popup.
    fn open_name(&mut self, side: Side, rename: bool, window: &mut Window, cx: &mut Context<Self>) {
        let old = if rename {
            match self.side(side).selection().as_slice() {
                [one] => Some((*one).clone()),
                _ => return,
            }
        } else {
            None
        };
        let field = cx.new(|cx| {
            TextField::new(cx, if rename { "" } else { "New folder" })
                .mono()
                .with_text(old.as_ref().map(|e| e.name.clone()).unwrap_or_default())
        });
        let subscription = cx.subscribe(&field, |this: &mut Self, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submitted => this.apply_dialog(cx),
            TextFieldEvent::Cancelled => this.close_dialog(cx),
            TextFieldEvent::Changed => {
                if let Some(Dialog { error, .. }) = &mut this.dialog {
                    *error = None;
                }
                cx.notify();
            }
        });
        field.read(cx).focus(window);
        self.dialog = Some(Dialog {
            side,
            kind: DialogKind::Name { old, field, _subscription: subscription },
            error: None,
            busy: false,
        });
        cx.notify();
    }

    fn open_delete(&mut self, side: Side, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = self.side(side).selection().into_iter().cloned().collect();
        if entries.is_empty() {
            return;
        }
        self.dialog = Some(Dialog { side, kind: DialogKind::Delete { entries }, error: None, busy: false });
        cx.notify();
    }

    fn open_permissions(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = self.remote_side.selection().into_iter().cloned().collect();
        let Some(first) = entries.first() else { return };
        let mode = first.mode.unwrap_or(0o644);
        let bits = std::array::from_fn(|i| mode & (0o400 >> i) != 0);
        self.dialog = Some(Dialog {
            side: Side::Remote,
            kind: DialogKind::Permissions { entries, bits },
            error: None,
            busy: false,
        });
        cx.notify();
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog.as_ref().is_some_and(|d| !d.busy) {
            self.dialog = None;
        }
        cx.notify();
    }

    /// Why the name popup's name cannot be used, or `None` when it can.
    fn name_problem(&self, side: Side, old: Option<&Entry>, name: &str) -> Option<&'static str> {
        if name.is_empty() {
            return Some("Enter a name.");
        }
        if name.contains('/') || name == "." || name == ".." {
            return Some("Enter a name without a slash.");
        }
        if side == Side::Local && (name.contains('\\') || plan::windows_unsafe(name)) {
            return Some("Windows cannot use this name.");
        }
        let same = old.is_some_and(|o| o.name == name);
        if !same && self.side(side).entries().iter().any(|e| e.name == name) {
            return Some("Something with this name already exists in this folder.");
        }
        None
    }

    /// The dialog's confirming button: Rename, Create, Delete or Apply.
    fn apply_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.dialog else { return };
        if dialog.busy {
            return;
        }
        let side = dialog.side;
        let Some(place) = self.side(side).place.clone() else { return };
        let remote = self.remote.clone();
        let op: Op = match &dialog.kind {
            DialogKind::Name { old, field, .. } => {
                let name = field.read(cx).text().trim().to_string();
                if let Some(problem) = self.name_problem(side, old.as_ref(), &name) {
                    if let Some(d) = &mut self.dialog {
                        d.error = Some(problem.into());
                    }
                    cx.notify();
                    return;
                }
                match (old, &place) {
                    (Some(old), _) if old.name == name => {
                        self.dialog = None;
                        cx.notify();
                        return;
                    }
                    (Some(old), Place::Local(dir)) => Op::LocalRename(dir.join(&old.name), dir.join(&name)),
                    (Some(old), Place::Remote(dir)) => {
                        Op::RemoteRename(dir.join(&old.raw_name), dir.join(name.as_bytes()))
                    }
                    (None, Place::Local(dir)) => Op::LocalFolder(dir.join(&name)),
                    (None, Place::Remote(dir)) => Op::RemoteFolder(dir.join(name.as_bytes())),
                    _ => return,
                }
            }
            DialogKind::Delete { entries } => match &place {
                Place::Local(dir) => Op::LocalRecycle(entries.iter().map(|e| dir.join(&e.name)).collect()),
                Place::Remote(dir) => Op::RemoteRemove(entries.iter().map(|e| dir.join(&e.raw_name)).collect()),
                Place::Drives => return,
            },
            DialogKind::Permissions { entries, bits } => {
                let Place::Remote(dir) = &place else { return };
                let grid = bits.iter().enumerate().fold(0u32, |m, (i, on)| if *on { m | (0o400 >> i) } else { m });
                Op::RemoteMode(
                    entries.iter().map(|e| (dir.join(&e.raw_name), (e.mode.unwrap_or(0) & 0o7000) | grid)).collect(),
                )
            }
        };
        if op.remote() && remote.is_none() {
            return;
        }
        if let Some(d) = &mut self.dialog {
            d.busy = true;
            d.error = None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(op.run(remote).await);
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(note) => {
                        this.dialog = None;
                        this.note = note.map(SharedString::from);
                        this.clear_selection(side, cx);
                    }
                    Err(e) => {
                        if let Some(d) = &mut this.dialog {
                            d.busy = false;
                            d.error = Some(e.to_string().into());
                        }
                    }
                }
                this.refresh(side, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_bit(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(Dialog { kind: DialogKind::Permissions { bits, .. }, .. }) = &mut self.dialog {
            bits[i] = !bits[i];
        }
        cx.notify();
    }

    /// The dialog card (Sftp.dc.html's popups); a warning card for Delete.
    fn card(warning: bool) -> gpui::Div {
        div()
            .w(px(CONFLICT_W))
            .flex()
            .flex_col()
            .gap(theme::SPACE_4)
            .p(theme::SPACE_5)
            .rounded(theme::RADIUS_2XL)
            .border_1()
            .shadow(theme::shadow_dialog())
            .when(warning, |d| d.bg(theme::warn_surface()).border_color(theme::warn()))
            .when(!warning, |d| d.bg(theme::card()).border_color(theme::line_strong()))
    }

    /// The names a popup acts on, in a mono box.
    fn names_box(names: impl Iterator<Item = String>) -> impl IntoElement {
        div()
            .id("dialog-names")
            .w_full()
            .flex_none()
            .max_h(px(180.))
            .overflow_y_scroll()
            .px(px(14.))
            .py(theme::SPACE_3)
            .rounded(theme::RADIUS_MD)
            .border_1()
            .border_color(theme::line())
            .bg(theme::ground())
            .text_mono()
            .text_color(theme::ink())
            .children(names.map(|n| div().child(SharedString::from(n))))
    }

    /// Cancel and the confirming button, side by side.
    fn dialog_buttons(&self, confirm: Button, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .gap(theme::SPACE_2)
            .child(
                div().flex_1().min_w_0().child(
                    Button::secondary("dialog-cancel", "Cancel")
                        .full_width()
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                ),
            )
            .child(div().flex_1().min_w_0().child(
                confirm.full_width().disabled(busy).on_click(cx.listener(|this, _, _, cx| this.apply_dialog(cx))),
            ))
    }

    fn dialog_view(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.dialog.as_ref()?;
        let side = dialog.side;
        let place = self.side(side).place.as_ref().map(Place::display).unwrap_or_default();
        let error = dialog.error.clone();
        let busy = dialog.busy;
        let card = match &dialog.kind {
            DialogKind::Name { old, field, .. } => {
                let name = field.read(cx).text().trim().to_string();
                let problem = error.clone().or_else(|| {
                    // Show a problem once something is typed; an empty
                    // field only says so on Rename or Create.
                    (!name.is_empty()).then(|| self.name_problem(side, old.as_ref(), &name)).flatten().map(Into::into)
                });
                let (title, label, confirm, note) = match old {
                    Some(old) => (
                        format!("Rename on {}", self.side_name(side)),
                        "New name",
                        "Rename",
                        format!("Renames {}", old.name),
                    ),
                    None => (
                        format!("New folder on {}", self.side_name(side)),
                        "Name",
                        "Create folder",
                        format!("Creates it in {place}"),
                    ),
                };
                Self::card(false)
                    .child(div().w_full().text_heading().text_color(theme::ink()).child(title))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(div().text_meta().text_color(theme::ink_muted()).child(label))
                            .child(field.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_meta()
                            .text_color(if problem.is_some() { theme::warn() } else { theme::ink_muted() })
                            .child(problem.unwrap_or_else(|| note.into())),
                    )
                    .child(self.dialog_buttons(Button::primary("dialog-confirm", confirm), busy, cx))
            }
            DialogKind::Delete { entries } => {
                let title = match entries.as_slice() {
                    [one] => format!("Delete {} from {}?", one.name, self.side_name(side)),
                    many => format!("Delete {} items from {}?", many.len(), self.side_name(side)),
                };
                let note = match side {
                    Side::Remote => "Files deleted over SFTP do not go to a recycle bin. This cannot be undone.",
                    Side::Local => "These are moved to the Recycle Bin on this PC.",
                };
                Self::card(true)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(div().w_full().text_heading().text_color(theme::ink()).child(title))
                            .child(div().w_full().text_body().text_color(theme::warn_ink()).child(note)),
                    )
                    .child(Self::names_box(entries.iter().map(|e| e.name.clone())))
                    .children(error.map(|e| div().w_full().text_meta().text_color(theme::warn()).child(e)))
                    .child(self.dialog_buttons(
                        Button::new("dialog-confirm", "Delete", ButtonKind::DestructiveFill),
                        busy,
                        cx,
                    ))
            }
            DialogKind::Permissions { entries, bits } => {
                let (title, detail) = match entries.as_slice() {
                    [one] => (
                        format!("Permissions for {}", one.name),
                        format!("{}/{}", place.trim_end_matches('/'), one.name),
                    ),
                    many => (
                        format!("Permissions for {} items", many.len()),
                        "The same permissions are applied to every selected item.".to_string(),
                    ),
                };
                let view = cx.entity().downgrade();
                let on_toggle: IndexHandler = std::rc::Rc::new(move |i, _, cx| {
                    let _ = view.update(cx, |this, cx| this.toggle_bit(i, cx));
                });
                let grid = permissions_grid(*bits, on_toggle);
                Self::card(false)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .gap(theme::SPACE_3)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(div().w_full().text_heading().text_color(theme::ink()).child(title))
                                    .child(
                                        div().w_full().text_mono_small().text_color(theme::ink_muted()).child(detail),
                                    ),
                            )
                            .child(nav_button(
                                "perm-close".into(),
                                "close",
                                !busy,
                                cx.listener(|this, _, _, cx| this.close_dialog(cx)),
                            )),
                    )
                    .child(grid)
                    .children(error.map(|e| div().w_full().text_meta().text_color(theme::warn()).child(e)))
                    .child(self.dialog_buttons(Button::primary("dialog-confirm", "Apply"), busy, cx))
            }
        };
        let view = cx.entity().downgrade();
        Some(modal("files-dialog", px(CONFLICT_W), card, move |_, cx| {
            let _ = view.update(cx, |this, cx| this.close_dialog(cx));
        }))
    }

    /// Transfers that would stop if the tab closed now.
    pub fn unfinished(&self) -> usize {
        self.jobs.iter().filter(|j| j.state != JobState::Done).count()
    }

    /// The tab is closing: drop every unfinished transfer. The engine
    /// cancels active copies and deletes downloads' part files.
    pub fn stop_all(&mut self) {
        for job in self.engine.snapshot().iter().filter(|j| j.state != JobState::Done) {
            self.engine.remove(job.id);
        }
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
            let mut el = file_row(
                (if remote { "remote-row" } else { "local-row" }, row),
                FileRow {
                    glyph,
                    color,
                    name: entry.name.clone().into(),
                    size: size.into(),
                    modified: format_modified(entry.modified, !remote).into(),
                    permissions: remote.then(|| entry.symbolic_mode().unwrap_or_default().into()),
                    selected,
                    checkable: !drives,
                },
            )
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
        let all = !pane.visible.is_empty() && pane.selected.len() == pane.visible.len();
        let select_all = (!pane.at_drives()).then(|| {
            div()
                .id(SharedString::from(format!("{}-all", side.tag())))
                .cursor_pointer()
                .child(check_box(all))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_all(side, cx)))
                .into_any_element()
        });
        file_header(side == Side::Remote, select_all)
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
        let can_edit = self.can_edit(side);
        let mut actions = vec![
            Button::primary(SharedString::from(format!("{tag}-send")), send_label)
                .compact()
                .disabled(!can_send)
                .on_click(cx.listener(move |this, _, _, cx| this.send(side, cx)))
                .into_any_element(),
            Button::secondary(SharedString::from(format!("{tag}-rename")), "Rename")
                .compact()
                .disabled(count != 1 || !can_edit)
                .on_click(cx.listener(move |this, _, window, cx| this.open_name(side, true, window, cx)))
                .into_any_element(),
        ];
        if side == Side::Remote {
            actions.push(
                Button::secondary("remote-permissions", "Permissions")
                    .compact()
                    .disabled(!can_edit)
                    .on_click(cx.listener(|this, _, _, cx| this.open_permissions(cx)))
                    .into_any_element(),
            );
        }
        actions.extend([
            Button::secondary(SharedString::from(format!("{tag}-copy-path")), "Copy path")
                .compact()
                .on_click(cx.listener(move |this, _, _, cx| this.copy_paths(side, cx)))
                .into_any_element(),
            Button::new(SharedString::from(format!("{tag}-delete")), "Delete", ButtonKind::DestructiveOutline)
                .compact()
                .disabled(!can_edit)
                .on_click(cx.listener(move |this, _, _, cx| this.open_delete(side, cx)))
                .into_any_element(),
            nav_button(
                SharedString::from(format!("{tag}-clear")),
                "close",
                true,
                cx.listener(move |this, _, _, cx| this.clear_selection(side, cx)),
            )
            .into_any_element(),
        ]);
        Some(selection_bar(count, actions))
    }

    fn pane(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.side(side);
        let tag = side.tag();
        let remote = side == Side::Remote;
        let view = cx.entity_id();
        let title: SharedString = if remote { self.host_name.clone() } else { "This PC".into() };
        // The host's side carries its group's color (Sftp.dc.html).
        let top = if remote {
            let tag = self.app.upgrade().and_then(|app| app.read(cx).group_tag(self.host_id, cx));
            crate::workspace::GroupTag::color_of(tag.as_ref())
        } else {
            theme::ink_muted()
        };
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
                    .child(nav_button(
                        format!("{tag}-new-folder").into(),
                        "folder-plus",
                        self.can_edit(side),
                        cx.listener(move |this, _, window, cx| this.open_name(side, false, window, cx)),
                    ))
                    .child({
                        let (labels, places): (Vec<SharedString>, Vec<Place>) =
                            crumbs.into_iter().map(|(label, place)| (SharedString::from(label), place)).unzip();
                        let view = cx.entity().downgrade();
                        let on_open: IndexHandler = std::rc::Rc::new(move |i, _, cx| {
                            let Some(place) = places.get(i).cloned() else { return };
                            let _ = view.update(cx, |this, cx| {
                                if this.side(side).place.as_ref() != Some(&place) {
                                    this.open(side, place, cx);
                                }
                            });
                        });
                        breadcrumbs(tag, labels, on_open)
                    }),
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

    /// The queue panel (Sftp.dc.html, TransferRow.md): the header with the
    /// summary and "Show finished (n)", then a row per transfer.
    fn transfers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows: Vec<usize> =
            self.jobs.iter().enumerate().filter(|(_, j)| j.state != JobState::Done).map(|(i, _)| i).collect();
        let finished: Vec<usize> =
            self.jobs.iter().enumerate().filter(|(_, j)| j.state == JobState::Done).map(|(i, _)| i).collect();
        if self.show_finished {
            rows.extend(finished.iter().copied());
        }
        let count = rows.len();
        let list: AnyElement = if count == 0 {
            div()
                .p(px(18.))
                .flex()
                .justify_center()
                .text_body()
                .text_color(theme::ink_muted())
                .child("Nothing in the queue. Drag files between the panes to start a transfer.")
                .into_any_element()
        } else {
            uniform_list(
                "transfer-rows",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.filter_map(|i| rows.get(i).map(|j| this.transfer_row(*j, cx))).collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.queue_scroll.clone())
            .h(px(QUEUE_ROW_H * (count as f32).min(QUEUE_ROWS_SHOWN)))
            .into_any_element()
        };
        let finished_n = finished.len();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .py(px(14.))
            .border_t_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        div()
                            .font_family(theme::DISPLAY)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.))
                            .text_color(theme::ink())
                            .child("Transfers"),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(self.summary.clone()))
                    .children(self.note.clone().map(|n| div().text_meta().text_color(theme::ink_soft()).child(n)))
                    .child(div().flex_1())
                    .when(finished_n > 0, |d| {
                        let label = if self.show_finished {
                            format!("Hide finished ({finished_n})")
                        } else {
                            format!("Show finished ({finished_n})")
                        };
                        d.child(Button::secondary("show-finished", label).compact().on_click(cx.listener(
                            |this, _, _, cx| {
                                this.show_finished = !this.show_finished;
                                cx.notify();
                            },
                        )))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(theme::line())
                    .rounded(theme::RADIUS_LG)
                    .bg(theme::card())
                    .overflow_hidden()
                    .child(list),
            )
    }

    /// One transfer: name and route, the bar coloured by direction with its
    /// text, and Pause, Resume or Retry with remove (TransferRow.md).
    fn transfer_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let job = &self.jobs[index];
        let id = job.id;
        let up = job.direction == Direction::Upload;
        let route =
            if up { format!("This PC → {}", self.host_name) } else { format!("{} → This PC", self.host_name) };
        let row = div()
            .id(("transfer", id.0 as usize))
            .w_full()
            .h(px(QUEUE_ROW_H))
            .flex()
            .items_center()
            .gap(px(14.))
            .pl(px(14.))
            .pr(theme::SPACE_2)
            .border_b_1()
            .border_color(theme::line());
        let name = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().truncate().text_body().text_color(theme::ink()).child(SharedString::from(job.name.clone())))
            .child(div().truncate().text_mono_small().text_color(theme::ink_muted()).child(SharedString::from(route)));
        if job.state == JobState::Done {
            let what = if up {
                format!("Uploaded to {} · {}", self.host_name, format::size(job.size))
            } else {
                format!("Downloaded to This PC · {}", format::size(job.size))
            };
            return row
                .child(name)
                .child(div().flex_none().pr(px(6.)).text_meta().text_color(theme::ink_muted()).child(what))
                .into_any_element();
        }
        let percent = job.percent();
        let (meta, color, toggle) = match &job.state {
            JobState::Active => {
                (job.progress_text(), if up { theme::accent() } else { theme::group_violet() }, Some("Pause"))
            }
            JobState::Paused => (format!("Paused at {percent}%"), theme::ink_faint(), Some("Resume")),
            JobState::Queued => ("Queued".to_string(), theme::ink_faint(), None),
            JobState::Interrupted => (format!("Waiting to reconnect at {percent}%"), theme::ink_faint(), None),
            JobState::Failed(why) => (format!("Failed: {why}"), theme::warn(), Some("Retry")),
            JobState::Done => unreachable!("handled above"),
        };
        let failed = matches!(job.state, JobState::Failed(_));
        row.child(name)
            .child(
                div().flex_1().min_w_0().flex().flex_col().gap(px(6.)).child(progress_bar(percent, color)).child(
                    div()
                        .truncate()
                        .text_meta()
                        .text_color(if failed { theme::warn() } else { theme::ink_muted() })
                        .child(SharedString::from(meta)),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .children(toggle.map(|label| {
                        Button::secondary(("transfer-toggle", id.0 as usize), label)
                            .compact()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_job(id, cx)))
                    }))
                    .child(nav_button(
                        format!("transfer-remove-{}", id.0).into(),
                        "close",
                        true,
                        cx.listener(move |this, _, _, cx| {
                            this.engine.remove(id);
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    /// The conflict popup (Sftp.dc.html): one choice for the whole transfer.
    fn conflict_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let pending = self.pending.as_ref()?;
        let names = pending.plan.conflicts();
        let title = match names.as_slice() {
            [one] => format!("{one} already exists"),
            many => format!("{} items already exist", many.len()),
        };
        let note = format!("{} Your choice applies to every conflict in this transfer.", pending.note);
        let list: Vec<SharedString> = names.iter().map(|n| SharedString::from(n.to_string())).collect();
        let choice = |id: &'static str, label: &'static str, primary: bool, pick: Option<Choice>| {
            let button = if primary { Button::primary(id, label) } else { Button::secondary(id, label) };
            div()
                .flex_1()
                .min_w_0()
                .child(button.full_width().on_click(cx.listener(move |this, _, _, cx| this.resolve_pending(pick, cx))))
        };
        // A definite width: percentages inside resolve against it (Taffy
        // otherwise sizes the buttons and wrapped text from their content).
        let card = div()
            .w(px(CONFLICT_W))
            .flex()
            .flex_col()
            .gap(theme::SPACE_4)
            .p(theme::SPACE_5)
            .rounded(theme::RADIUS_2XL)
            .border_1()
            .border_color(theme::line_strong())
            .bg(theme::card())
            .shadow(theme::shadow_dialog())
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().w_full().text_heading().text_color(theme::ink()).child(title))
                    .child(div().w_full().text_body().text_color(theme::ink_muted()).child(note)),
            )
            .child(
                div()
                    .id("conflict-names")
                    .w_full()
                    .flex_none()
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .px(px(14.))
                    .py(theme::SPACE_3)
                    .rounded(theme::RADIUS_MD)
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::ground())
                    .text_mono()
                    .text_color(theme::ink())
                    .children(list.into_iter().map(|n| div().child(n))),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap(theme::SPACE_2)
                            .child(choice("conflict-overwrite", "Overwrite", true, Some(Choice::Overwrite)))
                            .child(choice("conflict-keep-both", "Keep both", false, Some(Choice::KeepBoth)))
                            .child(choice("conflict-skip", "Skip", false, Some(Choice::Skip))),
                    )
                    .child(
                        Button::quiet("conflict-cancel", "Cancel all")
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| this.resolve_pending(None, cx))),
                    ),
            )
            .child(div().w_full().text_meta().text_color(theme::ink_muted()).child(
                "Keep both adds (1) to the name of the incoming file. Skip transfers only the files without a conflict.",
            ));
        let view = cx.entity().downgrade();
        Some(modal("conflict-dialog", px(CONFLICT_W), card, move |_, cx| {
            let _ = view.update(cx, |this, cx| this.resolve_pending(None, cx));
        }))
    }
}

impl Render for FilesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.dragging = None;
        }
        div()
            .relative()
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
            .child(self.transfers(cx))
            .children(self.conflict_popup(cx))
            .children(self.dialog_view(cx))
    }
}
