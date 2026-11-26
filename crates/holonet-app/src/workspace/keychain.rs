//! Keychain (Keychain.dc.html): the keys in the vault, identities (shared
//! sign-ins), the selected key's public key with Copy and Install on host,
//! imports from a file, `~/.ssh` or WSL, and the Generate key panel.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, App, ClipboardItem, Context, Div, Entity, FontWeight, IntoElement, PathPromptOptions, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use holonet_model::{Auth, HostId, Identity, IdentityId, KeyId, Secret, StoredKey, Timestamp, VaultData};
use holonet_ssh::{
    BoxFuture, ConnectError, ConnectOptions, HostKeyDecision, InstallOutcome, KeyKind, KeyStatus, PresentedKey,
    Prompts, SecretRequest, algorithm_label,
};

use super::hosts::format_date;
use super::{Workspace, short_path};
use crate::key_import::{self, Found};
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, ButtonKind, Checkbox, Segmented, Select, TextField, TextFieldEvent, callout, dialog, dialog_buttons, field,
    icon, modal, section_label,
};

const ASIDE: f32 = 320.;

/// The Keychain page's own state.
pub(crate) struct KeychainUi {
    selected: Option<KeyId>,
    gen_name: Entity<TextField>,
    gen_passphrase: Entity<TextField>,
    gen_kind: usize,
    gen_hello: bool,
    generating: bool,
    gen_problem: Option<SharedString>,
    /// "Copied" shows on the Copy button until this generation passes.
    copied: Option<u64>,
    copy_generation: u64,
    dialog: Option<KcDialog>,
    _subscriptions: Vec<Subscription>,
}

enum KcDialog {
    /// Looking for keys (reading a folder, or asking WSL).
    Scanning(&'static str),
    /// Keys found in `~/.ssh` or WSL, to tick and import.
    Pick {
        title: SharedString,
        found: Vec<Found>,
        checked: Vec<bool>,
        empty: SharedString,
    },
    /// A PuTTY key file that needs its passphrase to be converted.
    PpkPassphrase {
        found: Box<Found>,
        field: Entity<TextField>,
        problem: Option<SharedString>,
    },
    Rename {
        key: KeyId,
        field: Entity<TextField>,
    },
    Delete {
        key: KeyId,
    },
    Install {
        key: KeyId,
        hosts: Vec<HostId>,
        select: Entity<Select>,
        secret: Entity<TextField>,
        state: InstallState,
    },
    Identity(Box<IdentityForm>),
    DeleteIdentity {
        id: IdentityId,
    },
}

/// Create or edit an identity (undrawn: built from the host form's fields).
struct IdentityForm {
    /// `None` while creating.
    id: Option<IdentityId>,
    name: Entity<TextField>,
    username: Entity<TextField>,
    /// 0 = SSH key, 1 = password, as on the host form.
    auth: usize,
    key_ids: Vec<KeyId>,
    key_select: Entity<Select>,
    password: Entity<TextField>,
    save_password: bool,
    problem: Option<SharedString>,
}

#[derive(Clone)]
enum InstallState {
    Ready,
    Running,
    Done(SharedString),
    Failed(SharedString),
}

impl KeychainUi {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let gen_name = cx.new(|cx| TextField::new(cx, "e.g. surface-ed25519"));
        let gen_passphrase = cx.new(|cx| TextField::new(cx, "Optional but recommended").password());
        let subs = vec![
            cx.subscribe(&gen_name, |w: &mut Workspace, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Submitted => w.generate_key(cx),
                _ => {
                    w.kc.gen_problem = None;
                    cx.notify()
                }
            }),
            cx.subscribe(&gen_passphrase, |w: &mut Workspace, _, e: &TextFieldEvent, cx| {
                if let TextFieldEvent::Submitted = e {
                    w.generate_key(cx)
                }
            }),
        ];
        Self {
            selected: None,
            gen_name,
            gen_passphrase,
            gen_kind: 0,
            gen_hello: true,
            generating: false,
            gen_problem: None,
            copied: None,
            copy_generation: 0,
            dialog: None,
            _subscriptions: subs,
        }
    }
}

/// "Protected by", as drawn: a key without a passphrase is a warning.
fn protected_by(key: &StoredKey, hello: bool) -> (&'static str, bool) {
    match (key.encrypted, key.saved_passphrase.is_some(), hello) {
        (false, _, _) => ("No passphrase", true),
        (true, true, true) => ("Passphrase + Windows Hello", false),
        (true, true, false) => ("Passphrase, saved in vault", false),
        (true, false, _) => ("Passphrase", false),
    }
}

/// The type column: "Ed25519", "RSA 4096", "ECDSA P-256".
fn type_label(key: &StoredKey) -> String {
    let label = algorithm_label(&key.algorithm);
    if label != "RSA" {
        return label;
    }
    // RSA's size is in the public key: the modulus is the last field.
    rsa_bits(&key.public_key).map(|b| format!("RSA {b}")).unwrap_or(label)
}

fn rsa_bits(public_key: &str) -> Option<usize> {
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD.decode(public_key.split_whitespace().nth(1)?).ok()?;
    // string "ssh-rsa", mpint e, mpint n
    let mut rest = blob.as_slice();
    let mut fields = Vec::new();
    for _ in 0..3 {
        let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        fields.push(rest.get(4..4 + len)?);
        rest = &rest[4 + len..];
    }
    let n = fields[2];
    let n = &n[n.iter().position(|b| *b != 0)?..];
    Some(n.len() * 8 - n[0].leading_zeros() as usize)
}

/// Full public key line for copying and installing: key and comment.
fn public_line(key: &StoredKey) -> String {
    if key.comment.trim().is_empty() {
        key.public_key.clone()
    } else {
        format!("{} {}", key.public_key, key.comment.trim())
    }
}

/// "nas-01", "nas-01 and pi", "nas-01 and 3 more".
fn name_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} more", rest.len()),
    }
}

/// What stopped an Install on host connection at a hop.
#[derive(Default)]
struct InstallStops {
    /// A host key that is not trusted, and the hop's name.
    key: Option<(String, KeyStatus)>,
    /// A hop that needed a secret Holonet does not have.
    secret: Option<String>,
}

/// Prompts for one hop of Install on host: only an already-trusted host key
/// is accepted (a new or changed key is verified by connecting from Hosts),
/// trusted under this address or the host's other one, as panes do. A
/// secret that is not saved comes from the dialog's field, for the host
/// itself only.
struct InstallPrompts {
    name: String,
    known: Vec<holonet_model::KnownHost>,
    /// The host's other addresses, which share a matching key.
    host: holonet_model::Host,
    secret: Option<Secret>,
    stops: Arc<Mutex<InstallStops>>,
}

impl Prompts for InstallPrompts {
    fn host_key(&self, key: PresentedKey) -> BoxFuture<HostKeyDecision> {
        let aliases = VaultData::other_addresses(&self.host, &key.host);
        let trusted = self
            .known
            .iter()
            .find(|k| k.matches(&key.host, key.port))
            .or_else(|| aliases.iter().find_map(|a| self.known.iter().find(|k| k.matches(a, key.port))));
        let status = holonet_ssh::classify(trusted, &key);
        let decision = if status == KeyStatus::Trusted {
            HostKeyDecision::Continue
        } else {
            self.stops.lock().unwrap().key = Some((self.name.clone(), status));
            HostKeyDecision::Stop
        };
        Box::pin(async move { decision })
    }

    fn secret(&self, _request: SecretRequest) -> BoxFuture<Option<Secret>> {
        let secret = self.secret.clone().filter(|s| !s.is_empty());
        if secret.is_none() {
            self.stops.lock().unwrap().secret = Some(self.name.clone());
        }
        Box::pin(async move { secret })
    }
}

impl Workspace {
    pub(super) fn keychain_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault.as_ref().map(|v| v.data().clone()) else {
            return div().into_any_element();
        };
        let mut keys: Vec<&StoredKey> = data.keys.iter().collect();
        keys.sort_by_key(|k| k.name.to_lowercase());
        if self.kc.selected.is_none_or(|id| data.key(id).is_none()) {
            self.kc.selected = keys.first().map(|k| k.id);
        }
        let selected = self.kc.selected.and_then(|id| data.key(id));

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::SPACE_3)
            .child(div().flex_1().min_w(px(160.)).text_title().text_color(theme::ink()).child("Keychain"))
            .child(
                Button::secondary("kc-import-file", "Import key file")
                    .on_click(cx.listener(|this, _, window, cx| this.import_key_file(window, cx))),
            )
            .child(
                Button::secondary("kc-import-ssh", "Import from ~/.ssh")
                    .on_click(cx.listener(|this, _, _, cx| this.import_from(false, cx))),
            )
            .child(
                Button::secondary("kc-import-wsl", "Import from WSL")
                    .on_click(cx.listener(|this, _, _, cx| this.import_from(true, cx))),
            );

        let keys_section =
            div().flex().flex_col().gap(px(10.)).child(section_label("Keys")).child(if keys.is_empty() {
                self.kc_empty().into_any_element()
            } else {
                self.kc_table(&data, &keys, cx).into_any_element()
            });

        let identities = self.kc_identities(&data, cx);
        let public = selected.map(|k| self.kc_public_key(&data, k, cx));
        let dialog = self.kc_dialog_layer(&data, cx);

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("kc-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(22.))
                        .child(header)
                        .child(keys_section)
                        .child(identities)
                        .children(public),
                ),
            )
            .child(self.kc_generate_panel(cx))
            .children(dialog)
            .into_any_element()
    }

    fn kc_empty(&self) -> Div {
        div()
            .py(theme::SPACE_6)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme::SPACE_3)
            .bg(theme::card())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_LG)
            .child(icon("key", px(40.), theme::ink_faint()))
            .child(div().text_heading().text_color(theme::ink()).child("No keys yet"))
            .child(
                div()
                    .text_body()
                    .text_color(theme::ink_muted())
                    .child("Import a key you already use, or generate a new one with the panel on the right."),
            )
    }

    fn kc_table(&self, data: &VaultData, keys: &[&StoredKey], cx: &mut Context<Self>) -> Div {
        let hello = data.settings.hello_unlock;
        // Name | Type 110 | Protected by | Hosts 80 (Keychain.dc.html).
        let row = |d: Div| d.flex().items_center().gap(theme::SPACE_3).px(theme::SPACE_4);
        let header = row(div().py(px(10.)))
            .text_label()
            .text_color(theme::ink_muted())
            .border_b_1()
            .border_color(theme::line())
            .child(div().flex_grow().flex_basis(px(0.)).min_w_0().child("NAME"))
            .child(div().w(px(110.)).flex_none().child("TYPE"))
            .child(div().flex_1().min_w_0().child("PROTECTED BY"))
            .child(div().w(px(80.)).flex_none().child("HOSTS"));
        let count = keys.len();
        let lines = keys.iter().enumerate().map(|(i, key)| {
            let id = key.id;
            let on = self.kc.selected == Some(id);
            let (protection, warn) = protected_by(key, hello);
            let fingerprint = holonet_ssh::fingerprint(&key.public_key).unwrap_or_default();
            let users = data.key_users(id).0.len();
            row(div())
                .id(SharedString::from(format!("kc-key-{}", id.0)))
                .py(px(14.))
                .when(i + 1 < count, |d| d.border_b_1().border_color(theme::line()))
                .cursor_pointer()
                .when(on, |d| d.bg(theme::raised()))
                .when(!on, |d| d.hover(|s| s.bg(theme::tab_hover())))
                .child(
                    div()
                        .flex_grow()
                        .flex_basis(px(0.))
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_body()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::ink())
                                .truncate()
                                .child(SharedString::from(key.name.clone())),
                        )
                        .child(
                            div()
                                .text_mono_small()
                                .text_color(theme::ink_muted())
                                .truncate()
                                .child(SharedString::from(fingerprint)),
                        ),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_mono_small()
                        .text_color(theme::ink())
                        .child(SharedString::from(type_label(key))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_body()
                        .text_color(if warn { theme::warn() } else { theme::ink() })
                        .child(protection),
                )
                .child(
                    div()
                        .w(px(80.))
                        .flex_none()
                        .text_body()
                        .text_color(theme::ink())
                        .child(SharedString::from(users.to_string())),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kc.selected = Some(id);
                    this.kc.copied = None;
                    cx.notify();
                }))
        });
        div()
            .flex()
            .flex_col()
            .bg(theme::card())
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_LG)
            .overflow_hidden()
            .min_w(px(560.))
            .child(header)
            .children(lines)
    }

    fn kc_public_key(&self, data: &VaultData, key: &StoredKey, cx: &mut Context<Self>) -> Div {
        let id = key.id;
        let line = public_line(key);
        let copied = self.kc.copied == Some(self.kc.copy_generation);
        let made = match &key.source {
            Some(source) if source == "Generated in Holonet" => {
                format!("Generated in Holonet on {}", format_date(key.imported_at))
            }
            Some(source) => format!("Imported on {} from {}", format_date(key.imported_at), short_path(source, 70)),
            None => format!("Imported on {}", format_date(key.imported_at)),
        };
        let (host_ids, _) = data.key_users(id);
        let used_by: Vec<String> =
            host_ids.iter().filter_map(|h| data.host(*h)).map(|h| h.display_name().to_string()).collect();
        let used = if used_by.is_empty() {
            "No hosts sign in with this key yet.".to_string()
        } else {
            format!("Signs in to {}.", name_list(&used_by))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(section_label(&format!("Public key · {}", key.name)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .px(theme::SPACE_3)
                            .py(px(13.))
                            .bg(theme::sunken())
                            .border_1()
                            .border_color(theme::line())
                            .rounded(theme::RADIUS_MD)
                            .text_mono_small()
                            .text_color(theme::ink_soft())
                            .truncate()
                            .child(SharedString::from(line.clone())),
                    )
                    .child(Button::secondary("kc-copy", if copied { "Copied" } else { "Copy" }).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(line.clone()));
                            this.kc.copy_generation += 1;
                            let generation = this.kc.copy_generation;
                            this.kc.copied = Some(generation);
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(2)).await;
                                let _ = this.update(cx, |this, cx| {
                                    if this.kc.copied == Some(generation) {
                                        this.kc.copied = None;
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                            cx.notify();
                        },
                    )))
                    .child(
                        Button::secondary("kc-install", "Install on host")
                            .on_click(cx.listener(move |this, _, window, cx| this.open_install(id, window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .child(SharedString::from(made))
                    .child(SharedString::from(used)),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::SPACE_2)
                    .child(
                        Button::quiet("kc-rename", "Rename")
                            .compact()
                            .on_click(cx.listener(move |this, _, window, cx| this.open_rename(id, window, cx))),
                    )
                    .child(Button::new("kc-delete", "Delete key", ButtonKind::DestructiveOutline).compact().on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.kc.dialog = Some(KcDialog::Delete { key: id });
                            cx.notify();
                        }),
                    )),
            )
    }

    fn kc_generate_panel(&self, cx: &mut Context<Self>) -> Div {
        let kind = KeyKind::ALL[self.kc.gen_kind];
        let generating = self.kc.generating;
        div()
            .w(px(ASIDE))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(theme::SPACE_5)
            .py(px(28.))
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::line())
            .shadow(theme::shadow_panel())
            .child(div().text_heading().text_color(theme::ink()).child("Generate key"))
            // flex_none: in this full-height column, field()'s flex_1 would
            // share out the spare height between the fields.
            .child(field("Name", self.kc.gen_name.clone(), self.kc.gen_problem.clone().map(|p| (p, true))).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(crate::ui::field_label("Type"))
                    .child(
                        Segmented::new(
                            "kc-kind",
                            KeyKind::ALL.iter().map(|k| SharedString::from(k.label())).collect(),
                            self.kc.gen_kind,
                        )
                        .compact()
                        .columns(2)
                        .on_change({
                            let this = cx.weak_entity();
                            move |i, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.kc.gen_kind = i;
                                    cx.notify();
                                });
                            }
                        }),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(kind.note())),
            )
            .child(field("Passphrase", self.kc.gen_passphrase.clone(), None).flex_none())
            .child(Checkbox::new("kc-hello", "Unlock with Windows Hello", self.kc.gen_hello).on_toggle({
                let this = cx.weak_entity();
                move |v, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.kc.gen_hello = v;
                        cx.notify();
                    });
                }
            }))
            .child(
                div()
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .child("Saves the passphrase in the vault, so the key unlocks when the vault does."),
            )
            .child(
                Button::primary("kc-generate", if generating { "Generating…" } else { "Generate" })
                    .full_width()
                    .disabled(generating)
                    .on_click(cx.listener(|this, _, _, cx| this.generate_key(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .child("Private keys never leave this device unencrypted."),
            )
    }

    fn generate_key(&mut self, cx: &mut Context<Self>) {
        if self.kc.generating {
            return;
        }
        let name = self.kc.gen_name.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.kc.gen_problem = Some("Name the key, for example surface-ed25519.".into());
            cx.notify();
            return;
        }
        let kind = KeyKind::ALL[self.kc.gen_kind];
        let passphrase = self.kc.gen_passphrase.read(cx).text().to_string();
        let save_passphrase = self.kc.gen_hello && !passphrase.is_empty();
        self.kc.generating = true;
        self.kc.gen_problem = None;
        cx.notify();
        let comment = key_import::default_comment();
        let task = cx.background_executor().spawn(async move {
            let pass = (!passphrase.is_empty()).then_some(passphrase.as_str());
            holonet_ssh::generate_key(kind, &comment, pass)
                .map(|g| (g, save_passphrase.then(|| Secret::new(passphrase.clone()))))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.kc.generating = false;
                match result {
                    Ok((generated, saved)) => {
                        let key = StoredKey {
                            id: KeyId::new(),
                            name: name.clone(),
                            algorithm: generated.info.algorithm,
                            private_key: generated.private_key,
                            encrypted: generated.info.encrypted,
                            public_key: generated.info.public_key,
                            comment: generated.info.comment,
                            saved_passphrase: saved,
                            source: Some("Generated in Holonet".into()),
                            imported_at: Timestamp::now(),
                        };
                        let id = key.id;
                        this.save(move |d| d.keys.push(key), cx);
                        this.kc.selected = Some(id);
                        this.kc.gen_name.update(cx, |f, cx| f.set_text("", cx));
                        this.kc.gen_passphrase.update(cx, |f, cx| f.set_text("", cx));
                        this.notice =
                            Some(format!("Generated {name}. Install it on a host to sign in with it.").into());
                    }
                    Err(e) => this.kc.gen_problem = Some(format!("Holonet could not generate the key: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ imports --

    fn import_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import key".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let found = cx.background_executor().spawn(async move { key_import::read_file(&path, "") }).await;
            let _ = this.update_in(cx, |this, window, cx| this.add_found(found, window, cx));
        })
        .detach();
    }

    /// Import one file, asking for a PuTTY key's passphrase when needed.
    fn add_found(&mut self, found: Found, window: &mut Window, cx: &mut Context<Self>) {
        match &found.result {
            Err(e) => {
                self.notice = Some(format!("{} was not imported. {e}", found.name()).into());
            }
            Ok(info) if info.ppk && info.encrypted => {
                let field = cx.new(|cx| TextField::new(cx, "").password());
                self.kc_subscribe_dialog_field(&field, window, cx);
                field.read(cx).focus(window);
                self.kc.dialog = Some(KcDialog::PpkPassphrase { found: Box::new(found), field, problem: None });
            }
            Ok(_) => {
                self.import_found(vec![found], None, cx);
            }
        }
        cx.notify();
    }

    /// Copy keys into the vault, skipping ones already there.
    fn import_found(&mut self, found: Vec<Found>, ppk_passphrase: Option<&str>, cx: &mut Context<Self>) -> bool {
        let Some(data) = self.vault.as_ref().map(|v| v.data()) else { return false };
        let mut new_keys = Vec::new();
        let mut already = Vec::new();
        let mut failed = Vec::new();
        for f in &found {
            match key_import::stored_key(f, ppk_passphrase) {
                Ok(key) if data.keys.iter().chain(&new_keys).any(|k| k.public_key == key.public_key) => {
                    already.push(f.name())
                }
                Ok(key) => new_keys.push(key),
                Err(holonet_ssh::KeyFileError::WrongPassphrase) => return false,
                Err(e) => failed.push(format!("{} ({e})", f.name())),
            }
        }
        let imported: Vec<String> = new_keys.iter().map(|k| k.name.clone()).collect();
        if let Some(first) = new_keys.first() {
            self.kc.selected = Some(first.id);
        } else if let Some(existing) = found
            .first()
            .and_then(|f| f.result.as_ref().ok())
            .and_then(|info| data.keys.iter().find(|k| k.public_key == info.public_key))
        {
            self.kc.selected = Some(existing.id);
        }
        if !new_keys.is_empty() {
            self.save(move |d| d.keys.extend(new_keys), cx);
        }
        let mut parts = Vec::new();
        if !imported.is_empty() {
            parts.push(format!("Imported {}. The files are left as they are.", name_list(&imported)));
        }
        if !already.is_empty() {
            parts.push(format!(
                "{} {} already in the vault.",
                name_list(&already),
                if already.len() == 1 { "is" } else { "are" }
            ));
        }
        if !failed.is_empty() {
            parts.push(format!("Not imported: {}.", failed.join(", ")));
        }
        self.notice = Some(parts.join(" ").into());
        cx.notify();
        true
    }

    /// "Import from ~/.ssh" or "Import from WSL": list what is there to pick from.
    fn import_from(&mut self, wsl: bool, cx: &mut Context<Self>) {
        self.kc.dialog = Some(KcDialog::Scanning(if wsl { "Looking in WSL…" } else { "Looking in ~/.ssh…" }));
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            if wsl {
                match key_import::wsl_ssh_dirs() {
                    Err(e) => (Vec::new(), e),
                    Ok(dirs) => {
                        let names: Vec<String> = dirs.iter().map(|(d, _)| d.clone()).collect();
                        let found = dirs.iter().flat_map(|(distro, dir)| key_import::scan_dir(dir, distro)).collect();
                        (found, format!("No private keys in ~/.ssh in {}.", name_list(&names)))
                    }
                }
            } else {
                match key_import::windows_ssh_dir() {
                    Some(dir) => {
                        let empty = format!("No private keys in {}.", dir.display());
                        (key_import::scan_dir(&dir, "~/.ssh"), empty)
                    }
                    None => (Vec::new(), "Holonet could not find your Windows profile folder.".into()),
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let (found, empty) = task.await;
            let _ = this.update(cx, |this, cx| {
                if !matches!(this.kc.dialog, Some(KcDialog::Scanning(_))) {
                    return; // Closed while looking.
                }
                let known: Vec<String> = this
                    .vault
                    .as_ref()
                    .map(|v| v.data().keys.iter().map(|k| k.public_key.clone()).collect())
                    .unwrap_or_default();
                let checked = found
                    .iter()
                    .map(|f| f.importable() && f.result.as_ref().is_ok_and(|i| !known.contains(&i.public_key)))
                    .collect();
                let title = if wsl { "Import from WSL" } else { "Import from ~/.ssh" };
                this.kc.dialog = Some(KcDialog::Pick { title: title.into(), found, checked, empty: empty.into() });
                cx.notify();
            });
        })
        .detach();
    }

    fn kc_subscribe_dialog_field(&mut self, field: &Entity<TextField>, window: &mut Window, cx: &mut Context<Self>) {
        self.kc._subscriptions.push(cx.subscribe_in(
            field,
            window,
            |this, _, e: &TextFieldEvent, window, cx| match e {
                TextFieldEvent::Submitted => this.kc_submit_dialog(window, cx),
                TextFieldEvent::Cancelled => {
                    this.kc.dialog = None;
                    cx.notify();
                }
                TextFieldEvent::Changed => {}
            },
        ));
    }

    /// Enter in a dialog's field.
    fn kc_submit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.kc.dialog.take() {
            Some(KcDialog::PpkPassphrase { found, field, .. }) => {
                let passphrase = field.read(cx).text().to_string();
                if self.import_found(vec![(*found).clone()], Some(&passphrase), cx) {
                    self.kc.dialog = None;
                } else {
                    self.kc.dialog = Some(KcDialog::PpkPassphrase {
                        found,
                        field,
                        problem: Some("The passphrase did not unlock this key.".into()),
                    });
                }
            }
            Some(KcDialog::Rename { key, field }) => {
                let name = field.read(cx).text().trim().to_string();
                if name.is_empty() {
                    self.kc.dialog = Some(KcDialog::Rename { key, field });
                } else {
                    self.save(
                        move |d| {
                            if let Some(k) = d.key_mut(key) {
                                k.name = name;
                            }
                        },
                        cx,
                    );
                }
            }
            Some(KcDialog::Identity(form)) => {
                self.kc.dialog = Some(KcDialog::Identity(form));
                self.save_identity(cx);
            }
            install @ Some(KcDialog::Install { .. }) => {
                self.kc.dialog = install;
                self.run_install(window, cx);
            }
            other => self.kc.dialog = other,
        }
        cx.notify();
    }

    fn open_rename(&mut self, key: KeyId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.vault.as_ref().and_then(|v| v.data().key(key)).map(|k| k.name.clone()).unwrap_or_default();
        let field = cx.new(|cx| TextField::new(cx, "Key name").with_text(name));
        self.kc_subscribe_dialog_field(&field, window, cx);
        field.read(cx).focus(window);
        self.kc.dialog = Some(KcDialog::Rename { key, field });
        cx.notify();
    }

    // ---------------------------------------------------- install on host --

    fn open_install(&mut self, key: KeyId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault.as_ref().map(|v| v.data()) else { return };
        let mut hosts: Vec<&holonet_model::Host> = data.hosts.iter().collect();
        hosts.sort_by_key(|h| h.display_name().to_lowercase());
        let options =
            hosts.iter().map(|h| SharedString::from(format!("{} ({})", h.display_name(), h.target()))).collect();
        let ids: Vec<HostId> = hosts.iter().map(|h| h.id).collect();
        let select = cx.new(|cx| Select::new(cx, options, 0));
        let secret = cx.new(|cx| TextField::new(cx, "").password());
        self.kc_subscribe_dialog_field(&secret, window, cx);
        self.kc._subscriptions.push(cx.observe(&select, |this, _, cx| {
            if let Some(KcDialog::Install { state, .. }) = this.kc.dialog.as_mut() {
                *state = InstallState::Ready;
            }
            cx.notify();
        }));
        self.kc.dialog = Some(KcDialog::Install { key, hosts: ids, select, secret, state: InstallState::Ready });
        cx.notify();
    }

    /// What the chosen host still needs to sign in, if anything: a label
    /// for the dialog's field.
    fn install_needs(data: &VaultData, host: &holonet_model::Host) -> Option<String> {
        let (user, auth) = data.sign_in(host);
        let user = if user.is_empty() { "this user" } else { user };
        match auth {
            Auth::Password { saved: None } => Some(format!("Password for {user} on {}", host.display_name())),
            Auth::Key { key_id, .. } => match data.key(*key_id) {
                Some(k) if k.encrypted && k.saved_passphrase.is_none() => Some(format!("Passphrase for {}", k.name)),
                _ => None,
            },
            Auth::Password { .. } => None,
        }
    }

    fn run_install(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault.as_ref().map(|v| v.data().clone()) else { return };
        let Some(KcDialog::Install { key, hosts, select, secret, state }) = self.kc.dialog.as_mut() else { return };
        if matches!(state, InstallState::Running) {
            return;
        }
        let Some(host) = hosts.get(select.read(cx).selected()).and_then(|id| data.host(*id)).cloned() else { return };
        let Some(stored) = data.key(*key).cloned() else { return };
        let typed = secret.read(cx).text().to_string();
        let typed = (!typed.is_empty()).then(|| Secret::new(typed));
        let name = host.display_name().to_string();
        // Through the host's jump hosts and route, as a pane connects.
        let jumps = match crate::hops::jumps(&data, &host) {
            Ok(jumps) => jumps,
            Err(problem) => {
                *state = InstallState::Failed(format!("{name} cannot be reached: {}", problem.message()).into());
                cx.notify();
                return;
            }
        };
        let target = crate::hops::Hop::of(&data, &host);
        let stops = Arc::new(Mutex::new(InstallStops::default()));
        let timeout = Duration::from_secs(data.settings.connect_timeout_secs.max(1) as u64);
        let mut steps = Vec::new();
        for (i, hop) in jumps.iter().chain(std::iter::once(&target)).enumerate() {
            let own = i == jumps.len();
            // The dialog's field is for the host itself.
            let field = if own { typed.clone() } else { None };
            let Some(credential) = hop.credential(false, false, |_| field.clone()) else {
                *state = InstallState::Failed(
                    format!("{} signs in with a key that is no longer in the vault. Edit it first.", hop.name()).into(),
                );
                cx.notify();
                return;
            };
            steps.push(ConnectOptions {
                host: crate::hops::reach_address(&hop.host),
                port: hop.host.port,
                user: hop.host.username.clone(),
                credential,
                connect_timeout: timeout,
                keep_alive: None,
                prompts: Arc::new(InstallPrompts {
                    name: hop.name().to_string(),
                    known: data.known_hosts.clone(),
                    host: hop.host.clone(),
                    secret: field,
                    stops: stops.clone(),
                }),
                jump: None,
            });
        }
        *state = InstallState::Running;
        let opts = crate::hops::link(steps);
        let route_host = jumps.first().map_or(&target.host, |j| &j.host).clone();
        let fallback = data.settings.connection.fallback_wait.duration();
        let hop_count = jumps.len();
        let line = public_line(&stored);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let routed = crate::hops::connect_by_route(opts, &route_host, None, fallback, hop_count, |_| {}).await;
            let result = match routed {
                crate::hops::Routed::Plan(problem) => Err(Err(problem)),
                crate::hops::Routed::Connect(result) => match *result {
                    Ok(session) => {
                        let outcome = session.install_public_key(&line).await.map_err(|e| e.to_string());
                        let _ = session.close().await;
                        Ok(outcome)
                    }
                    Err(e) => Err(Ok(e)),
                },
            };
            let _ = tx.send(result);
        });
        let key_name = stored.name.clone();
        let uses_key = data.sign_in(&host).1.key_id() == Some(stored.id);
        let hop_names: Vec<String> = jumps.iter().map(|j| j.name().to_string()).chain([name.clone()]).collect();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let stops = std::mem::take(&mut *stops.lock().unwrap());
            let _ = this.update(cx, |this, cx| {
                let message = match result {
                    Ok(Ok(InstallOutcome::Added)) => Ok(if uses_key {
                        format!("Installed {key_name} on {name}.")
                    } else {
                        format!("Installed {key_name} on {name}. To sign in with it, edit {name} and choose this key.")
                    }),
                    Ok(Ok(InstallOutcome::AlreadyPresent)) => Ok(format!("{name} already accepts {key_name}. Nothing was changed.")),
                    Ok(Err(e)) => Err(format!("{name} did not accept the change: {e}")),
                    Err(Err(problem)) => Err(match problem {
                        holonet_net::PlanProblem::TailscaleUnavailable(why) => {
                            format!("{} connects over Tailscale. {why}", hop_names[0])
                        }
                        holonet_net::PlanProblem::NoTailscaleAddress => {
                            format!("{} has no Tailscale address. Edit it first.", hop_names[0])
                        }
                    }),
                    Err(Ok(e)) => {
                        // Name the hop the error belongs to.
                        let (e, wrappers) = e.innermost();
                        let hop = hop_names[hop_count.saturating_sub(wrappers.len())].clone();
                        Err(match e {
                            ConnectError::HostKeyRejected => match stops.key {
                                Some((at, KeyStatus::Changed { .. })) => format!("The host key for {at} has changed. Holonet stopped before signing in. Check it in Known hosts."),
                                Some((at, _)) => format!("Holonet has not verified {at}'s host key yet. Connect to {name} once from Hosts, then install the key."),
                                None => format!("{hop} did not accept the connection."),
                            },
                            ConnectError::Cancelled => match stops.secret {
                                Some(at) if at != name => format!("The jump host {at} needs a password or passphrase that is not saved. Connect to {name} once from Hosts and save it, then install again."),
                                _ => "Enter the password or passphrase above, then install again.".into(),
                            },
                            ConnectError::SignInRefused { .. } => format!("{hop} refused the sign-in. Check the password or key, then try again."),
                            ConnectError::WrongPassphrase => format!("That passphrase did not unlock {hop}'s key."),
                            ConnectError::JumpCouldNotReach { jump: _, target: _, reason } => {
                                format!("A jump host on the way could not reach {hop}: {reason}.")
                            }
                            e => format!("Holonet could not connect to {hop}: {}", crate::host_form::describe_error(e)),
                        })
                    }
                };
                if let Some(KcDialog::Install { state, .. }) = this.kc.dialog.as_mut() {
                    *state = match message {
                        Ok(m) => InstallState::Done(m.into()),
                        Err(m) => InstallState::Failed(m.into()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // --------------------------------------------------------- identities --

    fn kc_identities(&self, data: &VaultData, cx: &mut Context<Self>) -> Div {
        let mut identities: Vec<&Identity> = data.identities.iter().collect();
        identities.sort_by_key(|i| i.name.to_lowercase());
        let card = |id: SharedString| {
            div()
                .id(id)
                .flex_grow()
                .flex_basis(px(220.))
                .min_w(px(220.))
                .max_w(px(360.))
                .px(theme::SPACE_4)
                .py(px(14.))
                .rounded(theme::RADIUS_LG)
                .border_1()
                .cursor_pointer()
        };
        let cards = identities.iter().map(|identity| {
            let id = identity.id;
            let how = match &identity.auth {
                Auth::Key { key_id, .. } => match data.key(*key_id) {
                    Some(k) => format!("Key {}", k.name),
                    None => "Key no longer in the vault".to_string(),
                },
                Auth::Password { saved: Some(_) } => "Password, stored in vault".to_string(),
                Auth::Password { saved: None } => "Password, asked on connect".to_string(),
            };
            let users = data.identity_users(id).len();
            let meta = format!(
                "{} · {}",
                if identity.username.is_empty() {
                    "No username".to_string()
                } else {
                    format!("User {}", identity.username)
                },
                match users {
                    0 => "no hosts".to_string(),
                    1 => "1 host".to_string(),
                    n => format!("{n} hosts"),
                }
            );
            card(SharedString::from(format!("kc-identity-{}", id.0)))
                .bg(theme::card())
                .border_color(theme::line())
                .hover(|s| s.bg(theme::tab_hover()))
                .flex()
                .flex_col()
                .gap(theme::SPACE_1)
                .child(
                    div()
                        .text_body()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .truncate()
                        .child(SharedString::from(identity.name.clone())),
                )
                .child(div().text_meta().text_color(theme::ink_muted()).truncate().child(SharedString::from(how)))
                .child(div().text_meta().text_color(theme::ink_faint()).truncate().child(SharedString::from(meta)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_identity(Some(id), window, cx)))
        });
        let new_card = card("kc-identity-new".into())
            .border_color(theme::line_strong())
            .border_dashed()
            .hover(|s| s.bg(theme::card()))
            .flex()
            .items_center()
            .gap(theme::SPACE_2)
            .text_body()
            .text_color(theme::ink_muted())
            .child(icon("plus", px(16.), theme::ink_muted()))
            .child("New identity")
            .on_click(cx.listener(|this, _, window, cx| this.open_identity(None, window, cx)));
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(section_label("Identities"))
            .when(identities.is_empty(), |d| {
                d.child(div().text_body().text_color(theme::ink_muted()).child(
                    "An identity is a username and a key or password that several hosts can share. Choose it on a host as its authentication.",
                ))
            })
            .child(div().flex().flex_wrap().gap(theme::SPACE_3).children(cards).child(new_card))
    }

    fn open_identity(&mut self, id: Option<IdentityId>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.vault.as_ref().map(|v| v.data()) else { return };
        let existing = id.and_then(|id| data.identity(id)).cloned();
        let mut keys: Vec<&StoredKey> = data.keys.iter().collect();
        keys.sort_by_key(|k| k.name.to_lowercase());
        let key_ids: Vec<KeyId> = keys.iter().map(|k| k.id).collect();
        let (auth, key_index, saved) = match existing.as_ref().map(|i| &i.auth) {
            Some(Auth::Key { key_id, .. }) => (0, key_ids.iter().position(|k| k == key_id).unwrap_or(0), None),
            Some(Auth::Password { saved }) => (1, 0, saved.clone()),
            None => (if keys.is_empty() { 1 } else { 0 }, 0, None),
        };
        let options = keys.iter().map(|k| SharedString::from(format!("{} ({})", k.name, type_label(k)))).collect();
        let key_select = cx.new(|cx| Select::new(cx, options, key_index).mono());
        let name = cx.new(|cx| {
            TextField::new(cx, "e.g. deploy").with_text(existing.as_ref().map(|i| i.name.clone()).unwrap_or_default())
        });
        let username = cx.new(|cx| {
            TextField::new(cx, "e.g. deploy")
                .mono()
                .with_text(existing.as_ref().map(|i| i.username.clone()).unwrap_or_default())
        });
        let password = cx.new(|cx| {
            TextField::new(cx, "Leave empty to be asked on connect")
                .password()
                .with_text(saved.as_ref().map(|s| s.expose().to_string()).unwrap_or_default())
        });
        for f in [&name, &username, &password] {
            self.kc_subscribe_dialog_field(f, window, cx);
        }
        name.read(cx).focus(window);
        self.kc.dialog = Some(KcDialog::Identity(Box::new(IdentityForm {
            id,
            name,
            username,
            auth,
            key_ids,
            key_select,
            password,
            save_password: existing.is_none() || saved.is_some(),
            problem: None,
        })));
        cx.notify();
    }

    fn save_identity(&mut self, cx: &mut Context<Self>) {
        let Some(KcDialog::Identity(form)) = self.kc.dialog.as_mut() else { return };
        let name = form.name.read(cx).text().trim().to_string();
        let username = form.username.read(cx).text().trim().to_string();
        let problem = if name.is_empty() {
            Some("Name the identity, for example deploy.")
        } else if username.is_empty() {
            Some("Enter the username these hosts sign in as.")
        } else if form.auth == 0 && form.key_ids.is_empty() {
            Some("Choose a key, or use a password.")
        } else {
            None
        };
        if let Some(problem) = problem {
            form.problem = Some(problem.into());
            cx.notify();
            return;
        }
        let auth = if form.auth == 0 {
            Auth::key(form.key_ids[form.key_select.read(cx).selected().min(form.key_ids.len() - 1)])
        } else {
            let password = form.password.read(cx).text().to_string();
            Auth::Password { saved: (form.save_password && !password.is_empty()).then(|| Secret::new(password)) }
        };
        let id = form.id.unwrap_or_default();
        let identity = Identity { id, name: name.clone(), username, auth };
        let editing = form.id.is_some();
        self.save(
            move |d| match d.identities.iter_mut().find(|i| i.id == id) {
                Some(existing) => *existing = identity,
                None => d.identities.push(identity),
            },
            cx,
        );
        self.kc.dialog = None;
        if !editing {
            self.notice = Some(format!("Added the identity {name}. Choose it on a host as its authentication.").into());
        }
        cx.notify();
    }

    // ------------------------------------------------------------ dialogs --

    fn kc_dialog_layer(&self, data: &VaultData, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.kc.dialog.as_ref()?;
        let entity = cx.entity();
        let close = move |_: &mut Window, cx: &mut App| {
            entity.update(cx, |w, cx| {
                if !matches!(w.kc.dialog, Some(KcDialog::Install { state: InstallState::Running, .. })) {
                    w.kc.dialog = None;
                    cx.notify();
                }
            })
        };
        let cancel = |id: &'static str, label: &'static str| {
            let close = close.clone();
            Button::secondary(id, label).on_click(move |_, w, cx| close(w, cx))
        };
        let (width, card): (f32, AnyElement) = match open {
            KcDialog::Scanning(what) => (
                460.,
                dialog_card(*what, "WSL may take a few seconds to start.")
                    .child(dialog_buttons([cancel("kc-scan-cancel", "Cancel")]))
                    .into_any_element(),
            ),
            KcDialog::Pick { title, found, checked, empty } => {
                let known: Vec<&str> = data.keys.iter().map(|k| k.public_key.as_str()).collect();
                let any = checked.iter().any(|c| *c);
                let rows = found.iter().enumerate().map(|(i, f)| {
                    let (detail, enabled): (String, bool) = match &f.result {
                        Ok(info) if known.contains(&info.public_key.as_str()) => ("Already in the vault".into(), false),
                        Ok(info) if info.ppk && info.encrypted => {
                            ("PuTTY key with a passphrase: use Import key file to enter it".into(), false)
                        }
                        Ok(info) => (
                            format!(
                                "{} · {}{}",
                                algorithm_label(&info.algorithm),
                                if info.encrypted { "passphrase" } else { "no passphrase" },
                                if info.ppk { " · PuTTY key" } else { "" }
                            ),
                            true,
                        ),
                        Err(e) => (e.clone(), false),
                    };
                    let on = checked.get(i).copied().unwrap_or(false);
                    let this = cx.weak_entity();
                    div()
                        .flex()
                        .items_start()
                        .gap(theme::SPACE_3)
                        .py(theme::SPACE_2)
                        .child(Checkbox::new(SharedString::from(format!("kc-pick-{i}")), "", on).on_toggle(move |v, _, cx| {
                            if !enabled {
                                return;
                            }
                            let _ = this.update(cx, |this, cx| {
                                if let Some(KcDialog::Pick { checked, .. }) = this.kc.dialog.as_mut() {
                                    checked[i] = v;
                                }
                                cx.notify();
                            });
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .when(!enabled, |d| d.opacity(0.6))
                                .child(
                                    div()
                                        .text_body()
                                        .text_color(theme::ink())
                                        .child(SharedString::from(format!("{} · {}", f.place, f.name()))),
                                )
                                .child(div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(detail))),
                        )
                });
                let body = if found.is_empty() {
                    div().text_body().text_color(theme::ink_muted()).child(empty.clone()).into_any_element()
                } else {
                    div().id("kc-pick-list").max_h(px(320.)).overflow_y_scroll().children(rows).into_any_element()
                };
                (
                    520.,
                    dialog_card(title.clone(), "Keys are copied into the vault. The files are left as they are.")
                        .child(body)
                        .child(dialog_buttons([
                            cancel("kc-pick-cancel", "Cancel"),
                            Button::primary("kc-pick-import", "Import selected").disabled(!any).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if let Some(KcDialog::Pick { found, checked, .. }) = this.kc.dialog.take() {
                                        let chosen = found.into_iter().zip(checked).filter(|(_, c)| *c).map(|(f, _)| f).collect();
                                        this.import_found(chosen, None, cx);
                                    }
                                    cx.notify();
                                },
                            )),
                        ]))
                        .into_any_element(),
                )
            }
            KcDialog::PpkPassphrase { found, field: f, problem } => (
                460.,
                dialog_card(
                    format!("Enter the passphrase for {}", found.name()),
                    "PuTTY keys are converted to OpenSSH format for the vault. The converted key keeps this passphrase.",
                )
                .child(field("Passphrase", f.clone(), problem.clone().map(|p| (p, true))))
                .child(dialog_buttons([
                    cancel("kc-ppk-cancel", "Cancel"),
                    Button::primary("kc-ppk-import", "Import key")
                        .on_click(cx.listener(|this, _, window, cx| this.kc_submit_dialog(window, cx))),
                ]))
                .into_any_element(),
            ),
            KcDialog::Rename { field: f, .. } => (
                420.,
                dialog_card("Rename key", "The name is only shown in Holonet. The key itself does not change.")
                    .child(field("Name", f.clone(), None))
                    .child(dialog_buttons([
                        cancel("kc-rename-cancel", "Cancel"),
                        Button::primary("kc-rename-save", "Rename")
                            .on_click(cx.listener(|this, _, window, cx| this.kc_submit_dialog(window, cx))),
                    ]))
                    .into_any_element(),
            ),
            KcDialog::Delete { key } => {
                let key = *key;
                let name = data.key(key).map(|k| k.name.clone()).unwrap_or_default();
                let (hosts, identities) = data.key_users(key);
                let names: Vec<String> = hosts.iter().filter_map(|h| data.host(*h)).map(|h| h.display_name().to_string()).collect();
                let identity_names: Vec<String> =
                    identities.iter().filter_map(|i| data.identity(*i)).map(|i| i.name.clone()).collect();
                let also = if identity_names.is_empty() {
                    String::new()
                } else {
                    format!(
                        " The {} {} {} it too.",
                        if identity_names.len() == 1 { "identity" } else { "identities" },
                        name_list(&identity_names),
                        if identity_names.len() == 1 { "uses" } else { "use" }
                    )
                };
                let body = if names.is_empty() {
                    "No hosts sign in with it. The key is removed from the vault; any copies elsewhere are not touched.".to_string()
                } else {
                    format!(
                        "{} {} with it, and will need another key or a password. Servers that accept it are not changed.",
                        name_list(&names),
                        if names.len() == 1 { "signs in" } else { "sign in" }
                    )
                } + &also;
                (
                    460.,
                    dialog(true, format!("Delete {name}?"), body)
                        .child(dialog_buttons([
                            cancel("kc-delete-cancel", "Cancel"),
                            Button::new("kc-delete-confirm", "Delete key", ButtonKind::DestructiveFill).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.save(move |d| d.keys.retain(|k| k.id != key), cx);
                                    this.kc.dialog = None;
                                    this.kc.selected = None;
                                },
                            )),
                        ]))
                        .into_any_element(),
                )
            }
            KcDialog::Identity(form) => {
                let editing = form.id.is_some();
                let mut card = dialog_card(
                    if editing { "Edit identity" } else { "New identity" },
                    "A username and a key or password that several hosts can share. Changing it changes every host that uses it.",
                )
                .child(field("Name", form.name.clone(), None).flex_none())
                .child(field("Username", form.username.clone(), None).flex_none())
                .child(
                    div().flex().flex_col().gap(theme::SPACE_2).child(crate::ui::field_label("Authentication")).child(
                        Segmented::new("kc-id-auth", vec!["SSH key".into(), "Password".into()], form.auth).on_change({
                            let this = cx.weak_entity();
                            move |i, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    if let Some(KcDialog::Identity(f)) = this.kc.dialog.as_mut() {
                                        f.auth = i;
                                        f.problem = None;
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                    ),
                );
                card = if form.auth == 0 {
                    if form.key_ids.is_empty() {
                        card.child(
                            div()
                                .text_body()
                                .text_color(theme::ink_muted())
                                .child("The vault has no keys yet. Import or generate one first, or use a password."),
                        )
                    } else {
                        card.child(field("Key", form.key_select.clone(), None).flex_none())
                    }
                } else {
                    let this = cx.weak_entity();
                    card.child(field("Password", form.password.clone(), None).flex_none()).child(
                        Checkbox::new("kc-id-save", "Save in the encrypted vault", form.save_password).on_toggle(
                            move |v, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    if let Some(KcDialog::Identity(f)) = this.kc.dialog.as_mut() {
                                        f.save_password = v;
                                    }
                                    cx.notify();
                                });
                            },
                        ),
                    )
                };
                if let Some(problem) = &form.problem {
                    card = card.child(div().text_meta().text_color(theme::warn()).child(problem.clone()));
                }
                let id = form.id;
                let mut buttons = Vec::new();
                if let Some(id) = id {
                    buttons.push(
                        Button::new("kc-id-delete", "Delete identity", ButtonKind::DestructiveOutline).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.kc.dialog = Some(KcDialog::DeleteIdentity { id });
                                cx.notify();
                            },
                        )),
                    );
                }
                buttons.push(cancel("kc-id-cancel", "Cancel"));
                buttons.push(
                    Button::primary("kc-id-save-button", if editing { "Save identity" } else { "Add identity" })
                        .on_click(cx.listener(|this, _, _, cx| this.save_identity(cx))),
                );
                (480., card.child(dialog_buttons(buttons)).into_any_element())
            }
            KcDialog::DeleteIdentity { id } => {
                let id = *id;
                let name = data.identity(id).map(|i| i.name.clone()).unwrap_or_default();
                let users: Vec<String> =
                    data.identity_users(id).iter().filter_map(|h| data.host(*h)).map(|h| h.display_name().to_string()).collect();
                let body = if users.is_empty() {
                    "No hosts use it.".to_string()
                } else {
                    format!(
                        "{} {} it. {} its username and sign-in as {} own, so {} still connect.",
                        name_list(&users),
                        if users.len() == 1 { "uses" } else { "use" },
                        if users.len() == 1 { "It keeps" } else { "They keep" },
                        if users.len() == 1 { "its" } else { "their" },
                        if users.len() == 1 { "it can" } else { "they" },
                    )
                };
                (
                    460.,
                    dialog(true, format!("Delete the identity {name}?"), body)
                        .child(dialog_buttons([
                            cancel("kc-id-delete-cancel", "Cancel"),
                            Button::new("kc-id-delete-confirm", "Delete identity", ButtonKind::DestructiveFill).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.save(move |d| drop(d.delete_identity(id)), cx);
                                    this.kc.dialog = None;
                                }),
                            ),
                        ]))
                        .into_any_element(),
                )
            }
            KcDialog::Install { key, hosts, select, secret, state } => {
                let key_name = data.key(*key).map(|k| k.name.clone()).unwrap_or_default();
                let host = hosts.get(select.read(cx).selected()).and_then(|id| data.host(*id));
                let needs = host.and_then(|h| Self::install_needs(data, h));
                let running = matches!(state, InstallState::Running);
                let done = matches!(state, InstallState::Done(_));
                let status = match state {
                    InstallState::Ready | InstallState::Running => None,
                    InstallState::Done(m) => Some(callout(crate::ui::CalloutKind::Success).child(m.clone())),
                    InstallState::Failed(m) => Some(callout(crate::ui::CalloutKind::Warning).child(m.clone())),
                };
                let body: AnyElement = if hosts.is_empty() {
                    div().text_body().text_color(theme::ink_muted()).child("Add a host first.").into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_3)
                        .child(field("Host", select.clone(), None))
                        .children(needs.map(|label| field(label, secret.clone(), None)))
                        .into_any_element()
                };
                (
                    500.,
                    dialog_card(
                        format!("Install {key_name} on a host"),
                        "Holonet signs in the way the host is set up and adds this public key to ~/.ssh/authorized_keys. Existing keys there are left as they are.",
                    )
                    .child(body)
                    .children(status)
                    .child(dialog_buttons([
                        cancel("kc-install-cancel", if done { "Close" } else { "Cancel" }),
                        Button::primary("kc-install-run", if running { "Installing…" } else { "Install key" })
                            .disabled(running || hosts.is_empty() || done)
                            .on_click(cx.listener(|this, _, window, cx| this.run_install(window, cx))),
                    ]))
                    .into_any_element(),
                )
            }
        };
        Some(modal("kc-dialog", px(width), card, close).into_any_element())
    }
}

fn dialog_card(title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    dialog(false, title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsa_size_from_public_key() {
        let rsa = holonet_ssh::generate_key(KeyKind::Rsa4096, "c", None).unwrap();
        assert_eq!(rsa_bits(&rsa.info.public_key), Some(4096));
        let ed = holonet_ssh::generate_key(KeyKind::Ed25519, "c", None).unwrap();
        assert_eq!(rsa_bits(&ed.info.public_key), None);
    }

    #[test]
    fn name_lists_read_naturally() {
        let n = |v: &[&str]| name_list(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["a"]), "a");
        assert_eq!(n(&["a", "b"]), "a and b");
        assert_eq!(n(&["a", "b", "c", "d"]), "a and 3 more");
    }
}
