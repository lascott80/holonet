//! Bundled fonts and icons, compiled into the executable.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

/// Icons by name (`assets/icons/<name>.svg`), served to GPUI's `svg()` as
/// `icons/<name>.svg`.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons![
    "globe",
    "hosts",
    "terminal",
    "folder",
    "key",
    "settings",
    "forwarding",
    "snippets",
    "known-hosts",
    "history",
    "search",
    "shield-alert",
    "warning",
    "lock",
    "download",
    "chevron-left",
    "chevron-up",
    "chevron-down",
    "refresh",
    "bookmark",
    "plus",
    "check",
    "eye",
    "eye-off",
    "role-server",
    "role-database",
    "role-shield",
    "role-nas",
    "role-chip",
    "role-cloud",
    "win-minimize",
    "win-maximize",
    "win-close",
    "close",
];

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf"),
    include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf"),
];

/// JetBrainsMono Nerd Font v3.5.1, four builds in four styles
/// (`assets/fonts/jetbrainsmono-nerd/NOTICE.md`).
macro_rules! nerd_fonts {
    ($($stem:literal),*) => {
        const NERD_FONTS: &[&[u8]] = &[$(
            include_bytes!(concat!("../assets/fonts/jetbrainsmono-nerd/", $stem, "-Regular.ttf")),
            include_bytes!(concat!("../assets/fonts/jetbrainsmono-nerd/", $stem, "-Bold.ttf")),
            include_bytes!(concat!("../assets/fonts/jetbrainsmono-nerd/", $stem, "-Italic.ttf")),
            include_bytes!(concat!("../assets/fonts/jetbrainsmono-nerd/", $stem, "-BoldItalic.ttf")),
        )*];
    };
}

nerd_fonts![
    "JetBrainsMonoNerdFont",
    "JetBrainsMonoNerdFontMono",
    "JetBrainsMonoNerdFontPropo",
    "JetBrainsMonoNLNerdFont"
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

/// Register the bundled fonts with GPUI's text system.
pub fn load_fonts(cx: &mut App) {
    let fonts = FONTS.iter().chain(NERD_FONTS).map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        eprintln!("holonet: could not load bundled fonts: {err:#}");
    }
}

/// Icon path for `svg().path(..)`.
pub fn icon(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_valid_svg() {
        for (path, bytes) in ICONS {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg") && text.trim_end().ends_with("</svg>"), "{path}");
        }
        assert!(Assets.load("icons/hosts.svg").unwrap().is_some());
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
