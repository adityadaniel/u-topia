//! Colors and metrics. Light and dark follow the system appearance; high contrast is a
//! separate palette (§15.4).
//!
//! The token set every component reads (ALL-2): `text`, `muted`, `line`, `surface` (the
//! page's `bg`, a panel's `surface`), `accent`, `tag`, `danger` and `warning`. Adding a
//! theme means adding one `Theme` value, never a colour at a call site; a test keeps the
//! two palettes in step.

use gpui::{rgb, App, Global, Hsla, WindowAppearance};

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    /// Cards, menus and panels that sit above `bg`.
    pub surface: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub line: Hsla,
    pub hover: Hsla,
    pub accent: Hsla,
    pub tag: Hsla,
    pub selection: Hsla,
    pub cursor: Hsla,
    pub chip_bg: Hsla,
    pub danger_bg: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub highlight_bg: Hsla,
    pub code_bg: Hsla,
    pub banner_bg: Hsla,
    pub bullet: Hsla,
    /// The dimming layer under a modal palette.
    pub scrim: Hsla,
    /// PDF highlight overlays (independent of light/dark paper).
    pub pdf_highlight: Hsla,
    pub pdf_selection: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg: rgb(0xffffff).into(),
            sidebar_bg: rgb(0xf6f6f4).into(),
            surface: rgb(0xfafaf9).into(),
            text: rgb(0x1f2328).into(),
            muted: rgb(0x6b7076).into(),
            faint: rgb(0xa8acb1).into(),
            line: rgb(0xe4e4e1).into(),
            hover: rgb(0xeeeeec).into(),
            accent: rgb(0x2f6fdb).into(),
            tag: rgb(0x7a52c7).into(),
            selection: rgb(0xb7d2fb).into(),
            cursor: rgb(0x2f6fdb).into(),
            chip_bg: rgb(0xeef2f8).into(),
            danger_bg: rgb(0xfbe4e4).into(),
            danger: rgb(0xb42318).into(),
            warning: rgb(0xd97706).into(),
            highlight_bg: rgb(0xfff1a8).into(),
            code_bg: rgb(0xf0f0ee).into(),
            banner_bg: rgb(0xeaf2ff).into(),
            bullet: rgb(0x9aa0a6).into(),
            scrim: gpui::hsla(0., 0., 0., 0.18),
            pdf_highlight: gpui::hsla(0.14, 0.95, 0.55, 0.35),
            pdf_selection: gpui::hsla(0.6, 0.9, 0.55, 0.25),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1c1d1f).into(),
            sidebar_bg: rgb(0x232427).into(),
            surface: rgb(0x202124).into(),
            text: rgb(0xe6e6e3).into(),
            muted: rgb(0xa0a4a8).into(),
            faint: rgb(0x6b6f74).into(),
            line: rgb(0x34363a).into(),
            hover: rgb(0x2c2e31).into(),
            accent: rgb(0x7aa7ff).into(),
            tag: rgb(0xb79cf0).into(),
            selection: rgb(0x264f8f).into(),
            cursor: rgb(0x7aa7ff).into(),
            chip_bg: rgb(0x2a3140).into(),
            danger_bg: rgb(0x4a2222).into(),
            danger: rgb(0xff9c94).into(),
            warning: rgb(0xf0b45a).into(),
            highlight_bg: rgb(0x5c5020).into(),
            code_bg: rgb(0x2c2d30).into(),
            banner_bg: rgb(0x22314a).into(),
            bullet: rgb(0x80858b).into(),
            scrim: gpui::hsla(0., 0., 0., 0.42),
            pdf_highlight: gpui::hsla(0.14, 0.95, 0.55, 0.35),
            pdf_selection: gpui::hsla(0.6, 0.9, 0.55, 0.25),
        }
    }

    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }
}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

pub const TEXT_SIZE: f32 = 15.;
pub const LINE_HEIGHT: f32 = 24.;
pub const INDENT: f32 = 22.;
/// The UI caps visual indentation; deeper blocks show a depth badge (§6.1).
pub const MAX_VISUAL_DEPTH: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;

    /// ALL-2: the token set exists in both palettes, and light and dark are not the same
    /// colours with different names.
    #[test]
    fn every_token_is_named_in_both_palettes() {
        let l = Theme::light();
        let d = Theme::dark();
        for (a, b) in [
            (l.text, d.text),
            (l.muted, d.muted),
            (l.line, d.line),
            (l.surface, d.surface),
            (l.accent, d.accent),
            (l.tag, d.tag),
            (l.danger, d.danger),
            (l.warning, d.warning),
        ] {
            assert_ne!(a, b, "a dark theme has its own value for every token");
        }
        assert_ne!(l.danger, l.accent, "danger is not the link colour");
    }

}
