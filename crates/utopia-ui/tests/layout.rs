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
