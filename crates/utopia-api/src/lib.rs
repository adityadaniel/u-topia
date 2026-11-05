//! Local automation API (P2, §16 "In-app automation API → Local HTTP API + `utopia://`").
//!
//! The server only accepts loopback connections, requires a bearer token, and refuses
//! browser requests (any `Origin` header, or a `Host` that is not loopback — DNS
//! rebinding), so a web page cannot drive the graph. It never touches the store itself:
//! each request becomes an [`ApiCall`] that the owner of the [`Store`] (the app's main
//! thread, or `utopia serve-api`) runs with [`handle`], so the API sees exactly the
//! state the UI does and every write goes through the normal command path.
//!
//! Routes (JSON in and out):
//!
//! | Method | Path | Body / query |
//! |---|---|---|
//! | GET | `/v1/pages` | `?daily=true` to include journal pages |
//! | GET | `/v1/pages/{title}` | the page and its outline |
//! | GET | `/v1/pages/{title}/backlinks` | Linked References |
//! | POST | `/v1/pages/{title}/blocks` | `{"text", "parent"?, "create"?}` |
//! | GET | `/v1/blocks/{id}` | one block |
//! | PATCH | `/v1/blocks/{id}` | `{"text"}` |
//! | GET | `/v1/journal/{date}` | `today` or `YYYY-MM-DD` |
//! | GET | `/v1/search` | `?q=&limit=` |
//! | POST | `/v1/query` | `{"query", "limit"?}` — query-block syntax |
//! | POST | `/v1/capture` | `{"text"}` — appended to today's page |

use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use axum::{
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use utopia_core::{outline, title::normalize_title, BlockId, PageId};
use utopia_store::{resolve_page_key, RefQuery, Store, StoreError};

mod url;
pub use url::{parse_url, UrlAction};

/// One API operation. Also the JSON request plugins send (`{"op": "search", "q": …}`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    ListPages {
        #[serde(default)]
        daily: bool,
    },
    GetPage {
        title: String,
    },
    Backlinks {
        title: String,
    },
    AppendBlock {
        title: String,
        text: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        create: bool,
    },
    GetBlock {
        id: String,
    },
    UpdateBlock {
        id: String,
        text: String,
    },
    Journal {
        date: String,
    },
    Search {
        q: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Query {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Capture {
        text: String,
    },
}

fn default_limit() -> usize {
    20
}

impl Op {
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Op::AppendBlock { .. } | Op::UpdateBlock { .. } | Op::Capture { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        let status = match &e {
            StoreError::NotFound(_) => 404,
            StoreError::Invalid(_) | StoreError::Domain(_) => 400,
            _ => 500,
        };
        Self::new(status, e.to_string())
    }
}

/// The result of an operation: the JSON body and the pages a write touched (so the UI
/// can refresh them).
#[derive(Debug, Clone)]
pub struct Outcome {
    pub body: Value,
    pub changed: Vec<PageId>,
}

fn read(body: Value) -> Result<Outcome, ApiError> {
    Ok(Outcome {
        body,
        changed: Vec::new(),
    })
}

/// The page a title names, through the same resolver as `[[links]]` (dates, titles,
/// aliases).
fn page_by_title(store: &Store, title: &str) -> Result<PageId, ApiError> {
    resolve_page_key(store.conn(), &normalize_title(title), store.date_format())?
        .ok_or_else(|| ApiError::new(404, format!("no page titled {title:?}")))
}

fn block_json(store: &mut Store, page: &PageId, id: &BlockId) -> Result<Value, ApiError> {
    let st = store.page_state(page)?;
    let b = st
        .block(id)
        .filter(|_| st.is_live(id))
        .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?;
    Ok(json!({
        "id": id.as_str(),
        "page": page.as_str(),
        "text": b.text,
        "kind": b.kind.type_str(),
        "heading": b.kind.heading_level(),
        "checked": b.checked,
    }))
}

fn page_json(store: &mut Store, page: &PageId) -> Result<Value, ApiError> {
    let fmt = store.date_format().clone();
    let st = store.page_state(page)?.clone();
    if st.meta.deleted_at.is_some() {
        return Err(ApiError::new(404, "page is in the trash"));
    }
    // Rebuild the tree from the preorder walk: each row's parent is the nearest earlier
    // row one level up.
    fn node(st: &utopia_core::tree::PageState, id: &BlockId) -> Value {
        let b = st.block(id).expect("live row exists");
        json!({
            "id": id.as_str(),
            "text": b.text,
            "kind": b.kind.type_str(),
            "heading": b.kind.heading_level(),
            "checked": b.checked,
            "children": [],
        })
    }
    let mut stack: Vec<Value> = Vec::new();
    let mut roots = Vec::new();
    let rows = st.live_preorder();
    let close = |stack: &mut Vec<Value>, roots: &mut Vec<Value>, depth: usize| {
        while stack.len() > depth {
            let done = stack.pop().expect("non-empty");
            match stack.last_mut() {
                Some(parent) => parent["children"]
                    .as_array_mut()
                    .expect("children array")
                    .push(done),
                None => roots.push(done),
            }
        }
    };
    for (id, depth) in rows {
        close(&mut stack, &mut roots, depth);
        stack.push(node(&st, &id));
    }
    close(&mut stack, &mut roots, 0);
    Ok(json!({
        "id": page.as_str(),
        "title": utopia_store::display_title(&st, &fmt),
        "kind": if st.meta.daily_date.is_some() { "daily" } else { "page" },
        "date": st.meta.daily_date.map(|d| d.to_string()),
        "aliases": st.meta.aliases,
        "blocks": roots,
    }))
}

fn parse_date(store: &Store, s: &str) -> Result<NaiveDate, ApiError> {
    if s == "today" {
        return Ok(store.today());
    }
    utopia_core::date::parse_iso(s)
        .or_else(|| store.date_format().parse(s))
        .ok_or_else(|| ApiError::new(400, format!("not a date: {s:?}")))
}

/// Run one operation against the store. Writes commit before returning.
pub fn handle(store: &mut Store, op: Op) -> Result<Outcome, ApiError> {
    match op {
        Op::ListPages { daily } => {
            let pages = store.list_pages(daily)?;
            read(json!({ "pages": pages }))
        }
        Op::GetPage { title } => {
            let page = page_by_title(store, &title)?;
            read(page_json(store, &page)?)
        }
        Op::Backlinks { title } => {
            let page = page_by_title(store, &title)?;
            let refs = store.linked_references(&page, &RefQuery::default())?;
            read(serde_json::to_value(refs).map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        Op::Journal { date } => {
            let d = parse_date(store, &date)?;
            let key = normalize_title(&store.date_format().format(d));
            let page = resolve_page_key(store.conn(), &key, store.date_format())?
                .ok_or_else(|| ApiError::new(404, format!("no journal page for {d}")))?;
            read(page_json(store, &page)?)
        }
        Op::GetBlock { id } => {
            let id = BlockId(id);
            let page = store
                .block_row(&id)?
                .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?
                .page_id;
            read(block_json(store, &page, &id)?)
        }
        Op::Search { q, limit } => {
            let titles = store.search_titles(&q, limit)?;
            let blocks = store.search_blocks(&q, limit)?;
            read(json!({ "pages": titles, "blocks": blocks }))
        }
        Op::Query { query, limit } => {
            let expr = utopia_core::query::parse(&query).map_err(|e| ApiError::new(400, e))?;
            let r = store.run_query(&expr, limit)?;
            read(serde_json::to_value(r).map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        Op::Capture { text } => {
            if text.trim().is_empty() {
                return Err(ApiError::new(400, "text is empty"));
            }
            let (page, block) = store.capture(&text)?;
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &block)?,
                changed: vec![page],
            })
        }
        Op::UpdateBlock { id, text } => {
            let id = BlockId(id);
            let page = store
                .block_row(&id)?
                .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?
                .page_id;
            if !store.page_state(&page)?.is_live(&id) {
                return Err(ApiError::new(404, format!("no block {id}")));
            }
            store.seal_undo();
            store.set_block_text(&page, &id, &text, None, None)?;
            store.commit_block(&page, &id)?;
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &id)?,
                changed: vec![page],
            })
        }
        Op::AppendBlock {
            title,
            text,
            parent,
            create,
        } => {
            let page = match page_by_title(store, &title) {
                Ok(p) => p,
                Err(e) if e.status == 404 && create => store.create_page(&title)?,
                Err(e) => return Err(e),
            };
            let parent = parent.map(BlockId);
            let st = store.page_state(&page)?;
            if let Some(p) = &parent {
                if !st.is_live(p) {
                    return Err(ApiError::new(404, format!("no block {p} on {title:?}")));
                }
            }
            // A page whose only block is empty (a fresh page or journal day) fills it.
            let roots = st.live_roots();
            let reuse = match (&parent, roots.as_slice()) {
                (None, [only]) if st.block(only).is_some_and(|b| b.text.is_empty()) => {
                    !st.has_live_children(only)
                }
                _ => false,
            };
            store.seal_undo();
            let block = if reuse {
                roots[0].clone()
            } else {
                let plan = outline::append_block(store.page_state(&page)?, parent.as_ref());
                store
                    .execute_plan(&page, plan, None)?
                    .ok_or_else(|| ApiError::new(500, "insert produced no block"))?
                    .block
            };
            store.set_block_text(&page, &block, &text, None, None)?;
            store.commit_block(&page, &block)?;
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &block)?,
                changed: vec![page],
            })
        }
    }
}

/// A request waiting for the store's owner.
pub struct ApiCall {
    pub op: Op,
    reply: tokio::sync::oneshot::Sender<Result<Value, ApiError>>,
}

impl ApiCall {
    /// Run the operation and answer the HTTP request. Returns the pages a write changed.
    pub fn run(self, store: &mut Store) -> Vec<PageId> {
        let r = handle(store, self.op);
        let (reply, changed) = match r {
            Ok(o) => (Ok(o.body), o.changed),
            Err(e) => (Err(e), Vec::new()),
        };
        let _ = self.reply.send(reply);
        changed
    }
}

/// A running API server. Dropping it stops the server.
pub struct ApiServer {
    pub addr: SocketAddr,
    pub token: String,
    calls: std::sync::Mutex<mpsc::Receiver<ApiCall>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ApiServer {
    pub fn try_recv(&self) -> Option<ApiCall> {
        self.calls.lock().ok()?.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<ApiCall> {
        self.calls.lock().ok()?.recv_timeout(d).ok()
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Write `{"url", "token"}` for scripts to discover the server, readable by the user
    /// only. The token is a per-launch secret and lives outside the graph file (§15.6).
    pub fn write_discovery(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_vec_pretty(&json!({ "url": self.url(), "token": self.token }))
            .map_err(std::io::Error::other)?;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::remove_file(path);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(&body)
        }
        #[cfg(not(unix))]
        std::fs::write(path, body)
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 256 random bits, hex.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("system randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Clone)]
struct Ctx {
    calls: mpsc::Sender<ApiCall>,
    token: String,
    port: u16,
}

/// Start the server on a loopback address (`127.0.0.1:0` picks a free port).
pub fn serve(addr: &str, token: String) -> std::io::Result<ApiServer> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the automation API only listens on loopback",
        ));
    }
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let (calls_tx, calls) = mpsc::channel();
    let (shutdown, stop) = tokio::sync::oneshot::channel::<()>();
    let ctx = Ctx {
        calls: calls_tx,
        token: token.clone(),
        port: addr.port(),
    };
    let thread = std::thread::Builder::new()
        .name("utopia-api".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                let _ = axum::serve(listener, router(ctx))
                    .with_graceful_shutdown(async {
                        let _ = stop.await;
                    })
                    .await;
            });
        })?;
    Ok(ApiServer {
        addr,
        token,
        calls: std::sync::Mutex::new(calls),
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/v1/pages", get(list_pages))
        .route("/v1/pages/{title}", get(get_page))
        .route("/v1/pages/{title}/backlinks", get(backlinks))
        .route("/v1/pages/{title}/blocks", post(append_block))
        .route("/v1/blocks/{id}", get(get_block).patch(update_block))
        .route("/v1/journal/{date}", get(journal))
        .route("/v1/search", get(search))
        .route("/v1/query", post(query))
        .route("/v1/capture", post(capture))
        .with_state(ctx)
}

fn err(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({ "error": message }))).into_response()
}

/// Constant-time token comparison.
fn token_ok(given: &str, want: &str) -> bool {
    given.len() == want.len()
        && given
            .bytes()
            .zip(want.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn check(ctx: &Ctx, headers: &HeaderMap) -> Result<(), (u16, &'static str)> {
    if headers.contains_key("origin") {
        return Err((403, "browser requests are not accepted"));
    }
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let loopback = [
        format!("127.0.0.1:{}", ctx.port),
        format!("localhost:{}", ctx.port),
        format!("[::1]:{}", ctx.port),
    ];
    if !loopback.iter().any(|h| h == host) {
        return Err((403, "host must be loopback"));
    }
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !token_ok(auth, &ctx.token) {
        return Err((401, "missing or wrong bearer token"));
    }
    Ok(())
}

async fn call(ctx: Ctx, headers: HeaderMap, op: Op) -> Response {
    if let Err((status, msg)) = check(&ctx, &headers) {
        return err(status, msg);
    }
    let (reply, rx) = tokio::sync::oneshot::channel();
    if ctx.calls.send(ApiCall { op, reply }).is_err() {
        return err(503, "graph is closed");
    }
    match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(Ok(body))) => Json(body).into_response(),
        Ok(Ok(Err(e))) => err(e.status, &e.message),
        Ok(Err(_)) => err(503, "graph is closed"),
        Err(_) => err(504, "the app did not answer in time"),
    }
}

#[derive(Deserialize)]
struct PagesQ {
    #[serde(default)]
    daily: bool,
}

async fn list_pages(State(c): State<Ctx>, h: HeaderMap, Query(q): Query<PagesQ>) -> Response {
    call(c, h, Op::ListPages { daily: q.daily }).await
}

async fn get_page(State(c): State<Ctx>, h: HeaderMap, UrlPath(title): UrlPath<String>) -> Response {
    call(c, h, Op::GetPage { title }).await
}

async fn backlinks(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(title): UrlPath<String>,
) -> Response {
    call(c, h, Op::Backlinks { title }).await
}

#[derive(Deserialize)]
struct AppendBody {
    text: String,
    parent: Option<String>,
    #[serde(default)]
    create: bool,
}

async fn append_block(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(title): UrlPath<String>,
    Json(b): Json<AppendBody>,
) -> Response {
    call(
        c,
        h,
        Op::AppendBlock {
            title,
            text: b.text,
            parent: b.parent,
            create: b.create,
        },
    )
    .await
}

async fn get_block(State(c): State<Ctx>, h: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
    call(c, h, Op::GetBlock { id }).await
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

async fn update_block(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(b): Json<TextBody>,
) -> Response {
    call(c, h, Op::UpdateBlock { id, text: b.text }).await
}

async fn journal(State(c): State<Ctx>, h: HeaderMap, UrlPath(date): UrlPath<String>) -> Response {
    call(c, h, Op::Journal { date }).await
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    limit: Option<usize>,
}

async fn search(State(c): State<Ctx>, h: HeaderMap, Query(q): Query<SearchQ>) -> Response {
    let limit = q.limit.unwrap_or(20).min(500);
    call(c, h, Op::Search { q: q.q, limit }).await
}

#[derive(Deserialize)]
struct QueryBody {
    query: String,
    limit: Option<usize>,
}

async fn query(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<QueryBody>) -> Response {
    let limit = b.limit.unwrap_or(100).min(1000);
    call(
        c,
        h,
        Op::Query {
            query: b.query,
            limit,
        },
    )
    .await
}

async fn capture(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<TextBody>) -> Response {
    call(c, h, Op::Capture { text: b.text }).await
}
