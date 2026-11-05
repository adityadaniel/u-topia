//! Colors and metrics. Light and dark follow the system appearance; high contrast is a
//! separate palette (§15.4).

use gpui::{rgb, App, Global, Hsla, WindowAppearance};

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    pub panel_bg: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub border: Hsla,
    pub hover: Hsla,
    pub accent: Hsla,
    pub tag: Hsla,
    pub selection: Hsla,
    pub cursor: Hsla,
    pub chip_bg: Hsla,
    pub missing_bg: Hsla,
    pub missing_text: Hsla,
    pub highlight_bg: Hsla,
    pub code_bg: Hsla,
    pub banner_bg: Hsla,
    pub bullet: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg: rgb(0xffffff).into(),
            sidebar_bg: rgb(0xf6f6f4).into(),
            panel_bg: rgb(0xfafaf9).into(),
            text: rgb(0x1f2328).into(),
            muted: rgb(0x6b7076).into(),
            faint: rgb(0xa8acb1).into(),
            border: rgb(0xe4e4e1).into(),
            hover: rgb(0xeeeeec).into(),
            accent: rgb(0x2f6fdb).into(),
            tag: rgb(0x7a52c7).into(),
            selection: rgb(0xb7d2fb).into(),
            cursor: rgb(0x2f6fdb).into(),
            chip_bg: rgb(0xeef2f8).into(),
            missing_bg: rgb(0xfbe4e4).into(),
            missing_text: rgb(0xb42318).into(),
            highlight_bg: rgb(0xfff1a8).into(),
            code_bg: rgb(0xf0f0ee).into(),
            banner_bg: rgb(0xeaf2ff).into(),
            bullet: rgb(0x9aa0a6).into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1c1d1f).into(),
            sidebar_bg: rgb(0x232427).into(),
            panel_bg: rgb(0x202124).into(),
            text: rgb(0xe6e6e3).into(),
            muted: rgb(0xa0a4a8).into(),
            faint: rgb(0x6b6f74).into(),
            border: rgb(0x34363a).into(),
            hover: rgb(0x2c2e31).into(),
            accent: rgb(0x7aa7ff).into(),
            tag: rgb(0xb79cf0).into(),
            selection: rgb(0x264f8f).into(),
            cursor: rgb(0x7aa7ff).into(),
            chip_bg: rgb(0x2a3140).into(),
            missing_bg: rgb(0x4a2222).into(),
            missing_text: rgb(0xff9c94).into(),
            highlight_bg: rgb(0x5c5020).into(),
            code_bg: rgb(0x2c2d30).into(),
            banner_bg: rgb(0x22314a).into(),
            bullet: rgb(0x80858b).into(),
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
