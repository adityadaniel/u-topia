# Keymap overrides: editor copy, live gpui bindings, keymap.json

## What it controls

Which keys fire which action, what the Keybindings editor and the Shortcuts sheet show, what
Quick Jump prints next to a command, and what survives a restart. AGENTS.md says
`utopia_ui::actions::CATALOG` is the one source of truth; the *overrides* on top of it live
in three places. `crates/utopia-ui/src/actions.rs`, `settings.rs`, `quick_jump.rs`.

## Where the state lives

| Copy | Location | Written by |
|---|---|---|
| Editor's view | `settings.rs:134` `SettingsView.overrides` (`keys_for :1022`, `is_changed :1023`) | load `:167`, rebind, reset |
| Live bindings | gpui `cx.key_bindings()` via `install`/`reinstall` (`actions.rs:1037-1059`); `active_keys :1009-1026` is what Quick Jump shows (`quick_jump.rs:307, 1151`) | `apply :346` |
| Disk | `keymap.json` via `Overrides::save` (`actions.rs:958-961`) and `Overrides::load` at startup (`main.rs:670, 678` → `actions.rs:1063-1075`) | `apply :348` |
| Clash rule | `actions.rs:992` `a.context == b.context` (string equality) | – |

## States

Per action: **Default** / **Overridden (valid)** / **Overridden (invalid keys)** /
**Overridden equal to default** — and for each, whether disk, live and editor agree.

## Events and transitions

- `rebind` (`settings.rs:283-316`): set or (if equal to default, `:285`) remove the override;
  `apply` → `reinstall` then `save`; clash computed afterwards (`:292-304`).
- `reset_one :319`, `reset_all :338` → same `apply`.
- Startup: `install_keymap` reads the file; `SettingsView::new` reads it again (`:167`).

## Impossible or unhandled combinations (evidence)

1. **Invalid keys in the file: editor shows the override, gpui uses the default.**
   `Overrides::from_json` keeps any entry whose *action* is known without validating the keys
   (`actions.rs:924-925`); `install` drops it and keeps the default (`:1037-1042`, `:1047`,
   `eprintln!("ignoring keybinding …")`). So the editor shows e.g. `nonsense-key` with the
   changed marker, the Shortcuts tab agrees, Quick Jump shows `⇧⌘T`, and `⇧⌘T` fires. Test
   `an_unusable_override_keeps_the_default` (`tests/settings.rs:781-806`) asserts only the
   gpui side.
2. **Save fails: live and editor are new, disk is old.** `apply :346-351` reinstalls first,
   then saves; on `Err` the status says so (`:349`) but the chip shows no "unsaved" state and
   the next launch silently reverts. Intended per the comment at `:344-345`.
3. **Clash detection compares contexts by string.** `Some("BlockEditor")`,
   `Some("BlockEditor || LineInput")` and `None` are treated as unrelated (`:992`), yet all
   three apply inside the editor. Rebinding `Bold` (EDITOR, `:319-326`) onto `cmd-c` (Copy is
   TEXT, `:360-367`) produces no "Also used by" line; gpui resolves one of them. The doc
   comment (`:986-987`) describes only "editor vs whole window".
4. **An override equal to the default** (hand-edited file) shows the changed marker
   (`is_changed :971-973`) and installs an identical binding; it is only dropped when the
   chord is re-entered (`settings.rs:285-287`). Cosmetic.
5. **No file watcher**: both copies are read at open; an external edit while Settings is
   open is not reflected (uncertain whether that matters).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Default
    Default --> OverriddenValid : rebind (reinstall + save)
    OverriddenValid --> Default : reset | rebind to default chord
    OverriddenValid --> LiveOnly : save fails (status only; reverts at restart)
    [*] --> OverriddenInvalid : keymap.json has unparsable keys (editor: shown, gpui: default)
    OverriddenInvalid --> OverriddenValid : user re-enters a chord
    Default --> OverriddenEqualsDefault : hand-edited file (marker shown, no effect)
```

## Would an explicit state type help?

Yes: validate on load and carry the verdict, so every reader asks the same value.

```rust
enum Override { Valid(String), Invalid { raw: String, why: String }, SameAsDefault }
struct Overrides(BTreeMap<&'static str, Override>);
// install(): only Valid installs; the editor renders Invalid with a warning chip
// clash(): compare by *scope*, where scope = the set of contexts the binding is live in
```
