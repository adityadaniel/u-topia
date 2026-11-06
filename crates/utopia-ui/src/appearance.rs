//! Appearance preferences (Settings → Appearance): which font the notes and the interface
//! are set in, how tightly blocks are set, and the shape of the right sidebar.
//!
//! They live in `<config dir>/settings.json`, next to `keymap.json` and resolved the same way
//! (`UTOPIA_CONFIG_DIR`). Window and UI preferences are local to the machine and never sync
//! (§12.5, architecture rule 9), so nothing here goes near a CRDT shard. The file is a small
//! versioned object meant to grow more general preferences:
//!
//! ```json
//! {
//!   "version": 1,
//!   "font": "system",
//!   "density": "comfortable",
//!   "right_sidebar_width": 380.0,
//!   "pinned_pages": []
//! }
//! ```
//!
//! `font` is `"system"` (the default) or `"ibm-plex-sans"`; `density` is
//! `"comfortable"` (the default) or `"compact"` (ALL-5); `right_sidebar_width` is the width
//! the right sidebar was dragged to (RS-4); `pinned_pages` lists the page ids kept in the
//! right sidebar's Pinned tab, most recent first (RS-3). A missing file means
//! defaults; an unreadable file, an unknown value or a newer version is logged and read as
//! far as it can be, never a panic (rule 11). Keys this version does not know are kept when
//! the file is rewritten, so a newer release's preferences survive a round trip through an
//! older one.

use gpui::{App, Global, SharedString};
use serde_json::{Map, Value};
use std::path::Path;

use crate::fonts::{register_ibm_plex_sans, IBM_PLEX_SANS};
use crate::theme::{BODY_SIZE, LINE_HEIGHT};

pub const FILE: &str = "settings.json";
pub const VERSION: u64 = 1;

/// The font the UI and notes are set in. Code blocks stay in Menlo either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiFont {
    /// The platform's UI font (SF Pro on macOS), as before this setting existed.
    #[default]
    System,
    /// IBM Plex Sans, bundled with the app (`crate::fonts`).
    IbmPlexSans,
}

impl UiFont {
    pub const ALL: &'static [UiFont] = &[UiFont::System, UiFont::IbmPlexSans];

    /// How `settings.json` spells it.
    pub fn key(self) -> &'static str {
        match self {
            UiFont::System => "system",
            UiFont::IbmPlexSans => "ibm-plex-sans",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            UiFont::System => "System",
            UiFont::IbmPlexSans => "IBM Plex Sans",
        }
    }

    /// The family gpui is asked for.
    pub fn family(self) -> &'static str {
        match self {
            UiFont::System => ".SystemUIFont",
            UiFont::IbmPlexSans => IBM_PLEX_SANS,
        }
    }
}

/// How tightly blocks are set (ALL-5). Local to the machine, like the font.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    /// The body at 15 px on 24 px lines, as before the setting existed.
    #[default]
    Comfortable,
    /// 13.5 px body, 18 px lines and 4 px between blocks.
    Compact,
}

impl Density {
    pub const ALL: &'static [Density] = &[Density::Comfortable, Density::Compact];

    pub fn key(self) -> &'static str {
        match self {
            Density::Comfortable => "comfortable",
            Density::Compact => "compact",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Density::Comfortable => "Comfortable",
            Density::Compact => "Compact",
        }
    }

    pub fn text_size(self) -> f32 {
        match self {
            Density::Comfortable => BODY_SIZE,
            Density::Compact => 13.5,
        }
    }

    pub fn line_height(self) -> f32 {
        match self {
            Density::Comfortable => LINE_HEIGHT,
            Density::Compact => 18.,
        }
    }

    /// Space between two blocks. Rows take half of it above and half below.
    pub fn block_spacing(self) -> f32 {
        match self {
            Density::Comfortable => 0.,
            Density::Compact => 4.,
        }
    }
}

/// The right sidebar's width as it was before the setting existed.
pub const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 380.;
/// Narrow enough to leave a readable page beside it, wide enough for a reference.
pub const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 240.;
pub const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 640.;

/// Keep a width — dragged, typed or read from a file — inside what the layout can show.
pub fn clamp_sidebar_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(RIGHT_SIDEBAR_MIN_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH)
    } else {
        RIGHT_SIDEBAR_DEFAULT_WIDTH
    }
}

/// What `settings.json` holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub font: UiFont,
    pub density: Density,
    /// How wide the right sidebar is, in points (RS-4). Clamped to
    /// [`RIGHT_SIDEBAR_MIN_WIDTH`]..=[`RIGHT_SIDEBAR_MAX_WIDTH`] when read and written.
    pub right_sidebar_width: f32,
    /// Pages pinned in the right sidebar's Pinned tab (RS-3), most recent first. Ids are
    /// stable across renames; one from another graph simply does not resolve.
    pub pinned_pages: Vec<String>,
    /// Keys this version does not understand, written back untouched.
    rest: Map<String, Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            font: UiFont::default(),
            density: Density::default(),
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            pinned_pages: Vec::new(),
            rest: Map::new(),
        }
    }
}

impl Prefs {
    pub fn from_json(json: &str) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut rest = match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                warnings.push(format!("{FILE}: expected an object; using the defaults"));
                return (Self::default(), warnings);
            }
            Err(e) => {
                warnings.push(format!("{FILE}: {e}; using the defaults"));
                return (Self::default(), warnings);
            }
        };
        match rest.remove("version") {
            Some(Value::Number(n)) if n.as_u64() == Some(VERSION) => {}
            None => {}
            Some(v) => warnings.push(format!(
                "{FILE}: version {v} is not {VERSION}; reading what this version understands"
            )),
        }
        let font = match rest.remove("font") {
            None => UiFont::default(),
            Some(Value::String(s)) => UiFont::from_key(&s).unwrap_or_else(|| {
                warnings.push(format!("{FILE}: unknown font {s:?}; using System"));
                UiFont::default()
            }),
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: font should be a string, not {v}; using System"
                ));
                UiFont::default()
            }
        };
        let density = match rest.remove("density") {
            None => Density::default(),
            Some(Value::String(s)) => Density::from_key(&s).unwrap_or_else(|| {
                warnings.push(format!("{FILE}: unknown density {s:?}; using Comfortable"));
                Density::default()
            }),
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: density should be a string, not {v}; using Comfortable"
                ));
                Density::default()
            }
        };
        let right_sidebar_width = match rest.remove("right_sidebar_width") {
            None => RIGHT_SIDEBAR_DEFAULT_WIDTH,
            Some(Value::Number(n)) => match n.as_f64() {
                Some(w) if w.is_finite() => clamp_sidebar_width(w as f32),
                _ => {
                    warnings.push(format!(
                        "{FILE}: right_sidebar_width {n} is not a width; using \
                         {RIGHT_SIDEBAR_DEFAULT_WIDTH}"
                    ));
                    RIGHT_SIDEBAR_DEFAULT_WIDTH
                }
            },
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: right_sidebar_width should be a number, not {v}; using \
                     {RIGHT_SIDEBAR_DEFAULT_WIDTH}"
                ));
                RIGHT_SIDEBAR_DEFAULT_WIDTH
            }
        };
        let pinned_pages = match rest.remove("pinned_pages") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    other => {
                        warnings.push(format!(
                            "{FILE}: pinned_pages holds {other}, which is not a page id; \
                             skipping it"
                        ));
                        None
                    }
                })
                .collect(),
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: pinned_pages should be a list, not {v}; starting empty"
                ));
                Vec::new()
            }
        };
        (
            Self {
                font,
                density,
                right_sidebar_width,
                pinned_pages,
                rest,
            },
            warnings,
        )
    }

    pub fn to_json(&self) -> String {
        let mut m = Map::new();
        m.insert("version".into(), VERSION.into());
        m.insert("font".into(), self.font.key().into());
        m.insert("density".into(), self.density.key().into());
        m.insert(
            "right_sidebar_width".into(),
            serde_json::Number::from_f64(clamp_sidebar_width(self.right_sidebar_width) as f64)
                .map_or(Value::Null, Value::Number),
        );
        m.insert(
            "pinned_pages".into(),
            Value::Array(
                self.pinned_pages
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        for (k, v) in &self.rest {
            m.insert(k.clone(), v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default() + "\n"
    }

    /// Defaults when there is no file; a warning when there is one that cannot be read.
    pub fn load(config_dir: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(config_dir.join(FILE)) {
            Ok(s) => Self::from_json(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (
                Self::default(),
                vec![format!("{FILE}: {e}; using the defaults")],
            ),
        }
    }

    /// Write atomically: a crash mid-write leaves the old file, never half a new one.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let tmp = config_dir.join(format!(".{FILE}.tmp"));
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, config_dir.join(FILE))
    }
}

/// The preferences in force, shared by every window.
#[derive(Default)]
pub struct Appearance {
    pub prefs: Prefs,
    plex_registered: bool,
    /// Where `settings.json` lives, so a view that owns a preference (the right sidebar's
    /// pinned pages, RS-3) can write it without being handed the path.
    config_dir: Option<std::path::PathBuf>,
}

impl Global for Appearance {}

/// Read `settings.json` and put it in force. Returns the warnings, already logged.
pub fn install(cx: &mut App, config_dir: &Path) -> Vec<String> {
    let (prefs, warnings) = Prefs::load(config_dir);
    for w in &warnings {
        eprintln!("utopia: {w}");
    }
    let font = prefs.font;
    cx.set_global(Appearance {
        prefs,
        plex_registered: false,
        config_dir: Some(config_dir.to_path_buf()),
    });
    apply(cx, font);
    warnings
}

/// The font in force. System when nothing was installed (tests, early startup).
pub fn ui_font(cx: &App) -> UiFont {
    cx.try_global::<Appearance>()
        .map_or(UiFont::default(), |a| a.prefs.font)
}

/// The family every window's root sets, so all text below it follows the setting.
pub fn font_family(cx: &App) -> SharedString {
    ui_font(cx).family().into()
}

/// Write the preferences in force to the installed `settings.json`. A view that changed one
/// calls this; a failed write is returned, not fatal (the choice still holds this session).
pub fn remember(cx: &mut App) -> std::io::Result<()> {
    cx.refresh_windows();
    let appearance = cx.default_global::<Appearance>();
    match appearance.config_dir.clone() {
        Some(dir) => appearance.prefs.save(&dir),
        // No settings file (tests, early startup): the preference still applies here.
        None => Ok(()),
    }
}

/// The right sidebar's width in force (RS-4). 380 when nothing was installed.
pub fn right_sidebar_width(cx: &App) -> f32 {
    cx.try_global::<Appearance>()
        .map_or(RIGHT_SIDEBAR_DEFAULT_WIDTH, |a| {
            clamp_sidebar_width(a.prefs.right_sidebar_width)
        })
}

/// Set the sidebar's width in memory while it is being dragged; the file follows on release
/// through [`remember`] (RS-4: one write per drag, not one per pixel).
pub fn set_right_sidebar_width(cx: &mut App, width: f32) {
    cx.default_global::<Appearance>().prefs.right_sidebar_width = clamp_sidebar_width(width);
    cx.refresh_windows();
}

/// Put the sidebar back to its default width and write that down (Settings → Appearance).
pub fn reset_right_sidebar_width(cx: &mut App, config_dir: &Path) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.right_sidebar_width = RIGHT_SIDEBAR_DEFAULT_WIDTH;
    cx.refresh_windows();
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// The pages pinned in the right sidebar's Pinned tab, most recent first (RS-3).
pub fn pinned_pages(cx: &App) -> Vec<String> {
    cx.try_global::<Appearance>()
        .map(|a| a.prefs.pinned_pages.clone())
        .unwrap_or_default()
}

/// Pin `page`, or unpin it when it is already pinned (RS-3). Local UI state, kept in
/// `settings.json` (rule 9): it never enters a CRDT shard and never syncs.
pub fn toggle_pinned_page(cx: &mut App, page: &str) {
    {
        let appearance = cx.default_global::<Appearance>();
        match appearance.prefs.pinned_pages.iter().position(|p| p == page) {
            Some(i) => {
                appearance.prefs.pinned_pages.remove(i);
            }
            None => appearance.prefs.pinned_pages.insert(0, page.to_string()),
        }
    }
    if let Err(e) = remember(cx) {
        eprintln!("utopia: {FILE}: {e}");
    }
}

/// The density in force. Comfortable when nothing was installed (tests, early startup).
pub fn density(cx: &App) -> Density {
    cx.try_global::<Appearance>()
        .map_or(Density::default(), |a| a.prefs.density)
}

/// Switch font in every open window now, and remember it in `config_dir/settings.json`.
/// A failed write is returned, not fatal: the font still applies for this session.
pub fn set_font(cx: &mut App, font: UiFont, config_dir: &Path) -> std::io::Result<()> {
    apply(cx, font);
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// Switch density in every open window now and remember it (rule 9: local UI state).
pub fn set_density(cx: &mut App, density: Density, config_dir: &Path) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.density = density;
    cx.refresh_windows();
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// Make `font` drawable: the bundled faces are handed to gpui the first time they are
/// needed (the setting, or Settings showing a sample of each choice), not at every launch.
pub fn load_font(cx: &mut App, font: UiFont) {
    let a = cx.default_global::<Appearance>();
    if font == UiFont::IbmPlexSans && !a.plex_registered {
        a.plex_registered = true;
        register_ibm_plex_sans(cx);
    }
}

fn apply(cx: &mut App, font: UiFont) {
    load_font(cx, font);
    cx.default_global::<Appearance>().prefs.font = font;
    // Views only re-render when notified; every window reads the font at its root.
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Prefs {
            font: UiFont::IbmPlexSans,
            density: Density::Compact,
            right_sidebar_width: 420.0,
            pinned_pages: vec!["page-1".into(), "page-2".into()],
            ..Default::default()
        };
        let json = p.to_json();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "version": 1,
                "font": "ibm-plex-sans",
                "density": "compact",
                "right_sidebar_width": 420.0,
                "pinned_pages": ["page-1", "page-2"],
            })
        );
        assert_eq!(Prefs::from_json(&json), (p, vec![]));
    }

    /// Rule 11: whatever is in the file, the app comes up, on System, and says why.
    #[test]
    fn bad_files_degrade_to_system() {
        for bad in [
            "",
            "{ not json",
            "[1, 2]",
            r#"{"version": 1, "font": "comic-sans"}"#,
            r#"{"version": 1, "font": 3}"#,
        ] {
            let (p, warnings) = Prefs::from_json(bad);
            assert_eq!(p.font, UiFont::System, "{bad:?}");
            assert_eq!(warnings.len(), 1, "{bad:?}: {warnings:?}");
        }
    }

    #[test]
    fn missing_keys_are_defaults_without_complaint() {
        assert_eq!(Prefs::from_json("{}"), (Prefs::default(), vec![]));
        assert_eq!(Prefs::default().density, Density::Comfortable);
    }

    /// RS-4: the sidebar width is clamped to what the layout can show, and junk is the
    /// default with a warning rather than a panic (rule 11).
    #[test]
    fn the_sidebar_width_is_clamped_and_never_fatal() {
        assert_eq!(clamp_sidebar_width(10.), RIGHT_SIDEBAR_MIN_WIDTH);
        assert_eq!(clamp_sidebar_width(10_000.), RIGHT_SIDEBAR_MAX_WIDTH);
        assert_eq!(clamp_sidebar_width(f32::NAN), RIGHT_SIDEBAR_DEFAULT_WIDTH);
        let (p, w) = Prefs::from_json(r#"{"version": 1, "right_sidebar_width": 480}"#);
        assert_eq!(p.right_sidebar_width, 480.);
        assert!(w.is_empty(), "{w:?}");
        for bad in [
            r#"{"version": 1, "right_sidebar_width": "wide"}"#,
            r#"{"version": 1, "right_sidebar_width": null}"#,
        ] {
            let (p, w) = Prefs::from_json(bad);
            assert_eq!(p.right_sidebar_width, RIGHT_SIDEBAR_DEFAULT_WIDTH, "{bad}");
            assert_eq!(w.len(), 1, "{bad}: {w:?}");
        }
        // A width outside the range is brought back into it on the way in and out.
        let (p, _) = Prefs::from_json(r#"{"version": 1, "right_sidebar_width": 4000}"#);
        assert_eq!(p.right_sidebar_width, RIGHT_SIDEBAR_MAX_WIDTH);
        assert_eq!(
            serde_json::from_str::<Value>(&p.to_json()).unwrap()["right_sidebar_width"],
            serde_json::json!(RIGHT_SIDEBAR_MAX_WIDTH as f64)
        );
    }

    /// RS-3: the pinned list is a list of page ids; anything else is skipped, not fatal.
    #[test]
    fn pinned_pages_survive_a_round_trip_and_reject_junk() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned_pages": ["a", 3, "b"]}"#);
        assert_eq!(p.pinned_pages, ["a", "b"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned_pages": "a"}"#);
        assert!(p.pinned_pages.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned_pages": ["x"]}"#);
        assert_eq!(p.pinned_pages, ["x"]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// An unknown density is the comfortable one, said out loud, never a panic (rule 11).
    #[test]
    fn an_unknown_density_falls_back_to_comfortable() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "density": "tiny"}"#);
        assert_eq!(p.density, Density::Comfortable);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("tiny"), "{warnings:?}");
    }

    /// A newer release's file: what we know is read, the rest is kept for it.
    #[test]
    fn newer_files_are_read_and_kept() {
        let (p, warnings) =
            Prefs::from_json(r#"{"version": 2, "font": "ibm-plex-sans", "zoom": 1.25}"#);
        assert_eq!(p.font, UiFont::IbmPlexSans);
        assert_eq!(warnings.len(), 1, "the version is mentioned: {warnings:?}");
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["zoom"], 1.25);
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn save_is_atomic_and_load_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Prefs::load(dir.path()), (Prefs::default(), vec![]));
        let p = Prefs {
            font: UiFont::IbmPlexSans,
            ..Default::default()
        };
        p.save(dir.path()).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [FILE], "no temp file left behind");
        assert_eq!(Prefs::load(dir.path()), (p, vec![]));
    }
}
