//! Settings page (Settings.dc.html): Terminal with a live preview, Vault and
//! security, Connection, Keyboard and About. Sections from later milestones
//! (Appearance, SFTP, Updates) appear when they work.

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, ScrollHandle, SharedString, Window, div,
    prelude::*, px,
};
use holonet_model::{
    BundledFont, ColorScheme, CursorShape, FONT_SIZE_RANGE, KeepAlive, LINE_HEIGHT_RANGE, Scrollback, TerminalFont,
    TerminalSettings,
};

use holonet_vault::{LockedVault, VaultError};

use super::{Workspace, short_path};
use crate::terminal_view::TerminalView;
use crate::theme::{self, TypeStyles};
use crate::ui::{
    Button, Segmented, Select, SelectEvent, Slider, SliderEvent, TextField, TextFieldEvent, dialog, dialog_buttons,
    icon, modal, setting_card, setting_row, setting_row_with,
};

/// Sample output for the preview (Settings.dc.html), with Nerd Font icons so
/// the builds can be told apart. Colors are palette entries, so the preview
/// follows the scheme: blue for the prompt, green for hashes, bright black
/// for the dim parts.
const PREVIEW: &str = concat!(
    "\x1b[34mdeploy@prod-web-01\x1b[90m:~$ \x1b[0mgit log --oneline -3\r\n",
    "\x1b[32ma1b2c3d\x1b[0m fix: retry when status != 200 && tries <= 3\r\n",
    "\x1b[32md4e5f6a\x1b[0m feat: hosts |> filter -> connect => session\r\n",
    "\x1b[32m0f9e8d7\x1b[0m docs: 0O 1lI characters stay distinct\r\n",
    "\x1b[90m\u{f07b} ~/src/holonet \u{e0b0} \u{e725} main \u{f17c}\x1b[0m\r\n",
    "\x1b[34mdeploy@prod-web-01\x1b[90m:~$ \x1b[0m",
);
const PREVIEW_ROWS: usize = 6;

/// Shortcuts that work in this build (Settings.dc.html lists the full set;
/// each joins this list when its feature lands).
const SHORTCUTS: &[(&str, &str)] = &[
    ("New tab", "Ctrl Shift T"),
    ("Close pane or tab", "Ctrl Shift W"),
    ("Next tab", "Ctrl Tab"),
    ("Previous tab", "Ctrl Shift Tab"),
    ("Split right", "Ctrl Shift D"),
    ("Split down", "Ctrl Shift E"),
    ("Next pane", "Ctrl Shift ]"),
    ("Previous pane", "Ctrl Shift ["),
    ("Find in terminal", "Ctrl Shift F"),
    ("Copy", "Ctrl Shift C"),
    ("Paste", "Ctrl Shift V"),
];

const LICENCES: &[(&str, &str)] = &[
    ("JetBrainsMono Nerd Font: notices", include_str!("../../assets/fonts/jetbrainsmono-nerd/NOTICE.md")),
    (
        "JetBrainsMono Nerd Font: SIL Open Font License 1.1",
        include_str!("../../assets/fonts/jetbrainsmono-nerd/OFL.txt"),
    ),
    ("Inter: SIL Open Font License 1.1", include_str!("../../assets/fonts/Inter-OFL.txt")),
    ("Space Grotesk: SIL Open Font License 1.1", include_str!("../../assets/fonts/SpaceGrotesk-OFL.txt")),
];

/// Sections, in page order: (chip label, child index in the scroll container).
const SECTIONS: &[(&str, usize)] =
    &[("Terminal", 1), ("Vault and security", 2), ("Connection", 3), ("SFTP", 4), ("Keyboard", 5), ("About", 6)];

/// Settings, SFTP, "When a file already exists" and "Transfers at once"
/// options in order.
const ON_CONFLICT: [holonet_model::OnConflict; 3] =
    [holonet_model::OnConflict::Ask, holonet_model::OnConflict::Overwrite, holonet_model::OnConflict::KeepBoth];
const AT_ONCE: [holonet_model::AtOnce; 3] =
    [holonet_model::AtOnce::Two, holonet_model::AtOnce::Four, holonet_model::AtOnce::Eight];

/// Settings page state kept by the workspace.
pub(crate) struct SettingsUi {
    pub(super) unknown_keys: Entity<Select>,
    font_size: Entity<Slider>,
    line_height: Entity<Slider>,
    scrollback: Entity<Select>,
    keep_alive: Entity<Select>,
    fallback_wait: Entity<Select>,
    on_conflict: Entity<Select>,
    at_once: Entity<Select>,
    /// "Lock vault after": 15 minutes, 1 hour, Never.
    lock_after: Entity<Select>,
    /// Whether Windows Hello is set up on this PC; `None` while checking.
    hello_available: Option<bool>,
    /// "Turn on Windows Hello", asking for the vault password first.
    hello_dialog: Option<HelloDialog>,
    /// Created at the first render (it needs a window).
    preview: Option<Entity<TerminalView>>,
    scroll: ScrollHandle,
    font_picker: Option<FontPicker>,
    licences_open: bool,
}

struct HelloDialog {
    password: Entity<TextField>,
    problem: Option<SharedString>,
    busy: bool,
}

/// The side navigation's width (as on the Hosts page).
const NAV: f32 = 220.;

/// Settings, Vault and security, "Lock vault after" options in order.
const LOCK_AFTER: [Option<u32>; 3] = [Some(15), Some(60), None];

struct FontPicker {
    search: Entity<TextField>,
    /// Installed families, sorted, read when the picker opened.
    fonts: Vec<SharedString>,
}

fn scrollback_index(s: Scrollback) -> usize {
    match s {
        Scrollback::Lines10k => 0,
        Scrollback::Lines50k => 1,
        Scrollback::Unlimited => 2,
    }
}

/// Settings, Connection, "Tailscale fallback wait" options in order.
const FALLBACK_WAITS: [holonet_model::FallbackWait; 4] = [
    holonet_model::FallbackWait::Secs2,
    holonet_model::FallbackWait::Secs3,
    holonet_model::FallbackWait::Secs5,
    holonet_model::FallbackWait::Secs10,
];

fn keep_alive_index(k: KeepAlive) -> usize {
    match k {
        KeepAlive::Secs30 => 0,
        KeepAlive::Secs60 => 1,
        KeepAlive::Off => 2,
    }
}

impl SettingsUi {
    pub(super) fn new(settings: &holonet_model::Settings, cx: &mut Context<Workspace>) -> Self {
        let refuse = settings.unknown_host_keys == holonet_model::UnknownHostKeys::Refuse;
        let t = &settings.terminal;
        let unknown_keys = cx
            .new(|cx| Select::new(cx, vec!["Ask every time".into(), "Refuse to connect".into()], usize::from(refuse)));
        let font_size = cx.new(|cx| {
            Slider::new(cx, *FONT_SIZE_RANGE.start() as i32, *FONT_SIZE_RANGE.end() as i32, t.font_size() as i32, |v| {
                format!("{v}px")
            })
        });
        let line_height = cx.new(|cx| {
            Slider::new(
                cx,
                *LINE_HEIGHT_RANGE.start() as i32,
                *LINE_HEIGHT_RANGE.end() as i32,
                (t.line_height() * 10.0).round() as i32,
                |v| format!("{}.{}", v / 10, v % 10),
            )
        });
        let scrollback = cx.new(|cx| {
            Select::new(
                cx,
                vec!["10,000 lines".into(), "50,000 lines".into(), "Unlimited".into()],
                scrollback_index(t.scrollback),
            )
        });
        let keep_alive = cx.new(|cx| {
            Select::new(
                cx,
                vec!["30 seconds".into(), "60 seconds".into(), "Off".into()],
                keep_alive_index(settings.connection.keep_alive),
            )
        });
        let fallback_wait = cx.new(|cx| {
            Select::new(
                cx,
                vec!["2 seconds".into(), "3 seconds".into(), "5 seconds".into(), "10 seconds".into()],
                FALLBACK_WAITS.iter().position(|w| *w == settings.connection.fallback_wait).unwrap_or(1),
            )
        });
        let lock_after = cx.new(|cx| {
            Select::new(
                cx,
                vec!["15 minutes idle".into(), "1 hour idle".into(), "Never".into()],
                LOCK_AFTER.iter().position(|l| *l == settings.lock_after_idle_mins).unwrap_or(0),
            )
        });
        let on_conflict = cx.new(|cx| {
            Select::new(
                cx,
                vec!["Ask".into(), "Overwrite".into(), "Keep both".into()],
                ON_CONFLICT.iter().position(|c| *c == settings.sftp.on_conflict).unwrap_or(0),
            )
        });
        let at_once = cx.new(|cx| {
            Select::new(
                cx,
                vec!["2".into(), "4".into(), "8".into()],
                AT_ONCE.iter().position(|a| *a == settings.sftp.at_once).unwrap_or(0),
            )
        });
        // Asking Windows takes a moment: the switch waits for the answer.
        cx.spawn(async move |this, cx| {
            let available = cx.background_executor().spawn(async { crate::hello::available() }).await;
            let _ = this.update(cx, |w: &mut Workspace, cx| {
                w.settings_ui.hello_available = Some(available);
                cx.notify();
            });
        })
        .detach();
        Self {
            unknown_keys,
            fallback_wait,
            lock_after,
            on_conflict,
            at_once,
            hello_available: None,
            hello_dialog: None,
            font_size,
            line_height,
            scrollback,
            keep_alive,
            preview: None,
            scroll: ScrollHandle::new(),
            font_picker: None,
            licences_open: false,
        }
    }

    /// Subscriptions that save each control's changes.
    pub(super) fn subscribe(&self, cx: &mut Context<Workspace>) -> Vec<gpui::Subscription> {
        vec![
            cx.subscribe(&self.unknown_keys, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = event;
                let policy =
                    if *i == 1 { holonet_model::UnknownHostKeys::Refuse } else { holonet_model::UnknownHostKeys::Ask };
                this.save(|d| d.settings.unknown_host_keys = policy, cx);
            }),
            cx.subscribe(&self.font_size, |this: &mut Workspace, _, event: &SliderEvent, cx| {
                let SliderEvent::Changed(v) = *event;
                this.set_terminal(|t| t.font_size = v as u8, cx);
            }),
            cx.subscribe(&self.line_height, |this: &mut Workspace, _, event: &SliderEvent, cx| {
                let SliderEvent::Changed(v) = *event;
                this.set_terminal(|t| t.line_height_tenths = v as u8, cx);
            }),
            cx.subscribe(&self.scrollback, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let s = [Scrollback::Lines10k, Scrollback::Lines50k, Scrollback::Unlimited][i.min(2)];
                this.set_terminal(|t| t.scrollback = s, cx);
            }),
            cx.subscribe(&self.on_conflict, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let choice = ON_CONFLICT[i.min(2)];
                this.set_sftp(|s| s.on_conflict = choice, cx);
            }),
            cx.subscribe(&self.at_once, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let n = AT_ONCE[i.min(2)];
                this.set_sftp(|s| s.at_once = n, cx);
            }),
            cx.subscribe(&self.lock_after, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let after = LOCK_AFTER[i.min(2)];
                this.save(|d| d.settings.lock_after_idle_mins = after, cx);
            }),
            cx.subscribe(&self.fallback_wait, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let w = FALLBACK_WAITS[i.min(3)];
                this.save(|d| d.settings.connection.fallback_wait = w, cx);
            }),
            cx.subscribe(&self.keep_alive, |this: &mut Workspace, _, event: &SelectEvent, cx| {
                let SelectEvent::Changed(i) = *event;
                let k = [KeepAlive::Secs30, KeepAlive::Secs60, KeepAlive::Off][i.min(2)];
                this.save(|d| d.settings.connection.keep_alive = k, cx);
            }),
        ]
    }
}

impl Workspace {
    fn terminal_settings(&self) -> TerminalSettings {
        self.vault_data().map(|d| d.settings.terminal.clone()).unwrap_or_default()
    }

    /// Change and save SFTP settings; open SFTP tabs follow.
    pub(super) fn set_sftp(&mut self, change: impl FnOnce(&mut holonet_model::SftpSettings), cx: &mut Context<Self>) {
        self.save(|d| change(&mut d.settings.sftp), cx);
        let Some(settings) = self.vault_data().map(|d| d.settings.sftp.clone()) else { return };
        let app = self.app.clone();
        // Deferred: the app may be the one updating this workspace right now.
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| app.apply_sftp_settings(&settings, cx));
        });
    }

    /// Change and save terminal settings, then show them in the preview and in
    /// every open terminal.
    pub(super) fn set_terminal(&mut self, change: impl FnOnce(&mut TerminalSettings), cx: &mut Context<Self>) {
        self.save(|d| change(&mut d.settings.terminal), cx);
        let settings = self.terminal_settings();
        if let Some(preview) = &self.settings_ui.preview {
            preview.update(cx, |p, cx| p.set_settings(settings.clone(), cx));
        }
        let app = self.app.clone();
        // Deferred: the app may be the one updating this workspace right now.
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| app.apply_terminal_settings(&settings, cx));
        });
        cx.notify();
    }

    fn open_font_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut fonts: Vec<SharedString> =
            window.text_system().all_font_names().into_iter().map(SharedString::from).collect();
        fonts.sort_by_key(|f| f.to_lowercase());
        fonts.dedup();
        let search = cx.new(|cx| TextField::new(cx, "Find an installed font"));
        let sub = cx.subscribe(&search, |this: &mut Self, field, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Cancelled => {
                if field.read(cx).text().is_empty() {
                    this.settings_ui.font_picker = None;
                } else {
                    field.update(cx, |f, cx| f.set_text("", cx));
                }
                cx.notify();
            }
            _ => cx.notify(),
        });
        self._subscriptions.push(sub);
        search.update(cx, |f, _| f.focus(window));
        self.settings_ui.font_picker = Some(FontPicker { search, fonts });
        cx.notify();
    }

    fn pick_installed_font(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let font = BundledFont::ALL
            .into_iter()
            .find(|b| b.family() == name.as_ref())
            .map(TerminalFont::Bundled)
            .unwrap_or_else(|| TerminalFont::Installed(name.to_string()));
        self.settings_ui.font_picker = None;
        self.set_terminal(|t| t.font = font, cx);
    }

    pub(super) fn settings_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The reading width: the window less the side navigation and padding.
        let width = px((f32::from(window.viewport_size().width) - NAV - 56.).clamp(320., 1080.));
        let settings = self.terminal_settings();
        let connection = self.vault_data().map(|d| d.settings.connection.clone()).unwrap_or_default();
        let preview = self
            .settings_ui
            .preview
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut t = TerminalView::preview(PREVIEW_ROWS, window, cx);
                    t.apply_settings(&settings, window, cx);
                    t.feed(PREVIEW.as_bytes());
                    t
                })
            })
            .clone();
        let installed = window.text_system().all_font_names();
        let scroll = self.settings_ui.scroll.clone();

        let chips = div().flex().flex_wrap().gap(px(6.)).mt(px(14.)).children(SECTIONS.iter().map(|(label, index)| {
            let (index, scroll) = (*index, scroll.clone());
            div()
                .id(SharedString::from(format!("chip-{label}")))
                .px(px(14.))
                .py(theme::SPACE_3)
                .rounded(px(22.))
                .border_1()
                .border_color(theme::line())
                .text_body()
                .text_color(theme::ink())
                .cursor_pointer()
                .hover(|s| s.bg(theme::card()))
                .child(*label)
                .on_click(move |_, _, _| scroll.scroll_to_top_of_item(index))
        }));

        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.settings_ui.scroll)
            .px(px(28.))
            .pt(theme::SPACE_5)
            .pb(px(40.))
            .flex()
            .flex_col()
            .gap(theme::SPACE_5)
            .child(page_block(
                width,
                div().child(div().text_title().text_color(theme::ink()).child("Settings")).child(chips),
            ))
            .child(page_block(width, self.terminal_section(&settings, &installed, preview, cx)))
            .child(page_block(width, self.security_section(cx)))
            .child(page_block(width, self.connection_section(&connection, cx)))
            .child(page_block(width, self.sftp_section(cx)))
            .child(page_block(width, keyboard_section()))
            .child(page_block(width, self.about_section(cx)))
    }

    fn terminal_section(
        &self,
        s: &TerminalSettings,
        installed: &[String],
        preview: Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let line_px = s.font_size() * s.line_height();
        let preview_height = px((line_px * PREVIEW_ROWS as f32).ceil() + 28.0);
        let summary: SharedString = format!("{} · {}px", s.font.family(), s.font_size()).into();
        let preview_card = div()
            .border_1()
            .border_color(theme::line())
            .rounded(theme::RADIUS_LG)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(theme::SPACE_3)
                    .px(px(14.))
                    .py(theme::SPACE_2)
                    .bg(theme::panel())
                    .border_b_1()
                    .border_color(theme::line())
                    .text_size(px(12.))
                    .text_color(theme::ink_muted())
                    .child("Preview")
                    .child(div().font_family(theme::mono()).child(summary)),
            )
            .child(div().h(preview_height).child(preview));

        // Font cards: the four bundled builds, the Windows fonts that are
        // installed, and an installed font chosen with the picker.
        let mut cards: Vec<(TerminalFont, &'static str, bool)> = vec![
            (TerminalFont::Bundled(BundledFont::NerdFont), "Default. Ligatures and double-width icon glyphs.", true),
            (TerminalFont::Bundled(BundledFont::NerdFontMono), "Icons fit a single cell for strict alignment.", true),
            (
                TerminalFont::Bundled(BundledFont::NerdFontPropo),
                "Proportional icon widths, for prompts and status lines.",
                true,
            ),
            (TerminalFont::Bundled(BundledFont::NerdFontNl), "Same glyphs with ligatures removed.", true),
        ];
        for name in ["Cascadia Mono", "Consolas"] {
            if installed.iter().any(|f| f == name) {
                cards.push((TerminalFont::Installed(name.into()), "Uses the copy installed with Windows.", false));
            }
        }
        if !cards.iter().any(|(f, _, _)| *f == s.font) {
            cards.push((s.font.clone(), "Installed on this PC.", false));
        }
        let card_elems: Vec<AnyElement> = cards
            .into_iter()
            .enumerate()
            .map(|(i, (font, note, bundled))| {
                let on = font == s.font;
                let family: SharedString = font.family().to_string().into();
                let pick = font.clone();
                div()
                    .id(("font-card", i))
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w(px(300.))
                    .min_h(px(64.))
                    .px(px(14.))
                    .py(theme::SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .when(on, |d| d.bg(theme::raised()).border_color(theme::accent()))
                    .when(!on, |d| {
                        d.bg(theme::card()).border_color(theme::line()).hover(|s| s.bg(theme::card_selected()))
                    })
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme::SPACE_2)
                            .child(
                                div()
                                    .font_family(family.clone())
                                    .text_size(px(15.))
                                    .text_color(theme::ink())
                                    .child(family),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if bundled { theme::accent() } else { theme::ink_muted() })
                                    .child(if bundled { "BUNDLED" } else { "SYSTEM" }),
                            ),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(note))
                    .when(!on, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            let font = pick.clone();
                            this.set_terminal(|t| t.font = font, cx);
                        }))
                    })
                    .into_any_element()
            })
            .collect();
        let grid = card_pairs(card_elems);
        let fonts = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .items_baseline()
                    .gap(theme::SPACE_2)
                    .mb(px(10.))
                    .child(div().text_body_strong().text_color(theme::ink()).child("Font"))
                    .child(div().text_meta().text_color(theme::ink_muted()).child(
                        "Nerd Font builds add Powerline and icon glyphs for prompts like Starship and Oh My Posh",
                    )),
            )
            .child(grid)
            .child(
                div().mt(px(10.)).flex().child(
                    div()
                        .id("other-font")
                        .h(theme::CONTROL)
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .rounded(theme::RADIUS_MD)
                        .border_1()
                        .border_dashed()
                        .border_color(theme::line_strong())
                        .text_body()
                        .text_color(theme::ink_muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::ink()))
                        .child("Use another installed font")
                        .on_click(cx.listener(|this, _, window, cx| this.open_font_picker(window, cx))),
                ),
            );

        let lig_available = s.font.supports_ligatures();
        let rows = vec![
            setting_row("Font size", None, self.settings_ui.font_size.clone()).into_any_element(),
            setting_row("Line height", None, self.settings_ui.line_height.clone()).into_any_element(),
            check_row(
                "ligatures",
                "Font ligatures",
                Some(if lig_available {
                    "Joins character pairs such as != and => into one symbol."
                } else {
                    "Not available in the NL build of this font."
                }),
                s.ligatures_on(),
                lig_available,
                cx.listener(|this, _: &ClickEvent, _, cx| this.set_terminal(|t| t.ligatures = !t.ligatures, cx)),
            ),
            setting_row(
                "Cursor",
                None,
                Segmented::new(
                    "cursor",
                    vec!["Block".into(), "Bar".into(), "Underline".into()],
                    match s.cursor {
                        CursorShape::Block => 0,
                        CursorShape::Bar => 1,
                        CursorShape::Underline => 2,
                    },
                )
                .on_change(Self::on_terminal(cx, |t, i| {
                    t.cursor = [CursorShape::Block, CursorShape::Bar, CursorShape::Underline][i.min(2)]
                })),
            )
            .into_any_element(),
            check_row(
                "blink",
                "Blinking cursor",
                Some("Off while Windows animation effects are turned off."),
                s.cursor_blink,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| this.set_terminal(|t| t.cursor_blink = !t.cursor_blink, cx)),
            ),
            setting_row(
                "Color scheme",
                None,
                Segmented::new(
                    "scheme",
                    vec!["Holonet Dark".into(), "High contrast".into(), "Light".into()],
                    match s.scheme {
                        ColorScheme::HolonetDark => 0,
                        ColorScheme::HighContrast => 1,
                        ColorScheme::Light => 2,
                    },
                )
                .on_change(Self::on_terminal(cx, |t, i| {
                    t.scheme = [ColorScheme::HolonetDark, ColorScheme::HighContrast, ColorScheme::Light][i.min(2)]
                })),
            )
            .into_any_element(),
            setting_row("Scrollback", None, div().w(px(200.)).child(self.settings_ui.scrollback.clone()))
                .into_any_element(),
            check_row(
                "copy-on-select",
                "Copy on select",
                None,
                s.copy_on_select,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_terminal(|t| t.copy_on_select = !t.copy_on_select, cx)
                }),
            ),
            check_row(
                "right-click",
                "Right-click pastes",
                None,
                s.right_click_pastes,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_terminal(|t| t.right_click_pastes = !t.right_click_pastes, cx)
                }),
            ),
        ];

        section("Terminal").gap(theme::SPACE_4).child(preview_card).child(fonts).child(setting_card(rows))
    }

    /// A handler for an index control that edits terminal settings.
    fn on_terminal(
        cx: &Context<Self>,
        change: impl Fn(&mut TerminalSettings, usize) + 'static,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static {
        let workspace = cx.weak_entity();
        let change = std::rc::Rc::new(change);
        move |i, _, cx| {
            let change = change.clone();
            let _ = workspace.update(cx, |this, cx| this.set_terminal(|t| change(t, i), cx));
        }
    }

    fn security_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.path.clone();
        let has_hello = self.vault.as_ref().is_some_and(|v| v.has_hello());
        let lock_windows = self.vault_data().is_some_and(|d| d.settings.lock_when_windows_locks);
        let (hello_help, hello_enabled): (SharedString, bool) = match self.settings_ui.hello_available {
            _ if has_hello => (
                "Use your PIN, fingerprint or face instead of the vault password. The vault password always works too."
                    .into(),
                true,
            ),
            None => ("Checking whether Windows Hello is set up on this PC…".into(), false),
            Some(false) => (crate::hello::UNAVAILABLE.into(), false),
            Some(true) => (
                "Use your PIN, fingerprint or face instead of the vault password. Turning it on asks for the vault password first."
                    .into(),
                true,
            ),
        };
        section("Vault and security").child(setting_card(vec![
            check_row_with(
                "hello-unlock",
                "Unlock with Windows Hello",
                Some(hello_help),
                has_hello,
                hello_enabled && self.vault.is_some(),
                cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_hello(window, cx)),
            ),
            setting_row(
                "Lock vault after",
                Some(
                    "Only typing, clicking or moving the mouse in Holonet counts. Open sessions stay connected.".into(),
                ),
                div().w(px(200.)).child(self.settings_ui.lock_after.clone()),
            )
            .into_any_element(),
            check_row(
                "lock-windows",
                "Lock when Windows locks",
                Some("Win L, or the screen locking by itself."),
                lock_windows,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.save(|d| d.settings.lock_when_windows_locks = !d.settings.lock_when_windows_locks, cx);
                }),
            ),
            setting_row(
                "Unknown host keys",
                Some("Refuse to connect stops any server whose key is not already in Known hosts.".into()),
                div().w(px(240.)).child(self.settings_ui.unknown_keys.clone()),
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
                    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
                }),
            )
            .into_any_element(),
        ]))
    }

    fn connection_section(
        &self,
        connection: &holonet_model::ConnectionSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        section("Connection").child(setting_card(vec![
            setting_row(
                "Keep-alive interval",
                Some("Applies to new connections. Three unanswered keep-alives end a connection.".into()),
                div().w(px(200.)).child(self.settings_ui.keep_alive.clone()),
            )
            .into_any_element(),
            setting_row(
                "Tailscale fallback wait",
                Some(
                    "How long Automatic waits for a host's primary address before trying its Tailscale address. Skipped when the primary address is private and unreachable from this network."
                        .into(),
                ),
                div().w(px(200.)).child(self.settings_ui.fallback_wait.clone()),
            )
            .into_any_element(),
            check_row(
                "reconnect",
                "Reconnect after sleep or network change",
                Some("Up to 5 attempts. Scrollback is kept; the shell itself restarts."),
                connection.reconnect,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.save(|d| d.settings.connection.reconnect = !d.settings.connection.reconnect, cx);
                    cx.notify();
                }),
            ),
        ]))
    }

    /// Settings.dc.html, SFTP.
    fn sftp_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let show_hidden = self.vault_data().is_none_or(|d| d.settings.sftp.show_hidden);
        section("SFTP").child(setting_card(vec![
            setting_row(
                "When a file already exists",
                Some("Ask chooses once for each transfer: overwrite, keep both or skip.".into()),
                div().w(px(200.)).child(self.settings_ui.on_conflict.clone()),
            )
            .into_any_element(),
            setting_row(
                "Transfers at once",
                Some("Files beyond this wait in the queue.".into()),
                div().w(px(200.)).child(self.settings_ui.at_once.clone()),
            )
            .into_any_element(),
            check_row(
                "show-hidden",
                "Show hidden files",
                Some("Hidden files on this PC, and names starting with a dot on hosts."),
                show_hidden,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_sftp(|s| s.show_hidden = !s.show_hidden, cx);
                }),
            ),
        ]))
    }

    fn about_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        section("About").child(setting_card(vec![
            setting_row(
                "Holonet",
                Some(format!("Version {}", env!("CARGO_PKG_VERSION")).into()),
                div(),
            )
            .into_any_element(),
            setting_row(
                "Bundled fonts",
                Some(
                    "JetBrainsMono Nerd Font 3.5.1, Inter 4.1 and Space Grotesk 2.0.0, under the SIL Open Font License 1.1."
                        .into(),
                ),
                Button::secondary("licences", "View licences").on_click(cx.listener(|this, _, _, cx| {
                    this.settings_ui.licences_open = true;
                    cx.notify();
                })),
            )
            .into_any_element(),
        ]))
    }

    /// The Settings switch: on asks for the vault password, off removes the
    /// Hello slot and its credential at once (the password still unlocks).
    fn toggle_hello(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vault) = self.vault.as_mut() else { return };
        if let Some(request) = vault.hello() {
            match vault.remove_hello() {
                Ok(()) => {
                    let name = request.credential;
                    cx.background_executor().spawn(async move { crate::hello::delete(&name) }).detach();
                    self.save(|d| d.settings.hello_unlock = false, cx);
                    self.notice = Some("Windows Hello no longer unlocks the vault. Use the vault password.".into());
                }
                Err(e) => self.save_error = Some(format!("Holonet could not turn off Windows Hello: {e}").into()),
            }
            cx.notify();
            return;
        }
        let password = cx.new(|cx| TextField::new(cx, "").password());
        self._subscriptions.push(cx.subscribe_in(&password, window, |this, _, e: &TextFieldEvent, _, cx| match e {
            TextFieldEvent::Submitted => this.submit_hello(cx),
            TextFieldEvent::Cancelled => this.close_hello_dialog(cx),
            TextFieldEvent::Changed => {}
        }));
        password.read(cx).focus(window);
        self.settings_ui.hello_dialog = Some(HelloDialog { password, problem: None, busy: false });
        cx.notify();
    }

    fn close_hello_dialog(&mut self, cx: &mut Context<Self>) {
        if !self.settings_ui.hello_dialog.as_ref().is_some_and(|d| d.busy) {
            self.settings_ui.hello_dialog = None;
            cx.notify();
        }
    }

    /// Check the vault password, then have Windows Hello create its key and
    /// sign a new challenge, and add the Hello slot. All of it waits on
    /// Argon2 or Windows, so it runs off the UI thread with the vault.
    fn submit_hello(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings_ui.hello_dialog.as_mut() else { return };
        let password = dialog.password.read(cx).text().to_string();
        if dialog.busy || password.is_empty() {
            return;
        }
        let Some(mut vault) = self.vault.take() else { return };
        dialog.busy = true;
        dialog.problem = None;
        let path = self.path.clone();
        let task = cx.background_executor().spawn(async move {
            let mut add = || -> Result<(), String> {
                LockedVault::open(&path).and_then(|l| l.unlock(&password).map(drop)).map_err(|e| match e {
                    VaultError::WrongPassword => "The vault password is wrong.".to_string(),
                    e => e.to_string(),
                })?;
                let name = crate::hello::new_credential_name();
                let challenge = holonet_vault::new_hello_challenge();
                let signature = crate::hello::create_and_sign(&name, &challenge).map_err(|e| e.message())?;
                vault.set_hello(&name, &challenge, &signature).map_err(|e| {
                    crate::hello::delete(&name);
                    format!("Holonet could not save the vault: {e}")
                })
            };
            let result = add();
            (vault, result)
        });
        cx.spawn(async move |this, cx| {
            let (vault, result) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.vault = Some(vault);
                match result {
                    Ok(()) => {
                        this.settings_ui.hello_dialog = None;
                        this.save(|d| d.settings.hello_unlock = true, cx);
                        this.notice =
                            Some("Windows Hello now unlocks the vault. The vault password still works too.".into());
                    }
                    Err(message) => {
                        if let Some(d) = this.settings_ui.hello_dialog.as_mut() {
                            d.busy = false;
                            d.problem = Some(message.into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The font picker, the licences dialog or "Turn on Windows Hello", if open.
    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entity = cx.entity();
        if let Some(d) = &self.settings_ui.hello_dialog {
            let busy = d.busy;
            let close = entity.clone();
            return Some(
                modal(
                    "hello-dialog",
                    px(460.),
                    dialog(
                        false,
                        "Turn on Windows Hello",
                        "Enter the vault password. Windows then asks for your PIN, fingerprint or face, and Holonet saves a Windows Hello key for this vault on this PC.",
                    )
                    .child(crate::ui::field("Vault password", d.password.clone(), d.problem.clone().map(|p| (p, true))))
                    .child(dialog_buttons([
                        Button::quiet("hello-cancel", "Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.close_hello_dialog(cx))),
                        Button::primary("hello-on", if busy { "Waiting for Windows Hello…" } else { "Turn on" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_hello(cx))),
                    ])),
                    move |_, cx| close.update(cx, |this, cx| this.close_hello_dialog(cx)),
                )
                .into_any_element(),
            );
        }
        if let Some(picker) = &self.settings_ui.font_picker {
            let query = picker.search.read(cx).text().to_lowercase();
            let matches: Vec<SharedString> =
                picker.fonts.iter().filter(|f| f.to_lowercase().contains(&query)).take(200).cloned().collect();
            let empty = matches.is_empty();
            let list = div()
                .id("font-list")
                .max_h(px(360.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(matches.into_iter().enumerate().map(|(i, name)| {
                    let pick = name.clone();
                    div()
                        .id(("font", i))
                        .h(px(40.))
                        .px(theme::SPACE_3)
                        .flex_none()
                        .flex()
                        .items_center()
                        .rounded(theme::RADIUS_SM)
                        .text_body()
                        .text_color(theme::ink_soft())
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::raised()).text_color(theme::ink()))
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_installed_font(pick.clone(), cx)))
                }))
                .when(empty, |d| {
                    d.child(
                        div()
                            .p(theme::SPACE_3)
                            .text_meta()
                            .text_color(theme::ink_muted())
                            .child("No installed font matches."),
                    )
                });
            let close = cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
                this.settings_ui.font_picker = None;
                cx.notify();
            });
            return Some(
                modal(
                    "font-picker",
                    px(520.),
                    dialog(
                        false,
                        "Use another installed font",
                        "Monospaced fonts keep the terminal grid aligned. Holonet cannot tell them apart from the rest.",
                    )
                    .max_w(px(520.))
                    .child(picker.search.clone())
                    .child(list)
                    .child(dialog_buttons([Button::quiet("picker-cancel", "Cancel").on_click(close)])),
                    move |_, cx| {
                        entity.update(cx, |this, cx| {
                            this.settings_ui.font_picker = None;
                            cx.notify();
                        })
                    },
                )
                .into_any_element(),
            );
        }
        if self.settings_ui.licences_open {
            let close = cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
                this.settings_ui.licences_open = false;
                cx.notify();
            });
            let text = div()
                .id("licence-text")
                .max_h(px(440.))
                .overflow_y_scroll()
                .p(theme::SPACE_3)
                .rounded(theme::RADIUS_MD)
                .bg(theme::sunken())
                .flex()
                .flex_col()
                .gap(theme::SPACE_4)
                .children(LICENCES.iter().map(|(title, body)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_2)
                        .child(div().text_body_strong().text_color(theme::ink()).child(*title))
                        .child(div().text_mono_small().text_color(theme::ink_soft()).child(*body))
                }));
            return Some(
                modal(
                    "licences",
                    px(760.),
                    dialog(false, "Font licences", "The fonts bundled with Holonet and the terms they are used under.")
                        .max_w(px(760.))
                        .child(text)
                        .child(dialog_buttons([Button::primary("licences-close", "Close").on_click(close)])),
                    move |_, cx| {
                        entity.update(cx, |this, cx| {
                            this.settings_ui.licences_open = false;
                            cx.notify();
                        })
                    },
                )
                .into_any_element(),
            );
        }
        None
    }
}

/// Every top-level block keeps the page's reading width.
fn page_block(width: gpui::Pixels, child: impl IntoElement) -> gpui::Div {
    // A definite width: Taffy does not apply `w_full` with `max_w` inside
    // the scroll column, so blocks took their content's width and text was
    // measured at the wrong width (sections overlapped).
    div().w(width).flex_none().flex().flex_col().child(child)
}

/// Font cards two to a row, equal widths (the design's two-column grid);
/// a row wraps to one column when the page is narrow.
fn card_pairs(cards: Vec<AnyElement>) -> gpui::Div {
    let mut rows = Vec::new();
    let mut cards = cards.into_iter();
    while let Some(first) = cards.next() {
        let second =
            cards.next().unwrap_or_else(|| div().flex_1().flex_basis(px(0.)).min_w(px(300.)).into_any_element());
        rows.push(div().w_full().flex().flex_wrap().gap(px(10.)).child(first).child(second));
    }
    div().w_full().flex().flex_col().gap(px(10.)).children(rows)
}

fn section(title: &'static str) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(theme::SPACE_3)
        .child(div().text_heading().text_color(theme::ink()).child(title))
}

/// A setting row whose whole width toggles a checkbox on the right
/// (Settings.dc.html draws these as `<label>` rows).
fn check_row(
    id: &'static str,
    name: &'static str,
    help: Option<&'static str>,
    checked: bool,
    enabled: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    check_row_with(id, name, help.map(SharedString::from), checked, enabled, on_toggle)
}

fn check_row_with(
    id: &'static str,
    name: &'static str,
    help: Option<SharedString>,
    checked: bool,
    enabled: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let mark = div()
        .size(px(18.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_XS)
        .border_1()
        .when(checked, |b| b.bg(theme::accent()).border_color(theme::accent()))
        .when(!checked, |b| b.bg(theme::card()).border_color(theme::line_strong()))
        .when(!enabled, |b| b.opacity(0.4))
        .when(checked, |b| b.child(icon("check", px(14.), theme::on_accent())));
    div()
        .id(id)
        .flex()
        .flex_col()
        .when(enabled, |d| d.cursor_pointer().on_click(on_toggle))
        .child(setting_row(name, help, mark))
        .into_any_element()
}

fn keyboard_section() -> impl IntoElement {
    section("Keyboard")
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(theme::SPACE_5)
                .px(theme::SPACE_4)
                .py(theme::SPACE_1)
                .rounded(theme::RADIUS_LG)
                .border_1()
                .border_color(theme::line())
                .bg(theme::card())
                .children(SHORTCUTS.iter().map(|(action, combo)| {
                    div()
                        .flex_grow()
                        .flex_basis(px(300.))
                        .min_h(theme::CONTROL)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme::SPACE_4)
                        .border_b_1()
                        .border_color(theme::line())
                        .child(div().text_body().text_color(theme::ink()).child(*action))
                        .child(
                            div()
                                .font_family(theme::mono())
                                .text_size(px(12.))
                                .text_color(theme::ink_soft())
                                .border_1()
                                .border_color(theme::line_strong())
                                .rounded(theme::RADIUS_XS)
                                .px(theme::SPACE_2)
                                .py(px(3.))
                                .child(*combo),
                        )
                })),
        )
        .child(div().text_meta().text_color(theme::ink_muted()).child(
            "App shortcuts use Ctrl Shift, so Ctrl K, Ctrl T, Ctrl W and the other shell editing keys always reach the terminal.",
        ))
}
