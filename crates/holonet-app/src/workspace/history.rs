//! History (History.dc.html): every session, newest first, with its saved
//! output where the host logs. Connect again, export the log as text, or
//! delete it.

use gpui::{AnyElement, Context, Div, Entity, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};
use holonet_model::{RecordId, SessionKind, SessionRecord, Timestamp, VaultData};

use super::{Page, Workspace};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, ButtonKind, TextField, dialog, dialog_buttons, icon, modal, section_label};

const ASIDE: f32 = 380.;
/// Lines of saved output shown in the panel; Export log has all of them.
const SHOWN_LINES: usize = 400;

/// History page state kept by the workspace.
pub(crate) struct HistoryUi {
    pub(super) search: Entity<TextField>,
    selected: Option<RecordId>,
    /// The selected record's log as last read, or why it could not be.
    log: Option<(RecordId, Result<holonet_vault::LogText, String>)>,
    delete_open: bool,
}

impl HistoryUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            search: cx.new(|cx| TextField::new(cx, "Filter by host")),
            selected: None,
            log: None,
            delete_open: false,
        }
    }
}

/// "24 Nov 2026, 15:12".
fn format_start(ts: Timestamp) -> String {
    jiff::Timestamp::from_second(ts.0)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// A file name for Export log: "prod-web-02 2026-11-24 1512.txt".
fn export_name(r: &SessionRecord) -> String {
    let when = jiff::Timestamp::from_second(r.started.0)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H%M").to_string())
        .unwrap_or_default();
    let host: String =
        r.host_name.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    format!("{host} {when}.txt")
}

impl Workspace {
    /// Open History, optionally with a record selected.
    pub(crate) fn open_history(&mut self, cx: &mut Context<Self>) {
        self.hist.log = None;
        self.page = Page::History;
        cx.notify();
    }

    fn select_record(&mut self, id: RecordId, cx: &mut Context<Self>) {
        self.hist.selected = Some(id);
        // Read again on every pick, so an open session's log shows its latest.
        self.hist.log = None;
        cx.notify();
    }

    /// The selected record's log, read when first needed.
    fn ensure_log(&mut self, record: &SessionRecord) {
        if !record.logged || self.hist.log.as_ref().is_some_and(|(id, _)| *id == record.id) {
            return;
        }
        let read = match self.vault.as_ref() {
            Some(v) => v.logs().read(record.id).map_err(|e| match e {
                holonet_vault::VaultError::NotFound(_) => "The saved output is missing from the logs folder.".into(),
                other => format!("Holonet could not read this log: {other}"),
            }),
            None => return,
        };
        self.hist.log = Some((record.id, read));
    }

    pub(super) fn history_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault_data().cloned() else { return div().into_any_element() };
        let query = self.hist.search.read(cx).text().trim().to_lowercase();
        let shown: Vec<&SessionRecord> = data
            .history
            .iter()
            .rev()
            .filter(|r| query.is_empty() || r.host_name.to_lowercase().contains(&query))
            .collect();
        if self.hist.selected.is_none_or(|id| !shown.iter().any(|r| r.id == id)) {
            self.hist.selected = shown.first().map(|r| r.id);
        }
        let selected = self.hist.selected.and_then(|id| data.record(id)).cloned();
        if let Some(r) = &selected {
            self.ensure_log(r);
        }

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::SPACE_3)
            .child(
                div()
                    .flex_1()
                    .min_w(px(240.))
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .child(div().text_title().text_color(theme::ink()).child("History"))
                    .child(div().text_body().text_color(theme::ink_muted()).child(
                        "Past sessions. Output is saved only for hosts with logging turned on, and it is stored in the encrypted vault.",
                    )),
            )
            .child(div().w(px(320.)).child(self.hist.search.clone()));

        let body: AnyElement = if data.history.is_empty() {
            div()
                .py(theme::SPACE_6)
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::SPACE_3)
                .child(icon("history", px(40.), theme::ink_faint()))
                .child(div().text_heading().text_color(theme::ink()).child("No sessions yet"))
                .child(
                    div()
                        .text_body()
                        .text_color(theme::ink_muted())
                        .child("Each terminal and SFTP session you open is listed here."),
                )
                .into_any_element()
        } else {
            self.history_table(&data, &shown, cx).into_any_element()
        };

        let note = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(theme::SPACE_1)
            .text_meta()
            .text_color(theme::ink_muted())
            .child("Logging is off unless you turn it on for a host; SFTP sessions are never logged. Set the default, and how long logs are kept, in Settings.")
            .child(
                div()
                    .id("history-settings")
                    .text_color(theme::accent())
                    .cursor_pointer()
                    .child("Open Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Settings;
                        cx.notify();
                    })),
            );

        let detail = selected.as_ref().map(|r| self.history_detail(&data, r, window, cx));
        let dialog_layer = selected.as_ref().filter(|_| self.hist.delete_open).map(|r| self.delete_log_dialog(r, cx));

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("history-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(header)
                        .child(body)
                        .child(note),
                ),
            )
            .children(detail)
            .children(dialog_layer)
            .into_any_element()
    }

    fn history_table(&self, data: &VaultData, rows: &[&SessionRecord], cx: &mut Context<Self>) -> Div {
        let now = Timestamp::now();
        // History.dc.html: minmax(0, 1.3fr) 150px 90px 100px 80px.
        let columns = |d: Div| d.flex().items_center().gap(theme::SPACE_3).px(theme::SPACE_4);
        let cell = |w: f32| div().w(px(w)).flex_none().min_w_0().truncate();
        let header = columns(div().py(px(10.)))
            .text_label()
            .text_color(theme::ink_muted())
            .border_b_1()
            .border_color(theme::line())
            .child(div().flex_1().min_w_0().child("HOST"))
            .child(cell(150.).child("STARTED"))
            .child(cell(90.).child("LENGTH"))
            .child(cell(100.).child("ROUTE"))
            .child(cell(80.).child("LOG"));
        let lines = rows.iter().map(|r| {
            let on = self.hist.selected == Some(r.id);
            let id = r.id;
            let color = r.host.and_then(|h| data.host(h)).and_then(|h| super::GroupTag::of(data, h)).map(|t| t.color);
            let length = if r.ended.is_none() { "Open".to_string() } else { r.length(now) };
            columns(div())
                .id(SharedString::from(format!("hist-{}", r.id.0)))
                .min_h(px(48.))
                .border_b_1()
                .border_color(theme::line())
                .cursor_pointer()
                .when(on, |d| d.bg(theme::card_selected()))
                .when(!on, |d| d.hover(|s| s.bg(theme::raised())))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon("role-server", px(16.), color.unwrap_or_else(theme::no_group)))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_body()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::ink())
                                .child(SharedString::from(r.host_name.clone())),
                        )
                        .child(div().flex_none().text_meta().text_color(theme::ink_muted()).child(r.kind.label())),
                )
                .child(cell(150.).text_meta().text_color(theme::ink_muted()).child(format_start(r.started)))
                .child(cell(90.).text_mono_small().text_color(theme::ink_muted()).child(length))
                .child(cell(100.).text_meta().text_color(theme::ink_muted()).child(SharedString::from(r.route.clone())))
                .child(
                    cell(80.)
                        .text_mono_small()
                        .text_color(theme::ink_muted())
                        .child(if r.ended.is_none() && r.logged { "Writing".to_string() } else { r.log_size() }),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_record(id, cx)))
        });
        let empty = rows.is_empty().then(|| {
            div()
                .py(px(28.))
                .flex()
                .justify_center()
                .text_body()
                .text_color(theme::ink_muted())
                .child("No sessions match this filter.")
        });
        div()
            .bg(theme::card())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_LG)
            .min_w(px(640.))
            .child(header)
            .children(lines)
            .children(empty)
    }

    fn history_detail(&self, data: &VaultData, r: &SessionRecord, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let now = Timestamp::now();
        let open = r.ended.is_none();
        let host = r.host.and_then(|h| data.host(h));
        let color = host.and_then(|h| super::GroupTag::of(data, h)).map(|t| t.color).unwrap_or_else(theme::no_group);
        let length = if open { "open now".to_string() } else { r.length(now) };
        let summary = [format_start(r.started), length, r.route.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");

        // Connect again opens the same kind of tab, when the host still exists.
        let connect = host.map(|h| {
            let (id, kind) = (h.id, r.kind);
            Button::primary("hist-connect", "Connect again").full_width().on_click(cx.listener(
                move |this, _, window, cx| match kind {
                    SessionKind::Terminal => this.connect(id, window, cx),
                    SessionKind::Sftp => this.open_files(id, window, cx),
                },
            ))
        });
        let log_text = self.hist.log.as_ref().filter(|(id, _)| *id == r.id).map(|(_, t)| t);
        let export = (r.logged && matches!(log_text, Some(Ok(_)))).then(|| {
            let record = r.clone();
            Button::secondary("hist-export", "Export log")
                .full_width()
                .on_click(cx.listener(move |this, _, _, cx| this.export_log(&record, cx)))
        });

        let lines: Vec<(SharedString, bool)> = match (r.kind, r.logged, log_text) {
            (SessionKind::Sftp, _, _) => vec![("SFTP sessions are not logged.".into(), true)],
            (_, false, _) if r.log_deleted => vec![("Its saved output was deleted.".into(), true)],
            (_, false, _) => vec![("Logging is off for this host, so no output was saved.".into(), true)],
            (_, true, Some(Err(why))) => vec![(why.clone().into(), true)],
            (_, true, Some(Ok(text))) => {
                let all: Vec<&str> = text.text.lines().collect();
                let mut out = Vec::new();
                if open {
                    out.push(("This session is still open. Pick it again to see newer output.".into(), true));
                }
                if all.len() > SHOWN_LINES {
                    out.push((
                        format!("Showing the last {SHOWN_LINES} of {} lines. Export log for all of them.", all.len())
                            .into(),
                        true,
                    ));
                }
                if all.is_empty() {
                    out.push(("Nothing was printed.".into(), true));
                }
                out.extend(all[all.len().saturating_sub(SHOWN_LINES)..].iter().map(|l| (l.to_string().into(), false)));
                out
            }
            (_, true, None) => Vec::new(),
        };
        let output = div()
            .id("hist-output")
            .flex_1()
            .min_h(px(240.))
            .overflow_scroll()
            .bg(theme::terminal())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_MD)
            .px(px(14.))
            .py(theme::SPACE_3)
            .text_mono_small()
            .children(lines.into_iter().map(|(t, dim)| {
                // Saved lines keep their layout; Holonet's notes wrap.
                div()
                    .when(!dim, |d| d.whitespace_nowrap())
                    .text_color(if dim { theme::ink_dim() } else { theme::ink_soft() })
                    .child(t)
            }));

        let facts = div()
            .flex()
            .flex_col()
            .bg(theme::card())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_LG)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(theme::SPACE_3)
                    .px(px(14.))
                    .py(theme::SPACE_3)
                    .border_b_1()
                    .border_color(theme::line())
                    .child(div().text_color(theme::ink_muted()).child("Signed in as"))
                    .child(div().text_mono_small().text_color(theme::ink()).child(SharedString::from(
                        if r.user.is_empty() { "Not recorded".to_string() } else { r.user.clone() },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(theme::SPACE_3)
                    .px(px(14.))
                    .py(theme::SPACE_3)
                    .child(div().text_color(theme::ink_muted()).child("Ended"))
                    .child(div().text_color(theme::ink()).child(match r.end {
                        Some(end) => end.label(),
                        None => "Still open",
                    })),
            );

        div()
            .w(px(ASIDE))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(24.))
            .py(px(28.))
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::line())
            .shadow(theme::shadow_panel())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().size(px(10.)).rounded(px(3.)).bg(color))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_heading()
                                    .text_color(theme::ink())
                                    .child(SharedString::from(r.host_name.clone())),
                            ),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(summary))
                    .when(host.is_none() && r.host.is_some(), |d| {
                        d.child(div().text_meta().text_color(theme::ink_muted()).child("This host was deleted."))
                    }),
            )
            .when(connect.is_some() || export.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .gap(theme::SPACE_2)
                        .children(connect.map(|b| div().flex_1().child(b)))
                        .children(export.map(|b| div().flex_1().child(b))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(section_label("Saved output"))
                    .child(output),
            )
            .child(facts)
            // An open session's log is still being written.
            .when(r.logged && !open, |d| {
                d.child(
                    Button::new("hist-delete", "Delete this log", ButtonKind::DestructiveOutline)
                        .full_width()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hist.delete_open = true;
                            cx.notify();
                        })),
                )
            })
    }

    fn delete_log_dialog(&self, r: &SessionRecord, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let close = move |_: &mut Window, cx: &mut gpui::App| {
            entity.update(cx, |w, cx| {
                w.hist.delete_open = false;
                cx.notify()
            })
        };
        let id = r.id;
        let card = dialog(
            true,
            format!("Delete the saved output of {}?", r.host_name),
            format!(
                "The log from {} is deleted from this PC and cannot be recovered. The session stays in History.",
                format_start(r.started)
            ),
        )
        .child(dialog_buttons([
            Button::secondary("hist-delete-cancel", "Cancel").on_click({
                let close = close.clone();
                move |_, w, cx| close(w, cx)
            }),
            Button::new("hist-delete-confirm", "Delete log", ButtonKind::DestructiveFill).on_click(cx.listener(
                move |this, _, _, cx| {
                    let result = this.vault.as_ref().map(|v| v.logs().delete(id));
                    match result {
                        Some(Ok(())) => {
                            this.save(
                                move |d| {
                                    if let Some(r) = d.record_mut(id) {
                                        r.logged = false;
                                        r.log_bytes = 0;
                                        r.log_deleted = true;
                                    }
                                },
                                cx,
                            );
                            this.notice = Some("The log was deleted.".into());
                        }
                        Some(Err(e)) => this.save_error = Some(format!("Holonet could not delete the log: {e}").into()),
                        None => {}
                    }
                    this.hist.log = None;
                    this.hist.delete_open = false;
                    cx.notify();
                },
            )),
        ]));
        modal("hist-dialog", px(480.), card, close).into_any_element()
    }

    /// Export log: the whole log as a text file wherever you choose.
    fn export_log(&mut self, record: &SessionRecord, cx: &mut Context<Self>) {
        let Some(logs) = self.vault.as_ref().map(|v| v.logs()) else { return };
        let id = record.id;
        let dir = std::env::var_os("USERPROFILE")
            .map(|p| std::path::PathBuf::from(p).join("Documents"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let answer = cx.prompt_for_new_path(&dir, Some(&export_name(record)));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = answer.await else { return };
            let result = cx
                .background_executor()
                .spawn(async move {
                    let text = logs.read(id).map_err(|e| e.to_string())?.text;
                    // CRLF, so Notepad and every Windows editor read the lines.
                    std::fs::write(&path, text.replace('\n', "\r\n")).map_err(|e| e.to_string())?;
                    Ok::<_, String>(path)
                })
                .await;
            let _ = this.update(cx, |w: &mut Workspace, cx| {
                match result {
                    Ok(path) => {
                        let path = super::short_path(&path.display().to_string(), 80);
                        w.notice = Some(format!("Saved the log to {path}.").into())
                    }
                    Err(e) => w.save_error = Some(format!("Holonet could not export the log: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_are_safe_file_names() {
        let mut r = SessionRecord::new(None, "deploy@prod/web:01", SessionKind::Terminal, true);
        r.started = Timestamp(0);
        let name = export_name(&r);
        assert!(name.starts_with("deploy-prod-web-01 19"), "{name}");
        assert!(name.ends_with(".txt") && !name.contains(['/', ':', '@']));
    }
}
