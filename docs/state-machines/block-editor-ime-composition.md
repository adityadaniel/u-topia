# Block editor IME composition: marked text vs committed text vs store

## Status

Composition is now one explicit state, not a range anybody may clear:

- `field::Composition` (`field.rs:34`) is `None | Active { range }`. It is entered by the
  input method's `setMarkedText` (`TextField::begin_composition`, `field.rs:89`), left by
  its commit (`replace_text_in_range` → `TextField::replace`, `field.rs:142`) or its
  unmark (`unmark_text` → `TextField::end_composition`, `field.rs:95`). Every host path asks
  `TextField::composing()` (`field.rs:84`) before touching the buffer.
- The editor no longer *refuses* a host request while composing, it **queues** it:
  `Deferred` (`editor.rs:216`) holds the strongest of `Commit { finished }` / `End` /
  `Begin` (`rank`, `editor.rs:232`), `after_composition` (`editor.rs:613`) runs it when the
  input method lets go. A store change that arrived meanwhile sets `pending_sync`
  (`editor.rs:257`) and is looked at in `after_edit` (`editor.rs:578`).

Fixed, each with a failing-first acceptance test in `crates/utopia-ui/tests/workspace.rs`
(named after §13.5):

| Issue | Test | Fix |
|---|---|---|
| 1 host-driven commit mid-composition | `ac_13_5_host_commit_waits_for_the_ime` | `commit_block` (`editor.rs:684`), `end` (`editor.rs:368`) and `begin` (`editor.rs:326`) defer instead of rewriting the buffer. |
| 2 destructive actions clear `marked` | `ac_13_5_destructive_actions_leave_ime_composition_alone` | `backspace` (`editor.rs:1062`), `delete` (`:1092`), `soft_break` (`:1042`), `paste` (`:1334`), `cut` (`:1318`), `insert_text` (`:495`), menu `accept` (`:856`) and `newline` (`:1004`) return early; the IME's own edit arrives through `replace_and_mark_text_in_range`. |
| 3 `mark()` shifts text under a stale range | `ac_13_5_mark_leaves_ime_composition_alone` | `mark` (`editor.rs:1262`) returns early. |
| 4 remote/undo change dropped mid-composition | `ac_13_5_ime_composition_ends_when_its_block_is_deleted` | `sync_from_store` (`editor.rs:383`) sets `pending_sync`; `after_edit` re-reads. A block that went away ends the edit *before* persisting, a live one keeps the user's just-committed text. |
| 5 `LineInput::set_text` never emits | `ac_13_5_line_input_set_text_notifies_the_host` | `line_input.rs:76` emits `Changed`, so `utopia://search` filters on the text it put in the box. |

Left as it was, deliberately:

- `sync_from_store` still does not rewrite a live block's text while composing, and a
  remote edit to the *same* live block is superseded by the composition the user commits.
  That is the editor's existing last-writer-wins block write (`set_block_text`), not this
  machine; changing it means concurrent-edit merging, which the PRD puts on the CRDT side.
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
  can be *abandoned*; here it never is — the text the IME had marked is the text it
  committed when it lets go, and while composing no other edit can exist in the buffer, so
  there is nothing to restore to.
- `commit_block` alone was not enough: the wipe also came from `begin`'s `set_text`
  (clicking another block), and `end` takes the target away before the IME's commit lands.
  Both defer now.
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
| Composition | `field.rs:60` `composition: Composition` | `replace_and_mark_text_in_range :396` → `begin_composition :89` | `replace_and_mark_text_in_range` with empty text, `replace :142` (the IME's commit), `set_text :101`, `unmark_text :366` → `end_composition :95` |
| Selection | `field.rs:58-59` `sel`, `reversed` | IME sets at `:441-442` | – |
| Store copy of the text | via `persist` (`editor.rs:626`) | every `after_edit` | – |
| Persist gate | `editor.rs:578` `after_edit` returns early while composing | – | – |
| Store→field gate | `editor.rs:385` `sync_from_store` defers while composing | – | – |
| Queued host request | `editor.rs:257` `deferred: Option<Deferred>` | `defer :602` | `after_composition :613` |
| Queued store read | `editor.rs:257` `pending_sync: bool` | `sync_from_store :388` | `after_edit :582` |

Read outside: only through `EntityInputHandler` (`field.rs:357, 366, 371, 396`) and editor
render (`editor.rs:1519`).

## States

- **Not composing**: `Composition::None`; store text == field text after the last
  `persist`.
- **Composing**: `Composition::Active { range }`; field text contains provisional glyphs
  the store does not, and the editor holds any host request that would touch them in
  `deferred` (plus `pending_sync` if the store moved).

## Events and transitions

- `replace_and_mark_text_in_range` with text → Composing (no persist).
- `replace_and_mark_text_in_range` with empty text → Not composing (`after_edit` runs).
- `replace_text_in_range` / `unmark_text` → Not composing, then `after_edit` persists and
  runs the deferred request.
- `commit_block` / `end` / `begin` while composing → stays Composing, queues the request.
- A store change while composing → stays Composing, sets `pending_sync`.

## Impossible or unhandled combinations (evidence)

The five the old flags allowed, now closed (tests above; numbers are the doc's):

1. **Host-driven commit mid-composition.** `commit_block` had no guard and read
   `self.field.text` (provisional glyphs included) through `expand_relative_dates`, then
   wrote the older stored text back with `set_text`, which cleared `marked` while the
   platform still composed. Closed by the deferral.
2. **Action handlers clear `marked` silently.** `backspace`/`delete`/`paste`/`cut`/
   `soft_break`/`insert_text` replaced the selection and persisted. Closed by the guards;
   Backspace during composition is the IME's own backspace, which arrives as
   `replace_and_mark_text_in_range`.
3. **`mark()` mutates text under a stale range.** Closed by the guard.
4. **A remote/undo change arriving mid-composition was dropped.** Closed by
   `pending_sync`.
5. **`LineInput` was silent for `set_text`.** Closed by emitting `Changed`.

What remains possible: the platform can still send IME events after we stopped composing
only if something outside this machine ends the composition without the IME — the block
editor has no such path left, but `LineInput::set_text` (see above) and any future caller
of `TextField::set_text`/`replace` that skips `composing()` would recreate
`NotComposingDetached`. That is the residual risk of keeping the check at the call site
(see Status for why the type cannot enforce it).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> NotComposing
    NotComposing --> Composing : replace_and_mark_text_in_range (begin_composition)
    Composing --> Composing : more marked text
    Composing --> NotComposing : replace_text_in_range | unmark_text -> after_edit persists
    Composing --> Composing : host commit_block / end / begin -> Deferred (queued)
    Composing --> Composing : store change -> pending_sync
    NotComposing --> NotComposing : after_composition runs Deferred::Commit / End / Begin
```

`NotComposingDetached` — our composition ended while the OS composition was still live — is
now unreachable from the block editor's own paths; the remaining way in is a caller that
writes `TextField` without asking `composing()` (`LineInput::set_text` today).
