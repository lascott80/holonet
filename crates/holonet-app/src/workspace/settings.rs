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
    ("Close tab", "Ctrl Shift W"),
    ("Next tab", "Ctrl Tab"),
    ("Previous tab", "Ctrl Shift Tab"),
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
    &[("Terminal", 1), ("Vault and security", 2), ("Connection", 3), ("Keyboard", 4), ("About", 5)];

/// Settings page state kept by the workspace.
pub(crate) struct SettingsUi {
    pub(super) unknown_keys: Entity<Select>,
    font_size: Entity<Slider>,
    line_height: Entity<Slider>,
    scrollback: Entity<Select>,
    keep_alive: Entity<Select>,
    /// Created at the first render (it needs a window).
    preview: Option<Entity<TerminalView>>,
    scroll: ScrollHandle,
    font_picker: Option<FontPicker>,
    licences_open: bool,
}

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
        Self {
            unknown_keys,
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
            .child(page_block(div().child(div().text_title().text_color(theme::ink()).child("Settings")).child(chips)))
            .child(page_block(self.terminal_section(&settings, &installed, preview, cx)))
            .child(page_block(self.security_section(cx)))
            .child(page_block(self.connection_section(&connection)))
            .child(page_block(keyboard_section()))
            .child(page_block(self.about_section(cx)))
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
        section("Vault and security").child(setting_card(vec![
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

    fn connection_section(&self, _connection: &holonet_model::ConnectionSettings) -> impl IntoElement {
        // "Reconnect after sleep or network change" joins this section with
        // automatic reconnect (milestone 3 phase 7).
        section("Connection").child(setting_card(vec![
            setting_row(
                "Keep-alive interval",
                Some("Applies to new connections. Three unanswered keep-alives end a connection.".into()),
                div().w(px(200.)).child(self.settings_ui.keep_alive.clone()),
            )
            .into_any_element(),
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

    /// The font picker or the licences dialog, if open.
    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entity = cx.entity();
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
fn page_block(child: impl IntoElement) -> gpui::Div {
    div().w_full().max_w(px(1080.)).child(child)
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
        .when(enabled, |d| d.cursor_pointer().on_click(on_toggle))
        .child(setting_row(name, help.map(SharedString::from), mark))
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
