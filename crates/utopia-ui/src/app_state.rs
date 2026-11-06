//! Shared application state: the store plus a change feed views observe.
//!
//! The store runs in debounced mode (§13.3): edits apply to the in-memory shards at once,
//! commits reach the writer thread after 250 ms idle or 1 s at most (immediately for
//! structural edits), and [`IndexUpdated`] fires once the writer has made them durable so
//! reference panels can re-query.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{Context, EventEmitter, Subscription, Task};
use utopia_core::PageId;
use utopia_store::Store;

const IDLE_FLUSH: Duration = Duration::from_millis(250);
const MAX_FLUSH_DELAY: Duration = Duration::from_secs(1);

/// Emitted after every mutation so views refresh only what they show.
#[derive(Debug, Clone)]
pub struct Changed {
    pub pages: Vec<PageId>,
    /// Tree shape, page identity or references changed (not just a keystroke).
    pub structural: bool,
}

/// The index (backlinks, page list, search) now reflects every flushed change.
#[derive(Debug, Clone)]
pub struct IndexUpdated;

pub struct AppState {
    pub store: Store,
    /// Bumped on every change.
    pub version: u64,
    /// Bumped when the index caught up; reference panels re-query on change.
    pub refs_version: u64,
    pub last_error: Option<String>,
    /// A transient message (e.g. from a plugin), shown in the sidebar footer.
    pub notice: Option<String>,
    /// Plugins from `<config>/plugins` (P2), offered in the slash menu.
    pub plugins: Vec<std::rc::Rc<utopia_plugin::Plugin>>,
    /// Pages that exist only because of a `#tag` (SB-2), cached from the index. Views read
    /// this; the classification query runs at startup and on [`IndexUpdated`], never in a
    /// render (rule 10).
    pub tag_pages: HashSet<PageId>,
    dirty_since: Option<Instant>,
    flush_task: Option<Task<()>>,
    wait_task: Option<Task<()>>,
    _quit: Option<Subscription>,
}

impl EventEmitter<Changed> for AppState {}
impl EventEmitter<IndexUpdated> for AppState {}

impl AppState {
    pub fn new(mut store: Store) -> Self {
        if let Err(e) = store.set_debounced(true) {
            eprintln!("utopia: {e}");
        }
        // Classify once at startup; each commit publishes the new set with the job (SB-2),
        // so neither the sidebar's render nor an index update runs the query (rule 10).
        let tag_pages = store.hashtag_only_pages().unwrap_or_default();
        Self {
            store,
            version: 0,
            refs_version: 0,
            last_error: None,
            notice: None,
            plugins: Vec::new(),
            tag_pages,
            dirty_since: None,
            flush_task: None,
            wait_task: None,
            _quit: None,
        }
    }

    /// Make every pending edit durable before the app exits (no typing is lost).
    pub fn install_quit_hook(&mut self, cx: &mut Context<Self>) {
        self._quit = Some(cx.on_app_quit(|this, _| {
            let r = this.store.sync_index();
            this.report(r);
            let r = this.store.compact();
            this.report(r);
            async {}
        }));
    }

    pub fn changed(&mut self, pages: Vec<PageId>, structural: bool, cx: &mut Context<Self>) {
        self.version += 1;
        cx.emit(Changed { pages, structural });
        if structural {
            self.flush_now(cx);
        } else {
            self.schedule_flush(cx);
        }
        cx.notify();
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if !self.store.has_pending() {
            return;
        }
        let now = Instant::now();
        let first = *self.dirty_since.get_or_insert(now);
        if now.duration_since(first) >= MAX_FLUSH_DELAY {
            return self.flush_now(cx);
        }
        self.flush_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(IDLE_FLUSH).await;
            let _ = this.update(cx, |app, cx| app.flush_now(cx));
        }));
    }

    /// Hand pending edits to the writer and announce when they are durable.
    pub fn flush_now(&mut self, cx: &mut Context<Self>) {
        self.flush_task = None;
        self.dirty_since = None;
        let r = self.store.flush();
        self.report(r);
        let ticket = self.store.write_ticket();
        if ticket.is_done() {
            self.index_updated(cx);
            return;
        }
        self.wait_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { ticket.wait() })
                .await;
            let _ = this.update(cx, |app, cx| app.index_updated(cx));
        }));
    }

    fn index_updated(&mut self, cx: &mut Context<Self>) {
        self.refs_version += 1;
        // The commit that just landed published the tag-page classification with it (SB-2):
        // take that copy. Re-classifying here would put a whole-graph query on the UI thread
        // after every keystroke flush (§15.1, rule 10).
        if let Some(pages) = self.store.tag_pages() {
            self.tag_pages = pages;
        }
        cx.emit(IndexUpdated);
        cx.notify();
    }

    /// Record a store error for the status line instead of panicking (AGENTS.md rule 11).
    /// Load every plugin under `dir`; broken ones are reported, not fatal.
    pub fn load_plugins(&mut self, dir: &std::path::Path) {
        self.plugins.clear();
        for (path, r) in utopia_plugin::load_all(dir) {
            match r {
                Ok(p) => self.plugins.push(std::rc::Rc::new(p)),
                Err(e) => {
                    eprintln!("utopia: plugin {}: {e}", path.display());
                    self.last_error = Some(format!("plugin {}: {e}", path.display()));
                }
            }
        }
    }

    pub fn report<T>(&mut self, r: Result<T, utopia_store::StoreError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(utopia_store::StoreError::Domain(_)) => None,
            Err(e) => {
                eprintln!("utopia: {e}");
                self.last_error = Some(e.to_string());
                None
            }
        }
    }
}
