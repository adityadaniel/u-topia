# Daily Notes feed: loading older days

## Status

Implemented on `fix/journal-feed-pagination`. The machine is now an explicit `Feed`
enum in `crates/utopia-ui/src/page_view.rs`:

```rust
enum Feed {
    Idle { shown_through: NaiveDate },
    Loading { shown_through: NaiveDate, want_through: NaiveDate },
    Exhausted { shown_through: NaiveDate },
}
```

`Journal` holds `feed` (calendar days) and `pages` (the days that own a page, renamed
from `days`), so the two units are separate named fields instead of one `limit` counter.
`PageView::journal_shown_through` / `Workspace::journal_shown_through` expose the
calendar window for scripts; `journal_days` remains the page list. The file:line table
and the transition list below describe `main` before this change.

- **Issue 1, unbounded growth and no terminal state — fixed.** A rebuild asks
  `Store::oldest_daily` (oldest live daily page, drafts included, trashed days excluded)
  and `Feed::settle` returns `Exhausted` once that day is inside the loaded window.
  `Exhausted` never requests another batch and draws "No earlier notes." (debug selector
  `feed-end`) instead of the "Loading older days…" sentinel. The state is recomputed on
  every rebuild, so a page created or synced in later re-opens the feed instead of
  latching it shut. Tests: `fr_1_1_feed_stops_at_the_oldest_daily_page`
  (`crates/utopia-ui/tests/workspace.rs`), `fr_1_2_oldest_daily_page_bounds_the_feed`
  (`crates/utopia-store/tests/acceptance.rs`), and `feed_exhausts_at_the_oldest_daily_page`
  (unit). Deliberate behaviour change: scrolling no longer walks back through blank
  days older than the oldest daily page; use the calendar, a `[[date]]` link or Cmd+Shift+D
  to reach such a day. That is what `Exhausted` means (FR-1.2 still lists every calendar
  day back to the oldest note).
- **Issue 2, two units for "how much is loaded" — fixed by naming.** `Feed` dates count
  calendar days; `Journal::pages` is the page list, which is never longer than the
  window. Test: `feed_window_counts_calendar_days_not_pages`.
- **Issue 3, `loading` meaningless outside `rebuild_journal` — fixed by construction.**
  There is no boolean flag: a pending request is `Feed::Loading { want_through }`, so a
  rebuild that cannot draw (no page) keeps the request instead of stranding a separate
  flag, and `Feed::request` accepts one batch at a time. The old early-return path is
  guarded by a `debug_assert!` that a feed always has a page (`open_journal` → `show`).
  Test: `feed_requests_one_batch_at_a_time`. This combination was never live (the
  document says so), so there is no behaviour-level reproduction, only the unit test on
  the type.

No listed issue is left open. Verification shot of the terminal state:
`docs/verification/journal-feed-pagination/feed-end.png`.

## What it controls

How many calendar days the Daily Notes feed (FR-1.1) shows, when the next batch of older
days is requested, and when loading stops. Lives in `PageView` (`crates/utopia-ui/src/page_view.rs`).

## Where the state lives

| Piece | Location | Set | Cleared |
|---|---|---|---|
| Feed mode | `page_view.rs:337` `journal: Option<Journal>` | `open_journal :531-537` | `open :523` |
| Days requested | `page_view.rs:223` `Journal.limit: usize` (calendar days) | init `:532`; `+= JOURNAL_BATCH` at `:1276` | never shrinks |
| Batch in flight | `page_view.rs:225` `Journal.loading: bool` | `load_older_days :1274` | `rebuild_journal :1231` |
| Pages shown | `page_view.rs:224` `Journal.days: Vec<PageId>` | `:1230` | – |
| Sentinel row | `Item::JournalEnd` `:193`, pushed at `:1227` | every `rebuild_journal` | – |
| Trigger | `render_item(JournalEnd)` `:3563-3569` → `cx.defer(load_older_days)` | every frame the sentinel is laid out | – |

Read elsewhere: `journal_days`, `open_day_count`, `sticky_day` → `workspace.rs:1269-1281`;
tests at `tests/workspace.rs:599, 3151, 3183`.

## States

- **Not a feed** (`journal = None`).
- **Idle** (`loading = false`): sentinel rendered at the bottom.
- **Batch requested** (`loading = true`, `dirty = true`, `limit` already bumped).

There is **no terminal state**: nothing records "no older days exist".

## Events and transitions

- `open_journal` → Idle with `limit = JOURNAL_BATCH`.
- sentinel laid out (list renders a margin past the viewport) → `load_older_days`
  (`:1272-1281`): guard `!loading`; sets `loading`, bumps `limit`, dirties, notifies.
- `rebuild_journal` (`:1091-1234`): `days_between(from, today)` (`:1104`) where `from = today -
  (limit-1)`; loop at `:1114-1155` emits a `FeedDay` for **every** calendar day in the range
  (page or not); pushes `JournalEnd`; sets `loading = false`.

## Impossible or unhandled combinations (evidence)

1. **Unbounded growth.** Because `rebuild_journal` emits a row for every calendar day whether
   or not a page exists (`:1114-1155`), the sentinel is always `JOURNAL_BATCH` rows below the
   last request, and each frame in which it is within the list's render margin re-runs
   `load_older_days`. The only brake, `loading`, is cleared by the very rebuild the request
   caused (`:1231`). The result of `days_between` (`:1104`) is never inspected for "empty
   past this point". A user who scrolls to the bottom and keeps scrolling walks back through
   empty years, each batch costing a `days_between` query plus a per-day refs lookup
   (`:1138-1144`). `tests/workspace.rs:3182-3189` only scrolls until 26 pages appear, so the
   stop condition is untested because it does not exist.
2. **Two units for "how much is loaded".** `limit` counts calendar days; `days`/`journal_days()`
   (`:559`, used by `workspace.rs:1270`) counts pages. Callers reasoning about "N loaded"
   must know which.
3. **`loading` is meaningless outside `rebuild_journal`.** If `rebuild` takes the `page ==
   None` early return (`:919-922`) the flag stays `true`; only reachable in the transient
   state inside `open_journal` (`:538-539`) before `show` sets `page`. Not live.

## Diagram

```mermaid
stateDiagram-v2
    [*] --> NotFeed
    NotFeed --> Idle : open_journal (limit = BATCH)
    Idle --> Requested : JournalEnd laid out -> load_older_days (loading=true, limit += BATCH)
    Requested --> Idle : rebuild_journal (loading=false, sentinel re-pushed)
    Idle --> NotFeed : open(page)
    note right of Idle : no Exhausted state; the sentinel is always reachable
```

## Would an explicit state type help?

Yes: a small enum with an `Exhausted` arm, fed by a store query that reports the oldest
daily page (or by `days_between` returning nothing for the new window).

```rust
enum Feed {
    Idle { shown_through: NaiveDate },
    Loading { shown_through: NaiveDate, want_through: NaiveDate },
    Exhausted { shown_through: NaiveDate, oldest_page: Option<NaiveDate> },
}
// render JournalEnd only in Idle; in Exhausted render "No earlier notes" (§9.3 empty state)
```
