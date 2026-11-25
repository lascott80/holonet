//! Design tokens from docs/design/tokens.json, named as there.
//!
//! Colors are functions returning `Rgba` so they drop straight into GPUI
//! styling (`.bg(theme::card())`). Type styles are extension methods on
//! anything `Styled` (`.text_title()`), so every screen uses the same sizes.
//! A few values the screens draw that are not in tokens.json are named at the
//! bottom, with where they come from.

use gpui::{BoxShadow, FontWeight, Pixels, Rgba, Styled, hsla, point, px, rgb, rgba};

macro_rules! colors {
    ($($name:ident = $hex:literal;)*) => {
        $(pub fn $name() -> Rgba { rgb($hex) })*
    };
}

colors! {
    ground = 0x121212;
    sunken = 0x0A0A0A;
    terminal = 0x101010;
    panel = 0x171717;
    card = 0x1C1C1C;
    card_selected = 0x242424;
    raised = 0x2A2A2A;
    line = 0x2C2C2C;
    line_strong = 0x3D3D3D;
    line_soft = 0x1F1F1F;
    ink_faint = 0x666666;
    ink = 0xEDEDED;
    ink_soft = 0xCFCFCF;
    ink_muted = 0xA3A3A3;
    ink_dim = 0x8A8A8A;
    accent = 0x5CC8FF;
    on_accent = 0x06202E;
    group_violet = 0xB9A5FF;
    group_mint = 0x7FD6B5;
    group_gray = 0xA3A3A3;
    warn = 0xF2A65A;
    on_warn = 0x2A1A05;
    warn_surface = 0x1C1710;
    warn_line = 0x5A4A33;
    warn_ink = 0xD9CDBB;
}

/// `accent` at a given opacity (selection fills, icon tiles).
pub fn accent_alpha(alpha: f32) -> Rgba {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    rgba(0x5CC8FF00 | a)
}

/// Any token color at a given opacity.
pub fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

// Spacing.
pub const SPACE_1: Pixels = px(4.);
pub const SPACE_2: Pixels = px(8.);
pub const SPACE_3: Pixels = px(12.);
pub const SPACE_4: Pixels = px(16.);
pub const SPACE_5: Pixels = px(24.);
pub const SPACE_6: Pixels = px(32.);
/// Control height (decisions.md: 44px, mouse-first).
pub const CONTROL: Pixels = px(44.);
/// Control height in the title bar, toolbars and pane headers.
pub const CONTROL_COMPACT: Pixels = px(36.);

// Radius.
pub const RADIUS_XS: Pixels = px(4.);
pub const RADIUS_SM: Pixels = px(6.);
pub const RADIUS_MD: Pixels = px(8.);
pub const RADIUS_LG: Pixels = px(10.);
pub const RADIUS_XL: Pixels = px(12.);
pub const RADIUS_2XL: Pixels = px(14.);

// Families. Inter and Space Grotesk are bundled (assets/fonts).
pub const SANS: &str = "Inter";
pub const DISPLAY: &str = "Space Grotesk";
/// Mono family for interface text, chosen at startup from what is installed
/// (see [`init_fonts`]): JetBrains Mono, its Nerd Font builds, then Cascadia
/// Mono and Consolas.
pub fn mono() -> &'static str {
    MONO_FAMILY.get().copied().unwrap_or("Consolas")
}

static MONO_FAMILY: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

const MONO_PREFERENCE: &[&str] = &[
    "JetBrains Mono",
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "JetBrainsMonoNL Nerd Font Mono",
    "Cascadia Mono",
    "Consolas",
];

/// Pick the mono family from the installed fonts. Call once at startup.
pub fn init_fonts(cx: &gpui::App) {
    let installed = cx.text_system().all_font_names();
    let family = MONO_PREFERENCE.iter().copied().find(|f| installed.iter().any(|i| i == f)).unwrap_or("Consolas");
    let _ = MONO_FAMILY.set(family);
}

pub fn shadow_panel() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.28),
        offset: point(px(-16.), px(0.)),
        blur_radius: px(40.),
        spread_radius: px(0.),
    }]
}

pub fn shadow_hover() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.35),
        offset: point(px(0.), px(8.)),
        blur_radius: px(24.),
        spread_radius: px(0.),
    }]
}

pub fn shadow_dialog() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.6),
        offset: point(px(0.), px(32.)),
        blur_radius: px(80.),
        spread_radius: px(0.),
    }]
}

/// Type styles from tokens.json (`type.groups`).
pub trait TypeStyles: Styled + Sized {
    /// Display 28/1.2/600, -0.01em. Screen titles ("Hosts").
    fn text_title(self) -> Self {
        self.font_family(DISPLAY).text_size(px(28.)).line_height(px(28. * 1.2)).font_weight(FontWeight::SEMIBOLD)
    }
    /// Display 20/1.25/600. Detail panel titles.
    fn text_panel_title(self) -> Self {
        self.font_family(DISPLAY).text_size(px(20.)).line_height(px(25.)).font_weight(FontWeight::SEMIBOLD)
    }
    /// Display 18/1.3/600. Dialog titles.
    fn text_heading(self) -> Self {
        self.font_family(DISPLAY).text_size(px(18.)).line_height(px(18. * 1.3)).font_weight(FontWeight::SEMIBOLD)
    }
    /// Sans 14/1.5/400.
    fn text_body(self) -> Self {
        self.font_family(SANS).text_size(px(14.)).line_height(px(21.)).font_weight(FontWeight::NORMAL)
    }
    /// Sans 14/1.5/500.
    fn text_body_strong(self) -> Self {
        self.font_family(SANS).text_size(px(14.)).line_height(px(21.)).font_weight(FontWeight::MEDIUM)
    }
    /// Sans 12/1.5/400. Labels above fields, help text, timestamps.
    fn text_meta(self) -> Self {
        self.font_family(SANS).text_size(px(12.)).line_height(px(18.)).font_weight(FontWeight::NORMAL)
    }
    /// Sans 11/1.4/600, 0.08em, uppercase. Section labels ("ALL HOSTS").
    /// GPUI has no letter-spacing or text-transform; callers pass uppercase text.
    fn text_label(self) -> Self {
        self.font_family(SANS).text_size(px(11.)).line_height(px(11. * 1.4)).font_weight(FontWeight::SEMIBOLD)
    }
    /// Mono 13/1.6/400. Addresses, commands, ports.
    fn text_mono(self) -> Self {
        self.font_family(mono()).text_size(px(13.)).line_height(px(13. * 1.6)).font_weight(FontWeight::NORMAL)
    }
    /// Mono 12/1.5/400. Fingerprints, small technical values.
    fn text_mono_small(self) -> Self {
        self.font_family(mono()).text_size(px(12.)).line_height(px(18.)).font_weight(FontWeight::NORMAL)
    }
}

impl<T: Styled> TypeStyles for T {}

// Values drawn in the screens but missing from tokens.json (milestone 2 plan,
// "Defaults"). Named here so they are not scattered as literals.

/// Divider after the vault tab in the title bar (#4A4A4A in the screens).
pub fn tab_divider() -> Rgba {
    rgb(0x4A4A4A)
}

/// Inactive session tab hover (#161616 in the screens).
pub fn tab_hover() -> Rgba {
    rgb(0x161616)
}

/// Opacity of a group color on an inactive tab's top edge.
pub const INACTIVE_TAB_EDGE_ALPHA: f32 = 0.45;

/// Group color for hosts with no group (Known hosts uses `ink-faint`).
pub fn no_group() -> Rgba {
    ink_faint()
}
