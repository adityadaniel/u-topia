//! Layout geometry (§9.2) checked on GPUI's test platform: the window is laid out and
//! painted headlessly and element bounds are read back by `debug_selector`.

use chrono::NaiveDate;
use gpui::{px, size, AppContext as _, Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
use utopia_core::outline;
use utopia_core::PageId;
use utopia_store::Store;
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::AppState;
use utopia_ui::theme::{Theme, INDENT};
use utopia_ui::workspace::Workspace;

const W: f32 = 1280.;
const H: f32 = 820.;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

fn setup(cx: &mut TestAppContext) -> &mut VisualTestContext {
    setup_seeded(cx, |_| {})
}

/// The workspace over a store seeded directly (never through keystrokes): deterministic
/// content for the sidebar, and no race with the UI's debounce.
fn setup_seeded(cx: &mut TestAppContext, seed: impl FnOnce(&mut Store)) -> &mut VisualTestContext {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    seed(&mut store);
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx
}

/// `n` days of history, two blocks each, so the old two-line preview would have shown
/// both (SB-1).
fn seed_days(store: &mut Store, n: usize) {
    for i in 0..n {
        let d = today() - chrono::Duration::days(i as i64);
        let (p, b) = store.open_daily(d).unwrap();
        store
            .set_block_text(&p, &b, &format!("Day {i} first block"), None, None)
            .unwrap();
        store.commit_block(&p, &b).unwrap();
        let plan = outline::append_block(store.page_state(&p).unwrap(), None);
        let second = store.execute_plan(&p, plan, None).unwrap().unwrap().block;
        store
            .set_block_text(&p, &second, &format!("Day {i} second block"), None, None)
            .unwrap();
        store.commit_block(&p, &second).unwrap();
    }
}

/// Today's page on its own (the calendar's "Today"), not the Daily Notes feed.
fn setup_page(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open_date(
            NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
            false,
            true,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    cx
}

fn bounds(cx: &mut VisualTestContext, sel: &'static str) -> Bounds<Pixels> {
    bounds_of(cx, sel.to_string())
}

/// `debug_bounds` only takes a `&'static str`; a test-built selector is leaked for the
/// duration of the process (a handful of short strings).
fn bounds_of(cx: &mut VisualTestContext, sel: String) -> Bounds<Pixels> {
    let sel: &'static str = Box::leak(sel.into_boxed_str());
    cx.run_until_parked();
    cx.debug_bounds(sel)
        .unwrap_or_else(|| panic!("{sel} was not rendered"))
}

fn approx(a: Pixels, b: f32) -> bool {
    (f32::from(a) - b).abs() < 1.5
}

#[gpui::test]
fn three_column_window(cx: &mut TestAppContext) {
    let cx = setup(cx);
    let left = bounds(cx, "left-sidebar");
    let main = bounds(cx, "main-pane");
    assert!(
        approx(left.origin.x, 0.) && approx(left.size.width, 250.),
        "{left:?}"
    );
    assert!(
        approx(left.size.height, H),
        "sidebar fills the height: {left:?}"
    );
    assert!(
        approx(main.origin.x, 250.),
        "main pane starts after the sidebar: {main:?}"
    );
    assert!(
        approx(main.size.width, W - 250.),
        "main pane fills the rest: {main:?}"
    );
    assert!(
        cx.debug_bounds("right-sidebar").is_none(),
        "right sidebar starts closed"
    );
    // Cmd+Shift+\ opens the right sidebar at the right edge (§9.2).
    cx.simulate_keystrokes("escape cmd-shift-\\");
    let right = bounds(cx, "right-sidebar");
    let main = bounds(cx, "main-pane");
    assert!(
        approx(right.size.width, 380.) && approx(right.origin.x + right.size.width, W),
        "{right:?}"
    );
    assert!(approx(main.size.width, W - 250. - 380.), "{main:?}");
    assert!(
        main.origin.x + main.size.width <= right.origin.x + px(0.5),
        "no overlap"
    );
}

#[gpui::test]
fn outline_rows_indent_and_stack(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab enter");
    cx.simulate_input("grandchild");
    cx.simulate_keystrokes("tab escape");
    let title = bounds(cx, "page-title");
    // Items: 0 header, 1..=3 rows.
    let r1 = bounds(cx, "content-1");
    let r2 = bounds(cx, "content-2");
    let r3 = bounds(cx, "content-3");
    assert!(
        title.origin.y + title.size.height <= r1.origin.y + px(0.5),
        "title above rows"
    );
    assert!(
        r1.origin.y < r2.origin.y && r2.origin.y < r3.origin.y,
        "rows stack downward"
    );
    assert!(
        approx(r2.origin.x - r1.origin.x, INDENT),
        "one indent per level: {r1:?} {r2:?}"
    );
    assert!(approx(r3.origin.x - r2.origin.x, INDENT));
    let main = bounds(cx, "main-pane");
    for r in [r1, r2, r3] {
        assert!(
            r.origin.x > main.origin.x
                && r.origin.x + r.size.width <= main.origin.x + main.size.width + px(0.5)
        );
        assert!(
            r.size.height >= px(20.),
            "a row is at least one line tall: {r:?}"
        );
    }
    let refs = bounds(cx, "refs-header");
    assert!(
        refs.origin.y > r3.origin.y,
        "Linked References sit below the outline"
    );
    // List items shrink to their content unless wrapped; the header's rule and count
    // must span the column inside its 16px margins (found by screenshot). The column is
    // a readable width, centred in the pane.
    let column = f32::from(main.size.width).min(760.);
    let left = (f32::from(main.size.width) - column) / 2.;
    assert!(
        approx(refs.origin.x - main.origin.x, left + 16.) && approx(refs.size.width, column - 32.),
        "refs header spans the column: {refs:?} {main:?}"
    );
}

#[gpui::test]
fn editor_and_autocomplete_placement(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("[[fir");
    let editor = bounds(cx, "block-editor");
    let row2 = bounds(cx, "content-2");
    assert!(
        approx(editor.origin.y, f32::from(row2.origin.y)),
        "editor renders inside the focused row"
    );
    // "Create" is offered for a novel title, so the popup is open below the text.
    let popup = bounds(cx, "autocomplete");
    assert!(
        popup.origin.y >= editor.origin.y + px(10.),
        "popup below the caret line: {popup:?} {editor:?}"
    );
    assert!(
        popup.origin.x + popup.size.width <= px(W) + px(0.5),
        "popup stays inside the window"
    );
}

#[gpui::test]
fn quick_jump_is_centered(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    let p = bounds(cx, "jump-palette");
    assert!(approx(p.size.width, 520.));
    let center = f32::from(p.origin.x) + 260.;
    assert!((center - W / 2.).abs() < 2., "centered horizontally: {p:?}");
    assert!(
        p.origin.y > px(60.) && p.origin.y < px(H / 2.),
        "in the upper half: {p:?}"
    );
}

#[gpui::test]
fn minimum_window_size_still_lays_out(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_resize(size(px(720.), px(480.)));
    cx.simulate_input("still editable at the minimum size");
    let main = bounds(cx, "main-pane");
    let row = bounds(cx, "content-1");
    assert!(approx(main.size.width, 720. - 250.), "{main:?}");
    assert!(
        row.origin.x + row.size.width <= px(720.) + px(0.5),
        "rows wrap inside the window"
    );
}

#[gpui::test]
fn daily_notes_feed_stacks_days(cx: &mut TestAppContext) {
    let cx = setup(cx);
    // Launch shows the feed: today's title, then its first block.
    cx.simulate_input("today's note");
    cx.simulate_keystrokes("escape");
    let title = bounds(cx, "today-title");
    let row = bounds(cx, "content-1");
    let older = bounds(cx, "day-title");
    assert!(older.origin.y > row.origin.y, "older days follow today");
    assert!(title.origin.y + title.size.height <= row.origin.y + px(0.5));
    assert!(
        cx.debug_bounds("refs-header").is_none(),
        "no references inside the feed"
    );
}

/// Several hundred characters with no newline: a referencing block that must wrap.
const LONG: &str = "A deliberately long note that keeps going without any newline so that \
    it has to wrap at the readable column: the beta needs twenty testers invited by hand, \
    the import path has to stay unchanged so that old files still open, and the migration \
    notes must reach the reviewer before the end of the month so that nothing slips.";

/// Like `setup`, but the store first gets a long block on "Source" that links to the
/// "Target" page or, with `daily`, to today.
fn setup_refs(
    cx: &mut TestAppContext,
    daily: bool,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    let link = if daily {
        "[[November 2nd, 2026]]"
    } else {
        "[[Target]]"
    };
    store
        .set_block_text(&src, &b, &format!("{LONG} {link}"), None, None)
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    (cx, ws, target)
}

/// The first Linked References hit drawn (its list index depends on what sits above it).
/// Each test opens a fresh window, so no stale bounds from an earlier frame can match.
fn first_hit(cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.run_until_parked();
    (0..40)
        .find_map(|ix| {
            // `debug_bounds` takes a `&'static str`; leaking a few test strings is fine.
            let sel: &'static str = Box::leak(format!("hit-{ix}").into_boxed_str());
            cx.debug_bounds(sel)
        })
        .expect("a linked reference was rendered")
}

/// The readable column (760 px, centred) inside the main pane.
fn column(cx: &mut VisualTestContext) -> (f32, f32) {
    let main = bounds(cx, "main-pane");
    let width = f32::from(main.size.width).min(760.);
    let left = f32::from(main.origin.x) + (f32::from(main.size.width) - width) / 2.;
    (left, left + width)
}

fn assert_wraps_within(hit: Bounds<Pixels>, left: f32, right: f32) {
    let (x0, x1) = (
        f32::from(hit.origin.x),
        f32::from(hit.origin.x + hit.size.width),
    );
    assert!(
        x0 >= left && x1 <= right + 0.5,
        "the reference stays inside [{left}, {right}]: {hit:?}"
    );
    assert!(
        hit.size.height >= px(40.),
        "long text wraps onto several lines instead of running wide: {hit:?}"
    );
}

/// FR-3.1: a long referencing block wraps at the page's readable width, like the outline
/// above it, instead of running past the column (found by screenshot).
#[gpui::test]
fn linked_reference_wraps_at_readable_width(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    let hit = first_hit(cx);
    let (left, right) = column(cx);
    assert_wraps_within(hit, left, right);
}

/// FR-1.2: each day in the Daily Notes feed shows its Linked References, wrapped the same way.
#[gpui::test]
fn feed_linked_reference_wraps_at_readable_width(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_refs(cx, true);
    let hit = first_hit(cx);
    let (left, right) = column(cx);
    assert_wraps_within(hit, left, right);
}

/// §9.2: a page shift-opened in the right sidebar wraps its references at the sidebar's edge.
#[gpui::test]
fn sidebar_linked_reference_wraps(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, true, window, cx)
    });
    let hit = first_hit(cx);
    let side = bounds(cx, "right-sidebar");
    let left = f32::from(side.origin.x);
    assert_wraps_within(hit, left, left + f32::from(side.size.width));
}

/// FR-3.9: a long unlinked mention wraps too, keeping its "Link" button in the column.
#[gpui::test]
fn unlinked_mention_wraps_and_keeps_link_in_view(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    let app = ws.update(cx, |w, _| w.app().clone());
    app.update(cx, |app, _| {
        let p = app.store.create_page("Elsewhere").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, &format!("{LONG} target"), None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    let header = bounds(cx, "unlinked-header");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let (link, row) = (0..40)
        .find_map(|ix| {
            let link: &'static str = Box::leak(format!("link-{ix}").into_boxed_str());
            let row: &'static str = Box::leak(format!("mention-{ix}").into_boxed_str());
            Some((cx.debug_bounds(link)?, cx.debug_bounds(row)?))
        })
        .expect("a mention with a Link button was rendered");
    let (left, right) = column(cx);
    assert!(
        f32::from(link.origin.x + link.size.width) <= right + 0.5,
        "Link stays inside the column ({left}, {right}): {link:?}"
    );
    assert_wraps_within(row, left, right);
}

#[gpui::test]
fn left_sidebar_collapses_to_a_rail(cx: &mut TestAppContext) {
    let cx = setup(cx);
    // FR-1.3's date picker is one click away, not always open.
    assert!(cx.debug_bounds("left-calendar").is_none());
    // (Debug bounds of elements no longer drawn linger, so check what replaced them.)
    cx.simulate_keystrokes("escape cmd-\\");
    let rail = bounds(cx, "left-rail");
    let main = bounds(cx, "main-pane");
    assert!(approx(rail.size.width, 32.), "{rail:?}");
    assert!(
        approx(main.origin.x, 32.) && approx(main.size.width, W - 32.),
        "{main:?}"
    );
    cx.simulate_keystrokes("cmd-\\");
    let main = bounds(cx, "main-pane");
    assert!(approx(main.origin.x, 250.), "{main:?}");
    let toggle = bounds(cx, "toggle-calendar");
    cx.simulate_click(toggle.center(), gpui::Modifiers::none());
    let cal = bounds(cx, "left-calendar");
    assert!(cal.origin.y > toggle.origin.y);
}

/// ALL-3: "Delete page" is a danger item inside the page header's menu, not loose text.
#[gpui::test]
fn page_menu_holds_the_danger_delete(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    // An empty day is a draft until its first write (FR-1.2); write one so it has a page
    // with actions to offer.
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    assert!(
        cx.debug_bounds("page-menu-delete").is_none(),
        "the menu starts closed"
    );
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    let popup = bounds(cx, "page-menu-popup");
    let delete = bounds(cx, "page-menu-delete");
    assert!(
        popup.origin.y >= menu.origin.y + menu.size.height - px(1.),
        "the menu opens below its control: {popup:?} {menu:?}"
    );
    assert!(
        delete.origin.x >= popup.origin.x && delete.origin.x + delete.size.width <= popup.right(),
        "the danger item stays inside the menu: {delete:?} {popup:?}"
    );
}

/// Rule 6: a daily page's title is display-only, so its menu must not offer Rename (which
/// `start_rename` turns into the calendar on daily pages).
#[gpui::test]
fn daily_page_menu_does_not_offer_rename(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("page-menu-delete").is_some(),
        "the daily page still offers Delete page"
    );
    assert!(
        cx.debug_bounds("page-menu-rename").is_none(),
        "daily titles are display-only: no Rename action"
    );
}

/// ALL-1: page title 28, H1 20, body 15 — measured as line boxes, since the heading used to
/// draw at 24 px (larger than H1 and close to the title).
#[gpui::test]
fn type_scale_caps_outline_headings_at_h1(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("body");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Heading /head");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input(" one");
    cx.simulate_keystrokes("escape");
    let title = bounds(cx, "page-title");
    let body = bounds(cx, "content-1");
    let h1 = bounds(cx, "content-2");
    assert!(
        title.size.height > h1.size.height,
        "the title outranks H1: {title:?} {h1:?}"
    );
    assert!(
        h1.size.height > body.size.height,
        "H1 outranks body: {h1:?} {body:?}"
    );
    assert!(
        h1.size.height <= px(utopia_ui::theme::H1_SIZE + 8.),
        "H1 is 20 px on a 28 px line, never the old 24 px: {h1:?}"
    );
}

/// ALL-4: the block being edited carries a 2 px accent bar at its left edge.
#[gpui::test]
fn focused_block_has_an_accent_bar(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("focused");
    let bar = bounds(cx, "focus-bar-1");
    let content = bounds(cx, "content-1");
    assert!(approx(bar.size.width, 2.), "2 px wide: {bar:?}");
    assert!(
        bar.origin.x < content.origin.x,
        "the bar sits left of the block text: {bar:?} {content:?}"
    );
}

/// ALL-5: Compact shrinks the body and adds 4 px between blocks; both are measurable, and
/// the setting applies to the open window without a restart.
#[gpui::test]
fn compact_density_is_tighter(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("one");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("two");
    cx.simulate_keystrokes("escape");
    let body_before = bounds(cx, "content-1").size.height;
    let row_before = bounds(cx, "row-1").size.height;

    let dir = tempfile::tempdir().unwrap();
    cx.update(|_window, cx| {
        utopia_ui::appearance::set_density(cx, utopia_ui::appearance::Density::Compact, dir.path())
            .unwrap();
    });
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("ui-density-compact").is_some(),
        "the open window re-rendered at the new density"
    );
    let body_after = bounds(cx, "content-1").size.height;
    let row_after = bounds(cx, "row-1").size.height;
    assert!(
        approx(body_after, 18.),
        "Compact body is 13.5 px on an 18 px line: {body_after:?}"
    );
    assert!(
        body_after < body_before && row_after < row_before,
        "Compact is tighter: body {body_before:?} -> {body_after:?}, \
         row {row_before:?} -> {row_after:?}"
    );
    assert!(
        approx(row_after - body_after, 4.),
        "4 px of block spacing: row {row_after:?}, body {body_after:?}"
    );
}

/// A fresh page window for one menu state, so no debug bounds from the other state linger.
fn page_title_with_menu(cx: &mut TestAppContext, menu_open: bool) -> Bounds<Pixels> {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Menu Target").unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    if menu_open {
        let menu = cx
            .debug_bounds("page-menu")
            .expect("the page menu button is rendered");
        cx.simulate_click(menu.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("page-menu-popup").is_some(),
            "the menu opened"
        );
    }
    cx.debug_bounds("page-title")
        .expect("the page title is rendered")
}

/// ALL-3: the page menu floats over the page. Opening it must not push the title down.
#[gpui::test]
fn opening_the_page_menu_does_not_move_the_title(cx: &mut TestAppContext) {
    let closed = page_title_with_menu(cx, false);
    let open = page_title_with_menu(cx, true);
    assert_eq!(
        closed, open,
        "the title keeps its bounds when the menu opens"
    );
}

/// SB-1: ten days are ten one-line rows, each with a muted block count. At the old
/// two-preview height three days took half the sidebar; these ten fit without scrolling.
#[gpui::test]
fn recent_days_are_one_line_each(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| seed_days(s, 10));
    cx.simulate_keystrokes("escape");
    let sidebar = bounds(cx, "left-sidebar");
    let mut previous: Option<Bounds<Pixels>> = None;
    for i in 0..10 {
        let row = bounds_of(cx, format!("recent-day-{i}"));
        assert!(
            row.size.height <= px(22.),
            "row {i} is one line, not a preview block: {row:?}"
        );
        let count = bounds_of(cx, format!("recent-day-count-{i}"));
        assert!(
            count.origin.x > row.origin.x + row.size.width / 2.
                && count.origin.x + count.size.width <= row.origin.x + row.size.width + px(0.5),
            "the count sits at the right edge of row {i}: {count:?} {row:?}"
        );
        if let Some(p) = previous {
            assert!(
                row.origin.y >= p.origin.y + p.size.height - px(1.),
                "rows stack: {p:?} {row:?}"
            );
        }
        previous = Some(row);
    }
    let last = previous.expect("ten rows");
    assert!(
        last.origin.y + last.size.height <= sidebar.origin.y + sidebar.size.height,
        "ten compact days fit in the sidebar without scrolling: {last:?} {sidebar:?}"
    );
}

/// SB-1: a compact row keeps the day's first block one hover away.
#[gpui::test]
fn recent_day_hover_shows_its_first_block(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| seed_days(s, 3));
    cx.simulate_keystrokes("escape");
    let row = bounds_of(cx, "recent-day-0".into());
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
    // gpui waits 500 ms before showing a tooltip.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(700));
    cx.run_until_parked();
    let tip = bounds_of(cx, "recent-day-preview".into());
    assert!(
        tip.size.width > px(40.) && tip.size.height >= px(12.),
        "the first block is drawn as a tooltip: {tip:?}"
    );
}

/// SB-2: a page that only exists because of a `#tag` gets a `#` in the tag colour;
/// a page reached through a `[[link]]` does not. One list, display only (FR-2.15).
#[gpui::test]
fn tag_pages_get_a_hash_glyph(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        let (p, b) = s.open_daily(today()).unwrap();
        s.set_block_text(&p, &b, "#atlas and [[Atlas Launch]]", None, None)
            .unwrap();
        s.commit_block(&p, &b).unwrap();
    });
    cx.simulate_keystrokes("escape");
    let glyph = bounds_of(cx, "tag-glyph-atlas".into());
    let row = bounds_of(cx, "page-row-0".into());
    assert!(
        glyph.origin.x >= row.origin.x
            && glyph.origin.x + glyph.size.width <= row.origin.x + row.size.width + px(0.5),
        "the glyph sits inside the page row: {glyph:?} {row:?}"
    );
    assert!(
        cx.debug_bounds("tag-glyph-atlas launch").is_none(),
        "a [[link]] page is not marked as a tag page"
    );
}

/// SB-3: a page pinned from its "···" menu sits in a Pinned group above Pages, the pin is
/// written to settings.json as local state (rule 9), and the row's ✕ unpins it without
/// opening the page behind the click.
#[gpui::test]
fn pinned_pages_sit_above_pages_and_are_local(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        s.create_page("Alpha").unwrap();
        s.create_page("Beta").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_, cx| utopia_ui::appearance::install(cx, dir.path()));
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let alpha = PageId::for_title_key("alpha");
    let beta = PageId::for_title_key("beta");
    cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open(alpha.clone(), None, None, false, window, cx)
        })
    });
    cx.run_until_parked();
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    let pin = bounds(cx, "page-menu-pin");
    cx.simulate_click(pin.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    let pinned_header = bounds(cx, "pinned-header");
    let pages_header = bounds(cx, "pages-header");
    let row = bounds_of(cx, "pinned-row-0".into());
    assert!(
        pinned_header.origin.y < pages_header.origin.y,
        "the Pinned group is above Pages: {pinned_header:?} {pages_header:?}"
    );
    assert!(
        row.origin.y >= pinned_header.origin.y
            && row.origin.y + row.size.height <= pages_header.origin.y + px(0.5),
        "the pinned row sits between the two headers: {row:?}"
    );
    let saved = || -> serde_json::Value {
        let s = std::fs::read_to_string(dir.path().join("settings.json")).expect("settings.json");
        serde_json::from_str(&s).expect("settings.json is JSON")
    };
    assert_eq!(saved()["pinned"], serde_json::json!([alpha.as_str()]));
    assert!(
        !dir.path().join("keymap.json").exists(),
        "a pin is not a keybinding"
    );
    // A new launch reads the pin back from the file.
    cx.update(|_, cx| {
        utopia_ui::appearance::install(cx, dir.path());
        assert_eq!(utopia_ui::appearance::pinned(cx), [alpha.clone()]);
    });

    // Show another page, then unpin Alpha with the row's ✕. The click must not open Alpha.
    cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open(beta.clone(), None, None, false, window, cx)
        })
    });
    cx.run_until_parked();
    let before = pages_header.origin.y;
    let unpin = bounds_of(cx, "unpin-0".into());
    cx.simulate_click(unpin.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(beta),
        "unpinning does not follow the row's link"
    );
    assert_eq!(saved()["pinned"], serde_json::json!([]));
    let pages_header = bounds(cx, "pages-header");
    assert!(
        pages_header.origin.y < before,
        "Pages moves up once the Pinned group is gone: {pages_header:?}"
    );
}

/// SB-4: the footer is one row — the graph chip, the sync dot and (for an encrypted graph)
/// a lock — at the bottom of the sidebar. Clicking it opens the sync view as an overlay
/// above the footer; nothing in the page moves.
#[gpui::test]
fn footer_opens_the_sync_view_without_moving_the_page(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    let sidebar = bounds(cx, "left-sidebar");
    let footer = bounds(cx, "sidebar-footer");
    let chip = bounds(cx, "graph-chip");
    let dot = bounds(cx, "sync-dot-off");
    assert!(
        footer.origin.y + footer.size.height <= sidebar.origin.y + sidebar.size.height + px(0.5),
        "the footer is inside the sidebar: {footer:?} {sidebar:?}"
    );
    assert!(
        footer.origin.y > sidebar.origin.y + sidebar.size.height / 2.,
        "the footer sits at the bottom: {footer:?} {sidebar:?}"
    );
    // One row: the chip and the dot share a centre line.
    let chip_mid = f32::from(chip.origin.y) + f32::from(chip.size.height) / 2.;
    let dot_mid = f32::from(dot.origin.y) + f32::from(dot.size.height) / 2.;
    assert!(
        (chip_mid - dot_mid).abs() <= 2.,
        "the chip and the dot are on one row: {chip:?} {dot:?}"
    );
    assert!(
        approx(chip.origin.x, f32::from(footer.origin.x))
            && f32::from(dot.origin.x + dot.size.width) <= f32::from(footer.right()),
        "chip left, dot right, both inside the row: {chip:?} {dot:?} {footer:?}"
    );
    assert!(
        cx.debug_bounds("encryption-lock").is_none(),
        "an in-memory graph is not encrypted"
    );

    let main = bounds(cx, "main-pane");
    cx.simulate_click(footer.center(), gpui::Modifiers::default());
    let popover = bounds(cx, "sync-popover");
    assert!(
        f32::from(popover.origin.x) >= f32::from(sidebar.origin.x)
            && f32::from(popover.right()) <= f32::from(sidebar.right()) + 0.5,
        "the sync view stays inside the sidebar: {popover:?} {sidebar:?}"
    );
    assert!(
        f32::from(popover.bottom()) <= f32::from(footer.origin.y) + 0.5,
        "the sync view floats above the footer: {popover:?} {footer:?}"
    );
    let after = bounds(cx, "main-pane");
    assert!(
        approx(after.origin.x, f32::from(main.origin.x))
            && approx(after.size.width, f32::from(main.size.width)),
        "opening the sync view does not move the page: {main:?} {after:?}"
    );

    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    assert!(ws.update(cx, |w, _| w.sync_view_open()));
    cx.simulate_click(footer.center(), gpui::Modifiers::default());
    assert!(
        !ws.update(cx, |w, _| w.sync_view_open()),
        "clicking the footer again closes the sync view"
    );
}

/// SB-5: under Journal, a quiet 7-column month strip. Days with an entry get a dot, today
/// is drawn in the accent, a day click opens that day, and the month name folds the grid.
#[gpui::test]
fn month_strip_is_a_quiet_seven_column_grid(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| seed_days(s, 3));
    cx.simulate_keystrokes("escape");
    assert!(
        cx.debug_bounds("left-calendar").is_none(),
        "the strip starts folded, as the calendar did"
    );
    let strip = bounds(cx, "toggle-calendar");
    cx.simulate_click(strip.center(), gpui::Modifiers::default());
    let grid = bounds(cx, "left-calendar");
    assert!(
        grid.origin.y >= strip.origin.y + strip.size.height - px(0.5),
        "the grid is under the month name: {grid:?} {strip:?}"
    );

    // November 2026 starts on a Sunday, so the first Monday-first row is Oct 26–Nov 1.
    let row = [
        "2026-10-26",
        "2026-10-27",
        "2026-10-28",
        "2026-10-29",
        "2026-10-30",
        "2026-10-31",
        "2026-11-01",
    ];
    let cells: Vec<Bounds<Pixels>> = row
        .iter()
        .map(|iso| bounds_of(cx, format!("cal-{iso}")))
        .collect();
    for (i, cell) in cells.iter().enumerate() {
        assert!(
            approx(cell.origin.y, f32::from(cells[0].origin.y)),
            "seven columns share a line: {cell:?}"
        );
        assert!(
            approx(cell.size.width, f32::from(cells[0].size.width)),
            "columns are equally wide: {cell:?}"
        );
        if i > 0 {
            assert!(
                cell.origin.x > cells[i - 1].origin.x,
                "columns go left to right: {cell:?}"
            );
        }
        assert!(
            cell.origin.x >= grid.origin.x
                && cell.origin.x + cell.size.width <= grid.origin.x + grid.size.width + px(0.5),
            "cell {i} stays inside the grid: {cell:?} {grid:?}"
        );
    }

    // Seeded days (Nov 2, Nov 1, Oct 31) have dots; an untouched day in the month does not.
    for iso in ["2026-11-02", "2026-11-01", "2026-10-31"] {
        bounds_of(cx, format!("cal-dot-{iso}"));
    }
    assert!(
        cx.debug_bounds("cal-dot-2026-11-15").is_none(),
        "a day with no entry has no dot"
    );
    // Today is inside the grid it belongs to.
    let today = bounds_of(cx, "cal-2026-11-02".into());
    assert!(
        today.origin.x >= grid.origin.x && today.right() <= grid.right() + px(0.5),
        "today's cell is in the strip: {today:?} {grid:?}"
    );

    // Folding and unfolding is one click on the month name; the list below moves with it.
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let recent_before = bounds(cx, "recent-days-header");
    cx.simulate_click(strip.center(), gpui::Modifiers::default());
    assert!(!ws.update(cx, |w, _| w.calendar_open()));
    let recent = bounds(cx, "recent-days-header");
    assert!(
        recent.origin.y < recent_before.origin.y,
        "the folded month gives the list its room back: {recent:?} {recent_before:?}"
    );
    cx.simulate_click(strip.center(), gpui::Modifiers::default());
    assert!(ws.update(cx, |w, _| w.calendar_open()));

    // A day cell opens that day (a draft until written, FR-1.2).
    let cell = bounds_of(cx, "cal-2026-10-30".into());
    cx.simulate_click(cell.center(), gpui::Modifiers::default());
    let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(PageId::for_date(day)));
}
