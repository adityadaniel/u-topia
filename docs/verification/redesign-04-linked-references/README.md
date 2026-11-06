# Screen 04 — Linked References

The source changes are covered by `crates/utopia-ui/tests/layout.rs`,
`crates/utopia-ui/tests/workspace.rs`, `crates/utopia-ui/tests/settings.rs`
and `crates/utopia-store/tests/v1_1.rs`:

- `reference_controls_are_one_row_with_chips_below` (LR-1)
- `a_reference_menu_floats_without_moving_the_layout` (LR-1, ALL-3)
- `selecting_a_co_reference_chip_filters_and_fills` (LR-1)
- `reference_counts_sit_next_to_the_name` (LR-2)
- `reference_shows_the_full_path_above_the_block` (LR-3)
- `reference_path_parts_open_their_target` (LR-3)
- `reference_children_come_from_the_index_in_order` (LR-4, store)
- `reference_children_are_inline_up_to_the_limit` (LR-4)
- `choosing_reference_children_is_saved_and_applies` (LR-4, Settings)
- `unlinked_references_count_and_link_all` (LR-5)
- `promote_all_mentions_links_every_mention_in_one_step` (LR-5, store)
- `page_menu_items_take_their_clicks` (bug found while building LR-1)
- `reference_controls_fit_the_sidebar_pane`, `reference_controls_fit_a_narrow_split_pane` (LR-1 review)
- `co_reference_counts_match_the_hits_they_select` (LR-1 review)
- `promote_all_mentions_keeps_overlapping_mentions_apart` (LR-5 review, store)
- `unlinked_count_reads_the_index_without_flushing` (LR-5 review, store)
- `co_reference_counts_use_the_filter_predicate` (LR-1 review, store)

## Captures

`before-page.png` is the pre-change capture from `docs/images/backlinks.png`,
downscaled for this folder.

The after captures were taken with `UTOPIA_SHOT_SCRIPT` against a freshly seeded
scratch graph under `target/redesign-04/`: `cargo run -p xtask -- seed-refs
target/redesign-04/Refs.utopia` seeds the Help demo graph plus one more child
under today's reference to Ana and two unlinked mentions of her, and the app ran
with `UTOPIA_CONFIG_DIR=target/redesign-04/config` (or `config-ref0` for the
folded variant). Each capture was read before it was kept; the first attempts
were black because other worktrees were driving the screen at the same time, so
they were retaken.

| File | What it proves |
|---|---|
| `after-references.png` | LR-1: one control row — a 240 px filter field, then `Any time ▾` and `Title ▾` as menu buttons — with the co-reference chip (`Atlas Launch 3`, the exact number of hits selecting it leaves) and its count on the line below. LR-2: `Linked References`, `November 6th, 2026`, `Atlas Launch` and `Reading` each carry their count as a chip right after the name. LR-3: `Atlas Launch › Risks` above the nested hit. LR-4: two children inline under the first hit and `+1 more`. LR-5: `Unlinked References 2` with `Link all`, closed. |
| `after-date-menu.png` | LR-1: the date control opens a floating menu with the current choice marked, over the references, without moving them (the layout below is identical to `after-references.png`). |
| `after-co-reference-selected.png` | LR-1: clicking the `Atlas Launch 3` chip selects it — it fills with the soft accent and an `×`, and exactly the three promised references remain. |
| `after-references-no-children.png` | LR-4: the same page with Settings → Appearance → "Children shown in references" set to 0: no child inline, one `+3 more` chip. |
| `after-unlinked-open.png` | LR-5: the section open — each mention keeps its page name, the matched text is marked, and each row has a `Link` button; `Link all` sits in the header. |
| `after-link-all.png` | LR-5: after `Link all`, both mentions became links (they now appear as a `Reading 2` group), the count is 0, and the section says `No unlinked mentions`. |
| `after-settings-appearance.png` | LR-4: Settings → Appearance → "Children shown in references" with 0, 1, 2 and 3 offered and 2 selected, beside the existing Font and Density choices. |
| `after-feed-references.png` | LR-2 in the Daily Notes feed: November 5th's Linked References carry their count as a chip next to the name, and so does the referencing day group. The feed has no filter controls, as before. |

Dark mode was not captured: the system appearance was deliberately left unchanged.
