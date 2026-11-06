//! Appearance preferences (Settings → Appearance): which font the notes and the interface
//! are set in.
//!
//! They live in `<config dir>/settings.json`, next to `keymap.json` and resolved the same way
//! (`UTOPIA_CONFIG_DIR`). Window and UI preferences are local to the machine and never sync
//! (§12.5, architecture rule 9), so nothing here goes near a CRDT shard. The file is a small
//! versioned object meant to grow more general preferences:
//!
//! ```json
//! { "version": 1, "font": "system" }
//! ```
//!
//! `font` is `"system"` (the default) or `"ibm-plex-sans"`. A missing file means defaults; an
//! unreadable file, an unknown value or a newer version is logged and read as far as it can
//! be, never a panic (rule 11). Keys this version does not know are kept when the file is
//! rewritten, so a newer release's preferences survive a round trip through an older one.

use gpui::{App, Global, SharedString};
use serde_json::{Map, Value};
use std::path::Path;

use crate::fonts::{register_ibm_plex_sans, IBM_PLEX_SANS};

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

/// What `settings.json` holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub font: UiFont,
    /// Keys this version does not understand, written back untouched.
    rest: Map<String, Value>,
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
        (Self { font, rest }, warnings)
    }

    pub fn to_json(&self) -> String {
        let mut m = Map::new();
        m.insert("version".into(), VERSION.into());
        m.insert("font".into(), self.font.key().into());
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

/// Switch font in every open window now, and remember it in `config_dir/settings.json`.
/// A failed write is returned, not fatal: the font still applies for this session.
pub fn set_font(cx: &mut App, font: UiFont, config_dir: &Path) -> std::io::Result<()> {
    apply(cx, font);
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
            ..Default::default()
        };
        let json = p.to_json();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"version": 1, "font": "ibm-plex-sans"})
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
