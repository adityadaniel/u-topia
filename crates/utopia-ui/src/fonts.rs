//! Fonts shipped inside the binary, so a font choice works offline and inside a signed
//! bundle with no resource lookup.
//!
//! IBM Plex Sans 1.1.0, from the official release (`@ibm/plex-sans@1.1.0` on
//! github.com/IBM/plex, `fonts/complete/otf`), under the SIL Open Font License 1.1
//! (`assets/fonts/ibm-plex-sans/OFL.txt`). Only the faces the UI asks for are embedded:
//! regular, italic (quotes, `_italic_`), medium (section labels), bold (headings, titles,
//! `**bold**`) and bold italic — about 700 KB.

use gpui::App;
use std::borrow::Cow;

/// The family name the bundled faces register under; what `font_family` must say.
pub const IBM_PLEX_SANS: &str = "IBM Plex Sans";

macro_rules! plex {
    ($face:literal) => {
        (
            $face,
            include_bytes!(concat!(
                "../assets/fonts/ibm-plex-sans/IBMPlexSans-",
                $face,
                ".otf"
            ))
            .as_slice(),
        )
    };
}

/// Face name and OpenType bytes of every bundled IBM Plex Sans face.
pub const IBM_PLEX_SANS_FACES: &[(&str, &[u8])] = &[
    plex!("Regular"),
    plex!("Italic"),
    plex!("Medium"),
    plex!("Bold"),
    plex!("BoldItalic"),
];

/// Hand the bundled IBM Plex Sans faces to gpui's text system. Done on first use rather
/// than at startup, so the default (System) costs nothing at launch. A font gpui cannot
/// load is logged, and text falls back to the system font (rule 11).
pub fn register_ibm_plex_sans(cx: &App) {
    let faces = IBM_PLEX_SANS_FACES
        .iter()
        .map(|(_, bytes)| Cow::Borrowed(*bytes))
        .collect();
    if let Err(e) = cx.text_system().add_fonts(faces) {
        eprintln!("utopia: could not load the bundled IBM Plex Sans: {e}");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zed_font_kit::font::Font;
    use zed_font_kit::properties::{Style, Weight};

    /// The embedded bytes are the fonts we think they are: each parses, belongs to the
    /// family the setting asks gpui for, and has the weight and slant its name says.
    #[test]
    fn bundled_faces_are_ibm_plex_sans() {
        for (face, bytes) in IBM_PLEX_SANS_FACES {
            let font = Font::from_bytes(Arc::new(bytes.to_vec()), 0)
                .unwrap_or_else(|e| panic!("{face} does not parse: {e:?}"));
            assert_eq!(font.family_name(), IBM_PLEX_SANS, "{face}");
            let p = font.properties();
            let weight = match *face {
                "Medium" => Weight::MEDIUM,
                f if f.starts_with("Bold") => Weight::BOLD,
                _ => Weight::NORMAL,
            };
            assert_eq!(p.weight, weight, "{face}");
            let italic = face.ends_with("Italic");
            assert_eq!(p.style == Style::Italic, italic, "{face}");
        }
    }
}
