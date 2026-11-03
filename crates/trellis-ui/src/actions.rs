//! Actions and the keymap (§9.1). Defaults are macOS bindings; `cmd` becomes `ctrl`
//! elsewhere. A `keymap.json` in the config directory adds or overrides bindings;
//! unknown actions are reported and ignored, never fatal.

use gpui::{actions, App, KeyBinding};
use serde::Deserialize;

actions!(
    trellis,
    [
        Quit,
        QuickJump,
        GoToday,
        PrevDay,
        NextDay,
        ToggleRightSidebar,
        ToggleReferences,
        Undo,
        Redo,
    ]
);

actions!(
    editing,
    [
        Newline,
        SoftBreak,
        InsertAbove,
        Indent,
        Outdent,
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        WordLeft,
        WordRight,
        LineStart,
        LineEnd,
        MoveBlockUp,
        MoveBlockDown,
        ToggleCollapse,
        ToggleCollapseAll,
        ZoomIn,
        ZoomOut,
        Bold,
        Italic,
        Code,
        Copy,
        Cut,
        Paste,
        CopyBlockRef,
        DeleteBlock,
        Escape,
        ShowCharacterPalette,
        ToggleTodo,
    ]
);

actions!(
    trellis,
    [FullSearch, QuickCapture, ShowGraph, ToggleLeftSidebar]
);

pub const EDITOR_CONTEXT: &str = "BlockEditor";
pub const INPUT_CONTEXT: &str = "LineInput";

const DEFAULTS: &[(&str, &str, Option<&str>)] = &[
    ("cmd-q", "Quit", None),
    ("cmd-k", "QuickJump", None),
    ("cmd-shift-d", "GoToday", None),
    ("cmd-[", "PrevDay", None),
    ("cmd-]", "NextDay", None),
    ("cmd-shift-\\", "ToggleRightSidebar", None),
    ("cmd-\\", "ToggleLeftSidebar", None),
    ("cmd-shift-b", "ToggleReferences", None),
    ("cmd-z", "Undo", None),
    ("cmd-shift-z", "Redo", None),
    ("cmd-shift-f", "FullSearch", None),
    ("cmd-shift-space", "QuickCapture", None),
    ("cmd-shift-g", "ShowGraph", None),
    ("alt-enter", "ToggleTodo", Some("BlockEditor")),
    // Shared by the block editor and single-line inputs.
    ("backspace", "Backspace", Some("BlockEditor || LineInput")),
    ("delete", "Delete", Some("BlockEditor || LineInput")),
    ("left", "Left", Some("BlockEditor || LineInput")),
    ("right", "Right", Some("BlockEditor || LineInput")),
    ("up", "Up", Some("BlockEditor || LineInput")),
    ("down", "Down", Some("BlockEditor || LineInput")),
    ("shift-left", "SelectLeft", Some("BlockEditor || LineInput")),
    (
        "shift-right",
        "SelectRight",
        Some("BlockEditor || LineInput"),
    ),
    ("shift-up", "SelectUp", Some("BlockEditor")),
    ("shift-down", "SelectDown", Some("BlockEditor")),
    ("cmd-a", "SelectAll", Some("BlockEditor || LineInput")),
    ("home", "Home", Some("BlockEditor || LineInput")),
    ("end", "End", Some("BlockEditor || LineInput")),
    ("alt-left", "WordLeft", Some("BlockEditor || LineInput")),
    ("alt-right", "WordRight", Some("BlockEditor || LineInput")),
    ("cmd-left", "LineStart", Some("BlockEditor || LineInput")),
    ("cmd-right", "LineEnd", Some("BlockEditor || LineInput")),
    ("cmd-c", "Copy", Some("BlockEditor || LineInput")),
    ("cmd-x", "Cut", Some("BlockEditor || LineInput")),
    ("cmd-v", "Paste", Some("BlockEditor || LineInput")),
    ("escape", "Escape", Some("BlockEditor || LineInput")),
    ("enter", "Newline", Some("BlockEditor || LineInput")),
    ("cmd-enter", "InsertAbove", Some("BlockEditor || LineInput")),
    (
        "ctrl-cmd-space",
        "ShowCharacterPalette",
        Some("BlockEditor || LineInput"),
    ),
    // Outliner (§9.1).
    ("shift-enter", "SoftBreak", Some("BlockEditor || LineInput")),
    ("tab", "Indent", Some("BlockEditor")),
    ("shift-tab", "Outdent", Some("BlockEditor")),
    ("cmd-shift-up", "MoveBlockUp", Some("BlockEditor")),
    ("cmd-shift-down", "MoveBlockDown", Some("BlockEditor")),
    ("cmd-.", "ToggleCollapse", Some("BlockEditor")),
    ("cmd-alt-.", "ToggleCollapseAll", Some("BlockEditor")),
    ("cmd-alt-down", "ZoomIn", Some("BlockEditor")),
    ("cmd-alt-up", "ZoomOut", Some("BlockEditor")),
    ("cmd-b", "Bold", Some("BlockEditor")),
    ("cmd-i", "Italic", Some("BlockEditor")),
    ("cmd-e", "Code", Some("BlockEditor")),
    ("cmd-shift-c", "CopyBlockRef", Some("BlockEditor")),
    ("cmd-shift-backspace", "DeleteBlock", Some("BlockEditor")),
];

fn platform_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.to_string()
    } else {
        keys.replace("cmd-", "ctrl-")
    }
}

macro_rules! binding_for {
    ($name:expr, $keys:expr, $ctx:expr, [$($action:ident),* $(,)?]) => {
        match $name {
            $(stringify!($action) => Some(KeyBinding::new($keys, $action, $ctx)),)*
            _ => None,
        }
    };
}

/// Build a binding from an action name; `None` for unknown names.
pub fn binding(name: &str, keys: &str, context: Option<&str>) -> Option<KeyBinding> {
    // Validate keystrokes first: KeyBinding::new panics on invalid input.
    if keys
        .split_whitespace()
        .any(|k| gpui::Keystroke::parse(k).is_err())
    {
        return None;
    }
    let keys = keys.to_string();
    let keys = keys.as_str();
    binding_for!(
        name,
        keys,
        context,
        [
            Quit,
            QuickJump,
            GoToday,
            PrevDay,
            NextDay,
            ToggleRightSidebar,
            ToggleReferences,
            Undo,
            Redo,
            FullSearch,
            QuickCapture,
            ShowGraph,
            ToggleLeftSidebar,
            ToggleTodo,
            Newline,
            SoftBreak,
            InsertAbove,
            Indent,
            Outdent,
            Backspace,
            Delete,
            Left,
            Right,
            Up,
            Down,
            SelectLeft,
            SelectRight,
            SelectUp,
            SelectDown,
            SelectAll,
            Home,
            End,
            WordLeft,
            WordRight,
            LineStart,
            LineEnd,
            MoveBlockUp,
            MoveBlockDown,
            ToggleCollapse,
            ToggleCollapseAll,
            ZoomIn,
            ZoomOut,
            Bold,
            Italic,
            Code,
            Copy,
            Cut,
            Paste,
            CopyBlockRef,
            DeleteBlock,
            Escape,
            ShowCharacterPalette,
        ]
    )
}

#[derive(Debug, Deserialize)]
pub struct KeymapEntry {
    pub key: String,
    pub action: String,
    #[serde(default)]
    pub context: Option<String>,
}

/// Parse `keymap.json` (a list of `{ "key", "action", "context"? }`), returning the valid
/// bindings and a warning for each entry that could not be used.
pub fn parse_keymap(json: &str) -> (Vec<KeyBinding>, Vec<String>) {
    let entries: Vec<KeymapEntry> = match serde_json::from_str(json) {
        Ok(e) => e,
        Err(e) => return (Vec::new(), vec![format!("keymap.json: {e}")]),
    };
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for e in entries {
        match binding(&e.action, &e.key, e.context.as_deref()) {
            Some(b) => out.push(b),
            None => warnings.push(format!(
                "keymap.json: ignoring unknown action or keys: {} -> {}",
                e.key, e.action
            )),
        }
    }
    (out, warnings)
}

/// Install default bindings, then the user's `keymap.json` (if any) on top.
pub fn install_keymap(cx: &mut App, user_keymap: Option<&str>) {
    let defaults: Vec<KeyBinding> = DEFAULTS
        .iter()
        .filter_map(|(keys, name, ctx)| binding(name, &platform_keys(keys), *ctx))
        .collect();
    cx.bind_keys(defaults);
    if let Some(json) = user_keymap {
        let (bindings, warnings) = parse_keymap(json);
        for w in warnings {
            eprintln!("trellis: {w}");
        }
        cx.bind_keys(bindings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_binding_resolves() {
        for (keys, name, ctx) in DEFAULTS {
            assert!(
                binding(name, &platform_keys(keys), *ctx).is_some(),
                "{name} {keys}"
            );
        }
    }

    #[test]
    fn unknown_actions_are_reported_not_fatal() {
        let (b, w) = parse_keymap(
            r#"[{"key":"cmd-j","action":"QuickJump"},{"key":"cmd-y","action":"Nope"}]"#,
        );
        assert_eq!(b.len(), 1);
        assert_eq!(w.len(), 1);
        let (b, w) = parse_keymap("not json");
        assert!(b.is_empty() && w.len() == 1);
    }
}
