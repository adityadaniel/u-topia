# Quick jump (⌘K): QJ-1 … QJ-5

Branch: `redesign/05-quick-jump` (based on `redesign/00-foundations`).

## What to look for

| Shot | Idea | What it proves |
|---|---|---|
| `01-empty.png` | QJ-1 | Nothing typed: Today, then Yesterday, each with a day icon and its type label. No create row, no matches. |
| `02-typed.png` | QJ-2, QJ-4 | A page row with the page icon and the type label "Page"; the typed letters are semibold inside the result. |
| `03-create.png` | QJ-2 | "Create …" is the last row, in the accent colour, under its own hairline, while the matching page stays a result above it. |
| `04-commands.png` | QJ-5 | After `>`, the actions from `actions::CATALOG` with their keys, in catalog order. |
| `05-empty-recents.png` | QJ-1 | Today, Yesterday and the page opened last, offered before typing. |
| `06-tag-page.png` | QJ-2 | A page only a `#tag` ever reached gets the `#` icon and the label "Tag". |
| `before-quick-jump.png` | — | The previous palette, from `crates/utopia-ui/assets/help/quick-jump.png`: flat rows, a create row that looked like a result, no footer. |

QJ-3 (the footer of key hints: "↑↓ move · ↩ open · ⌘↩ open in split · esc") is visible
at the bottom of every palette shot.

## Status: shots not captured — the screen was locked

`docs/verification/redesign-05-quick-jump/shots.sh` is the capture script. On the machine
this branch was written on the login session stayed locked for the whole session
(`CGSSessionScreenIsLocked = true`), and a locked screen makes a window capture black —
all five PNGs came out uniformly black (checked, not assumed), so they were deleted rather
than committed as false evidence. Run `shots.sh` with the screen unlocked and read every
PNG before merging; the layout is otherwise covered by
`crates/utopia-ui/tests/layout.rs` (row order, gutter, right-aligned type labels, palette
height cap, Compact density, layout stability) and by `crates/utopia-ui/tests/workspace.rs`
(empty state opens today and a recent page, tag-page labelling, `>` commands running).

## Also stale

`crates/utopia-ui/assets/help/quick-jump.png` (Settings → Help) shows the old palette and is
embedded with `include_bytes!`. Rerun `cargo run -p xtask -- help-shots` once every redesign
screen has landed, so the Help tour is regenerated once rather than per branch.
