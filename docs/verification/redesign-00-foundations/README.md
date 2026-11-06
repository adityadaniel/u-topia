# Screen 00 — across all screens

The source changes are covered by `crates/utopia-ui/tests/layout.rs` and
`crates/utopia-ui/tests/settings.rs`:

- `type_scale_caps_outline_headings_at_h1` (ALL-1)
- `page_menu_holds_the_danger_delete` (ALL-3)
- `focused_block_has_an_accent_bar` (ALL-4)
- `compact_density_is_tighter` (ALL-5)
- `choosing_density_applies_everywhere_and_is_saved` (ALL-5)
- `keybindings_scroll_inside_a_short_window` (Settings body fix)
- `daily_page_menu_does_not_offer_rename` (daily title fix)

## Captures

`before-daily-notes.png` and `before-page.png` are the pre-change captures from
`docs/images/`, downscaled for this folder.

The after captures were taken with `UTOPIA_SHOT_SCRIPT` against a freshly seeded
`target/help-shots/Demo.utopia`, copied to `target/redesign-00/`. Compact used
`target/redesign-00/config-compact/settings.json` with
`{"version": 1, "font": "system", "density": "compact"}`.

| File | What it proves |
|---|---|
| `after-daily-notes.png` | The Daily Notes feed with the 28 px day headings, the body/caption type scale, and the quiet sidebar controls. |
| `after-page.png` | The Atlas Launch page with its 28 px title, H1/H2 outline scale, metadata header and the quiet `Any time ▾` / `Sort: title ▾` controls. |
| `after-page-focus.png` | The block being edited (`A beta for twenty testers…`) carries the 2 px accent focus bar at its left edge. |
| `after-page-menu.png` | The page `···` menu is open: ordinary `Rename` and the danger-coloured `Delete page`. |
| `after-hover.png` | The reference date control under the pointer shows the quiet-button hover state, a soft fill and 1 px line (clicking moved it from `Any time` to `Last 7 days`). |
| `after-settings-appearance.png` | Settings → Appearance with the Font choices and the Density control, Comfortable selected. |
| `after-page-compact.png` | The same Atlas Launch page in Compact density: smaller body text and tighter block spacing, so more content fits above the fold. |

Dark mode was not captured: the system appearance was deliberately left unchanged.
