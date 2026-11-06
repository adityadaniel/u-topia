//! Acceptance scripts from the PRD (§5.4, §6.4, §7.4, §8.4, §8.6, §4.3) run against a
//! real SQLite store.

use chrono::NaiveDate;
use utopia_core::outline::{self, Plan};
use utopia_core::{BlockId, PageId, PageKind};
use utopia_doc::Frontier;
use utopia_store::{BlockRefStatus, RefQuery, RefTarget, Store};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A file-backed store, so every acceptance script runs through the writer thread. The
/// store is dropped (flushing and joining the writer) before its directory is removed.
struct TestStore {
    store: Store,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl std::ops::DerefMut for TestStore {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

fn store() -> TestStore {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("graph.utopia")).unwrap();
    store.set_today(Some(date(2026, 11, 2)));
    TestStore { store, _dir: dir }
}

fn run(s: &mut Store, page: &PageId, plan: Plan) -> Option<BlockId> {
    s.execute_plan(page, plan, None).unwrap().map(|f| f.block)
}

/// Append a block to the end of a page, type `text`, and commit it.
fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = run(s, page, plan).unwrap();
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

/// Type without committing (no page auto-creation).
fn type_only(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = run(s, page, plan).unwrap();
    s.set_block_text(page, &id, text, None, None).unwrap();
    id
}

fn text(s: &mut Store, page: &PageId, b: &BlockId) -> String {
    s.page_state(page).unwrap().block(b).unwrap().text.clone()
}

fn live_texts(s: &mut Store, page: &PageId) -> Vec<(String, usize)> {
    let st = s.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
        .collect()
}

fn assert_doctor(s: &mut Store) {
    let r = s.doctor().unwrap();
    assert!(r.is_ok(), "doctor: {:#?}", r.problems);
}

#[test]
fn ac_1_1_lazy_daily_and_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let page;
    {
        let mut s = Store::open(&path).unwrap();
        s.set_today(Some(date(2026, 11, 2)));
        assert!(
            s.list_pages(true).unwrap().is_empty(),
            "untouched graph stays empty"
        );
        let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
        s.set_block_text(&p, &b, "hello", None, None).unwrap();
        page = p;
    }
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    assert_eq!(p, page);
    assert_eq!(text(&mut s, &p, &b), "hello");
    assert_eq!(s.page_row(&p).unwrap().unwrap().title, "November 2nd, 2026");
    assert_doctor(&mut s);
}

#[test]
fn ac_1_4_backfill_thirty_days() {
    let mut s = store();
    for i in 0..30 {
        let d = date(2026, 10, 1) + chrono::Days::new(i);
        let (p, b) = s.open_daily(d).unwrap();
        assert_eq!(s.page_state(&p).unwrap().live_count(), 1);
        // The feed and sidebar list days with entries (FR-1.2).
        s.set_block_text(&p, &b, "x", None, None).unwrap();
    }
    assert_eq!(s.list_pages(true).unwrap().len(), 30);
    assert_eq!(s.journal(14).unwrap().len(), 14);
}

#[test]
fn fr_1_2_visited_day_is_a_draft_until_written() {
    let mut s = store();
    let d = date(2026, 11, 2);
    let p = s.draft_daily(d).unwrap();
    assert_eq!(p, PageId::for_date(d));
    assert_eq!(s.draft_daily(d).unwrap(), p, "one draft per day");
    assert!(s.is_draft(&p));
    assert_eq!(s.page_state(&p).unwrap().live_count(), 1);
    assert!(s.find_page("2026-11-02").unwrap().is_none());
    assert!(s.list_pages(true).unwrap().is_empty());
    assert!(s.outbox_pending(10).unwrap().is_empty(), "nothing to sync");
    assert_doctor(&mut s);
    // The first edit writes the whole page: meta, first block and the edit.
    let b = s.page_state(&p).unwrap().live_roots()[0].clone();
    s.set_block_text(&p, &b, "hello", None, None).unwrap();
    assert!(!s.is_draft(&p));
    assert_eq!(s.find_page("2026-11-02").unwrap(), Some(p.clone()));
    assert!(!s.outbox_pending(10).unwrap().is_empty());
    assert_eq!(live_texts(&mut s, &p), [("hello".to_string(), 0)]);
    assert_doctor(&mut s);
    // A link to a drafted day writes that draft instead of a second page (FR-1.10).
    let next = s.draft_daily(date(2026, 11, 3)).unwrap();
    write(&mut s, &p, "see [[2026-11-03]]");
    assert_eq!(s.find_page("2026-11-03").unwrap(), Some(next.clone()));
    assert!(!s.is_draft(&next));
    assert_doctor(&mut s);
}

#[test]
fn ac_1_5_deleted_daily_is_recreated_and_recoverable() {
    let mut s = store();
    let (old, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&old, &b, "kept", None, None).unwrap();
    s.delete_page(&old).unwrap();
    let (new, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    assert_ne!(
        old, new,
        "a tombstoned deterministic id is never resurrected"
    );
    assert_eq!(s.list_trash().unwrap().len(), 1);
    // Restoring while the date is taken merges instead of duplicating.
    let survivor = s.restore_page(&old).unwrap();
    assert_eq!(survivor, new);
    let texts: Vec<String> = live_texts(&mut s, &new).into_iter().map(|t| t.0).collect();
    assert!(texts.contains(&"kept".to_string()));
    assert_doctor(&mut s);
}

#[test]
fn ac_1_6_scheduled_for_this_day() {
    let mut s = store();
    let (nov2, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    let blk = write(&mut s, &nov2, "Pay rent [[2026-12-01]]");
    let dec1 = s
        .find_page("2026-12-01")
        .unwrap()
        .expect("pre-created by reference (FR-1.10)");
    assert_eq!(s.page_row(&dec1).unwrap().unwrap().kind, PageKind::Daily);
    assert_eq!(
        s.page_state(&dec1).unwrap().live_count(),
        0,
        "created empty, unvisited"
    );
    let refs = s.linked_references(&dec1, &RefQuery::default()).unwrap();
    assert_eq!(refs.groups.len(), 1);
    assert_eq!(refs.groups[0].page.id, nov2, "navigates to the source page");
    assert_eq!(refs.groups[0].hits[0].block, blk);
    assert!(
        s.journal(14).unwrap().iter().all(|j| j.page.id != dec1),
        "future days not in journal"
    );
}

#[test]
fn ac_1_7_and_1_8_date_forms_and_format_change() {
    let mut s = store();
    s.set_date_format("yyyy/MM/dd").unwrap();
    let a = s.create_page("A").unwrap();
    let b = s.create_page("B").unwrap();
    let c = s.create_page("C").unwrap();
    write(&mut s, &a, "due [[2026-12-01]]");
    write(&mut s, &b, "due [[December 1st, 2026]]");
    write(&mut s, &c, "due [[2026/12/01]]");
    let day = s.find_page("2026-12-01").unwrap().unwrap();
    let dailies: Vec<_> = s
        .list_pages(true)
        .unwrap()
        .into_iter()
        .filter(|p| p.is_daily())
        .collect();
    assert_eq!(dailies.len(), 1, "one page, not three");
    assert_eq!(s.backlink_count(&day).unwrap(), 3);
    assert_eq!(s.page_row(&day).unwrap().unwrap().title, "2026/12/01");
    s.set_date_format("yyyy-MM-dd").unwrap();
    assert_eq!(s.page_row(&day).unwrap().unwrap().title, "2026-12-01");
    assert_eq!(
        s.find_page("2026-12-01").unwrap(),
        Some(day.clone()),
        "ids unchanged"
    );
    assert_eq!(s.backlink_count(&day).unwrap(), 3, "backlinks identical");
    assert_doctor(&mut s);
}

#[test]
fn ambiguous_numeric_date_is_a_plain_page() {
    let mut s = store();
    let p = s.create_page("Notes").unwrap();
    write(&mut s, &p, "see [[11/6/26]]");
    let target = s.find_page("11/6/26").unwrap().unwrap();
    assert_eq!(s.page_row(&target).unwrap().unwrap().kind, PageKind::Page);
}

#[test]
fn ac_2_3_two_hundred_undos_restore_projection() {
    let mut s = store();
    let (page, first) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.seal_undo();
    let projection = |s: &mut Store| -> Vec<String> {
        let mut stmt = s
            .conn()
            .prepare(
                "SELECT id||'|'||ifnull(parent_id,'')||'|'||depth||'|'||text||'|'||block_type||'|'||ifnull(deleted_at,'')
                 FROM block ORDER BY id",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    let start = projection(&mut s);
    let mut focus = first.clone();
    let mut done = 0;
    let mut i = 0u64;
    while done < 200 {
        i += 1;
        let st = s.page_state(&page).unwrap().clone();
        let plan = match i % 6 {
            0 | 1 => outline::split_block(&st, &focus, st.block(&focus).unwrap().text.len(), false),
            2 => outline::indent(&st, &focus),
            3 => outline::outdent(&st, &focus),
            4 => outline::move_up(&st, &focus),
            _ => {
                let t = format!("{} w{i}", st.block(&focus).unwrap().text);
                s.set_block_text(&page, &focus, &t, None, None).unwrap();
                s.seal_undo();
                done += 1;
                continue;
            }
        };
        if let Ok(plan) = plan {
            if let Some(f) = run(&mut s, &page, plan) {
                focus = f;
            }
            done += 1;
        }
    }
    let end = projection(&mut s);
    for _ in 0..200 {
        s.undo().unwrap();
    }
    assert_eq!(projection(&mut s), start, "byte-identical after 200 undos");
    while s.can_redo() {
        s.redo().unwrap();
    }
    assert_eq!(projection(&mut s), end);
    assert_doctor(&mut s);
}

#[test]
fn ac_2_5_paste_five_hundred_lines_one_undo() {
    let mut s = store();
    let (page, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    let md: String = (0..500)
        .map(|i| format!("{}- line {i}\n", "  ".repeat(i % 4)))
        .collect();
    let st = s.page_state(&page).unwrap();
    let plan = outline::paste(st, &b, 0..0, &md).unwrap();
    run(&mut s, &page, plan);
    let rows = live_texts(&mut s, &page);
    assert_eq!(rows.len(), 500);
    assert_eq!(rows[3], ("line 3".into(), 3));
    assert_eq!(rows[4], ("line 4".into(), 0));
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &page), vec![(String::new(), 0)]);
}

#[test]
fn ac_2_7_hashtag_equals_link() {
    let mut s = store();
    let a = s.create_page("A").unwrap();
    let b = s.create_page("B").unwrap();
    write(&mut s, &a, "practice #mindfulness");
    write(&mut s, &b, "more [[mindfulness]]");
    let m: Vec<_> = s
        .list_pages(false)
        .unwrap()
        .into_iter()
        .filter(|p| p.title_key == "mindfulness")
        .collect();
    assert_eq!(m.len(), 1);
    let refs = s.linked_references(&m[0].id, &RefQuery::default()).unwrap();
    assert_eq!(refs.shown(), 2);
}

#[test]
fn ac_2_8_multi_word_tags() {
    let mut s = store();
    s.create_page("Modern Religion").unwrap();
    let p = s.create_page("Notes").unwrap();
    let b = write(&mut s, &p, "#Modern Religion is interesting");
    assert_eq!(text(&mut s, &p, &b), "#[[Modern Religion]] is interesting");
    let b2 = write(&mut s, &p, "#modern society");
    assert_eq!(
        text(&mut s, &p, &b2),
        "#modern society",
        "tag ends at the space"
    );
    assert!(s.find_page("modern").unwrap().is_some());
    assert!(s.find_page("modern society").unwrap().is_none());
}

#[test]
fn ac_2_10_deleted_page_becomes_unresolved_intent() {
    let mut s = store();
    let notes = s.create_page("Notes").unwrap();
    write(&mut s, &notes, "#[[Modern Religion]]");
    let mr = s.find_page("Modern Religion").unwrap().unwrap();
    s.delete_page(&mr).unwrap();
    let unresolved = s.unresolved_refs(&notes).unwrap();
    assert_eq!(unresolved.len(), 1);
    assert!(
        matches!(&unresolved[0].target, RefTarget::UnresolvedPage { key } if key == "modern religion")
    );
    let again = s.create_page("Modern Religion").unwrap();
    assert_ne!(again, mr);
    assert!(s.unresolved_refs(&notes).unwrap().is_empty());
    assert_eq!(s.backlink_count(&again).unwrap(), 1);
}

#[test]
fn ac_3_2_unclosed_link_creates_nothing() {
    let mut s = store();
    let p = s.create_page("P").unwrap();
    let b = write(&mut s, &p, "typing [[Alpha");
    assert!(s.find_page("Alpha").unwrap().is_none());
    s.set_block_text(&p, &b, "typing [[Alpha]]", None, None)
        .unwrap();
    s.commit_block(&p, &b).unwrap();
    let alpha = s.find_page("Alpha").unwrap().unwrap();
    assert_eq!(s.backlink_count(&alpha).unwrap(), 1);
}

#[test]
fn ac_3_3_rename_keeps_references_via_alias() {
    let mut s = store();
    let src = s.create_page("Log").unwrap();
    for i in 0..5 {
        write(&mut s, &src, &format!("note {i} about [[Alpha]]"));
    }
    let alpha = s.find_page("Alpha").unwrap().unwrap();
    assert_eq!(s.rename_page(&alpha, "Alpha Prime").unwrap(), alpha);
    assert_eq!(s.backlink_count(&alpha).unwrap(), 5);
    assert_eq!(s.find_page("alpha").unwrap(), Some(alpha.clone()));
    assert_eq!(s.aliases(&alpha).unwrap(), vec!["Alpha".to_string()]);
    assert_eq!(s.rewrite_references(&alpha).unwrap(), 5);
    let first = &live_texts(&mut s, &src)[0].0;
    assert_eq!(first, "note 0 about [[Alpha Prime]]");
    assert_eq!(s.backlink_count(&alpha).unwrap(), 5);
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &src)[0].0, "note 0 about [[Alpha]]");
    assert_doctor(&mut s);
}

#[test]
fn ac_3_6_co_reference_filter() {
    let mut s = store();
    let src = s.create_page("Reading").unwrap();
    write(&mut s, &src, "[[Mindfulness]] alone");
    write(&mut s, &src, "[[Mindfulness]] and [[Books]]");
    let parent = write(&mut s, &src, "about [[Books]]");
    let st = s.page_state(&src).unwrap();
    let plan = outline::split_block(st, &parent, "about [[Books]]".len(), false).unwrap();
    let child = run(&mut s, &src, plan).unwrap();
    let st = s.page_state(&src).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut s, &src, plan);
    s.set_block_text(&src, &child, "child of books: [[Mindfulness]]", None, None)
        .unwrap();
    s.commit_block(&src, &child).unwrap();
    let m = s.find_page("Mindfulness").unwrap().unwrap();
    let books = s.find_page("Books").unwrap().unwrap();
    let all = s.linked_references(&m, &RefQuery::default()).unwrap();
    assert_eq!(all.shown(), 3);
    let q = RefQuery {
        also: vec![books],
        ..Default::default()
    };
    let filtered = s.linked_references(&m, &q).unwrap();
    assert_eq!(filtered.shown(), 2, "direct co-reference and via ancestor");
    assert_eq!(filtered.total, 3);
    let q = RefQuery {
        text: Some("alone".into()),
        ..Default::default()
    };
    assert_eq!(s.linked_references(&m, &q).unwrap().shown(), 1);
    let crumbs = &filtered.groups[0]
        .hits
        .iter()
        .find(|h| h.block == child)
        .unwrap()
        .breadcrumb;
    assert_eq!(crumbs.len(), 1);
    assert_eq!(crumbs[0].1, "about Books");
}

#[test]
fn ac_3_8_creating_a_page_resolves_pending_intents() {
    let mut s = store();
    let p = s.create_page("Inbox").unwrap();
    for i in 0..6 {
        type_only(&mut s, &p, &format!("{i} [[Zettelkasten]]"));
    }
    assert_eq!(s.unresolved_refs(&p).unwrap()[0].count, 6);
    let z = s.create_page("Zettelkasten").unwrap();
    assert_eq!(s.backlink_count(&z).unwrap(), 6);
    assert!(s.unresolved_refs(&p).unwrap().is_empty());
    assert_doctor(&mut s);
}

#[test]
fn fr_3_4_and_3_7_live_updates_and_self_references() {
    let mut s = store();
    let x = s.create_page("X").unwrap();
    let b = write(&mut s, &x, "I refer to [[X]] myself");
    assert_eq!(s.backlink_count(&x).unwrap(), 1, "self-references count");
    let st = s.page_state(&x).unwrap();
    let plan = outline::delete_subtree(st, &b, 1).unwrap();
    run(&mut s, &x, plan);
    assert_eq!(
        s.backlink_count(&x).unwrap(),
        0,
        "deleting the source removes its edge"
    );
    assert!(
        s.find_page("X").unwrap().is_some(),
        "no cleanup debt: page stays"
    );
}

#[test]
fn ac_4_2_edit_through_reference_and_undo() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let target = write(&mut s, &src, "a thesis");
    let other = s.create_page("Other").unwrap();
    write(&mut s, &other, &format!("see (({target}))"));
    // An embed edits the original block, on its own page.
    let page = s.block_page(&target).unwrap();
    assert_eq!(page, src);
    s.set_block_text(&page, &target, "a better thesis", None, None)
        .unwrap();
    assert!(matches!(
        s.block_ref_status(target.as_str()).unwrap(),
        BlockRefStatus::Live { ref text, .. } if text == "a better thesis"
    ));
    assert_eq!(s.block_backlinks(&target).unwrap().len(), 1);
    s.undo().unwrap();
    assert_eq!(text(&mut s, &src, &target), "a thesis");
}

#[test]
fn ac_4_5_deleted_and_missing_block_refs() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let target = write(&mut s, &src, "doomed");
    let other = s.create_page("Other").unwrap();
    for _ in 0..3 {
        write(&mut s, &other, &format!("ref (({target}))"));
    }
    write(&mut s, &other, "ghost ((01NOTAREALBLOCK00000000000))");
    let st = s.page_state(&src).unwrap();
    let plan = outline::delete_subtree(st, &target, 5).unwrap();
    run(&mut s, &src, plan);
    assert!(matches!(
        s.block_ref_status(target.as_str()).unwrap(),
        BlockRefStatus::Deleted { .. }
    ));
    assert_eq!(
        s.block_ref_status("01NOTAREALBLOCK00000000000").unwrap(),
        BlockRefStatus::Missing
    );
    let unresolved = s.unresolved_refs(&other).unwrap();
    assert_eq!(
        unresolved.len(),
        1,
        "only the never-existing uid is unresolved"
    );
}

#[test]
fn ac_4_6_and_4_7_merge_on_rename_and_undo() {
    let mut s = store();
    let a = s.create_page("Apple Watch").unwrap();
    for i in 0..3 {
        write(&mut s, &a, &format!("a{i}"));
    }
    let b = s.create_page("applewatch").unwrap();
    let mut b_ids = Vec::new();
    for i in 0..5 {
        b_ids.push(write(&mut s, &b, &format!("b{i}")));
    }
    // Give b0 a two-level subtree.
    for (depth, label) in [(1, "b0.1"), (2, "b0.1.1")] {
        let st = s.page_state(&b).unwrap();
        let anchor = st.live_preorder()[depth - 1].0.clone();
        let plan = outline::split_block(st, &anchor, st.block(&anchor).unwrap().text.len(), false)
            .unwrap();
        let n = run(&mut s, &b, plan).unwrap();
        s.set_block_text(&b, &n, label, None, None).unwrap();
        let st = s.page_state(&b).unwrap();
        if st.depth(&n) < depth {
            let plan = outline::indent(st, &n).unwrap();
            run(&mut s, &b, plan);
        }
    }
    assert_eq!(
        live_texts(&mut s, &b)[..3],
        [("b0".into(), 0), ("b0.1".into(), 1), ("b0.1.1".into(), 2)]
    );
    let refs = s.create_page("Refs").unwrap();
    for _ in 0..4 {
        write(&mut s, &refs, "[[applewatch]]");
    }
    let before_a = live_texts(&mut s, &a);
    let before_b = live_texts(&mut s, &b);
    let survivor = s.rename_page(&b, "apple watch").unwrap();
    assert_eq!(survivor, a);
    let merged = live_texts(&mut s, &a);
    let roots: Vec<_> = merged.iter().filter(|(_, d)| *d == 0).collect();
    assert_eq!(roots.len(), 8);
    assert_eq!(
        &merged[3..6],
        &[("b0".into(), 0), ("b0.1".into(), 1), ("b0.1.1".into(), 2)]
    );
    for id in &b_ids {
        assert_eq!(
            s.block_page(id).unwrap(),
            a,
            "ids preserved, moved not copied"
        );
    }
    assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_some());
    assert_eq!(s.backlink_count(&a).unwrap(), 4);
    assert_eq!(s.find_page("applewatch").unwrap(), Some(a.clone()));
    assert_doctor(&mut s);
    // One Cmd+Z reverses the whole merge.
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &a), before_a);
    assert_eq!(live_texts(&mut s, &b), before_b);
    assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_none());
    assert_eq!(s.backlink_count(&b).unwrap(), 4);
    assert_doctor(&mut s);
}

#[test]
fn ac_4_9_unicode_equivalent_titles_merge() {
    let mut s = store();
    let a = s.create_page("Caf\u{e9}  Notes").unwrap();
    let b = s.create_page("Other").unwrap();
    assert_eq!(s.rename_page(&b, "Cafe\u{301} Notes").unwrap(), a);
}

#[test]
fn rename_to_date_merges_into_daily_page() {
    let mut s = store();
    let p = s.create_page("Meeting").unwrap();
    write(&mut s, &p, "agenda");
    let daily = s.rename_page(&p, "2026-11-06").unwrap();
    let row = s.page_row(&daily).unwrap().unwrap();
    assert_eq!(row.daily_date, Some(date(2026, 11, 6)));
    assert_eq!(live_texts(&mut s, &daily), vec![("agenda".into(), 0)]);
    assert_eq!(
        s.find_page("Meeting").unwrap(),
        Some(daily),
        "old title still resolves"
    );
    let d = s.find_page("2026-11-06").unwrap().unwrap();
    assert!(
        s.rename_page(&d, "x").is_err(),
        "daily titles are display-only"
    );
}

#[test]
fn w3_placeholder_pages_exist_without_content() {
    let mut s = store();
    let (today, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    write(
        &mut s,
        &today,
        "Thinking about #[[Modern Religion]] and [[Dana]]",
    );
    let titles: Vec<String> = s
        .list_pages(false)
        .unwrap()
        .into_iter()
        .map(|p| p.title)
        .collect();
    assert_eq!(titles, vec!["Dana", "Modern Religion"]);
    assert_doctor(&mut s);
}

#[test]
fn quick_jump_and_block_search() {
    let mut s = store();
    s.create_page("Alpha Prime").unwrap();
    s.create_page("Reading log").unwrap();
    let p = s.create_page("Notes").unwrap();
    write(&mut s, &p, "the central thesis of everything");
    let hits = s.search_titles("alp", 5).unwrap();
    assert_eq!(hits[0].title, "Alpha Prime");
    let hits = s.search_titles("rdlg", 5).unwrap();
    assert_eq!(hits[0].title, "Reading log", "subsequence match");
    let blocks = s.search_blocks("thes", 5).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].page_title, "Notes");
}

#[test]
fn collapse_state_is_local() {
    let mut s = store();
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_collapsed(&b, true).unwrap();
    assert!(s.is_collapsed(&b));
    // Not part of the shard: the page's outbox payloads never mention it.
    let before = s.outbox_pending(100).unwrap().len();
    s.set_collapsed(&b, false).unwrap();
    assert_eq!(s.outbox_pending(100).unwrap().len(), before);
    let _ = p;
}

#[test]
fn export_markdown_and_json() {
    let mut s = store();
    let (today, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&today, &b, "Woke up thinking about [[Alpha]]", None, None)
        .unwrap();
    s.commit_block(&today, &b).unwrap();
    let st = s.page_state(&today).unwrap();
    let plan = outline::split_block(st, &b, st.block(&b).unwrap().text.len(), false).unwrap();
    let child = run(&mut s, &today, plan).unwrap();
    let st = s.page_state(&today).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut s, &today, plan);
    s.set_block_text(&today, &child, "nested", None, None)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(s.export_markdown(dir.path()).unwrap(), 2);
    let md = std::fs::read_to_string(dir.path().join("2026-11-02.md")).unwrap();
    assert_eq!(
        md,
        "# November 2nd, 2026\n\n- Woke up thinking about [[Alpha]]\n  - nested\n"
    );
    let json = s.export_json_value().unwrap();
    let pages = json["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
}

fn push(from: &mut Store, to: &mut Store) {
    let batch: Vec<(PageId, Vec<u8>)> = from
        .all_page_ids()
        .unwrap()
        .into_iter()
        .map(|p| {
            let bytes = from.export_since(&p, &Frontier::default()).unwrap();
            (p, bytes)
        })
        .collect();
    to.import_batch(&batch).unwrap();
}

fn exchange(a: &mut Store, b: &mut Store) {
    push(a, b);
    push(b, a);
}

fn snapshot(s: &mut Store) -> Vec<(String, Vec<(String, usize)>)> {
    let pages = s.list_pages(true).unwrap();
    pages
        .into_iter()
        .map(|p| (p.title.clone(), live_texts(s, &p.id)))
        .collect()
}

#[test]
fn both_devices_create_same_title_offline_converge() {
    let mut a = store();
    let mut b = store();
    let pa = a.create_page("Alpha").unwrap();
    let pb = b.create_page("Alpha").unwrap();
    assert_eq!(pa, pb, "deterministic uuidv5");
    write(&mut a, &pa, "from a");
    write(&mut b, &pb, "from b");
    exchange(&mut a, &mut b);
    exchange(&mut a, &mut b);
    assert_eq!(snapshot(&mut a), snapshot(&mut b));
    assert_eq!(live_texts(&mut a, &pa).len(), 2);
    assert_doctor(&mut a);
    assert_doctor(&mut b);
}

#[test]
fn import_time_dedupe_merges_colliding_pages() {
    let mut a = store();
    let mut b = store();
    // Device A's "Beta" got a fresh id (its uuidv5 was tombstoned); B's did not.
    let old = a.create_page("Beta").unwrap();
    a.delete_page(&old).unwrap();
    let pa = a.create_page("Beta").unwrap();
    let pb = b.create_page("Beta").unwrap();
    assert_ne!(pa, pb);
    write(&mut a, &pa, "a's note");
    write(&mut b, &pb, "b's note");
    exchange(&mut a, &mut b);
    exchange(&mut a, &mut b);
    let live: Vec<_> = a
        .list_pages(false)
        .unwrap()
        .into_iter()
        .filter(|p| p.title == "Beta")
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(snapshot(&mut a), snapshot(&mut b));
    let texts: Vec<String> = live_texts(&mut a, &live[0].id)
        .into_iter()
        .map(|t| t.0)
        .collect();
    assert!(texts.contains(&"a's note".to_string()) && texts.contains(&"b's note".to_string()));
    assert_doctor(&mut a);
    assert_doctor(&mut b);
}

#[test]
fn ac_4_8_offline_blocks_follow_merge() {
    let mut d1 = store();
    let mut d2 = store();
    let a = d1.create_page("Alpha").unwrap();
    write(&mut d1, &a, "a0");
    let bpage = d1.create_page("Bravo").unwrap();
    let b0 = write(&mut d1, &bpage, "b0");
    exchange(&mut d1, &mut d2);
    // Device 1 merges Bravo into Alpha while device 2, offline, adds three blocks.
    d1.merge_pages(&bpage, &a).unwrap();
    for i in 0..3 {
        write(&mut d2, &bpage, &format!("offline {i}"));
    }
    let st = d2.page_state(&bpage).unwrap();
    let plan = outline::split_block(st, &b0, 2, false).unwrap();
    let child = run(&mut d2, &bpage, plan).unwrap();
    let st = d2.page_state(&bpage).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut d2, &bpage, plan);
    d2.set_block_text(&bpage, &child, "child of b0", None, None)
        .unwrap();
    exchange(&mut d1, &mut d2);
    exchange(&mut d1, &mut d2);
    assert_eq!(snapshot(&mut d1), snapshot(&mut d2));
    let merged: Vec<String> = live_texts(&mut d1, &a).into_iter().map(|t| t.0).collect();
    for want in [
        "a0",
        "b0",
        "offline 0",
        "offline 1",
        "offline 2",
        "child of b0",
    ] {
        assert!(
            merged.contains(&want.to_string()),
            "{want} missing from {merged:?}"
        );
    }
    assert_eq!(merged.len(), 6, "no duplicates: {merged:?}");
    for s in [&mut d1, &mut d2] {
        assert!(s.page_row(&bpage).unwrap().unwrap().deleted_at.is_some());
        assert!(live_texts(s, &bpage).is_empty());
        assert_doctor(s);
    }
}

#[test]
fn rolling_backups_keep_six_generations() {
    let mut s = store();
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&p, &b, "backed up", None, None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for i in 0..8 {
        s.set_clock(Some(1_000 + i));
        s.backup(dir.path(), 6).unwrap();
    }
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(files.len(), 6);
    // A backup is a complete, openable graph.
    let newest = dir.path().join("graph-1007.db");
    let mut restored = Store::open(&newest).unwrap();
    let st = restored.page_state(&p).unwrap();
    assert_eq!(st.block(&b).unwrap().text, "backed up");
}

#[test]
fn debounced_commits_reach_disk_on_flush() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let page;
    {
        let mut s = Store::open(&path).unwrap();
        s.set_today(Some(date(2026, 11, 2)));
        s.set_debounced(true).unwrap();
        let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
        for i in 0..50 {
            s.set_block_text(&p, &b, &format!("draft {i} [[Later]]"), None, None)
                .unwrap();
        }
        assert!(s.has_pending(), "keystrokes stay in memory until a flush");
        s.flush().unwrap();
        s.write_ticket().wait();
        assert!(!s.has_pending());
        page = p;
        // Dropping the store flushes, stops the writer and compacts.
        s.set_block_text(&page, &b, "final [[Later]]", None, None)
            .unwrap();
    }
    let mut s = Store::open(&path).unwrap();
    let st = s.page_state(&page).unwrap();
    let text = st.block(&st.live_roots()[0]).unwrap().text.clone();
    assert_eq!(text, "final [[Later]]");
    assert!(s.doctor().unwrap().is_ok());
}

#[test]
fn reopening_mid_session_sees_writer_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut a = Store::open(&path).unwrap();
    let p = a.create_page("Shared").unwrap();
    write(&mut a, &p, "from the writer thread");
    a.sync_index().unwrap();
    // A second connection (e.g. `utopia doctor` while the app runs) sees the commit.
    let mut b = Store::open(&path).unwrap();
    assert_eq!(
        live_texts(&mut b, &p),
        vec![("from the writer thread".to_string(), 0)]
    );
}

/// SR-1: each hit carries the byte ranges of the matched terms in the plain text the
/// palette draws, so the view never has to scan for them (and never marks a bracket the
/// renderer hides). Prefixes match at a word start, as the FTS tokenizer does.
#[test]
fn sr_1_search_marks_the_matched_terms() {
    let mut s = store();
    let p = s.create_page("Notes").unwrap();
    write(
        &mut s,
        &p,
        "The **zebra** crossing and [[Zebra Crossing]] again",
    );
    write(&mut s, &p, "bathes in the sun");
    let hits = s.search_blocks("zeb", 5).unwrap();
    assert_eq!(hits.len(), 1);
    let plain = utopia_core::inline::plain_text(&hits[0].text);
    let marked: Vec<&str> = hits[0].anchors.iter().map(|&(a, b)| &plain[a..b]).collect();
    assert_eq!(marked, vec!["zebra", "Zebra"], "{plain:?}");
    // The ranges are the ones gpui's StyledText requires: sorted and non-overlapping.
    assert!(hits[0].anchors.windows(2).all(|w| w[0].1 <= w[1].0));
    assert!(
        s.search_blocks("thes", 5).unwrap().is_empty(),
        "a match inside a word is not the index's match either"
    );
    // Two terms merge into one painted run rather than a nested highlight.
    let both = utopia_store::match_anchors("zebra crossing", "zeb cross");
    assert_eq!(both, vec![(0, 5), (6, 14)], "the whole word is washed");
}

/// SR-2: results sit under a header per page, with that page's blocks in one group and
/// the page's title/daily flag from the index.
#[test]
fn sr_2_search_groups_results_by_page() {
    let mut s = store();
    let a = s.create_page("Atlas").unwrap();
    write(&mut s, &a, "atlas launch plan");
    write(&mut s, &a, "atlas risks");
    let b = s.create_page("Notes").unwrap();
    write(&mut s, &b, "the atlas is late");
    let groups = s.search_groups("atlas", 30).unwrap();
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert!(!groups[0].daily && !groups[1].daily);
    let atlas = groups.iter().find(|g| g.title == "Atlas").unwrap();
    assert_eq!(atlas.page, a);
    assert_eq!(atlas.hits.len(), 2);
    assert!(atlas.hits.iter().all(|h| h.page == a));
    let notes = groups.iter().find(|g| g.title == "Notes").unwrap();
    assert_eq!(notes.page, b);
    assert_eq!(notes.hits.len(), 1);
}

/// SR-3: the scope chips narrow to daily pages, ordinary pages or to-dos, and the age
/// menu drops hits older than the window. A daily hit is dated by its day, other blocks
/// by the local date of their last edit.
#[test]
fn sr_3_search_scopes_and_age() {
    use chrono::TimeZone as _;
    use utopia_core::BlockKind;
    use utopia_store::{SearchAge, SearchScope};

    // Seed at local noon so "the block was edited today" holds in every timezone.
    let ts = |y, m, d, h| {
        date(y, m, d)
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let mut s = store();
    s.set_clock(Some(ts(2026, 11, 2, 12)));
    let atlas = s.create_page("Atlas").unwrap();
    write(&mut s, &atlas, "atlas launch");
    let (day, db) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&day, &db, "atlas daily note", None, None)
        .unwrap();
    s.commit_block(&day, &db).unwrap();
    let work = s.create_page("Work").unwrap();
    let todo = write(&mut s, &work, "atlas todo");
    s.set_block_kind(&work, &todo, BlockKind::Todo, false)
        .unwrap();

    // An older block, written before the window.
    s.set_clock(Some(ts(2026, 10, 1, 12)));
    let old = s.create_page("Old").unwrap();
    write(&mut s, &old, "atlas old");
    s.set_clock(Some(ts(2026, 11, 2, 12)));
    s.sync_index().unwrap();

    let pages = |s: &Store, scope, age| -> Vec<String> {
        let mut v: Vec<String> = s
            .search_blocks_filtered("atlas", scope, age, 50)
            .unwrap()
            .into_iter()
            .map(|h| h.page_title)
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        pages(&s, SearchScope::All, SearchAge::AnyTime),
        vec!["Atlas", "November 2nd, 2026", "Old", "Work"]
    );
    assert_eq!(
        pages(&s, SearchScope::Journal, SearchAge::AnyTime),
        vec!["November 2nd, 2026"]
    );
    assert_eq!(
        pages(&s, SearchScope::Pages, SearchAge::Today),
        vec!["Atlas", "Work"],
        "the October note is outside Today"
    );
    assert_eq!(
        pages(&s, SearchScope::Todos, SearchAge::AnyTime),
        vec!["Work"]
    );
    let daily = s
        .search_groups_filtered("atlas", SearchScope::Journal, SearchAge::Today, 30)
        .unwrap();
    assert_eq!(daily.len(), 1);
    assert!(daily[0].daily && daily[0].hits.len() == 1);
    assert_eq!(
        s.search_blocks_filtered("atlas", SearchScope::All, SearchAge::Year, 50)
            .unwrap()
            .len(),
        4
    );
}

/// SR-4: the preview of a search hit carries its path from the root, its state and its
/// first children (with the total count), read from the index rather than at paint time.
#[test]
fn sr_4_preview_has_path_children_and_state() {
    use utopia_core::BlockKind;

    let mut s = store();
    let p = s.create_page("Atlas").unwrap();
    let parent = write(&mut s, &p, "Risks");
    let plan = outline::append_block(s.page_state(&p).unwrap(), Some(&parent));
    let hit = run(&mut s, &p, plan).unwrap();
    s.set_block_text(&p, &hit, "zebra migration is late", None, None)
        .unwrap();
    s.set_block_kind(&p, &hit, BlockKind::Todo, true).unwrap();
    // A couple of children under the hit, one more than the preview lists.
    for i in 0..(utopia_store::PREVIEW_CHILDREN + 1) {
        let plan = outline::append_block(s.page_state(&p).unwrap(), Some(&hit));
        let c = run(&mut s, &p, plan).unwrap();
        s.set_block_text(&p, &c, &format!("child {i}"), None, None)
            .unwrap();
    }
    s.sync_index().unwrap();
    let preview = s.preview_block(&hit).unwrap().expect("a live block");
    assert_eq!(preview.page, p);
    assert_eq!(preview.page_title, "Atlas");
    assert!(!preview.daily);
    assert_eq!(preview.path.len(), 1, "the parent above the hit");
    assert_eq!(preview.path[0].1, "Risks");
    assert_eq!((preview.kind, preview.checked), (BlockKind::Todo, true));
    assert_eq!(
        preview.child_count,
        utopia_store::PREVIEW_CHILDREN + 1,
        "the count is the whole list"
    );
    assert_eq!(
        preview.children.len(),
        utopia_store::PREVIEW_CHILDREN,
        "the pane lists the first few"
    );
    assert_eq!(preview.children[0].text, "child 0");
    // No panic on user data: a missing block is `None`, never an error.
    assert!(s.preview_block(&BlockId("nope".into())).unwrap().is_none());
}

/// SR-3: the age window is part of the query. A pile of older matches must not starve the
/// candidate list: 305 October blocks used to fill the 10x over-fetch before one Today
/// block was ever looked at, so Today came back empty.
#[test]
fn sr_3_age_window_is_applied_before_the_limit() {
    use chrono::TimeZone as _;
    use utopia_store::{SearchAge, SearchScope};

    let ts = |m, d, h| {
        date(2026, m, d)
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let mut s = store();
    s.set_clock(Some(ts(11, 2, 12)));
    let (day, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&day, &b, "zebra newest", None, None)
        .unwrap();
    s.commit_block(&day, &b).unwrap();

    s.set_clock(Some(ts(10, 1, 12)));
    let old = s.create_page("Archive").unwrap();
    for i in 0..305 {
        write(&mut s, &old, &format!("zebra old {i}"));
    }
    s.set_clock(Some(ts(11, 2, 12)));
    s.sync_index().unwrap();

    let hits = s
        .search_blocks_filtered("zebra", SearchScope::All, SearchAge::Today, 30)
        .unwrap();
    assert!(
        hits.iter().any(|h| h.page == day),
        "Today must still find the day's block (got {} hits)",
        hits.len()
    );
}

/// SR-3: Today and the past windows end today. A future daily page (reachable through
/// day navigation) must not pass Today or Past week/month/year; Any time still has it.
#[test]
fn sr_3_age_window_excludes_future_days() {
    use utopia_store::{SearchAge, SearchScope};

    let mut s = store();
    let (today, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&today, &b, "zebra today", None, None)
        .unwrap();
    s.commit_block(&today, &b).unwrap();
    let (tomorrow, b) = s.open_daily(date(2026, 11, 3)).unwrap();
    s.set_block_text(&tomorrow, &b, "zebra tomorrow", None, None)
        .unwrap();
    s.commit_block(&tomorrow, &b).unwrap();
    s.sync_index().unwrap();

    let days = |age| -> Vec<Option<NaiveDate>> {
        let mut d: Vec<_> = s
            .search_blocks_filtered("zebra", SearchScope::All, age, 30)
            .unwrap()
            .into_iter()
            .map(|h| s.page_row(&h.page).unwrap().and_then(|p| p.daily_date))
            .collect();
        d.sort();
        d
    };
    assert_eq!(
        days(SearchAge::Today),
        vec![Some(date(2026, 11, 2))],
        "tomorrow is not Today"
    );
    assert_eq!(
        days(SearchAge::Week),
        vec![Some(date(2026, 11, 2))],
        "nor in the past week"
    );
    assert_eq!(
        days(SearchAge::AnyTime),
        vec![Some(date(2026, 11, 2)), Some(date(2026, 11, 3))],
        "Any time is unrestricted"
    );
}
