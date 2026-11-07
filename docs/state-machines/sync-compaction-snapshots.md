# Sync compaction: pending snapshot queue and the two definitions of "quiet"

## What it controls

When this client uploads a full page snapshot so the relay can truncate its log (PRD
`§12.3` step 5), and whether a round counts as "busy" for the poll interval.
`crates/utopia-sync/src/lib.rs`.

## Where the state lives

| Piece | Location | Set | Cleared |
|---|---|---|---|
| Queue | `sync/lib.rs:253` `SyncEngine.pending_snapshots: Vec<(PageId, u64, Vec<u8>)>` (memory only) | `apply :374-384` when `local >= head && page_exists` | `prepare :282` `std::mem::take` |
| Relay's wish list | `Incoming.compact` (`:182`) | `exchange :240-243` from manifest thresholds (`SNAPSHOT_TAIL_BYTES :40`, `SNAPSHOT_TAIL_UPDATES :43`) | per round |
| Sent count | `Incoming.snapshots_sent` (`:178`) | `exchange :210-213` | – |
| Quiet (desktop) | `SyncReport::is_quiet` (`:130-132`): `pushed == 0 && imported.is_empty() && snapshots == 0` | – | – |
| Quiet (`sync_once`) | `:401-405`: also `!had_work && pending_snapshots.is_empty() && blobs == 0` | – | – |

## States

**Nothing queued** → **Queued (memory)** → **Sent** (`put_snapshot` returned) or
**Lost** (exchange failed after `prepare` drained the queue).

## Events and transitions

- manifest says tail is long → `compact` → `apply` exports the full doc (`:381`) and queues.
- next `prepare` moves the queue into `out.snapshots`; `exchange` uploads after pushes.
- relay ignores a snapshot whose `covers_seq <= existing` silently (`syncd/lib.rs:366-368`,
  `relay.rs:121-124`); `covers_seq > head` → 400 (`syncd:354-358`).

## Impossible or unhandled combinations (evidence)

1. **Lost on exchange failure.** `prepare` drains the queue (`:282`); if `exchange` errors
   the `Outgoing` is dropped (`workspace.rs:548`) and the snapshot is gone until the manifest
   triggers compaction again. Self-healing, but each attempt re-exports the full page.
2. **Desktop waits up to a minute to send it.** The desktop runner runs one
   prepare/exchange/apply per tick (`workspace.rs:511-540`), never `sync_once`. After an
   `apply` that only queued a snapshot, `is_quiet()` is true (`:130-132` ignores
   `pending_snapshots`) → `next_interval` doubles toward 60 s (`sync_runner.rs:150-155`).
   `sync_once` (FFI, tests) counts the queue (`:403`). Two definitions of quiet for one engine.
3. **Stale snapshot counted as work.** The relay's silent ignore still yields
   `snapshots_sent += 1` (`:212`) → `is_quiet` false → one extra FAST round. Cosmetic.
4. **Folder transport never compacts** (`folder.rs:98-99` reports `snapshot_seq = n`,
   `tail_bytes = 0`; `put_snapshot` is a no-op `:156-158`). Intentional (`folder.rs:6-7`).
5. **Blobs are ignored by the desktop's quiet test** (`:131`), so with more than 8 pending
   blobs per direction the runner backs off to 60 s between batches of 8 (`:42, :324-326`).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Empty
    Empty --> Queued : apply sees compact want && local >= head (full export)
    Queued --> InFlight : prepare (mem::take)
    InFlight --> Empty : put_snapshot ok (snapshots_sent++, round not quiet)
    InFlight --> Lost : exchange Err (Outgoing dropped)
    Lost --> Queued : manifest asks again next round
    Queued --> Queued : desktop: round quiet by is_quiet() -> interval doubles
```

## Would an explicit state type help?

Modestly: make the queue durable-ish and the quiet test single-sourced.

```rust
impl SyncEngine {
    pub fn has_backlog(&self) -> bool { !self.pending_snapshots.is_empty() }
}
impl SyncReport { pub fn is_quiet(&self, engine: &SyncEngine) -> bool { .. && !engine.has_backlog() && self.blobs_up + self.blobs_down == 0 } }
```

and in `prepare`, clone rather than take, clearing only in `apply` on success.
