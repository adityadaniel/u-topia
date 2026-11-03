//! Multi-device scenarios from §12.4 and AC-4.8, through the reference relay.

use chrono::NaiveDate;
use trellis_core::outline;
use trellis_core::{BlockId, PageId};
use trellis_store::Store;
use trellis_sync::relay::{Relay, RelayTransport};
use trellis_sync::{SyncClient, SyncStatus};

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
    use trellis_sync::Transport;
    let (_, page, client_seq, payload) = pending[0].clone();
    let resp = t
        .push(
            &page,
            trellis_sync::PushRequest {
                updates: vec![trellis_sync::UpdateIn {
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
