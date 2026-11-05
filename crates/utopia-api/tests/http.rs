//! The automation API over real HTTP, with the store owned by another thread as in the app.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::{json, Value};
use utopia_api::{serve, ApiServer};
use utopia_store::Store;

struct Running {
    server: Arc<ApiServer>,
    stop: Arc<AtomicBool>,
    owner: Option<std::thread::JoinHandle<Store>>,
    _dir: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.owner.take() {
            let _ = t.join();
        }
    }
}

fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let server = Arc::new(serve("127.0.0.1:0", "secret-token".into()).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (server.clone(), stop.clone());
    let owner = std::thread::spawn(move || {
        let mut store = Store::open(path).unwrap();
        store.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
        while !st2.load(Ordering::SeqCst) {
            if let Some(call) = s2.recv_timeout(Duration::from_millis(20)) {
                call.run(&mut store);
            }
        }
        store
    });
    Running {
        server,
        stop,
        owner: Some(owner),
        _dir: dir,
    }
}

fn req(r: &Running, method: &str, path: &str) -> ureq::Request {
    ureq::request(method, &format!("{}{path}", r.server.url()))
        .set("Authorization", "Bearer secret-token")
}

// Mirrors ureq's own `send_string`, whose error type is large.
#[allow(clippy::result_large_err)]
trait SendJson {
    fn send_string_json(self, v: Value) -> Result<ureq::Response, ureq::Error>;
}

impl SendJson for ureq::Request {
    fn send_string_json(self, v: Value) -> Result<ureq::Response, ureq::Error> {
        self.set("Content-Type", "application/json")
            .send_string(&v.to_string())
    }
}

fn ok(resp: Result<ureq::Response, ureq::Error>) -> Value {
    match resp {
        Ok(r) => serde_json::from_str(&r.into_string().unwrap()).unwrap(),
        Err(ureq::Error::Status(code, r)) => panic!("{code}: {}", r.into_string().unwrap()),
        Err(e) => panic!("{e}"),
    }
}

fn status(resp: Result<ureq::Response, ureq::Error>) -> u16 {
    match resp {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn write_then_read_over_http() {
    let r = start();
    // Capture lands in today's (empty) first block.
    let b =
        ok(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "met [[Dana]] #crm"})));
    assert_eq!(b["text"], "met [[Dana]] #crm");
    let day = ok(req(&r, "GET", "/v1/journal/today").call());
    assert_eq!(day["title"], "November 2nd, 2026");
    assert_eq!(day["blocks"].as_array().unwrap().len(), 1);
    // Append under a block, creating the page on demand.
    let parent = ok(req(&r, "POST", "/v1/pages/Reading%20list/blocks")
        .send_string_json(json!({"text": "Books", "create": true})));
    let pid = parent["id"].as_str().unwrap().to_string();
    ok(req(&r, "POST", "/v1/pages/reading%20LIST/blocks")
        .send_string_json(json!({"text": "Dune #toread", "parent": pid})));
    let page = ok(req(&r, "GET", "/v1/pages/Reading%20list").call());
    assert_eq!(page["blocks"][0]["text"], "Books");
    assert_eq!(page["blocks"][0]["children"][0]["text"], "Dune #toread");
    // Edit a block; references follow.
    let child = page["blocks"][0]["children"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(req(&r, "PATCH", &format!("/v1/blocks/{child}"))
        .send_string_json(json!({"text": "Dune #toread [[Dana]]"})));
    let back = ok(req(&r, "GET", "/v1/pages/Dana/backlinks").call());
    assert_eq!(back["total"], 2);
    let got = ok(req(&r, "GET", &format!("/v1/blocks/{child}")).call());
    assert_eq!(got["text"], "Dune #toread [[Dana]]");
    // Search and query.
    let s = ok(req(&r, "GET", "/v1/search?q=dune").call());
    assert_eq!(s["blocks"].as_array().unwrap().len(), 1);
    let q =
        ok(req(&r, "POST", "/v1/query")
            .send_string_json(json!({"query": "{and: [[Dana]] #toread}"})));
    assert_eq!(q["total"], 1);
    assert_eq!(q["hits"][0]["page_title"], "Reading list");
    let pages = ok(req(&r, "GET", "/v1/pages").call());
    let titles: Vec<&str> = pages["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Reading list") && titles.contains(&"Dana"));
}

#[test]
fn errors_are_json_with_status() {
    let r = start();
    assert_eq!(status(req(&r, "GET", "/v1/pages/Nowhere").call()), 404);
    assert_eq!(
        status(req(&r, "POST", "/v1/pages/Nowhere/blocks").send_string_json(json!({"text": "x"}))),
        404,
        "no implicit page creation"
    );
    assert_eq!(
        status(req(&r, "POST", "/v1/query").send_string_json(json!({"query": "{and: [[A]]"}))),
        400
    );
    assert_eq!(status(req(&r, "GET", "/v1/journal/someday").call()), 400);
    assert_eq!(status(req(&r, "GET", "/v1/blocks/nope").call()), 404);
    assert_eq!(
        status(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "  "}))),
        400
    );
}

#[test]
fn only_authorized_local_non_browser_requests() {
    let r = start();
    let url = format!("{}/v1/pages", r.server.url());
    assert_eq!(status(ureq::get(&url).call()), 401, "no token");
    assert_eq!(
        status(
            ureq::get(&url)
                .set("Authorization", "Bearer wrong-token!")
                .call()
        ),
        401
    );
    assert_eq!(
        status(
            req(&r, "GET", "/v1/pages")
                .set("Origin", "https://evil.example")
                .call()
        ),
        403,
        "browsers cannot call it"
    );
    assert_eq!(
        status(
            req(&r, "GET", "/v1/pages")
                .set("Host", "evil.example")
                .call()
        ),
        403,
        "DNS rebinding"
    );
    assert!(
        serve("0.0.0.0:0", "t".into()).is_err(),
        "never binds publicly"
    );
    // Discovery file is private to the user.
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("api.json");
    r.server.write_discovery(&f).unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    assert_eq!(v["token"], "secret-token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(utopia_api::new_token().len(), 64);
}
