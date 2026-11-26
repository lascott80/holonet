//! New host and Edit host (NewHost.dc.html, EditHost.dc.html), with the
//! milestone 2 fields: label, address, port, tags, username, SSH key or
//! password, keep-alive. Later milestones add group, Tailscale, jump host,
//! startup snippet, SFTP folder and logging.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    Context, Entity, EventEmitter, IntoElement, PathPromptOptions, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use holonet_model::{Auth, Host, HostId, KeyId, KnownHost, Secret, StoredKey};
use holonet_ssh::{ConnectError, KeyStatus, PresentedKey, algorithm_label, classify};

use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, ButtonKind, CalloutKind, Checkbox, Segmented, Select, TextField, TextFieldEvent, callout, code_box, dialog,
    dialog_buttons, field, fieldset, key_values, modal, section_label,
};

pub enum HostFormEvent {
    /// `passphrase`: for an encrypted key, the passphrase to save on the key
    /// (`None` inside clears it). Passphrases belong to keys (schema 2).
    Saved {
        host: Box<Host>,
        new_keys: Vec<StoredKey>,
        passphrase: Option<(KeyId, Option<Secret>)>,
        connect: bool,
    },
    Cancelled,
    Deleted {
        id: HostId,
        forget_key: bool,
    },
}

enum TestState {
    Idle,
    Running,
    Reachable { key: PresentedKey, status: KeyStatus },
    Failed(SharedString),
}

pub struct HostForm {
    original: Option<Host>,
    label: Entity<TextField>,
    address: Entity<TextField>,
    port: Entity<TextField>,
    tags: Entity<TextField>,
    username: Entity<TextField>,
    password: Entity<TextField>,
    passphrase: Entity<TextField>,
    key_select: Entity<Select>,
    /// 0 = SSH key, 1 = password.
    auth: usize,
    keys: Vec<StoredKey>,
    new_keys: Vec<StoredKey>,
    save_password: bool,
    save_passphrase: bool,
    /// "Forget saved passphrase" was chosen for the selected key. The
    /// passphrase belongs to the key, which other hosts may share, so it is
    /// only cleared when asked.
    forget_passphrase: bool,
    keep_alive: bool,
    /// Show problems once Save has been tried.
    show_problems: bool,
    import_message: Option<(SharedString, bool)>,
    test: TestState,
    known_hosts: Vec<KnownHost>,
    connect_timeout: Duration,
    runtime: tokio::runtime::Handle,
    delete_dialog: Option<bool>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HostFormEvent> for HostForm {}

fn key_option(key: &StoredKey) -> SharedString {
    format!("{} ({})", key.name, algorithm_label(&key.algorithm)).into()
}

impl HostForm {
    /// `host` is `None` for a new host. `keys` and `known_hosts` are
    /// snapshots from the vault.
    pub fn new(
        host: Option<Host>,
        keys: Vec<StoredKey>,
        known_hosts: Vec<KnownHost>,
        connect_timeout: Duration,
        runtime: tokio::runtime::Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let h = host.clone().unwrap_or_else(|| Host::new(""));
        let text = |cx: &mut Context<Self>, placeholder: &str, value: String, mono: bool| {
            cx.new(|cx| {
                let f = TextField::new(cx, placeholder.to_string()).with_text(value);
                if mono { f.mono() } else { f }
            })
        };
        let label = text(cx, "Defaults to the address", h.label.clone(), false);
        let address = text(cx, "Hostname or IP address", h.address.clone(), true);
        let port = text(cx, "22", h.port.to_string(), true);
        let tags = text(cx, "Comma separated, e.g. api, arm64", h.tags.join(", "), false);
        let username = text(cx, "e.g. root", h.username.clone(), true);
        let (auth, saved_password, key_id, saved_passphrase) = match &h.auth {
            Auth::Password { saved } => (1, saved.clone(), None, None),
            Auth::Key { key_id, .. } => {
                let saved = keys.iter().find(|k| k.id == *key_id).and_then(|k| k.saved_passphrase.clone());
                (0, None, Some(*key_id), saved)
            }
        };
        // A new host defaults to a key when the vault has one.
        let auth = if host.is_none() { if keys.is_empty() { 1 } else { 0 } } else { auth };
        let password = cx.new(|cx| {
            TextField::new(cx, "Leave empty to be asked on connect")
                .password()
                .with_text(saved_password.as_ref().map(|s| s.expose().to_string()).unwrap_or_default())
        });
        let passphrase = cx.new(|cx| {
            TextField::new(cx, "Leave empty to be asked on connect")
                .password()
                .with_text(saved_passphrase.as_ref().map(|s| s.expose().to_string()).unwrap_or_default())
        });
        let selected = key_id.and_then(|id| keys.iter().position(|k| k.id == id)).unwrap_or(0);
        let key_select = cx.new(|cx| Select::new(cx, keys.iter().map(key_option).collect(), selected).mono());

        let mut subscriptions = Vec::new();
        for f in [&label, &address, &port, &tags, &username, &password, &passphrase] {
            subscriptions.push(cx.subscribe_in(f, window, |this: &mut Self, _, e: &TextFieldEvent, window, cx| {
                match e {
                    TextFieldEvent::Changed => {
                        // Any edit clears the test result (NewHost screen).
                        this.test = TestState::Idle;
                        cx.notify();
                    }
                    TextFieldEvent::Submitted => this.save(window, cx),
                    TextFieldEvent::Cancelled => cx.emit(HostFormEvent::Cancelled),
                }
            }));
        }
        subscriptions.push(cx.observe(&key_select, |this: &mut Self, _, cx| {
            this.test = TestState::Idle;
            cx.notify();
        }));
        if host.is_none() {
            address.read(cx).focus(window);
        } else {
            label.read(cx).focus(window);
        }
        Self {
            save_password: host.is_none() || saved_password.is_some(),
            save_passphrase: host.is_none() || saved_passphrase.is_some(),
            forget_passphrase: false,
            keep_alive: h.keep_alive,
            original: host,
            label,
            address,
            port,
            tags,
            username,
            password,
            passphrase,
            key_select,
            auth,
            keys,
            new_keys: Vec::new(),
            show_problems: false,
            import_message: None,
            test: TestState::Idle,
            known_hosts,
            connect_timeout,
            runtime,
            delete_dialog: None,
            _subscriptions: subscriptions,
        }
    }

    fn text(&self, field: &Entity<TextField>, cx: &Context<Self>) -> String {
        field.read(cx).text().trim().to_string()
    }

    fn selected_key(&self, cx: &Context<Self>) -> Option<&StoredKey> {
        self.keys.get(self.key_select.read(cx).selected())
    }

    fn port(&self, cx: &Context<Self>) -> Option<u16> {
        let text = self.text(&self.port, cx);
        if text.is_empty() { Some(22) } else { text.parse::<u16>().ok().filter(|p| *p > 0) }
    }

    /// The host as the form currently describes it.
    fn build(&self, cx: &Context<Self>) -> Host {
        let mut host = self.original.clone().unwrap_or_else(|| Host::new(""));
        host.label = self.text(&self.label, cx);
        host.address = self.text(&self.address, cx);
        host.port = self.port(cx).unwrap_or(0);
        host.username = self.text(&self.username, cx);
        host.tags =
            self.text(&self.tags, cx).split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
        host.keep_alive = self.keep_alive;
        host.auth = if self.auth == 1 {
            let password = self.password.read(cx).text().to_string();
            Auth::Password { saved: (self.save_password && !password.is_empty()).then(|| Secret::new(password)) }
        } else {
            Auth::key(self.selected_key(cx).map(|k| k.id).unwrap_or_default())
        };
        host
    }

    /// The passphrase to save on the chosen key, when it has one.
    fn key_passphrase(&self, cx: &Context<Self>) -> Option<(KeyId, Option<Secret>)> {
        let key = self.selected_key(cx).filter(|k| self.auth == 0 && k.encrypted)?;
        let passphrase = self.passphrase.read(cx).text().to_string();
        if self.save_passphrase && !passphrase.is_empty() {
            Some((key.id, Some(Secret::new(passphrase))))
        } else if self.forget_passphrase && key.saved_passphrase.is_some() {
            Some((key.id, None))
        } else {
            // Nothing typed and nothing unticked: leave the key as it is.
            None
        }
    }

    /// Problems that stop saving, as interface text.
    fn problems(&self, cx: &Context<Self>) -> Vec<SharedString> {
        let mut problems: Vec<SharedString> =
            self.build(cx).problems().into_iter().map(|p| p.message().into()).collect();
        if self.port(cx).is_none() && !problems.iter().any(|p| p.contains("port")) {
            problems.push("Use a port from 1 to 65535.".into());
        }
        if self.auth == 0 && self.selected_key(cx).is_none() {
            problems.push("Import a key, or sign in with a password.".into());
        }
        problems
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_with(false, window, cx)
    }

    fn save_with(&mut self, connect: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_problems = true;
        if self.problems(cx).is_empty() {
            let host = self.build(cx);
            // Only keys this host uses, among the ones imported here, are kept.
            let used = match &host.auth {
                Auth::Key { key_id, .. } => Some(*key_id),
                Auth::Password { .. } => None,
            };
            let new_keys = self.new_keys.iter().filter(|k| Some(k.id) == used).cloned().collect();
            let passphrase = self.key_passphrase(cx);
            cx.emit(HostFormEvent::Saved { host: Box::new(host), new_keys, passphrase, connect });
        }
        cx.notify();
    }

    fn import_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import key".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, _window, cx| {
                this.add_key_file(path, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn add_key_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The same reading as the Keychain: PuTTY keys are converted.
        let found = crate::key_import::read_file(&path, "");
        let info = match &found.result {
            Err(e) => {
                self.import_message = Some((e.clone().into(), true));
                return;
            }
            Ok(info) if info.ppk && info.encrypted => {
                self.import_message = Some((
                    "This PuTTY key has a passphrase. Import it in the Keychain, which asks for it, then choose it here."
                        .into(),
                    true,
                ));
                return;
            }
            Ok(info) => info.clone(),
        };
        let existing = self.keys.iter().position(|k| k.public_key == info.public_key);
        let index = match existing {
            Some(i) => {
                self.import_message = Some(("This key is already in the vault; it is now selected.".into(), false));
                i
            }
            None => {
                let key = match crate::key_import::stored_key(&found, None) {
                    Ok(key) => key,
                    Err(e) => {
                        self.import_message = Some((e.to_string().into(), true));
                        return;
                    }
                };
                self.import_message = Some((
                    format!(
                        "Imported {} ({}). It is copied into the vault when you save; the file is left as it is.",
                        key.name,
                        algorithm_label(&key.algorithm)
                    )
                    .into(),
                    false,
                ));
                self.new_keys.push(key.clone());
                self.keys.push(key);
                self.keys.len() - 1
            }
        };
        let options = self.keys.iter().map(key_option).collect();
        self.key_select.update(cx, |s, cx| s.set_options(options, index, cx));
        self.auth = 0;
        self.test = TestState::Idle;
    }

    fn run_test(&mut self, cx: &mut Context<Self>) {
        let address = self.text(&self.address, cx);
        let Some(port) = self.port(cx) else { return };
        if address.is_empty() || matches!(self.test, TestState::Running) {
            self.show_problems = true;
            cx.notify();
            return;
        }
        self.test = TestState::Running;
        let timeout = self.connect_timeout;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let _ = tx.send(holonet_ssh::probe(&address, port, timeout).await);
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|_| Err(ConnectError::Protocol("test stopped".into())));
            let _ = this.update(cx, |this, cx| {
                this.test = match result {
                    Ok(key) => {
                        let known = this.known_hosts.iter().find(|k| k.matches(&key.host, key.port));
                        let status = classify(known, &key);
                        TestState::Reachable { key, status }
                    }
                    Err(e) => TestState::Failed(describe_error(&e).into()),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn test_result(&self) -> Option<gpui::Div> {
        Some(match &self.test {
            TestState::Idle => return None,
            TestState::Running => callout(CalloutKind::Info).child("Testing the connection…"),
            TestState::Reachable { key, status } => {
                let fingerprint = div()
                    .text_mono_small()
                    .text_color(theme::ink_soft())
                    .child(SharedString::from(format!("{} {}", key.key_type_label(), key.fingerprint)));
                match status {
                    KeyStatus::Unknown => callout(CalloutKind::Success)
                        .child(div().text_color(theme::ink()).child("Reachable. Host key not yet trusted"))
                        .child(fingerprint)
                        .child(
                            div()
                                .text_meta()
                                .text_color(theme::ink_muted())
                                .child("You will be asked to verify this fingerprint on first connect."),
                        ),
                    KeyStatus::Trusted => callout(CalloutKind::Success)
                        .child(div().text_color(theme::ink()).child("Reachable. The host key matches Known hosts"))
                        .child(fingerprint),
                    KeyStatus::Changed { .. } => callout(CalloutKind::Warning)
                        .child(div().child("Reachable, but the host key has changed"))
                        .child(fingerprint)
                        .child(
                            div()
                                .text_meta()
                                .child("Holonet will stop the connection until you review the key in Known hosts."),
                        ),
                }
            }
            TestState::Failed(message) => callout(CalloutKind::Warning).child(message.clone()),
        })
    }

    fn auth_summary(&self, cx: &Context<Self>) -> SharedString {
        if self.auth == 1 {
            let saved = self.save_password && !self.password.read(cx).text().is_empty();
            if saved { "Password, saved".into() } else { "Password, asked on connect".into() }
        } else {
            match self.selected_key(cx) {
                Some(k) => k.name.clone().into(),
                None => "No key yet".into(),
            }
        }
    }
}

/// Interface text for a failed connection or test.
pub fn describe_error(e: &ConnectError) -> String {
    match e {
        ConnectError::Resolve { host, .. } => format!("Holonet could not find {host}. Check the address."),
        ConnectError::Refused => "The host refused the connection. Check the port, and that SSH is running.".into(),
        ConnectError::TimedOut(s) => format!("No answer after {s} seconds."),
        ConnectError::Network(d) => format!("Network error: {d}"),
        other => other.to_string(),
    }
}

impl Render for HostForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.original.is_some();
        let host = self.build(cx);
        let title: SharedString = if !host.label.is_empty() {
            host.label.clone().into()
        } else if !host.address.is_empty() {
            host.address.clone().into()
        } else {
            "Untitled host".into()
        };
        let problems = if self.show_problems { self.problems(cx) } else { Vec::new() };
        let address_problem = self.show_problems && host.problems().iter().any(|p| p.message().contains("address"));
        self.address.update(cx, |f, cx| f.set_problem(address_problem, cx));
        let narrow = window.viewport_size().width < px(1100.);

        // ------------------------------------------------------ fieldsets --
        let general = fieldset("General")
            .child(field("Label", self.label.clone(), None))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::SPACE_4)
                    .child(div().flex_grow().min_w(px(240.)).child(field(
                        "Primary address (required)",
                        self.address.clone(),
                        None,
                    )))
                    .child(div().w(px(110.)).child(field("Port", self.port.clone(), None))),
            )
            .child(field("Tags", self.tags.clone(), None));

        let entity = cx.entity();
        let auth_choice = Segmented::new("auth", vec!["SSH key".into(), "Password".into()], self.auth).on_change({
            let entity = entity.clone();
            move |i, _, cx| {
                entity.update(cx, |f, cx| {
                    f.auth = i;
                    f.test = TestState::Idle;
                    cx.notify()
                })
            }
        });
        let mut credentials = fieldset("Credentials")
            .child(field("Username", self.username.clone(), None))
            .child(field("Authentication", auth_choice, None));
        if self.auth == 0 {
            let encrypted = self.selected_key(cx).is_some_and(|k| k.encrypted);
            let import = Button::secondary("import-key", "Import key file…")
                .on_click(cx.listener(|this, _, window, cx| this.import_key(window, cx)));
            credentials = if self.keys.is_empty() {
                credentials.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_2)
                        .child(callout(CalloutKind::Info).child(
                            "The vault has no keys yet. Import a private key file, such as one from your .ssh folder.",
                        ))
                        .child(div().flex().child(import)),
                )
            } else {
                credentials.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_end()
                        .gap(theme::SPACE_3)
                        .child(div().flex_grow().min_w(px(240.)).child(field("Key", self.key_select.clone(), None)))
                        .child(import),
                )
            };
            if let Some((message, problem)) = &self.import_message {
                credentials = credentials.child(
                    callout(if *problem { CalloutKind::Warning } else { CalloutKind::Info }).child(message.clone()),
                );
            }
            let saved_on_key = self.selected_key(cx).is_some_and(|k| k.saved_passphrase.is_some());
            if encrypted && saved_on_key && !self.forget_passphrase {
                credentials =
                    credentials.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme::SPACE_3)
                            .child(
                                div().flex_1().text_body().text_color(theme::ink_muted()).child(
                                    "This key's passphrase is saved in the vault, so it unlocks without asking.",
                                ),
                            )
                            .child(Button::quiet("forget-passphrase", "Forget saved passphrase").compact().on_click({
                                let entity = entity.clone();
                                move |_, _, cx| {
                                    entity.update(cx, |f, cx| {
                                        f.forget_passphrase = true;
                                        f.save_passphrase = false;
                                        cx.notify()
                                    })
                                }
                            })),
                    );
            } else if encrypted {
                credentials = credentials.child(field("Key passphrase", self.passphrase.clone(), None)).child(
                    Checkbox::new("save-passphrase", "Save passphrase in the encrypted vault", self.save_passphrase)
                        .on_toggle({
                            let entity = entity.clone();
                            move |v, _, cx| {
                                entity.update(cx, |f, cx| {
                                    f.save_passphrase = v;
                                    cx.notify()
                                })
                            }
                        }),
                );
            }
        } else {
            credentials = credentials.child(field("Password", self.password.clone(), None)).child(
                Checkbox::new("save-password", "Save in the encrypted vault", self.save_password).on_toggle({
                    let entity = entity.clone();
                    move |v, _, cx| {
                        entity.update(cx, |f, cx| {
                            f.save_password = v;
                            cx.notify()
                        })
                    }
                }),
            );
        }

        let connection = fieldset("Connection").child(
            Checkbox::new("keep-alive", "Keep connection alive", self.keep_alive).on_toggle({
                let entity = entity.clone();
                move |v, _, cx| {
                    entity.update(cx, |f, cx| {
                        f.keep_alive = v;
                        cx.notify()
                    })
                }
            }),
        );

        // --------------------------------------------------------- aside --
        let mut aside = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(theme::SPACE_5)
            .py(px(28.))
            .bg(theme::panel())
            .when(!narrow, |d| d.w(px(320.)).flex_none().h_full().shadow(theme::shadow_panel()))
            .when(narrow, |d| d.w_full().rounded(theme::RADIUS_LG))
            .child(div().text_heading().text_color(theme::ink()).child(title))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(section_label("Equivalent command"))
                    .child(code_box(host.ssh_command())),
            )
            .child(key_values(vec![
                ("Authentication".into(), self.auth_summary(cx).into_any_element()),
                ("Keep alive".into(), (if self.keep_alive { "On" } else { "Off" }).into_any_element()),
            ]));
        for problem in &problems {
            aside = aside.child(div().text_body().text_color(theme::warn()).child(problem.clone()));
        }
        aside = aside
            .child(
                Button::secondary(
                    "test",
                    if matches!(self.test, TestState::Running) { "Testing…" } else { "Test connection" },
                )
                .full_width()
                .disabled(matches!(self.test, TestState::Running))
                .on_click(cx.listener(|this, _, _, cx| this.run_test(cx))),
            )
            .children(self.test_result())
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(
                        Button::primary("save-connect", "Save and connect")
                            .full_width()
                            .on_click(cx.listener(|this, _, window, cx| this.save_with(true, window, cx))),
                    )
                    .child(
                        Button::secondary("save", if editing { "Save changes" } else { "Save" })
                            .full_width()
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(editing, |d| {
                        d.child(
                            Button::new("delete", "Delete host", ButtonKind::DestructiveOutline).full_width().on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.delete_dialog = Some(false);
                                    cx.notify()
                                }),
                            ),
                        )
                    })
                    .child(
                        Button::quiet("cancel", "Cancel")
                            .full_width()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HostFormEvent::Cancelled))),
                    ),
            );

        // --------------------------------------------------------- page --
        let breadcrumb = div()
            .flex()
            .items_center()
            .gap(theme::SPACE_2)
            .text_meta()
            .child(
                div()
                    .id("crumb-hosts")
                    .text_color(theme::accent())
                    .cursor_pointer()
                    .child("Hosts")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HostFormEvent::Cancelled))),
            )
            .child(div().text_color(theme::ink_faint()).child("/"))
            .child(div().text_color(theme::ink_muted()).child(if editing {
                SharedString::from(host_name(self.original.as_ref().unwrap()))
            } else {
                "New host".into()
            }));
        let form = div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .px(theme::SPACE_6)
            .py(px(28.))
            .flex_1()
            .min_w(px(360.))
            .max_w(px(820.))
            .child(div().flex().flex_col().gap(theme::SPACE_1).child(breadcrumb).child(
                div().text_title().text_color(theme::ink()).child(if editing { "Edit host" } else { "New host" }),
            ))
            .child(general)
            .child(credentials)
            .child(connection);

        let delete = self.delete_dialog.map(|forget| {
            let name = host_name(self.original.as_ref().unwrap());
            let close = {
                let entity = entity.clone();
                move |_: &mut Window, cx: &mut gpui::App| {
                    entity.update(cx, |f, cx| {
                        f.delete_dialog = None;
                        cx.notify()
                    })
                }
            };
            modal(
                "delete-host",
                px(460.),
                dialog(true, format!("Delete {name}?"), "The host is removed from the vault. This cannot be undone.")
                    .child(Checkbox::new("forget-key", "Also forget its server key in Known hosts", forget).on_toggle(
                        {
                            let entity = entity.clone();
                            move |v, _, cx| {
                                entity.update(cx, |f, cx| {
                                    f.delete_dialog = Some(v);
                                    cx.notify()
                                })
                            }
                        },
                    ))
                    .child(dialog_buttons([
                        Button::secondary("delete-cancel", "Cancel").on_click({
                            let close = close.clone();
                            move |_, window, cx| close(window, cx)
                        }),
                        Button::new("delete-confirm", "Delete host", ButtonKind::DestructiveFill).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(original) = &this.original {
                                    cx.emit(HostFormEvent::Deleted { id: original.id, forget_key: forget });
                                }
                            }),
                        ),
                    ])),
                close,
            )
        });

        // On a narrow window the aside stacks below the form (NewHost wraps).
        let (aside_below, aside_beside) = if narrow { (Some(aside), None) } else { (None, Some(aside)) };
        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div()
                    .id("host-form-scroll")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(form)
                    .children(aside_below.map(|a| div().px(theme::SPACE_6).pb(theme::SPACE_6).child(a))),
            )
            .children(aside_beside)
            .children(delete)
    }
}

pub fn host_name(host: &Host) -> String {
    host.display_name().to_string()
}
