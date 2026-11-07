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
    let queued = SyncClient::<RelayTransport>::queued(&a).unwrap();
    assert!(queued > 0);
    assert_eq!(ca.sync_status(&mut a), SyncStatus::Offline { queued });
    assert_eq!(
        SyncClient::<RelayTransport>::queued(&a).unwrap(),
        queued,
        "outbox retained"
    );
    ca.transport_mut().offline = false;
    ca.sync_once(&mut a).unwrap();
    assert_eq!(SyncClient::<RelayTransport>::queued(&a).unwrap(), 0);
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &p), vec!["one", "two"]);
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
