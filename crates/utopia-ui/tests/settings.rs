//! The Settings keybinding editor (§9.1), driven through GPUI's headless test platform.

use gpui::TestAppContext;
use utopia_ui::actions::{doc, install_keymap, platform_keys, Overrides, Quit};
use utopia_ui::settings::{SettingsView, Tab};
use utopia_ui::theme::Theme;

fn keymap_json(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("keymap.json")).ok()
}

/// Rebinding writes `keymap.json`, and only for the action that changed.
#[gpui::test]
fn rebinding_is_applied_and_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    assert!(keymap_json(dir.path()).is_none(), "nothing saved yet");
    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    assert_eq!(
        view.update(cx, |v, _| v.capturing().map(|d| d.action)),
        Some("GoToday")
    );

    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();

    view.update(cx, |v, _| {
        assert!(v.capturing().is_none(), "capture ends on the first key");
        assert_eq!(
            v.overrides().keys_for(today),
            "alt-cmd-t",
            "gpui canonical order"
        );
        // Untouched actions are not written down, so later default changes still reach you.
        assert_eq!(v.overrides().0.len(), 1);
    });
    let saved = keymap_json(dir.path()).expect("keymap.json written");
    assert!(
        saved.contains("GoToday") && saved.contains("alt-cmd-t"),
        "{saved}"
    );
    assert!(
        !saved.contains("GoYesterday"),
        "only the changed action: {saved}"
    );

    // And it survives a reload.
    let (back, warnings) = Overrides::from_json(&saved);
    assert!(warnings.is_empty());
    assert_eq!(back.keys_for(today), "alt-cmd-t");
}

/// Escape leaves the binding alone rather than binding Escape to it.
#[gpui::test]
fn escape_cancels_capture(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    view.update(cx, |v, _| {
        assert!(v.capturing().is_none());
        assert!(v.overrides().0.is_empty(), "nothing was rebound");
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
    assert!(keymap_json(dir.path()).is_none(), "nothing to save");
}

/// The keys being recorded must not also fire their own action. Rebinding onto Cmd+Q would
/// otherwise quit the app in the middle of the edit.
#[gpui::test]
fn capturing_swallows_the_keystroke(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let quit = std::rc::Rc::new(std::cell::Cell::new(false));
    let q = quit.clone();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        cx.on_action(move |_: &Quit, _| q.set(true));
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    // Cmd+Q really is bound to Quit here: it fires when we are not capturing.
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();
    assert!(quit.get(), "Cmd+Q quits when nothing is being captured");
    quit.set(false);

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();

    assert!(
        !quit.get(),
        "the recorded keystroke must not also run its action"
    );
    view.update(cx, |v, _| {
        assert_eq!(v.overrides().keys_for(today), "cmd-q", "Cmd+Q was recorded");
    });
}

/// Rebinding onto keys another action already uses is allowed, but said out loud.
#[gpui::test]
fn a_clash_is_reported(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes(&platform_keys("cmd-shift-y"));
    cx.run_until_parked();

    view.update(cx, |v, _| {
        let status = v.status().unwrap_or_default();
        assert!(status.contains("Yesterday"), "clash not reported: {status}");
        assert_eq!(v.overrides().conflicts().len(), 1);
    });
}

/// Reset puts an action back and removes it from the file.
#[gpui::test]
fn reset_restores_the_default(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();
    assert!(keymap_json(dir.path()).unwrap().contains("alt-cmd-t"));

    view.update(cx, |v, cx| v.reset_one(today, cx));
    view.update(cx, |v, _| {
        assert!(v.overrides().0.is_empty());
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
    assert_eq!(keymap_json(dir.path()).unwrap().trim(), "[]");
}

/// A `keymap.json` that cannot be read must not take the window down with it (rule 11).
#[gpui::test]
fn a_broken_keymap_is_reported_not_fatal(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keymap.json"), "{ not json").unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.run_until_parked();
    view.update(cx, |v, _| {
        assert!(v.overrides().0.is_empty());
        assert!(
            v.status().is_some(),
            "the user is told the file was unreadable"
        );
    });
}
