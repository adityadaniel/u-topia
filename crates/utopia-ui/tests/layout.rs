//! Layout geometry (§9.2) checked on GPUI's test platform: the window is laid out and
//! painted headlessly and element bounds are read back by `debug_selector`.

use chrono::NaiveDate;
use gpui::{px, size, AppContext as _, Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
use utopia_core::PageId;
use utopia_store::Store;
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::AppState;
use utopia_ui::theme::{Theme, INDENT};
use utopia_ui::workspace::Workspace;

const W: f32 = 1280.;
const H: f32 = 820.;

fn setup(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx
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

/// The list index of the first Linked References hit drawn.
fn first_hit_ix(cx: &mut VisualTestContext) -> usize {
    cx.run_until_parked();
    (0..40)
        .find(|ix| {
            let sel: &'static str = Box::leak(format!("hit-{ix}").into_boxed_str());
            cx.debug_bounds(sel).is_some()
        })
        .expect("a linked reference was rendered")
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

/// A graph whose "Target" page has two referencing blocks, one of them also mentioning "Ana
/// Lovelace", so the reference controls offer a co-reference chip with a count (LR-1).
fn setup_ref_controls(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    store.create_page("Ana Lovelace").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    store
        .set_block_text(
            &src,
            &b,
            "A note about [[Target]] with [[Ana Lovelace]]",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    let plan = utopia_core::outline::append_block(store.page_state(&src).unwrap(), None);
    let b2 = store.execute_plan(&src, plan, None).unwrap().unwrap().block;
    store
        .set_block_text(&src, &b2, "Another note about [[Target]] alone", None, None)
        .unwrap();
    store.commit_block(&src, &b2).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target)
}

/// LR-1: one control row — a 240 px filter field, then the date and sort menus as buttons —
/// and the co-reference chips (with their counts) wrapping on the line below it.
#[gpui::test]
fn reference_controls_are_one_row_with_chips_below(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let filter = bounds(cx, "refs-filter");
    let row = bounds(cx, "refs-control-row");
    let date = bounds(cx, "refs-date-menu-any-time");
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        approx(filter.size.width, 240.),
        "a 240 px field: {filter:?}"
    );
    assert!(
        approx(
            date.origin.y + date.size.height / 2.,
            f32::from(row.origin.y) + f32::from(row.size.height) / 2.
        ),
        "the date menu sits on the same row as the field: {date:?} {row:?}"
    );
    assert!(
        date.origin.x >= filter.origin.x + filter.size.width,
        "after the field: {date:?} {filter:?}"
    );
    assert!(sort.origin.x >= date.origin.x + date.size.width, "{sort:?}");
    // Both are quiet control buttons (ALL-3): the same compact height, not a form field.
    assert!(
        approx(sort.size.height, f32::from(date.size.height)) && sort.size.height <= px(34.),
        "compact menu buttons: {sort:?} {date:?}"
    );
    let chips = bounds(cx, "refs-chips");
    let chip = bounds(cx, "suggest-chip-0");
    assert!(
        chips.origin.y >= row.origin.y + row.size.height - px(0.5),
        "co-reference chips wrap onto the next line: {chips:?} {row:?}"
    );
    assert!(
        chip.origin.y >= row.origin.y + row.size.height - px(0.5),
        "a chip is below the control row: {chip:?}"
    );
}

/// ALL-3/LR-1: the filter menus float over the page. Opening one must not move the controls
/// or the references under them.
#[gpui::test]
fn a_reference_menu_floats_without_moving_the_layout(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let row_before = bounds(cx, "refs-control-row");
    let hit_before = first_hit(cx);
    assert!(
        cx.debug_bounds("refs-menu-popup").is_none(),
        "no menu until the control is clicked"
    );
    let date = bounds(cx, "refs-date-menu-any-time");
    cx.simulate_click(date.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let popup = bounds(cx, "refs-menu-popup");
    assert!(
        popup.origin.y >= date.origin.y + date.size.height - px(1.),
        "the menu opens below its control: {popup:?} {date:?}"
    );
    assert!(
        approx(
            row_before.origin.y,
            f32::from(bounds(cx, "refs-control-row").origin.y)
        ) && approx(hit_before.origin.y, f32::from(first_hit(cx).origin.y)),
        "opening the menu takes no space in the column"
    );
    // Picking a choice applies it (the label and its state change) and closes the menu.
    let option = bounds(cx, "refs-date-option-last-7-days");
    cx.simulate_click(option.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-date-menu-last-7-days");
    let sort = bounds(cx, "refs-sort-menu-title");
    cx.simulate_click(sort.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let recent = bounds(cx, "refs-sort-option-recent");
    cx.simulate_click(recent.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-sort-menu-recent");
}
/// ALL-3 follow-up: the page menu is drawn over the page, so it must also take the clicks
/// meant for it. Without `occlude` the "Delete page" item is visible but never fires (found
/// while building the reference menus).
#[gpui::test]
fn page_menu_items_take_their_clicks(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let delete = bounds(cx, "page-menu-delete");
    cx.simulate_click(delete.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let app = ws.update(cx, |w, _| w.app().clone());
    let deleted = app.update(cx, |app, _| {
        app.store
            .page_row(&target)
            .unwrap()
            .is_some_and(|r| r.deleted_at.is_some())
    });
    assert!(deleted, "the danger item deleted the page it belongs to");
}

/// The first `group-title-{ix}` and its count chip, by trying list indices.
fn first_group(cx: &mut VisualTestContext) -> (Bounds<Pixels>, Bounds<Pixels>) {
    cx.run_until_parked();
    (0..40)
        .find_map(|ix| {
            let title: &'static str = Box::leak(format!("group-title-{ix}").into_boxed_str());
            let title = cx.debug_bounds(title)?;
            // The count is in the selector, so try the counts a small graph can have.
            (1..=9).find_map(|n| {
                let count: &'static str =
                    Box::leak(format!("group-count-{ix}-{n}").into_boxed_str());
                Some((title, cx.debug_bounds(count)?))
            })
        })
        .expect("a referencing group was rendered")
}

/// LR-2: the count is a small chip right after the page or day name, not a number pushed to
/// the far edge of the row.
#[gpui::test]
fn reference_counts_sit_next_to_the_name(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let title = bounds(cx, "refs-title");
    let count = bounds(cx, "refs-count-2");
    let header = bounds(cx, "refs-header");
    assert!(
        count.origin.x >= title.origin.x + title.size.width - px(1.),
        "the count follows the title: {count:?} {title:?}"
    );
    assert!(
        count.origin.x - (title.origin.x + title.size.width) < px(16.),
        "and sits right next to it: {count:?} {title:?}"
    );
    assert!(
        approx(
            count.origin.y + count.size.height / 2.,
            f32::from(title.origin.y) + f32::from(title.size.height) / 2.
        ),
        "on the same line: {count:?} {title:?}"
    );
    assert!(
        count.origin.x + count.size.width < header.origin.x + header.size.width - px(200.),
        "not at the far edge: {count:?} {header:?}"
    );

    let (group, group_count) = first_group(cx);
    assert!(
        group_count.origin.x >= group.origin.x + group.size.width - px(1.),
        "the group's count follows its name: {group_count:?} {group:?}"
    );
    assert!(
        group_count.origin.x - (group.origin.x + group.size.width) < px(16.),
        "right next to it: {group_count:?} {group:?}"
    );
}

/// "Atlas Launch" → heading "Risks" → a block that links to "Target", so the reference has a
/// path above it.
fn setup_nested_refs(
    cx: &mut TestAppContext,
) -> (
    &mut VisualTestContext,
    Entity<Workspace>,
    PageId,
    Vec<utopia_core::BlockId>,
) {
    use utopia_core::{BlockKind, Command};
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    let atlas = store.create_page("Atlas Launch").unwrap();
    let risks = utopia_core::BlockId::new();
    let risky = utopia_core::BlockId::new();
    let mut cmds = vec![
        Command::InsertBlock {
            id: risks.clone(),
            parent: None,
            after: None,
            text: "Risks".into(),
            kind: BlockKind::Heading(2),
        },
        Command::InsertBlock {
            id: risky.clone(),
            parent: Some(risks.clone()),
            after: None,
            text: "The beta date depends on [[Target]]".into(),
            kind: BlockKind::Bullet,
        },
    ];
    // Four children under the referencing block, for the inline-children limit (LR-4).
    let mut after = None;
    for n in 1..=4 {
        let id = utopia_core::BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: Some(risky.clone()),
            after: after.clone(),
            text: format!("Reason {n}"),
            kind: BlockKind::Bullet,
        });
        after = Some(id);
    }
    store.execute(&atlas, cmds, Default::default()).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target, vec![risks, risky])
}

/// LR-3: a referencing block nested under "Risks" shows "Atlas Launch › Risks" as one caption
/// line above it, and each part is its own click target.
#[gpui::test]
fn reference_shows_the_full_path_above_the_block(cx: &mut TestAppContext) {
    let (cx, _, _, _) = setup_nested_refs(cx);
    let hit = first_hit(cx);
    cx.run_until_parked();
    let (page, ancestor) = (0..40)
        .find_map(|ix| {
            let page: &'static str = Box::leak(format!("hit-path-part-{ix}-0").into_boxed_str());
            let ancestor: &'static str =
                Box::leak(format!("hit-path-part-{ix}-1").into_boxed_str());
            Some((cx.debug_bounds(page)?, cx.debug_bounds(ancestor)?))
        })
        .expect("a path above the reference");
    let path = cx
        .debug_bounds("hit-path-7")
        .or_else(|| {
            (0..40).find_map(|ix| {
                let sel: &'static str = Box::leak(format!("hit-path-{ix}").into_boxed_str());
                cx.debug_bounds(sel)
            })
        })
        .expect("a path line was rendered");
    assert!(
        path.origin.y + path.size.height <= hit.origin.y + px(0.5),
        "the path sits above the block: {path:?} {hit:?}"
    );
    assert!(
        path.size.height <= px(24.),
        "one 12.5 px line, not a stacked crumb list: {path:?}"
    );
    assert!(
        ancestor.origin.x >= page.origin.x + page.size.width - px(1.),
        "the ancestor follows the page name: {ancestor:?} {page:?}"
    );
    assert!(
        approx(page.origin.y, f32::from(ancestor.origin.y)),
        "both parts on the same line: {page:?} {ancestor:?}"
    );
    let (left, right) = column(cx);
    assert!(
        f32::from(page.origin.x) >= left
            && f32::from(ancestor.origin.x + ancestor.size.width) <= right + 0.5,
        "the path stays inside the column: {path:?} ({left}, {right})"
    );
}

/// LR-4: a reference shows the first two children inline and folds the rest behind a
/// "+N more" chip, which opens them. The numbers come from the index, not the paint.
#[gpui::test]
fn reference_children_are_inline_up_to_the_limit(cx: &mut TestAppContext) {
    let (cx, _, _, _) = setup_nested_refs(cx);
    let ix = first_hit_ix(cx);
    let first: &'static str = Box::leak(format!("hit-child-{ix}-0").into_boxed_str());
    let second: &'static str = Box::leak(format!("hit-child-{ix}-1").into_boxed_str());
    let third: &'static str = Box::leak(format!("hit-child-{ix}-2").into_boxed_str());
    let hit = first_hit(cx);
    let first = bounds(cx, first);
    let second = bounds(cx, second);
    assert!(
        first.origin.y >= hit.origin.y + hit.size.height - px(0.5),
        "children sit under their block: {first:?} {hit:?}"
    );
    assert!(
        second.origin.y > first.origin.y,
        "and stack in outline order: {first:?} {second:?}"
    );
    assert!(
        first.origin.x >= hit.origin.x - px(0.5) && first.origin.x <= hit.origin.x + px(20.),
        "children line up under the block's text, not its bullet: {first:?} {hit:?}"
    );
    assert!(
        cx.debug_bounds(third).is_none(),
        "only two are shown by default"
    );
    let more: &'static str = Box::leak(format!("hit-more-{ix}-2").into_boxed_str());
    let chip = bounds(cx, more);
    assert!(
        chip.origin.y >= second.origin.y + second.size.height - px(0.5),
        "the chip follows the shown children: {chip:?} {second:?}"
    );
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let third = bounds(cx, third);
    assert!(
        third.origin.y >= second.origin.y + second.size.height - px(0.5),
        "the chip opened the rest, in order: {third:?} {second:?}"
    );
}

/// LR-1: clicking a co-reference chip narrows the references and the choice becomes a filled
/// chip on the same line; clicking it again widens them back.
#[gpui::test]
fn selecting_a_co_reference_chip_filters_and_fills(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    assert!(
        cx.debug_bounds("filter-chip-0").is_none(),
        "nothing is selected yet"
    );
    let chip = bounds(cx, "suggest-chip-0");
    let chips = bounds(cx, "refs-chips");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let selected = bounds(cx, "filter-chip-0");
    assert!(
        selected.origin.y >= chips.origin.y - px(0.5),
        "the selected chip joins the chips line: {selected:?} {chips:?}"
    );
    // One of the two references also mentions Ana, so the list narrowed to one.
    bounds(cx, "refs-count-1");
    let selected = bounds(cx, "filter-chip-0");
    cx.simulate_click(selected.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-count-2");
}

/// LR-1 review fix: the reference controls also render in the 380 px right sidebar. The
/// filter field keeps 240 px as its preferred width but must give way instead of pushing
/// the menu buttons past the pane.
#[gpui::test]
fn reference_controls_fit_the_sidebar_pane(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_ref_controls(cx);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, true, window, cx)
    });
    let side = bounds(cx, "right-sidebar");
    let row = bounds(cx, "refs-control-row");
    assert!(
        row.origin.x >= side.origin.x && row.origin.x + row.size.width <= side.right() + px(0.5),
        "the control row stays inside the sidebar: {row:?} {side:?}"
    );
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        sort.origin.x + sort.size.width <= side.right() + px(0.5),
        "the sort menu is still reachable: {sort:?} {side:?}"
    );
    assert!(
        approx(
            row.size.height,
            f32::from(bounds(cx, "refs-date-menu-any-time").size.height)
        ),
        "the controls stay on one line instead of wrapping: {row:?}"
    );
}

/// LR-1 review fix: a split pane can be narrower than the sidebar. The row must fit there
/// too, with all three controls still on it.
#[gpui::test]
fn reference_controls_fit_a_narrow_split_pane(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_ref_controls(cx);
    cx.simulate_resize(size(px(900.), px(700.)));
    ws.update_in(cx, |w, window, cx| w.open_split(target, window, cx));
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    let split = bounds(cx, "split-0");
    let row = bounds(cx, "refs-control-row");
    assert!(
        row.origin.x >= split.origin.x && row.origin.x + row.size.width <= split.right() + px(0.5),
        "the control row stays inside the split: {row:?} {split:?}"
    );
    let filter = bounds(cx, "refs-filter");
    assert!(
        filter.size.width < px(240.),
        "the field shrank to make room: {filter:?} {split:?}"
    );
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        sort.origin.x + sort.size.width <= split.right() + px(0.5),
        "{sort:?} {split:?}"
    );
}

/// A graph where one hit mentions "Ana Lovelace" twice and another once: the co-reference
/// chip must count the two hits it selects, not the three occurrences of the link (LR-1).
fn setup_ref_token_counts(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    store.create_page("Ana Lovelace").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    store
        .set_block_text(
            &src,
            &b,
            "[[Target]] with [[Ana Lovelace]] and [[Ana Lovelace]] again",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    let plan = utopia_core::outline::append_block(store.page_state(&src).unwrap(), None);
    let b2 = store.execute_plan(&src, plan, None).unwrap().unwrap().block;
    store
        .set_block_text(
            &src,
            &b2,
            "another [[Target]] with [[Ana Lovelace]]",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b2).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target)
}

/// LR-1 review fix: a co-reference chip counts the distinct hits it would leave, exactly
/// like the filter it applies, instead of every occurrence of the link in the text.
#[gpui::test]
fn co_reference_counts_match_the_hits_they_select(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_token_counts(cx);
    // Two hits mention Ana; one of them mentions her twice. The chip says 2, not 3.
    bounds(cx, "suggest-count-0-2");
    let chip = bounds(cx, "suggest-chip-0");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-count-2");
}
