//! The Settings keybinding editor (§9.1), driven through GPUI's headless test platform.

use gpui::{px, size, TestAppContext};
use utopia_ui::actions::{active_keys, doc, install_keymap, platform_keys, Overrides, Quit};
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

/// QJ-5: what the palette shows for a command is the keys in force. Settings installs a
/// rebinding at once, so a row must describe the new chord, not the catalog default.
#[gpui::test]
fn a_rebinding_changes_the_keys_a_command_shows(cx: &mut TestAppContext) {
    use utopia_ui::actions::reinstall;
    let mut o = Overrides::default();
    o.set("GoToday", "alt-cmd-t");
    cx.update(|cx| {
        install_keymap(cx, None);
        reinstall(cx, &o);
    });
    let keys = cx.update(|cx| active_keys(cx, doc("GoToday").unwrap()));
    assert_eq!(keys, "alt-cmd-t");
    assert_eq!(
        utopia_ui::actions::pretty_keys(&keys),
        "⌥⌘T",
        "rendered the way the palette draws it"
    );
    // An action nobody rebound still reports its catalog default.
    let other = cx.update(|cx| active_keys(cx, doc("GoYesterday").unwrap()));
    assert_eq!(other, platform_keys("cmd-shift-y"));
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
    use utopia_core::PageId;
    use utopia_store::Store;
    use utopia_ui::actions::install_keymap;
    use utopia_ui::app_state::AppState;
    use utopia_ui::appearance::{self, AttributeBlocks, Density, UiFont};
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

    /// A main window showing "Target" (referenced by a block with four children) and the
    /// Settings window, both open, as in the app (LR-4).
    fn open_both_with_reference(
        cx: &mut TestAppContext,
        dir: &std::path::Path,
    ) -> (
        gpui::AnyWindowHandle,
        gpui::AnyWindowHandle,
        utopia_core::PageId,
        VisualTestContext,
    ) {
        use utopia_core::{BlockId, BlockKind, Command};
        let path = dir.to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let target = store.create_page("Target").unwrap();
        let src = store.create_page("Source").unwrap();
        let link = BlockId::new();
        let mut cmds = vec![Command::InsertBlock {
            id: link.clone(),
            parent: None,
            after: None,
            text: "The release depends on [[Target]]".into(),
            kind: BlockKind::Bullet,
        }];
        let mut after = None;
        for n in 1..=4 {
            let id = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: id.clone(),
                parent: Some(link.clone()),
                after: after.clone(),
                text: format!("Reason {n}"),
                kind: BlockKind::Bullet,
            });
            after = Some(id);
        }
        store.execute(&src, cmds, Default::default()).unwrap();
        store.sync_index().unwrap();
        let app = cx.new(|_| AppState::new(store));
        let main: gpui::AnyWindowHandle = cx
            .add_window(|window, cx| Workspace::new(app, window, cx))
            .into();
        let settings =
            cx.add_window(move |window, cx| SettingsView::new(path, Tab::Appearance, window, cx));
        cx.run_until_parked();
        let mut w = VisualTestContext::from_window(main, cx);
        let ws = w
            .update(|window, _| window.root::<Workspace>())
            .flatten()
            .expect("workspace root");
        ws.update_in(&mut w, |w, window, cx| {
            w.open(target.clone(), None, None, false, window, cx)
        });
        w.run_until_parked();
        (main, settings.into(), target, w)
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
                "ref_children": 2,
                "pinned": [],
                "attribute-blocks": "compact",
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

    /// SB-3: pins live in settings.json (not keymap.json, never the graph). Settings →
    /// Appearance says how many there are and can clear them.
    #[gpui::test]
    fn pinned_pages_are_local_and_cleared_from_settings(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (_main, settings) = open_both(cx, dir.path());
        let page = PageId("page-a".into());
        cx.update(|cx| {
            let on = appearance::toggle_pinned(cx, &page).expect("settings.json is writable");
            assert!(on);
        });
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned"],
            serde_json::json!(["page-a"]),
            "the pin is local state in settings.json"
        );
        assert!(
            !dir.path().join("keymap.json").exists(),
            "a pin is not a keybinding"
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(
            s.debug_bounds("settings-pinned").is_some(),
            "Settings → Appearance shows the pinned pages"
        );
        let clear = s
            .debug_bounds("settings-clear-pinned")
            .expect("a Clear control when something is pinned");
        s.simulate_click(clear.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned"],
            serde_json::json!([]),
            "Clear unpins every page"
        );
        assert!(s.update(|_, cx| appearance::pinned(cx).is_empty()));
        // Nothing to clear any more. (The old control's debug bounds linger, so assert on
        // what replaced it: the status line says the pins are gone.)
        let view = s
            .update(|window, _| window.root::<SettingsView>())
            .flatten()
            .expect("settings root");
        assert_eq!(
            view.update(cx, |v, _| v.status().map(str::to_string)),
            Some("Unpinned every page.".to_string())
        );
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

    /// PG-1: how attribute blocks are drawn is a third local preference, picked in
    /// Settings → Appearance, applied to every open window and written to settings.json.
    #[gpui::test]
    fn choosing_attribute_blocks_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(
            cx.update(|cx| appearance::attribute_blocks(cx)),
            AttributeBlocks::Compact,
            "compact rows are the default"
        );
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        let option = s
            .debug_bounds("settings-attributes-blocks")
            .expect("the As blocks choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            s.update(|_, cx| appearance::attribute_blocks(cx)),
            AttributeBlocks::Blocks
        );
        let saved = settings_json(dir.path()).expect("settings.json written");
        assert_eq!(saved["attribute-blocks"], "blocks");
        assert_eq!(saved["font"], "system", "the other preferences are kept");

        // The main window follows, without being asked, and can be switched back here.
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds("ui-density-comfortable").is_some(),
            "the page window still draws"
        );
        let option = s
            .debug_bounds("settings-attributes-compact")
            .expect("the Compact choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["attribute-blocks"],
            "compact",
            "switching back is saved too"
        );
    }

    /// LR-4: "Children shown in references" is a local preference (rule 9). Picking 0 folds
    /// every child behind the chip in a page that is already open; 3 shows three of four.
    #[gpui::test]
    fn choosing_reference_children_is_saved_and_applies(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (_main, settings, _target, mut w) = open_both_with_reference(cx, dir.path());
        let ix = (0..40)
            .find(|ix| {
                let sel: &'static str = Box::leak(format!("hit-child-{ix}-0").into_boxed_str());
                w.debug_bounds(sel).is_some()
            })
            .expect("the default of two children is shown");
        let (more_four, more_one): (&'static str, &'static str) = (
            Box::leak(format!("hit-more-{ix}-4").into_boxed_str()),
            Box::leak(format!("hit-more-{ix}-1").into_boxed_str()),
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ref-children-2").is_some());
        let zero = s
            .debug_bounds("settings-ref-children-0")
            .expect("the 0 choice is offered");
        s.simulate_click(zero.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["ref_children"],
            0
        );
        assert!(s.debug_bounds("settings-ref-children-0").is_some());

        // The page that was already open folds all four behind the chip.
        let mut w = VisualTestContext::from_window(_main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds(more_four).is_some(),
            "0 hides all four children"
        );

        let three = s
            .debug_bounds("settings-ref-children-3")
            .expect("the 3 choice is offered");
        s.simulate_click(three.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(settings_json(dir.path()).unwrap()["ref_children"], 3);
        let mut w = VisualTestContext::from_window(_main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds(more_one).is_some(),
            "three shown, one folded"
        );
    }
}
