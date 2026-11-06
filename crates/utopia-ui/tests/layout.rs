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

/// ALL-1 / PG-3: page title 28, H1 20, H2 17, H3 15.5 — measured as line boxes, since the
/// heading used to draw at 24 px (larger than H1 and close to the title).
#[gpui::test]
fn type_scale_caps_outline_headings_at_h1(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("body");
    for level in 1..=3 {
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.simulate_input(&format!("h{level} /heading {level}"));
        cx.run_until_parked();
        // Enter takes the slash command; the next one splits the block.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let title = bounds(cx, "page-title");
    let body = bounds(cx, "content-1");
    let h1 = bounds(cx, "content-2");
    let h2 = bounds(cx, "content-3");
    let h3 = bounds(cx, "content-4");
    assert!(
        title.size.height > h1.size.height,
        "the title outranks H1: {title:?} {h1:?}"
    );
    assert!(
        h1.size.height > h2.size.height && h2.size.height > h3.size.height,
        "the three heading steps descend: {h1:?} {h2:?} {h3:?}"
    );
    assert!(
        h1.size.height > body.size.height,
        "H1 outranks body: {h1:?} {body:?}"
    );
    assert!(
        h1.size.height <= px(utopia_ui::theme::H1_SIZE + 8.),
        "H1 is 20 px on a 28 px line, never the old 24 px: {h1:?}"
    );
    assert!(
        h3.size.height >= px(utopia_ui::theme::H3_SIZE),
        "H3 still holds its 15.5 px glyphs: {h3:?}"
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

/// A page whose outline holds a quote, then a code block that names its language (PG-5):
/// items are 0 header, 1 quote, 2 code.
fn setup_quote_and_code(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Vec<utopia_core::BlockId>) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Types").unwrap();
    // A quote, a code block that names its language, and a plain bullet to measure against.
    let rows = [
        (BlockKind::Quote, "Ship the small thing that works."),
        (BlockKind::Code, "rust\nfn main() {}"),
        (BlockKind::Bullet, "plain"),
    ];
    let mut ids = Vec::new();
    let mut cmds = Vec::new();
    let mut prev: Option<BlockId> = None;
    for (kind, text) in rows {
        let id = BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: prev.clone(),
            text: text.to_string(),
            kind,
        });
        prev = Some(id.clone());
        ids.push(id);
    }
    store
        .execute(
            &page,
            cmds,
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    (cx, ids)
}

/// PG-5: a quote is set off from the outline by its bar and padding, and a code block's
/// language sits at the top right of its card, above the code, not as a line of code.
#[gpui::test]
fn quote_is_indented_and_code_names_its_language(cx: &mut TestAppContext) {
    let (cx, ids) = setup_quote_and_code(cx);
    let text = |cx: &mut VisualTestContext, block: &utopia_core::BlockId| {
        // `debug_bounds` takes a `&'static str`; leaking a few test selectors is fine.
        let sel: &'static str = Box::leak(format!("text-{}", block.0).into_boxed_str());
        bounds(cx, sel)
    };
    let quote = text(cx, &ids[0]);
    let code = text(cx, &ids[1]);
    let bullet = text(cx, &ids[2]);
    let card = bounds(cx, "content-2");
    let lang = bounds(cx, "code-language-2");
    let row = bounds(cx, "row-2");
    // 2 px bar plus 8 px padding for the quote, the card's 8 px padding for the code.
    assert!(
        approx(quote.origin.x - bullet.origin.x, 10.),
        "the quote's bar and padding inset its text: {quote:?} {bullet:?}"
    );
    assert!(
        approx(code.origin.x - bullet.origin.x, 8.),
        "the code sits inside its card: {code:?} {bullet:?}"
    );
    assert!(
        lang.origin.y < code.origin.y,
        "the label is above the first line of code: {lang:?} {code:?}"
    );
    assert!(
        lang.origin.x + lang.size.width <= card.right() + px(1.),
        "the label stays inside the card: {lang:?} {card:?}"
    );
    assert!(
        card.right() - (lang.origin.x + lang.size.width) < px(12.),
        "the label is right-aligned in the card: {lang:?} {card:?}"
    );
    assert!(
        row.size.height >= card.size.height,
        "the card takes its own room in the outline: {row:?} {card:?}"
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

/// PG-4: the bullet reads as a drag handle only under the pointer, and a parent's collapse
/// arrow appears with it — but only for the row being hovered.
#[gpui::test]
fn block_affordances_appear_on_hover(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab escape");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("grip-1").is_none(),
        "a row shows no drag handle until the pointer is over it"
    );
    assert!(
        cx.debug_bounds("arrow-1").is_none(),
        "an expanded parent's arrow is quiet at rest"
    );

    let child = bounds(cx, "row-2");
    cx.simulate_mouse_move(child.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("grip-2").is_some(),
        "the hovered row shows its drag handle"
    );
    assert!(
        cx.debug_bounds("arrow-1").is_none(),
        "hovering one row does not reveal another row's arrow"
    );

    let parent = bounds(cx, "row-1");
    cx.simulate_mouse_move(parent.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();
    let grip = bounds(cx, "grip-1");
    let arrow = bounds(cx, "arrow-1");
    let content = bounds(cx, "content-1");
    assert!(
        arrow.origin.x < grip.origin.x && grip.origin.x < content.origin.x,
        "arrow, then the handle, then the text: {arrow:?} {grip:?} {content:?}"
    );
    assert!(
        grip.size.height > px(4.) && grip.size.height < px(14.),
        "the grip is a small drawn handle: {grip:?}"
    );
}

/// PG-4: the to-do box is a drawn 3 px-cornered control, and clicking it checks the to-do
/// (the box, not the font's ☐ glyph, is the target).
#[gpui::test]
fn todo_box_is_a_drawn_control(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("milk");
    cx.simulate_keystrokes("alt-enter escape");
    cx.run_until_parked();
    let boxed = bounds(cx, "todo-1");
    let content = bounds(cx, "content-1");
    assert!(
        approx(boxed.size.width, 13.) && approx(boxed.size.height, 13.),
        "a 13 px box: {boxed:?}"
    );
    assert!(
        boxed.origin.x < content.origin.x,
        "the box sits left of the text: {boxed:?} {content:?}"
    );
    cx.simulate_click(boxed.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        let first = st.live_roots().first().cloned().expect("one block");
        let b = st.block(&first).expect("the block");
        assert_eq!(b.kind, utopia_core::BlockKind::Todo, "still a to-do");
        assert!(b.checked, "clicking the box checked it");
    });
}

/// A page with one alias, opened on its own, for the header toolbar (PG-2).
fn setup_alias_page(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    store.add_alias(&page, "the board").unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    cx
}

/// PG-2: under the title sits one toolbar — alias chips, "+ alias", then the "···" menu —
/// and the menu's items run Open in graph, Export, Rename, Delete page last.
#[gpui::test]
fn page_toolbar_sits_under_the_title(cx: &mut TestAppContext) {
    let cx = setup_alias_page(cx);
    let title = bounds(cx, "page-title");
    let toolbar = bounds(cx, "page-toolbar");
    let alias = bounds(cx, "alias-0");
    let add = bounds(cx, "add-alias");
    let menu = bounds(cx, "page-menu");
    assert!(
        toolbar.origin.y >= title.origin.y + title.size.height - px(1.),
        "the toolbar follows the title: {title:?} {toolbar:?}"
    );
    assert!(
        alias.origin.x < add.origin.x && add.origin.x < menu.origin.x,
        "chips, then + alias, then the menu: {alias:?} {add:?} {menu:?}"
    );
    for (what, b) in [("alias", alias), ("+ alias", add), ("menu", menu)] {
        assert!(
            b.origin.y >= toolbar.origin.y - px(1.)
                && b.origin.y + b.size.height <= toolbar.bottom() + px(1.),
            "{what} sits in the toolbar row: {b:?} {toolbar:?}"
        );
    }

    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let opened = bounds(cx, "page-toolbar");
    assert_eq!(
        toolbar, opened,
        "opening the menu floats over the page; the toolbar stays put"
    );
    let graph = bounds(cx, "page-menu-graph");
    let export = bounds(cx, "page-menu-export");
    let rename = bounds(cx, "page-menu-rename");
    let delete = bounds(cx, "page-menu-delete");
    assert!(
        graph.origin.y < export.origin.y
            && export.origin.y < rename.origin.y
            && rename.origin.y < delete.origin.y,
        "menu order: {graph:?} {export:?} {rename:?} {delete:?}"
    );
    assert!(
        delete.origin.y > rename.origin.y + rename.size.height,
        "a hairline sets the danger item apart, last: {rename:?} {delete:?}"
    );
}

/// PG-2: "Open in graph" in the page menu shows the graph view with the page highlighted,
/// the same view Cmd+Shift+G opens.
#[gpui::test]
fn page_menu_opens_the_graph(cx: &mut TestAppContext) {
    let cx = setup_alias_page(cx);
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let item = bounds(cx, "page-menu-graph");
    cx.simulate_click(item.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_some(),
        "the graph view is showing"
    );
    assert!(
        cx.debug_bounds("graph-view").is_some(),
        "and it is drawn over the page"
    );
}

/// A page with two `key:: value` root blocks and one ordinary block (PG-1), with
/// `settings.json` living in `dir` so a toggle can be checked on disk.
fn setup_attribute_page<'a>(
    cx: &'a mut TestAppContext,
    dir: &std::path::Path,
) -> &'a mut VisualTestContext {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let path = dir.to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    let mut prev: Option<BlockId> = None;
    let mut cmds = Vec::new();
    for (kind, text) in [
        (BlockKind::Bullet, "status:: dogfooding"),
        (BlockKind::Bullet, "owner:: Ana"),
        (BlockKind::Bullet, "An ordinary note"),
    ] {
        let id = BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: prev.clone(),
            text: text.to_string(),
            kind,
        });
        prev = Some(id);
    }
    store
        .execute(
            &page,
            cmds,
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    cx
}

/// PG-1: an attribute block is one compact muted row, not a second full block under the
/// metadata header; the blocks themselves are not drawn until they are asked for.
#[gpui::test]
fn attributes_are_shown_once_as_compact_rows(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cx = setup_attribute_page(cx, dir.path());
    let first = bounds(cx, "attr-1");
    let second = bounds(cx, "attr-2");
    let note = bounds(cx, "content-3");
    assert!(
        first.origin.y < second.origin.y && second.origin.y < note.origin.y,
        "the attributes read as a compact list above the notes: {first:?} {second:?} {note:?}"
    );
    assert!(
        cx.debug_bounds("content-1").is_none(),
        "the attribute block is not also drawn as a full block"
    );
    assert!(
        first.size.height < px(24.),
        "a compact row, less than a full block line: {first:?}"
    );
    let toggle = bounds(cx, "attribute-blocks-toggle");
    let title = bounds(cx, "page-title");
    assert!(
        toggle.origin.y > title.origin.y + title.size.height,
        "the toggle belongs to the metadata header, under the title: {toggle:?} {title:?}"
    );
}

/// PG-1: the "Show as blocks" toggle brings the blocks back — it switches the preference,
/// applies at once and is written to settings.json (never into the graph, rule 9).
#[gpui::test]
fn show_as_blocks_reveals_the_blocks_and_is_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cx = setup_attribute_page(cx, dir.path());
    assert!(
        cx.update(|_, cx| utopia_ui::appearance::attribute_blocks(cx))
            == utopia_ui::appearance::AttributeBlocks::Compact,
        "the default keeps each attribute to one row"
    );
    let toggle = bounds(cx, "attribute-blocks-toggle");
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| utopia_ui::appearance::attribute_blocks(cx)),
        utopia_ui::appearance::AttributeBlocks::Blocks
    );
    let block = bounds(cx, "content-1");
    assert!(
        block.size.height >= px(20.),
        "the attribute's block is drawn in full again: {block:?}"
    );
    let saved = std::fs::read_to_string(dir.path().join("settings.json")).expect("saved");
    let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["attribute-blocks"], "blocks", "{saved}");
}

/// PG-1: a compact attribute row is a way into its block, not a dead end — clicking it
/// shows the blocks and puts the caret in the block, where keystrokes land.
#[gpui::test]
fn a_compact_attribute_row_opens_the_block_for_editing(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cx = setup_attribute_page(cx, dir.path());
    let attr = bounds(cx, "attr-1");
    cx.simulate_click(attr.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| utopia_ui::appearance::attribute_blocks(cx))
            == utopia_ui::appearance::AttributeBlocks::Blocks
    );
    assert!(
        cx.debug_bounds("focus-bar-1").is_some(),
        "the block's editor is in its row"
    );
    cx.simulate_input("!");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        let first = st.live_roots().first().cloned().expect("one block");
        assert_eq!(
            st.block(&first).expect("the block").text,
            "!status:: dogfooding",
            "typing went into the attribute's own block"
        );
    });
}

/// A page whose first root is an attribute with one child, followed by an ordinary note.
/// In Compact (the default) the attribute is one row and its child is hidden with it.
fn setup_attribute_tree<'a>(
    cx: &'a mut TestAppContext,
    dir: &std::path::Path,
    pref: utopia_ui::appearance::AttributeBlocks,
) -> (&'a mut VisualTestContext, Vec<utopia_core::BlockId>) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let path = dir.to_path_buf();
    let key = pref.key();
    std::fs::write(
        dir.join("settings.json"),
        format!("{{\"version\": 1, \"attribute-blocks\": \"{key}\"}}\n"),
    )
    .unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    let attr = BlockId::new();
    let child = BlockId::new();
    let note = BlockId::new();
    store
        .execute(
            &page,
            vec![
                Command::InsertBlock {
                    id: attr.clone(),
                    parent: None,
                    after: None,
                    text: "status:: x".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: child.clone(),
                    parent: Some(attr.clone()),
                    after: None,
                    text: "hidden child".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: note.clone(),
                    parent: None,
                    after: Some(attr.clone()),
                    text: "note".into(),
                    kind: BlockKind::Bullet,
                },
            ],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    (cx, vec![attr, child, note])
}

/// The texts of a page's live root blocks, in order.
fn root_texts(cx: &mut VisualTestContext) -> Vec<String> {
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        st.live_roots()
            .iter()
            .map(|b| st.block(b).expect("block").text.clone())
            .collect()
    })
}

/// A page's block text by id (any depth).
fn block_text(cx: &mut VisualTestContext, block: &utopia_core::BlockId) -> String {
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        st.block(block).map(|b| b.text.clone()).unwrap_or_default()
    })
}

/// Finding 1: switching to Compact rows while a descendant of an attribute is being edited
/// must keep that whole ancestor path drawn, or the editor loses its row and its keystrokes.
#[gpui::test]
fn switching_to_compact_keeps_the_edited_descendant_rendered(cx: &mut TestAppContext) {
    use utopia_ui::appearance::{self, AttributeBlocks};
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_attribute_tree(cx, dir.path(), AttributeBlocks::Blocks);
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(
            w.main_page(cx).expect("a page"),
            None,
            Some(ids[1].clone()),
            false,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    cx.simulate_input(">");
    assert_eq!(block_text(cx, &ids[1]), "hidden child>", "the editor works");

    // Settings → Appearance → Compact rows, while the child is still being edited.
    cx.update(|_, cx| {
        appearance::set_attribute_blocks(cx, AttributeBlocks::Compact).unwrap();
    });
    cx.run_until_parked();

    cx.simulate_input("!");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        block_text(cx, &ids[1]),
        "hidden child>!",
        "keystrokes still reach the edited block after the switch"
    );
    assert_eq!(
        block_text(cx, &ids[2]),
        "note",
        "and they went into the block being edited, not another"
    );
}

/// Finding 2: Backspace at the start of the block after a folded attribute merges into the
/// previous *displayed* row (the attribute), not into its hidden child (FR-2.5).
#[gpui::test]
fn backspace_merges_into_the_previous_visible_row(cx: &mut TestAppContext) {
    use utopia_ui::appearance::AttributeBlocks;
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_attribute_tree(cx, dir.path(), AttributeBlocks::Compact);
    let attr = bounds(cx, "attr-1");
    assert!(attr.size.height > px(0.));
    let note = bounds(cx, "content-2");
    // The caret at the start of "note": click just inside its left edge.
    cx.simulate_click(
        gpui::point(note.origin.x + px(1.), note.origin.y + px(8.)),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(
        block_text(cx, &ids[0]),
        "status:: xnote",
        "the note joined the previous visible row"
    );
    assert_eq!(
        block_text(cx, &ids[1]),
        "hidden child",
        "the hidden child under the attribute is untouched"
    );
}

/// Finding 3: opening a block through the public focus path (search, query, backlink) works
/// for a root attribute that Compact draws as one row.
#[gpui::test]
fn opening_a_compact_attribute_target_focuses_it(cx: &mut TestAppContext) {
    use utopia_ui::appearance::AttributeBlocks;
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_attribute_tree(cx, dir.path(), AttributeBlocks::Compact);
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(
            w.main_page(cx).expect("a page"),
            None,
            Some(ids[0].clone()),
            false,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    cx.simulate_input("!");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        block_text(cx, &ids[0]),
        "status:: x!",
        "the attribute block was focused and took the keystroke"
    );
}

/// Finding 3, the hidden case: a descendant of a compact attribute is revealed with it.
#[gpui::test]
fn opening_a_hidden_descendant_reveals_and_focuses_it(cx: &mut TestAppContext) {
    use utopia_ui::appearance::AttributeBlocks;
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_attribute_tree(cx, dir.path(), AttributeBlocks::Compact);
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(
            w.main_page(cx).expect("a page"),
            None,
            Some(ids[1].clone()),
            false,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    cx.simulate_input(">");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        block_text(cx, &ids[1]),
        "hidden child>",
        "the descendant under a compact attribute is reachable"
    );
}

/// Finding 4: on a page whose only block is a compact attribute, "Click to start writing"
/// must add a note, not type into the attribute's key.
#[gpui::test]
fn starting_a_note_on_an_attribute_only_page_appends_a_block(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    use utopia_ui::appearance::AttributeBlocks;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        "{\"version\": 1, \"attribute-blocks\": \"compact\"}\n",
    )
    .unwrap();
    let path = dir.path().to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    assert_eq!(
        cx.update(|cx| utopia_ui::appearance::attribute_blocks(cx)),
        AttributeBlocks::Compact
    );
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    let attr = BlockId::new();
    store
        .execute(
            &page,
            vec![Command::InsertBlock {
                id: attr.clone(),
                parent: None,
                after: None,
                text: "status:: x".into(),
                kind: BlockKind::Bullet,
            }],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let prompt = bounds(cx, "empty");
    cx.simulate_click(prompt.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        root_texts(cx),
        vec!["status:: x".to_string(), "note".to_string()],
        "a new block holds the note; the attribute is untouched"
    );
}

/// Finding 4, the feed: an attribute-only day's "Click to start writing" adds a block too.
#[gpui::test]
fn starting_a_note_on_an_attribute_only_day_appends_a_block(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today));
    // Yesterday: one attribute only. Every other day of the feed has a note, so yesterday's
    // prompt is the only "Click to start writing" in the feed.
    let page = store
        .ensure_daily_page(today - chrono::Duration::days(1))
        .unwrap();
    store
        .execute(
            &page,
            vec![Command::InsertBlock {
                id: BlockId::new(),
                parent: None,
                after: None,
                text: "status:: x".into(),
                kind: BlockKind::Bullet,
            }],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    for back in 0..10 {
        if back == 1 {
            continue;
        }
        let day = store
            .ensure_daily_page(today - chrono::Duration::days(back))
            .unwrap();
        let b = store.ensure_first_block(&day).unwrap();
        store
            .set_block_text(&day, &b, &format!("note {back}"), None, None)
            .unwrap();
        store.commit_block(&day, &b).unwrap();
    }
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    let prompt = bounds(cx, "day-empty");
    cx.simulate_click(prompt.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        let texts: Vec<String> = st
            .live_roots()
            .iter()
            .map(|b| st.block(b).expect("block").text.clone())
            .collect();
        assert_eq!(texts, vec!["status:: x".to_string(), "note".to_string()]);
    });
}
