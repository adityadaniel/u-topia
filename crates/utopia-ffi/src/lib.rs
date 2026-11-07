//! The mobile core (PRD §14, Option A): the same store, CRDT and sync engine as the
//! desktop, behind a small UniFFI surface. The surface covers the minimal mobile scope
//! (§14.3): today's page, bullet capture, references (with title search for `[[`),
//! backlinks, full offline editing and sync. Nothing here knows about any UI toolkit.
//!
//! Swift bindings: `cargo run -p utopia-ffi --bin uniffi-bindgen -- generate
//! --library <libutopia_ffi> --language swift --out-dir <dir>` (see `xtask mobile`).

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use utopia_core::{outline, BlockId, PageId};
use utopia_store::{RefQuery, Store, StoreError};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("this graph is encrypted; the passphrase is missing or wrong")]
    Locked,
    #[error("not found: {msg}")]
    NotFound { msg: String },
    #[error("{msg}")]
    Invalid { msg: String },
    #[error("sync: {msg}")]
    Sync { msg: String },
    #[error("{msg}")]
    Internal { msg: String },
}

impl From<StoreError> for FfiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Encrypted => FfiError::Locked,
            StoreError::NotFound(msg) => FfiError::NotFound { msg },
            StoreError::Invalid(msg) => FfiError::Invalid { msg },
            StoreError::Domain(d) => FfiError::Invalid { msg: d.to_string() },
            other => FfiError::Internal {
                msg: other.to_string(),
            },
        }
    }
}

type Result<T, E = FfiError> = std::result::Result<T, E>;

/// One visible block of a page, in outline order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BlockView {
    pub id: String,
    pub text: String,
    /// Plain text with markup removed (for list rows).
    pub plain: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PageView {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD` for journal pages.
    pub date: Option<String>,
    pub blocks: Vec<BlockView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RefView {
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SyncSummary {
    pub pushed: u32,
    pub pulled_pages: u32,
    pub imported_pages: u32,
}

/// An open graph. All calls are serialized on one store.
#[derive(uniffi::Object)]
pub struct Graph {
    store: Mutex<Store>,
}

impl Graph {
    fn with<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut s = self.store.lock().map_err(|_| FfiError::Internal {
            msg: "graph lock poisoned".into(),
        })?;
        f(&mut s)
    }
}

fn page_view(store: &mut Store, page: &PageId) -> Result<PageView> {
    let fmt = store.date_format().clone();
    let st = store.page_state(page)?.clone();
    let blocks = st
        .live_preorder()
        .into_iter()
        .filter_map(|(id, depth)| {
            let b = st.block(&id)?;
            Some(BlockView {
                id: id.as_str().to_string(),
                plain: utopia_core::inline::plain_text(&b.text),
                text: b.text.clone(),
                depth: depth as u32,
            })
        })
        .collect();
    Ok(PageView {
        id: page.as_str().to_string(),
        title: utopia_store::display_title(&st, &fmt),
        date: st.meta.daily_date.map(|d| d.to_string()),
        blocks,
    })
}

#[uniffi::export]
impl Graph {
    /// Open (or create) the graph file. `passphrase` unlocks a graph encrypted at rest.
    #[uniffi::constructor]
    pub fn open(path: String, passphrase: Option<String>) -> Result<Arc<Self>> {
        let store = Store::open_with_key(&path, passphrase.as_deref())?;
        Ok(Arc::new(Self {
            store: Mutex::new(store),
        }))
    }

    /// An in-memory graph (previews and tests).
    #[uniffi::constructor]
    pub fn in_memory() -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            store: Mutex::new(Store::open_in_memory()?),
        }))
    }

    /// Pin "today" (tests, screenshots); `None` follows the clock.
    pub fn set_today(&self, date: Option<String>) -> Result<()> {
        let d = match date {
            Some(s) => Some(
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| FfiError::Invalid { msg: e.to_string() })?,
            ),
            None => None,
        };
        self.with(|s| {
            s.set_today(d);
            Ok(())
        })
    }

    /// Today's journal page, created (with one empty block) on first open (FR-1.1).
    pub fn today(&self) -> Result<PageView> {
        self.with(|s| {
            let (page, _) = s.open_daily(s.today())?;
            page_view(s, &page)
        })
    }

    pub fn page(&self, page_id: String) -> Result<PageView> {
        self.with(|s| page_view(s, &PageId(page_id)))
    }

    /// The page a title names (dates and aliases resolve like links), if any.
    pub fn find_page(&self, title: String) -> Result<Option<PageView>> {
        self.with(|s| {
            let key = utopia_core::title::normalize_title(&title);
            match utopia_store::resolve_page_key(s.conn(), &key, s.date_format())? {
                Some(p) => page_view(s, &p).map(Some),
                None => Ok(None),
            }
        })
    }

    /// Append a top-level block (fills a lone empty block instead). Links in the text
    /// create their pages and references on commit.
    pub fn append_block(&self, page_id: String, text: String) -> Result<BlockView> {
        self.with(|s| {
            let page = PageId(page_id);
            let st = s.page_state(&page)?.clone();
            let roots = st.live_roots();
            let reuse = match roots.as_slice() {
                [only] => {
                    st.block(only).is_some_and(|b| b.text.is_empty()) && !st.has_live_children(only)
                }
                _ => false,
            };
            s.seal_undo();
            let block = if reuse {
                roots[0].clone()
            } else {
                let plan = outline::append_block(&st, None);
                s.execute_plan(&page, plan, None)?
                    .ok_or_else(|| FfiError::Internal {
                        msg: "insert produced no block".into(),
                    })?
                    .block
            };
            s.set_block_text(&page, &block, &text, None, None)?;
            s.commit_block(&page, &block)?;
            s.flush()?;
            let depth = 0;
            Ok(BlockView {
                id: block.as_str().to_string(),
                plain: utopia_core::inline::plain_text(&text),
                text,
                depth,
            })
        })
    }

    pub fn set_block_text(&self, block_id: String, text: String) -> Result<()> {
        self.with(|s| {
            let block = BlockId(block_id);
            let row = s.block_row(&block)?.ok_or_else(|| FfiError::NotFound {
                msg: format!("block {block}"),
            })?;
            s.seal_undo();
            s.set_block_text(&row.page_id, &block, &text, None, None)?;
            s.commit_block(&row.page_id, &block)?;
            s.flush()?;
            Ok(())
        })
    }

    /// Linked References of a page, newest source pages first.
    pub fn backlinks(&self, page_id: String) -> Result<Vec<RefView>> {
        self.with(|s| {
            let page = PageId(page_id);
            let refs = s.linked_references(
                &page,
                &RefQuery {
                    sort: utopia_store::RefSort::Recency,
                    ..Default::default()
                },
            )?;
            Ok(refs
                .groups
                .into_iter()
                .flat_map(|g| {
                    let title = g.page.title.clone();
                    let pid = g.page.id.as_str().to_string();
                    g.hits.into_iter().map(move |h| RefView {
                        page_id: pid.clone(),
                        page_title: title.clone(),
                        block_id: h.block.as_str().to_string(),
                        text: h.text,
                    })
                })
                .collect())
        })
    }

    /// Page titles for `[[` autocomplete.
    pub fn search_titles(&self, query: String, limit: u32) -> Result<Vec<String>> {
        self.with(|s| {
            Ok(s.search_titles(&query, limit as usize)?
                .into_iter()
                .map(|h| h.title)
                .collect())
        })
    }

    /// One sync round with a relay (§12). `e2e_passphrase` turns on end-to-end
    /// encryption; the same passphrase must be used on every device.
    pub fn sync(
        &self,
        relay_url: String,
        graph_id: String,
        token: Option<String>,
        e2e_passphrase: Option<String>,
    ) -> Result<SyncSummary> {
        use utopia_sync::{http::HttpTransport, SyncClient, Transport};
        let err = |e: utopia_sync::SyncError| FfiError::Sync { msg: e.to_string() };
        let http = HttpTransport::new(&relay_url, &graph_id, token).map_err(err)?;
        let transport: Box<dyn Transport> = match e2e_passphrase {
            Some(p) => {
                let key = utopia_sync::e2e::GraphKey::derive(&p, &graph_id).map_err(err)?;
                Box::new(utopia_sync::e2e::Encrypted::new(
                    http,
                    key,
                    graph_id.clone(),
                ))
            }
            None => Box::new(http),
        };
        self.with(|s| {
            let mut client = SyncClient::new(transport, s).map_err(err)?;
            let r = client.sync_once(s).map_err(err)?;
            // A round can apply most pages and still leave one failed (a page this device
            // cannot read, a store error): the shell has one summary and no per-page state,
            // so it keeps the old "a round with a failure is an error" contract (§12.6).
            if let Some(f) = r.failed.first() {
                return Err(err(f.to_error()));
            }
            Ok(SyncSummary {
                pushed: r.pushed as u32,
                pulled_pages: r.pulled_pages as u32,
                imported_pages: r.imported.len() as u32,
            })
        })
    }
}
