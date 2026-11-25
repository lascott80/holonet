//! The vault gate: create a vault on first start, unlock it, and the
//! wrong-password delay. Locked follows docs/design/screens/Locked.dc.html
//! with the vault password as the method (Windows Hello is milestone 4).
//! Create vault is not drawn; it uses the same card.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    BoxShadow, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, WeakEntity, Window, div,
    hsla, point, prelude::*, px,
};
use holonet_vault::{LockedVault, MIN_PASSWORD_LEN, UnlockedVault, VaultError};

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
    _subscriptions: Vec<Subscription>,
}

impl Locked {
    pub fn new(
        app: WeakEntity<HolonetApp>,
        path: PathBuf,
        reason: LockReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(|cx| TextField::new(cx, "").password());
        let subscriptions = vec![cx.subscribe_in(&password, window, Self::on_field)];
        password.read(cx).focus(window);
        Self {
            app,
            path,
            reason,
            password,
            problem: None,
            failures: 0,
            retry_at: None,
            busy: false,
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
        let body: SharedString = match self.reason {
            LockReason::Startup => "Enter your vault password to open your hosts, keys and passwords.".into(),
            LockReason::Manual => {
                "You locked the vault. Your hosts, keys and passwords are encrypted until you unlock.".into()
            }
        };
        let waiting = self.waiting();
        let help: Option<(SharedString, bool)> = match (&self.problem, waiting) {
            (Some(p), Some(secs)) => Some((format!("{p} Try again in {secs} s.").into(), true)),
            (Some(p), None) => Some((p.clone(), true)),
            (None, _) => None,
        };
        let label = if self.busy {
            "Unlocking…"
        } else if waiting.is_some() {
            "Wait to try again"
        } else {
            "Unlock"
        };
        gate_body(
            gate_card("lock", "Vault locked", body)
                .child(div().w_full().flex().flex_col().child(field("Vault password", self.password.clone(), help)))
                .child(
                    Button::primary("unlock", label)
                        .full_width()
                        .disabled(self.busy || waiting.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.unlock(window, cx))),
                ),
        )
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
}
