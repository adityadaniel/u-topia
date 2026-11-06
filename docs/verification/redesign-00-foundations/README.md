# Screen 00 — across all screens

The source changes are covered by `crates/utopia-ui/tests/layout.rs` and
`crates/utopia-ui/tests/settings.rs`:

- `type_scale_caps_outline_headings_at_h1` (ALL-1)
- `page_menu_holds_the_danger_delete` (ALL-3)
- `focused_block_has_an_accent_bar` (ALL-4)
- `compact_density_is_tighter` (ALL-5)
- `choosing_density_applies_everywhere_and_is_saved` (ALL-5)

## Captures

`before-daily-notes.png` and `before-page.png` are the pre-change captures from
`docs/images/`, downscaled for this folder.

The post-change captures could not be taken in this session: by the time the
feature branch had built, the macOS console session was at the login screen.
`UTOPIA_SHOT_SCRIPT` then writes a black PNG because `CGWindowListCreateImage`
cannot see a window on a locked screen. The graph and configs used for the
attempt are under `target/redesign-00/`; rerun once the session is unlocked:

```bash
source ~/.cargo/env
cp target/help-shots/Demo.utopia target/redesign-00/Demo.utopia
UTOPIA_CONFIG_DIR=target/redesign-00/config \
UTOPIA_SHOT_SCRIPT="wait 1200; keys escape; wait 400; \
  shot target/redesign-00/shots/daily-notes.png; \
  keys cmd-k; wait 300; type Atlas; wait 500; keys enter; wait 900; keys escape; \
  shot target/redesign-00/shots/page-atlas.png" \
  cargo run -p utopia-app -- --graph target/redesign-00/Demo.utopia
```

Settings → Appearance (font and density controls) is captured in a second window:

```bash
UTOPIA_CONFIG_DIR=target/redesign-00/config \
UTOPIA_SHOT_SCRIPT="wait 1200; keys cmd-,; wait 1200; target last; wait 500; \
  shot target/redesign-00/shots/settings-appearance.png" \
  cargo run -p utopia-app -- --graph target/redesign-00/Demo.utopia
```

Compact density uses `target/redesign-00/config-compact/settings.json`
(`{"version": 1, "font": "system", "density": "compact"}`).
