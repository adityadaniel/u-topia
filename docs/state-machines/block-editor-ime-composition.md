# Block editor IME composition: marked text vs committed text vs store

## Status

Composition is now one explicit state, not a range anybody may clear:

- `field::Composition` (`field.rs:34`) is `None | Active { range }`. It is entered by the
  input method's `setMarkedText` (`TextField::begin_composition`, `field.rs:89`), left by
  its commit (`replace_text_in_range` → `TextField::replace`, `field.rs:142`), its unmark
  (`unmark_text` → `TextField::end_composition`, `field.rs:95`) or by the editor taking the
  marked text as committed when the field stops being the input handler
  (`accept_composition`, `editor.rs:616`; `focus_lost`, `editor.rs:625`). Every host path
  asks `TextField::composing()` (`field.rs:84`) before touching the buffer.
- The editor does not *refuse* a host request while composing, it **queues** it:
  `Deferred` (`editor.rs:220`) holds `Commit { finished }` or `Begin`, and
  `after_composition` (`editor.rs:642`) runs it when the input method lets go. The latest
  request wins, except the idle commit, which never displaces one (`defer`,
  `editor.rs:634`). A store change that arrived meanwhile sets `pending_sync`
  (`editor.rs:254`) and `settle` (`editor.rs:587`) looks at it.
- `end` (`editor.rs:365`) and a focus move do **not** queue: they take the marked text as
  committed and persist it, because the platform never delivers another IME event to a field
  that is no longer its input handler. `Workspace::new` registers the focus-out listener
  (`workspace.rs:271`).

Fixed, each with a failing-first acceptance test in `crates/utopia-ui/tests/workspace.rs`
(named after §13.5):

| Issue | Test | Fix |
|---|---|---|
| 1 host-driven commit mid-composition | `ac_13_5_host_commit_waits_for_the_ime` (idle commit) and `ac_13_5_focus_leaving_commits_the_ime_composition` (end / focus move) | `commit_block` (`editor.rs:721`) and `begin` (`editor.rs:323`) defer; `end` and `focus_lost` take the marked text as committed and persist it (`settle`) instead of rewriting the buffer. |
| 2 destructive actions clear `marked` | `ac_13_5_destructive_actions_leave_ime_composition_alone` | `backspace` (`editor.rs:1099`), `delete` (`:1129`), `soft_break` (`:1079`), `paste` (`:1371`), `cut` (`:1355`), `insert_text` (`:494`), menu `accept` (`:893`) and `newline` (`:1041`) return early; the IME's own edit arrives through `replace_and_mark_text_in_range`. |
| 3 `mark()` shifts text under a stale range | `ac_13_5_mark_leaves_ime_composition_alone` | `mark` (`editor.rs:1299`) returns early. |
| 4 remote/undo change dropped mid-composition | `ac_13_5_ime_composition_ends_when_its_block_is_deleted`; `ac_13_5_a_live_store_change_under_a_composition_keeps_the_users_text` pins what is *not* fixed | `sync_from_store` (`editor.rs:382`) sets `pending_sync`; `settle` re-reads. **Partially fixed**: a block that went away ends the edit, after persisting so the composed characters stay in the tombstone; a block that is still live keeps the user's just-committed text (see below). |
| 5 `LineInput::set_text` never emits | `ac_13_5_line_input_set_text_notifies_the_host` | `line_input.rs:80` emits `Changed` on a real change, so `utopia://search` filters on the text it put in the box. |

Added after review:

- `ac_13_5_begin_on_another_block_waits_for_the_ime`, `ac_13_5_a_later_request_supersedes_a_queued_begin`,
  `ac_13_5_composition_leaves_the_idle_commit_to_its_timer`,
  `ac_13_5_ime_ranges_and_unmark_commit_cleanly` (explicit UTF-16 ranges over a non-BMP
  character, a dead key, `unmark_text`), `ac_13_5_empty_marked_text_ends_the_composition`,
  and `line_input_set_text_notifies_only_a_change` (`crates/utopia-ui/tests/line_input.rs`).

Left as it was, deliberately:

- A remote edit or undo that lands **while** the composition is live, to the same **live**
  block, is superseded by the text the user commits. That is the editor's existing
  last-writer-wins block write (`set_block_text`), not this machine: rebasing the buffer
  onto the new stored text (`utopia_core::text::diff` around the marked range, which is
  well defined because the composition is one contiguous range) means deciding what a
  concurrent edit *means* mid-composition, and the PRD puts concurrent edits on the CRDT
  side. Pinned by a test rather than hidden.
- `LineInput` still emits no `Changed` for composition text itself (only for the commit).
  That is right: a query should not filter on glyphs the IME has not committed.
- `LineInput::set_text` still replaces the buffer even while the user composes in it, so a
  `utopia://search` link that arrives mid-composition drops the composition (the field
  cannot queue host writes the way the block editor does). It is not on the doc's list and
  needs a deferral the single-line input does not have; recorded rather than guessed at.
- Caret moves (`mouse_down`, left/right/home/end) still move `sel` while composing if the
  platform lets the key through. The composition text survives; the reported selection is
  then outside the marked range, which macOS takes as "the IME's selection changed". No
  data is lost and no test in the doc's list covers it, so it is left for a real IME
  session rather than guessed at from the harness.

Where this doc's sketch was wrong, and what was done instead:

- `snapshot_before` is not needed and is not in the type. It only pays off if a composition
  can be *abandoned* (it cannot: what the IME marked is what it committed) or if the buffer
  were rebased onto a concurrent store edit (not done, see above).
- `commit_block` alone was not enough, and deferring `end` was worse: the wipe also came
  from `begin`'s `set_text`, while deferring `end` left the composition live after every
  caller had moved focus — a state with no way out. `begin` defers (the editor keeps focus,
  so the IME can still reach it); `end` and focus loss accept the marked text.
- "`field.replace`/`set_text` would take a `&mut Composition` by value" is not implementable
  as written: `replace` *is* the input method's commit path (`replace_text_in_range`), so it
  must end a composition, and taking the state by value at every call site would force the
  IME's own commit through a different API than every other replacement. The mode is
  checked by the host paths instead, and `set_text`/`replace` end it explicitly rather than
  by assigning `None` in passing.
- Issue 4's "uncertain whether `app.changed` re-notifies" is settled: it does
  (`cx.observe(&app)` in `BlockEditor::new`), the early return really did drop the change.

## What it controls

Whether the text in the field is provisional (an IME composition in progress, `§13.5`),
whether keystrokes persist to the store, and whether the store may overwrite the field.
Lives in `crates/utopia-ui/src/field.rs` (`TextField`) and is consumed by
`crates/utopia-ui/src/editor.rs` and `line_input.rs`.

## Where the state lives

| Piece | Location | Set | Cleared |
|---|---|---|---|
| Composition | `field.rs:60` `composition: Composition` | `replace_and_mark_text_in_range :396` → `begin_composition :89` | `replace_and_mark_text_in_range` with empty text, `replace :142` (the IME's commit), `set_text :101`, `unmark_text :366` → `end_composition :95`, `editor.rs:616` (focus leaving) |
| Selection | `field.rs:58-59` `sel`, `reversed` | IME sets at `:440-441` | – |
| Store copy of the text | via `persist` (`editor.rs:660`) | every `settle` | – |
| Persist gate | `editor.rs:577` `after_edit` returns early while composing | – | – |
| Store→field gate | `editor.rs:384` `sync_from_store` defers while composing | – | – |
| Queued host request | `editor.rs:254` `deferred: Option<Deferred>` | `defer :634` | `after_composition :642`, `accept_composition :616` |
| Queued store read | `editor.rs:254` `pending_sync: bool` | `sync_from_store :387` | `settle :587` |
| Focus-out listener | `workspace.rs:271` | `Workspace::new` | with the workspace |

Read outside: only through `EntityInputHandler` (`field.rs:357, 366, 371, 396`) and editor
render (`editor.rs:1556`).

## States

- **Not composing**: `Composition::None`; store text == field text after the last
  `persist`.
- **Composing**: `Composition::Active { range }`; field text contains provisional glyphs
  the store does not, and the editor holds any host request that would touch them in
  `deferred` (plus `pending_sync` if the store moved). The field is the window's input
  handler, so the IME can still reach it.

## Events and transitions

- `replace_and_mark_text_in_range` with text → Composing (no persist).
- `replace_and_mark_text_in_range` with empty text → Not composing (`after_edit` runs).
- `replace_text_in_range` / `unmark_text` → Not composing, then `after_edit` persists and
  runs the deferred request.
- `commit_block` / `begin` while composing → stays Composing, queues the request (the
  editor keeps focus).
- `end` / focus moving off the editor's handle while composing → Not composing; the marked
  text is persisted and anything queued for the field is dropped.
- A store change while composing → stays Composing, sets `pending_sync`.

## Impossible or unhandled combinations (evidence)

The five the old flags allowed, now closed (tests above; numbers are the doc's):

1. **Host-driven commit mid-composition.** `commit_block` had no guard and read
   `self.field.text` (provisional glyphs included) through `expand_relative_dates`, then
   wrote the older stored text back with `set_text`, which cleared `marked` while the
   platform still composed. Closed by the deferral, and by accepting the marked text where
   the host moves focus (the platform sends nothing more).
2. **Action handlers clear `marked` silently.** `backspace`/`delete`/`paste`/`cut`/
   `soft_break`/`insert_text` replaced the selection and persisted. Closed by the guards;
   Backspace during composition is the IME's own backspace, which arrives as
   `replace_and_mark_text_in_range`.
3. **`mark()` mutates text under a stale range.** Closed by the guard.
4. **A remote/undo change arriving mid-composition was dropped.** Closed for a block that
   goes away; a live block keeps last-writer-wins (above).
5. **`LineInput` was silent for `set_text`.** Closed by emitting `Changed` on a change.

What remains possible: the composition is only known to this crate, so a caller that
writes `TextField::set_text`/`replace` without asking `composing()` recreates
`NotComposingDetached` — `LineInput::set_text` does that today (above), and any new host
that writes the buffer must ask. That is the residual risk of keeping the check at the call
site (see Status for why the type cannot enforce it).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> NotComposing
    NotComposing --> Composing : replace_and_mark_text_in_range (begin_composition)
    Composing --> Composing : more marked text
    Composing --> NotComposing : replace_text_in_range | unmark_text | empty text -> settle
    Composing --> Composing : commit_block / begin -> Deferred (queued)
    Composing --> Composing : store change -> pending_sync
    Composing --> NotComposing : end | focus out -> settle (accept the marked text)
    NotComposing --> NotComposing : after_composition runs Deferred::Commit / Begin
```

`NotComposingDetached` — our composition ended while the OS composition was still live — is
unreachable from the block editor's own paths: it always answers the platform (`marked`
until it truly ends) and closes the composition only where the platform cannot reach it
any more. The remaining way in is a caller that writes `TextField` without asking
`composing()`.
