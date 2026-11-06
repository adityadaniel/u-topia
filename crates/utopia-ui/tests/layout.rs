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
    // SR-5: the one palette is wide enough for the preview; Cmd+K opens it on pages.
    assert!(approx(p.size.width, 860.));
    let center = f32::from(p.origin.x) + 430.;
    assert!((center - W / 2.).abs() < 2., "centered horizontally: {p:?}");
    assert!(
        p.origin.y > px(60.) && p.origin.y < px(H / 2.),
        "in the upper half: {p:?}"
    );
}

/// SR-4: search widens to 860 px and the preview fills the right half, beside the list
/// rather than over it.
#[gpui::test]
fn search_preview_sits_in_the_right_half(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("zebra crossing");
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    let palette = bounds(cx, "jump-palette");
    assert!(
        approx(palette.size.width, 860.),
        "the search dialog is about 860 px wide: {palette:?}"
    );
    let results = bounds(cx, "jump-results");
    let preview = bounds(cx, "search-preview");
    assert!(
        f32::from(preview.origin.x) >= f32::from(palette.origin.x) + 430.,
        "the preview starts in the right half: {preview:?} {palette:?}"
    );
    assert!(
        preview.origin.x >= results.origin.x + results.size.width - px(0.5),
        "the list ends before the preview: {results:?} {preview:?}"
    );
    assert!(
        preview.right() <= palette.right() + px(0.5),
        "the preview stays inside the dialog: {preview:?} {palette:?}"
    );
    assert!(
        results.size.width >= px(300.),
        "the list keeps usable width: {results:?}"
    );
}

/// SR-3: the scope chips sit under the field inside the palette, and the "Any time" menu
/// floats over the results without moving them (deferred + anchored, like the page menu).
#[gpui::test]
fn search_scope_row_and_age_menu(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-shift-f");
    let palette = bounds(cx, "jump-palette");
    let scopes = bounds(cx, "search-scopes");
    assert!(
        scopes.origin.y > palette.origin.y + px(60.),
        "under the field: {scopes:?} {palette:?}"
    );
    assert!(
        scopes.origin.x >= palette.origin.x && scopes.right() <= palette.right() + px(0.5),
        "inside the palette: {scopes:?} {palette:?}"
    );
    let age = bounds(cx, "search-age");
    assert!(
        age.right() <= palette.right() + px(0.5),
        "the age control stays inside: {age:?}"
    );
    cx.simulate_click(age.center(), gpui::Modifiers::default());
    let popup = bounds(cx, "search-age-popup");
    assert!(
        popup.origin.y >= age.origin.y + age.size.height - px(1.),
        "the menu opens below its control: {popup:?} {age:?}"
    );
    assert!(
        popup.origin.x >= palette.origin.x
            && popup.right() <= px(W)
            && popup.origin.y + popup.size.height <= px(H),
        "the menu stays in the window: {popup:?}"
    );
    let after = bounds(cx, "search-scopes");
    assert_eq!(scopes, after, "opening the menu does not move the chips");
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
