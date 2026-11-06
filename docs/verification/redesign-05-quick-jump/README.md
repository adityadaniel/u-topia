# Quick jump (⌘K): QJ-1 … QJ-5

Branch: `redesign/05-quick-jump` (based on `redesign/00-foundations`).

Captured with `shots.sh` on 2026-11-06, on the seeded scratch graph (`seed.json` through
`import-roam`), window 2560×1704, downscaled to 1200 px. Every shot was read before it was
committed.

| File | What it proves |
|---|---|
| `before-quick-jump.png` | The old palette, from `crates/utopia-ui/assets/help/quick-jump.png`: flat rows, a create row that looked like a result, no footer. |
| `01-empty.png` | QJ-1, QJ-3: with nothing typed the palette offers today and yesterday — day dot, date, and "Today"/"Yesterday" as the type label — with the key-hint footer under a hairline. The page behind is exactly where it was. |
| `02-typed.png` | QJ-2, QJ-4, QJ-3: typing `atl` gives a page row (document icon, "Page" label) with the typed letters semibold, and "＋ Create \"atl\"" last, in the accent colour, under its own hairline; footer visible. |
| `03-commands.png` | QJ-5: a leading `>` lists `actions::CATALOG` with the keys in force (⇧⌘D, ⇧⌘T, ⇧⌘Y, ⌘[, ⌘], ⌘K, ⇧⌘F, ⌥⌘↓ …). Window commands are dark; editor commands (`Zoom In To Block`, `New Block`, …) are greyed out because no block is in edit, so Enter cannot run them. The list is clipped at the palette's height cap and scrolls, the footer stays put. |
| `04-empty-recents.png` | QJ-1: after opening `Atlas Launch`, it is the third row under today and yesterday, with a page icon and "Page". The page behind did not move. |
| `05-tag-page.png` | QJ-2: `operations` — a page only a `#tag` ever reached — gets the purple `#` icon and the type label "Tag". |

The review fixes are visible in these shots: character-boundary highlighting (02; non-ASCII
titles are covered by `quick_jump::tests::match_ranges_stay_on_char_boundaries`), the
greyed-out editor commands and the scrolling list (03), the live recent list (04), and the
keys in force (03 shows the catalog defaults; `settings.rs` covers a rebinding).

## Still stale

`crates/utopia-ui/assets/help/quick-jump.png` (Settings → Help) still embeds the old palette.
Rerun `cargo run -p xtask -- help-shots` once every redesign screen has landed, so the Help
tour is regenerated once rather than per branch.
