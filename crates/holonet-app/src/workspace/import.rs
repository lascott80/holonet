//! The import dialog: choose where from (FirstRun.dc.html's three cards),
//! pick a WSL distribution when there are several, read, then preview the
//! hosts with a checkbox each before anything is copied. `known_hosts`
//! imports straight away and reports what it did.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, PathPromptOptions, SharedString, Window, div, prelude::*, px,
};

use super::Workspace;
use crate::import::{self, ConfigPlan, Source, VaultView};
use crate::key_import;
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, dialog, dialog_buttons, icon, modal, pick_row};

/// What is being imported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportKind {
    Config,
    KnownHosts,
}

pub(crate) enum ImportDialog {
    Choose(ImportKind),
    Distro { kind: ImportKind, distros: Vec<(String, PathBuf)> },
    Reading(SharedString),
    Preview { plan: ConfigPlan, checked: Vec<bool>, show_skipped: bool },
    Failed(SharedString),
}

impl ImportKind {
    fn file(self) -> &'static str {
        match self {
            ImportKind::Config => "config",
            ImportKind::KnownHosts => "known_hosts",
        }
    }

    /// The three sources, as FirstRun.dc.html words them.
    pub(crate) fn choices(self) -> [(&'static str, &'static str, &'static str); 3] {
        match self {
            ImportKind::Config => [
                (
                    "OpenSSH config",
                    r"%USERPROFILE%\.ssh\config",
                    "Hosts, ports, users, jump hosts and identity files from your Windows profile.",
                ),
                (
                    "OpenSSH config in WSL",
                    "~/.ssh/config in a WSL distribution",
                    "Reads the config, and the keys it names, from the Linux home folder of a distribution you pick.",
                ),
                ("Another config file", "Any location", "Pick an OpenSSH config file saved anywhere on this PC."),
            ],
            ImportKind::KnownHosts => [
                (
                    "known_hosts",
                    r"%USERPROFILE%\.ssh\known_hosts",
                    "The server keys you have trusted with OpenSSH on Windows.",
                ),
                (
                    "known_hosts in WSL",
                    "~/.ssh/known_hosts in a WSL distribution",
                    "The server keys trusted in the Linux home folder of a distribution you pick.",
                ),
                ("Another known_hosts file", "Any location", "Pick a known_hosts file saved anywhere on this PC."),
            ],
        }
    }
}

impl Workspace {
    pub(crate) fn open_import(&mut self, kind: ImportKind, cx: &mut Context<Self>) {
        self.imp = Some(ImportDialog::Choose(kind));
        cx.notify();
    }

    pub(crate) fn import_choice(&mut self, kind: ImportKind, choice: usize, cx: &mut Context<Self>) {
        match choice {
            0 => self.run_import(kind, Source::Windows, cx),
            1 => {
                self.imp = Some(ImportDialog::Reading("Looking for WSL distributions…".into()));
                cx.spawn(async move |this, cx| {
                    let found = cx.background_executor().spawn(async { key_import::wsl_ssh_dirs() }).await;
                    let _ = this.update(cx, |w: &mut Workspace, cx| {
                        match found {
                            Ok(dirs) => {
                                let distros: Vec<(String, PathBuf)> = dirs
                                    .into_iter()
                                    .filter_map(|(d, ssh)| ssh.parent().map(|home| (d, home.to_path_buf())))
                                    .collect();
                                match distros.as_slice() {
                                    [] => {
                                        w.imp = Some(ImportDialog::Failed(
                                            "Holonet could not find a home folder in any WSL distribution.".into(),
                                        ))
                                    }
                                    [(distro, home)] => {
                                        let source = Source::Wsl { distro: distro.clone(), home: home.clone() };
                                        w.run_import(kind, source, cx);
                                    }
                                    _ => w.imp = Some(ImportDialog::Distro { kind, distros }),
                                }
                            }
                            Err(why) => w.imp = Some(ImportDialog::Failed(why.into())),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Import".into()),
                });
                self.imp = None;
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(paths))) = paths.await else { return };
                    let Some(path) = paths.into_iter().next() else { return };
                    let _ = this.update(cx, |w: &mut Workspace, cx| w.run_import(kind, Source::File(path), cx));
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Read the source on a background thread, then preview (config) or
    /// import and report (known_hosts).
    fn run_import(&mut self, kind: ImportKind, source: Source, cx: &mut Context<Self>) {
        let Some(view) = self.vault_data().map(VaultView::of) else { return };
        self.imp = Some(ImportDialog::Reading(format!("Reading {}…", source.label(kind.file())).into()));
        cx.notify();
        cx.spawn(async move |this, cx| match kind {
            ImportKind::Config => {
                let plan = cx.background_executor().spawn(async move { import::plan_config(&source, &view) }).await;
                let _ = this.update(cx, |w: &mut Workspace, cx| {
                    w.imp = Some(match plan {
                        Ok(plan) => {
                            let checked = plan.hosts.iter().map(|h| !h.exists).collect();
                            ImportDialog::Preview { plan, checked, show_skipped: false }
                        }
                        Err(why) => ImportDialog::Failed(why.into()),
                    });
                    cx.notify();
                });
            }
            ImportKind::KnownHosts => {
                let read = cx.background_executor().spawn(async move { import::read_known_hosts(&source) }).await;
                let _ = this.update(cx, |w: &mut Workspace, cx| {
                    match read {
                        Ok((_, parsed)) => {
                            let mut message = String::new();
                            w.save(|d| message = import::apply_known_hosts(d, &parsed), cx);
                            w.notice = Some(message.into());
                            w.imp = None;
                        }
                        Err(why) => w.imp = Some(ImportDialog::Failed(why.into())),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn finish_config_import(&mut self, cx: &mut Context<Self>) {
        let Some(ImportDialog::Preview { plan, checked, .. }) = self.imp.take() else { return };
        let chosen: Vec<_> = plan.hosts.into_iter().zip(checked).filter(|(_, c)| *c).map(|(h, _)| h).collect();
        let skipped = plan.skipped.len();
        let mut message = String::new();
        self.save(|d| message = import::apply_config(d, chosen).message(skipped), cx);
        self.notice = Some(message.into());
        self.page = super::Page::Hosts;
        cx.notify();
    }

    pub(super) fn import_dialog_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.imp.as_ref()?;
        let entity = cx.entity();
        let close = move |_: &mut Window, cx: &mut gpui::App| {
            entity.update(cx, |w, cx| {
                w.imp = None;
                cx.notify()
            })
        };
        let cancel = |id: &'static str, label: &'static str| {
            let close = close.clone();
            Button::secondary(id, label).on_click(move |_, w, cx| close(w, cx))
        };
        let note = || {
            div()
                .text_meta()
                .text_color(theme::ink_muted())
                .child("Imports are copied into the encrypted vault. Your original files are left as they are.")
        };
        let (width, card): (f32, AnyElement) = match open {
            ImportDialog::Choose(kind) => {
                let kind = *kind;
                let title = match kind {
                    ImportKind::Config => "Import hosts from OpenSSH",
                    ImportKind::KnownHosts => "Import known_hosts",
                };
                let cards = kind.choices().into_iter().enumerate().map(|(i, (name, place, about))| {
                    div()
                        .id(SharedString::from(format!("import-choice-{i}")))
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_1)
                        .p(theme::SPACE_4)
                        .bg(theme::card())
                        .border_1()
                        .border_color(theme::line())
                        .rounded(theme::RADIUS_LG)
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme::accent()))
                        .child(div().text_body().font_weight(FontWeight::MEDIUM).text_color(theme::ink()).child(name))
                        .child(div().text_mono_small().text_color(theme::ink_muted()).child(place))
                        .child(div().text_meta().text_color(theme::ink_muted()).child(about))
                        .on_click(cx.listener(move |w, _, _, cx| w.import_choice(kind, i, cx)))
                });
                (
                    520.,
                    dialog(false, title, "Choose where to import from.")
                        .child(div().flex().flex_col().gap(theme::SPACE_2).children(cards))
                        .child(note())
                        .child(dialog_buttons([cancel("import-cancel", "Cancel")]))
                        .into_any_element(),
                )
            }
            ImportDialog::Distro { kind, distros } => {
                let kind = *kind;
                let rows = distros.iter().enumerate().map(|(i, (distro, home))| {
                    let source = Source::Wsl { distro: distro.clone(), home: home.clone() };
                    div()
                        .id(SharedString::from(format!("import-distro-{i}")))
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_3)
                        .p(theme::SPACE_3)
                        .rounded(theme::RADIUS_MD)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::raised()))
                        .child(icon("terminal", px(16.), theme::ink_muted()))
                        .child(div().text_body().text_color(theme::ink()).child(SharedString::from(distro.clone())))
                        .on_click(cx.listener(move |w, _, _, cx| w.run_import(kind, source.clone(), cx)))
                });
                (
                    460.,
                    dialog(false, "Choose a WSL distribution", "Holonet reads ~/.ssh in its home folder.")
                        .child(div().flex().flex_col().children(rows))
                        .child(dialog_buttons([cancel("import-distro-cancel", "Cancel")]))
                        .into_any_element(),
                )
            }
            ImportDialog::Reading(what) => (
                460.,
                dialog(false, what.clone(), "WSL may take a few seconds to start.")
                    .child(dialog_buttons([cancel("import-reading-cancel", "Cancel")]))
                    .into_any_element(),
            ),
            ImportDialog::Failed(why) => (
                480.,
                dialog(false, "Nothing was imported", why.clone())
                    .child(dialog_buttons([cancel("import-failed-close", "Close")]))
                    .into_any_element(),
            ),
            ImportDialog::Preview { plan, checked, show_skipped } => {
                let count = checked.iter().filter(|c| **c).count();
                let rows = plan.hosts.iter().enumerate().map(|(i, h)| {
                    let mut detail = h.detail();
                    if !h.notes.is_empty() {
                        detail.push_str(&format!(" · {}", h.notes.join(" ")));
                    }
                    let this = cx.weak_entity();
                    pick_row(
                        format!("import-host-{i}"),
                        h.host.display_name().to_string(),
                        detail,
                        checked[i],
                        true,
                        move |v, _, cx| {
                            let _ = this.update(cx, |w, cx| {
                                if let Some(ImportDialog::Preview { checked, .. }) = w.imp.as_mut() {
                                    checked[i] = v;
                                }
                                cx.notify();
                            });
                        },
                    )
                });
                let body: AnyElement = if plan.hosts.is_empty() {
                    div()
                        .text_body()
                        .text_color(theme::ink_muted())
                        .child("This file names no hosts to import. Host lines with only patterns, such as Host *, set defaults.")
                        .into_any_element()
                } else {
                    div().id("import-list").max_h(px(340.)).overflow_y_scroll().children(rows).into_any_element()
                };
                let skipped = (!plan.skipped.is_empty()).then(|| {
                    let lines = plan.skipped.iter().map(|s| {
                        let at = s.source.as_ref().map(|src| format!(" (line {})", src.line)).unwrap_or_default();
                        div()
                            .text_meta()
                            .text_color(theme::ink_muted())
                            .child(SharedString::from(format!("{}{at}: {}", s.what, s.reason)))
                    });
                    let show = *show_skipped;
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_1)
                        .child(
                            div()
                                .id("import-skipped-toggle")
                                .text_meta()
                                .text_color(theme::accent())
                                .cursor_pointer()
                                .child(SharedString::from(if show {
                                    "Hide what is not imported".to_string()
                                } else {
                                    format!("Show what is not imported ({})", plan.skipped.len())
                                }))
                                .on_click(cx.listener(|w, _, _, cx| {
                                    if let Some(ImportDialog::Preview { show_skipped, .. }) = w.imp.as_mut() {
                                        *show_skipped = !*show_skipped;
                                    }
                                    cx.notify();
                                })),
                        )
                        .when(show, |d| {
                            d.child(div().id("import-skipped").max_h(px(160.)).overflow_y_scroll().children(lines))
                        })
                });
                let label = match count {
                    0 => "Import hosts".to_string(),
                    1 => "Import 1 host".to_string(),
                    n => format!("Import {n} hosts"),
                };
                (
                    640.,
                    dialog(
                        false,
                        "Import hosts from OpenSSH",
                        format!(
                            "From {}. Hosts come in without a group; keys they name are copied into the Keychain.",
                            plan.file
                        ),
                    )
                    .child(body)
                    .children(skipped)
                    .child(note())
                    .child(dialog_buttons([
                        cancel("import-preview-cancel", "Cancel"),
                        Button::primary("import-confirm", label)
                            .disabled(count == 0)
                            .on_click(cx.listener(|w, _, _, cx| w.finish_config_import(cx))),
                    ]))
                    .into_any_element(),
                )
            }
        };
        Some(modal("import-dialog", px(width), card, close).into_any_element())
    }

    /// Export known_hosts: every trusted key, to a file you choose.
    pub(crate) fn export_known_hosts(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.vault_data().map(import::export_known_hosts) else { return };
        let dir = std::env::var_os("USERPROFILE")
            .map(|p| PathBuf::from(p).join("Documents"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let answer = cx.prompt_for_new_path(&dir, Some("known_hosts"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = answer.await else { return };
            let result = std::fs::write(&path, text);
            let _ = this.update(cx, |w: &mut Workspace, cx| {
                match result {
                    Ok(()) => {
                        let path = super::short_path(&path.display().to_string(), 80);
                        w.notice = Some(format!("Saved the trusted keys to {path}.").into())
                    }
                    Err(e) => w.save_error = Some(format!("Holonet could not export known_hosts: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
