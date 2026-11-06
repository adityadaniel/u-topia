# Left sidebar redesign — verification (SB-1 … SB-5)

Branch `redesign/01-left-sidebar`, based on `redesign/00-foundations`. Everything below
was captured from the real app (`UTOPIA_SHOT_SCRIPT`) against a scratch graph under
`target/redesign-01/`, seeded by driving the `Store` (pages, tag-only pages, six days of
notes, two pins). No real config or notes were touched.

`UTOPIA_CONFIG_DIR=target/redesign-01/config`,
`--graph target/redesign-01/demo.utopia`. The encrypted shots used
`target/redesign-01/enc.utopia` with `UTOPIA_DB_PASSPHRASE=secret`.

Reproduce the graph with the throwaway seeder (it drives the `Store`, so it is
deterministic and cannot race the UI):

```bash
cargo run -p utopia-store --example seed_sidebar -- \
  target/redesign-01/demo.utopia target/redesign-01/config
```

For the compact shot, copy that `settings.json` with `"density": "compact"` into
`target/redesign-01/config-compact`. For the lock, seed `target/redesign-01/enc.utopia`
the same way and run
`UTOPIA_DB_NEW_PASSPHRASE=secret cargo run -p utopia-app -- encrypt target/redesign-01/enc.utopia`.

## Before

| Shot | What it shows |
|---|---|
| `before-left-sidebar.png` | The pre-redesign sidebar (from `docs/images/daily-notes.png`): two preview lines per day so three days take half the list, `atlas` and `beta` indistinguishable from real pages, no pinned group, a two-line footer ("Local · …", "Sync: off"), and "Calendar" as a disclosure row. |

## After

| Shot | What it proves |
|---|---|
| `after-sidebar.png` | SB-1: Recent Days is one line per day with a muted block count (6th: 4, 5th: 4, 3rd: 2, 1st: 3) and many more days fit. SB-2: `# atlas` and `# beta` carry a `#` in the tag colour, in the one Pages list. SB-3: a Pinned group above Pages (Atlas Launch, Reading) with an unpin ✕ on each. SB-4: one footer row — the `demo.utopia` chip, the grey sync dot (sync off) — with no lock, because this graph is not encrypted. SB-5: the month strip is folded to its month-name row under Tomorrow. |
| `after-month-strip.png` | SB-5: the strip open. Seven equal columns; days with an entry (Nov 1, 3, 5, 6) get a dot; today (Nov 6) is drawn in the accent colour with an accent dot; out-of-month days are faint. One click on "November 2026" folds it (the recent list moves up, tested in `layout.rs`). |
| `after-sync-view.png` | SB-4: clicking the footer floats a sync view above it — "Off — this graph stays on this machine.", the (unset) target, "Not encrypted at rest." and how to turn sync on. It overlays the list instead of moving it. |
| `after-encrypted-lock.png` | SB-4: the footer of an encrypted graph (`enc.utopia`): the grey dot plus the drawn lock next to it. (A crop of the same footer; the lock is 8 pt wide.) |
| `after-page-menu-pin.png` | SB-3: an ordinary page's "···" menu offers "Pin to sidebar". The menu floats over the page and does not move the layout. |
| `after-page-menu-unpin.png` | SB-3: the same menu on a pinned page says "Unpin from sidebar"; the Pinned group above shows the page active. |
| `after-compact.png` | ALL-5 in the sidebar: the same content with `settings.json` `"density": "compact"` — Recent Days, Pinned and Pages rows are all tighter. |
| `after-settings-pinned.png` | SB-3: Settings → Appearance names the new preference — "Pinned pages", "2 pinned pages" and a Clear control. It is kept in `settings.json`, not `keymap.json` or the graph. |
| `after-pinned-daily.png` | SB-3 (review fix): a daily page can be pinned too. "November 6th, 2026" sits in the Pinned group above Pages; pins resolve against every live page, not only the Pages list. The scratch config put the id there directly because scripted AppKit clicks inside the deferred page menu do not fire in this build (see below). |

## What the screenshots do not show

- **The hover tooltip on a Recent Day** (SB-1). The shot script drives keys, clicks and
  scrolling but has no pointer-move step, so a tooltip cannot be captured. It is covered by
  `recent_day_hover_shows_its_first_block` in `crates/utopia-ui/tests/layout.rs`, which moves
  the pointer over the row, advances gpui's 500 ms tooltip delay and asserts the
  `recent-day-preview` element is laid out.
- **Dot colours for a live sync.** The mapping off/synced/syncing/error is unit-tested
  (`sync_runner::tests::the_dot_tells_off_synced_syncing_and_error_apart`); the captures only
  have sync off (grey). A relay was not started for the shots.
- **Drag to pin.** Pinning is offered from the page menu; dragging a page into the Pinned
  group was not implemented (the idea allows either).
- **Scripted clicks on the deferred page menu do not fire.** `click` on "Rename", "Delete
  page" or "Pin to sidebar" leaves the menu open (the row shows its hover fill, so the pointer
  is over it). This is pre-existing from ALL-3 and affects all three rows, not only the pin;
  the same listeners work through the test platform (`pinned_daily_pages_show_above_pages`,
  `pinned_pages_sit_above_pages_and_are_local`). It was not fixed here (out of this screen's
  scope) and is the reason the shot above seeds the pin directly.
