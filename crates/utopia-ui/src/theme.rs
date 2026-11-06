//! Colors and metrics. Light and dark follow the system appearance; high contrast is a
//! separate palette (§15.4).
//!
//! The token set every component reads (ALL-2): `text`, `muted`, `line`, `surface` (the
//! page's `bg`, a panel's `surface`), `accent`, `tag`, `danger` and `warning`. Adding a
//! theme means adding one `Theme` value, never a colour at a call site; a test keeps the
//! two palettes in step.

use gpui::{
    div, prelude::*, px, rgb, transparent_black, App, Div, ElementId, Global, Hsla, SharedString,
    Stateful, WindowAppearance,
};

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub sidebar_bg: Hsla,
    /// Cards, menus and panels that sit above `bg`.
    pub surface: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    /// The warm grey a quotation is set in (PG-5): grey with the page's paper temperature,
    /// not the cooler interface `muted`.
    pub quote: Hsla,
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
    /// A healthy state that is not a link: the in-sync dot (SB-4).
    pub success: Hsla,
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
            quote: rgb(0x6d6357).into(),
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
            success: rgb(0x1f7a3d).into(),
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
            quote: rgb(0xb3a795).into(),
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
            success: rgb(0x6fd08c).into(),
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

/// A quiet control (ALL-3): 6 px radius, a 13 px label and one hairline that appears only
/// on hover, so controls read as controls without shouting over the notes. Use
/// [`quiet_menu`] when a menu opens from it.
pub fn quiet_control(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(transparent_black())
        .text_size(px(crate::theme::CONTROL_SIZE))
        .text_color(th.muted)
        .cursor_pointer()
        .hover(|d| d.bg(th.hover).border_color(th.line).text_color(th.text))
        .child(label.into())
}

/// A destructive quiet control: "Delete page" in a menu, red at rest and on hover.
pub fn quiet_danger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(transparent_black())
        .text_size(px(crate::theme::CONTROL_SIZE))
        .text_color(th.danger)
        .cursor_pointer()
        .hover(|d| {
            d.bg(th.danger_bg)
                .border_color(th.line)
                .text_color(th.danger)
        })
        .child(label.into())
}

/// A control that opens a menu; the chevron says so.
pub fn quiet_menu(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    quiet_control(id, format!("{label} ▾"), th)
}

/// Label size for controls, between body and caption.
pub const CONTROL_SIZE: f32 = 13.;
/// The one monospace family: code blocks (PG-5), whatever font the notes are set in.
pub const MONO: &str = "Menlo";

/// The small count that follows a name (LR-2): a quiet chip right after the page or day
/// name, not a number pushed to the far edge of the row.
pub fn count_chip(count: usize, th: &Theme) -> Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .bg(th.chip_bg)
        .text_size(px(CAPTION_SIZE))
        .text_color(th.muted)
        .child(count.to_string())
}

/// The one type scale (ALL-1): titles, outline headings, body and captions. Page titles
/// and day headings share `TITLE_SIZE`; an outline heading never draws larger than H1.
pub const TITLE_SIZE: f32 = 28.;
pub const H1_SIZE: f32 = 20.;
pub const H2_SIZE: f32 = 17.;
pub const H3_SIZE: f32 = 15.5;
pub const BODY_SIZE: f32 = 15.;
pub const CAPTION_SIZE: f32 = 12.5;
/// Body text and leading at the default (Comfortable) density. `Density` in
/// [`crate::appearance`] picks these or the compact pair.
pub const TEXT_SIZE: f32 = BODY_SIZE;
pub const LINE_HEIGHT: f32 = 24.;
pub const INDENT: f32 = 22.;
/// The UI caps visual indentation; deeper blocks show a depth badge (§6.1).
pub const MAX_VISUAL_DEPTH: usize = 12;

/// The point size and line box of an outline heading (PG-3). H1 never passes 20 px, and the
/// 28 px title stays above all three, so a heading in the outline never competes with it.
pub fn heading_metrics(level: u8) -> (f32, f32) {
    match level {
        1 => (H1_SIZE, 28.),
        2 => (H2_SIZE, 24.),
        _ => (H3_SIZE, 22.),
    }
}

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
            (l.quote, d.quote),
            (l.line, d.line),
            (l.surface, d.surface),
            (l.accent, d.accent),
            (l.tag, d.tag),
            (l.danger, d.danger),
            (l.warning, d.warning),
            (l.success, d.success),
        ] {
            assert_ne!(a, b, "a dark theme has its own value for every token");
        }
        assert_ne!(l.danger, l.accent, "danger is not the link colour");
    }

    /// ALL-1: the scale is ordered and an outline heading never passes H1.
    #[test]
    fn type_scale_is_ordered() {
        let scale = [
            TITLE_SIZE,
            H1_SIZE,
            H2_SIZE,
            H3_SIZE,
            BODY_SIZE,
            CAPTION_SIZE,
        ];
        assert!(
            scale.windows(2).all(|w| w[0] > w[1]),
            "the scale descends: {scale:?}"
        );
        let (text, body) = (TEXT_SIZE, BODY_SIZE);
        assert!(text == body, "the body size is the default text size");
    }

    /// PG-3: the three outline steps, each below the 28 px title and above the body, on a
    /// line box that shrinks with them.
    #[test]
    fn outline_headings_step_down_from_the_title() {
        assert_eq!((H1_SIZE, H2_SIZE, H3_SIZE), (20., 17., 15.5));
        let mut last = TITLE_SIZE;
        for level in 1..=3 {
            let (size, line) = heading_metrics(level);
            assert!(size < last && size > BODY_SIZE, "level {level}: {size}");
            assert!(line >= size, "the line box holds level {level}: {line}");
            last = size;
        }
        assert!(
            heading_metrics(1).1 > heading_metrics(2).1
                && heading_metrics(2).1 > heading_metrics(3).1,
            "each heading takes less vertical room than the one above it"
        );
    }
}
