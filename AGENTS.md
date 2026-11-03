# AGENTS.md — working on Trellis

Trellis is a local-first, Roam-style outliner for the desktop, written in Rust with
a GPUI front end, a SQLite store, and Loro CRDT documents. **`PRD.md` is the spec.**
Read the section you are touching before changing behaviour, and cite requirement
ids (`FR-2.5`, `AC-4.6`, `§10.3.1`) in tests and doc comments.

## Environment setup (macOS)

```bash
source ~/.cargo/env                                             # rustup is not on PATH by default
export DEVELOPER_DIR=/Applications/Xcode-27.2.0.app/Contents/Developer
```

- GPUI is built with the `runtime_shaders` feature, so the Metal toolchain
  (`xcodebuild -downloadComponent MetalToolchain`) is **not** needed. Don't remove that feature.
- `rusqlite` uses the `bundled` feature (SQLite + FTS5 compiled in). Don't link the system SQLite.
  On macOS `trellis-store` adds `bundled-sqlcipher` (encryption at rest; its crypto is bundled,
  no OpenSSL). Elsewhere encryption reports unavailable (`encryption_available()`).
- The first build compiles GPUI and takes several minutes; run long builds in the background.
- Disk is tight on this machine: build with `CARGO_INCREMENTAL=0`, and if `target/debug` balloons
  (stale copies of the GPUI and Loro rlibs pile up when features change) delete it and rebuild.

## Commands (definition of done)

A change is finished only when all of these pass from the repo root:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run the app with `cargo run -p trellis-app` (binary name `trellis`). The graph is chosen in
this order: `--graph <file>`, `TRELLIS_GRAPH`, the last graph opened (recorded in the config
dir), the only `*.trellis` in `~/Documents/Trellis`; otherwise onboarding asks for a name (§9.3).
`TRELLIS_CONFIG_DIR` overrides the config dir (`keymap.json`, `last_graph`); point both env vars
at a scratch folder when trying the app so you never touch the user's real graph.

Headless commands (`cargo run -p trellis-app -- <cmd>`): `doctor <file> [--rebuild]` checks
the §10.4 invariants (exit 1 on problems); `export <file> <dir>` writes Markdown (attachments in
`blobs/`), `graph.json` and `graph.opml`; `import-roam <file> <roam.json>` imports a Roam export;
`capture <text…>` appends to today's note; `encrypt` / `decrypt <file>` switch encryption at rest
(`TRELLIS_DB_PASSPHRASE`, `TRELLIS_DB_NEW_PASSPHRASE`, keychain service `trellis-db`, or a
prompt); `serve-api <file> [addr]` runs the automation API without a window; a
`trellis://…` argument opens that link.

**Looking at the UI.** `TRELLIS_SHOT_SCRIPT` drives the real window and captures it to PNG (an
app may capture its own window without Screen Recording permission), e.g.

```bash
TRELLIS_CONFIG_DIR=/tmp/t TRELLIS_SHOT_SCRIPT="type Met [[Ana]]; keys escape; wait 300; shot /tmp/t/a.png; keys cmd-k; type ana; shot /tmp/t/b.png" \
  cargo run -p trellis-app -- --graph /tmp/t/g.trellis
```

Steps (`;` or newline separated): `keys <k…>`, `type <text>` (lines are trimmed; a second space
after `type` keeps a leading one), `wait <ms>`, `shot <png>`, `drop <file>` (as from Finder),
`ws open-pdf` / `ws highlight <page> <x> <y> <w> <h>` (area) / `ws select <page> <x0> <y0> <x1> <y1>`
(text selection). Then read the PNG. Do this for any visible
change: it has caught bugs the geometry tests could not.

UI behaviour is tested without a screen: `crates/trellis-ui/tests/workspace.rs` drives the real
`Workspace` through GPUI's test platform (`gpui` `test-support` dev-feature) with simulated
keystrokes and asserts on store state; `crates/trellis-ui/tests/layout.rs` asserts layout
geometry via `debug_selector` bounds. Add a script there for any new interaction or region.

Performance budgets (§15.1 and the timed ACs) — run in release, fails on any miss:

```bash
cargo build --release -p trellis-app            # needed for --ui
cargo run -p xtask --release -- bench --ui      # --ui opens a real window for < 1 s
```

`TRELLIS_PROBE=1` makes the app print its first-frame time and keystroke→frame cost, then quit.

Automation (P2): `TRELLIS_API=1` (or `=127.0.0.1:<port>`) starts the loopback HTTP API with the
app and writes `{url, token}` to `<config>/api.json` (mode 0600). Routes are documented in
`crates/trellis-api/src/lib.rs`. `cargo run -p xtask -- bundle` builds `target/Trellis.app`, whose
Info.plist claims the `trellis://` scheme (registering it with LaunchServices happens when the
bundle is opened, so don't open it on a user's machine without asking).

Plugins (P2): directories under `<config>/plugins/<name>/` with `plugin.json` + a `.wasm`/`.wat`
module; commands appear in the slash menu. See `crates/trellis-plugin/src/lib.rs` for the ABI and
`examples/plugins/word-count` for a complete one.

Mobile (§14): `cargo run -p xtask -- mobile` builds `trellis-ffi`, generates Swift bindings
(UniFFI) into `target/mobile/gen`, runs `mobile/ios/Check` against the real core, builds the
SwiftUI shell for macOS (`target/mobile/TrellisMobile-macos`, `TRELLIS_MOBILE_GRAPH`,
`TRELLIS_TODAY`, `TRELLIS_SHOT=<png>`), type-checks it for the iOS Simulator SDK, and generates
Kotlin bindings (`target/mobile/kotlin`) for an Android shell. `--ios` also
links the simulator app; it needs `rustup target add aarch64-apple-ios-sim` and an installed
iOS Simulator runtime (neither is present here; both are large downloads, so ask first).

Sync (§12; secrets never go into the graph file, §15.6):
- Relay: `cargo run -p trellis-syncd` (env `TRELLIS_SYNCD_ADDR`, `TRELLIS_SYNCD_DB`,
  `TRELLIS_SYNCD_TOKENS`, `TRELLIS_SYNCD_QUOTA_MB`); realtime notices on `/v1/graphs/{gid}/events`.
- App: `TRELLIS_SYNC_URL=<origin>` (or `lan` to discover over mDNS), or `TRELLIS_SYNC_FOLDER=<dir>`
  for folder sync; `TRELLIS_LAN_SERVE=0.0.0.0:8787` hosts and advertises a relay from the app.
  `TRELLIS_SYNC_GRAPH`, `TRELLIS_SYNC_TOKEN`, `TRELLIS_SYNC_PASSPHRASE` (end-to-end encryption)
  — token and passphrase may also come from the macOS keychain (services `trellis-sync`,
  `trellis-sync-passphrase`, account = graph id).

## Workspace layout (§13.1)

| Crate | Responsibility | May depend on |
|---|---|---|
| `crates/trellis-core` | Domain: ids, title normalization, dates, inline markdown parser, ref extraction, page tree model, `Command`, `DocDelta`, undo stack, markdown import/export | std + small pure crates. **Never** `loro`, `rusqlite`, `gpui` |
| `crates/trellis-doc` | The CRDT boundary: the `PageDoc` trait and its Loro implementation (one shard per page) | `trellis-core`, `loro` |
| `crates/trellis-store` | SQLite: schema/migrations, projections (`block`, `ref`, `block_fts`, `attr`), resolver, backlinks, rename/merge, trash, outbox, export, `doctor`, query evaluation, page graph, attachments (`<graph>.blobs/<sha256>`), PDF highlights, encryption at rest | `trellis-core`, `trellis-doc`, `rusqlite` |
| `crates/trellis-proto` | Sync wire format v1 (§12.3), shared by client and relay | `serde` only |
| `crates/trellis-sync` | Sync client: `Transport` trait, `SyncEngine` (prepare → `exchange` → apply), `HttpTransport`, `FolderTransport`, `e2e::Encrypted`, `realtime::listen`, in-memory reference relay | `trellis-core`, `trellis-doc`, `trellis-store`, `trellis-proto` |
| `crates/trellis-syncd` | Relay server (axum, §12.8): per-page logs, dedupe, auth, quota, snapshots, WebSocket notices; `lan` = in-process relay + mDNS. **No CRDT dependency** | `trellis-proto`, `rusqlite` |
| `crates/trellis-api` | Local automation API (loopback HTTP, bearer token) and `trellis://` link parsing; `handle(store, Op)` is the one implementation shared with plugins | `trellis-core`, `trellis-store`, `axum` |
| `crates/trellis-plugin` | WASM plugin sandbox (wasmi: fuel, memory cap, permissioned API calls) | `trellis-api`, `trellis-store`, `wasmi` |
| `crates/trellis-ui` | GPUI components: block editor (IME via `EntityInputHandler`), outline view, chips, panels, sidebar, background sync runner, graph view, PDF viewer (PDFKit on macOS) | `trellis-core`, `trellis-store`, `trellis-sync`, `trellis-plugin`, `gpui` |
| `crates/trellis-app` | Binary: window, keymap, actions, app assembly, CLI subcommands, startup probe, screenshot scripts, API/link inbox, unlock screen | everything above |
| `crates/trellis-ffi` | Mobile core: UniFFI surface over the store and sync (§14) | `trellis-store`, `trellis-sync`, `uniffi` |
| `mobile/ios` | SwiftUI shell (`TrellisMobile`) and the Swift binding check | the generated bindings |
| `xtask` | Dev tasks: `bench` (budgets), `profile-merge`, `bundle`, `mobile` | anything |

Dependency direction is one-way: `app → ui → (plugin → api) → store → doc → core`;
`ffi → sync → store`. If you find yourself importing upward, move the type down instead.

## Architecture rules (do not break these)

1. **The block is the atom (§2.3).** Every mutation of page content is a `trellis_core::Command`
   applied through `PageDoc::apply`. There is no other write path to block content.
2. **CRDT doc is the source of truth; SQLite rows are projections (§11.1).** `block`, `ref`
   and `block_fts` must always be rebuildable from `page_doc` (+ `page_update`) shards. Mutate
   through `Store::execute` / `execute_multi` / `execute_plan`; commands apply to the in-memory
   shards at once and each commit (shard delta, outbox row, every projection) is **one SQLite
   transaction** on the writer thread (`writer.rs`, §13.3). `doctor` asserts "incremental
   projection == full rebuild"; keep that test green.
   - *Strict* mode (default; tests, CLI): commits ship immediately and index reads wait for them.
   - *Debounced* mode (the UI, `AppState`): plain text edits flush after 250 ms idle / 1 s max,
     structural and identity edits ship at once, index reads never wait; the UI re-queries on
     `IndexUpdated`. Anything that reads the index to decide what to write (create, rename,
     merge, commit_block, import) calls `sync_index()` first.
   - All write transactions are `BEGIN IMMEDIATE` (a deferred upgrade fails under WAL).
3. **Resolution happens at index time, never at paint time (§10.3.1).** The renderer reads stored
   resolutions. The resolver order is frozen: unambiguous date → title_key → alias → unresolved.
4. **Identity is frozen (§10.3).** Pages created from a title use `uuidv5(NS_PAGE, "title:" + key)`;
   daily pages use `uuidv5(NS_DAILY, "daily:" + YYYY-MM-DD)`; blocks use ULIDs. If the computed
   uuidv5 is tombstoned, issue a fresh ULID — never resurrect. `normalize_title` (casefold +
   NFKC + whitespace collapse) is versioned: changing it is a migration, not a refactor.
5. **Never lose a character (§2.3.7).** Deleting a block tombstones it (`deleted_at`) and keeps its
   id so `((refs))` render "deleted block". Hard delete exists only for empty-trash.
6. **Dates are pages.** Daily titles are display-only; identity is `daily_date`. Ambiguous numeric
   dates (`11/6/26`) never resolve to a date (§5.5).
7. **Hashtags are page links (FR-2.15).** `#tag`, `#[[Multi Word]]` and `[[tag]]` produce identical
   edges; there is no tags subsystem.
8. **Rename onto an existing title merges (§8.6)** in one transaction and one undo step.
9. **Local-only UI state** (collapse, sidebar width, scroll, caret) never goes into a CRDT shard.
10. **No blocking I/O in GPUI render paths.** Keep `render` cheap; do store work in actions/handlers.
    Network I/O never runs on the UI thread (`sync_runner.rs` exchanges on a background thread).
11. **No panics on user data.** Unknown keymap actions, malformed text, missing blocks: log and
    degrade (e.g. "Missing block" chip). `unwrap()` is only acceptable on invariants that are
    proven locally, with a comment.
12. **Blobs are content-addressed and verified (§10.3).** Attachments are written once under
    their SHA-256 and rejected on sync if the bytes don't hash to the name. Blocks reference them
    only through markup (`[name](blob:<sha256>)`, `![…]`, `{{pdf: …}}`, `{{highlight: …}}`).
13. **Every API, plugin and FFI write goes through the same command path** as the editor
    (`trellis_api::handle` / `Store` methods), so undo, references and sync behave identically.
14. **Schema changes are migrations** (`schema.rs`, `PRAGMA user_version`), never edits to an
    existing version: graphs on disk must keep opening (§14.4). Check FK-heavy changes against
    `xtask bench`: SQLite cannot use partial indexes for foreign-key checks. Current: v7
   (v6 `attr`, v7 `attachment`).

## Conventions

- Rust 2021 edition, workspace-level dependency versions in the root `Cargo.toml`
  (`[workspace.dependencies]`). Pin GPUI exactly (`=0.2.2`, risk R1).
- Errors: `thiserror` enums in library crates (`DomainError`, `StoreError`), `anyhow` only in
  `trellis-app`.
- Timestamps: Unix ms UTC (`i64`). Ordering uses fractional positions, never timestamps (§10.7).
- Text: UTF-8, NFC-normalized on commit; reference spans are byte offsets into the NFC text;
  cursor movement is grapheme-aware (`unicode-segmentation`).
- Tests live next to the code (`#[cfg(test)] mod tests`) for unit behaviour, and in
  `crates/*/tests/` for acceptance scripts. Name acceptance tests after the PRD id, e.g.
  `ac_4_6_merge_preserves_subtrees`. Prefer real SQLite (in-memory or tempfile) over mocks.
- Keep comments sparse and about *why*; the PRD already explains *what*.

## Scope

Implemented:
- **v1 / P0 (§4.2):** daily notes, outliner, backlinks (linked references, "Scheduled for this
  day"), reference nodes (page/block refs, transclusion, inspector, unresolved intents),
  tags-as-pages, date links, rename/merge, right sidebar, local persistence, undo/redo, quick
  jump, Markdown/JSON export, rolling backups.
- **v1.1 / P1 (§4.2, §4.1 v1.1 column):** full-text search UI (Cmd+Shift+F), block types
  (to-do, H1–H3, quote, code, numbered; Alt+Enter cycles to-dos), slash menu, unlinked references
  + one-click Link, co-reference and date filters, attributes + page metadata header, aliases UI,
  journal and page templates (`templates/<name>` pages), extract block to page, drag across
  pages, block embeds (`{{embed: ((uid))}}`), quick capture (in-app and system-wide hotkey,
  plus `trellis capture`), split panes (Cmd+Shift+Click), Roam JSON import, OPML export.
- **Sync (§11.7, §12):** HTTP relay and client, background sync with status and merge notices,
  snapshot compaction, Loro-cursor caret preservation, end-to-end encryption (Argon2id +
  XChaCha20-Poly1305), realtime WebSocket notices, folder transport, LAN relay with mDNS.
- **v1.2+ / P2 (§4.2, §16):** query blocks (`{{query: …}}`, Roam syntax, SQL-backed, `/Query`)
  with queryable `key:: value` attributes; graph view (Cmd+Shift+G, force-directed, local/global,
  journal toggle); attachments (drop files or `/Attach a file`, image previews, file chips,
  synced and E2E-encrypted, exported); PDF viewer with rectangle highlights stored as quote
  blocks (macOS, PDFKit); WASM plugin runtime; local HTTP automation API + `trellis://` links;
  SQLCipher encryption at rest with an unlock screen; mobile core (`trellis-ffi`) and SwiftUI
  shell (§14.3 scope).

Known limits (pick these up deliberately, don't paper over them):
- Keystroke→frame in `bench --ui` measures input handling + full render/layout/paint; it does
  not include GPU present or waiting for the display refresh.
- The first launch after a fresh build takes ~1 s (binary page-in plus runtime Metal shader
  compilation, because the Metal toolchain is not installed); later launches draw in ~150 ms.
- Screenshots come from the app capturing its own window (`TRELLIS_SHOT_SCRIPT`); colours and
  typography are checked by eye, not by tests.
- Mobile has not run on an iOS device or simulator here (no iOS runtime or Rust iOS target is
  installed); the Swift bindings run against the real core on macOS and the SwiftUI shell
  type-checks against the iOS SDK. Android: Kotlin bindings generate; no Compose shell (the PRD
  ships iOS first, §14.2, M4) and no kotlinc here to compile them.
- PDF highlights select text the way a reader does (per-line rectangles + quoted text); Option-drag
  highlights an area instead. New highlights go under the block the PDF was opened from. PDF
  viewing needs macOS (elsewhere PDFs open in the system viewer).
- Needs the user's permission, so not done here: installing `aarch64-apple-ios-sim` and an iOS
  Simulator runtime (to run the mobile app), and opening `target/Trellis.app` (registers the
  `trellis://` handler with LaunchServices). Both were refused by the sandbox in this session.
- Plugins run in an interpreter (wasmi): fine for commands, slow for heavy computation.
- Query `{attr: …}` matches the attribute block itself, not the page or parent that owns it.
- LAN direct uses the HTTP relay protocol over TCP, not QUIC as sketched in §12.7.4.
- The folder transport re-reads every file of a changed page (imports are idempotent) and
  never compacts, because it must not delete other devices' files.
- Very large flat sibling lists (≫5k blocks under one parent) make each insert O(n).

Beyond the PRD's scope ladder there is nothing scheduled; new feature requests go through the
PRD first (risk R8).

## Working agreement for agents

- Before editing a crate, read its `lib.rs` module list and the PRD section it implements.
- After any change: build, test, clippy, fmt (see above). Never leave the workspace not compiling.
- When the PRD is ambiguous, pick the reading that preserves data and identity, note the choice in a
  doc comment with the PRD section, and mention it in your summary.
- Do not commit, push, or create branches unless the user asks.
