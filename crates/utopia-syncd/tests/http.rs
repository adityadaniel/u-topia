//! Two devices syncing through a real `utopia-syncd` over HTTP (M3 exit criteria).

use std::sync::Arc;

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;
use utopia_sync::http::HttpTransport;
use utopia_sync::{SyncClient, SyncError, SyncStatus, Transport};
use utopia_syncd::{router, Config, Db};

struct Server {
    base: String,
    db: Arc<Db>,
    _rt: tokio::runtime::Runtime,
}

fn serve(cfg: Config) -> Server {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let app = router(db.clone(), cfg);
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base: format!("http://{addr}"),
        db,
        _rt: rt,
    }
}

fn device(server: &Server, token: Option<&str>) -> (Store, SyncClient<HttpTransport>) {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let t = HttpTransport::new(&server.base, "graph-1", token.map(str::to_string)).unwrap();
    let c = SyncClient::new(t, &s).unwrap();
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

#[test]
fn two_machines_converge_over_http() {
    let server = serve(Config::default());
    let (mut a, mut ca) = device(&server, None);
    let (mut b, mut cb) = device(&server, None);
    let (page, first) = a
        .open_daily(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
        .unwrap();
    a.set_block_text(&page, &first, "shared line", None, None)
        .unwrap();
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page), vec!["shared line"]);
    // Both edit the same page offline.
    a.set_block_text(&page, &first, "shared line, edited on A", None, None)
        .unwrap();
    write(&mut b, &page, "added on B with [[Topic]]");
    for _ in 0..2 {
        ca.sync_once(&mut a).unwrap();
        cb.sync_once(&mut b).unwrap();
    }
    assert_eq!(graph(&mut a), graph(&mut b));
    assert_eq!(
        texts(&mut a, &page),
        vec!["shared line, edited on A", "added on B with [[Topic]]"]
    );
    assert!(a.find_page("Topic").unwrap().is_some());
    assert!(a.doctor().unwrap().is_ok() && b.doctor().unwrap().is_ok());
}

#[test]
fn manifest_is_conditional_and_pushes_are_idempotent() {
    let server = serve(Config::default());
    let (mut a, mut ca) = device(&server, None);
    let p = a.create_page("Once").unwrap();
    write(&mut a, &p, "x");
    ca.sync_once(&mut a).unwrap();
    let t = ca.transport_mut();
    let m = t.manifest(None).unwrap().unwrap();
    assert!(
        t.manifest(Some(m.rev)).unwrap().is_none(),
        "304 Not Modified"
    );
    let pull = t.pull(&p, 0).unwrap();
    let dup = utopia_sync::PushRequest {
        updates: vec![utopia_sync::UpdateIn {
            id: pull.updates[0].id.clone(),
            client_seq: 1,
            frontier: String::new(),
            payload: pull.updates[0].payload.clone(),
        }],
    };
    let r = t.push(&p, dup).unwrap();
    assert_eq!((r.accepted, r.duplicate), (0, 1));
}

/// Serve a relay on an address chosen earlier; the `Arc<Db>` is one store instance.
fn serve_on(rt: &tokio::runtime::Runtime, addr: std::net::SocketAddr, cfg: Config) -> Arc<Db> {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let app = router(db.clone(), cfg);
    let listener = rt.block_on(tokio::net::TcpListener::bind(addr)).unwrap();
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    db
}

/// §12.6: a relay whose database was wiped keeps its URL but not its sequence. The
/// manifest's instance id is what tells the two apart, so the client pulls again instead
/// of trusting a cursor from the old store.
#[test]
fn ac_12_6_a_wiped_relay_is_a_new_cursor_space() {
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let addr = {
        let l = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        l.local_addr().unwrap()
    };
    let base = format!("http://{addr}");
    let first = serve_on(&rt, addr, Config::default());
    let mut a = Store::open_in_memory().unwrap();
    a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let mut ca = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &a).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..5 {
        write(&mut a, &page, &format!("before {i}"));
    }
    ca.sync_once(&mut a).unwrap();
    assert!(!first.manifest("graph-1").unwrap().instance.is_empty());
    drop(ca);
    drop(first);
    drop(rt);

    // The relay's database is wiped and served again on the same address.
    rt = tokio::runtime::Runtime::new().unwrap();
    let second = serve_on(&rt, addr, Config::default());
    let (mut b, mut cb) = {
        let mut s = Store::open_in_memory().unwrap();
        let c = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &s).unwrap();
        s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        (s, c)
    };
    let same = b.create_page("Shared").unwrap();
    assert_eq!(same, page);
    write(&mut b, &same, "after the wipe");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &a).unwrap();
    ca.sync_once(&mut a).unwrap();
    let seen = texts(&mut a, &page);
    assert!(seen.contains(&"after the wipe".to_string()), "{seen:?}");
    assert!(!second.manifest("graph-1").unwrap().instance.is_empty());
}

#[test]
fn auth_is_enforced() {
    let server = serve(Config {
        tokens: vec!["secret".into()],
        ..Default::default()
    });
    let (mut a, mut ca) = device(&server, Some("wrong"));
    a.create_page("Private").unwrap();
    assert!(matches!(ca.sync_once(&mut a), Err(SyncError::Unauthorized)));
    assert_eq!(ca.sync_status(&mut a), SyncStatus::NeedsAuth);
    assert!(
        SyncClient::<HttpTransport>::queued(&mut a).unwrap() > 0,
        "edits stay queued"
    );
    let (mut b, mut cb) = device(&server, Some("secret"));
    b.create_page("Allowed").unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(SyncClient::<HttpTransport>::queued(&mut b).unwrap(), 0);
}

/// §12.6: a batch the relay stored but whose response was lost is re-pushed. Dedupe runs
/// before quota, so its own bytes cannot make it a permanent 413.
#[test]
fn ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota() {
    use utopia_sync::{PushRequest, UpdateIn};
    let db = Db::open_in_memory().unwrap();
    let req = PushRequest {
        updates: vec![UpdateIn {
            id: "client:1".into(),
            client_seq: 1,
            frontier: String::new(),
            payload: vec![7u8; 1000],
        }],
    };
    let first = db
        .push("graph-1", "page-1", "owner", req.clone(), 1500)
        .unwrap();
    assert_eq!(first.accepted, 1);
    let used = db.graph_bytes("graph-1").unwrap();
    assert_eq!(used, 1000);
    // The response was lost: the same batch is pushed again when no new byte fits.
    let again = db
        .push("graph-1", "page-1", "owner", req, used)
        .expect("a duplicate batch needs no new space");
    assert_eq!((again.accepted, again.duplicate), (0, 1));
    // Genuinely new bytes are still refused.
    let new = PushRequest {
        updates: vec![UpdateIn {
            id: "client:2".into(),
            client_seq: 2,
            frontier: String::new(),
            payload: vec![7u8; 10],
        }],
    };
    assert!(matches!(
        db.push("graph-1", "page-1", "owner", new, used),
        Err(utopia_syncd::ApiError::QuotaExceeded)
    ));
}

#[test]
fn quota_rejects_and_keeps_outbox() {
    let server = serve(Config {
        tokens: Vec::new(),
        quota_bytes: 64,
    });
    let (mut a, mut ca) = device(&server, None);
    let p = a.create_page("Big").unwrap();
    write(&mut a, &p, &"x".repeat(500));
    let err = ca.sync_once(&mut a).unwrap_err();
    assert!(
        matches!(&err, SyncError::Transport(m) if m.contains("413")),
        "{err}"
    );
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Offline { queued } if queued > 0));
}

/// A client that hands the push to the relay and then loses the response, as a dropped
/// connection would.
struct LostAck {
    inner: HttpTransport,
    lost: bool,
}

impl Transport for LostAck {
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
        let resp = self.inner.push(page, req);
        if self.lost {
            self.lost = false;
            return Err(SyncError::Transport(
                "connection reset after the push".into(),
            ));
        }
        resp
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

/// §12.6, end to end over HTTP: a batch the relay stored whose response was lost is
/// re-pushed; the replay is a duplicate, so a quota that is nearly full cannot turn it
/// into a permanent 413.
#[test]
fn ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota() {
    let mut a = Store::open_in_memory().unwrap();
    a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = a.create_page("Lost ack").unwrap();
    write(&mut a, &page, &"x".repeat(2000));
    let payload_bytes: usize = a
        .outbox_pending(100)
        .unwrap()
        .iter()
        .map(|row| row.3.len())
        .sum();
    assert!(payload_bytes > 512, "{payload_bytes}");
    // Room for one copy of the batch, not for a second.
    let server = serve(Config {
        tokens: Vec::new(),
        quota_bytes: (payload_bytes + 512) as u64,
    });
    let mut ca = SyncClient::new(
        LostAck {
            inner: HttpTransport::new(&server.base, "graph-1", None).unwrap(),
            lost: true,
        },
        &a,
    )
    .unwrap();
    let first = ca.sync_once(&mut a);
    assert!(
        first.is_err(),
        "the lost response fails that round: {first:?}"
    );

    // The next round re-pushes the very same batch.
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Synced { .. }));
    assert_eq!(
        server.db.graph_bytes("graph-1").unwrap() as usize,
        payload_bytes,
        "the relay stored the batch once"
    );
    assert_eq!(
        SyncClient::<HttpTransport>::queued(&mut a).unwrap(),
        0,
        "the replay was acknowledged"
    );
}

#[test]
fn snapshots_compact_the_log_and_bootstrap_new_devices() {
    let server = serve(Config::default());
    let (mut a, mut ca) = device(&server, None);
    let p = a.create_page("Long").unwrap();
    let blk = write(&mut a, &p, "");
    for i in 0..220 {
        a.set_block_text(&p, &blk, &format!("rev {i}"), None, None)
            .unwrap();
        if i % 20 == 0 {
            ca.sync_once(&mut a).unwrap();
        }
    }
    ca.sync_once(&mut a).unwrap();
    ca.sync_once(&mut a).unwrap();
    assert!(
        server.db.log_len("graph-1", p.as_str()).unwrap() < 50,
        "log truncated"
    );
    let (mut fresh, mut cf) = device(&server, None);
    cf.sync_once(&mut fresh).unwrap();
    assert_eq!(texts(&mut fresh, &p), vec!["rev 219"]);
}

#[test]
fn relay_down_means_offline_not_data_loss() {
    let (mut a, mut ca) = {
        let mut s = Store::open_in_memory().unwrap();
        let t = HttpTransport::new("http://127.0.0.1:9", "graph-1", None).unwrap();
        let c = SyncClient::new(t, &s).unwrap();
        s.create_page("Offline note").unwrap();
        (s, c)
    };
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Offline { queued } if queued > 0));
    assert!(a.find_page("Offline note").unwrap().is_some());
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn e2e_encrypted_sync_relay_sees_only_ciphertext() {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let server = serve(Config::default());
    let enc = |s: &Store, pass: &str| {
        let t = HttpTransport::new(&server.base, "graph-1", None).unwrap();
        let key = GraphKey::derive(pass, "graph-1").unwrap();
        SyncClient::new(Encrypted::new(t, key, "graph-1"), s).unwrap()
    };
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca = enc(&a, "correct horse battery");
    let mut cb = enc(&b, "correct horse battery");
    let p = a.create_page("Diary").unwrap();
    write(&mut a, &p, "a very secret sentence");
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &p), vec!["a very secret sentence"]);
    let stored = server.db.all_payloads().unwrap();
    assert!(!stored.is_empty());
    for payload in stored {
        assert!(
            !contains(&payload, b"secret sentence"),
            "relay stored plaintext"
        );
        assert!(!contains(&payload, b"Diary"));
    }
    // A device with the wrong passphrase cannot read, and says so.
    let mut c = Store::open_in_memory().unwrap();
    let mut cc = enc(&c, "wrong passphrase");
    let err = cc.sync_once(&mut c).unwrap_err();
    assert!(err.to_string().contains("decryption failed"), "{err}");
}

#[test]
fn folder_transport_converges_without_a_server() {
    use utopia_sync::folder::FolderTransport;
    let shared = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca =
        SyncClient::new(FolderTransport::new(shared.path(), "deviceA").unwrap(), &a).unwrap();
    let mut cb =
        SyncClient::new(FolderTransport::new(shared.path(), "deviceB").unwrap(), &b).unwrap();
    let p = a.create_page("Shared via iCloud").unwrap();
    write(&mut a, &p, "from A");
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    write(&mut b, &p, "from B");
    cb.sync_once(&mut b).unwrap();
    ca.sync_once(&mut a).unwrap();
    assert_eq!(graph(&mut a), graph(&mut b));
    assert_eq!(texts(&mut a, &p), vec!["from A", "from B"]);
    // Append-only: each device writes only into its own folder.
    let dirs: Vec<String> = std::fs::read_dir(shared.path().join("pages").join(p.as_str()))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
}

#[test]
fn realtime_notice_on_push() {
    let server = serve(Config::default());
    let base = server.base.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = std::thread::spawn(move || {
        utopia_sync::realtime::listen(&base, "graph-1", None, |n| {
            tx.send(n).unwrap();
            false
        })
    });
    // Give the socket a moment to subscribe before pushing.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (mut a, mut ca) = device(&server, None);
    let p = a.create_page("Live").unwrap();
    ca.sync_once(&mut a).unwrap();
    let notice = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("notice arrives");
    assert_eq!(notice.page_id, p.as_str());
    assert!(notice.head_seq >= 1);
    listener.join().unwrap().unwrap();
}

#[test]
fn lan_relay_serves_and_is_discoverable() {
    let dir = tempfile::tempdir().unwrap();
    let relay = utopia_syncd::lan::serve_lan(
        &dir.path().join("lan.db"),
        "127.0.0.1:0",
        "home-graph",
        Config::default(),
        true,
    )
    .unwrap();
    let base = format!("http://{}", relay.addr);
    // Two devices sync through the in-process LAN relay.
    let mk = |s: &Store| {
        SyncClient::new(HttpTransport::new(&base, "home-graph", None).unwrap(), s).unwrap()
    };
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let (mut ca, mut cb) = (mk(&a), mk(&b));
    let p = a.create_page("Kitchen list").unwrap();
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert!(b.page_row(&p).unwrap().is_some());
    // mDNS discovery: the advertised relay for this graph is found on the local network.
    match utopia_syncd::lan::discover("home-graph", std::time::Duration::from_secs(4)) {
        Some(url) => assert!(url.ends_with(&format!(":{}", relay.addr.port())), "{url}"),
        // Multicast can be unavailable in sandboxed CI; serving above is the contract.
        None => eprintln!("mDNS discovery unavailable in this environment"),
    }
}

#[test]
fn attachments_sync_encrypted_and_verified() {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let server = serve(Config::default());
    let dir = tempfile::tempdir().unwrap();
    let enc = |s: &Store| {
        let t = HttpTransport::new(&server.base, "graph-1", None).unwrap();
        let key = GraphKey::derive("pass phrase", "graph-1").unwrap();
        SyncClient::new(Encrypted::new(t, key, "graph-1"), s).unwrap()
    };
    let mut a = Store::open(dir.path().join("a.utopia")).unwrap();
    let mut b = Store::open(dir.path().join("b.utopia")).unwrap();
    let (mut ca, mut cb) = (enc(&a), enc(&b));
    let photo = b"\x89PNG\r\n\x1a\nsecret pixels".repeat(500);
    let att = a.add_blob("holiday.png", &photo).unwrap();
    let p = a.create_page("Trip").unwrap();
    write(
        &mut a,
        &p,
        &utopia_core::attach::markup(&att.name, &att.hash, &att.mime),
    );
    let r = ca.sync_once(&mut a).unwrap();
    assert_eq!(r.blobs_up, 1);
    assert!(b.missing_blobs().unwrap().is_empty());
    let r = cb.sync_once(&mut b).unwrap();
    assert_eq!(r.blobs_down, 1, "{r:?}");
    assert_eq!(b.blob(&att.hash).unwrap().unwrap(), photo);
    let got = b.attachment(&att.hash).unwrap().unwrap();
    assert_eq!(
        (got.name.as_str(), got.mime.as_str()),
        ("holiday.png", "image/png")
    );
    assert!(b
        .blob_path(&att.hash)
        .unwrap()
        .starts_with(dir.path().join("b.blobs")));
    // The relay holds ciphertext only.
    for payload in server.db.all_payloads().unwrap() {
        assert!(
            !contains(&payload, b"secret pixels"),
            "relay stored plaintext"
        );
    }
    // Nothing more to move.
    let r = ca.sync_once(&mut a).unwrap();
    assert_eq!((r.blobs_up, r.blobs_down), (0, 0));
}

#[test]
fn tampered_blob_is_rejected() {
    let server = serve(Config::default());
    let (mut a, mut ca) = device(&server, None);
    let (mut b, mut cb) = device(&server, None);
    let att = a.add_blob("notes.txt", b"the real notes").unwrap();
    let p = a.create_page("Notes").unwrap();
    write(&mut a, &p, &format!("[notes.txt](blob:{})", att.hash));
    // Someone with relay access swaps the stored bytes before A uploads.
    server
        .db
        .put_blob("graph-1", &att.hash, "x", b"forged notes", 1 << 30)
        .unwrap();
    ca.sync_once(&mut a).unwrap();
    let r = cb.sync_once(&mut b).unwrap();
    assert_eq!(r.blobs_down, 0);
    assert!(
        !b.has_blob(&att.hash),
        "a blob that fails its hash is never stored"
    );
    assert_eq!(b.missing_blobs().unwrap(), vec![att.hash.clone()]);
}
