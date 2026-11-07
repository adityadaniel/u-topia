//! Reference relay (§12.8) with an in-memory backend: per-page append-only logs,
//! dedupe by change id, monotonic `seq`, snapshot truncation, and a manifest whose `rev`
//! bumps on every write. No CRDT knowledge.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use utopia_core::PageId;

use crate::wire::{Manifest, ManifestPage, PullResponse, PushRequest, PushResponse, UpdateOut};
use crate::{CursorKind, CursorSpace, Result, SyncError, Transport, PROTOCOL_VERSION};

#[derive(Debug, Default)]
struct PageLog {
    next_seq: u64,
    log: Vec<UpdateOut>,
    snapshot: Option<(u64, Vec<u8>)>,
    updated_at: i64,
}

#[derive(Debug, Default)]
struct Graph {
    rev: u64,
    pages: BTreeMap<String, PageLog>,
    change_ids: HashSet<String>,
    blobs: BTreeMap<String, Vec<u8>>,
}

/// Shared relay state; clone the `Arc` to give several clients the same server.
#[derive(Debug, Default)]
pub struct Relay {
    graphs: Mutex<BTreeMap<String, Graph>>,
}

impl Relay {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn with_graph<R>(&self, gid: &str, f: impl FnOnce(&mut Graph) -> R) -> R {
        let mut graphs = self.graphs.lock().expect("relay lock poisoned");
        f(graphs.entry(gid.to_string()).or_default())
    }

    pub fn manifest(&self, gid: &str) -> Manifest {
        self.with_graph(gid, |g| Manifest {
            rev: g.rev,
            protocol_version: PROTOCOL_VERSION,
            pages: g
                .pages
                .iter()
                .map(|(id, p)| ManifestPage {
                    page_id: id.clone(),
                    head_seq: p.next_seq,
                    frontier: p.log.last().map(|u| u.frontier.clone()).unwrap_or_default(),
                    snapshot_seq: p.snapshot.as_ref().map_or(0, |(s, _)| *s),
                    tail_bytes: p.log.iter().map(|u| u.payload.len() as u64).sum(),
                    updated_at: p.updated_at,
                    deleted: false,
                })
                .collect(),
        })
    }

    pub fn push(&self, gid: &str, page: &str, req: PushRequest, now: i64) -> PushResponse {
        self.with_graph(gid, |g| {
            let mut accepted = 0;
            let mut duplicate = 0;
            for u in req.updates {
                if !g.change_ids.insert(u.id.clone()) {
                    duplicate += 1;
                    continue;
                }
                let p = g.pages.entry(page.to_string()).or_default();
                p.next_seq += 1;
                p.updated_at = now;
                p.log.push(UpdateOut {
                    seq: p.next_seq,
                    id: u.id,
                    frontier: u.frontier,
                    payload: u.payload,
                });
                accepted += 1;
            }
            if accepted > 0 {
                g.rev += 1;
            }
            PushResponse {
                accepted,
                head_seq: g.pages.get(page).map_or(0, |p| p.next_seq),
                duplicate,
            }
        })
    }

    pub fn pull(&self, gid: &str, page: &str, since: u64) -> PullResponse {
        self.with_graph(gid, |g| {
            let Some(p) = g.pages.get(page) else {
                return PullResponse {
                    snapshot: None,
                    updates: Vec::new(),
                    head_seq: 0,
                };
            };
            let snapshot = match &p.snapshot {
                Some((covers, bytes)) if since < *covers => Some(bytes.clone()),
                _ => None,
            };
            PullResponse {
                snapshot,
                updates: p.log.iter().filter(|u| u.seq > since).cloned().collect(),
                head_seq: p.next_seq,
            }
        })
    }

    /// Store a snapshot covering `covers_seq` and truncate the log up to it.
    pub fn put_snapshot(&self, gid: &str, page: &str, covers_seq: u64, payload: Vec<u8>) {
        self.with_graph(gid, |g| {
            let p = g.pages.entry(page.to_string()).or_default();
            if covers_seq > p.next_seq || p.snapshot.as_ref().is_some_and(|(s, _)| *s >= covers_seq)
            {
                return;
            }
            p.log.retain(|u| u.seq > covers_seq);
            p.snapshot = Some((covers_seq, payload));
            g.rev += 1;
        })
    }

    /// Total stored log entries for a page (tests and diagnostics).
    pub fn log_len(&self, gid: &str, page: &str) -> usize {
        self.with_graph(gid, |g| g.pages.get(page).map_or(0, |p| p.log.len()))
    }
}

/// A client's connection to an in-process relay. `offline` simulates a network failure.
pub struct RelayTransport {
    relay: Arc<Relay>,
    graph: String,
    pub offline: bool,
}

impl RelayTransport {
    pub fn new(relay: Arc<Relay>, graph: impl Into<String>) -> Self {
        Self {
            relay,
            graph: graph.into(),
            offline: false,
        }
    }

    fn check(&self) -> Result<()> {
        if self.offline {
            Err(SyncError::Transport("network unreachable".into()))
        } else {
            Ok(())
        }
    }
}

impl Transport for RelayTransport {
    fn cursor_space(&self) -> CursorSpace {
        // The in-process relay instance and graph: two separate relays with the same
        // graph name do not share a sequence.
        CursorSpace::new(
            format!(
                "memory:{:p}#{}",
                std::sync::Arc::as_ptr(&self.relay),
                self.graph
            ),
            CursorKind::Seq,
        )
    }

    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        self.check()?;
        let m = self.relay.manifest(&self.graph);
        Ok((Some(m.rev) != if_none_match).then_some(m))
    }

    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        self.check()?;
        Ok(self
            .relay
            .push(&self.graph, page.as_str(), req, utopia_core::now_ms()))
    }

    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
        self.check()?;
        Ok(self.relay.pull(&self.graph, page.as_str(), since_seq))
    }

    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()> {
        self.check()?;
        self.relay
            .put_snapshot(&self.graph, page.as_str(), covers_seq, payload);
        Ok(())
    }

    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        self.check()?;
        self.relay.with_graph(&self.graph, |g| {
            g.blobs.entry(hash.to_string()).or_insert(bytes);
        });
        Ok(true)
    }

    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        self.check()?;
        Ok(self
            .relay
            .with_graph(&self.graph, |g| g.blobs.get(hash).cloned()))
    }
}
