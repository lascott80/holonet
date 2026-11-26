//! The unlocked workspace: side navigation and pages (Hosts and Groups, the
//! host form, Keychain, Known hosts, Settings).

mod forwarding;
mod groups;
mod hosts;
mod keychain;
mod known_hosts;
mod settings;

pub use groups::GroupTag;
pub use hosts::format_date;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Context, Entity, IntoElement, Render, SharedString, Subscription, WeakEntity, Window, div, prelude::*, px};
use holonet_model::{Host, HostId};
use holonet_vault::{LockedVault, MIN_PASSWORD_LEN, UnlockedVault, VaultError};

use crate::app::HolonetApp;
use crate::host_form::{HostForm, HostFormEvent};
use crate::shell::{NavEntry, SideNav};
use crate::theme;
use crate::ui::{Button, CalloutKind, TextField, TextFieldEvent, callout, dialog, dialog_buttons, field, modal};

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
const PAGES: [NavEntry; 5] =
    [NavEntry::Hosts, NavEntry::Keychain, NavEntry::PortForwarding, NavEntry::KnownHosts, NavEntry::Settings];

pub(crate) enum Page {
    Hosts,
    Groups,
    Forwarding,
    Form(Entity<HostForm>),
    Keychain,
    KnownHosts,
    Settings,
}

/// Dialog open on the Known hosts page.
#[derive(Clone, Copy)]
pub(crate) enum KhDialog {
    Forget,
    Review { expected: bool },
}

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
    page: Page,
    runtime: tokio::runtime::Handle,
    /// Hosts page: search field, cards or list, selected host.
    search: Entity<TextField>,
    cards: bool,
    selected: Option<HostId>,
    /// Hosts page: the group card picked to filter the hosts.
    group_filter: Option<holonet_model::GroupId>,
    groups_ui: groups::GroupsUi,
    forwarder: Entity<crate::forwards::Forwarder>,
    fw: forwarding::ForwardingUi,
    /// Known hosts page: search, filter pill, selected entry, open dialog.
    kh_search: Entity<TextField>,
    kh_filter: usize,
    kh_selected: Option<(String, u16)>,
    kh_dialog: Option<KhDialog>,
    restored_from_backup: bool,
    save_error: Option<SharedString>,
    notice: Option<SharedString>,
    settings_ui: settings::SettingsUi,
    kc: keychain::KeychainUi,
    change_password: Option<ChangePassword>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        app: WeakEntity<HolonetApp>,
        vault: UnlockedVault,
        restored_from_backup: bool,
        runtime: tokio::runtime::Handle,
        forwarder: Entity<crate::forwards::Forwarder>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings_ui = settings::SettingsUi::new(&vault.data().settings, cx);
        let settings_subs = settings_ui.subscribe(cx);
        let search = cx.new(|cx| TextField::new(cx, "Find a host"));
        let kh_search = cx.new(|cx| TextField::new(cx, "Filter by host, address or fingerprint"));
        let subscriptions = vec![
            cx.subscribe(&search, |_: &mut Self, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Cancelled => field.update(cx, |f, cx| f.set_text("", cx)),
                _ => cx.notify(),
            }),
            cx.subscribe(&kh_search, |_: &mut Self, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Cancelled => field.update(cx, |f, cx| f.set_text("", cx)),
                _ => cx.notify(),
            }),
        ];
        let path = vault.path().to_path_buf();
        let kc = keychain::KeychainUi::new(cx);
        let groups_ui = groups::GroupsUi::new(cx);
        let groups_subs = groups_ui.subscribe(cx);
        let fw = forwarding::ForwardingUi::new(cx);
        let fw_subs = fw.subscribe(&forwarder, cx);
        Self {
            app,
            vault: Some(vault),
            path,
            page: Page::Hosts,
            runtime,
            search,
            cards: true,
            selected: None,
            group_filter: None,
            groups_ui,
            forwarder,
            fw,
            kh_search,
            kh_filter: 0,
            kh_selected: None,
            kh_dialog: None,
            restored_from_backup,
            save_error: None,
            notice: None,
            settings_ui,
            kc,
            change_password: None,
            _subscriptions: subscriptions.into_iter().chain(settings_subs).chain(groups_subs).chain(fw_subs).collect(),
        }
    }

    /// Change the vault and save it, reporting a failed save.
    pub(crate) fn save(&mut self, change: impl FnOnce(&mut holonet_model::VaultData), cx: &mut Context<Self>) {
        let Some(vault) = self.vault.as_mut() else { return };
        self.save_error = match vault.update(change) {
            Ok(()) => None,
            Err(e) => Some(
                format!("Holonet could not save the vault: {e}. Your change is kept until you close Holonet.").into(),
            ),
        };
        cx.notify();
    }

    /// New host (`None`) or Edit host.
    pub(crate) fn open_form(&mut self, host: Option<Host>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault.as_ref().map(|v| v.data()) else { return };
        let timeout = Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64);
        let runtime = self.runtime.clone();
        let form = cx.new(|cx| HostForm::new(host, data, timeout, runtime, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            |this: &mut Self, _, event: &HostFormEvent, window, cx| {
                match event {
                    HostFormEvent::Saved { host, new_keys, passphrase, connect } => {
                        let (host, new_keys, passphrase) = ((**host).clone(), new_keys.clone(), passphrase.clone());
                        let id = host.id;
                        this.save(
                            move |d| {
                                d.keys.extend(new_keys);
                                if let Some((key_id, secret)) = passphrase
                                    && let Some(key) = d.key_mut(key_id)
                                {
                                    key.saved_passphrase = secret;
                                }
                                match d.host_mut(id) {
                                    Some(existing) => *existing = host,
                                    None => d.hosts.push(host),
                                }
                            },
                            cx,
                        );
                        this.selected = Some(id);
                        if *connect {
                            this.connect(id, window, cx);
                        }
                    }
                    HostFormEvent::Deleted { id, forget_key } => {
                        let (id, forget) = (*id, *forget_key);
                        this.save(move |d| drop(d.delete_host(id, forget)), cx);
                        this.selected = None;
                    }
                    HostFormEvent::Cancelled => {}
                }
                this.page = Page::Hosts;
                cx.notify();
            },
        ));
        self.page = Page::Form(form);
        cx.notify();
    }

    pub(crate) fn vault_data(&self) -> Option<&holonet_model::VaultData> {
        self.vault.as_ref().map(|v| v.data())
    }

    /// Open Known hosts with one entry selected.
    pub(crate) fn show_known_host(&mut self, host: &str, port: u16, cx: &mut Context<Self>) {
        self.kh_filter = 0;
        self.kh_selected = Some((host.to_ascii_lowercase(), port));
        self.page = Page::KnownHosts;
        cx.notify();
    }

    pub(crate) fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    /// Ask the app to open a session tab. Deferred: the app reads this
    /// workspace while connecting, which would clash with this update.
    pub(crate) fn connect(&mut self, id: HostId, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| app.connect(id, window, cx));
        });
    }

    /// Ask the app to open an SFTP tab (deferred, as [`Self::connect`]).
    pub(crate) fn open_files(&mut self, id: HostId, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| app.open_files(id, Vec::new(), window, cx));
        });
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let active = match self.page {
            Page::Settings => NavEntry::Settings,
            Page::KnownHosts => NavEntry::KnownHosts,
            Page::Keychain => NavEntry::Keychain,
            Page::Forwarding => NavEntry::PortForwarding,
            _ => NavEntry::Hosts,
        };
        let page = match &self.page {
            Page::Settings => self.settings_page(window, cx).into_any_element(),
            Page::Form(form) => form.clone().into_any_element(),
            Page::Hosts => self.hosts_page(window, cx),
            Page::Groups => self.groups_page(cx),
            Page::Forwarding => self.forwarding_page(window, cx),
            Page::KnownHosts => self.known_hosts_page(cx),
            Page::Keychain => self.keychain_page(cx),
        };
        let app = self.app.clone();
        let dialog = self.change_password_dialog(cx).or_else(|| self.settings_dialog(cx));

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                SideNav::new(Some(active), PAGES.to_vec())
                    .hello(self.vault.as_ref().is_some_and(|v| v.has_hello()))
                    .on_select({
                        move |entry, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.page = match entry {
                                    NavEntry::Settings => Page::Settings,
                                    NavEntry::KnownHosts => Page::KnownHosts,
                                    NavEntry::Keychain => Page::Keychain,
                                    NavEntry::PortForwarding => Page::Forwarding,
                                    _ => Page::Hosts,
                                };
                                cx.notify();
                            })
                        }
                    })
                    .on_lock(move |_, window, cx| {
                        let _ = app.update(cx, |app, cx| app.lock(window, cx));
                    }),
            )
            .child(
                // Pages scroll themselves (Hosts and the form keep their
                // detail panel fixed beside the scrolling list).
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(theme::ground())
                    .flex()
                    .flex_col()
                    .when(!notices.is_empty(), |d| {
                        d.child(
                            div()
                                .px(theme::SPACE_6)
                                .pt(theme::SPACE_4)
                                .flex()
                                .flex_col()
                                .gap(theme::SPACE_2)
                                .children(notices),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(page)),
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
