//! Sync (PRD §12): a deliberately dumb relay that stores opaque CRDT updates per page,
//! and a client loop that pushes the outbox and pulls what it has not seen.
//!
//! - [`wire`]: the v1 protocol shapes (§12.3), JSON with base64 payloads.
//! - [`Transport`]: what a client needs from a relay. HTTP, folder and LAN transports
//!   (§12.7) implement the same trait over the same update format.
//! - [`relay::Relay`]: the reference server semantics (§12.8) in memory: dedupe by
//!   change id, monotonic per-page `seq`, snapshot truncation, manifest with `rev`.
//! - [`SyncClient`]: the client loop (§12.3 steps 1–5).
//!
//! The relay never parses payloads, so it needs no CRDT dependency and works unchanged
//! once payloads are end-to-end encrypted (§11.7). Because the relay cannot interpret a
//! Loro frontier, pulls are addressed by the relay's own `seq`; frontiers are carried
//! opaquely for diagnostics.

pub mod e2e;
pub mod folder;
pub mod http;
pub mod realtime;
pub mod relay;

/// The v1 wire format, shared with the relay server.
pub use utopia_proto as wire;

use std::collections::BTreeMap;

use thiserror::Error;
use utopia_core::PageId;
use utopia_doc::Frontier;
use utopia_store::{Store, StoreError};

pub use wire::{
    Manifest, ManifestPage, PullResponse, PushRequest, PushResponse, UpdateIn, UpdateOut,
};

pub use utopia_proto::PROTOCOL_VERSION;
/// Max bytes per push batch (§12.3 step 2).
pub const MAX_PUSH_BYTES: usize = 1 << 20;
/// Upload a fresh snapshot when a page's server tail exceeds either bound (§12.3 step 5).
pub const SNAPSHOT_TAIL_BYTES: u64 = 256 * 1024;
/// Attachments moved per direction per round (memory stays bounded; the loop continues).
pub const BLOBS_PER_ROUND: usize = 8;
pub const SNAPSHOT_TAIL_UPDATES: u64 = 200;

#[derive(Debug, Error)]
pub enum SyncError {
    /// 401/403: halt sync and re-authenticate; local editing continues (§12.6).
    #[error("unauthorized")]
    Unauthorized,
    /// Network failure or 5xx: retry with backoff; the outbox keeps every payload.
    #[error("transport: {0}")]
    Transport(String),
    /// The relay speaks a protocol this client does not; never push (§12.6).
    #[error("unsupported protocol version {0}")]
    ProtocolVersion(u32),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        (**self).manifest(if_none_match)
    }
    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        (**self).push(page, req)
    }
    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
        (**self).pull(page, since_seq)
    }
    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()> {
        (**self).put_snapshot(page, covers_seq, payload)
    }
    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        (**self).put_blob(hash, bytes)
    }
    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        (**self).get_blob(hash)
    }
}

/// What a client needs from a relay (§12.3). One instance talks to one graph.
pub trait Transport {
    /// `GET manifest` with `If-None-Match: rev`; `None` means 304 Not Modified.
    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>>;
    /// `POST pages/{pid}/updates`.
    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse>;
    /// `GET pages/{pid}/updates?since={seq}`.
    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse>;
    /// `POST pages/{pid}/snapshot`: replaces the log up to `covers_seq`.
    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()>;
    /// `PUT blobs/{sha256}` (P2 attachments). `false`: this transport keeps no blobs.
    fn put_blob(&mut self, _hash: &str, _bytes: Vec<u8>) -> Result<bool> {
        Ok(false)
    }
    /// `GET blobs/{sha256}`; `None` when the relay does not have it (yet).
    fn get_blob(&mut self, _hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub pushed: usize,
    pub duplicates: usize,
    pub pulled_pages: usize,
    pub imported: Vec<PageId>,
    pub snapshots: usize,
    pub blobs_up: usize,
    pub blobs_down: usize,
}

impl SyncReport {
    fn absorb(&mut self, other: SyncReport) {
        self.pushed += other.pushed;
        self.duplicates += other.duplicates;
        self.pulled_pages += other.pulled_pages;
        self.snapshots += other.snapshots;
        self.blobs_up += other.blobs_up;
        self.blobs_down += other.blobs_down;
        for p in other.imported {
            if !self.imported.contains(&p) {
                self.imported.push(p);
            }
        }
    }

    /// Whether anything moved in either direction.
    pub fn is_quiet(&self) -> bool {
        self.pushed == 0 && self.imported.is_empty() && self.snapshots == 0
    }
}

/// Status shown in the sidebar (§9.2, §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Synced {
        at_ms: i64,
    },
    /// The relay was not reached, or answered with a transport failure: everything is
    /// still queued locally (§12.6).
    Offline {
        queued: usize,
    },
    /// The round failed on this device (the store, a shard, a projection). The network was
    /// never tried, so saying "offline" would blame the relay for a local fault.
    LocalError {
        message: String,
    },
    NeedsAuth,
    Unsupported,
}

impl SyncStatus {
    pub fn from_result(r: &Result<SyncReport>, store: &Store) -> Self {
        match r {
            Ok(_) => SyncStatus::Synced { at_ms: store.now() },
            Err(SyncError::Unauthorized) => SyncStatus::NeedsAuth,
            Err(SyncError::ProtocolVersion(_)) => SyncStatus::Unsupported,
            Err(SyncError::Store(e)) => SyncStatus::LocalError {
                message: e.to_string(),
            },
            Err(_) => SyncStatus::Offline {
                // A count, not the rows: this runs on the UI thread on every backoff round
                // with no network, and payloads can be large (rule 10).
                queued: store.outbox_count().unwrap_or(0),
            },
        }
    }
}

/// Everything a round needs from the store, gathered on the store's thread.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    /// `(page, outbox seqs, request)`, each request at most [`MAX_PUSH_BYTES`].
    pub pushes: Vec<(PageId, Vec<i64>, PushRequest)>,
    /// Highest relay `seq` already imported, per page.
    pub local_seqs: BTreeMap<PageId, u64>,
    pub rev: Option<u64>,
    pub snapshots: Vec<(PageId, u64, Vec<u8>)>,
    /// Attachments to upload: `(sha256, bytes)`.
    pub blob_puts: Vec<(String, Vec<u8>)>,
    /// Attachments referenced here but missing locally.
    pub blob_wants: Vec<String>,
}

/// What came back from the relay; applied on the store's thread.
#[derive(Debug, Clone, Default)]
pub struct Incoming {
    pub sent: Vec<i64>,
    pub accepted: usize,
    pub duplicates: usize,
    pub snapshots_sent: usize,
    pub manifest: Option<Manifest>,
    pub pulls: Vec<(PageId, PullResponse)>,
    /// Pages whose server tail is long enough to compact.
    pub compact: Vec<(PageId, u64)>,
    /// Uploaded attachment hashes.
    pub blobs_put: Vec<String>,
    /// Downloaded attachments, not yet verified.
    pub blobs_got: Vec<(String, Vec<u8>)>,
}

fn seq_key(page: &PageId) -> String {
    format!("seq:{page}")
}

/// The network half of a round (§12.3 steps 1–2, 5): touches only the transport, so it
/// can run on a background thread while the UI keeps editing.
pub fn exchange<T: Transport + ?Sized>(t: &mut T, out: Outgoing) -> Result<Incoming> {
    let mut inc = Incoming::default();
    let first = t.manifest(None)?;
    if let Some(m) = &first {
        if m.protocol_version != PROTOCOL_VERSION {
            return Err(SyncError::ProtocolVersion(m.protocol_version));
        }
    }
    for (page, seqs, req) in out.pushes {
        let resp = t.push(&page, req)?;
        // Duplicates are success: the relay already has them (§12.6, 409).
        inc.sent.extend(seqs);
        inc.accepted += resp.accepted;
        inc.duplicates += resp.duplicate;
    }
    for (page, covers, payload) in out.snapshots {
        t.put_snapshot(&page, covers, payload)?;
        inc.snapshots_sent += 1;
    }
    for (hash, bytes) in out.blob_puts {
        if t.put_blob(&hash, bytes)? {
            inc.blobs_put.push(hash);
        }
    }
    for hash in out.blob_wants {
        if let Some(bytes) = t.get_blob(&hash)? {
            inc.blobs_got.push((hash, bytes));
        }
    }
    let manifest = if inc.sent.is_empty() && inc.snapshots_sent == 0 {
        match first {
            Some(m) if Some(m.rev) != out.rev => Some(m),
            _ => None,
        }
    } else {
        t.manifest(None)?
    };
    if let Some(m) = &manifest {
        for p in &m.pages {
            let page = PageId(p.page_id.clone());
            let local = out.local_seqs.get(&page).copied().unwrap_or(0);
            if p.head_seq > local {
                let resp = t.pull(&page, local)?;
                inc.pulls.push((page.clone(), resp));
            }
            let tail = p.head_seq.saturating_sub(p.snapshot_seq);
            if p.tail_bytes > SNAPSHOT_TAIL_BYTES || tail > SNAPSHOT_TAIL_UPDATES {
                inc.compact.push((page, p.head_seq));
            }
        }
    }
    inc.manifest = manifest;
    Ok(inc)
}

/// The store half of the client (§12.3 steps 3–5), independent of any transport.
pub struct SyncEngine {
    client_id: String,
    pending_snapshots: Vec<(PageId, u64, Vec<u8>)>,
}

impl SyncEngine {
    pub fn new(store: &Store) -> Result<Self> {
        let client_id = match store.sync_get("client_id")? {
            Some(id) => id,
            None => {
                let id = ulid::Ulid::new().to_string();
                store.sync_set("client_id", &id)?;
                id
            }
        };
        Ok(Self {
            client_id,
            pending_snapshots: Vec::new(),
        })
    }

    /// This device's sync client id (stable per graph file).
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Gather pushes (outbox), local pull positions and queued snapshot uploads.
    pub fn prepare(&mut self, store: &mut Store) -> Result<Outgoing> {
        store.sync_index()?;
        let mut out = Outgoing {
            rev: store.sync_get("rev")?.and_then(|r| r.parse().ok()),
            snapshots: std::mem::take(&mut self.pending_snapshots),
            ..Default::default()
        };
        let mut by_page: BTreeMap<PageId, Vec<(i64, i64, Vec<u8>)>> = BTreeMap::new();
        for (seq, page, client_seq, payload) in store.outbox_pending(10_000)? {
            by_page
                .entry(page)
                .or_default()
                .push((seq, client_seq, payload));
        }
        for (page, rows) in by_page {
            let frontier = store.frontier(&page).unwrap_or_default();
            let mut seqs = Vec::new();
            let mut updates = Vec::new();
            let mut bytes = 0;
            for (seq, client_seq, payload) in rows {
                if bytes + payload.len() > MAX_PUSH_BYTES && !updates.is_empty() {
                    out.pushes.push((
                        page.clone(),
                        std::mem::take(&mut seqs),
                        PushRequest {
                            updates: std::mem::take(&mut updates),
                        },
                    ));
                    bytes = 0;
                }
                bytes += payload.len();
                seqs.push(seq);
                updates.push(UpdateIn {
                    id: format!("{}:{client_seq}", self.client_id),
                    client_seq,
                    frontier: frontier.0.clone(),
                    payload,
                });
            }
            if !updates.is_empty() {
                out.pushes.push((page, seqs, PushRequest { updates }));
            }
        }
        for (k, v) in store.sync_seqs()? {
            out.local_seqs.insert(PageId(k), v);
        }
        out.blob_puts = store.blobs_to_upload(BLOBS_PER_ROUND)?;
        out.blob_wants = store.missing_blobs()?;
        out.blob_wants.truncate(BLOBS_PER_ROUND);
        Ok(out)
    }

    /// Mark pushes sent, import pulled updates (idempotent), record positions, and queue
    /// compaction snapshots for pages this client is fully caught up on.
    pub fn apply(&mut self, store: &mut Store, inc: Incoming) -> Result<SyncReport> {
        store.outbox_mark_sent(&inc.sent)?;
        store.mark_blobs_uploaded(&inc.blobs_put)?;
        let mut blobs_in = 0;
        for (hash, bytes) in &inc.blobs_got {
            // A blob that does not match its hash is dropped, never stored (§15.6).
            match store.put_synced_blob(hash, bytes) {
                Ok(()) => blobs_in += 1,
                Err(StoreError::Invalid(e)) => eprintln!("utopia-sync: {e}"),
                Err(e) => return Err(e.into()),
            }
        }
        let mut report = SyncReport {
            pushed: inc.accepted,
            duplicates: inc.duplicates,
            snapshots: inc.snapshots_sent,
            pulled_pages: inc.pulls.len(),
            blobs_up: inc.blobs_put.len(),
            blobs_down: blobs_in,
            ..Default::default()
        };
        let mut batch: Vec<(PageId, Vec<u8>)> = Vec::new();
        let mut heads: Vec<(PageId, u64)> = Vec::new();
        for (page, resp) in inc.pulls {
            if let Some(snap) = resp.snapshot {
                batch.push((page.clone(), snap));
            }
            for u in resp.updates {
                batch.push((page.clone(), u.payload));
            }
            heads.push((page, resp.head_seq));
        }
        if !batch.is_empty() {
            report.imported = store.import_batch(&batch)?;
        }
        for (page, head) in &heads {
            store.sync_set(&seq_key(page), &head.to_string())?;
        }
        if let Some(m) = &inc.manifest {
            store.sync_set("rev", &m.rev.to_string())?;
        }
        store.sync_set("synced_at", &store.now().to_string())?;
        for (page, head) in inc.compact {
            let local: u64 = store
                .sync_get(&seq_key(&page))?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // Only a client that has imported everything up to head may cover it.
            if local >= head && store.page_exists(&page)? {
                let full = store.export_since(&page, &Frontier::default())?;
                self.pending_snapshots.push((page, head, full));
            }
        }
        Ok(report)
    }

    /// Prepare → exchange → apply until nothing moves (imports may create local ops,
    /// e.g. re-merges, which must be pushed in turn).
    pub fn sync_once<T: Transport + ?Sized>(
        &mut self,
        store: &mut Store,
        t: &mut T,
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        for _ in 0..6 {
            let out = self.prepare(store)?;
            let had_work = !out.pushes.is_empty() || !out.snapshots.is_empty();
            let inc = exchange(t, out)?;
            let pass = self.apply(store, inc)?;
            let quiet = pass.imported.is_empty()
                && !had_work
                && self.pending_snapshots.is_empty()
                && pass.blobs_up == 0
                && pass.blobs_down == 0;
            report.absorb(pass);
            if quiet {
                break;
            }
        }
        Ok(report)
    }
}

/// A transport plus an engine: the simple synchronous client used by tests and tools.
pub struct SyncClient<T: Transport> {
    transport: T,
    engine: SyncEngine,
}

impl<T: Transport> SyncClient<T> {
    pub fn new(transport: T, store: &Store) -> Result<Self> {
        Ok(Self {
            transport,
            engine: SyncEngine::new(store)?,
        })
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Number of local updates not yet accepted by the relay.
    pub fn queued(store: &Store) -> Result<usize> {
        Ok(store.outbox_pending(usize::MAX >> 1)?.len())
    }

    pub fn sync_once(&mut self, store: &mut Store) -> Result<SyncReport> {
        self.engine.sync_once(store, &mut self.transport)
    }

    /// Run a round and translate failures into a status for the UI (§9.3).
    pub fn sync_status(&mut self, store: &mut Store) -> SyncStatus {
        let r = self.sync_once(store);
        SyncStatus::from_result(&r, store)
    }
}
