# App root: startup state, side tasks, Settings window handle, quit paths

## What it controls

The desktop binary's lifecycle (`crates/utopia-app/src/main.rs`): choosing a graph, the
onboarding/unlock/ready/failed screens, the side tasks that only exist in the Ready state,
the single Settings window, and how the app quits.

## Where the state lives

| Piece | Location | Notes |
|---|---|---|
| Root state | `main.rs:165-180` `RootState::{Onboarding, Locked, Ready, Failed(String)}` | explicit enum; starts as `Failed(String::new())` placeholder (`:205`) until `:213-216` |
| Side tasks | `:187-190` `_hotkey`, `_discovery`, `_lan`, `_inbox` (`Option`s) | set only on the Ready path (`:340, :372, :383, :437`), never cleared (no transition leaves Ready) |
| Scripted run | `UTOPIA_SHOT_SCRIPT` read at `:217` (spawn) and again at `:329` (skip hotkey/probe) | two reads of one fact |
| Probe | `UTOPIA_PROBE` at `:331` | – |
| Hotkey latch | `hotkey.rs:8` `FIRED: AtomicBool`, polled every 150 ms (`main.rs:340-353`) | presses within one poll coalesce |
| Settings window | `:573` `SettingsWindow(Option<WindowHandle<SettingsView>>)`; set `:615`, cleared `:598` only when an `update` fails; `open_settings` deferred (`:582-584`) | stale after close until the next Cmd+, probes it (`:589-594`) |
| Theme | global set `:676`, `:696`, and by `observe_window_appearance` on the main window only (`:200-203`) | Settings reads it (`settings.rs:577`) without an observer |
| Quit | app-level `on_action(Quit) → cx.quit()` (`:680`); `Workspace::quit` runs `sync_index` first (`workspace.rs:1478-1485`); `on_window_closed` → `cx.quit()` when no windows remain (`:704-709`); `AppState::install_quit_hook` runs `sync_index` + `compact` on app quit (`app_state.rs:78-86`) | – |

## States

**Onboarding** / **Locked** (wrong passphrase → `lock()` again, `:308-311`) / **Ready**
(+ side tasks) / **Failed** (terminal, no retry UI, `:464-466`) / **Failed("")** placeholder.

## Events and transitions

- `Root::new` → `open_graph` or `onboard` (`:213-216`); script spawned regardless of state
  (`:217-227`; `ScriptHooks::workspace` returns `None` unless Ready, `:52-57`).
- Ready → start sync (LAN serve / LAN discover / config, `:355-409`), API, hotkey, probe.
- Cmd+, → `open_settings` (deferred) → raise or open (`:587-618`).
- Cmd+Q from the main window → `Workspace::quit`; from Settings → app-level handler; last
  window closed → `on_window_closed`.

## Impossible or unhandled combinations (evidence)

1. **Script fails to parse → no script, no hotkey, no quit.** `:225` logs and spawns nothing,
   but `:329-330` still sees the env var and skips hotkey registration; nothing calls
   `cx.quit()`, so the app sits there as a normal window with no global hotkey.
2. **Quit has three paths with different behaviour.** `Workspace::quit` wins only when the
   main window is focused (`workspace.rs:1478-1485` runs `sync_index`); Cmd+Q with Settings
   focused hits `:680`, and closing the last window hits `:704-709` — both skip the
   workspace's `sync_index`. The `on_app_quit` hook (`app_state.rs:78-86`) still flushes, so
   whether anything is lost is uncertain; the inconsistency is certain, and a sticky writer
   error at that point clears shards (see `store-commit-pipeline.md`).
3. **Side tasks live outside the enum.** `_hotkey`/`_discovery`/`_lan`/`_inbox` are
   `Option`s that are meaningful only in Ready; the enum cannot express "Ready but discovery
   still running".
4. **`Failed("")` is a representable nonsense value** (`:205`), never observable.
5. **Settings window handle goes stale silently.** Nothing listens for the window closing
   (`:573-618`); the next `open_settings` probes and replaces it. Two deferred calls in one
   frame: first opens, second raises — fine.
6. **Theme freezes for Settings** if the main window closes while Settings stays open (no
   observer on the Settings window).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Placeholder : Root::new (Failed(""))
    Placeholder --> Onboarding : no graph
    Placeholder --> Locked : graph encrypted
    Placeholder --> Ready : graph opened (side tasks spawned)
    Placeholder --> Failed : open error (terminal)
    Locked --> Locked : wrong passphrase
    Locked --> Ready : passphrase ok
    Onboarding --> Ready : name chosen
    Ready --> [*] : Cmd+Q in main window (sync_index) | Cmd+Q in Settings (no sync_index) | last window closed
    note right of Ready : UTOPIA_SHOT_SCRIPT parse error -> no script, no hotkey, no quit
```

## Would an explicit state type help?

`RootState` already exists; the fixes are to move the side tasks into `Ready { tasks:
ReadyTasks }`, parse the script once into an `enum Mode { Interactive, Scripted(Vec<Step>),
Probe }`, and route every quit through one `Quit` handler that owns the `sync_index` call.

```rust
enum RootState { Onboarding, Locked { attempts: u32 }, Ready(Ready), Failed(String) }
struct Ready { ws: Entity<Workspace>, hotkey: Option<Task<()>>, discovery: Option<Task<()>>, lan: Option<LanRelay>, inbox: Option<Task<()>> }
```
