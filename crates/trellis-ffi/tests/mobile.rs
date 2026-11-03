//! The mobile surface (§14.3): daily capture, references, backlinks, offline + sync.

use std::sync::Arc;

use trellis_ffi::{FfiError, Graph};

#[test]
fn capture_links_and_backlinks() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path().join("m.trellis").to_string_lossy().into(), None).unwrap();
    g.set_today(Some("2026-11-02".into())).unwrap();
    let today = g.today().unwrap();
    assert_eq!(today.title, "November 2nd, 2026");
    assert_eq!(today.date.as_deref(), Some("2026-11-02"));
    assert_eq!(today.blocks.len(), 1, "one empty block to type into");
    // The first capture fills the empty block; the next ones append.
    let b1 = g
        .append_block(today.id.clone(), "Call [[Dana]] about #taxes".into())
        .unwrap();
    assert_eq!(b1.plain, "Call Dana about #taxes");
    g.append_block(today.id.clone(), "Lunch with [[Dana]]".into())
        .unwrap();
    let today = g.today().unwrap();
    assert_eq!(today.blocks.len(), 2);
    let dana = g
        .find_page("dana".into())
        .unwrap()
        .expect("page created on commit");
    let refs = g.backlinks(dana.id.clone()).unwrap();
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|r| r.page_title == "November 2nd, 2026"));
    assert_eq!(g.search_titles("da".into(), 5).unwrap(), vec!["Dana"]);
    // Edits re-index references.
    g.set_block_text(b1.id.clone(), "Call the accountant".into())
        .unwrap();
    assert_eq!(g.backlinks(dana.id.clone()).unwrap().len(), 1);
    assert!(matches!(
        g.set_block_text("nope".into(), "x".into()),
        Err(FfiError::NotFound { .. })
    ));
    assert!(g.find_page("Nobody".into()).unwrap().is_none());
}

#[test]
fn phone_and_desktop_sync_through_a_relay() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let db = Arc::new(trellis_syncd::Db::open_in_memory().unwrap());
    let app = trellis_syncd::router(db, trellis_syncd::Config::default());
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    rt.spawn(async move { axum_serve(listener, app).await });
    let phone = Graph::in_memory().unwrap();
    let laptop = Graph::in_memory().unwrap();
    for g in [&phone, &laptop] {
        g.set_today(Some("2026-11-02".into())).unwrap();
    }
    let sync = |g: &Graph| {
        g.sync(url.clone(), "g1".into(), None, Some("shared secret".into()))
            .unwrap()
    };
    // Offline capture on the phone, then sync.
    let day = phone.today().unwrap();
    phone
        .append_block(day.id.clone(), "idea on the train [[Atlas]]".into())
        .unwrap();
    assert!(sync(&phone).pushed > 0);
    sync(&laptop);
    let on_laptop = laptop.today().unwrap();
    assert!(on_laptop
        .blocks
        .iter()
        .any(|b| b.text == "idea on the train [[Atlas]]"));
    let atlas = laptop.find_page("Atlas".into()).unwrap().unwrap();
    assert_eq!(laptop.backlinks(atlas.id).unwrap().len(), 1);
    // And back.
    laptop
        .append_block(on_laptop.id.clone(), "reply from the desk".into())
        .unwrap();
    sync(&laptop);
    sync(&phone);
    let texts: Vec<String> = phone
        .today()
        .unwrap()
        .blocks
        .into_iter()
        .map(|b| b.text)
        .collect();
    assert!(
        texts.contains(&"reply from the desk".to_string()),
        "{texts:?}"
    );
    // Wrong E2E passphrase is a sync error, not corruption.
    let stranger = Graph::in_memory().unwrap();
    let e = stranger
        .sync(url.clone(), "g1".into(), None, Some("wrong".into()))
        .unwrap_err();
    assert!(matches!(e, FfiError::Sync { .. }), "{e}");
}

async fn axum_serve(listener: tokio::net::TcpListener, app: trellis_syncd::Router) {
    trellis_syncd::serve(listener, app).await.unwrap();
}
