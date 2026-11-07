# Screen 09 — right sidebar

The source changes are covered by `crates/utopia-ui/tests/layout.rs`,
`crates/utopia-ui/tests/workspace.rs` and `crates/utopia-ui/tests/settings.rs`:

- `inspector_shows_the_referencing_block` (RS-1): the panel's rows carry the block body and
  wrap inside the sidebar.
- `a_reference_row_in_the_sidebar_opens_its_block` (RS-1): clicking a row opens its page with
  the referencing block focused.
- `unresolved_references_can_be_created_from_the_sidebar` (RS-2): "Create page" resolves the
  intent, and the empty tab says "No unresolved references".
- `the_outline_lists_headings_and_jumps_to_them` (RS-3): only headings are listed, at the
  heading sizes, and a click puts the caret in that heading.
- `pinned_pages_can_be_added_and_removed` (RS-3): pin and unpin from the panel.
- `pinning_a_page_is_written_to_settings_json` (RS-3): the pinned list lives in
  `settings.json` (rule 9), not in a shard.
- `the_right_sidebar_resizes_and_closes` (RS-4): the 4 px handle widens the sidebar by the
  distance dragged, the × closes it, and the width comes back when it reopens.
- `the_sidebar_width_is_dragged_and_saved` (RS-4): one write to `settings.json` per drag, and
  Settings → Appearance reports the width and resets it.
- `a_saved_sidebar_width_is_used_at_startup` (RS-4).
- `sidebar_cards_collapse_and_close` (RS-5): a card folds to its header, closes with its ×,
  and re-opening a folded page keeps it folded with no duplicate.

## Captures

The graph was seeded with a Roam JSON export imported through the app's own CLI, so the
content is deterministic and no keystrokes typed it:

```bash
cargo run -p utopia-app -- import-roam target/redesign-09/Demo.utopia target/redesign-09/roam.json
```

Pages: *Atlas Launch* (attributes, H2/H3 headings, a to-do list, a quote, a missing
`((block))` chip), *Ana Lovelace* (two references to Atlas Launch, one of them a long
paragraph), *Reading*, and today's daily page (three references between them).
`target/redesign-09/config` was the scratch config dir.

| File | What it proves |
|---|---|
| `after-references.png` | RS-1: the References tab lists each source page with its count next to the name, and under it the referencing block itself — the same row Linked References draws, wrapped at the panel's edge — instead of a page name and a count. |
| `after-outline.png` | RS-3: the Outline tab lists the page's headings in document order, indented as in the outline (H3 *Decisions* under H2 *Risks*), drawn at the heading sizes. |
| `after-pinned.png` | RS-3: the Pinned tab with a pinned page and its unpin ×, and "Unpin this page" for the page being read. The pin came from `settings.json` (the list is a local preference, rule 9). |
| `after-unresolved-empty.png` | RS-2: the Unresolved tab's empty state, "No unresolved references". |
| `after-width-480.png` | RS-4: a remembered width of 480 px from `settings.json`; the reference rows re-wrap for the wider panel and the page gives the space up. |
| `after-stacked-cards.png` | RS-5: two pages stacked in the sidebar as cards, each with its fold arrow and close ×, above the References panel. |

### Not captured

- **The "Create page" row.** A dangling *page* intent only exists between deleting a page and
  the next commit that re-resolves its links (the app recreates the page then, by design —
  FR-4.14). Scripted deletion through the page menu did not take, so only the empty state is
  captured; the button is covered by `unresolved_references_can_be_created_from_the_sidebar`.
- **A drag of the resize handle and a folded card.** `UTOPIA_SHOT_SCRIPT` has no drag step, and
  its clicks on the card headers, the fold arrow, the pin control and the reference rows did not
  register in the runs captured here — so those states are proven by their tests only.
  `after-stacked-cards.png` shows the two card headers that carry the arrow and the ×.
- **Dark mode**: the system appearance was left unchanged.

Harness notes for the next screen, both found while capturing this one:

- Coordinates are content points: half the Retina pixels, minus the 28 pt title bar. The window
  frame in this session was `(640, 278) 1280x852`, content `1280x820`.
- A click into the sidebar lands on the *page title's* stale hitbox if the sidebar is opened
  after the page; opening the sidebar first (then the page) makes it land where the PNG says.
- Even then, a click registered in some runs and missed in the next with the window in the same
  place (the pointer moved, so the hover style followed it, but no handler ran). Only the
  Outline capture here came from a click that took. The same clicks pass under the test
  platform, so this is the AppKit event path, not the widgets.
