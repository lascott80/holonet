//! The vault gate: create a vault on first start, unlock it, and the
//! wrong-password delay. Locked follows docs/design/screens/Locked.dc.html:
//! "Unlock with Windows Hello" when the vault has a Hello key slot, with
//! "Use vault password instead", otherwise the vault password.
//! Create vault is not drawn; it uses the same card.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    BoxShadow, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, WeakEntity, Window, div,
    hsla, point, prelude::*, px,
};
use holonet_vault::{HelloRequest, LockedVault, MIN_PASSWORD_LEN, UnlockedVault, VaultError};

use crate::app::HolonetApp;
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, CalloutKind, TextField, TextFieldEvent, callout, field, icon};

/// Delay before another unlock attempt after `failures` wrong passwords:
/// none for the first three, then 1, 2, 4 ... seconds, at most 30.
pub fn lockout_delay(failures: u32) -> Duration {
    if failures < 3 { Duration::ZERO } else { Duration::from_secs((1u64 << (failures - 3).min(5)).min(30)) }
}

/// The centred card from Locked.dc.html: `panel`, radius 16, padding 36/32,
/// gap 18, an `accent` icon tile, title and body.
fn gate_card(icon_name: &'static str, title: &'static str, body: SharedString) -> gpui::Div {
    div()
        .w_full()
        .max_w(px(420.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(18.))
        .px(theme::SPACE_6)
        .py(px(36.))
        .bg(theme::panel())
        .rounded(px(16.))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.5),
            offset: point(px(0.), px(24.)),
            blur_radius: px(60.),
            spread_radius: px(0.),
        }])
        .child(
            div()
                .size(px(64.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(16.))
                .bg(theme::accent_alpha(0.14))
                .child(icon(icon_name, px(30.), theme::accent())),
        )
        .child(
            div()
                .font_family(theme::DISPLAY)
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::ink())
                .child(title),
        )
        .child(div().text_body().text_color(theme::ink_muted()).text_center().child(body))
}

/// Full-window gate body: `sunken`, padding 32, card centred.
fn gate_body(card: impl IntoElement) -> gpui::Div {
    div().size_full().flex().items_center().justify_center().p(theme::SPACE_6).bg(theme::sunken()).child(card)
}

// ----------------------------------------------------------- Create vault --

pub struct CreateVault {
    app: WeakEntity<HolonetApp>,
    path: PathBuf,
    password: Entity<TextField>,
    confirm: Entity<TextField>,
    problem: Option<SharedString>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl CreateVault {
    pub fn new(app: WeakEntity<HolonetApp>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| TextField::new(cx, "").password());
        let confirm = cx.new(|cx| TextField::new(cx, "").password());
        let subscriptions =
            vec![cx.subscribe_in(&password, window, Self::on_field), cx.subscribe_in(&confirm, window, Self::on_field)];
        password.read(cx).focus(window);
        Self { app, path, password, confirm, problem: None, busy: false, _subscriptions: subscriptions }
    }

    fn on_field(&mut self, _: &Entity<TextField>, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Submitted => self.create(window, cx),
            TextFieldEvent::Changed if self.problem.is_some() => {
                self.problem = None;
                cx.notify();
            }
            _ => {}
        }
    }

    fn create(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let password = self.password.read(cx).text().to_string();
        let confirm = self.confirm.read(cx).text().to_string();
        if password.chars().count() < MIN_PASSWORD_LEN {
            self.problem = Some(format!("Use at least {MIN_PASSWORD_LEN} characters.").into());
        } else if password != confirm {
            self.problem = Some("The two passwords do not match.".into());
        } else {
            self.problem = None;
            self.busy = true;
            let path = self.path.clone();
            // Argon2 takes about a third of a second: keep it off the UI thread.
            let task = cx.background_executor().spawn(async move { UnlockedVault::create(&path, &password) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(vault) => {
                            this.password.update(cx, |f, cx| f.set_text("", cx));
                            this.confirm.update(cx, |f, cx| f.set_text("", cx));
                            let _ = this.app.update(cx, |app, cx| app.opened(vault, false, cx));
                        }
                        Err(e) => this.problem = Some(format!("The vault could not be created: {e}").into()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }
}

impl Render for CreateVault {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let problem = self.problem.clone();
        gate_body(
            gate_card(
                "lock",
                "Create your vault",
                "Holonet keeps your hosts, keys and passwords in an encrypted vault on this PC. Choose the password that unlocks it."
                    .into(),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_4)
                    .child(field("Vault password", self.password.clone(), Some(("At least 8 characters.".into(), false))))
                    .child(field("Confirm password", self.confirm.clone(), problem.map(|p| (p, true)))),
            )
            .child(callout(CalloutKind::Warning).w_full().child(
                "If you forget this password, the vault cannot be recovered. Holonet has no way to reset it.",
            ))
            .child(
                Button::primary("create-vault", if self.busy { "Creating vault…" } else { "Create vault" })
                    .full_width()
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
            ),
        )
    }
}

// ----------------------------------------------------------------- Locked --

/// Why the vault is locked, for the body text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockReason {
    /// Holonet just started.
    Startup,
    /// "Lock now".
    Manual,
    /// No input in Holonet for this many minutes.
    Idle(u32),
    /// Windows locked.
    WindowsLocked,
}

impl LockReason {
    /// The Locked screen's body (Locked.dc.html).
    fn body(self, hello: bool) -> String {
        const AFTER: &str = "Your hosts, keys and passwords are encrypted until you unlock.";
        match self {
            LockReason::Startup if hello => "Unlock to open your hosts, keys and passwords.".into(),
            LockReason::Startup => "Enter your vault password to open your hosts, keys and passwords.".into(),
            LockReason::Manual => format!("You locked the vault. {AFTER}"),
            LockReason::Idle(60) => format!("Holonet locked after 1 hour idle. {AFTER}"),
            LockReason::Idle(1) => format!("Holonet locked after 1 minute idle. {AFTER}"),
            LockReason::Idle(m) => format!("Holonet locked after {m} minutes idle. {AFTER}"),
            LockReason::WindowsLocked => format!("Holonet locked when Windows locked. {AFTER}"),
        }
    }
}

pub struct Locked {
    app: WeakEntity<HolonetApp>,
    path: PathBuf,
    reason: LockReason,
    password: Entity<TextField>,
    problem: Option<SharedString>,
    failures: u32,
    retry_at: Option<Instant>,
    busy: bool,
    /// Sessions that stay connected while locked.
    open_sessions: usize,
    /// The vault's Hello key slot, read from the file when locking.
    hello: Option<HelloRequest>,
    /// "Use vault password instead" was chosen.
    use_password: bool,
    _subscriptions: Vec<Subscription>,
}

impl Locked {
    pub fn new(
        app: WeakEntity<HolonetApp>,
        path: PathBuf,
        reason: LockReason,
        open_sessions: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| TextField::new(cx, "").password());
        let subscriptions = vec![cx.subscribe_in(&password, window, Self::on_field)];
        // Reading the header needs no password; a file without a Hello slot
        // (or one that cannot be read) uses the password.
        let hello = LockedVault::open(&path).ok().and_then(|v| v.hello());
        if hello.is_none() {
            password.read(cx).focus(window);
        }
        Self {
            hello,
            use_password: false,
            app,
            path,
            reason,
            password,
            problem: None,
            failures: 0,
            retry_at: None,
            busy: false,
            open_sessions,
            _subscriptions: subscriptions,
        }
    }

    fn on_field(&mut self, _: &Entity<TextField>, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        if *event == TextFieldEvent::Submitted {
            self.unlock(window, cx);
        }
    }

    fn waiting(&self) -> Option<u64> {
        self.retry_at.and_then(|at| at.checked_duration_since(Instant::now())).map(|d| d.as_secs() + 1)
    }

    fn unlock(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let password = self.password.read(cx).text().to_string();
        if self.busy || self.waiting().is_some() || password.is_empty() {
            return;
        }
        self.busy = true;
        self.problem = None;
        let path = self.path.clone();
        let task = cx.background_executor().spawn(async move { LockedVault::open(&path)?.unlock(&password) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(vault) => {
                        this.password.update(cx, |f, cx| f.set_text("", cx));
                        let restored = vault.restored_from_backup;
                        let _ = this.app.update(cx, |app, cx| app.opened(vault, restored, cx));
                    }
                    Err(VaultError::WrongPassword) => {
                        this.failures += 1;
                        this.password.update(cx, |f, cx| f.set_text("", cx));
                        let delay = lockout_delay(this.failures);
                        this.problem = Some("Wrong vault password.".into());
                        if !delay.is_zero() {
                            this.retry_at = Some(Instant::now() + delay);
                            this.tick(cx);
                        }
                    }
                    Err(e) => this.problem = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Unlock with Windows Hello": Hello signs the slot's challenge, and the
    /// signature opens the vault. Both wait on Windows, off the UI thread.
    fn unlock_with_hello(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.hello.clone() else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        self.problem = None;
        let path = self.path.clone();
        let task = cx.background_executor().spawn(async move {
            let signature = crate::hello::sign(&request.credential, &request.challenge).map_err(|e| e.message())?;
            LockedVault::open(&path).and_then(|v| v.unlock_with_hello(&signature)).map_err(|e| match e {
                VaultError::HelloRejected => "Windows Hello's key did not open this vault. Unlock with the vault password, then turn Windows Hello on again in Settings.".to_string(),
                e => e.to_string(),
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(vault) => {
                        let restored = vault.restored_from_backup;
                        let _ = this.app.update(cx, |app, cx| app.opened(vault, restored, cx));
                    }
                    Err(message) => this.problem = Some(message.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Redraw every second while the retry delay runs down.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(Duration::from_millis(250));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                cx.notify();
                if this.waiting().is_some() {
                    this.tick(cx);
                } else {
                    this.retry_at = None;
                }
            });
        })
        .detach();
    }
}

impl Render for Locked {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hello = self.hello.is_some() && !self.use_password;
        let body: SharedString = self.reason.body(self.hello.is_some()).into();
        let waiting = self.waiting();
        let help: Option<(SharedString, bool)> = match (&self.problem, waiting) {
            (Some(p), Some(secs)) => Some((format!("{p} Try again in {secs} s.").into(), true)),
            (Some(p), None) => Some((p.clone(), true)),
            (None, _) => None,
        };
        let card = gate_card("lock", "Vault locked", body);
        let card = if hello {
            // Locked.dc.html: the Hello button, then the password as an alternative.
            card.child(
                Button::primary(
                    "unlock-hello",
                    if self.busy { "Waiting for Windows Hello…" } else { "Unlock with Windows Hello" },
                )
                .full_width()
                .disabled(self.busy)
                .on_click(cx.listener(|this, _, _, cx| this.unlock_with_hello(cx))),
            )
            .children(
                self.problem
                    .clone()
                    .map(|p| div().w_full().text_body().text_color(theme::warn()).text_center().child(p)),
            )
            .child(
                Button::quiet("use-password", "Use vault password instead").disabled(self.busy).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.use_password = true;
                        this.problem = None;
                        this.password.read(cx).focus(window);
                        cx.notify();
                    },
                )),
            )
        } else {
            let label = if self.busy {
                "Unlocking…"
            } else if waiting.is_some() {
                "Wait to try again"
            } else {
                "Unlock"
            };
            card.child(div().w_full().flex().flex_col().child(field("Vault password", self.password.clone(), help)))
                .child(
                    Button::primary("unlock", label)
                        .full_width()
                        .disabled(self.busy || waiting.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.unlock(window, cx))),
                )
                .when(self.hello.is_some(), |d| {
                    d.child(Button::quiet("use-hello", "Use Windows Hello instead").disabled(self.busy).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.use_password = false;
                            this.problem = None;
                            cx.notify();
                        }),
                    ))
                })
        };
        gate_body(card.when(self.open_sessions > 0, |d| {
            let n = self.open_sessions;
            d.child(
                div()
                    .w_full()
                    .pt(theme::SPACE_3)
                    .border_t_1()
                    .border_color(theme::line())
                    .text_meta()
                    .text_color(theme::ink_muted())
                    .text_center()
                    .child(SharedString::from(if n == 1 {
                        "1 session is open. It stays connected in the background and reappears when you unlock."
                            .to_string()
                    } else {
                        format!(
                            "{n} sessions are open. They stay connected in the background and reappear when you unlock."
                        )
                    })),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_grows_after_three_failures_and_caps() {
        assert_eq!(lockout_delay(0), Duration::ZERO);
        assert_eq!(lockout_delay(2), Duration::ZERO);
        assert_eq!(lockout_delay(3), Duration::from_secs(1));
        assert_eq!(lockout_delay(4), Duration::from_secs(2));
        assert_eq!(lockout_delay(7), Duration::from_secs(16));
        assert_eq!(lockout_delay(8), Duration::from_secs(30));
        assert_eq!(lockout_delay(50), Duration::from_secs(30));
    }

    #[test]
    fn lock_reasons_read_as_drawn() {
        assert!(LockReason::Idle(15).body(true).starts_with("Holonet locked after 15 minutes idle."));
        assert!(LockReason::Idle(60).body(true).starts_with("Holonet locked after 1 hour idle."));
        assert!(LockReason::WindowsLocked.body(false).starts_with("Holonet locked when Windows locked."));
        assert!(LockReason::Startup.body(false).starts_with("Enter your vault password"));
    }
}
