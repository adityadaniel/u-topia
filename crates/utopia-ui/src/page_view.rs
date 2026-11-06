//! A page (or zoomed block) as a virtualized outline plus its Linked References
//! (§6, §7, §8). Used for the main pane and for every right-sidebar pane.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use gpui::{
    anchored, deferred, div, list, point, prelude::*, px, AnyElement, Context, Corner, Entity,
    EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, MouseButton, MouseDownEvent,
    SharedString, StyledText, Subscription, Window,
};
use utopia_core::inline::{extract_refs, plain_text};
use utopia_core::outline::{self, Focus, Plan};
use utopia_core::undo::Caret;
use utopia_core::{BlockId, BlockKind, PageId, PageKind};
use utopia_store::{BlockRefStatus, LinkedRefs, Mention, PageRow, RefHit, RefQuery, RefSort};

use crate::app_state::{AppState, Changed, IndexUpdated};
use crate::editor::{BlockEditor, EditTarget, EditorEvent};
use crate::field::Goal;
use crate::line_input::{LineInput, LineInputEvent};
use crate::rich::{self, ChipState, LinkTarget};
use crate::theme::{quiet_control, quiet_danger, quiet_menu, theme, INDENT, MAX_VISUAL_DEPTH};

/// Transclusions stop expanding at this depth (FR-4.5).
const MAX_EMBED_DEPTH: u8 = 3;
/// A query block lists at most this many results.
const MAX_QUERY_HITS: usize = 50;
/// An embed renders at most this many blocks (§13.4).
const MAX_EMBED_BLOCKS: usize = 100;
/// Groups show this many references before "+N more" (FR-3.2).
const GROUP_PREVIEW: usize = 3;
/// The Daily Notes feed loads this many more days whenever its end comes into view.
const JOURNAL_BATCH: usize = 10;
/// Widest the page column gets (about 95 characters of body text per line).
const READABLE_WIDTH: f32 = 760.;

#[derive(Debug, Clone)]
pub enum PageViewEvent {
    Open {
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        sidebar: bool,
    },
    Close,
    /// Cmd+Shift+Click: open beside the main pane (P1 split panes).
    OpenSplit {
        page: PageId,
    },
    /// Slash "Extract block to page": the workspace asks for a title (FR-2.18).
    RequestExtract {
        page: PageId,
        block: BlockId,
    },
    /// A `{{pdf: blob:…}}` attachment was clicked (P2 PDF viewer).
    OpenPdf {
        hash: String,
        name: String,
        /// 0-based page to show first.
        page: Option<usize>,
        /// The block the PDF was opened from; new highlights go under it.
        host: Option<BlockId>,
    },
}

#[derive(Debug, Clone)]
pub struct RowItem {
    pub page: PageId,
    pub block: BlockId,
    pub text: String,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
    pub embed: u8,
    pub kind: BlockKind,
    pub checked: bool,
    /// 1-based position among live siblings, for numbered blocks.
    pub number: usize,
}

#[derive(Debug, Clone)]
enum Item {
    Header,
    Row(RowItem),
    Notice {
        depth: usize,
        text: SharedString,
    },
    /// Results of a `{{query: …}}` block (P2), under the block.
    QueryHeader {
        depth: usize,
        label: SharedString,
    },
    QueryHit {
        depth: usize,
        hit: utopia_store::QueryHit,
    },
    /// An attachment referenced by the row above (P2).
    Attachment {
        depth: usize,
        blob: utopia_core::attach::BlobRef,
        path: Option<std::path::PathBuf>,
        size: Option<u64>,
        /// This device has the bytes (in-memory graphs have no path).
        present: bool,
        /// The block whose text attaches it.
        host: BlockId,
    },
    Empty,
    RefsHeader {
        title: SharedString,
        count: usize,
    },
    RefsControls,
    GroupHeader {
        page: PageRow,
        count: usize,
        scheduled: bool,
    },
    Hit(RefHit),
    More {
        page: PageId,
        hidden: usize,
    },
    NoRefs,
    UnlinkedHeader {
        count: Option<usize>,
        open: bool,
    },
    Mention(Mention),
    /// A day's title in the Daily Notes feed (`page` is None for a blank day).
    DayHeader {
        date: NaiveDate,
        page: Option<PageId>,
        title: SharedString,
        /// "Thursday · Today" above the date (DN-2).
        caption: SharedString,
        /// Today's caption is in the accent colour (DN-2).
        today: bool,
        first: bool,
    },
    /// A day in the feed with no blocks.
    DayEmpty {
        date: NaiveDate,
    },
    /// The end of the loaded days; older ones load when it comes into view.
    JournalEnd,
}

/// One calendar day of the feed while it is being built.
struct FeedDay {
    date: NaiveDate,
    page: Option<PageId>,
    title: String,
    rows: Vec<Item>,
}

/// The Daily Notes feed: today, then every older calendar day, as one editable outline.
#[derive(Debug, Clone)]
struct Journal {
    /// How many days to show; grows as the end of the feed comes into view.
    limit: usize,
    days: Vec<PageId>,
    loading: bool,
    /// Each day's Linked References, valid for one `refs_version`.
    refs: HashMap<PageId, LinkedRefs>,
    refs_version: u64,
}

#[derive(Debug, Clone, Default)]
struct Header {
    title: String,
    kind: PageKind,
    date: Option<NaiveDate>,
    aliases: Vec<String>,
    deleted: bool,
    missing: bool,
    /// A day that is shown but not written yet (FR-1.2): nothing to delete.
    draft: bool,
    zoom_path: Vec<(BlockId, String)>,
    /// `Key:: value` root blocks: the page metadata header (P1, W7).
    attributes: Vec<(String, String)>,
}

#[derive(Clone)]
struct DraggedBlock {
    page: PageId,
    block: BlockId,
    label: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .px_2()
            .py_1()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .text_color(th.text)
            .child(self.0.clone())
    }
}

pub struct PageView {
    app: Entity<AppState>,
    editor: Entity<BlockEditor>,
    pub page: Option<PageId>,
    pub zoom: Option<BlockId>,
    pub compact: bool,
    items: Vec<Item>,
    list: ListState,
    header: Header,
    refs: LinkedRefs,
    refs_key: Option<(PageId, Option<BlockId>, u64)>,
    query: RefQuery,
    suggestions: Vec<(PageId, String)>,
    filter_titles: BTreeMap<PageId, String>,
    expanded_groups: HashSet<PageId>,
    open_embeds: HashSet<(BlockId, String)>,
    rename: Option<Entity<LineInput>>,
    filter: Entity<LineInput>,
    show_calendar: bool,
    unlinked_open: bool,
    unlinked: Option<(u64, Vec<Mention>)>,
    alias_input: Option<Entity<LineInput>>,
    calendar_month: NaiveDate,
    /// Set while showing the Daily Notes feed; `page` is then today's page.
    journal: Option<Journal>,
    /// The page header's "···" menu is open (ALL-3).
    page_menu: bool,
    dirty: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PageViewEvent> for PageView {}

impl PageView {
    pub fn new(
        app: Entity<AppState>,
        editor: Entity<BlockEditor>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| LineInput::new("Filter references…", cx));
        let subs = vec![
            cx.subscribe(&app, |this, _, e: &Changed, cx| {
                let shown = this.page.as_ref().is_some_and(|p| e.pages.contains(p))
                    || this
                        .journal
                        .as_ref()
                        .is_some_and(|j| j.days.iter().any(|d| e.pages.contains(d)));
                if e.structural || e.pages.is_empty() || shown || this.has_embeds() {
                    this.dirty = true;
                    cx.notify();
                }
            }),
            cx.subscribe(&app, |this, _, _: &IndexUpdated, cx| {
                this.dirty = true;
                cx.notify();
            }),
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.subscribe(&filter, |this, _, e: &LineInputEvent, cx| {
                if let LineInputEvent::Changed(t) = e {
                    this.query.text = (!t.trim().is_empty()).then(|| t.clone());
                    this.refs_key = None;
                    this.dirty = true;
                    cx.notify();
                }
            }),
        ];
        let today = app.read(cx).store.today();
        Self {
            app,
            editor,
            page: None,
            zoom: None,
            compact,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            header: Header::default(),
            refs: LinkedRefs::default(),
            refs_key: None,
            query: RefQuery::default(),
            suggestions: Vec::new(),
            filter_titles: BTreeMap::new(),
            expanded_groups: HashSet::new(),
            open_embeds: HashSet::new(),
            rename: None,
            filter,
            show_calendar: false,
            unlinked_open: false,
            unlinked: None,
            alias_input: None,
            calendar_month: today,
            journal: None,
            page_menu: false,
            dirty: true,
            _subs: subs,
        }
    }

    fn has_embeds(&self) -> bool {
        !self.open_embeds.is_empty()
            || self
                .items
                .iter()
                .any(|i| matches!(i, Item::QueryHeader { .. }))
    }

    /// Show `page` (optionally zoomed into a block).
    pub fn open(&mut self, page: PageId, zoom: Option<BlockId>, cx: &mut Context<Self>) {
        self.journal = None;
        self.show(page, zoom, cx);
    }

    /// The Daily Notes feed (Roam's default view): `today` first, then every older journal
    /// page newest first, each editable in place. Days are never created by scrolling past
    /// them (FR-1.2).
    pub fn open_journal(&mut self, today: PageId, cx: &mut Context<Self>) {
        self.journal = Some(Journal {
            limit: JOURNAL_BATCH,
            days: Vec::new(),
            loading: false,
            refs: HashMap::new(),
            refs_version: 0,
        });
        self.page = None;
        self.show(today, None, cx);
    }

    pub fn is_journal(&self) -> bool {
        self.journal.is_some()
    }

    /// Days in the Daily Notes feed, newest first (empty outside the feed).
    pub fn journal_days(&self) -> Vec<PageId> {
        self.journal
            .as_ref()
            .map(|j| j.days.clone())
            .unwrap_or_default()
    }

    fn show(&mut self, page: PageId, zoom: Option<BlockId>, cx: &mut Context<Self>) {
        let changed_page = self.page.as_ref() != Some(&page);
        self.page = Some(page.clone());
        self.zoom = zoom;
        if changed_page {
            self.query = RefQuery::default();
            self.filter_titles.clear();
            self.rename = None;
            self.show_calendar = false;
            self.unlinked = None;
            self.alias_input = None;
            self.filter.update(cx, |f, cx| f.set_text("", cx));
            let saved = self
                .app
                .read(cx)
                .store
                .local_get(&format!("refs_expanded:{page}"))
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();
            self.expanded_groups = saved.into_iter().map(PageId).collect();
        }
        self.page_menu = false;
        self.refs_key = None;
        self.dirty = true;
        self.rebuild(cx);
        self.list.reset(self.items.len());
        cx.notify();
    }

    /// Start editing the first block (creating one on an empty page, §9.3).
    pub fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page.clone() else {
            return;
        };
        // `r.page == page`: in the Daily Notes feed the next rows belong to older days.
        let first = self.items.iter().find_map(|i| match i {
            Item::Row(r) if r.embed == 0 && r.page == page => Some(r.block.clone()),
            _ => None,
        });
        let block = match first {
            Some(b) => b,
            None => {
                let zoom = self.zoom.clone();
                let created = self.app.update(cx, |app, cx| {
                    let r = match &zoom {
                        Some(z) => {
                            let plan = app
                                .store
                                .page_state(&page)
                                .map(|st| outline::append_block(st, Some(z)));
                            plan.and_then(|p| app.store.execute_plan(&page, p, None))
                                .map(|f| f.map(|f| f.block))
                        }
                        None => app.store.ensure_first_block(&page).map(Some),
                    };
                    let b = app.report(r).flatten();
                    app.changed(vec![page.clone()], true, cx);
                    b
                });
                self.rebuild(cx);
                match created {
                    Some(b) => b,
                    None => return,
                }
            }
        };
        self.edit(page, block, Some(0), None, window, cx);
    }

    pub fn focus_block(
        &mut self,
        block: &BlockId,
        offset: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_if_dirty(cx);
        let row = self.items.iter().find_map(|i| match i {
            Item::Row(r) if r.block == *block => Some(r.page.clone()),
            _ => None,
        });
        if let Some(page) = row {
            self.edit(page, block.clone(), offset, None, window, cx);
        }
    }

    fn edit(
        &mut self,
        page: PageId,
        block: BlockId,
        caret: Option<usize>,
        goal: Option<Goal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_if_dirty(cx);
        if self.row_index(&block).is_none() {
            // Not editable here (e.g. the zoom root shown as the header): stop editing
            // rather than focus an editor no row renders.
            self.editor.update(cx, |e, cx| e.end(cx));
            return;
        }
        let host = cx.entity_id();
        self.editor.update(cx, |e, cx| {
            e.begin(
                EditTarget {
                    host,
                    page,
                    block: block.clone(),
                },
                caret,
                goal,
                window,
                cx,
            )
        });
        self.splice_items(self.items.len(), cx);
        if let Some(ix) = self.row_index(&block) {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    fn row_index(&self, block: &BlockId) -> Option<usize> {
        self.items
            .iter()
            .position(|i| matches!(i, Item::Row(r) if r.block == *block))
    }

    fn rebuild_if_dirty(&mut self, cx: &mut Context<Self>) {
        if self.dirty {
            self.rebuild(cx);
        }
    }

    /// Tell the list every item changed, keeping the scroll position (a rebuild after an
    /// edit, or older days loading into the feed, must not jump back to the top).
    fn replace_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        let old_len = self.items.len();
        self.items = items;
        self.splice_items(old_len, cx);
    }

    /// Re-register every item with the list. The row being edited carries the editor's
    /// focus handle, so the list keeps rendering it after it scrolls out of view: a
    /// focused element that is not rendered receives no keystrokes, and every shortcut
    /// (Cmd+[, Cmd+K, undo) would stop working until something was clicked.
    fn splice_items(&mut self, old_len: usize, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        let host = cx.entity_id();
        let editor = self.editor.read(cx);
        let handle = editor.focus_handle(cx);
        let handles: Vec<Option<FocusHandle>> = self
            .items
            .iter()
            .map(|i| match i {
                Item::Row(r) if editor.is_editing(host, &r.page, &r.block) => Some(handle.clone()),
                _ => None,
            })
            .collect();
        self.list.splice_focusable(0..old_len, handles);
        if top.item_ix > 0 || top.offset_in_item > px(0.) {
            self.list.scroll_to(top);
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let Some(page) = self.page.clone() else {
            self.replace_items(Vec::new(), cx);
            return;
        };
        if let Some(limit) = self.journal.as_ref().map(|j| j.limit) {
            self.rebuild_journal(page, limit, cx);
            return;
        }
        let zoom = self.zoom.clone();
        let open_embeds = self.open_embeds.clone();
        let refs_version = self.app.read(cx).refs_version;
        let need_refs = self.refs_key != Some((page.clone(), zoom.clone(), refs_version));
        let query = self.query.clone();
        let (header, rows, refs) = self.app.update(cx, |app, _| {
            let mut header = Header::default();
            let collapsed = app.store.collapsed_set().clone();
            let Ok(st) = app.store.page_state(&page).cloned() else {
                header.missing = true;
                return (header, Vec::new(), None);
            };
            // From the shard, not the index: the index may lag by one flush.
            header.title = utopia_store::display_title(&st, app.store.date_format());
            header.deleted = st.meta.deleted_at.is_some();
            header.kind = st.meta.kind;
            header.draft = app.store.is_draft(&page);
            header.attributes = st
                .live_roots()
                .iter()
                .filter_map(|r| {
                    let t = &st.block(r)?.text;
                    utopia_core::inline::attribute(t)
                        .map(|(k, v)| (k, utopia_core::inline::plain_text(&t[v])))
                })
                .collect();
            header.date = st.meta.daily_date;
            header.aliases = st.meta.aliases.clone();
            if let Some(z) = &zoom {
                for a in st
                    .ancestors(z)
                    .into_iter()
                    .chain(std::iter::once(z.clone()))
                {
                    let t = st
                        .block(&a)
                        .map(|b| plain_text(&b.text))
                        .unwrap_or_default();
                    header.zoom_path.push((a, t));
                }
            }
            let mut rows = Vec::new();
            for (id, depth) in st.visible_rows(zoom.as_ref(), &|b| collapsed.contains(b)) {
                let b = st.block(&id).expect("visible row exists");
                let item = RowItem {
                    page: page.clone(),
                    block: id.clone(),
                    text: b.text.clone(),
                    depth,
                    has_children: st.has_live_children(&id),
                    collapsed: collapsed.contains(&id),
                    embed: 0,
                    kind: b.kind,
                    checked: b.checked,
                    number: st.index_in_parent(&id, true).map_or(1, |i| i + 1),
                };
                push_with_embeds(
                    app,
                    item,
                    &open_embeds,
                    &collapsed,
                    &mut vec![id.clone()],
                    &mut rows,
                );
            }
            let refs = if need_refs {
                Some(match &zoom {
                    Some(z) => {
                        let hits = app.store.block_backlinks(z).unwrap_or_default();
                        let mut groups: Vec<utopia_store::RefGroup> = Vec::new();
                        for h in hits {
                            match groups.iter_mut().find(|g| g.page.id == h.page) {
                                Some(g) => g.hits.push(h),
                                None => {
                                    if let Ok(Some(p)) = app.store.page_row(&h.page) {
                                        groups.push(utopia_store::RefGroup {
                                            page: p,
                                            hits: vec![h],
                                        });
                                    }
                                }
                            }
                        }
                        let total = groups.iter().map(|g| g.hits.len()).sum();
                        LinkedRefs { total, groups }
                    }
                    None => app
                        .store
                        .linked_references(&page, &query)
                        .unwrap_or_default(),
                })
            } else {
                None
            };
            (header, rows, refs)
        });
        if let Some(refs) = refs {
            self.refs = refs;
            self.refs_key = Some((page.clone(), zoom.clone(), refs_version));
            self.suggestions = self.compute_suggestions(&page, cx);
        }
        self.header = header;
        let mut items = vec![Item::Header];
        if self.header.missing {
            self.replace_items(items, cx);
            return;
        }
        if rows.is_empty() {
            items.push(Item::Empty);
        }
        for r in rows {
            items.push(r);
        }
        self.push_refs(&page, &mut items);
        if self.zoom.is_none() {
            // Unlinked References (FR-3.9): computed only when expanded, like Roam.
            if self.unlinked_open
                && self
                    .unlinked
                    .as_ref()
                    .is_none_or(|(v, _)| *v != refs_version)
            {
                let found = self.app.update(cx, |app, _| {
                    let r = app.store.unlinked_references(&page);
                    app.report(r).unwrap_or_default()
                });
                self.unlinked = Some((refs_version, found));
            }
            let mentions = self
                .unlinked
                .as_ref()
                .map(|(_, m)| m.clone())
                .filter(|_| self.unlinked_open);
            items.push(Item::UnlinkedHeader {
                count: mentions.as_ref().map(Vec::len),
                open: self.unlinked_open,
            });
            for m in mentions.unwrap_or_default() {
                items.push(Item::Mention(m));
            }
        }
        self.replace_items(items, cx);
    }

    fn rebuild_journal(&mut self, today: PageId, limit: usize, cx: &mut Context<Self>) {
        let open_embeds = self.open_embeds.clone();
        let refs_version = self.app.read(cx).refs_version;
        let mut refs_cache = match &mut self.journal {
            Some(j) if j.refs_version == refs_version => std::mem::take(&mut j.refs),
            _ => HashMap::new(),
        };
        let (header, days, today_date) = self.app.update(cx, |app, _| {
            let collapsed = app.store.collapsed_set().clone();
            let fmt = app.store.date_format().clone();
            let today_date = app.store.today();
            let from = today_date
                .checked_sub_days(chrono::Days::new(limit.saturating_sub(1) as u64))
                .unwrap_or(today_date);
            let mut pages = app.store.days_between(from, today_date).unwrap_or_default();
            // Today comes from the caller (its draft), not the index, which may lag.
            pages.insert(today_date, today.clone());
            let mut header = Header {
                kind: PageKind::Daily,
                date: Some(today_date),
                ..Default::default()
            };
            let mut days = Vec::new();
            let mut date = today_date;
            loop {
                let mut day = FeedDay {
                    date,
                    page: None,
                    title: fmt.format(date),
                    rows: Vec::new(),
                };
                let st = pages
                    .get(&date)
                    .and_then(|p| Some((p.clone(), app.store.page_state(p).ok()?.clone())))
                    .filter(|(_, st)| st.meta.deleted_at.is_none());
                if let Some((page, st)) = st {
                    day.title = utopia_store::display_title(&st, &fmt);
                    for (id, depth) in st.visible_rows(None, &|b| collapsed.contains(b)) {
                        let b = st.block(&id).expect("visible row exists");
                        let item = RowItem {
                            page: page.clone(),
                            block: id.clone(),
                            text: b.text.clone(),
                            depth,
                            has_children: st.has_live_children(&id),
                            collapsed: collapsed.contains(&id),
                            embed: 0,
                            kind: b.kind,
                            checked: b.checked,
                            number: st.index_in_parent(&id, true).map_or(1, |i| i + 1),
                        };
                        push_with_embeds(
                            app,
                            item,
                            &open_embeds,
                            &collapsed,
                            &mut vec![id.clone()],
                            &mut day.rows,
                        );
                    }
                    if !refs_cache.contains_key(&page) {
                        let refs = app
                            .store
                            .linked_references(&page, &RefQuery::default())
                            .unwrap_or_default();
                        refs_cache.insert(page.clone(), refs);
                    }
                    day.page = Some(page);
                }
                if date == today_date {
                    header.title = day.title.clone();
                }
                days.push(day);
                match date.pred_opt() {
                    Some(d) if date > from => date = d,
                    _ => break,
                }
            }
            (header, days, today_date)
        });
        self.header = header;
        let mut items = Vec::new();
        let mut ids = Vec::new();
        for (i, day) in days.into_iter().enumerate() {
            items.push(Item::DayHeader {
                date: day.date,
                page: day.page.clone(),
                title: day.title.into(),
                caption: day_caption(day.date, today_date).into(),
                today: day.date == today_date,
                first: i == 0,
            });
            if day.rows.is_empty() {
                items.push(Item::DayEmpty { date: day.date });
            }
            items.extend(day.rows);
            let Some(page) = day.page else { continue };
            // Like Roam's feed: Linked References under the day when it has any, and no
            // Unlinked References (those stay on the day's own page).
            if let Some(refs) = refs_cache.get(&page).filter(|r| !r.groups.is_empty()) {
                items.push(Item::RefsHeader {
                    title: "Linked References".into(),
                    count: refs.shown(),
                });
                self.push_ref_groups(&page, refs, true, &mut items);
            }
            ids.push(page);
        }
        items.push(Item::JournalEnd);
        if let Some(j) = &mut self.journal {
            j.days = ids;
            j.loading = false;
            j.refs = refs_cache;
            j.refs_version = refs_version;
        }
        self.replace_items(items, cx);
    }

    /// Load the next batch of older days into the feed (once per batch).
    fn load_older_days(&mut self, cx: &mut Context<Self>) {
        if let Some(j) = &mut self.journal {
            if !j.loading {
                j.loading = true;
                j.limit += JOURNAL_BATCH;
                self.dirty = true;
                cx.notify();
            }
        }
    }

    /// Start writing on a feed day with nothing on it: a draft until the first keystroke
    /// (FR-1.2), or the first block of a page that so far only has references.
    fn start_day(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(date).and_then(|page| {
                let first = app.store.page_state(&page)?.live_roots().first().cloned();
                let block = match first {
                    Some(b) => b,
                    None => app.store.ensure_first_block(&page)?,
                };
                Ok((page, block))
            });
            let t = app.report(r);
            app.changed(Vec::new(), true, cx);
            t
        });
        self.rebuild(cx);
        if let Some((page, block)) = target {
            self.edit(page, block, Some(0), None, window, cx);
        }
    }

    fn push_refs(&self, page: &PageId, items: &mut Vec<Item>) {
        let shown = self.refs.shown();
        let title = if self.zoom.is_some() {
            "Referenced by"
        } else {
            "Linked References"
        };
        items.push(Item::RefsHeader {
            title: title.into(),
            count: shown,
        });
        if self.zoom.is_none() {
            items.push(Item::RefsControls);
        }
        if self.refs.groups.is_empty() {
            items.push(Item::NoRefs);
            return;
        }
        let daily = self.header.kind == PageKind::Daily;
        self.push_ref_groups(page, &self.refs, daily, items);
    }

    /// One header per referencing page, then its hits (folded past `GROUP_PREVIEW`).
    fn push_ref_groups(
        &self,
        page: &PageId,
        refs: &LinkedRefs,
        daily: bool,
        items: &mut Vec<Item>,
    ) {
        for g in &refs.groups {
            let scheduled = daily && g.page.id != *page;
            items.push(Item::GroupHeader {
                page: g.page.clone(),
                count: g.hits.len(),
                scheduled,
            });
            let limit = if self.expanded_groups.contains(&g.page.id) {
                usize::MAX
            } else {
                GROUP_PREVIEW
            };
            for h in g.hits.iter().take(limit) {
                items.push(Item::Hit(h.clone()));
            }
            if g.hits.len() > limit {
                items.push(Item::More {
                    page: g.page.id.clone(),
                    hidden: g.hits.len() - limit,
                });
            }
        }
    }

    /// Pages co-referenced by the current results, for "+ filter" chips (FR-3.10).
    fn compute_suggestions(&self, page: &PageId, cx: &mut Context<Self>) -> Vec<(PageId, String)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for g in &self.refs.groups {
            for h in &g.hits {
                for r in extract_refs(&h.text) {
                    if r.is_page_like() {
                        *counts.entry(r.target.clone()).or_default() += 1;
                    }
                }
            }
        }
        let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let store = &self.app.read(cx).store;
        let mut out: Vec<(PageId, String)> = Vec::new();
        for (target, _) in ranked {
            if let Ok(Some(id)) = store.find_page(&target) {
                if id != *page
                    && !self.query.also.contains(&id)
                    && !out.iter().any(|(p, _)| *p == id)
                {
                    let title = store
                        .page_row(&id)
                        .ok()
                        .flatten()
                        .map_or(target.clone(), |r| r.title);
                    out.push((id, title));
                }
            }
            if out.len() >= 6 {
                break;
            }
        }
        out
    }

    // ---------------------------------------------------------------- editor events

    fn on_editor_event(
        &mut self,
        editor: &Entity<BlockEditor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = editor.read(cx).target.clone();
        if let EditorEvent::Restore(caret) = event {
            self.restore(caret, window, cx);
            return;
        }
        let Some(t) = target else { return };
        if t.host != cx.entity_id() {
            return;
        }
        self.rebuild_if_dirty(cx);
        match event {
            EditorEvent::Split { caret } => {
                let collapsed = self.app.read(cx).store.is_collapsed(&t.block);
                self.run_plan(
                    &t.page,
                    |st| outline::split_block(st, &t.block, *caret, collapsed),
                    window,
                    cx,
                );
            }
            EditorEvent::InsertAbove => {
                self.run_plan(
                    &t.page,
                    |st| outline::insert_above(st, &t.block),
                    window,
                    cx,
                );
            }
            EditorEvent::Indent => {
                let caret = editor.read(cx).cursor();
                self.run_plan_keep(&t, caret, |st| outline::indent(st, &t.block), window, cx);
            }
            EditorEvent::Outdent => {
                let caret = editor.read(cx).cursor();
                let zoom = self.zoom.clone();
                // Never outdent out of the zoomed subtree.
                let at_zoom_root = self.app.update(cx, |app, _| {
                    app.store
                        .page_state(&t.page)
                        .ok()
                        .and_then(|st| st.parent_of(&t.block))
                        == zoom
                }) && zoom.is_some();
                if !at_zoom_root {
                    self.run_plan_keep(&t, caret, |st| outline::outdent(st, &t.block), window, cx);
                }
            }
            EditorEvent::MergeBackward => {
                let zoom = if t.page == *self.page.as_ref().unwrap_or(&t.page) {
                    self.zoom.clone()
                } else {
                    None
                };
                let collapsed = self.app.read(cx).store.collapsed_set().clone();
                let now = self.app.read(cx).store.now();
                self.run_plan(
                    &t.page,
                    |st| {
                        outline::merge_into_previous(
                            st,
                            &t.block,
                            zoom.as_ref(),
                            &|b| collapsed.contains(b),
                            now,
                        )
                    },
                    window,
                    cx,
                );
            }
            EditorEvent::Navigate { up, x } => {
                let Some(ix) = self.row_index(&t.block) else {
                    return;
                };
                let rows: Vec<(usize, &RowItem)> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| match it {
                        Item::Row(r) => Some((i, r)),
                        _ => None,
                    })
                    .collect();
                let pos = rows.iter().position(|(i, _)| *i == ix);
                let next = pos.and_then(|p| if *up { p.checked_sub(1) } else { Some(p + 1) });
                if let Some(r) = next.and_then(|n| rows.get(n)).map(|(_, r)| (*r).clone()) {
                    let x = x.unwrap_or(px(0.));
                    let goal = if *up {
                        Goal::LastLine(x)
                    } else {
                        Goal::FirstLine(x)
                    };
                    let caret = if *up { r.text.len() } else { 0 };
                    self.edit(r.page, r.block, Some(caret), Some(goal), window, cx);
                }
            }
            EditorEvent::MoveBlock { up } => {
                let caret = editor.read(cx).cursor();
                let up = *up;
                self.run_plan_keep(
                    &t,
                    caret,
                    |st| {
                        if up {
                            outline::move_up(st, &t.block)
                        } else {
                            outline::move_down(st, &t.block)
                        }
                    },
                    window,
                    cx,
                );
            }
            EditorEvent::ToggleCollapse => self.toggle_collapse(&t.block, cx),
            EditorEvent::ToggleCollapseAll => self.toggle_collapse_all(&t.page, cx),
            EditorEvent::ZoomIn => {
                self.editor.update(cx, |e, cx| e.end(cx));
                if Some(&t.page) == self.page.as_ref() {
                    self.open(t.page.clone(), Some(t.block.clone()), cx);
                    self.focus_first(window, cx);
                } else {
                    cx.emit(PageViewEvent::Open {
                        page: t.page.clone(),
                        zoom: Some(t.block.clone()),
                        focus: None,
                        sidebar: false,
                    });
                }
            }
            EditorEvent::ZoomOut => self.zoom_out(window, cx),
            EditorEvent::DeleteBlock => {
                let now = self.app.read(cx).store.now();
                self.run_plan(
                    &t.page,
                    |st| outline::delete_subtree(st, &t.block, now),
                    window,
                    cx,
                );
            }
            EditorEvent::PasteOutline(text) => {
                let sel = editor.read(cx).selection();
                self.run_plan(
                    &t.page,
                    |st| outline::paste(st, &t.block, sel.clone(), text),
                    window,
                    cx,
                );
            }
            EditorEvent::ExtractRequested => {
                self.editor.update(cx, |e, cx| e.end(cx));
                cx.emit(PageViewEvent::RequestExtract {
                    page: t.page.clone(),
                    block: t.block.clone(),
                });
            }
            EditorEvent::InsertTemplate(template) => {
                let first = self.app.update(cx, |app, cx| {
                    let parent = app
                        .store
                        .page_state(&t.page)
                        .ok()
                        .and_then(|st| st.parent_of(&t.block));
                    let r = app.store.apply_template(
                        &t.page,
                        parent.as_ref(),
                        Some(&t.block),
                        template,
                    );
                    let f = app.report(r).flatten();
                    app.changed(vec![t.page.clone()], true, cx);
                    f
                });
                self.rebuild(cx);
                if let Some(f) = first {
                    self.edit(t.page.clone(), f, None, None, window, cx);
                }
            }
            EditorEvent::Restore(_) | EditorEvent::Blurred => {}
        }
    }

    fn restore(&mut self, caret: &Caret, window: &mut Window, cx: &mut Context<Self>) {
        // The view that hosted the edit restores the caret; with no edit in progress the
        // main pane does.
        let mine = match &self.editor.read(cx).target {
            Some(t) => t.host == cx.entity_id(),
            None => !self.compact,
        };
        if mine {
            self.restore_caret(caret, window, cx);
        }
    }

    /// Put the caret back where an undo/redo says, if that block is shown here.
    pub fn restore_caret(&mut self, caret: &Caret, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild(cx);
        if self.row_index(&caret.block).is_some() {
            self.edit(
                caret.page.clone(),
                caret.block.clone(),
                Some(caret.offset),
                None,
                window,
                cx,
            );
        }
    }

    fn apply_plan(&mut self, page: &PageId, plan: Plan, cx: &mut Context<Self>) -> Option<Focus> {
        let caret_before = {
            let e = self.editor.read(cx);
            e.target.as_ref().map(|t| Caret {
                page: t.page.clone(),
                block: t.block.clone(),
                offset: e.cursor(),
            })
        };
        let page = page.clone();
        let focus = self.app.update(cx, |app, cx| {
            let r = app.store.execute_plan(&page, plan, caret_before);
            let f = app.report(r).flatten();
            app.changed(vec![page.clone()], true, cx);
            f
        });
        self.rebuild(cx);
        focus
    }

    fn run_plan(
        &mut self,
        page: &PageId,
        plan: impl FnOnce(&utopia_core::PageState) -> Result<Plan, utopia_core::DomainError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan = self
            .app
            .update(cx, |app, _| app.store.page_state(page).ok().map(plan));
        let Some(Ok(plan)) = plan else { return };
        if let Some(f) = self.apply_plan(page, plan, cx) {
            self.edit(page.clone(), f.block, Some(f.offset), None, window, cx);
        }
    }

    /// Run a plan and keep the caret where it was (indent, outdent, move).
    fn run_plan_keep(
        &mut self,
        t: &EditTarget,
        caret: usize,
        plan: impl FnOnce(&utopia_core::PageState) -> Result<Plan, utopia_core::DomainError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self
            .app
            .update(cx, |app, _| app.store.page_state(&t.page).ok().map(plan));
        let Some(Ok(plan)) = p else { return };
        self.apply_plan(&t.page, plan, cx);
        self.edit(
            t.page.clone(),
            t.block.clone(),
            Some(caret),
            None,
            window,
            cx,
        );
    }

    fn toggle_collapse(&mut self, block: &BlockId, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let c = app.store.is_collapsed(block);
            let r = app.store.set_collapsed(block, !c);
            app.report(r);
            app.changed(Vec::new(), false, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    fn toggle_collapse_all(&mut self, page: &PageId, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let Ok(st) = app.store.page_state(page).cloned() else {
                return;
            };
            let parents: Vec<BlockId> = st
                .live_preorder()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| st.has_live_children(id))
                .collect();
            let any_collapsed = parents.iter().any(|p| app.store.is_collapsed(p));
            for p in parents {
                let r = app.store.set_collapsed(&p, !any_collapsed);
                app.report(r);
            }
            app.changed(Vec::new(), false, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page.clone() else {
            return;
        };
        let Some(z) = self.zoom.clone() else { return };
        let parent = self.app.update(cx, |app, _| {
            app.store
                .page_state(&page)
                .ok()
                .and_then(|st| st.parent_of(&z))
        });
        self.editor.update(cx, |e, cx| e.end(cx));
        self.open(page, parent, cx);
        self.focus_block(&z, None, window, cx);
    }

    fn follow(
        &mut self,
        link: LinkTarget,
        row: Option<&RowItem>,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link {
            LinkTarget::Page(title) => {
                let page = self.app.update(cx, |app, cx| {
                    let found = app.store.find_page(&title).ok().flatten();
                    match found {
                        Some(p) => Some(p),
                        // An unresolved intent: following it creates the page (FR-4.14).
                        None => {
                            let r = app.store.create_page(&title);
                            let p = app.report(r);
                            app.changed(Vec::new(), true, cx);
                            p
                        }
                    }
                });
                if let Some(page) = page {
                    self.editor.update(cx, |e, cx| e.end(cx));
                    if sidebar && window.modifiers().platform {
                        cx.emit(PageViewEvent::OpenSplit { page });
                    } else {
                        cx.emit(PageViewEvent::Open {
                            page,
                            zoom: None,
                            focus: None,
                            sidebar,
                        });
                    }
                }
            }
            LinkTarget::Block(uid) => {
                if sidebar {
                    if let Ok(Some(b)) = self.app.read(cx).store.block_row(&BlockId(uid.clone())) {
                        cx.emit(PageViewEvent::Open {
                            page: b.page_id,
                            zoom: Some(b.id),
                            focus: None,
                            sidebar: true,
                        });
                    }
                } else if let Some(row) = row {
                    let key = (row.block.clone(), uid);
                    if !self.open_embeds.remove(&key) {
                        self.open_embeds.insert(key);
                    }
                    self.rebuild(cx);
                    cx.notify();
                }
            }
            LinkTarget::Url(url) if url.starts_with("pdfpage:") => {
                // `{{highlight: …}}`: open the PDF at the highlighted page.
                let rest = &url["pdfpage:".len()..];
                let (hash, page) = rest.split_once('#').unwrap_or((rest, "1"));
                let page = page.parse::<usize>().unwrap_or(1).saturating_sub(1);
                let name = self
                    .app
                    .read(cx)
                    .store
                    .attachment(hash)
                    .ok()
                    .flatten()
                    .map_or_else(|| "PDF".to_string(), |a| a.name);
                // From a highlight's "p. N": its parent is the block that holds the PDF.
                let host = row.and_then(|r| {
                    self.app
                        .read(cx)
                        .store
                        .block_row(&r.block)
                        .ok()
                        .flatten()
                        .and_then(|b| b.parent_id)
                });
                cx.emit(PageViewEvent::OpenPdf {
                    hash: hash.to_string(),
                    name,
                    page: Some(page),
                    host,
                });
            }
            LinkTarget::Url(url) => match url.strip_prefix("blob:") {
                // Attachments open locally (PDFs in the viewer), never as a URL.
                Some(hash) => {
                    let store = &self.app.read(cx).store;
                    let pdf = store
                        .attachment(hash)
                        .ok()
                        .flatten()
                        .filter(|a| a.mime == "application/pdf");
                    match (pdf, store.blob_path(hash)) {
                        (Some(a), Some(_)) => cx.emit(PageViewEvent::OpenPdf {
                            hash: a.hash,
                            name: a.name,
                            page: None,
                            host: row.map(|r| r.block.clone()),
                        }),
                        (None, Some(p)) => cx.open_with_system(&p),
                        _ => {}
                    }
                }
                None => cx.open_url(&url),
            },
        }
        let _ = window;
    }

    fn drop_block(&mut self, d: &DraggedBlock, target: &RowItem, cx: &mut Context<Self>) {
        if d.block == target.block {
            return;
        }
        if d.page != target.page {
            // Drag across pages (P1): the block keeps its id and lands after the target.
            let (dp, tp) = (d.page.clone(), target.page.clone());
            self.app.update(cx, |app, cx| {
                let parent = app
                    .store
                    .page_state(&tp)
                    .ok()
                    .and_then(|st| st.parent_of(&target.block));
                let r = app.store.move_block_to_page(
                    &dp,
                    &d.block,
                    &tp,
                    parent.as_ref(),
                    Some(&target.block),
                );
                app.report(r);
                app.changed(vec![dp.clone(), tp.clone()], true, cx);
            });
            cx.notify();
            return;
        }
        let page = d.page.clone();
        let plan = self.app.update(cx, |app, _| {
            app.store.page_state(&page).ok().map(|st| {
                outline::move_to(
                    st,
                    &d.block,
                    st.parent_of(&target.block).as_ref(),
                    Some(&target.block),
                )
            })
        });
        if let Some(Ok(plan)) = plan {
            self.apply_plan(&page, plan, cx);
            cx.notify();
        }
    }

    fn set_expanded(&mut self, group: PageId, cx: &mut Context<Self>) {
        self.expanded_groups.insert(group);
        if let Some(page) = &self.page {
            let list: Vec<&str> = self.expanded_groups.iter().map(|p| p.as_str()).collect();
            let json = serde_json::to_string(&list).unwrap_or_default();
            let _ = self
                .app
                .read(cx)
                .store
                .local_set(&format!("refs_expanded:{page}"), &json);
        }
        self.rebuild(cx);
        cx.notify();
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.header.kind == PageKind::Daily || self.zoom.is_some() {
            self.show_calendar = !self.show_calendar;
            if let Some(d) = self.header.date {
                self.calendar_month = d;
            }
            cx.notify();
            return;
        }
        let title = self.header.title.clone();
        let input = cx.new(|cx| {
            let mut i = LineInput::new("Page title", cx);
            i.set_text(&title, cx);
            i
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, e: &LineInputEvent, window, cx| match e {
                LineInputEvent::Submit { .. } => {
                    let text = input.read(cx).text().to_string();
                    this.finish_rename(&text, window, cx);
                }
                LineInputEvent::Cancel => {
                    this.rename = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self._subs.push(sub);
        input.update(cx, |i, _| i.focus(window));
        self.rename = Some(input);
        cx.notify();
    }

    fn finish_rename(&mut self, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        let Some(page) = self.page.clone() else {
            return;
        };
        let survivor = self.app.update(cx, |app, cx| {
            let r = app.store.rename_page(&page, text);
            let s = app.report(r);
            app.changed(vec![page.clone()], true, cx);
            s
        });
        match survivor {
            Some(s) if s != page => cx.emit(PageViewEvent::Open {
                page: s,
                zoom: None,
                focus: None,
                sidebar: false,
            }),
            _ => {
                self.dirty = true;
                cx.notify();
            }
        }
    }

    fn pick_date(&mut self, d: NaiveDate, sidebar: bool, _w: &mut Window, cx: &mut Context<Self>) {
        self.show_calendar = false;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(d);
            let p = app.report(r);
            app.changed(Vec::new(), true, cx);
            p
        });
        if let Some(page) = page {
            cx.emit(PageViewEvent::Open {
                page,
                zoom: None,
                focus: None,
                sidebar,
            });
        }
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.calendar_month = crate::calendar::shift_month(self.calendar_month, delta);
        cx.notify();
    }

    fn page_action(&mut self, action: &str, cx: &mut Context<Self>) {
        let Some(page) = self.page.clone() else {
            return;
        };
        self.editor.update(cx, |e, cx| e.end(cx));
        self.app.update(cx, |app, cx| {
            let r = match action {
                "delete" => app.store.delete_page(&page).map(|_| ()),
                "restore" => app.store.restore_page(&page).map(|_| ()),
                "rewrite" => app.store.rewrite_references(&page).map(|_| ()),
                _ => Ok(()),
            };
            app.report(r);
            app.changed(Vec::new(), true, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    // ---------------------------------------------------------------- rendering

    fn chip_state(store: &utopia_store::Store, uid: &str, row_block: &BlockId) -> ChipState {
        if uid == row_block.as_str() {
            return ChipState::Cyclic;
        }
        match store.block_ref_status(uid) {
            Ok(BlockRefStatus::Live { text, .. }) => ChipState::Live(text),
            Ok(BlockRefStatus::Deleted { .. }) => ChipState::Deleted,
            _ => ChipState::Missing,
        }
    }

    fn render_rich(
        &self,
        text: &str,
        row_block: &BlockId,
        row: Option<RowItem>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let lh = crate::appearance::density(cx).line_height();
        let rendered = {
            let store = &self.app.read(cx).store;
            rich::render(text, &th, &mut |uid| {
                Self::chip_state(store, uid, row_block)
            })
        };
        let styled =
            StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
        let layout = styled.layout().clone();
        let same = rendered.text.as_ref() == text;
        let source_len = text.len();
        let row2 = row.clone();
        let links = rendered.links.iter().map(|(r, _)| r.clone()).collect();
        div()
            .flex_1()
            .min_h(px(lh))
            // An editable row shows the I-beam between its links; a read-only one (a query
            // or search hit) is itself clickable, so it keeps its container's pointer.
            .when(row.is_some(), |d| d.cursor_text())
            .child(rich::LinkText::new(styled, links))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    let ix = layout.index_for_position(e.position);
                    if let Ok(i) = ix {
                        if let Some(link) = rendered.link_at(i).cloned() {
                            cx.stop_propagation();
                            this.follow(link, row2.as_ref(), e.modifiers.shift, window, cx);
                            return;
                        }
                    }
                    if let Some(r) = &row2 {
                        let caret = match ix {
                            Ok(i) | Err(i) if same => i.min(source_len),
                            _ => source_len,
                        };
                        cx.stop_propagation();
                        this.edit(
                            r.page.clone(),
                            r.block.clone(),
                            Some(caret),
                            None,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .into_any_element()
    }

    fn render_row(&self, ix: usize, r: &RowItem, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let density = crate::appearance::density(cx);
        let lh = density.line_height();
        let vpad = density.block_spacing() / 2.;
        let host = cx.entity_id();
        let editing = self.editor.read(cx).is_editing(host, &r.page, &r.block);
        let indent = r.depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT;
        let row = r.clone();
        let content: AnyElement = if editing {
            self.editor.clone().into_any_element()
        } else {
            self.render_rich(&r.text, &r.block, Some(row.clone()), cx)
        };
        let arrow = {
            let block = r.block.clone();
            div()
                .id(("arrow", ix))
                .w(px(14.))
                .h(px(lh))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.faint)
                .when(r.has_children, |d| {
                    d.cursor_pointer()
                        .child(if r.collapsed { "▸" } else { "▾" })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_collapse(&block, cx)),
                        )
                })
        };
        let bullet = {
            let page = r.page.clone();
            let block = r.block.clone();
            let label: SharedString = plain_text(&r.text)
                .chars()
                .take(60)
                .collect::<String>()
                .into();
            div()
                .id(("bullet", ix))
                .w(px(18.))
                .h(px(lh))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(r.kind == BlockKind::Numbered, |d| {
                    d.text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.muted)
                        .child(format!("{}.", r.number))
                })
                .when(r.kind != BlockKind::Numbered, |d| {
                    d.child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(th.bullet)
                            .when(r.collapsed && r.has_children, |d| {
                                d.size(px(8.)).border_2().border_color(th.chip_bg)
                            }),
                    )
                })
                .on_drag(
                    DraggedBlock {
                        page: page.clone(),
                        block: block.clone(),
                        label,
                    },
                    |d, _, _, cx| cx.new(|_| DragPreview(d.label.clone())),
                )
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    this.editor.update(cx, |ed, cx| ed.end(cx));
                    if e.modifiers().shift || Some(&page) != this.page.as_ref() {
                        cx.emit(PageViewEvent::Open {
                            page: page.clone(),
                            zoom: Some(block.clone()),
                            focus: None,
                            sidebar: e.modifiers().shift,
                        });
                    } else {
                        this.open(page.clone(), Some(block.clone()), cx);
                        this.focus_first(window, cx);
                    }
                }))
        };
        let target = r.clone();
        div()
            .id(("row", ix))
            .debug_selector(move || format!("row-{ix}"))
            .relative()
            .flex()
            .flex_row()
            .items_start()
            .pl(px(indent))
            .pr_2()
            .py(px(vpad))
            // ALL-4: the focused block is marked by a 2 px accent bar, not by a heavier
            // text colour (the caret already moves). Absolutely placed so it costs no
            // layout shift when editing starts.
            .when(editing, |d| {
                d.child(
                    div()
                        .debug_selector(move || format!("focus-bar-{ix}"))
                        .absolute()
                        .left(px(indent))
                        .top(px(0.))
                        .w(px(2.))
                        .h(px(lh))
                        .bg(th.accent),
                )
            })
            .when(r.embed > 0, |d| {
                d.bg(th.surface).border_l_2().border_color(th.chip_bg)
            })
            .drag_over::<DraggedBlock>(move |s, _, _, _| s.border_b_2().border_color(th.accent))
            .on_drop(
                cx.listener(move |this, d: &DraggedBlock, _, cx| this.drop_block(d, &target, cx)),
            )
            .child(arrow)
            .child(bullet)
            .when(r.kind == BlockKind::Todo, |d| {
                let row = r.clone();
                d.child(
                    div()
                        .id(("todo", ix))
                        .debug_selector(|| format!("todo-{ix}"))
                        .w(px(20.))
                        .h(px(lh))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_color(if r.checked { th.accent } else { th.muted })
                        .child(if r.checked { "☑" } else { "☐" })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_checked(&row, cx))),
                )
            })
            .child({
                // Block types (FR-2.13): headings, quotes, code and done to-dos.
                let mut c = div()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| format!("content-{ix}"));
                c = match r.kind {
                    BlockKind::Heading(l) => c
                        .text_size(px(match l {
                            1 => crate::theme::H1_SIZE,
                            2 => crate::theme::H2_SIZE,
                            _ => crate::theme::H3_SIZE,
                        }))
                        .line_height(px(match l {
                            1 => 28.,
                            2 => 24.,
                            _ => 22.,
                        }))
                        .font_weight(gpui::FontWeight::BOLD),
                    BlockKind::Quote => c
                        .border_l_2()
                        .border_color(th.faint)
                        .pl_2()
                        .text_color(th.muted)
                        .italic(),
                    BlockKind::Code => c
                        .font_family("Menlo")
                        .text_size(px(13.))
                        .bg(th.code_bg)
                        .px_1()
                        .rounded_sm(),
                    BlockKind::Todo if r.checked => c.text_color(th.faint).line_through(),
                    _ => c,
                };
                c.child(content)
            })
            .when(r.depth > MAX_VISUAL_DEPTH, |d| {
                d.child(
                    div()
                        .ml_1()
                        .px_1()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .bg(th.chip_bg)
                        .rounded_sm()
                        .child(format!("depth {}", r.depth)),
                )
            })
            .into_any_element()
    }

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        quiet_control(id, label, &th)
    }

    /// The page actions menu. It is drawn deferred and anchored, so opening it floats over
    /// the page instead of taking a row in the column (ALL-3).
    fn page_menu_popup(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        // Daily titles are display-only (rule 6): `start_rename` would open the calendar
        // for them, so Rename is only offered on ordinary pages.
        let rename = (self.header.kind != PageKind::Daily).then(|| {
            quiet_control("page-menu-rename", "Rename", &th)
                .debug_selector(|| "page-menu-rename".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.page_menu = false;
                    this.start_rename(window, cx);
                }))
        });
        let delete = quiet_danger("page-menu-delete", "Delete page", &th)
            .debug_selector(|| "page-menu-delete".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                this.page_action("delete", cx);
            }));
        div()
            .id("page-menu-popup")
            .debug_selector(|| "page-menu-popup".into())
            .w(px(180.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .shadow_md()
            .children(rename.map(|r| r.w_full().justify_start()))
            .child(delete.w_full().justify_start())
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let h = self.header.clone();
        let mut col = div().flex().flex_col().gap_1().px_4().pt_4().pb_2();
        if h.missing {
            return col
                .child(
                    div()
                        .text_color(th.muted)
                        .child("This page no longer exists."),
                )
                .into_any_element();
        }
        let top = div()
            .flex()
            .items_center()
            .gap_1()
            .when(self.compact, |d| {
                d.child(
                    self.button("close", "✕", cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PageViewEvent::Close))),
                )
            })
            .when(h.kind == PageKind::Daily, |d| {
                d.child(self.button("prev-day", "‹", cx).on_click(cx.listener(
                    |this, e: &gpui::ClickEvent, w, cx| {
                        if let Some(date) = this.header.date {
                            this.pick_date(
                                date.pred_opt().unwrap_or(date),
                                e.modifiers().shift,
                                w,
                                cx,
                            );
                        }
                    },
                )))
                .child(self.button("next-day", "›", cx).on_click(cx.listener(
                    |this, e: &gpui::ClickEvent, w, cx| {
                        if let Some(date) = this.header.date {
                            this.pick_date(
                                date.succ_opt().unwrap_or(date),
                                e.modifiers().shift,
                                w,
                                cx,
                            );
                        }
                    },
                )))
            })
            .child(div().flex_1())
            .when(h.deleted, |d| {
                d.child(
                    self.button("restore", "Restore from trash", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.page_action("restore", cx))),
                )
            })
            .when(!h.deleted && !h.draft && self.zoom.is_none(), |d| {
                d.child(
                    div()
                        .relative()
                        .child(
                            self.button("page-menu", "···", cx)
                                .debug_selector(|| "page-menu".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.page_menu = !this.page_menu;
                                    cx.notify();
                                })),
                        )
                        .children(self.page_menu.then(|| {
                            deferred(
                                anchored()
                                    .anchor(Corner::TopRight)
                                    .offset(point(px(0.), px(6.)))
                                    .child(self.page_menu_popup(cx)),
                            )
                            .with_priority(1)
                        })),
                )
            });
        col = col.child(top);
        if !h.zoom_path.is_empty() {
            let mut crumbs = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted);
            crumbs = crumbs.child(
                div()
                    .id("crumb-page")
                    .cursor_pointer()
                    .hover(|s| s.text_color(th.accent))
                    .child(h.title.clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(p) = this.page.clone() {
                            this.editor.update(cx, |e, cx| e.end(cx));
                            this.open(p, None, cx);
                        }
                    })),
            );
            let last = h.zoom_path.len() - 1;
            for (i, (id, text)) in h.zoom_path.iter().enumerate() {
                let id = id.clone();
                let label: String = text.chars().take(40).collect();
                crumbs = crumbs.child("›").child(
                    div()
                        .id(("crumb", i))
                        .cursor_pointer()
                        .when(i == last, |d| d.text_color(th.text))
                        .hover(|s| s.text_color(th.accent))
                        .child(if label.is_empty() {
                            "(empty)".to_string()
                        } else {
                            label
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.page.clone() {
                                this.editor.update(cx, |e, cx| e.end(cx));
                                this.open(p, Some(id.clone()), cx);
                            }
                        })),
                );
            }
            col = col.child(crumbs);
        }
        let title_text = if self.zoom.is_some() {
            h.zoom_path
                .last()
                .map(|(_, t)| t.clone())
                .unwrap_or_default()
        } else {
            h.title.clone()
        };
        let title_el: AnyElement = match &self.rename {
            Some(input) => div()
                .text_size(px(crate::theme::TITLE_SIZE))
                .line_height(px(34.))
                .border_b_1()
                .border_color(th.accent)
                .child(input.clone())
                .into_any_element(),
            None => div()
                .id("title")
                .debug_selector(|| "page-title".into())
                .text_size(px(crate::theme::TITLE_SIZE))
                .line_height(px(34.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(if h.deleted { th.muted } else { th.text })
                .cursor_pointer()
                .child(if title_text.is_empty() {
                    "Untitled".to_string()
                } else {
                    title_text
                })
                .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
                .into_any_element(),
        };
        col = col.child(title_el);
        if self.show_calendar {
            let today = self.app.read(cx).store.today();
            let existing: HashSet<NaiveDate> = HashSet::new();
            col = col.child(
                div()
                    .w(px(200.))
                    .p_2()
                    .border_1()
                    .border_color(th.line)
                    .rounded_md()
                    .child(crate::calendar::render(
                        cx,
                        self.calendar_month,
                        today,
                        &existing,
                        Self::pick_date,
                        Self::shift_month,
                    )),
            );
        }
        if self.zoom.is_none() && !h.attributes.is_empty() {
            // Page metadata header (P1, W7): the page's `Key:: value` blocks at a glance.
            let mut grid = div()
                .flex()
                .flex_col()
                .gap_0p5()
                .py_1()
                .text_size(px(crate::theme::BODY_SIZE));
            for (k, v) in &h.attributes {
                grid = grid.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().w(px(110.)).text_color(th.muted).child(k.clone()))
                        .child(div().text_color(th.text).child(v.clone())),
                );
            }
            col = col.child(grid);
        }
        if self.zoom.is_none() && !h.deleted {
            let mut row = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted);
            if !h.aliases.is_empty() {
                row = row.child("Also known as:");
            }
            for (i, a) in h.aliases.iter().enumerate() {
                let alias = a.clone();
                row = row.child(
                    div()
                        .id(("alias", i))
                        .px_1()
                        .rounded_sm()
                        .bg(th.chip_bg)
                        .cursor_pointer()
                        .child(format!("{a} ×"))
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_alias(&alias, cx))),
                );
            }
            match &self.alias_input {
                Some(input) => {
                    row = row.child(
                        div()
                            .w(px(160.))
                            .px_1()
                            .border_1()
                            .border_color(th.accent)
                            .rounded_sm()
                            .child(input.clone()),
                    );
                }
                None => {
                    row =
                        row.child(self.button("add-alias", "+ alias", cx).on_click(
                            cx.listener(|this, _, window, cx| this.start_alias(window, cx)),
                        ));
                }
            }
            if !h.aliases.is_empty() && h.kind == PageKind::Page {
                row = row.child(
                    self.button("rewrite", "Rewrite references to this title", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.page_action("rewrite", cx))),
                );
            }
            col = col.child(row);
        }
        col.into_any_element()
    }

    fn start_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| LineInput::new("New alias", cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, e: &LineInputEvent, _, cx| match e {
                LineInputEvent::Submit { .. } => {
                    let text = input.read(cx).text().to_string();
                    this.alias_input = None;
                    if let Some(page) = this.page.clone() {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.add_alias(&page, &text);
                            if let Err(e) = &r {
                                app.last_error = Some(e.to_string());
                            }
                            app.changed(vec![page.clone()], true, cx);
                        });
                    }
                    this.dirty = true;
                    cx.notify();
                }
                LineInputEvent::Cancel => {
                    this.alias_input = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self._subs.push(sub);
        input.update(cx, |i, _| i.focus(window));
        self.alias_input = Some(input);
        cx.notify();
    }

    fn remove_alias(&mut self, alias: &str, cx: &mut Context<Self>) {
        let Some(page) = self.page.clone() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let r = app.store.remove_alias(&page, alias);
            app.report(r);
            app.changed(vec![page.clone()], true, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    /// "Link" on an unlinked mention (FR-3.11).
    fn promote(&mut self, m: &Mention, cx: &mut Context<Self>) {
        let Some(page) = self.page.clone() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let r = app.store.promote_mention(&m.block, m.range.clone(), &page);
            app.report(r);
            app.changed(vec![m.page.clone(), page.clone()], true, cx);
        });
        self.unlinked = None;
        self.dirty = true;
        cx.notify();
    }

    fn toggle_checked(&mut self, r: &RowItem, cx: &mut Context<Self>) {
        let (page, block, checked) = (r.page.clone(), r.block.clone(), !r.checked);
        self.app.update(cx, |app, cx| {
            let res = app
                .store
                .set_block_kind(&page, &block, BlockKind::Todo, checked);
            app.report(res);
            app.changed(vec![page.clone()], true, cx);
        });
        self.dirty = true;
        cx.notify();
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let sort_label = match self.query.sort {
            RefSort::Title => "Sort: title",
            RefSort::Recency => "Sort: recent",
        };
        let mut chips = div().flex().flex_wrap().gap_1().items_center();
        for (i, f) in self.query.also.iter().enumerate() {
            let id = f.clone();
            let title = self
                .filter_titles
                .get(f)
                .cloned()
                .unwrap_or_else(|| f.to_string());
            chips = chips.child(
                div()
                    .id(("filter", i))
                    .px_2()
                    .rounded_md()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .bg(th.chip_bg)
                    .text_color(th.accent)
                    .cursor_pointer()
                    .child(format!("{title} ×"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.also.retain(|p| *p != id);
                        this.refs_key = None;
                        this.dirty = true;
                        cx.notify();
                    })),
            );
        }
        for (i, (id, title)) in self.suggestions.iter().enumerate() {
            let id = id.clone();
            let t = title.clone();
            chips = chips.child(
                div()
                    .id(("suggest", i))
                    .px_2()
                    .rounded_md()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .border_1()
                    .border_color(th.line)
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(format!("+ {title}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.also.push(id.clone());
                        this.filter_titles.insert(id.clone(), t.clone());
                        this.refs_key = None;
                        this.dirty = true;
                        cx.notify();
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .pb_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .border_1()
                            .border_color(th.line)
                            .rounded_md()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .child(self.filter.clone()),
                    )
                    .child({
                        let label = match self.query.since {
                            None => "Any time".to_string(),
                            Some(d) => {
                                let today = self.app.read(cx).store.today();
                                format!("Last {} days", (today - d).num_days() + 1)
                            }
                        };
                        quiet_menu("date-filter", label, &th).on_click(cx.listener(
                            |this, _, _, cx| {
                                // Any time → last 7 days → last 30 days → any time.
                                let today = this.app.read(cx).store.today();
                                let span = this.query.since.map(|d| (today - d).num_days() + 1);
                                this.query.since = match span {
                                    None => today.checked_sub_days(chrono::Days::new(6)),
                                    Some(7) => today.checked_sub_days(chrono::Days::new(29)),
                                    _ => None,
                                };
                                this.refs_key = None;
                                this.dirty = true;
                                cx.notify();
                            },
                        ))
                    })
                    .child(quiet_menu("sort", sort_label, &th).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.query.sort = match this.query.sort {
                                RefSort::Title => RefSort::Recency,
                                RefSort::Recency => RefSort::Title,
                            };
                            this.refs_key = None;
                            this.dirty = true;
                            cx.notify();
                        },
                    ))),
            )
            .child(chips)
            .into_any_element()
    }

    fn render_item(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let Some(item) = self.items.get(ix).cloned() else {
            return div().into_any_element();
        };
        match item {
            Item::Header => self.render_header(cx),
            Item::Row(r) => self.render_row(ix, &r, cx),
            Item::Notice { depth, text } => div()
                .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child(text)
                .into_any_element(),
            Item::QueryHeader { depth, label } => div()
                .debug_selector(|| "query-header".into())
                .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                .pt_1()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child(label)
                .into_any_element(),
            Item::QueryHit { depth, hit } => {
                let page = hit.page.clone();
                let block = hit.block.clone();
                let text = self.render_rich(&hit.text, &hit.block, None, cx);
                div()
                    .id(("query-hit", ix))
                    .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                    .pr_4()
                    .py_0p5()
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(
                        div()
                            .flex_none()
                            .w(px(160.))
                            .overflow_hidden()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.accent)
                            .child(hit.page_title.clone()),
                    )
                    .child(div().flex_1().child(text))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: page.clone(),
                            zoom: None,
                            focus: Some(block.clone()),
                            sidebar: e.modifiers().shift,
                        });
                    }))
                    .into_any_element()
            }
            Item::Attachment {
                depth,
                blob,
                path,
                size,
                present,
                host,
            } => {
                use utopia_core::attach::BlobKind;
                let pad = px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.);
                let open_path = path.clone();
                let kind = blob.kind;
                match (kind, path.clone()) {
                    (BlobKind::Image, Some(p)) => div()
                        .id(("attachment", ix))
                        .debug_selector(|| "attachment-image".into())
                        .pl(pad)
                        .py_1()
                        .cursor_pointer()
                        .child(
                            gpui::img(p)
                                .max_w(px(480.))
                                .max_h(px(320.))
                                .rounded_md()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(p) = &open_path {
                                cx.open_with_system(p);
                            }
                        }))
                        .into_any_element(),
                    _ => {
                        let icon = match kind {
                            BlobKind::Image => "🖼",
                            BlobKind::Pdf => "📄",
                            BlobKind::File => "📎",
                        };
                        let detail = match (present, size) {
                            (false, _) => "waiting for sync".to_string(),
                            (true, Some(n)) => human_size(n),
                            (true, None) => String::new(),
                        };
                        let (hash, name) = (blob.hash.clone(), blob.name.clone());
                        let host = host.clone();
                        div()
                            .id(("attachment", ix))
                            .debug_selector(|| "attachment-chip".into())
                            .pl(pad)
                            .py_0p5()
                            .flex()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(th.chip_bg)
                                    .text_size(px(crate::theme::BODY_SIZE))
                                    .when(path.is_some(), |d| {
                                        d.cursor_pointer().hover(|s| s.bg(th.hover))
                                    })
                                    .child(icon)
                                    .child(blob.name.clone())
                                    .child(
                                        div()
                                            .text_size(px(crate::theme::CAPTION_SIZE))
                                            .text_color(th.faint)
                                            .child(detail),
                                    ),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| match (kind, &open_path) {
                                (BlobKind::Pdf, Some(_)) => cx.emit(PageViewEvent::OpenPdf {
                                    hash: hash.clone(),
                                    name: name.clone(),
                                    page: None,
                                    host: Some(host.clone()),
                                }),
                                (_, Some(p)) => cx.open_with_system(p),
                                _ => {}
                            }))
                            .into_any_element()
                    }
                }
            }
            Item::Empty => div()
                .id("empty")
                .px_4()
                .py_1()
                .pl(px(32.))
                .text_color(th.faint)
                .cursor_text()
                .child("Click to start writing")
                .on_click(cx.listener(|this, _, window, cx| this.focus_first(window, cx)))
                .into_any_element(),
            Item::DayHeader {
                date,
                page,
                title,
                caption,
                today,
                first,
            } => div()
                .debug_selector(move || if first { "today-title" } else { "day-title" }.into())
                .when(!first, |d| d.mt_8().border_t_1().border_color(th.line))
                .px_4()
                .pt_4()
                .pb_2()
                .child(
                    // DN-2: the weekday, plus "Today"/"Yesterday". Titles stay display
                    // only: identity is still `daily_date` (rule 6).
                    div()
                        .debug_selector(move || {
                            format!("day-caption-{}", utopia_core::date::iso(date))
                        })
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(if today { th.accent } else { th.muted })
                        .child(caption),
                )
                .child(
                    div()
                        .id(("day-title", ix))
                        .debug_selector(move || {
                            format!("day-date-{}", utopia_core::date::iso(date))
                        })
                        .text_size(px(crate::theme::TITLE_SIZE))
                        .line_height(px(34.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.text_color(th.accent))
                        .child(title)
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            this.editor.update(cx, |ed, cx| ed.end(cx));
                            let page = page.clone().or_else(|| {
                                this.app.update(cx, |app, _| {
                                    let r = app.store.draft_daily(date);
                                    app.report(r)
                                })
                            });
                            if let Some(page) = page {
                                cx.emit(PageViewEvent::Open {
                                    page,
                                    zoom: None,
                                    focus: None,
                                    sidebar: e.modifiers().shift,
                                });
                            }
                        })),
                )
                .into_any_element(),
            Item::DayEmpty { date } => div()
                .id(("day-empty", ix))
                .debug_selector(|| "day-empty".into())
                .px_4()
                .py_1()
                .pl(px(32.))
                .text_color(th.faint)
                .cursor_text()
                .child("Click to start writing")
                .on_click(cx.listener(move |this, _, window, cx| this.start_day(date, window, cx)))
                .into_any_element(),
            Item::JournalEnd => {
                // Coming into view (the list renders a margin past the viewport) loads
                // older days, so the feed scrolls on without a button, like Roam's.
                let this = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |v, cx| v.load_older_days(cx));
                });
                div()
                    .debug_selector(|| "journal-end".into())
                    .px_4()
                    .pt_8()
                    .pb(px(120.))
                    .text_size(px(crate::theme::BODY_SIZE))
                    .text_color(th.faint)
                    .child("Loading older days…")
                    .into_any_element()
            }
            Item::RefsHeader { title, count } => div()
                .debug_selector(|| "refs-header".into())
                .mt_6()
                .mx_4()
                .pt_2()
                .pb_1()
                .border_t_1()
                .border_color(th.line)
                .flex()
                .justify_between()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child(title)
                .child(count.to_string())
                .into_any_element(),
            Item::RefsControls => self.render_controls(cx),
            Item::GroupHeader {
                page,
                count,
                scheduled,
            } => {
                let id = page.id.clone();
                div()
                    .id(("group", ix))
                    .flex()
                    .justify_between()
                    .mx_2()
                    .px_2()
                    .pt_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_baseline()
                            .when(scheduled, |d| {
                                d.child(
                                    div()
                                        .text_size(px(crate::theme::CAPTION_SIZE))
                                        .text_color(th.tag)
                                        .child("Scheduled for this day"),
                                )
                            })
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(th.accent)
                                    .child(page.title.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: id.clone(),
                            zoom: None,
                            focus: None,
                            sidebar: e.modifiers().shift,
                        });
                    }))
                    .into_any_element()
            }
            Item::Hit(hit) => {
                let crumbs = hit
                    .breadcrumb
                    .iter()
                    .map(|(_, t)| t.chars().take(30).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" › ");
                let page = hit.page.clone();
                let block = hit.block.clone();
                let text = self.render_rich(&hit.text, &hit.block, None, cx);
                div()
                    .id(("hit", ix))
                    .px_6()
                    .py_0p5()
                    .flex()
                    .flex_col()
                    .cursor_pointer()
                    .when(!crumbs.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .text_color(th.faint)
                                .child(format!("› {crumbs}")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .child(div().w(px(14.)).text_color(th.bullet).child("•"))
                            // As in `render_row`: a flex item's minimum width is its
                            // unwrapped text unless `min_w_0`, so it would run wide.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .debug_selector(|| format!("hit-{ix}"))
                                    .child(text),
                            ),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: page.clone(),
                            zoom: None,
                            focus: Some(block.clone()),
                            sidebar: e.modifiers().shift,
                        });
                    }))
                    .into_any_element()
            }
            Item::More { page, hidden } => div()
                .id(("more", ix))
                .px_6()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.accent)
                .cursor_pointer()
                .child(format!("+{hidden} more"))
                .on_click(cx.listener(move |this, _, _, cx| this.set_expanded(page.clone(), cx)))
                .into_any_element(),
            Item::NoRefs => div()
                .px_4()
                .pb_6()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.faint)
                .child("No linked references yet.")
                .into_any_element(),
            Item::UnlinkedHeader { count, open } => div()
                .id("unlinked-header")
                .debug_selector(|| "unlinked-header".into())
                .mt_4()
                .mx_4()
                .pt_2()
                .pb_1()
                .border_t_1()
                .border_color(th.line)
                .flex()
                .justify_between()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .cursor_pointer()
                .hover(|s| s.text_color(th.text))
                .child(format!(
                    "{} Unlinked References",
                    if open { "▾" } else { "▸" }
                ))
                .child(count.map(|c| c.to_string()).unwrap_or_default())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.unlinked_open = !this.unlinked_open;
                    this.dirty = true;
                    cx.notify();
                }))
                .into_any_element(),
            Item::Mention(m) => {
                let th2 = th.clone();
                let mut hl = vec![(
                    m.range.clone(),
                    gpui::HighlightStyle {
                        background_color: Some(th2.highlight_bg),
                        ..Default::default()
                    },
                )];
                hl.retain(|(r, _)| r.end <= m.text.len());
                let m2 = m.clone();
                div()
                    .id(("mention", ix))
                    .debug_selector(|| format!("mention-{ix}"))
                    .px_6()
                    .py_0p5()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.accent)
                                    .child(m.page_title.clone()),
                            )
                            .child(StyledText::new(m.text.clone()).with_highlights(hl)),
                    )
                    .child(
                        div()
                            .id(("link-mention", ix))
                            .debug_selector(|| format!("link-{ix}"))
                            .px_2()
                            .rounded_sm()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.accent)
                            .border_1()
                            .border_color(th.line)
                            .cursor_pointer()
                            .hover(|s| s.bg(th2.hover))
                            .child("Link")
                            .on_click(cx.listener(move |this, _, _, cx| this.promote(&m2, cx))),
                    )
                    .into_any_element()
            }
        }
    }
}

/// Uids of `{{embed: ((uid))}}` blocks, which always render expanded (P1 block embeds).
fn embed_uids(text: &str) -> Vec<String> {
    fn walk(nodes: &[utopia_core::inline::Inline], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                utopia_core::inline::Inline::Embed { uid, .. } => out.push(uid.clone()),
                utopia_core::inline::Inline::Emph { children, .. } => walk(children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&utopia_core::inline::parse(text), &mut out);
    out
}

/// Push a row and, after it, any expanded transclusions of the blocks it references
/// (FR-4.3–4.5): nested rows edit the original block, cycles stop at depth 3.
fn push_with_embeds(
    app: &mut AppState,
    item: RowItem,
    open: &HashSet<(BlockId, String)>,
    collapsed: &HashSet<BlockId>,
    chain: &mut Vec<BlockId>,
    out: &mut Vec<Item>,
) {
    let embedded = embed_uids(&item.text);
    let refs: Vec<String> = extract_refs(&item.text)
        .into_iter()
        .filter(|r| r.kind == utopia_core::inline::RefKind::Block)
        .map(|r| r.target)
        .filter(|uid| embedded.contains(uid) || open.contains(&(item.block.clone(), uid.clone())))
        .collect();
    let depth = item.depth;
    let embed = item.embed;
    let query = utopia_core::query::query_block(&item.text);
    let mut blobs = utopia_core::attach::blob_refs(&item.text);
    let host_block = item.block.clone();
    // `{{pdf: …}} [name.pdf](blob:…)` names one file twice; show it once.
    let mut seen = HashSet::new();
    blobs.retain(|b| seen.insert(b.hash.clone()));
    out.push(Item::Row(item));
    for blob in blobs {
        let path = app.store.blob_path(&blob.hash);
        let size = app
            .store
            .attachment(&blob.hash)
            .ok()
            .flatten()
            .map(|a| a.size);
        let present = app.store.has_blob(&blob.hash);
        out.push(Item::Attachment {
            depth: depth + 1,
            blob,
            path,
            size,
            present,
            host: host_block.clone(),
        });
    }
    match query {
        Some(Ok(expr)) => match app.store.run_query(&expr, MAX_QUERY_HITS) {
            Ok(r) => {
                let shown = if r.total > r.hits.len() {
                    format!("{} of {}", r.hits.len(), r.total)
                } else {
                    r.total.to_string()
                };
                out.push(Item::QueryHeader {
                    depth: depth + 1,
                    label: format!("Query {expr} · {shown} results").into(),
                });
                out.extend(r.hits.into_iter().map(|hit| Item::QueryHit {
                    depth: depth + 1,
                    hit,
                }));
            }
            Err(e) => out.push(Item::Notice {
                depth: depth + 1,
                text: format!("Query failed: {e}").into(),
            }),
        },
        Some(Err(e)) => out.push(Item::Notice {
            depth: depth + 1,
            text: format!("Query syntax: {e}").into(),
        }),
        None => {}
    }
    for uid in refs {
        let target = BlockId(uid.clone());
        if chain.contains(&target) || embed >= MAX_EMBED_DEPTH {
            out.push(Item::Notice {
                depth: depth + 1,
                text: "↻ cyclic reference".into(),
            });
            continue;
        }
        let Ok(Some(row)) = app.store.block_row(&target) else {
            out.push(Item::Notice {
                depth: depth + 1,
                text: format!("Missing block (({uid}))").into(),
            });
            continue;
        };
        let Ok(st) = app.store.page_state(&row.page_id).cloned() else {
            continue;
        };
        if !st.is_live(&target) {
            out.push(Item::Notice {
                depth: depth + 1,
                text: "deleted block".into(),
            });
            continue;
        }
        let mut count = 0;
        let mut rows: Vec<(BlockId, usize)> = vec![(target.clone(), 0)];
        rows.extend(
            st.visible_rows(Some(&target), &|b| collapsed.contains(b))
                .into_iter()
                .map(|(id, d)| (id, d + 1)),
        );
        for (id, d) in rows {
            count += 1;
            if count > MAX_EMBED_BLOCKS {
                break;
            }
            let Some(b) = st.block(&id) else { continue };
            let child = RowItem {
                page: row.page_id.clone(),
                block: id.clone(),
                text: b.text.clone(),
                depth: depth + 1 + d,
                has_children: st.has_live_children(&id),
                collapsed: collapsed.contains(&id),
                embed: embed + 1,
                kind: b.kind,
                checked: b.checked,
                number: st.index_in_parent(&id, true).map_or(1, |i| i + 1),
            };
            chain.push(target.clone());
            push_with_embeds(app, child, open, collapsed, chain, out);
            chain.pop();
        }
    }
}

impl Render for PageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rebuild_if_dirty(cx);
        let th = theme(cx).clone();
        let view = cx.entity();
        div().size_full().bg(th.bg).child(
            list(self.list.clone(), move |ix, window, cx| {
                // List items are laid out as roots, which shrink to their content; a
                // full-width wrapper gives margins, spacers and rules the column's width.
                // The column stops at a readable line length and stays centred, like Roam.
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(READABLE_WIDTH))
                            .child(view.update(cx, |this, cx| this.render_item(ix, window, cx))),
                    )
                    .into_any_element()
            })
            .size_full(),
        )
    }
}

fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}

/// DN-2: the caption above a day in the feed: "Thursday · Today", "Wednesday ·
/// Yesterday", or just "Monday". Display only (§10.3: a day's identity is its date).
fn day_caption(date: NaiveDate, today: NaiveDate) -> String {
    let weekday = utopia_core::date::weekday_name_of(date);
    match (today - date).num_days() {
        0 => format!("{weekday} · Today"),
        1 => format!("{weekday} · Yesterday"),
        _ => weekday.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DN-2: "Thursday · Today", "Wednesday · Yesterday", "Monday".
    #[test]
    fn day_captions_name_the_weekday_and_the_neighbours() {
        // Thursday, November 5th, 2026.
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        assert_eq!(day_caption(today, today), "Thursday · Today");
        assert_eq!(
            day_caption(today.pred_opt().unwrap(), today),
            "Wednesday · Yesterday"
        );
        assert_eq!(
            day_caption(today - chrono::Duration::days(3), today),
            "Monday"
        );
        // Days further back get the weekday alone; the feed never shows a future day.
        assert_eq!(
            day_caption(today - chrono::Duration::days(30), today),
            "Tuesday"
        );
        assert_eq!(day_caption(today.succ_opt().unwrap(), today), "Friday");
    }
}
