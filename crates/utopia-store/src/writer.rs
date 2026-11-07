//! The SQLite writer (PRD §13.3): commits of shard deltas and projections run on a
//! dedicated thread with its own connection, so keystrokes never wait on disk. Jobs are
//! applied strictly in submission order; each one is a single transaction.
//!
//! In-memory graphs cannot share a database between connections, so they write inline
//! on the caller's thread with the same code path.

use std::collections::HashSet;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use rusqlite::{params, Connection, TransactionBehavior};
use utopia_core::{DateFormat, DocDelta, PageId, PageState};

use crate::project::{Projector, ResolutionWork};
use crate::{schema, Result, StoreError};

/// One page's share of a commit.
pub(crate) struct PageJob {
    pub page: PageId,
    pub state: PageState,
    pub delta: Option<DocDelta>,
    /// Full snapshot (and its frontier) replacing `page_doc` and the update log.
    pub snapshot: Option<(Vec<u8>, String)>,
    /// Incremental update appended to `page_update`.
    pub update: Option<Vec<u8>>,
    /// Payload queued for sync.
    pub outbox: Option<Vec<u8>>,
}

pub(crate) struct Job {
    pub pages: Vec<PageJob>,
    pub fmt: DateFormat,
    pub now: i64,
}

/// Write one job in one transaction (§11.2: shard, outbox and projections together).
///
/// Returns the destination pages whose reference edges this job added or removed. Only
/// those, plus the pages the job wrote, can change tag-page classification; a destination
/// whose edges are untouched (a widely used `#todo` on an edited page) needs no rescan
/// (SB-2).
pub(crate) fn write_job(conn: &mut Connection, job: &Job) -> Result<HashSet<PageId>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut work = ResolutionWork::default();
    {
        let projector = Projector {
            conn: &tx,
            fmt: &job.fmt,
            now: job.now,
        };
        // Pages that become tombstoned project first so unique title/date indexes never
        // see two live rows mid-transaction (§8.6, import-time dedupe).
        let mut order: Vec<&PageJob> = job.pages.iter().collect();
        order.sort_by_key(|p| p.state.meta.deleted_at.is_none());
        let mut flips = Vec::with_capacity(order.len());
        for p in &order {
            flips.push(projector.project_page(&p.state, &mut work)?);
        }
        // Block rows of live pages first: blocks merged into a survivor are re-homed in
        // place before the tombstoned page cleans up what is left (§8.6).
        let mut blocks: Vec<(&PageJob, bool)> = order.iter().copied().zip(flips).collect();
        blocks.sort_by_key(|(p, _)| p.state.meta.deleted_at.is_some());
        for (p, flip) in &blocks {
            projector.project_blocks(&p.state, p.delta.as_ref(), *flip, &mut work)?;
        }
        for p in order {
            if let Some((snap, frontier)) = &p.snapshot {
                tx.execute(
                    "INSERT INTO page_doc(page_id, snapshot, frontier, bytes) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(page_id) DO UPDATE SET snapshot=excluded.snapshot,
                       frontier=excluded.frontier, bytes=excluded.bytes",
                    params![p.page.as_str(), snap, frontier, snap.len() as i64],
                )?;
                tx.execute(
                    "DELETE FROM page_update WHERE page_id=?1",
                    [p.page.as_str()],
                )?;
            }
            if let Some(update) = &p.update {
                tx.execute(
                    "INSERT INTO page_update(page_id, payload) VALUES (?1, ?2)",
                    params![p.page.as_str(), update],
                )?;
            }
            if let Some(payload) = &p.outbox {
                // max(seq) reads the rowid b-tree's last entry: O(1).
                let client_seq: i64 =
                    tx.query_row("SELECT ifnull(max(seq), 0) + 1 FROM outbox", [], |r| {
                        r.get(0)
                    })?;
                tx.execute(
                    "INSERT INTO outbox(page_id, payload, client_seq, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![p.page.as_str(), payload, client_seq, job.now],
                )?;
            }
        }
        projector.reresolve(&mut work)?;
    }
    tx.commit()?;
    Ok(work.changed)
}

#[derive(Default)]
struct Progress {
    completed: u64,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct Shared {
    progress: Mutex<Progress>,
    cv: Condvar,
    /// The tag-page classification as of the last committed job (SB-2). Published here,
    /// off the UI thread, so an index update never runs the whole-graph query.
    tag_pages: Mutex<Option<HashSet<PageId>>>,
    /// How many whole-graph classifications this writer has run. Only the first commit
    /// needs one; later commits update the pages they can affect (SB-2).
    tag_scans: std::sync::atomic::AtomicU64,
    /// How many per-page classifications the incremental path ran (tests, SB-2).
    tag_classifications: std::sync::atomic::AtomicU64,
    /// How many pages the incremental tag publisher looked at — classified or probed
    /// (tests, SB-2). A commit must never walk the published set, so this stays as small as
    /// the pages it wrote plus the destinations it changed.
    tag_pages_visited: std::sync::atomic::AtomicU64,
}

impl Shared {
    /// Update the published tag-page classification after a commit (SB-2). Only the pages
    /// the commit wrote and the destinations whose reference edges it changed can flip:
    /// losing the last reference to a page removes that edge, so it is one of
    /// `changed_refs`. Everything else keeps its entry, so no commit walks the whole graph.
    /// The first commit (before there is a set) classifies once. A failure leaves the
    /// previous set in place.
    fn publish_tag_pages(&self, conn: &Connection, job: &Job, changed_refs: &HashSet<PageId>) {
        let mut slot = match self.tag_pages.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        let Some(mut set) = slot.clone() else {
            self.tag_scans
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match crate::query::hashtag_only_pages(conn) {
                Ok(pages) => *slot = Some(pages),
                Err(e) => eprintln!("utopia: tag pages: {e}"),
            }
            return;
        };
        let written: Vec<PageId> = job.pages.iter().map(|p| p.page.clone()).collect();
        let mut affected: HashSet<PageId> = written.iter().cloned().collect();
        // Only destinations whose reference edges changed can flip classification. The
        // pages a job wrote are included above; unchanged refs on them are not rescanned.
        affected.extend(changed_refs.iter().cloned());
        for page in affected {
            self.tag_pages_visited
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.tag_classifications
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match crate::query::classify_tag_page(conn, &page) {
                Ok(Some(true)) => {
                    set.insert(page);
                }
                Ok(_) => {
                    set.remove(&page);
                }
                Err(e) => {
                    eprintln!("utopia: tag pages: {e}");
                    return;
                }
            }
        }
        *slot = Some(set);
    }
}

pub(crate) enum Msg {
    Job(u64, Job),
    Stop,
}

/// Lets a caller (e.g. a UI background task) wait until everything submitted so far is
/// durable, without holding the store.
#[derive(Clone)]
pub struct WriteTicket {
    shared: Option<Arc<Shared>>,
    target: u64,
}

impl WriteTicket {
    pub fn is_done(&self) -> bool {
        self.shared.as_ref().is_none_or(|s| {
            s.progress
                .lock()
                .map_or(true, |p| p.completed >= self.target)
        })
    }

    /// Block until the writer has committed every job up to this ticket.
    pub fn wait(&self) {
        let Some(s) = &self.shared else { return };
        let mut p = match s.progress.lock() {
            Ok(p) => p,
            Err(_) => return,
        };
        while p.completed < self.target && p.error.is_none() {
            p = match s.cv.wait(p) {
                Ok(p) => p,
                Err(_) => return,
            };
        }
    }
}

pub(crate) enum Persister {
    Inline {
        shared: Arc<Shared>,
    },
    Thread {
        tx: Sender<Msg>,
        shared: Arc<Shared>,
        handle: Option<JoinHandle<()>>,
        submitted: u64,
    },
}

impl Persister {
    /// Writes on the caller's thread (in-memory graphs, tests, the CLI).
    pub fn inline() -> Self {
        Persister::Inline {
            shared: Arc::new(Shared::default()),
        }
    }

    /// Spawn a writer thread with its own connection to `path`.
    pub fn spawn(path: &std::path::Path, key: Option<&str>) -> Result<Self> {
        let mut conn = crate::crypt::open_keyed(path, key)?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        let (tx, rx) = channel::<Msg>();
        let shared = Arc::new(Shared::default());
        let s2 = shared.clone();
        let handle = std::thread::Builder::new()
            .name("utopia-sqlite-writer".into())
            .spawn(move || {
                for msg in rx {
                    match msg {
                        Msg::Job(seq, job) => {
                            let r = write_job(&mut conn, &job);
                            if let Ok(referenced_before) = &r {
                                s2.publish_tag_pages(&conn, &job, referenced_before);
                            }
                            if let Ok(mut p) = s2.progress.lock() {
                                p.completed = seq;
                                if let Err(e) = r {
                                    eprintln!("utopia: commit failed: {e}");
                                    p.error.get_or_insert(e.to_string());
                                }
                            }
                            s2.cv.notify_all();
                        }
                        Msg::Stop => break,
                    }
                }
            })?;
        Ok(Persister::Thread {
            tx,
            shared,
            handle: Some(handle),
            submitted: 0,
        })
    }

    pub fn submit(&mut self, conn: &mut Connection, job: Job) -> Result<()> {
        match self {
            Persister::Inline { shared } => {
                let referenced_before = write_job(conn, &job)?;
                shared.publish_tag_pages(conn, &job, &referenced_before);
                Ok(())
            }
            Persister::Thread {
                tx,
                submitted,
                shared,
                ..
            } => {
                *submitted += 1;
                if tx.send(Msg::Job(*submitted, job)).is_err() {
                    // The writer is gone; make waiters return instead of hanging.
                    if let Ok(mut p) = shared.progress.lock() {
                        p.error.get_or_insert("writer thread stopped".into());
                    }
                    shared.cv.notify_all();
                    return Err(StoreError::Writer("writer thread stopped".into()));
                }
                Ok(())
            }
        }
    }

    pub fn ticket(&self) -> WriteTicket {
        match self {
            Persister::Inline { .. } => WriteTicket {
                shared: None,
                target: 0,
            },
            Persister::Thread {
                shared, submitted, ..
            } => WriteTicket {
                shared: Some(shared.clone()),
                target: *submitted,
            },
        }
    }

    /// The tag-page classification published by the last commit, if any (SB-2).
    pub fn tag_pages(&self) -> Option<HashSet<PageId>> {
        let shared = match self {
            Persister::Inline { shared } => shared,
            Persister::Thread { shared, .. } => shared,
        };
        shared.tag_pages.lock().ok().and_then(|slot| slot.clone())
    }

    /// How many per-page tag classifications the incremental path has run (tests, SB-2).
    pub fn tag_classifications(&self) -> u64 {
        let shared = match self {
            Persister::Inline { shared } => shared,
            Persister::Thread { shared, .. } => shared,
        };
        shared
            .tag_classifications
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many whole-graph tag classifications this writer has run (tests, SB-2).
    /// How many pages the incremental tag publisher looked at (tests, SB-2): the pages a
    /// commit wrote plus the destinations whose ref edges it changed, never the whole
    /// published set.
    pub fn tag_pages_visited(&self) -> u64 {
        let shared = match self {
            Persister::Inline { shared } => shared,
            Persister::Thread { shared, .. } => shared,
        };
        shared
            .tag_pages_visited
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn tag_scans(&self) -> u64 {
        let shared = match self {
            Persister::Inline { shared } => shared,
            Persister::Thread { shared, .. } => shared,
        };
        shared.tag_scans.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wait for every submitted job; report (and clear) the first failure.
    pub fn wait(&self) -> Result<()> {
        let Persister::Thread { shared, .. } = self else {
            return Ok(());
        };
        self.ticket().wait();
        let mut p = shared
            .progress
            .lock()
            .map_err(|_| StoreError::Writer("writer lock poisoned".into()))?;
        match p.error.take() {
            Some(e) => Err(StoreError::Writer(e)),
            None => Ok(()),
        }
    }

    pub fn stop(&mut self) {
        if let Persister::Thread { tx, handle, .. } = self {
            let _ = tx.send(Msg::Stop);
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}
