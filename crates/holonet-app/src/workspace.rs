//! The unlocked workspace: side navigation and pages. Phase 4 has the Hosts
//! empty state and Settings; phases 5 to 7 add the rest.

use std::path::PathBuf;

use gpui::{Context, Entity, IntoElement, Render, SharedString, Subscription, WeakEntity, Window, div, prelude::*, px};
use holonet_model::UnknownHostKeys;
use holonet_vault::{LockedVault, MIN_PASSWORD_LEN, UnlockedVault, VaultError};

use crate::app::HolonetApp;
use crate::shell::{NavEntry, SideNav};
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, CalloutKind, Select, SelectEvent, TextField, TextFieldEvent, callout, dialog, dialog_buttons, field, icon,
    modal, setting_card, setting_row, setting_row_with,
};

/// Shorten a long Windows path in the middle, keeping the drive and as many
/// trailing parts as fit: `C:\…\Holonet\vault.holonet`. GPUI cannot wrap
/// text that has no spaces.
pub fn short_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('\\').collect();
    let mut tail: Vec<&str> = Vec::new();
    // Drive, separator, ellipsis, separator.
    let mut len = parts[0].chars().count() + 3;
    for part in parts.iter().skip(1).rev() {
        let add = part.chars().count() + usize::from(!tail.is_empty());
        if len + add > max && !tail.is_empty() {
            break;
        }
        len += add;
        tail.insert(0, part);
    }
    format!("{}\\…\\{}", parts[0], tail.join("\\"))
}

/// Pages that exist in this build (milestone 2 plan: no control that does nothing).
const PAGES: [NavEntry; 2] = [NavEntry::Hosts, NavEntry::Settings];

struct ChangePassword {
    current: Entity<TextField>,
    new: Entity<TextField>,
    confirm: Entity<TextField>,
    problem: Option<SharedString>,
    busy: bool,
}

pub struct Workspace {
    app: WeakEntity<HolonetApp>,
    /// `None` only while a background task (password change) holds it.
    vault: Option<UnlockedVault>,
    path: PathBuf,
    page: NavEntry,
    restored_from_backup: bool,
    save_error: Option<SharedString>,
    notice: Option<SharedString>,
    unknown_keys: Entity<Select>,
    change_password: Option<ChangePassword>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        app: WeakEntity<HolonetApp>,
        vault: UnlockedVault,
        restored_from_backup: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let refuse = vault.data().settings.unknown_host_keys == UnknownHostKeys::Refuse;
        let unknown_keys = cx
            .new(|cx| Select::new(cx, vec!["Ask every time".into(), "Refuse to connect".into()], usize::from(refuse)));
        let subscriptions = vec![cx.subscribe(&unknown_keys, |this: &mut Self, _, event: &SelectEvent, cx| {
            let SelectEvent::Changed(i) = event;
            let policy = if *i == 1 { UnknownHostKeys::Refuse } else { UnknownHostKeys::Ask };
            this.save(|d| d.settings.unknown_host_keys = policy, cx);
        })];
        let path = vault.path().to_path_buf();
        Self {
            app,
            vault: Some(vault),
            path,
            page: NavEntry::Hosts,
            restored_from_backup,
            save_error: None,
            notice: None,
            unknown_keys,
            change_password: None,
            _subscriptions: subscriptions,
        }
    }

    /// Change the vault and save it, reporting a failed save.
    fn save(&mut self, change: impl FnOnce(&mut holonet_model::VaultData), cx: &mut Context<Self>) {
        let Some(vault) = self.vault.as_mut() else { return };
        self.save_error = match vault.update(change) {
            Ok(()) => None,
            Err(e) => Some(
                format!("Holonet could not save the vault: {e}. Your change is kept until you close Holonet.").into(),
            ),
        };
        cx.notify();
    }

    fn open_change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = cx.new(|cx| TextField::new(cx, "").password());
        let new = cx.new(|cx| TextField::new(cx, "").password());
        let confirm = cx.new(|cx| TextField::new(cx, "").password());
        for f in [&current, &new, &confirm] {
            self._subscriptions.push(cx.subscribe_in(f, window, |this, _, e: &TextFieldEvent, window, cx| match e {
                TextFieldEvent::Submitted => this.submit_change_password(window, cx),
                TextFieldEvent::Cancelled => {
                    this.change_password = None;
                    cx.notify()
                }
                TextFieldEvent::Changed => {}
            }));
        }
        current.read(cx).focus(window);
        self.change_password = Some(ChangePassword { current, new, confirm, problem: None, busy: false });
        cx.notify();
    }

    fn submit_change_password(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.change_password.as_mut() else { return };
        if form.busy {
            return;
        }
        let current = form.current.read(cx).text().to_string();
        let new = form.new.read(cx).text().to_string();
        let confirm = form.confirm.read(cx).text().to_string();
        if new.chars().count() < MIN_PASSWORD_LEN {
            form.problem = Some(format!("Use at least {MIN_PASSWORD_LEN} characters for the new password.").into());
        } else if new != confirm {
            form.problem = Some("The new passwords do not match.".into());
        } else if let Some(mut vault) = self.vault.take() {
            form.problem = None;
            form.busy = true;
            let path = self.path.clone();
            // Checking the current password and deriving the new key each take
            // about a third of a second: do both off the UI thread.
            let task = cx.background_executor().spawn(async move {
                let checked = LockedVault::open(&path).and_then(|locked| locked.unlock(&current).map(drop));
                let result = checked.and_then(|()| vault.change_password(&new));
                (vault, result)
            });
            cx.spawn(async move |this, cx| {
                let (vault, result) = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.vault = Some(vault);
                    match result {
                        Ok(()) => {
                            this.change_password = None;
                            this.notice = Some("Vault password changed.".into());
                        }
                        Err(e) => {
                            if let Some(form) = this.change_password.as_mut() {
                                form.busy = false;
                                form.problem = Some(match e {
                                    VaultError::WrongPassword => "The current password is wrong.".into(),
                                    e => e.to_string().into(),
                                });
                            }
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn hosts_page(&self) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme::SPACE_3)
            .child(icon("hosts", px(40.), theme::ink_faint()))
            .child(div().text_heading().text_color(theme::ink()).child("No hosts yet"))
            .child(div().text_body().text_color(theme::ink_muted()).child("Hosts you add appear here."))
    }

    fn settings_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.path.clone();
        div()
            .px(theme::SPACE_6)
            .py(px(28.))
            .max_w(px(820.))
            .flex()
            .flex_col()
            .gap(theme::SPACE_5)
            .child(div().text_title().text_color(theme::ink()).child("Settings"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_3)
                    .child(div().text_heading().text_color(theme::ink()).child("Vault and security"))
                    .child(setting_card(vec![
                        setting_row(
                            "Unknown host keys",
                            Some("Refuse to connect stops any server whose key is not already in Known hosts.".into()),
                            div().w(px(240.)).child(self.unknown_keys.clone()),
                        )
                        .into_any_element(),
                        setting_row(
                            "Vault password",
                            Some("Unlocks the vault. It cannot be recovered if forgotten.".into()),
                            Button::secondary("change-password", "Change password")
                                .on_click(cx.listener(|this, _, window, cx| this.open_change_password(window, cx))),
                        )
                        .into_any_element(),
                        setting_row_with(
                            "Vault file",
                            Some(
                                div()
                                    .text_mono_small()
                                    .text_color(theme::ink_muted())
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(SharedString::from(short_path(&path.display().to_string(), 64)))
                                    .into_any_element(),
                            ),
                            Button::secondary("show-vault", "Show in folder").on_click(move |_, _, _| {
                                let _ = std::process::Command::new("explorer")
                                    .arg(format!("/select,{}", path.display()))
                                    .spawn();
                            }),
                        )
                        .into_any_element(),
                    ])),
            )
    }

    fn change_password_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let form = self.change_password.as_ref()?;
        let busy = form.busy;
        let close = cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
            if !this.change_password.as_ref().is_some_and(|f| f.busy) {
                this.change_password = None;
                cx.notify();
            }
        });
        let entity = cx.entity();
        Some(
            modal(
                "change-password",
                px(460.),
                dialog(false, "Change vault password", "You will use the new password from the next unlock.")
                    .child(field("Current password", form.current.clone(), None))
                    .child(field("New password", form.new.clone(), Some(("At least 8 characters.".into(), false))))
                    .child(field("Confirm new password", form.confirm.clone(), form.problem.clone().map(|p| (p, true))))
                    .child(dialog_buttons([
                        Button::quiet("cp-cancel", "Cancel").disabled(busy).on_click(close),
                        Button::primary("cp-change", if busy { "Changing…" } else { "Change password" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit_change_password(window, cx))),
                    ])),
                move |_, cx| {
                    entity.update(cx, |this, cx| {
                        if !this.change_password.as_ref().is_some_and(|f| f.busy) {
                            this.change_password = None;
                            cx.notify();
                        }
                    })
                },
            )
            .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut notices = Vec::new();
        if self.restored_from_backup {
            notices.push((
                "restored",
                CalloutKind::Warning,
                SharedString::from(
                    "The vault file was damaged, so Holonet opened the previous save. Changes from your last session may be missing.",
                ),
            ));
        }
        if let Some(e) = &self.save_error {
            notices.push(("save-error", CalloutKind::Warning, e.clone()));
        }
        if let Some(n) = &self.notice {
            notices.push(("notice", CalloutKind::Success, n.clone()));
        }
        let notices = notices
            .into_iter()
            .map(|(id, kind, text)| {
                callout(kind).child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme::SPACE_3)
                        .child(div().flex_1().child(text))
                        .child(Button::quiet(id, "Dismiss").compact().on_click(cx.listener(move |this, _, _, cx| {
                            match id {
                                "restored" => this.restored_from_backup = false,
                                "save-error" => this.save_error = None,
                                _ => this.notice = None,
                            }
                            cx.notify();
                        }))),
                )
            })
            .collect::<Vec<_>>();

        let entity = cx.entity();
        let page = match self.page {
            NavEntry::Settings => self.settings_page(cx).into_any_element(),
            _ => self.hosts_page().into_any_element(),
        };
        let app = self.app.clone();
        let dialog = self.change_password_dialog(cx);

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                SideNav::new(Some(self.page), PAGES.to_vec())
                    .on_select({
                        move |entry, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.page = entry;
                                cx.notify();
                            })
                        }
                    })
                    .on_lock(move |_, window, cx| {
                        let _ = app.update(cx, |app, cx| app.lock(window, cx));
                    }),
            )
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .bg(theme::ground())
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(theme::SPACE_6)
                            .pt(theme::SPACE_4)
                            .flex()
                            .flex_col()
                            .gap(theme::SPACE_2)
                            .children(notices),
                    )
                    .child(div().flex_1().child(page)),
            )
            .children(dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::short_path;

    #[test]
    fn short_paths_keep_drive_and_tail() {
        let normal = r"C:\Users\me\AppData\Roaming\Holonet\vault.holonet";
        assert_eq!(short_path(normal, 100), normal);
        let long = r"C:\Users\LAWREN~1\AppData\Local\Temp\claude\scratchpad\vaulttest-2d2587\vault.holonet";
        let short = short_path(long, 40);
        assert!(short.starts_with(r"C:\…\") && short.ends_with(r"vaulttest-2d2587\vault.holonet"), "{short}");
        assert!(short.chars().count() <= 40, "{short}");
    }
}
