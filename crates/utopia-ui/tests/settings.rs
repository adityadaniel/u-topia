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

    /// Open the main window's launch page and its right sidebar, as a user would.
    fn open_sidebar(w: &mut VisualTestContext) {
        let ws = w
            .update(|window, _| window.root::<Workspace>())
            .flatten()
            .expect("the workspace root");
        ws.update_in(w, |this, window, cx| this.open_launch_page(window, cx));
        w.run_until_parked();
        w.simulate_keystrokes("escape cmd-shift-\\");
        w.run_until_parked();
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
                "right_sidebar_width": 380.0,
                "pinned_pages": [],
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

    /// RS-3: pinning a page is local UI state, written to `settings.json` (rule 9) and
    /// read back by the next launch.
    #[gpui::test]
    fn pinning_a_page_is_written_to_settings_json(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let page = store.create_page("Keep me").unwrap();
        let app = cx.new(|_| AppState::new(store));
        let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
        ws.update_in(cx, |w, window, cx| {
            w.open(page.clone(), None, None, false, window, cx)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape cmd-shift-\\");
        cx.run_until_parked();
        let pinned_tab = cx.debug_bounds("panel-pinned").expect("the Pinned tab");
        cx.simulate_click(pinned_tab.center(), Modifiers::default());
        cx.run_until_parked();
        let pin = cx.debug_bounds("pin-current").expect("the pin control");
        cx.simulate_click(pin.center(), Modifiers::default());
        cx.run_until_parked();

        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["pinned_pages"],
            serde_json::json!([page.as_str()])
        );
        let (prefs, warnings) = appearance::Prefs::load(dir.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(prefs.pinned_pages, vec![page.as_str().to_string()]);
        assert_eq!(
            prefs.density,
            Density::Comfortable,
            "pinning does not disturb the other preferences"
        );

        let unpin = cx.debug_bounds("unpin-0").expect("the unpin control");
        cx.simulate_click(unpin.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned_pages"],
            serde_json::json!([])
        );
    }

    /// RS-4: dragging the sidebar's edge changes its width at once and writes the width to
    /// `settings.json` on release; Settings → Appearance reports it and can reset it.
    #[gpui::test]
    fn the_sidebar_width_is_dragged_and_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        open_sidebar(&mut w);
        let handle = w.debug_bounds("right-resize").expect("the 4 px handle");
        let start = handle.center();
        w.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
        let to = gpui::point(start.x - gpui::px(80.), start.y);
        w.simulate_mouse_move(to, gpui::MouseButton::Left, Modifiers::default());
        w.simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::default());
        w.run_until_parked();

        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 460.).abs() < 1.5,
            "dragged 80 px wider: {side:?}"
        );
        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["right_sidebar_width"],
            serde_json::json!(460.0),
            "one write on release, with the width in it"
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(
            s.debug_bounds("settings-sidebar-width-460").is_some(),
            "Settings reports the width in force"
        );
        let reset = s
            .debug_bounds("settings-sidebar-reset")
            .expect("the reset control");
        s.simulate_click(reset.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["right_sidebar_width"],
            serde_json::json!(380.0),
            "the default is written down too"
        );
        w.run_until_parked();
        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 380.).abs() < 1.5,
            "the sidebar went back to the default width: {side:?}"
        );
    }

    /// RS-4: the width a previous session left is the width the next one opens with.
    #[gpui::test]
    fn a_saved_sidebar_width_is_used_at_startup(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "right_sidebar_width": 480}"#,
        )
        .unwrap();
        let (main, _) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        open_sidebar(&mut w);
        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 480.).abs() < 1.5,
            "the saved width is in force: {side:?}"
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
}
