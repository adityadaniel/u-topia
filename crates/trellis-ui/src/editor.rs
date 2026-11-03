//! The block editor (§6.2, §13.5): one focused block owns the caret. Text edits persist
//! on every keystroke (coalesced into one undo step per 500 ms), the block is committed on
//! blur, Enter or 300 ms idle (AC-3.2), and outliner gestures are emitted as
//! [`EditorEvent`]s for the hosting view, which knows the visible row order.

use std::time::Duration;

use gpui::{
    anchored, deferred, div, prelude::*, px, ClipboardItem, Context, Entity, EntityId,
    EventEmitter, FocusHandle, Focusable, HighlightStyle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyledText, Subscription, Task,
    UnderlineStyle, Window,
};
use trellis_core::inline::{is_tag_char, page_link_text};
use trellis_core::title::normalize_title;
use trellis_core::undo::Caret;
use trellis_core::{BlockId, BlockKind, PageId};

use crate::actions::*;
use crate::app_state::AppState;
use crate::field::{FieldElement, FieldHost, Goal, TextField};
use crate::rich::source_highlights;
use crate::theme::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    /// The view whose row hosts the editor (the same block can be visible twice).
    pub host: EntityId,
    pub page: PageId,
    pub block: BlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorEvent {
    Split {
        caret: usize,
    },
    InsertAbove,
    Indent,
    Outdent,
    MergeBackward,
    Navigate {
        up: bool,
        x: Option<Pixels>,
    },
    MoveBlock {
        up: bool,
    },
    ToggleCollapse,
    ToggleCollapseAll,
    ZoomIn,
    ZoomOut,
    DeleteBlock,
    PasteOutline(String),
    /// Slash menu: extract the block to a page (asks for a title).
    ExtractRequested,
    /// Slash menu: insert a page template after the block.
    InsertTemplate(PageId),
    /// After undo/redo: where the caret should go.
    Restore(Caret),
    Blurred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcKind {
    /// `[[query`
    Page,
    /// `#[[query`
    BracketTag,
    /// `#query`
    Tag,
    /// `((query`
    Block,
    /// `/query`: the slash command menu (P1).
    Slash,
}

/// Slash menu commands (§4.2 P1: block types, templates, extract, embeds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCmd {
    Kind(BlockKind),
    Today,
    Embed,
    Query,
    Attach,
    TagPhrase,
    Extract,
    Template(PageId),
    /// `(plugin index, command id)` (P2 plugin runtime).
    Plugin(usize, String),
}

/// The static part of the slash menu.
pub fn slash_commands() -> Vec<(SlashCmd, &'static str)> {
    vec![
        (SlashCmd::Kind(BlockKind::Todo), "To-do"),
        (SlashCmd::Kind(BlockKind::Heading(1)), "Heading 1"),
        (SlashCmd::Kind(BlockKind::Heading(2)), "Heading 2"),
        (SlashCmd::Kind(BlockKind::Heading(3)), "Heading 3"),
        (SlashCmd::Kind(BlockKind::Quote), "Quote"),
        (SlashCmd::Kind(BlockKind::Code), "Code block"),
        (SlashCmd::Kind(BlockKind::Numbered), "Numbered list"),
        (SlashCmd::Kind(BlockKind::Bullet), "Bullet (plain)"),
        (SlashCmd::Today, "Today's date"),
        (SlashCmd::Embed, "Embed a block"),
        (SlashCmd::Query, "Query"),
        (SlashCmd::Attach, "Attach a file"),
        (SlashCmd::TagPhrase, "Tag the whole phrase"),
        (SlashCmd::Extract, "Extract block to page"),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcItem {
    Page {
        title: String,
        alias: Option<String>,
    },
    Create(String),
    Command {
        cmd: SlashCmd,
        label: String,
    },
    Block {
        uid: String,
        text: String,
        page_title: String,
    },
}

#[derive(Debug, Clone)]
pub struct Autocomplete {
    pub kind: AcKind,
    pub start: usize,
    pub query: String,
    pub items: Vec<AcItem>,
    pub selected: usize,
}

/// The reference trigger the caret is inside, if any (FR-2.17).
pub fn detect_trigger(text: &str, cursor: usize) -> Option<(AcKind, usize, String)> {
    let before = text.get(..cursor)?;
    if let Some(i) = before.rfind("[[") {
        let q = &before[i + 2..];
        if !q.contains("]]") && !q.contains('\n') {
            return Some(if before[..i].ends_with('#') {
                (AcKind::BracketTag, i - 1, q.to_string())
            } else {
                (AcKind::Page, i, q.to_string())
            });
        }
    }
    if let Some(i) = before.rfind("((") {
        let q = &before[i + 2..];
        if !q.contains("))") && !q.contains('\n') {
            return Some((AcKind::Block, i, q.to_string()));
        }
    }
    if let Some(i) = before.rfind('#') {
        let q = &before[i + 1..];
        let boundary = before[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '#');
        if boundary && q.chars().all(is_tag_char) {
            return Some((AcKind::Tag, i, q.to_string()));
        }
    }
    if let Some(i) = before.rfind('/') {
        let q = &before[i + 1..];
        let boundary = before[..i]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if boundary
            && q.len() <= 24
            && q.chars().all(|c| c.is_alphanumeric() || c == ' ')
            && !q.ends_with("  ")
        {
            return Some((AcKind::Slash, i, q.to_string()));
        }
    }
    None
}

/// Text inserted for an autocomplete choice, and how far past the caret an already
/// typed closing delimiter extends the replaced range.
pub fn completion_text(kind: AcKind, item: &AcItem, after_caret: &str) -> (String, usize) {
    let title = match item {
        AcItem::Page { title, .. } | AcItem::Create(title) => title.clone(),
        AcItem::Block { uid, .. } => {
            let skip = if after_caret.starts_with("))") { 2 } else { 0 };
            return (format!("(({uid}))"), skip);
        }
        AcItem::Command { .. } => return (String::new(), 0),
    };
    match kind {
        AcKind::Page => (
            page_link_text(&title, false),
            if after_caret.starts_with("]]") { 2 } else { 0 },
        ),
        AcKind::BracketTag => (
            format!("#[[{title}]]"),
            if after_caret.starts_with("]]") { 2 } else { 0 },
        ),
        AcKind::Tag => (page_link_text(&title, true), 0),
        AcKind::Block | AcKind::Slash => (format!("[[{title}]]"), 0),
    }
}

pub struct BlockEditor {
    app: Entity<AppState>,
    field: TextField,
    focus: FocusHandle,
    pub target: Option<EditTarget>,
    pub ac: Option<Autocomplete>,
    ac_dismissed: Option<usize>,
    /// The caret anchored in the text CRDT, so remote edits don't move it (§6.5).
    anchor: Option<trellis_store::DocCursor>,
    idle: Option<Task<()>>,
    edits: u64,
    _sub: Subscription,
}

impl EventEmitter<EditorEvent> for BlockEditor {}

impl Focusable for BlockEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FieldHost for BlockEditor {
    fn field(&self) -> &TextField {
        &self.field
    }
    fn field_mut(&mut self) -> &mut TextField {
        &mut self.field
    }
    fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus
    }
}

crate::impl_field_input_handler!(BlockEditor, BlockEditor::after_edit);

impl BlockEditor {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&app, |this, _, cx| this.sync_from_store(cx));
        Self {
            app,
            field: TextField::new(true),
            focus: cx.focus_handle(),
            target: None,
            ac: None,
            ac_dismissed: None,
            anchor: None,
            idle: None,
            edits: 0,
            _sub: sub,
        }
    }

    pub fn is_editing(&self, host: EntityId, page: &PageId, block: &BlockId) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| t.host == host && t.page == *page && t.block == *block)
    }

    pub fn cursor(&self) -> usize {
        self.field.cursor()
    }

    pub fn selection(&self) -> std::ops::Range<usize> {
        self.field.sel.clone()
    }

    /// Start editing `target`, committing the previous block first.
    pub fn begin(
        &mut self,
        target: EditTarget,
        caret: Option<usize>,
        goal: Option<Goal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target.as_ref() != Some(&target) {
            self.commit(cx);
            let text = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&target.page)
                    .ok()
                    .and_then(|st| st.block(&target.block).map(|b| b.text.clone()))
            });
            let Some(text) = text else { return };
            self.field.set_text(&text, caret.unwrap_or(text.len()));
            self.target = Some(target);
            self.ac = None;
            self.ac_dismissed = None;
        } else if let Some(c) = caret {
            self.field.move_to(c);
        }
        self.field.goal = goal;
        self.anchor_caret(cx);
        window.focus(&self.focus);
        cx.notify();
    }

    /// Stop editing (Escape, navigation). Commits the block. Emits `Blurred` so the
    /// workspace can take focus back if nothing else claims it.
    pub fn end(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);
        let was_editing = self.target.take().is_some();
        self.ac = None;
        if was_editing {
            cx.emit(EditorEvent::Blurred);
        }
        cx.notify();
    }

    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        if self.field.marked.is_some() {
            return;
        }
        let text = self.app.update(cx, |app, _| {
            app.store.page_state(&t.page).ok().and_then(|st| {
                st.block(&t.block)
                    .filter(|b| b.live)
                    .map(|b| b.text.clone())
            })
        });
        match text {
            Some(text) if text != self.field.text => {
                // Changed underneath us (undo, rewrite, sync): keep the caret in range.
                let anchored = self.anchor.clone().and_then(|a| {
                    self.app
                        .update(cx, |app, _| app.store.resolve_caret(&t.page, &a))
                });
                let c = anchored.unwrap_or_else(|| self.field.cursor().min(text.len()));
                self.field.set_text(&text, c);
                self.anchor_caret(cx);
                cx.notify();
            }
            Some(_) => {}
            None => {
                // The block went away (undo of its creation, deletion, merge).
                self.target = None;
                self.ac = None;
                cx.emit(EditorEvent::Blurred);
                cx.notify();
            }
        }
    }

    fn anchor_caret(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else {
            self.anchor = None;
            return;
        };
        let caret = self.field.cursor();
        self.anchor = self.app.update(cx, |app, _| {
            app.store.caret_cursor(&t.page, &t.block, caret)
        });
    }

    /// Run a plugin command on the block being edited and apply what it returns.
    pub fn run_plugin(
        &mut self,
        index: usize,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.target.clone() else { return };
        let input = serde_json::json!({
            "page": t.page.as_str(),
            "block": t.block.as_str(),
            "text": self.field.text,
        });
        let result = self.app.update(cx, |app, cx| {
            let plugin = app.plugins.get(index).cloned()?;
            let r = plugin.run(&mut app.store, command, input);
            match &r {
                Ok(out) => {
                    if !out.changed.is_empty() {
                        app.changed(out.changed.clone(), true, cx);
                    }
                    app.notice = out.message.clone();
                }
                Err(e) => app.last_error = Some(format!("{}: {e}", plugin.manifest.name)),
            }
            r.ok()
        });
        let Some(out) = result else {
            cx.notify();
            return;
        };
        if let Some(text) = out.replace {
            let len = text.len();
            self.field.set_text(&text, len);
            self.after_edit(window, cx);
        } else if out.changed.contains(&t.page) {
            // The plugin edited this page through the API: show the stored text.
            let stored = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&t.page)
                    .ok()
                    .and_then(|st| st.block(&t.block).map(|b| b.text.clone()))
            });
            if let Some(s) = stored.filter(|s| *s != self.field.text) {
                let len = s.len();
                self.field.set_text(&s, len);
            }
        }
        if let Some(text) = out.insert {
            self.insert_text(&text, window, cx);
        }
        cx.notify();
    }

    /// Type `text` at the caret (replacing any selection). `false` when not editing.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.target.is_none() {
            return false;
        }
        let sel = self.field.sel.clone();
        let start = sel.start.min(sel.end);
        self.field.replace(sel, text);
        self.field.move_to(start + text.len());
        self.after_edit(window, cx);
        cx.notify();
        true
    }

    /// Copy files into the graph's blob store and insert their markup at the caret
    /// (P2 attachments). Returns whether anything was inserted.
    pub fn attach_paths(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let markup: Vec<String> = self.app.update(cx, |app, _| {
            paths
                .iter()
                .filter_map(|p| {
                    let r = app.store.attach_file(p);
                    app.report(r)
                })
                .map(|a| trellis_core::attach::markup(&a.name, &a.hash, &a.mime))
                .collect()
        });
        !markup.is_empty() && self.insert_text(&markup.join(" "), window, cx)
    }

    fn after_edit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.field.marked.is_some() {
            return;
        }
        self.persist(cx);
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        let text = self.field.text.clone();
        let caret = self.field.cursor();
        let changed = self.app.update(cx, |app, cx| {
            let r = app
                .store
                .set_block_text(&t.page, &t.block, &text, None, Some(caret));
            let changed = app.report(r).unwrap_or(false);
            if changed {
                app.changed(vec![t.page.clone()], false, cx);
            }
            changed
        });
        if changed {
            self.edits += 1;
            let seen = self.edits;
            self.idle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.edits == seen {
                        this.commit(cx);
                    }
                });
            }));
        }
    }

    /// Commit the current block: NFC, hashtag longest-match, page creation for new links.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        self.idle = None;
        self.expand_relative_dates(&t, cx);
        let before = self.field.text.clone();
        let caret = self.field.cursor();
        let after = self.app.update(cx, |app, cx| {
            let r = app.store.commit_block(&t.page, &t.block);
            app.report(r);
            app.store.seal_undo();
            app.changed(vec![t.page.clone()], true, cx);
            app.store
                .page_state(&t.page)
                .ok()
                .and_then(|st| st.block(&t.block).map(|b| b.text.clone()))
        });
        if let Some(after) = after.filter(|a| *a != before) {
            let shift = after.len() as isize - before.len() as isize;
            let c = if caret >= before.len() {
                after.len()
            } else {
                (caret as isize + shift.min(0)).max(0) as usize
            };
            self.field.set_text(&after, c);
            cx.notify();
        }
    }

    /// `[[yesterday]]`, `[[next sunday]]` → a link to that day's page, once the link is
    /// closed and the caret has left it. The phrase is replaced, not resolved later,
    /// because its meaning changes every day (§10.3.1).
    fn expand_relative_dates(&mut self, t: &EditTarget, cx: &mut Context<Self>) {
        let caret = self.field.cursor();
        let expanded = self.app.update(cx, |app, cx| {
            let store = &app.store;
            let (text, caret) = trellis_core::date::expand_relative_date_links(
                &self.field.text,
                caret,
                store.today(),
                store.date_format(),
                |title| store.find_page(title).ok().flatten().is_some(),
            )?;
            let r = app
                .store
                .set_block_text(&t.page, &t.block, &text, None, Some(caret));
            app.report(r)?;
            app.changed(vec![t.page.clone()], false, cx);
            Some((text, caret))
        });
        if let Some((text, caret)) = expanded {
            self.field.set_text(&text, caret);
            cx.notify();
        }
    }

    fn refresh_autocomplete(&mut self, cx: &mut Context<Self>) {
        let cursor = self.field.cursor();
        let Some((kind, start, query)) = detect_trigger(&self.field.text, cursor) else {
            self.ac = None;
            self.ac_dismissed = None;
            return;
        };
        if self.ac_dismissed == Some(start) {
            return;
        }
        let own = self.target.as_ref().map(|t| t.block.clone());
        let items = self.app.update(cx, |app, _| -> Vec<AcItem> {
            match kind {
                AcKind::Slash => {
                    let q = query.trim().to_lowercase();
                    let mut all: Vec<(SlashCmd, String)> = slash_commands()
                        .into_iter()
                        .map(|(c, l)| (c, l.to_string()))
                        .collect();
                    for (id, name) in app.store.template_pages().unwrap_or_default() {
                        all.push((SlashCmd::Template(id), format!("Template: {name}")));
                    }
                    for (i, p) in app.plugins.iter().enumerate() {
                        for c in &p.manifest.commands {
                            all.push((
                                SlashCmd::Plugin(i, c.id.clone()),
                                format!("{}: {}", p.manifest.name, c.title),
                            ));
                        }
                    }
                    all.into_iter()
                        .filter(|(_, l)| q.is_empty() || l.to_lowercase().contains(&q))
                        .map(|(cmd, label)| AcItem::Command { cmd, label })
                        .collect()
                }
                AcKind::Block => app
                    .store
                    .search_blocks(&query, 8)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|h| Some(&h.block) != own.as_ref())
                    .map(|h| AcItem::Block {
                        uid: h.block.0,
                        text: trellis_core::inline::plain_text(&h.text),
                        page_title: h.page_title,
                    })
                    .collect(),
                _ => {
                    let hits = app.store.search_titles(&query, 8).unwrap_or_default();
                    let key = normalize_title(&query);
                    let exact = hits.iter().any(|h| {
                        normalize_title(&h.title) == key
                            || h.alias.as_deref().map(normalize_title) == Some(key.clone())
                    });
                    let mut items: Vec<AcItem> = hits
                        .into_iter()
                        .map(|h| AcItem::Page {
                            title: h.title,
                            alias: h.alias,
                        })
                        .collect();
                    // `[[tomorrow` offers tomorrow's page first (after an exact title
                    // match, so a page really called "Today" still wins).
                    if let Some(d) = trellis_core::date::relative_date(&query, app.store.today()) {
                        let title = app.store.date_format().format(d);
                        items
                            .retain(|i| !matches!(i, AcItem::Page { title: t, .. } if *t == title));
                        let at = usize::from(exact);
                        items.insert(at.min(items.len()), AcItem::Page { title, alias: None });
                    }
                    // A bare `#tag` is already a complete link (created on commit), so it
                    // offers no "Create" row; Enter then splits the block as usual.
                    if !key.is_empty() && !exact && kind != AcKind::Tag {
                        items.push(AcItem::Create(query.trim().to_string()));
                    }
                    items
                }
            }
        });
        let selected = self
            .ac
            .as_ref()
            .filter(|a| a.start == start)
            .map_or(0, |a| a.selected.min(items.len().saturating_sub(1)));
        self.ac = Some(Autocomplete {
            kind,
            start,
            query,
            items,
            selected,
        });
        cx.notify();
    }

    /// Accept the selected suggestion; `create` forces "create page from query"
    /// (Cmd+Enter, FR-2.17).
    fn accept(
        &mut self,
        create: bool,
        ix: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ac) = self.ac.take() else { return };
        let item = if create {
            if ac.kind == AcKind::Block || ac.query.trim().is_empty() {
                self.ac = Some(ac);
                return;
            }
            AcItem::Create(ac.query.trim().to_string())
        } else {
            match ac.items.get(ix.unwrap_or(ac.selected)) {
                Some(i) => i.clone(),
                None => return,
            }
        };
        if let AcItem::Create(title) = &item {
            self.app.update(cx, |app, cx| {
                let r = app.store.create_page(title);
                app.report(r);
                app.changed(Vec::new(), true, cx);
            });
        }
        let cursor = self.field.cursor();
        if let AcItem::Command { cmd, .. } = item {
            self.field.replace(ac.start..cursor, "");
            self.after_edit(window, cx);
            self.ac = None;
            return self.run_slash(cmd, window, cx);
        }
        let (insert, skip) = completion_text(ac.kind, &item, &self.field.text[cursor..]);
        self.field.replace(ac.start..cursor + skip, &insert);
        // An accepted `#tag` still looks like a trigger; don't reopen on it.
        self.ac_dismissed = Some(ac.start);
        self.after_edit(window, cx);
        self.ac = None;
        cx.notify();
    }

    fn run_slash(&mut self, cmd: SlashCmd, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        match cmd {
            SlashCmd::Kind(kind) => self.set_kind(&t, kind, false, cx),
            SlashCmd::Today => {
                let link = self.app.read_with(cx, |app, _| {
                    let s = &app.store;
                    page_link_text(&s.date_format().format(s.today()), false)
                });
                let sel = self.field.sel.clone();
                self.field.replace(sel, &link);
                self.after_edit(window, cx);
            }
            SlashCmd::Embed => {
                let c = self.field.cursor();
                self.field.replace(c..c, "{{embed: (())}}");
                self.field.move_to(c + "{{embed: ((".len());
                self.after_edit(window, cx);
            }
            SlashCmd::Query => {
                let c = self.field.cursor();
                self.field.replace(c..c, "{{query: {and: }}}");
                self.field.move_to(c + "{{query: {and: ".len());
                self.after_edit(window, cx);
            }
            SlashCmd::Attach => {
                let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Attach".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picked.await {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.attach_paths(&paths, window, cx)
                        });
                    }
                })
                .detach();
            }
            SlashCmd::TagPhrase => {
                let c = self.field.cursor();
                let tag = trellis_core::inline::extract_refs(&self.field.text)
                    .into_iter()
                    .rfind(|r| {
                        r.kind == trellis_core::inline::RefKind::Tag
                            && r.range.end <= c
                            && !self.field.text[r.range.clone()].starts_with("#[[")
                    });
                if let Some(r) = tag {
                    if let Some((range, rep)) =
                        trellis_core::inline::whole_phrase_tag(&self.field.text, r.range.start)
                    {
                        let end = range.start + rep.len();
                        self.field.replace(range, &rep);
                        self.field.move_to(end);
                        self.after_edit(window, cx);
                    }
                }
            }
            SlashCmd::Extract => self.emit_after_commit(EditorEvent::ExtractRequested, cx),
            SlashCmd::Template(id) => self.emit_after_commit(EditorEvent::InsertTemplate(id), cx),
            SlashCmd::Plugin(i, command) => self.run_plugin(i, &command, window, cx),
        }
        cx.notify();
    }

    fn set_kind(&mut self, t: &EditTarget, kind: BlockKind, checked: bool, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let r = app.store.set_block_kind(&t.page, &t.block, kind, checked);
            app.report(r);
            app.changed(vec![t.page.clone()], true, cx);
        });
    }

    /// Alt+Enter: bullet → to-do → done → bullet.
    fn toggle_todo(&mut self, _: &ToggleTodo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        let current = self.app.update(cx, |app, _| {
            app.store
                .page_state(&t.page)
                .ok()
                .and_then(|st| st.block(&t.block).map(|b| (b.kind, b.checked)))
        });
        let next = match current {
            Some((BlockKind::Todo, false)) => (BlockKind::Todo, true),
            Some((BlockKind::Todo, true)) => (BlockKind::Bullet, false),
            _ => (BlockKind::Todo, false),
        };
        self.set_kind(&t, next.0, next.1, cx);
    }

    fn emit_after_commit(&mut self, e: EditorEvent, cx: &mut Context<Self>) {
        self.commit(cx);
        cx.emit(e);
    }

    // ---------------------------------------------------------------- actions

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.ac.as_ref().is_some_and(|a| !a.items.is_empty()) {
            return self.accept(false, None, window, cx);
        }
        let caret = self.field.cursor();
        self.emit_after_commit(EditorEvent::Split { caret }, cx);
    }

    fn insert_above(&mut self, _: &InsertAbove, window: &mut Window, cx: &mut Context<Self>) {
        if self.ac.is_some() {
            return self.accept(true, None, window, cx);
        }
        self.emit_after_commit(EditorEvent::InsertAbove, cx);
    }

    fn soft_break(&mut self, _: &SoftBreak, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.field.sel.clone();
        self.field.replace(sel, "\n");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.ac = None;
        cx.emit(EditorEvent::Indent);
    }

    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.ac = None;
        cx.emit(EditorEvent::Outdent);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.sel.is_empty() {
            if self.field.cursor() == 0 {
                self.ac = None;
                return self.emit_after_commit(EditorEvent::MergeBackward, cx);
            }
            let to = self.field.prev_boundary();
            self.field.select_to(to);
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, "");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.sel.is_empty() {
            let to = self.field.next_boundary();
            self.field.select_to(to);
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, "");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.prev_boundary()
        } else {
            self.field.sel.start
        };
        self.field.move_to(to);
        self.moved(cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.next_boundary()
        } else {
            self.field.sel.end
        };
        self.field.move_to(to);
        self.moved(cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.prev_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.next_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        let len = self.field.text.len();
        self.field.select_to(len);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        self.moved(cx);
    }

    fn end_(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(len);
        self.moved(cx);
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.line_start();
        self.field.move_to(to);
        self.moved(cx);
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.line_end();
        self.field.move_to(to);
        self.moved(cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = trellis_core::text::prev_word_start(&self.field.text, self.field.cursor());
        self.field.move_to(to);
        self.moved(cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = trellis_core::text::next_word_end(&self.field.text, self.field.cursor());
        self.field.move_to(to);
        self.moved(cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, cx);
    }

    fn vertical(&mut self, up: bool, cx: &mut Context<Self>) {
        if let Some(ac) = &mut self.ac {
            if !ac.items.is_empty() {
                let n = ac.items.len();
                ac.selected = if up {
                    (ac.selected + n - 1) % n
                } else {
                    (ac.selected + 1) % n
                };
                cx.notify();
                return;
            }
        }
        match self.field.vertical(up, None) {
            Some(ix) => {
                self.field.move_to(ix);
                self.moved(cx);
            }
            None => {
                let x = self.field.caret_x();
                self.emit_after_commit(EditorEvent::Navigate { up, x }, cx);
            }
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.vertical(true, None).unwrap_or(0);
        self.field.select_to(to);
        cx.notify();
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        let to = self.field.vertical(false, None).unwrap_or(len);
        self.field.select_to(to);
        cx.notify();
    }

    fn move_up(&mut self, _: &MoveBlockUp, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::MoveBlock { up: true }, cx);
    }

    fn move_down(&mut self, _: &MoveBlockDown, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::MoveBlock { up: false }, cx);
    }

    fn toggle_collapse(&mut self, _: &ToggleCollapse, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::ToggleCollapse);
    }

    fn toggle_collapse_all(
        &mut self,
        _: &ToggleCollapseAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(EditorEvent::ToggleCollapseAll);
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::ZoomIn, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::ZoomOut, cx);
    }

    fn mark(&mut self, delim: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (text, sel) =
            trellis_core::text::toggle_mark(&self.field.text, self.field.sel.clone(), delim);
        self.field.text = text;
        self.field.sel = sel;
        self.field.reversed = false;
        self.after_edit(window, cx);
        cx.notify();
    }

    fn bold(&mut self, _: &Bold, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("**", window, cx);
    }

    fn italic(&mut self, _: &Italic, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("_", window, cx);
    }

    fn code(&mut self, _: &Code, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("`", window, cx);
    }

    /// Copy: the selection, or with nothing selected the block's subtree as nested
    /// Markdown (FR-2.11).
    fn copy_text(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.field.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.field.selected_text().to_string(),
            ));
            return true;
        }
        let Some(t) = self.target.clone() else {
            return false;
        };
        let md = self.app.update(cx, |app, _| {
            app.store
                .page_state(&t.page)
                .ok()
                .map(|st| trellis_core::markdown::subtree_to_markdown(st, &t.block))
        });
        if let Some(md) = md {
            cx.write_to_clipboard(ClipboardItem::new_string(md));
        }
        false
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_text(cx);
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_text(cx) {
            let sel = self.field.sel.clone();
            self.field.replace(sel, "");
            self.after_edit(window, cx);
            cx.notify();
        } else {
            self.emit_after_commit(EditorEvent::DeleteBlock, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        if text.trim_end_matches('\n').contains('\n') {
            return self.emit_after_commit(EditorEvent::PasteOutline(text), cx);
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, text.trim_end_matches('\n'));
        self.after_edit(window, cx);
        cx.notify();
    }

    fn copy_block_ref(&mut self, _: &CopyBlockRef, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = &self.target {
            cx.write_to_clipboard(ClipboardItem::new_string(format!("(({}))", t.block)));
        }
    }

    fn delete_block(&mut self, _: &DeleteBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::DeleteBlock, cx);
    }

    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ac) = self.ac.take() {
            self.ac_dismissed = Some(ac.start);
            cx.notify();
            return;
        }
        self.end(cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(true, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(false, cx);
    }

    fn history(&mut self, undo: bool, cx: &mut Context<Self>) {
        self.commit(cx);
        let caret = self.app.update(cx, |app, cx| {
            let r = if undo {
                app.store.undo()
            } else {
                app.store.redo()
            };
            let caret = app.report(r).flatten();
            app.changed(Vec::new(), true, cx);
            caret
        });
        self.sync_from_store(cx);
        if let Some(c) = caret {
            cx.emit(EditorEvent::Restore(c));
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.field.index_at(e.position);
        if e.click_count >= 2 {
            let r = trellis_core::text::word_range_at(&self.field.text, ix);
            self.field.move_to(r.start);
            self.field.select_to(r.end);
        } else if e.modifiers.shift {
            self.field.select_to(ix);
        } else {
            self.field.move_to(ix);
        }
        self.field.selecting = true;
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.field.selecting {
            let ix = self.field.index_at(e.position);
            self.field.select_to(ix);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.field.selecting = false;
    }

    fn render_autocomplete(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ac = self.ac.as_ref()?;
        if ac.items.is_empty() {
            return None;
        }
        let th = theme(cx).clone();
        let rows = ac.items.iter().enumerate().map(|(i, item)| {
            let (label, detail): (SharedString, Option<SharedString>) = match item {
                AcItem::Page { title, alias } => (
                    title.clone().into(),
                    alias.as_ref().map(|a| format!("alias: {a}").into()),
                ),
                AcItem::Create(t) => (format!("Create \"{t}\"   ⌘↵").into(), None),
                AcItem::Command { label, .. } => (format!("/ {label}").into(), None),
                AcItem::Block {
                    text, page_title, ..
                } => (
                    text.chars().take(60).collect::<String>().into(),
                    Some(page_title.clone().into()),
                ),
            };
            div()
                .id(("ac", i))
                .px_2()
                .py_1()
                .rounded_sm()
                .when(i == ac.selected, |d| d.bg(th.hover))
                .child(div().text_color(th.text).child(label))
                .when_some(detail, |d, s| {
                    d.child(div().text_xs().text_color(th.muted).child(s))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.accept(false, Some(i), window, cx);
                    }),
                )
        });
        Some(
            deferred(
                anchored().child(
                    div()
                        .occlude()
                        .debug_selector(|| "autocomplete".into())
                        .mt_1()
                        .w(px(340.))
                        .p_1()
                        .bg(th.panel_bg)
                        .border_1()
                        .border_color(th.border)
                        .rounded_md()
                        .shadow_md()
                        .children(rows),
                ),
            )
            .with_priority(1),
        )
    }
}

impl Render for BlockEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let text = self.field.text.clone();
        let (styled, shown) = if text.is_empty() {
            let hint = "Type, or [[ to link, # to tag, (( to reference";
            (
                StyledText::new(hint).with_highlights([(
                    0..hint.len(),
                    HighlightStyle {
                        color: Some(th.faint),
                        ..Default::default()
                    },
                )]),
                hint.to_string(),
            )
        } else {
            let mut hl = source_highlights(&text, &th);
            if let Some(m) = &self.field.marked {
                hl.retain(|(r, _)| r.end <= m.start || r.start >= m.end);
                hl.push((
                    m.clone(),
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(th.text),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                hl.sort_by_key(|(r, _)| r.start);
            }
            (StyledText::new(text.clone()).with_highlights(hl), text)
        };
        div()
            .key_context(EDITOR_CONTEXT)
            .debug_selector(|| "block-editor".into())
            .track_focus(&self.focus)
            .w_full()
            .min_h(px(crate::theme::LINE_HEIGHT))
            .cursor_text()
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::insert_above))
            .on_action(cx.listener(Self::soft_break))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end_))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::toggle_collapse))
            .on_action(cx.listener(Self::toggle_collapse_all))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::italic))
            .on_action(cx.listener(Self::code))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy_block_ref))
            .on_action(cx.listener(Self::delete_block))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::toggle_todo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(FieldElement::new(cx.entity(), styled, shown))
            .children(self.render_autocomplete(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers() {
        assert_eq!(
            detect_trigger("see [[Alp", 9),
            Some((AcKind::Page, 4, "Alp".into()))
        );
        assert_eq!(
            detect_trigger("#[[Modern R", 11),
            Some((AcKind::BracketTag, 0, "Modern R".into()))
        );
        assert_eq!(
            detect_trigger("x #min", 6),
            Some((AcKind::Tag, 2, "min".into()))
        );
        assert_eq!(
            detect_trigger("x #", 3),
            Some((AcKind::Tag, 2, String::new()))
        );
        assert_eq!(
            detect_trigger("((thes", 6),
            Some((AcKind::Block, 0, "thes".into()))
        );
        assert_eq!(detect_trigger("[[A]] done", 10), None);
        assert_eq!(detect_trigger("C#", 2), None);
        assert_eq!(detect_trigger("#a b", 4), None);
        assert_eq!(
            detect_trigger("/hea", 4),
            Some((AcKind::Slash, 0, "hea".into()))
        );
        assert_eq!(
            detect_trigger("x /", 3),
            Some((AcKind::Slash, 2, String::new()))
        );
        assert_eq!(detect_trigger("http://a", 8), None, "urls are not commands");
        assert_eq!(detect_trigger("a/b", 3), None);
    }

    #[test]
    fn completions_are_canonical() {
        let page = |t: &str| AcItem::Page {
            title: t.into(),
            alias: None,
        };
        assert_eq!(
            completion_text(AcKind::Page, &page("Alpha"), "]] x"),
            ("[[Alpha]]".into(), 2)
        );
        assert_eq!(
            completion_text(AcKind::Tag, &page("Modern Religion"), ""),
            ("#[[Modern Religion]]".into(), 0)
        );
        assert_eq!(
            completion_text(AcKind::Tag, &page("mind"), ""),
            ("#mind".into(), 0)
        );
        assert_eq!(
            completion_text(AcKind::BracketTag, &page("A B"), ""),
            ("#[[A B]]".into(), 0)
        );
        let blk = AcItem::Block {
            uid: "U1".into(),
            text: String::new(),
            page_title: String::new(),
        };
        assert_eq!(
            completion_text(AcKind::Block, &blk, "))"),
            ("((U1))".into(), 2)
        );
    }
}
