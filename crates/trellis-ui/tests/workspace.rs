//! End-to-end UI scripts (§4.3 workflows) driving the real workspace through GPUI's
//! headless test platform: keystrokes in, store state out.

use chrono::NaiveDate;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use trellis_core::PageId;
use trellis_store::Store;
use trellis_ui::actions::install_keymap;
use trellis_ui::app_state::AppState;
use trellis_ui::theme::Theme;
use trellis_ui::workspace::Workspace;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

fn setup(cx: &mut TestAppContext) -> (Entity<AppState>, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    (app, ws, cx)
}

fn today_page(app: &Entity<AppState>, cx: &mut VisualTestContext) -> PageId {
    app.update(cx, |app, _| {
        app.store.find_page("2026-11-02").unwrap().unwrap()
    })
}

/// Live rows of a page as `(text, depth)`.
fn rows(app: &Entity<AppState>, page: &PageId, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    app.update(cx, |app, _| {
        let st = app.store.page_state(page).unwrap();
        st.live_preorder()
            .into_iter()
            .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
            .collect()
    })
}

fn rows_str(r: &[(String, usize)]) -> Vec<(&str, usize)> {
    r.iter().map(|(t, d)| (t.as_str(), *d)).collect()
}

#[gpui::test]
fn w1_capture_into_the_day(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    // AC-1.1: the caret is already in today's first block.
    cx.simulate_input("Met [[Dana]] about #project");
    // No page matches "project" yet, so no suggestion is open and Enter splits.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    app.update(cx, |app, _| {
        assert!(
            app.store.find_page("Dana").unwrap().is_some(),
            "pages exist on commit"
        );
        assert!(app.store.find_page("project").unwrap().is_some());
        let dana = app.store.find_page("Dana").unwrap().unwrap();
        assert_eq!(app.store.backlink_count(&dana).unwrap(), 1);
    });
}

#[gpui::test]
fn outliner_keyboard_ergonomics(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("b", 1)]);
    cx.simulate_keystrokes("shift-tab");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("b", 0)]);
    // Enter mid-block splits; Backspace at offset 0 merges back (FR-2.5).
    cx.simulate_keystrokes("left");
    cx.simulate_input("c");
    cx.simulate_keystrokes("left enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("a", 0), ("", 0), ("cb", 0)]
    );
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("cb", 0)]);
    // Up moves to the previous block; typing lands there.
    cx.simulate_keystrokes("up");
    cx.simulate_input("!");
    cx.run_until_parked();
    let r = rows(&app, &page, cx);
    assert!(r[0].0.contains('!'), "{r:?}");
    // Cmd+Shift+Up/Down reorders.
    cx.simulate_keystrokes("cmd-shift-down");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[1].0, r[0].0);
}

#[gpui::test]
fn undo_redo_from_the_editor(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("one");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("two");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("one", 0), ("", 0)]);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("one", 0)]);
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    // The caret came back after the undos: typing still lands in the page.
    cx.simulate_input("x");
    cx.run_until_parked();
    let r = rows(&app, &page, cx);
    assert!(r.iter().any(|(t, _)| t.contains('x')), "{r:?}");
}

#[gpui::test]
fn tag_suggestion_accepts_then_enter_splits(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("mindfulness").unwrap();
    });
    cx.simulate_input("practice #mind");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("practice #mindfulness", 0)]
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx).len(),
        2,
        "the accepted tag does not reopen the menu"
    );
}

#[gpui::test]
fn autocomplete_inserts_canonical_links(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Alpha Prime").unwrap();
        app.store.create_page("Modern Religion").unwrap();
    });
    cx.simulate_input("see [[alp");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.simulate_input(" and #mod");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = rows(&app, &page, cx)[0].0.clone();
    assert_eq!(text, "see [[Alpha Prime]] and #[[Modern Religion]]");
    // Escape dismisses and leaves the typed text intact (FR-2.17).
    cx.simulate_input(" [[Nov");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(rows(&app, &page, cx)[0].0.ends_with("[[Nov"));
}

#[gpui::test]
fn cmd_enter_creates_page_from_query(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("[[Brand New");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Brand New]]");
    app.update(cx, |app, _| {
        assert!(app.store.find_page("Brand New").unwrap().is_some())
    });
}

#[gpui::test]
fn quick_jump_creates_and_opens(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input("Reading log");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let created = app.update(cx, |app, _| app.store.find_page("Reading log").unwrap());
    assert!(created.is_some());
    let shown = ws.update(cx, |w, cx| w.main_page(cx));
    assert_eq!(shown, created);
    // The caret waits in the new page's first block (found by screenshot: typing was lost).
    cx.simulate_input("first thought");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let page = created.unwrap();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("first thought", 0)]);
}

#[gpui::test]
fn query_blocks_list_results(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("Read [[Dune]] #book");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Watch [[Dune]]");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("{{query: {and: [[Dune]] #book}}}");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("query-header").is_some(),
        "results header drawn"
    );
    let header = cx.debug_bounds("query-header").unwrap();
    let row3 = cx.debug_bounds("content-3").unwrap();
    assert!(
        header.origin.y > row3.origin.y,
        "results sit under the block"
    );
    // Results follow edits: un-tagging the first block empties the query.
    let page = today_page(&app, cx);
    app.update(cx, |app, cx| {
        let first = app.store.page_state(&page).unwrap().live_roots()[0].clone();
        app.store
            .set_block_text(&page, &first, "Read [[Dune]]", None, None)
            .unwrap();
        app.store.commit_block(&page, &first).unwrap();
        app.changed(vec![page.clone()], false, cx);
    });
    cx.run_until_parked();
    let r = app.update(cx, |app, _| {
        let e = trellis_core::query::parse("{and: [[Dune]] #book}").unwrap();
        app.store.run_query(&e, 10).unwrap()
    });
    assert_eq!(r.total, 0);
}

#[gpui::test]
fn day_navigation(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape cmd-[");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    let row = app.update(cx, |app, _| app.store.page_row(&shown).unwrap().unwrap());
    assert_eq!(row.daily_date, NaiveDate::from_ymd_opt(2026, 11, 1));
    cx.simulate_keystrokes("cmd-shift-d");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    assert_eq!(shown, today_page(&app, cx));
}

#[gpui::test]
fn marks_and_soft_breaks(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("**bold");
    cx.simulate_keystrokes("cmd-b");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("line two");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "**bold**\nline two");
}

#[gpui::test]
fn zoom_collapse_and_delete(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab up");
    cx.run_until_parked();
    // Cmd+. collapses locally; nothing changes in the page's content.
    cx.simulate_keystrokes("cmd-.");
    cx.run_until_parked();
    let parent = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    assert!(app.read_with(cx, |app, _| app.store.is_collapsed(&parent)));
    cx.simulate_keystrokes("cmd-.");
    cx.run_until_parked();
    // Cmd+Alt+Down zooms into the block; typing lands in its children.
    cx.simulate_keystrokes("cmd-alt-down");
    cx.run_until_parked();
    let zoom = ws.update(cx, |w, cx| w.main_zoom(cx));
    assert_eq!(zoom, Some(parent.clone()));
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("parent", 0), ("!child", 1)]
    );
    // Cmd+Shift+Backspace tombstones the block (recoverable with undo).
    cx.simulate_keystrokes("cmd-shift-backspace");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("parent", 0)]);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("parent", 0), ("!child", 1)]
    );
}

#[gpui::test]
fn block_reference_autocomplete(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a central thesis");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("see ((thes");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (target, text) = app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        let roots = st.live_roots();
        (roots[0].clone(), st.block(&roots[1]).unwrap().text.clone())
    });
    assert_eq!(text, format!("see (({target}))"));
    // AC-3.2: the edge appears once the block commits (300 ms idle), not per keystroke.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        assert_eq!(app.store.block_backlinks(&target).unwrap().len(), 1);
    });
}

#[gpui::test]
fn sidebar_stacks_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (a, b) = app.update(cx, |app, _| {
        (
            app.store.create_page("Alpha").unwrap(),
            app.store.create_page("Beta").unwrap(),
        )
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(a.clone(), None, None, true, window, cx);
        w.open(b.clone(), None, None, true, window, cx);
        w.open(a.clone(), None, None, true, window, cx);
    });
    cx.run_until_parked();
    let panes = ws.update(cx, |w, cx| w.sidebar_pages(cx));
    assert_eq!(
        panes,
        vec![a, b],
        "re-opening moves a pane to the top, no duplicates"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(today_page(&app, cx))
    );
}

#[gpui::test]
fn keystrokes_are_debounced_then_committed(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("[[Later]] typed fast");
    cx.run_until_parked();
    // In memory immediately; the index catches up after the idle flush (§13.3).
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Later]] typed fast");
    assert!(app.read_with(cx, |app, _| app.store.has_pending()));
    let before = app.read_with(cx, |app, _| app.refs_version);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.store.has_pending()));
    assert!(
        app.read_with(cx, |app, _| app.refs_version) > before,
        "index update announced"
    );
    app.update(cx, |app, _| {
        let later = app
            .store
            .find_page("Later")
            .unwrap()
            .expect("created on idle commit");
        assert_eq!(app.store.backlink_count(&later).unwrap(), 1);
    });
}

#[gpui::test]
fn caret_survives_remote_edits(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("hello world");
    cx.simulate_keystrokes("left left left left left");
    cx.run_until_parked();
    // Another device inserts text at the start of the same block; sync delivers it.
    let block = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    let update = app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        let bytes = app.store.export_since(&page, &Default::default()).unwrap();
        let mut remote = Store::open_in_memory().unwrap();
        remote.import(&page, &bytes).unwrap();
        remote
            .set_block_text(&page, &block, ">> hello world", None, None)
            .unwrap();
        remote.sync_index().unwrap();
        remote.export_since(&page, &Default::default()).unwrap()
    });
    app.update(cx, |app, cx| {
        app.store.import(&page, &update).unwrap();
        app.changed(vec![page.clone()], true, cx);
    });
    cx.run_until_parked();
    // The caret stayed before "world", not at the old byte offset.
    cx.simulate_input("big ");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, ">> hello big world");
}

#[gpui::test]
fn background_sync_imports_and_pushes(cx: &mut TestAppContext) {
    use trellis_sync::relay::{Relay, RelayTransport};
    use trellis_sync::{SyncClient, SyncStatus};
    let (app, ws, cx) = setup(cx);
    let relay = Relay::new();
    ws.update(cx, |w, cx| {
        w.enable_sync_with(Box::new(RelayTransport::new(relay.clone(), "g")), None, cx)
    });
    // Another device publishes a page.
    let mut other = Store::open_in_memory().unwrap();
    let mut oc = SyncClient::new(RelayTransport::new(relay.clone(), "g"), &other).unwrap();
    let remote_page = other.create_page("From the phone").unwrap();
    oc.sync_once(&mut other).unwrap();
    // Local typing, then one sync tick.
    cx.simulate_input("local words");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(6));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        assert!(
            app.store.page_row(&remote_page).unwrap().is_some(),
            "remote page imported"
        );
    });
    assert!(matches!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(SyncStatus::Synced { .. })
    ));
    // The other device now receives the local edit.
    oc.sync_once(&mut other).unwrap();
    let today = today_page(&app, cx);
    let st = other.page_state(&today).unwrap();
    assert_eq!(st.block(&st.live_roots()[0]).unwrap().text, "local words");
}

// ---------------------------------------------------------------- v1.1 (P1)

fn kind_of(
    app: &Entity<AppState>,
    page: &PageId,
    ix: usize,
    cx: &mut VisualTestContext,
) -> (trellis_core::BlockKind, bool) {
    app.update(cx, |app, _| {
        let st = app.store.page_state(page).unwrap();
        let id = st.live_preorder()[ix].0.clone();
        let b = st.block(&id).unwrap();
        (b.kind, b.checked)
    })
}

#[gpui::test]
fn slash_menu_sets_block_types(cx: &mut TestAppContext) {
    use trellis_core::BlockKind;
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("Plan /head");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "Plan ",
        "the /command text is removed"
    );
    assert_eq!(kind_of(&app, &page, 0, cx).0, BlockKind::Heading(1));
    cx.simulate_input("/quote");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx).0, BlockKind::Quote);
    // Alt+Enter cycles bullet → to-do → done.
    cx.simulate_keystrokes("alt-enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx), (BlockKind::Todo, false));
    cx.simulate_keystrokes("alt-enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx), (BlockKind::Todo, true));
}

#[gpui::test]
fn slash_template_and_today(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        let t = app.store.create_page("templates/meeting").unwrap();
        let b = app.store.ensure_first_block(&t).unwrap();
        app.store
            .set_block_text(&t, &b, "Attendees::", None, None)
            .unwrap();
    });
    cx.simulate_input("standup /meet");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("standup ", 0), ("Attendees::", 0)]
    );
    cx.simulate_input(" on /today");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[1].0,
        "Attendees:: on [[November 2nd, 2026]]"
    );
}

#[gpui::test]
fn slash_extract_block_to_page(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a big idea /extr");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("Big Idea");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Big Idea]]");
    let dst = app.update(cx, |app, _| {
        app.store.find_page("Big Idea").unwrap().unwrap()
    });
    assert_eq!(rows(&app, &dst, cx)[0].0, "a big idea ");
    assert_eq!(
        ws.update(cx, |w, cx| w.sidebar_pages(cx)),
        vec![dst],
        "opened beside"
    );
}

#[gpui::test]
fn full_search_and_quick_capture(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let target = app.update(cx, |app, _| {
        let p = app.store.create_page("Archive").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "the zebra crossing story", None, None)
            .unwrap();
        app.store.sync_index().unwrap();
        p
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(target.clone()));
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &target, cx)[0].0,
        "the zebra crossing story!",
        "editing the hit"
    );
    // Quick capture appends to today without leaving the page.
    cx.simulate_keystrokes("escape cmd-shift-space");
    cx.simulate_input("remember the milk");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let today = today_page(&app, cx);
    assert!(rows(&app, &today, cx)
        .iter()
        .any(|(t, _)| t == "remember the milk"));
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(target));
}

#[gpui::test]
fn unlinked_references_link_button(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (m, src) = app.update(cx, |app, _| {
        let m = app.store.create_page("Mindfulness").unwrap();
        let src = app.store.create_page("Reading").unwrap();
        let b = app.store.ensure_first_block(&src).unwrap();
        app.store
            .set_block_text(&src, &b, "practising mindfulness daily", None, None)
            .unwrap();
        app.store.sync_index().unwrap();
        (m, src)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(m.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let header = cx
        .debug_bounds("unlinked-header")
        .expect("section rendered");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let link = cx
        .debug_bounds("link-4")
        .or_else(|| cx.debug_bounds("link-5"))
        .or_else(|| cx.debug_bounds("link-6"));
    let link = link.expect("a Link button per mention");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &src, cx)[0].0,
        "practising [[Mindfulness]] daily"
    );
    app.update(cx, |app, _| {
        assert_eq!(app.store.backlink_count(&m).unwrap(), 1)
    });
}

#[gpui::test]
fn embeds_render_and_split_panes(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (other, target) = app.update(cx, |app, _| {
        let p = app.store.create_page("Source").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "embedded thought", None, None)
            .unwrap();
        (p, b)
    });
    cx.simulate_input(&format!("{{{{embed: (({target}))}}}}"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    // The embed renders the source block as an extra (editable) row.
    assert!(
        cx.debug_bounds("content-2").is_some(),
        "embedded row rendered"
    );
    ws.update_in(cx, |w, window, cx| w.open_split(other.clone(), window, cx));
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.split_pages(cx)), vec![other]);
    let split = cx.debug_bounds("split-0").expect("split pane drawn");
    let main = cx.debug_bounds("main-pane").unwrap();
    assert!(split.origin.x > main.origin.x && split.size.width > gpui::px(100.));
}

#[gpui::test]
fn encrypted_folder_sync_from_config(cx: &mut TestAppContext) {
    use trellis_sync::e2e::{Encrypted, GraphKey};
    use trellis_sync::folder::FolderTransport;
    use trellis_sync::SyncClient;
    use trellis_ui::sync_runner::{SyncConfig, SyncTarget};
    let (app, ws, cx) = setup(cx);
    let shared = tempfile::tempdir().unwrap();
    ws.update(cx, |w, cx| {
        w.enable_sync(
            SyncConfig {
                target: SyncTarget::Folder(shared.path().to_path_buf()),
                graph: "notes".into(),
                passphrase: Some("hunter2 hunter2".into()),
            },
            cx,
        )
    });
    cx.simulate_input("synced privately");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(6));
    cx.run_until_parked();
    // Another device with the passphrase reads it; the folder holds only ciphertext.
    let mut other = Store::open_in_memory().unwrap();
    let t = FolderTransport::new(shared.path(), "phone").unwrap();
    let key = GraphKey::derive("hunter2 hunter2", "notes").unwrap();
    let mut c = SyncClient::new(Encrypted::new(t, key, "notes"), &other).unwrap();
    c.sync_once(&mut other).unwrap();
    let today = today_page(&app, cx);
    let st = other.page_state(&today).unwrap();
    assert_eq!(
        st.block(&st.live_roots()[0]).unwrap().text,
        "synced privately"
    );
    for entry in walk(shared.path()) {
        let bytes = std::fs::read(&entry).unwrap();
        assert!(
            !bytes.windows(8).any(|w| w == b"privatel"),
            "plaintext in {entry:?}"
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[gpui::test]
fn graph_view_lays_out_and_opens_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    assert!(cx.debug_bounds("graph-view").is_some());
    // Alpha, Beta and today's page, linked through the journal page.
    let (nodes, edges, laid_out) =
        graph.update(cx, |g, _| (g.node_count(), g.edge_count(), g.is_laid_out()));
    assert_eq!((nodes, edges, laid_out), (3, 2, true));
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let i = (0..nodes)
        .find(|&i| graph.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = graph.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    let canvas = cx.debug_bounds("graph-canvas").unwrap();
    assert!(
        canvas.contains(&at),
        "node drawn inside the canvas: {at:?} {canvas:?}"
    );
    cx.simulate_click(at, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_none(),
        "graph closes"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(alpha));
    // Escape closes it too.
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_view()).is_none());
}

#[gpui::test]
fn dropped_files_become_attachments(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("minutes.txt");
    std::fs::write(&file, b"meeting minutes").unwrap();
    // While editing: inserted at the caret.
    cx.simulate_input("see ");
    ws.update_in(cx, |w, window, cx| {
        w.drop_files(std::slice::from_ref(&file), window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let page = today_page(&app, cx);
    let hash = trellis_store::sha256_hex(b"meeting minutes");
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![(format!("see [minutes.txt](blob:{hash})").as_str(), 0)]
    );
    assert!(cx.debug_bounds("attachment-chip").is_some(), "chip drawn");
    // Not editing: appended as a new block.
    ws.update_in(cx, |w, window, cx| {
        w.drop_files(std::slice::from_ref(&file), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    assert!(app.update(cx, |a, _| a.store.has_blob(&hash)));
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn pdf_viewer_drag_creates_highlight_block(cx: &mut TestAppContext) {
    use gpui::{point, px, Modifiers, MouseButton, MouseMoveEvent};
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("g.trellis")).unwrap();
    store.set_today(Some(today()));
    let pdf = trellis_ui::pdf::sample_pdf(&[&["Leverage points matter", "a second line"]]);
    let att = store.add_blob("paper.pdf", &pdf).unwrap();
    let page = store.create_page("Reading").unwrap();
    let host = store.ensure_first_block(&page).unwrap();
    store
        .set_block_text(
            &page,
            &host,
            &trellis_core::attach::markup(&att.name, &att.hash, &att.mime),
            None,
            None,
        )
        .unwrap();
    store.commit_block(&page, &host).unwrap();
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    cx.simulate_resize(gpui::size(px(1280.), px(820.)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx);
        w.open_pdf(
            att.hash.clone(),
            att.name.clone(),
            None,
            Some(host.clone()),
            window,
            cx,
        );
    });
    cx.run_until_parked();
    let view = ws.update(cx, |w, _| w.pdf_view()).expect("viewer open");
    assert_eq!(view.update(cx, |v, _| v.page_count()), 1);
    assert!(cx.debug_bounds("pdf-view").is_some());
    let b = view
        .update(cx, |v, _| v.page_bounds(0))
        .expect("page drawn");
    // Drag across the first line (about 9–11% down the page).
    let at = |x: f32, y: f32| {
        point(
            b.origin.x + b.size.width * x,
            b.origin.y + b.size.height * y,
        )
    };
    cx.simulate_mouse_down(at(0.05, 0.07), MouseButton::Left, Modifiers::default());
    cx.simulate_event(MouseMoveEvent {
        position: at(0.9, 0.105),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_mouse_up(at(0.9, 0.105), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    assert_eq!(hl.len(), 1, "one highlight");
    assert!(
        hl[0].text.contains("Leverage points matter"),
        "{:?}",
        hl[0].text
    );
    assert!(!hl[0].text.contains("second line"));
    // It is a quote block under the PDF's block.
    let parent = app.update(cx, |a, _| {
        a.store.page_state(&page).unwrap().parent_of(&hl[0].block)
    });
    assert_eq!(parent, Some(host));
    assert_eq!(hl[0].rects.len(), 1, "a one-line text selection");
    // Across two lines: one rectangle per line, both lines quoted.
    let drag = |cx: &mut VisualTestContext,
                a: gpui::Point<gpui::Pixels>,
                b: gpui::Point<gpui::Pixels>,
                m: Modifiers| {
        cx.simulate_mouse_down(a, MouseButton::Left, m);
        cx.simulate_event(MouseMoveEvent {
            position: b,
            pressed_button: Some(MouseButton::Left),
            modifiers: m,
        });
        cx.simulate_mouse_up(b, MouseButton::Left, m);
        cx.run_until_parked();
    };
    // (Starting on line 2: pressing on an existing highlight opens its block instead.)
    drag(cx, at(0.3, 0.125), at(0.15, 0.09), Modifiers::default());
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    let two = hl
        .iter()
        .find(|h| h.rects.len() == 2)
        .expect("two-line highlight");
    assert!(two.text.contains("second"), "{:?}", two.text);
    assert!(cx.debug_bounds("pdf-hl-1-1").is_some() || cx.debug_bounds("pdf-hl-0-1").is_some());
    // Option-drag over blank paper: an area highlight with no quote.
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    drag(cx, at(0.2, 0.5), at(0.6, 0.7), alt);
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    assert_eq!(hl.len(), 3);
    assert!(hl
        .iter()
        .any(|h| h.text.is_empty() && h.rects.len() == 1 && h.rects[0][1] > 0.45));
    // A plain click is not a highlight.
    cx.simulate_click(at(0.5, 0.5), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.update(cx, |v, _| v.highlights().len()), 3);
}

/// Text of every live block on the daily page for `d`.
fn day_texts(app: &Entity<AppState>, d: NaiveDate, cx: &mut VisualTestContext) -> Vec<String> {
    let page = app.update(cx, |app, _| {
        app.store
            .find_page(&trellis_core::date::iso(d))
            .unwrap()
            .expect("daily page exists")
    });
    rows(app, &page, cx).into_iter().map(|(t, _)| t).collect()
}

#[gpui::test]
fn daily_notes_feed_scrolls_into_older_days(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Notes on every other day before today: 25 older days.
    app.update(cx, |app, _| {
        for i in 1..=25 {
            let d = today() - chrono::Duration::days(i * 2);
            let (p, b) = app.store.open_daily(d).unwrap();
            app.store
                .set_block_text(&p, &b, &format!("note {i}"), None, None)
                .unwrap();
            app.store.commit_block(&p, &b).unwrap();
        }
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    let days = ws.update(cx, |w, cx| w.journal_days(cx));
    let today_id = today_page(&app, cx);
    assert_eq!(days[0], today_id, "today comes first");
    // Batches load until the end of the feed is out of view, not all at once.
    assert!(
        (10..26).contains(&days.len()),
        "a screenful of days, not all 26: {}",
        days.len()
    );
    let dates: Vec<NaiveDate> = app.update(cx, |app, _| {
        days.iter()
            .map(|p| app.store.page_row(p).unwrap().unwrap().daily_date.unwrap())
            .collect()
    });
    assert!(
        dates.windows(2).all(|w| w[0] > w[1]),
        "newest first: {dates:?}"
    );
    // The caret starts in today; Down crosses into the next day, editable in place.
    cx.simulate_input("hello");
    cx.simulate_keystrokes("down cmd-right");
    cx.simulate_input(" edited");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(day_texts(&app, today(), cx), ["hello"]);
    assert_eq!(
        day_texts(&app, today() - chrono::Duration::days(2), cx),
        ["note 1 edited"]
    );
    // Scrolling to the end loads the remaining days; none are created (FR-1.2).
    let main = cx.debug_bounds("main-pane").expect("main pane");
    for _ in 0..6 {
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: main.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-4000.))),
            ..Default::default()
        });
        cx.run_until_parked();
    }
    let days = ws.update(cx, |w, cx| w.journal_days(cx));
    assert_eq!(days.len(), 26, "every journal page, and only those");
    let daily = app.update(cx, |app, _| {
        app.store
            .list_pages(true)
            .unwrap()
            .into_iter()
            .filter(|p| p.is_daily())
            .count()
    });
    assert_eq!(daily, 26);
}

#[gpui::test]
fn relative_date_links(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // Autocomplete offers tomorrow's page first.
    cx.simulate_input("Pay rent [[tomorrow");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // A closed link is replaced once the caret leaves it.
    cx.simulate_keystrokes("cmd-right enter");
    cx.simulate_input("Met Ana [[Yesterday]] and #[[next friday]] too");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        [
            "Pay rent [[November 3rd, 2026]]",
            "Met Ana [[November 1st, 2026]] and #[[November 6th, 2026]] too",
        ]
    );
    app.update(cx, |app, _| {
        let s = &mut app.store;
        s.sync_index().unwrap();
        for iso in ["2026-11-03", "2026-11-01", "2026-11-06"] {
            let p = s.find_page(iso).unwrap().expect(iso);
            assert_eq!(s.backlink_count(&p).unwrap(), 1, "{iso}");
        }
        assert!(
            s.find_page("Yesterday").unwrap().is_none(),
            "no page named Yesterday"
        );
    });
}
