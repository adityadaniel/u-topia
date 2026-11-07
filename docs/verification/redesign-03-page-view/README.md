# Screen 03 — Page view

PG-1 … PG-5, each a view change: no CRDT shard, page identity, resolver or schema was
touched. The one new preference (PG-1) lives in `settings.json` and in Settings →
Appearance, like the font and density.

The source changes are covered by `crates/utopia-ui/tests/layout.rs`,
`crates/utopia-ui/tests/settings.rs` and the page-view unit tests:

- `attributes_are_shown_once_as_compact_rows` (PG-1)
- `show_as_blocks_reveals_the_blocks_and_is_saved` (PG-1)
- `a_compact_attribute_row_opens_the_block_for_editing` (PG-1, and the focused-row rule)
- `choosing_attribute_blocks_applies_everywhere_and_is_saved` (PG-1, Settings)
- `page_toolbar_sits_under_the_title` (PG-2)
- `page_menu_opens_the_graph` (PG-2)
- `type_scale_caps_outline_headings_at_h1` (PG-3, H1–H3 in order)
- `outline_headings_step_down_from_the_title` (PG-3, theme unit test)
- `block_affordances_appear_on_hover` (PG-4)
- `todo_box_is_a_drawn_control` (PG-4)
- `quote_is_indented_and_code_names_its_language` (PG-5)
- `a_code_blocks_language_is_its_first_line_when_it_names_one` (PG-5)

The compact rendering PG-1 adds also has to keep the page usable, so these pin the cases a
review found:

- `switching_to_compact_keeps_the_edited_descendant_rendered` — switching Settings →
  Appearance to Compact rows while a child of an attribute is being edited keeps that
  ancestor path expanded, so the editor's row (and focus handle) is still drawn.
- `backspace_merges_into_the_previous_visible_row` — Backspace at the start of the block
  after a folded attribute merges into the attribute, never into a hidden child (FR-2.5).
- `opening_a_compact_attribute_target_focuses_it` /
  `opening_a_hidden_descendant_reveals_and_focuses_it` — the public focus path (search,
  query, backlink, undo caret) opens a folded attribute's path rather than silently doing
  nothing.
- `starting_a_note_on_an_attribute_only_page_appends_a_block` and
  `starting_a_note_on_an_attribute_only_day_appends_a_block` — "Click to start writing" on
  a page or day whose only blocks are attributes adds a new block; typing never rewrites an
  attribute's key.

## Captures

Both sides use the same seeded scratch graph:
`cargo run -p xtask -- seed-page-view target/redesign-03/Demo.utopia` — a page with
three `key:: value` attributes, an alias, an H1/H2/H3 outline, a done and an open to-do,
a collapsed parent, a quote and two code blocks (one naming its language). Every capture
was taken with `UTOPIA_SHOT_SCRIPT`, `UTOPIA_CONFIG_DIR` and `--graph` under
`target/redesign-03/`, with `keys escape` before each shot.

`before-page.png` is the same page from a binary built from the branch's base
(`1439a80`, before this screen's commits); `after-*` are this branch.

| File | What it proves |
|---|---|
| `before-page.png` | The old page: attributes twice (metadata header and `status:: dogfooding` blocks), "Also known as: the board × + alias Rewrite references to this title" as one loose row, the `···` above the title, bold (700) outline headings, the quote in the interface grey, the code block as a grey band with `rust` on a line of its own, and ☑/☐ glyphs. |
| `after-page.png` | The same page after: one toolbar under the title (alias chip, `+ alias`, `···`), the metadata header with its `Show as blocks` toggle, each attribute once as a muted `key · value` row, semibold 20 px / 17 px / 15.5 px headings under the 28 px title, the quote with a 2 px accent bar in warm grey italic, the code block as a surface card in Menlo with `rust` at its top right, drawn to-do boxes (done struck through in `muted`) and the collapsed `Later` arrow. |
| `after-menu.png` | The page `···` menu, floating over the page without moving it: `Open in graph`, `Export`, `Rename`, `Rewrite references`, a hairline, then `Delete page` in the danger colour and last. |
| `after-hover.png` | The pointer over `Open questions`: the bullet has become the six-dot drag handle and the collapse arrow has appeared, while every other row stays quiet. |
| `after-blocks.png` | `Show as blocks` checked in the metadata header: the attribute blocks are drawn in full again, and the compact rows are gone (the preference is written to `settings.json`, never to the graph). |
| `after-settings-appearance.png` | Settings → Appearance with the new third group, `Attribute blocks`: `Compact rows` (the default) or `As blocks`, the same preference the page's `Show as blocks` toggle writes. |
| `after-compact.png` | The same page in Compact density (`{"version": 1, "density": "compact"}`): the toolbar, compact attribute rows, quote, code card and to-do boxes all still lay out and fit more on screen. |

The `before` capture was taken from `target/redesign-03/utopia-03-before` (a copy of a build
of the base tree), the after captures from `target/redesign-03/utopia-03`; both are copies
because `CARGO_TARGET_DIR` is shared with other worktrees and the binary there is rebuilt
by other agents.

Dark mode was not captured: the system appearance was deliberately left unchanged.
