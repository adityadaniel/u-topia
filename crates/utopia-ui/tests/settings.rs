//! The Settings keybinding editor (§9.1), driven through GPUI's headless test platform.

use gpui::{px, size, TestAppContext};
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

/// The Settings body is a scroll region, not a growing column: in a short window the key
/// rows scroll inside it and the status line stays on screen.
#[gpui::test]
fn keybindings_scroll_inside_a_short_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keymap.json"), "{ not json").unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(720.), px(480.)));
    cx.run_until_parked();
    let status = cx
        .debug_bounds("settings-status")
        .expect("the unreadable keymap status is shown");
    assert!(
        status.origin.y + status.size.height <= px(480.) + px(0.5),
        "the status line stays inside the window: {status:?}"
    );
}

/// ST-1: a pane is one centred 720 px column, one card per category, with the rows of a
/// card touching so the hairline between them is the only separation.
#[gpui::test]
fn keybindings_are_a_readable_column_of_cards(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let nav = cx
        .debug_bounds("settings-card-Navigation")
        .expect("the Navigation card is drawn");
    let width = f32::from(nav.size.width);
    assert!((width - 720.).abs() < 1.5, "the column is 720 px: {nav:?}");
    // Centred in the pane beside the tab rail, not in the whole window (ST-5).
    let pane = cx.debug_bounds("settings-pane").unwrap();
    let left = f32::from(nav.origin.x) - f32::from(pane.origin.x);
    let right = f32::from(pane.size.width) - left - width;
    assert!(
        (left - right).abs() < 2.,
        "the column is centred in the pane: {left} left, {right} right"
    );
    let app = cx
        .debug_bounds("settings-card-Application")
        .expect("the Application card is drawn");
    assert!(
        (f32::from(app.origin.x) - f32::from(nav.origin.x)).abs() < 1.5
            && (f32::from(app.size.width) - width).abs() < 1.5,
        "every category card shares the column: {app:?}"
    );

    // Rows are stacked flush; the card's hairline separates them (no gap).
    let today = cx.debug_bounds("settings-row-GoToday").unwrap();
    let yesterday = cx.debug_bounds("settings-row-GoYesterday").unwrap();
    assert!(
        (f32::from(yesterday.origin.y) - f32::from(today.origin.y + today.size.height)).abs() < 1.,
        "rows touch: {today:?} then {yesterday:?}"
    );
}

/// ST-5: the tabs are a list down the left, each with an icon, and picking one swaps the
/// pane without moving the rail.
#[gpui::test]
fn side_tabs_switch_panes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    for key in ["appearance", "keybindings", "shortcuts", "sync", "help"] {
        assert!(
            cx.debug_bounds(match key {
                "appearance" => "settings-nav-appearance",
                "keybindings" => "settings-nav-keybindings",
                "shortcuts" => "settings-nav-shortcuts",
                "sync" => "settings-nav-sync",
                _ => "settings-nav-help",
            })
            .is_some(),
            "the {key} tab is in the list"
        );
    }
    // The rail fills the left edge and the pane starts where it ends.
    let first = cx.debug_bounds("settings-nav-appearance").unwrap();
    assert!(f32::from(first.origin.x) < 30., "{first:?}");
    let pane = cx.debug_bounds("settings-pane").unwrap();
    assert!(f32::from(pane.origin.x) < 221., "{pane:?}");
    assert!(
        f32::from(pane.size.width) > 800.,
        "the pane takes the rest of the window: {pane:?}"
    );
    // The top tab row is gone (never drawn, so a bounds check is sound).
    assert!(cx.debug_bounds("settings-tab-Keybindings").is_none());

    let appearance = cx.debug_bounds("settings-nav-appearance").unwrap();
    cx.simulate_click(appearance.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.tab()), Tab::Appearance);
    assert!(
        cx.debug_bounds("settings-card-Font").is_some(),
        "the Appearance pane is drawn after the click"
    );

    let sync = cx.debug_bounds("settings-nav-sync").unwrap();
    cx.simulate_click(sync.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.tab()), Tab::Sync);
    assert!(
        cx.debug_bounds("settings-sync").is_some(),
        "the Sync pane is drawn after the click"
    );
}

/// ST-5: the Sync tab reports the transport in force and never a secret. The facts are
/// built from the config the app resolved at launch, which holds the token and passphrase.
#[gpui::test]
fn the_sync_tab_shows_the_transport(cx: &mut TestAppContext) {
    use utopia_ui::sync_runner::{SyncConfig, SyncFacts, SyncTarget};
    let cfg = SyncConfig {
        target: SyncTarget::Relay {
            url: "https://relay.example".into(),
            token: Some("s3cret-token".into()),
        },
        graph: "graph-123".into(),
        passphrase: Some("hunter2".into()),
    };
    let facts = SyncFacts::from_config(&cfg);
    // The token and the passphrase are never carried into the view.
    let shown = format!("{facts:?}");
    assert!(facts.encrypted);
    assert!(shown.contains("https://relay.example"), "{shown}");
    assert!(!shown.contains("s3cret-token") && !shown.contains("hunter2"));

    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        cx.set_global(facts.clone());
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Sync, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-sync").is_some());
    assert!(
        cx.debug_bounds("settings-sync-transport").is_some(),
        "the transport has a row of its own"
    );
}

/// ST-5: Help is one readable column now that the topics sit above it instead of in a
/// second sidebar; the 640 px screenshots fit in the 720 px column.
#[gpui::test]
fn help_screenshots_fit_the_column(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Help, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    let pane = cx.debug_bounds("settings-pane").unwrap();
    let shot = cx
        .debug_bounds("help-shot")
        .expect("the topic has a screenshot");
    assert!((f32::from(shot.size.width) - 640.).abs() < 1.5, "{shot:?}");
    assert!(
        f32::from(shot.origin.x) >= f32::from(pane.origin.x)
            && f32::from(shot.origin.x + shot.size.width)
                <= f32::from(pane.origin.x + pane.size.width),
        "the screenshot is inside the pane: {shot:?} in {pane:?}"
    );
}

/// ST-4: the field at the top narrows the rows by name, description or key, and no match
/// says so instead of showing an empty pane.
#[gpui::test]
fn search_filters_by_name_description_and_key(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    let all = view.read_with(cx, |v, _| v.visible_actions().len());
    assert_eq!(all, utopia_ui::actions::CATALOG.len());

    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("zoom");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.query().to_string()), "zoom");
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["ZoomIn", "ZoomOut"],
        "by name"
    );

    // By keys, in the spelling the catalog stores them in.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()).len(),
        all,
        "escape clears the filter"
    );
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("cmd-shift-y");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["GoYesterday"],
        "by key"
    );

    // By description ("cheat sheet" is in ShowShortcuts' help text).
    cx.simulate_keystrokes("escape");
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("cheat");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["ShowShortcuts"],
        "by description"
    );

    // Nothing matches: the pane says so.
    cx.simulate_keystrokes("escape");
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("zzzz");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.visible_actions().is_empty()));
    assert!(
        cx.debug_bounds("settings-no-match").is_some(),
        "the empty state is drawn"
    );
}

/// ST-2: the key chip is the control. Clicking it records keys, a changed row shows the
/// small ↺, and the Change button is gone.
#[gpui::test]
fn the_key_chip_records_and_resets(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("settings-change-GoToday").is_none(),
        "no separate Change button (never drawn, so a bounds check is sound)"
    );
    assert!(
        cx.debug_bounds("settings-reset-GoToday").is_none(),
        "the ↺ only appears once the row changed"
    );

    let chip = cx
        .debug_bounds("settings-chip-GoToday")
        .expect("the key chip is the control");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let today = doc("GoToday").unwrap();
    assert_eq!(
        view.read_with(cx, |v, _| v.capturing().map(|d| d.action)),
        Some("GoToday"),
        "clicking the chip starts recording"
    );

    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.capturing().is_none());
        assert_eq!(v.overrides().keys_for(today), "alt-cmd-t");
    });
    assert!(
        cx.debug_bounds("settings-reset-GoToday").is_some(),
        "the changed row carries the ↺"
    );

    // Clicking the ↺ puts the catalog default back.
    let reset = cx.debug_bounds("settings-reset-GoToday").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.overrides().0.is_empty());
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
}

/// ST-3: a clash is an amber line under each of the two rows, and its link jumps to the
/// other row instead of only naming it.
#[gpui::test]
fn a_clash_is_shown_inline_and_links_to_the_other_row(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes(&platform_keys("cmd-shift-y"));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.overrides().conflicts().len()),
        1
    );

    let chip = cx
        .debug_bounds("settings-chip-GoToday")
        .expect("the rebound row's key chip");
    let line = cx
        .debug_bounds("settings-clash-GoToday")
        .expect("the clash line under the row");
    assert!(
        f32::from(line.origin.y) >= f32::from(chip.origin.y + chip.size.height) - 1.,
        "the line sits under the row's own line: {chip:?} then {line:?}"
    );
    // Both rows say who else uses the keys, so whichever one you are looking at, it shows.
    assert!(
        cx.debug_bounds("settings-clash-GoYesterday").is_some(),
        "the other row carries the line too"
    );

    let link = cx
        .debug_bounds("settings-clash-link-GoYesterday")
        .expect("the other action is a link");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.flashed()),
        Some("GoYesterday"),
        "the link jumps to the other row"
    );
}

/// ST-3: a clash across categories jumps the list to the other card, so "Also used by
/// Quit" actually takes you to Quit rather than naming it out of reach.
#[gpui::test]
fn a_clash_link_scrolls_to_the_other_card(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut o = Overrides::default();
    o.set("GoToday", "cmd-q");
    std::fs::write(dir.path().join("keymap.json"), o.to_json()).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.key_list_offset()), 0.);

    let link = cx
        .debug_bounds("settings-clash-link-Quit")
        .expect("Quit is linked under Today");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.flashed()), Some("Quit"));
    assert!(
        view.read_with(cx, |v, _| v.key_list_offset()) < -100.,
        "the list scrolled down to the Application card"
    );
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

/// A rebound action answers to its new keys only: the default it replaced is not left
/// installed alongside, invisible in Settings but still firing.
#[gpui::test]
fn rebinding_replaces_the_default_keys(cx: &mut TestAppContext) {
    use utopia_ui::actions::{install, GoToday};
    let mut o = Overrides::default();
    o.set("GoToday", "alt-cmd-t");
    cx.update(|cx| install(cx, &o));
    let keys: Vec<String> = cx.update(|cx| {
        cx.key_bindings()
            .borrow()
            .bindings_for_action(&GoToday)
            .map(|b| {
                b.keystrokes()
                    .iter()
                    .map(|k| k.unparse())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    });
    assert_eq!(keys, ["alt-cmd-t"]);
}

/// Review fix: an override whose keys cannot be used (empty, or not a keystroke) is
/// ignored with a warning, and the action keeps its default rather than ending up with
/// no binding at all (rule 11: degrade, don't break).
#[gpui::test]
fn an_unusable_override_keeps_the_default(cx: &mut TestAppContext) {
    use utopia_ui::actions::{install, GoToday};
    for bad in ["", "   ", "nonsense-key"] {
        let (o, warnings) =
            Overrides::from_json(&format!(r#"[{{"key":"{bad}","action":"GoToday"}}]"#));
        assert!(warnings.is_empty(), "a known action is read as before");
        cx.update(|cx| {
            cx.clear_key_bindings();
            install(cx, &o);
        });
        let keys: Vec<String> = cx.update(|cx| {
            cx.key_bindings()
                .borrow()
                .bindings_for_action(&GoToday)
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(|k| k.unparse())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        });
        assert_eq!(keys, [platform_keys("cmd-shift-t")], "override {bad:?}");
    }
}

mod appearance {
    use chrono::NaiveDate;
    use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};
    use utopia_store::Store;
    use utopia_ui::actions::install_keymap;
    use utopia_ui::app_state::AppState;
    use utopia_ui::appearance::{self, Density, UiFont};
    use utopia_ui::settings::{SettingsView, Tab};
    use utopia_ui::theme::Theme;
    use utopia_ui::workspace::Workspace;

    fn settings_json(dir: &std::path::Path) -> Option<serde_json::Value> {
        let s = std::fs::read_to_string(dir.join("settings.json")).ok()?;
        Some(serde_json::from_str(&s).expect("settings.json is JSON"))
    }

    /// A main window and the Settings window, both open, as in the app.
    fn open_both(
        cx: &mut TestAppContext,
        dir: &std::path::Path,
    ) -> (gpui::AnyWindowHandle, gpui::AnyWindowHandle) {
        let path = dir.to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let app = cx.new(|_| AppState::new(store));
        let main = cx.add_window(|window, cx| Workspace::new(app, window, cx));
        let settings =
            cx.add_window(move |window, cx| SettingsView::new(path, Tab::Appearance, window, cx));
        cx.run_until_parked();
        (main.into(), settings.into())
    }

    /// Settings → Appearance: picking IBM Plex Sans applies it to every open window at once
    /// and writes it to `settings.json` (not `keymap.json`), versioned.
    #[gpui::test]
    fn choosing_a_font_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::System);
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");
        {
            let mut w = VisualTestContext::from_window(main, cx);
            assert!(w.debug_bounds("ui-font-system").is_some());
        }

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ui-font-system").is_some());
        let option = s
            .debug_bounds("settings-font-ibm-plex-sans")
            .expect("the IBM Plex Sans choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();

        assert_eq!(
            s.update(|_, cx| appearance::ui_font(cx)),
            UiFont::IbmPlexSans
        );
        assert_eq!(
            s.update(|_, cx| appearance::font_family(cx)),
            "IBM Plex Sans",
            "the family the bundled faces register under"
        );
        // Both windows were drawn again with the new family at their root, unprompted.
        assert!(s.debug_bounds("settings-ui-font-ibm-plex-sans").is_some());
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(w.debug_bounds("ui-font-ibm-plex-sans").is_some());

        assert_eq!(
            settings_json(dir.path()),
            Some(serde_json::json!({
                "version": 1,
                "font": "ibm-plex-sans",
                "density": "comfortable",
            }))
        );
        assert!(
            !dir.path().join("keymap.json").exists(),
            "the font is not a keybinding"
        );

        // And back: System is written down too, so the choice is explicit.
        let mut s = VisualTestContext::from_window(settings, cx);
        let option = s.debug_bounds("settings-font-system").unwrap();
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(s.update(|_, cx| appearance::ui_font(cx)), UiFont::System);
        assert_eq!(
            settings_json(dir.path()).unwrap()["font"],
            "system",
            "switching back is saved"
        );
    }

    /// ALL-5: density is a local preference like the font. Picking Compact applies to every
    /// open window and is written to settings.json; Comfortable can be picked back.
    #[gpui::test]
    fn choosing_density_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(
            cx.update(|cx| appearance::density(cx)),
            Density::Comfortable
        );
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");
        {
            let mut w = VisualTestContext::from_window(main, cx);
            assert!(w.debug_bounds("ui-density-comfortable").is_some());
        }

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ui-density-comfortable").is_some());
        let option = s
            .debug_bounds("settings-density-compact")
            .expect("the Compact choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();

        assert_eq!(s.update(|_, cx| appearance::density(cx)), Density::Compact);
        assert!(s.debug_bounds("settings-ui-density-compact").is_some());
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds("ui-density-compact").is_some(),
            "the main window was redrawn compact without being asked"
        );
        let saved = settings_json(dir.path()).expect("settings.json written");
        assert_eq!(saved["density"], "compact");
        assert_eq!(saved["font"], "system", "the font is kept");

        // And back: Comfortable is explicit in the file, like System.
        let option = s.debug_bounds("settings-density-comfortable").unwrap();
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            s.update(|_, cx| appearance::density(cx)),
            Density::Comfortable
        );
        assert_eq!(settings_json(dir.path()).unwrap()["density"], "comfortable");
    }

    /// The saved choice is what a new launch comes up with.
    #[gpui::test]
    fn a_saved_font_is_used_at_startup(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "font": "ibm-plex-sans"}"#,
        )
        .unwrap();
        let (main, _) = open_both(cx, dir.path());
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::IbmPlexSans);
        let mut w = VisualTestContext::from_window(main, cx);
        assert!(w.debug_bounds("ui-font-ibm-plex-sans").is_some());
    }

    /// Rule 11: an unknown font in the file comes up as System, says so, and the window opens.
    #[gpui::test]
    fn an_unknown_font_falls_back_to_system(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "font": "comic-sans"}"#,
        )
        .unwrap();
        let warnings = cx.update(|cx| appearance::install(cx, dir.path()));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::System);

        let (main, settings) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        assert!(w.debug_bounds("ui-font-system").is_some());
        let view = settings.downcast::<SettingsView>().unwrap();
        let status = cx
            .read(|cx| view.read(cx).map(|v| v.status().map(str::to_string)))
            .unwrap()
            .unwrap_or_default();
        assert!(
            status.contains("comic-sans"),
            "the user is told: {status:?}"
        );
        // The file is left as it was until the user picks something.
        assert!(std::fs::read_to_string(dir.path().join("settings.json"))
            .unwrap()
            .contains("comic-sans"));
    }
}
