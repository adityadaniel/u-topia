# Screen 06 — Search (⌘⇧F)

The palette is now one component for Quick jump and Search (SR-5): pages first, then
blocks grouped under their page or day. Search mode opens on the first block, Quick jump
on the first page.

## Tests

`crates/utopia-store/tests/acceptance.rs` (the index work the views read):

- `sr_1_search_marks_the_matched_terms` — hit anchors cover the whole matched word in the
  plain text the row draws, never a bracket the renderer drops; prefix only at a word start.
- `sr_2_search_groups_results_by_page` — groups keep the page's title and daily flag.
- `sr_3_search_scopes_and_age` — All/Journal/Pages/To-dos and Any time/Today/Past
  week/month/year, daily hits dated by the day and other hits by the last edit.
- `sr_3_age_window_is_applied_before_the_limit` — 305 older matches cannot starve a Today
  hit out of the candidate list: the window is part of the query.
- `sr_3_age_window_excludes_future_days` — a tomorrow daily page is not Today or the past
  week (Any time still has it).
- `sr_4_preview_has_path_children_and_state` — path root-first, first children and total
  count, block kind and checked state; a missing block is `None`, never an error.

`crates/utopia-ui/tests/workspace.rs` (simulated keystrokes and clicks in the real window):

- `search_groups_results_under_their_page` — a header per page, the icon on the top line
  of the header, blocks indented and stacked below it.
- `group_icon_stays_on_the_top_line_when_a_name_wraps` — SR-2's decision, with a page name
  long enough to wrap onto two lines.
- `search_scope_and_age_filter_the_results` — the Journal chip filters to days; Pages +
  Today drops an October note; the age menu is clicked, not just opened.
- `search_preview_follows_the_selection` — a page selection has no preview; the first
  block's preview has its path and state; ↓ moves to the next block and the preview
  follows.
- `search_scope_filters_page_titles` — Journal/Pages/To-dos filter title rows too, and the
  inserted ISO-date hit obeys the same scope.
- `one_palette_opens_on_pages_or_blocks` — ⌘K selects the page row that sits above the
  grouped blocks and offers Create page; ⌘⇧F selects the first block and offers no Create.
- `search_scrolls_the_selected_block_into_view` — twelve title rows fill the viewport and
  the selected block is scrolled into view, so Enter never opens an invisible result.
- `full_search_and_quick_capture` — the existing ⌘⇧F / quick-capture script still passes.

`crates/utopia-ui/tests/layout.rs`:

- `search_scope_row_and_age_menu` — the chips sit under the field inside the palette; the
  "Any time" menu floats below its control and opening it does not move the chips.
- `search_preview_sits_in_the_right_half` — the dialog is 860 px, the list ends before
  the preview, and the preview stays inside the dialog.
- `search_palette_fits_the_minimum_window` — at the supported 720×480 window the palette,
  input, chips, list and preview all stay inside the window.
- `quick_jump_is_centered` — updated for the one 860 px palette.

## Captures

Seeded with a Roam JSON import into `target/redesign-06/scratch/Demo.utopia` (Atlas Launch,
Ana Lovelace, Reading and two journal days), then driven with `UTOPIA_SHOT_SCRIPT` and a
scratch `UTOPIA_CONFIG_DIR` under the same folder. `keys escape` precedes every shot so
the block editor shows rendered text, not `[[markup]]`.

| File | What it proves |
|---|---|
| `before-search.png` | The old 520 px flat list: no highlighting, grouping, scope or preview. Downscaled from `docs/images/search.png`. |
| `after-search-results.png` | "beta": matched words carry a soft accent wash (SR-1), results sit under page/day headers with counts (SR-2), the scope and age chips are under the field (SR-3), and the selected block has its path, state and children on the right (SR-4). |
| `after-age-menu.png` | The "Any time" menu floats over the preview; the chips, input and palette bounds are unchanged from the shot above (deferred + anchored). |
| `after-journal-scope.png` | The Journal chip selected: only daily pages remain grouped, and the preview names the block's state and edit date. |
| `after-quick-jump-merged.png` | ⌘K in the same palette: the page row ("Atlas Launch") is selected first, Create page follows the pages, then the grouped block hits (SR-5). |
| `after-todo-preview.png` | A to-do hit's preview shows the accent "To-do" state chip beside the edit date. |
| `after-nested-path.png` | A nested hit's path is "Reading › On conflict-free replicated data types", not just the page. |
| `after-preview-children.png` | A heading hit: "Heading 2" state, "3 children", bullet and to-do markers, and the block's text. |
| `after-compact.png` | The same query with `"density": "compact"` in `settings.json`: the palette, groups and preview tighten with the rest of the app and retain 860 px width. |
| `after-scrolled-selection.png` | A graph with twelve matching page titles: the list is scrolled so the block ⌘⇧F selected is visible at the bottom, with its preview. |

Dark mode was not captured; the system appearance was left alone.

## Checks

`make check` (fmt, build, test, clippy with `-D warnings`) passed:

```
cargo fmt --all -- --check          (no output)
cargo build --workspace             Finished `dev` profile ... in 7.62s
cargo test --workspace              295 passed, 0 failed (36 suites)
  tests/acceptance.rs               38 passed
  tests/layout.rs                   20 passed
  tests/settings.rs                 13 passed
  tests/workspace.rs                56 passed
cargo clippy --workspace --all-targets -- -D warnings
                                    Finished `dev` profile ... in 3.81s
```

One earlier run failed `utopia-syncd --test http::lan_relay_serves_and_is_discoverable`
because mDNS found another agent's relay advertising `home-graph` on the shared network;
the test passed on the retry, and this branch does not touch sync.
