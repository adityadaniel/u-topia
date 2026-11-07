//! Multi-device scenarios from §12.4 and AC-4.8, through the reference relay.

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;
use utopia_sync::relay::{Relay, RelayTransport};
use utopia_sync::{SyncClient, SyncStatus};

fn device(relay: &std::sync::Arc<Relay>) -> (Store, SyncClient<RelayTransport>) {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let c = SyncClient::new(RelayTransport::new(relay.clone(), "g1"), &s).unwrap();
    (s, c)
}

fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

fn texts(s: &mut Store, page: &PageId) -> Vec<String> {
    let st = s.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, _)| st.block(&id).unwrap().text.clone())
        .collect()
}

fn graph(s: &mut Store) -> Vec<(String, Vec<String>)> {
    s.list_pages(true)
        .unwrap()
        .into_iter()
        .map(|p| (p.title.clone(), texts(s, &p.id)))
        .collect()
}

fn settle(devs: &mut [(&mut Store, &mut SyncClient<RelayTransport>)]) {
    for _ in 0..2 {
        for (s, c) in devs.iter_mut() {
            c.sync_once(s).unwrap();
        }
    }
}

/// §12.3 step 4: the folder cursor is a revision, not a file count. A peer that drops two
/// files and adds two others leaves the count unchanged but must still be pulled.
#[test]
fn ac_12_3_a_folder_revision_with_an_unchanged_file_count_still_pulls() {
    use utopia_sync::folder::FolderTransport;
    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(FolderTransport::new(dir.path(), "a").unwrap(), &a).unwrap();
    let mut cb = SyncClient::new(FolderTransport::new(dir.path(), "b").unwrap(), &b).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..6 {
        write(&mut a, &page, &format!("first {i}"));
    }
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page).len(), 6);

    // Two more local updates, not yet pushed, plus the same number of already-imported
    // files removed: the file count stays the same and only the contents change.
    write(&mut a, &page, "later one");
    write(&mut a, &page, "later two");
    let pending = a.outbox_pending(100).unwrap();
    let page_dir = dir.path().join("pages").join(page.as_str()).join("a");
    let mut existing: Vec<_> = std::fs::read_dir(&page_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "upd"))
        .collect();
    existing.sort();
    assert!(existing.len() >= pending.len());
    for f in existing.iter().take(pending.len()) {
        std::fs::remove_file(f).unwrap();
    }
    for (_, _, client_seq, payload) in &pending {
        std::fs::write(
            page_dir.join(format!("{:012}.upd", client_seq.max(&0))),
            payload,
        )
        .unwrap();
    }

    cb.sync_once(&mut b).unwrap();
    let seen = texts(&mut b, &page);
    assert!(seen.contains(&"later one".to_string()), "{seen:?}");
    assert!(seen.contains(&"later two".to_string()), "{seen:?}");
}

/// §12.3 step 4: a cursor measured on one transport is not compared with another's
/// `head_seq`. Switching from a folder (a file count) to a fresh relay must pull.
#[test]
fn ac_12_3_switching_transport_pulls_the_new_transport_from_zero() {
    use utopia_sync::folder::FolderTransport;
    let relay = Relay::new();
    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(FolderTransport::new(dir.path(), "a").unwrap(), &a).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..10 {
        write(&mut a, &page, &format!("folder {i}"));
    }
    ca.sync_once(&mut a).unwrap();

    // The same page on a fresh relay: its sequence starts at 1, far below the folder's
    // file count.
    let (mut c, mut cc) = device(&relay);
    let same = c.create_page("Shared").unwrap();
    assert_eq!(same, page, "titles make the same page id");
    write(&mut c, &same, "from the relay");
    cc.sync_once(&mut c).unwrap();

    let mut ca = SyncClient::new(RelayTransport::new(relay.clone(), "g1"), &a).unwrap();
    ca.sync_once(&mut a).unwrap();
    let seen = texts(&mut a, &page);
    assert!(seen.contains(&"from the relay".to_string()), "{seen:?}");
}

/// A relay that refuses one page (a 413, a poison batch) and serves every other page.
struct Flaky {
    inner: RelayTransport,
    refuse: PageId,
}

impl utopia_sync::Transport for Flaky {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        if page == &self.refuse {
            return Err(utopia_sync::SyncError::Transport(
                "HTTP 413: quota exceeded".into(),
            ));
        }
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.inner.pull(page, since_seq)
    }
    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// Records how the manifest and pulls were asked for, to prove the conditional request.
struct Conditional {
    inner: RelayTransport,
    asked: std::sync::Arc<std::sync::Mutex<Vec<Option<u64>>>>,
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for Conditional {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.asked.lock().unwrap().push(if_none_match);
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.pulls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.pull(page, since_seq)
    }
    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// §12.3 step 1: an idle round asks with `If-None-Match: <applied rev>` and does no pulls
/// when the relay has not moved.
#[test]
fn ac_12_3_an_unchanged_manifest_is_asked_for_conditionally() {
    let relay = Relay::new();
    let mut a = Store::open_in_memory().unwrap();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let pulls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ca = SyncClient::new(
        Conditional {
            inner: RelayTransport::new(relay.clone(), "g1"),
            asked: asked.clone(),
            pulls: pulls.clone(),
        },
        &a,
    )
    .unwrap();
    let page = a.create_page("Once").unwrap();
    write(&mut a, &page, "hello");
    ca.sync_once(&mut a).unwrap();
    let rev: u64 = a.sync_get("rev").unwrap().unwrap().parse().unwrap();

    let before = asked.lock().unwrap().len();
    ca.sync_once(&mut a).unwrap();
    let asked_all = asked.lock().unwrap().clone();
    assert_eq!(
        asked_all[before],
        Some(rev),
        "the second round sends the applied rev: {asked_all:?}"
    );
    assert_eq!(
        pulls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the first round pulled"
    );
}

/// A relay whose pull payloads for one page are corrupt: a shard the relay accepted but
/// this device cannot import.
struct Corrupt {
    inner: RelayTransport,
    page: PageId,
}

impl utopia_sync::Transport for Corrupt {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        let mut resp = self.inner.pull(page, since_seq)?;
        if page == &self.page {
            for u in &mut resp.updates {
                u.payload = b"not a loro update".to_vec();
            }
        }
        Ok(resp)
    }
    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// §12.3 steps 3–4: an import that fails must leave that page queued and its cursor
/// unmoved, instead of recording the acks and losing the position.
#[test]
fn ac_12_3_a_failed_import_does_not_mark_the_page_synced() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let broken = a.create_page("Broken").unwrap();
    write(&mut a, &broken, "the relay's copy will not import");
    let readable = b.create_page("Readable").unwrap();
    write(&mut b, &readable, "written on b");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(
        Corrupt {
            inner: RelayTransport::new(relay.clone(), "g1"),
            page: broken.clone(),
        },
        &a,
    )
    .unwrap();
    let report = ca
        .sync_once(&mut a)
        .expect("the readable page still imports");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&broken));
    assert_eq!(texts(&mut a, &readable), vec!["written on b"]);
    // The broken page's push was accepted, but its import failed: its rows stay queued.
    let queued = a.outbox_pending(100).unwrap();
    assert!(!queued.is_empty(), "the failed page is still queued");
    assert!(
        queued.iter().all(|(_, p, _, _)| p == &broken),
        "the readable page's acks were recorded: {queued:?}"
    );
    assert_eq!(
        a.sync_get("rev").unwrap(),
        None,
        "an unfinished manifest is fetched again next round"
    );
}

/// §12.3 steps 3–4: when the batch import fails on one page, every other page must still
/// be committed and reported. The corrupt page is tried both first and last in `PageId`
/// order: when it is last, the readable page's shard is already in memory when the batch
/// fails, so a naive per-page retry would skip it as "already imported".
#[test]
fn ac_12_3_a_failed_batch_import_still_stores_the_readable_pages() {
    let relay = Relay::new();
    let (mut b, mut cb) = device(&relay);
    let alpha = b.create_page("Alpha").unwrap();
    let bravo = b.create_page("Bravo").unwrap();
    write(&mut b, &alpha, "alpha from b");
    write(&mut b, &bravo, "bravo from b");
    cb.sync_once(&mut b).unwrap();
    let (low, high) = if alpha < bravo {
        (alpha.clone(), bravo.clone())
    } else {
        (bravo.clone(), alpha.clone())
    };
    for broken in [low, high] {
        let readable = if broken == alpha {
            bravo.clone()
        } else {
            alpha.clone()
        };
        let want = if readable == alpha {
            "alpha from b"
        } else {
            "bravo from b"
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        let mut a = Store::open(&path).unwrap();
        a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let mut ca = SyncClient::new(
            Corrupt {
                inner: RelayTransport::new(relay.clone(), "g1"),
                page: broken.clone(),
            },
            &a,
        )
        .unwrap();
        let report = ca.sync_once(&mut a).expect("the readable page imports");
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].page.as_ref(), Some(&broken));
        assert!(
            report.imported.contains(&readable),
            "the readable page is reported: {report:?}"
        );
        assert_eq!(texts(&mut a, &readable), vec![want]);
        // Reopen from disk: the readable page must have been committed, not just left in
        // the in-memory shard the failed batch import touched.
        drop(ca);
        drop(a);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(
            texts(&mut reopened, &readable),
            vec![want],
            "committed to disk (corrupt page {broken:?})"
        );
    }
}

/// An encrypted device: same relay, payloads sealed with the graph key.
fn encrypted_device(
    relay: &std::sync::Arc<Relay>,
    passphrase: &str,
) -> (
    Store,
    SyncClient<utopia_sync::e2e::Encrypted<RelayTransport>>,
) {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let key = GraphKey::derive(passphrase, "g1").unwrap();
    let t = Encrypted::new(RelayTransport::new(relay.clone(), "g1"), key, "g1");
    let c = SyncClient::new(t, &s).unwrap();
    (s, c)
}

/// §12.6: a page this device cannot read (here: plaintext left by a device without the
/// passphrase) must not stop every other page's pull, nor lose their cursors.
#[test]
fn ac_12_6_one_unreadable_page_does_not_halt_the_others() {
    let relay = Relay::new();
    let (mut plain, mut cp) = device(&relay);
    let poison = plain.create_page("Plaintext").unwrap();
    write(&mut plain, &poison, "not for encrypted readers");
    cp.sync_once(&mut plain).unwrap();

    let (mut a, mut ca) = encrypted_device(&relay, "correct horse");
    let sealed = a.create_page("Sealed").unwrap();
    write(&mut a, &sealed, "readable by both encrypted devices");
    let report = ca
        .sync_once(&mut a)
        .expect("the unreadable page does not fail the round");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&poison));

    let (mut b, mut cb) = encrypted_device(&relay, "correct horse");
    let report = cb
        .sync_once(&mut b)
        .expect("the second device gets the readable page");
    assert_eq!(
        texts(&mut b, &sealed),
        vec!["readable by both encrypted devices"]
    );
    assert_eq!(
        report.failed.len(),
        1,
        "only the poison page failed: {report:?}"
    );
}

/// §12.3 steps 2–5: a page the relay refuses must not cost the other pages their acks,
/// nor stop the pull of what the relay does have.
#[test]
fn ac_12_3_a_refused_page_keeps_the_other_acks_and_the_pulls() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let refused = a.create_page("Refused").unwrap();
    let accepted = a.create_page("Accepted").unwrap();
    write(&mut a, &refused, "this batch is too big");
    write(&mut a, &accepted, "this one fits");
    let remote = b.create_page("Remote").unwrap();
    write(&mut b, &remote, "written on b");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(
        Flaky {
            inner: RelayTransport::new(relay.clone(), "g1"),
            refuse: refused.clone(),
        },
        &a,
    )
    .unwrap();
    let report = ca
        .sync_once(&mut a)
        .expect("the round carries on past the refused page");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&refused));
    assert!(
        report.pushed > 0,
        "the other page was acknowledged: {report:?}"
    );
    assert_eq!(
        texts(&mut a, &remote),
        vec!["written on b"],
        "the pull still ran"
    );
    // The accepted page's outbox rows are marked sent; only the refused page's remain.
    let queued = a.outbox_pending(100).unwrap();
    assert!(!queued.is_empty());
    assert!(
        queued.iter().all(|(_, p, _, _)| p == &refused),
        "only the refused page is queued: {queued:?}"
    );
}

#[test]
fn two_devices_converge_on_different_blocks_and_same_block() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let (page, first) = a.open_daily(a.today()).unwrap();
    a.set_block_text(&page, &first, "hello world", None, None)
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(texts(&mut b, &page), vec!["hello world"]);
    // Offline: both edit the same block and add different blocks.
    a.set_block_text(&page, &first, "hello big world", None, None)
        .unwrap();
    b.set_block_text(&page, &first, "hello world!", None, None)
        .unwrap();
    write(&mut a, &page, "from a");
    write(&mut b, &page, "from b");
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    let t = texts(&mut a, &page);
    assert_eq!(t[0], "hello big world!", "both inserts survive");
    assert!(t.contains(&"from a".to_string()) && t.contains(&"from b".to_string()));
    for s in [&mut a, &mut b] {
        assert!(s.doctor().unwrap().is_ok());
    }
}

#[test]
fn same_daily_note_offline_is_one_page() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let (pa, ba) = a.open_daily(a.today()).unwrap();
    let (pb, bb) = b.open_daily(b.today()).unwrap();
    assert_eq!(pa, pb, "daily ids are deterministic");
    a.set_block_text(&pa, &ba, "a's morning", None, None)
        .unwrap();
    b.set_block_text(&pb, &bb, "b's morning", None, None)
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    assert_eq!(a.list_pages(true).unwrap().len(), 1);
    assert_eq!(texts(&mut a, &pa).len(), 2);
}

#[test]
fn offline_edits_queue_and_flush() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    ca.transport_mut().offline = true;
    let p = a.create_page("Queued").unwrap();
    write(&mut a, &p, "one");
    write(&mut a, &p, "two");
    let queued = SyncClient::<RelayTransport>::queued(&mut a).unwrap();
    assert!(queued > 0);
    assert_eq!(ca.sync_status(&mut a), SyncStatus::Offline { queued });
    assert_eq!(
        SyncClient::<RelayTransport>::queued(&mut a).unwrap(),
        queued,
        "outbox retained"
    );
    ca.transport_mut().offline = false;
    ca.sync_once(&mut a).unwrap();
    assert_eq!(SyncClient::<RelayTransport>::queued(&mut a).unwrap(), 0);
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &p), vec!["one", "two"]);
}

/// §9.3/§12.3: the "N changes queued" count must not lag behind a debounced edit.
#[test]
fn ac_12_3_queued_counts_edits_still_waiting_for_the_writer() {
    let mut s = Store::open_in_memory().unwrap();
    s.set_debounced(true).unwrap();
    let page = s.create_page("Debounced").unwrap();
    let block = write(&mut s, &page, "first");
    let before = SyncClient::<RelayTransport>::queued(&mut s).unwrap();
    // A plain text edit stays in memory until a flush: still unsent, so still queued.
    s.set_block_text(&page, &block, "first, edited", None, None)
        .unwrap();
    assert!(s.has_pending(), "the edit is debounced");
    let after = SyncClient::<RelayTransport>::queued(&mut s).unwrap();
    assert_eq!(after, before + 1, "the waiting edit is counted");
    assert!(!s.has_pending(), "counting flushed it");
}

#[test]
fn ac_4_8_merge_while_peer_offline_converges() {
    let relay = Relay::new();
    let (mut d1, mut c1) = device(&relay);
    let (mut d2, mut c2) = device(&relay);
    let a = d1.create_page("Alpha").unwrap();
    write(&mut d1, &a, "a0");
    let b = d1.create_page("Bravo").unwrap();
    write(&mut d1, &b, "b0");
    settle(&mut [(&mut d1, &mut c1), (&mut d2, &mut c2)]);
    d1.merge_pages(&b, &a).unwrap();
    for i in 0..3 {
        write(&mut d2, &b, &format!("offline {i}"));
    }
    settle(&mut [(&mut d1, &mut c1), (&mut d2, &mut c2)]);
    assert_eq!(graph(&mut d1), graph(&mut d2));
    let merged = texts(&mut d1, &a);
    assert_eq!(merged.len(), 5, "{merged:?}");
    for s in [&mut d1, &mut d2] {
        assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_some());
        assert!(s.doctor().unwrap().is_ok());
    }
}

#[test]
fn concurrent_renames_keep_both_titles_resolvable() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let p = a.create_page("Alpha").unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    a.rename_page(&p, "Alpha One").unwrap();
    b.rename_page(&p, "Alpha Two").unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    for t in ["Alpha", "Alpha One", "Alpha Two"] {
        assert_eq!(a.find_page(t).unwrap(), Some(p.clone()), "{t}");
    }
}

#[test]
fn snapshot_compaction_bootstraps_new_device() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let p = a.create_page("Long").unwrap();
    let blk = write(&mut a, &p, "");
    for i in 0..250 {
        a.set_block_text(&p, &blk, &format!("rev {i}"), None, None)
            .unwrap();
        // Each edit is its own sync so the relay log grows past the threshold.
        if i % 10 == 0 {
            ca.sync_once(&mut a).unwrap();
        }
    }
    ca.sync_once(&mut a).unwrap();
    let r = ca.sync_once(&mut a).unwrap();
    assert!(
        relay.log_len("g1", p.as_str()) < 200,
        "log truncated (snapshots: {})",
        r.snapshots
    );
    let (mut fresh, mut cf) = device(&relay);
    cf.sync_once(&mut fresh).unwrap();
    assert_eq!(texts(&mut fresh, &p), vec!["rev 249"]);
}

#[test]
fn pushes_are_idempotent() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let p = a.create_page("Once").unwrap();
    write(&mut a, &p, "x");
    let pending = a.outbox_pending(100).unwrap();
    ca.sync_once(&mut a).unwrap();
    // Replay the same outbox rows (e.g. a lost ack): the relay reports duplicates.
    let mut t = RelayTransport::new(relay.clone(), "g1");
    let client_id = a.sync_get("client_id").unwrap().unwrap();
    use utopia_sync::Transport;
    let (_, page, client_seq, payload) = pending[0].clone();
    let resp = t
        .push(
            &page,
            utopia_sync::PushRequest {
                updates: vec![utopia_sync::UpdateIn {
                    id: format!("{client_id}:{client_seq}"),
                    client_seq,
                    frontier: String::new(),
                    payload,
                }],
            },
        )
        .unwrap();
    assert_eq!((resp.accepted, resp.duplicate), (0, 1));
}
