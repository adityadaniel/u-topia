# Appearance preferences: applied in memory vs saved to settings.json

## What it controls

Font, density, `ref_children`, attribute-block style, pinned pages and right-sidebar width
(rule 9: local UI state never enters a shard). Each has an in-memory value in the
`Appearance` global and a value in `settings.json`; two different save paths exist.
`crates/utopia-ui/src/appearance.rs`, `settings.rs`, `workspace.rs`.

## Where the state lives

| Piece | Location | Notes |
|---|---|---|
| Global | `appearance.rs:456-462` `Appearance { prefs, config_dir, plex_registered }`, installed at `:467-480` | `install` resets everything, including `plex_registered` |
| Second path copy | `settings.rs:133` `SettingsView.config_dir` | passed to `set_font`/`set_density`/`set_ref_children`/`reset_right_sidebar_width` (`appearance.rs:587-605, :515-519`), which **always write** |
| Save-by-global | `remember(cx)` (`appearance.rs:577-583`) used by `set_attribute_blocks`, `toggle_pinned`, `clear_pinned`, sidebar drag end | returns `Ok(())` without writing when `config_dir` is empty (`:579-581`) |
| Third width copy | `workspace.rs:150` `right_width` | see `right-sidebar-panes.md` |
| Startup warning | `SettingsView::new :172` re-reads the file for warnings only | – |

## States

Per preference: **Default** / **Applied + saved** / **Applied, unsaved** (memory ahead of
disk) / **Reported saved, not saved** (`remember` no-op).

## Events and transitions

- Settings control → `set_*` with the view's `config_dir` → write.
- Pin, attribute style, sidebar width → `remember(cx)` → write if the global has a dir.
- `install` (startup, tests) → whole global replaced.

## Impossible or unhandled combinations (evidence)

1. **Sidebar width applied, not saved** when a drag ends via `pressed_button.is_none()`
   (`workspace.rs:1380-1383`, no `remember`); the Settings row "Width: N px"
   (`settings.rs:1466-1480`) reads the global and shows the unsaved value. Any later
   `prefs.save` (e.g. choosing a font, `appearance.rs:589`) writes the whole struct and
   heals it by accident.
2. **"Unpinned every page." with nothing written.** `clear_pins :510-520` reports success on
   `Ok(())`, which `remember` returns without writing when `Appearance.config_dir` is empty
   (`:579-581`). In the app both dirs come from `paths::config_dir()` (`main.rs:600, 677`),
   so this only bites a test that constructs `SettingsView` without `appearance::install`
   (`tests/settings.rs:14-22`): `choose_font` writes to the temp dir, `clear_pins` and
   `choose_attribute_blocks :546` write nowhere.
3. **A second `install` re-registers fonts and drops session-only changes** (`:473-477`).
   Only tests call it twice.
4. **Two readers of the file at startup** (global prefs vs the warning pass at `:172`); if
   a save repaired the file in between, the warning vanishes; if not, the user is told the
   file is bad although the sanitized default is in force. Consistent with the comment at
   `:204-205`.

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Default
    Default --> AppliedSaved : set_font/set_density/... (dir from the view)
    Default --> AppliedUnsaved : set_right_sidebar_width per pixel
    AppliedUnsaved --> AppliedSaved : resize_done -> remember
    AppliedUnsaved --> AppliedUnsaved : drag ends with no button (no remember)
    AppliedUnsaved --> AppliedSaved : any later prefs.save (whole struct)
    Default --> ReportedSavedNotSaved : remember with empty config_dir (tests)
```

## Would an explicit state type help?

The fix is one owner, not an enum: give `Appearance` the only `config_dir`, make every setter
go through `remember`, and have `remember` return `Err` (or a `Saved::Skipped` enum) when it
cannot write so the UI never says "saved".

```rust
enum Saved { Written, Skipped(&'static str) }
pub fn remember(cx: &mut App) -> std::io::Result<Saved>;
```
