# Screen 02 — Daily Notes feed

The source changes are covered by `crates/utopia-ui/tests/layout.rs`,
`crates/utopia-ui/tests/workspace.rs` and unit tests next to the helpers:

- `feed_folds_empty_days_to_one_row` (DN-1, geometry)
- `folded_empty_day_opens_on_a_click` (DN-1, interaction; FR-1.2 still holds)
- `daily_notes_feed_stacks_days` (DN-2, caption above the date)
- `day_captions_name_the_weekday_and_the_neighbours` (DN-2, the caption text)
- `long_day_keeps_its_date_at_the_top` (DN-3, the pinned heading)
- `a_folded_day_does_not_keep_the_previous_day_pinned` (DN-3 with DN-1)
- `opening_the_page_menu_does_not_move_the_title` etc. still pass unchanged

## Captures

`before-feed.png` is the base branch's feed, copied from
`docs/verification/redesign-00-foundations/after-daily-notes.png` (the same Demo
graph as `docs/images/daily-notes.png`, after the type scale). It shows every empty
day carrying a full 28 px heading.

The after captures were taken with `UTOPIA_SHOT_SCRIPT` against a freshly seeded
scratch graph, never a real one:

```bash
cargo run -p xtask -- seed-daily target/redesign-02/Feed.utopia
UTOPIA_CONFIG_DIR=$PWD/target/redesign-02/config \
UTOPIA_SHOT_SCRIPT="wait 1500; keys escape; wait 500; shot …/after-feed-top.png; \
  scroll -1000; wait 800; shot …/after-feed-sticky.png; \
  hover 700 658; wait 500; shot …/after-feed-folded-hover.png" \
  target/debug/utopia --graph $PWD/target/redesign-02/Feed.utopia
```

`seed-daily` (`xtask/src/main.rs`) writes a long today, a written yesterday, two
empty days between them and an older day with notes, all linking to the same three
pages. A big `scroll` clamps at the end of the feed; `hover` is the new shot-script
step used for the hover-only hint.

| File | What it proves |
|---|---|
| `after-feed-top.png` | Today's caption "Friday · Today" sits above the 28 px date in the accent colour (DN-2), and nothing is pinned at the top of the feed. |
| `after-feed-sticky.png` | Scrolled into the tail of today: its caption and date stay pinned at the top of the column with a hairline under them (DN-3), while yesterday's open day and the folded days scroll beneath. |
| `after-feed-folded-hover.png` | Empty days after today and yesterday are one 36 px row each — "Wednesday  November 4th, 2026" and "Tuesday  November 3rd, 2026" in the muted colour — and the row under the pointer shows the faint "Click to start writing" hint (DN-1). |

Verified by eye: the captions name the right weekdays for the seeded dates, today's
caption is the accent colour while the others are muted, the folded rows are 36 px
and stack without rules, the hint appears only on the hovered row, the pinned date
covers the rows underneath it and stays inside the 760 px column, and no text
overflows or clips. Text left of the pin is the feed's own rows (not duplication).

Not captured: dark mode (the system appearance was left unchanged) and Compact
density (the folded row is a fixed 36 px row by design; the rest of the feed keeps
following the density setting).
