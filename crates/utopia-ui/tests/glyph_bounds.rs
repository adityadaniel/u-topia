//! A glyph's bitmap must hold the whole glyph (§15.4: text has to be legible).
//!
//! gpui sizes each glyph's bitmap with font-kit's `raster_bounds` and then draws the glyph
//! into it with CoreText at the real size (`rasterize_glyph`). Two things made the ink land
//! outside that bitmap, so it was cut off:
//!
//! - font-kit measured a 16 pt instance of the font and scaled it, but the system font
//!   (SF Pro) has an optical-size axis: from about 20 pt its glyphs sit further left than the
//!   scaled box, so headings and page titles lost their left edge;
//! - CoreText's anti-aliasing spills up to a pixel past the outline, and the bounds were the
//!   bare outline, so the soft bottom row of every bowl (b, o, d, p) was cut flat.
//!
//! This loads fonts the way gpui does, draws each glyph the way gpui does (including its
//! subpixel shifts and the extra row/column it adds for them), and checks the ink stays inside.
#![cfg(target_os = "macos")]

use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::CGPoint;
use pathfinder_geometry::transform2d::Transform2F;
use zed_font_kit::canvas::RasterizationOptions;
use zed_font_kit::font::Font;
use zed_font_kit::hinting::HintingOptions;
use zed_font_kit::properties::{Style, Weight};
use zed_font_kit::source::SystemSource;

/// CoreGraphics `kCGImageAlphaOnly` (gpui keeps its own copy too).
const ALPHA_ONLY: u32 = 7;
/// Body, code, H3, H2, H1, the settings title, rename field and the page title.
const SIZES: &[f32] = &[13., 15., 17., 20., 22., 24., 26., 28.];
const CHARS: &str = "bdgopqyGOQ";
/// gpui's `SUBPIXEL_VARIANTS_X` / `_Y` on macOS.
const VARIANTS: u8 = 4;

/// Left, top, right, bottom in device pixels, y down; right and bottom exclusive.
type Box = (i32, i32, i32, i32);

fn fits(outer: Box, inner: Box) -> bool {
    outer.0 <= inner.0 && outer.1 <= inner.1 && inner.2 <= outer.2 && inner.3 <= outer.3
}

/// Ink of one glyph drawn as gpui's `rasterize_glyph` draws it: at `size`, shifted right and
/// up by the subpixel variant. Device pixels relative to the glyph origin, y down.
fn ink(font: &Font, glyph: u32, size: f32, scale: f32, variant: (u8, u8)) -> Option<Box> {
    const W: usize = 256;
    const H: usize = 256;
    const OX: usize = 64; // origin, device px from the left
    const OY: usize = 96; // origin, device px from the bottom
    let mut bytes = vec![0u8; W * H];
    let cx = CGContext::create_bitmap_context(
        Some(bytes.as_mut_ptr() as *mut _),
        W,
        H,
        8,
        W,
        &CGColorSpace::create_device_gray(),
        ALPHA_ONLY,
    );
    cx.scale(scale as f64, scale as f64);
    cx.set_gray_fill_color(0.0, 1.0);
    cx.set_allows_antialiasing(true);
    cx.set_should_antialias(true);
    cx.set_allows_font_subpixel_positioning(true);
    cx.set_should_subpixel_position_fonts(true);
    cx.set_allows_font_subpixel_quantization(false);
    cx.set_should_subpixel_quantize_fonts(false);
    let shift = |v: u8| v as f64 / VARIANTS as f64;
    font.native_font()
        .clone_with_font_size(size as f64)
        .draw_glyphs(
            &[glyph as u16],
            &[CGPoint::new(
                (OX as f64 + shift(variant.0)) / scale as f64,
                (OY as f64 + shift(variant.1)) / scale as f64,
            )],
            cx.clone(),
        );
    drop(cx);
    let (mut l, mut t, mut r, mut b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for row in 0..H {
        for col in 0..W {
            if bytes[row * W + col] > 0 {
                let x = col as i32 - OX as i32;
                let y = row as i32 - (H - OY) as i32;
                l = l.min(x);
                t = t.min(y);
                r = r.max(x + 1);
                b = b.max(y + 1);
            }
        }
    }
    (l <= r).then_some((l, t, r, b))
}

/// Every glyph's ink, at every size, scale and subpixel variant, inside the bitmap gpui
/// allocates for it. Returns the misses, described.
fn misses(name: &str, font: &Font) -> Vec<String> {
    let mut out = Vec::new();
    for scale in [1., 2.] {
        for &size in SIZES {
            for ch in CHARS.chars() {
                let Some(g) = font.glyph_for_char(ch) else {
                    continue;
                };
                let bounds = font
                    .raster_bounds(
                        g,
                        size,
                        Transform2F::from_scale(scale),
                        HintingOptions::None,
                        RasterizationOptions::GrayscaleAa,
                    )
                    .unwrap();
                for vx in 0..VARIANTS {
                    for vy in 0..VARIANTS {
                        let Some(ink) = ink(font, g, size, scale, (vx, vy)) else {
                            continue;
                        };
                        // gpui adds a column on the right and a row on top for a shifted glyph.
                        let room = (
                            bounds.min_x(),
                            bounds.min_y() - (vy > 0) as i32,
                            bounds.max_x() + (vx > 0) as i32,
                            bounds.max_y(),
                        );
                        if !fits(room, ink) {
                            out.push(format!(
                                "{name} {size}pt @{scale}x '{ch}' variant ({vx},{vy}): ink {ink:?}, bitmap {room:?}"
                            ));
                        }
                    }
                }
            }
        }
    }
    out
}

/// The regular and bold faces gpui loads for ".SystemUIFont".
fn system_fonts() -> Vec<(String, Font)> {
    let family = SystemSource::new()
        .select_family_by_name(".AppleSystemUIFont")
        .expect("the system UI font is always present on macOS");
    let fonts: Vec<Font> = family
        .fonts()
        .iter()
        .filter_map(|h| h.load().ok())
        .collect();
    let mut out = Vec::new();
    for want in [Weight::NORMAL, Weight::BOLD] {
        if let Some(f) = fonts
            .iter()
            .find(|f| f.properties().weight == want && f.properties().style == Style::Normal)
        {
            out.push((f.postscript_name().unwrap_or_default(), f.clone()));
        }
    }
    assert_eq!(out.len(), 2, "regular and bold system font");
    out
}

#[test]
fn system_font_glyphs_fit_their_bitmaps() {
    let mut all = Vec::new();
    for (name, font) in system_fonts() {
        all.extend(misses(&name, &font));
    }
    assert!(
        all.is_empty(),
        "{} glyphs drawn outside their bitmap, e.g.:\n{}",
        all.len(),
        all[..all.len().min(20)].join("\n")
    );
}
