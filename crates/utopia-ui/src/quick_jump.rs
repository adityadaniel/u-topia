//! Quick jump (Cmd+K, §9.1): fuzzy titles and aliases; Enter opens, Shift/Cmd+Enter opens
//! in the sidebar, and a novel title offers "Create". Dates resolve to daily pages.
//!
//! One shell is shared by every palette mode (quick jump, full-text search, quick capture,
//! free text). In page mode it has a useful empty state — today, yesterday and the pages
//! opened last (QJ-1) — labelled rows with an icon per type (QJ-2), the typed letters in
//! semibold (QJ-4), a footer of key hints (QJ-3), and, after a leading `>`, the actions
//! from [`crate::actions::CATALOG`] with their keys (QJ-5).
//!
//! All of this is view state. Rows come from the index (`Store::search_titles`,
//! `Store::tag_pages`) when the query changes, never while painting (rule 3), and nothing
//! here writes a shard.

use std::collections::HashSet;
use std::ops::Range;

use chrono::NaiveDate;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, EventEmitter, FontWeight, HighlightStyle,
    MouseButton, MouseDownEvent, StyledText, Subscription, Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockHit, TitleHit};

use crate::actions::{platform_keys, pretty_keys, ActionDoc, CATALOG};
use crate::app_state::AppState;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::{theme, Theme, CAPTION_SIZE, CONTROL_SIZE};

/// How many pages the empty state offers after today and yesterday (QJ-1).
pub const RECENT_ROWS: usize = 8;

#[derive(Debug, Clone)]
pub enum QjItem {
    Page(TitleHit),
    /// A day offered by the empty state (QJ-1). It opens by date, so a day nobody has
    /// written yet still opens as a draft rather than as a page (FR-1.2).
    Day {
        date: NaiveDate,
        title: String,
        /// "Today" or "Yesterday": the type label of a pinned day.
        label: &'static str,
    },
    Create(String),
    Block(BlockHit),
    /// An action from [`CATALOG`], listed after a leading `>` (QJ-5).
    Command(&'static ActionDoc),
}

/// What a result row is. The palette draws one glyph and one type label per kind (QJ-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Page,
    Day,
    Tag,
    Block,
    Command,
    Create,
}

impl RowKind {
    /// The name the row's debug selector carries, for layout tests.
    fn key(self) -> &'static str {
        match self {
            RowKind::Page => "page",
            RowKind::Day => "day",
            RowKind::Tag => "tag",
            RowKind::Block => "block",
            RowKind::Command => "command",
            RowKind::Create => "create",
        }
    }

    /// The type label at the right edge of the row.
    fn label(self) -> &'static str {
        match self {
            RowKind::Page => "Page",
            RowKind::Day => "Day",
            RowKind::Tag => "Tag",
            RowKind::Block => "",
            RowKind::Command => "",
            RowKind::Create => "",
        }
    }
}

/// What the palette is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Cmd+K: titles, aliases and dates.
    Pages,
    /// Cmd+Shift+F: full-text search over block bodies (P1, FTS5).
    Search,
    /// Cmd+Shift+Space: one line appended to today's note (FR-2.19).
    Capture,
    /// A free-text answer (e.g. the title for "Extract block to page").
    Text,
}

#[derive(Debug, Clone)]
pub enum QuickJumpEvent {
    Chosen {
        page: PageId,
        sidebar: bool,
        focus: Option<BlockId>,
    },
    /// A day from the empty state (QJ-1): opened by date, draft or not.
    ChosenDay {
        date: NaiveDate,
        sidebar: bool,
    },
    /// A command from the catalog (QJ-5), by its `keymap.json` name.
    RunCommand(&'static str),
    Text(String),
    Dismissed,
}

pub struct JumpPalette {
    app: Entity<AppState>,
    pub input: Entity<LineInput>,
    results: Vec<QjItem>,
    /// Ids among `results` that exist only because of a `#tag` (QJ-2).
    tags: HashSet<PageId>,
    /// The pages opened last, newest first, when this palette offers the empty state
    /// (QJ-1). `None` for the modes where the empty state would be a lie: redirecting a
    /// link or extracting a block has nothing to do with the pages you just visited.
    recent: Option<Vec<PageId>>,
    selected: usize,
    title: &'static str,
    mode: PaletteMode,
    _sub: Subscription,
}

impl EventEmitter<QuickJumpEvent> for JumpPalette {}

impl JumpPalette {
    pub fn new(
        app: Entity<AppState>,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_mode(app, title, PaletteMode::Pages, None, window, cx)
    }

    pub fn with_mode(
        app: Entity<AppState>,
        title: &'static str,
        mode: PaletteMode,
        recent: Option<Vec<PageId>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match mode {
            PaletteMode::Pages => "Go to page or date, or > for a command…",
            PaletteMode::Search => "Search all blocks…",
            PaletteMode::Capture => "Capture to today's note…",
            PaletteMode::Text => "Title…",
        };
        let input = cx.new(|cx| LineInput::new(placeholder, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, e: &LineInputEvent, window, cx| match e {
                LineInputEvent::Changed(q) => this.search(q, cx),
                LineInputEvent::Up => this.step(-1, cx),
                LineInputEvent::Down => this.step(1, cx),
                LineInputEvent::Submit { alt } => this.choose(this.selected, *alt, window, cx),
                LineInputEvent::Cancel => cx.emit(QuickJumpEvent::Dismissed),
            },
        );
        input.update(cx, |i, _| i.focus(window));
        let mut me = Self {
            app,
            input,
            results: Vec::new(),
            tags: HashSet::new(),
            recent,
            selected: 0,
            title,
            mode,
            _sub: sub,
        };
        me.search("", cx);
        me
    }

    fn search(&mut self, q: &str, cx: &mut Context<Self>) {
        match self.mode {
            PaletteMode::Capture | PaletteMode::Text => {
                self.results.clear();
                self.tags.clear();
                cx.notify();
                return;
            }
            PaletteMode::Search => {
                let hits = if q.trim().is_empty() {
                    Vec::new()
                } else {
                    self.app
                        .read(cx)
                        .store
                        .search_blocks(q, 30)
                        .unwrap_or_default()
                };
                self.results = hits.into_iter().map(QjItem::Block).collect();
                self.tags.clear();
                self.selected = 0;
                cx.notify();
                return;
            }
            PaletteMode::Pages => {}
        }
        // QJ-5: a leading `>` lists the command catalog instead of pages.
        if let Some(rest) = q.strip_prefix('>') {
            self.results = commands(rest).into_iter().map(QjItem::Command).collect();
            self.tags.clear();
            self.selected = 0;
            cx.notify();
            return;
        }
        // QJ-1: with nothing typed, offer today, yesterday and the pages opened last.
        if q.trim().is_empty() {
            self.results = self.empty_state(cx);
            self.selected = 0;
            self.rank_tags(cx);
            cx.notify();
            return;
        }
        let store = &self.app.read(cx).store;
        let hits = store.search_titles(q, 12).unwrap_or_default();
        let key = normalize_title(q);
        let exact = hits.iter().any(|h| {
            normalize_title(&h.title) == key
                || h.alias.as_deref().map(normalize_title) == Some(key.clone())
        }) || store.find_page(q).ok().flatten().is_some();
        let mut results: Vec<QjItem> = hits.into_iter().map(QjItem::Page).collect();
        if !key.is_empty() && !exact {
            results.push(QjItem::Create(q.trim().to_string()));
        }
        // A date typed in any accepted form jumps straight to its day.
        if let Ok(Some(p)) = store.find_page(q) {
            if !results
                .iter()
                .any(|r| matches!(r, QjItem::Page(h) if h.page == p))
            {
                if let Ok(Some(row)) = store.page_row(&p) {
                    let daily = row.is_daily();
                    results.insert(
                        0,
                        QjItem::Page(TitleHit {
                            page: p,
                            title: row.title,
                            alias: None,
                            daily,
                            score: i64::MAX,
                        }),
                    );
                }
            }
        }
        self.results = results;
        self.selected = 0;
        self.rank_tags(cx);
        cx.notify();
    }

    /// The empty state (QJ-1): today, yesterday, then the pages opened last, newest
    /// first. Recent pages come from local UI state the workspace keeps (rule 9); a day
    /// nobody has written yet is offered by date (FR-1.2).
    fn empty_state(&self, cx: &mut Context<Self>) -> Vec<QjItem> {
        let app = self.app.read(cx);
        let store = &app.store;
        let today = store.today();
        let yesterday = today.pred_opt();
        let fmt = store.date_format().clone();
        let mut out: Vec<QjItem> = vec![QjItem::Day {
            date: today,
            title: fmt.format(today),
            label: "Today",
        }];
        if let Some(d) = yesterday {
            out.push(QjItem::Day {
                date: d,
                title: fmt.format(d),
                label: "Yesterday",
            });
        }
        let Some(recent) = &self.recent else {
            // Not a "go to page" palette: no empty state.
            return Vec::new();
        };
        for id in recent {
            if out.len() >= 2 + RECENT_ROWS {
                break;
            }
            let Ok(Some(row)) = store.page_row(id) else {
                continue; // deleted, or a draft day that was never written
            };
            // Today and yesterday are already the first two rows.
            if row.is_daily() && (row.daily_date == Some(today) || row.daily_date == yesterday) {
                continue;
            }
            out.push(QjItem::Page(TitleHit {
                page: row.id.clone(),
                daily: row.is_daily(),
                title: row.title,
                alias: None,
                score: 0,
            }));
        }
        out
    }

    /// Mark the pages that exist only because of a `#tag` (QJ-2). One index query per
    /// keystroke at most, over the handful of rows on screen.
    fn rank_tags(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<PageId> = self
            .results
            .iter()
            .filter_map(|r| match r {
                QjItem::Page(h) => Some(h.page.clone()),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            self.tags.clear();
            return;
        }
        match self.app.read(cx).store.tag_pages(&ids) {
            Ok(tags) => self.tags = tags,
            Err(e) => {
                eprintln!("utopia: {e}");
                self.tags.clear();
            }
        }
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.results.len() as isize;
        if n > 0 {
            self.selected = ((self.selected as isize + d).rem_euclid(n)) as usize;
            cx.notify();
        }
    }

    fn choose(&mut self, ix: usize, sidebar: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode, PaletteMode::Capture | PaletteMode::Text) {
            let text = self.input.read(cx).text().trim().to_string();
            if !text.is_empty() {
                cx.emit(QuickJumpEvent::Text(text));
            }
            return;
        }
        let Some(item) = self.results.get(ix).cloned() else {
            if self.mode == PaletteMode::Search {
                return;
            }
            // Nothing matched yet: treat the query as a title (dates resolve first).
            let q = self.input.read(cx).text().to_string();
            if q.trim().is_empty() || q.starts_with('>') {
                return;
            }
            return self.create(&q, sidebar, cx);
        };
        match item {
            QjItem::Page(h) => cx.emit(QuickJumpEvent::Chosen {
                page: h.page,
                sidebar,
                focus: None,
            }),
            QjItem::Day { date, .. } => cx.emit(QuickJumpEvent::ChosenDay { date, sidebar }),
            QjItem::Block(b) => cx.emit(QuickJumpEvent::Chosen {
                page: b.page,
                sidebar,
                focus: Some(b.block),
            }),
            QjItem::Command(d) => cx.emit(QuickJumpEvent::RunCommand(d.action)),
            QjItem::Create(t) => self.create(&t, sidebar, cx),
        }
    }

    fn create(&mut self, title: &str, sidebar: bool, cx: &mut Context<Self>) {
        // A new page opens with the caret in its first block, so typing continues there.
        let created = self.app.update(cx, |app, cx| {
            let r = app
                .store
                .create_page(title)
                .and_then(|p| app.store.ensure_first_block(&p).map(|b| (p, b)));
            let p = app.report(r);
            app.changed(Vec::new(), true, cx);
            p
        });
        if let Some((page, block)) = created {
            cx.emit(QuickJumpEvent::Chosen {
                page,
                sidebar,
                focus: Some(block),
            });
        }
    }

    /// What a row says, how it is labelled and what it is (QJ-2).
    fn row_parts(&self, item: &QjItem) -> (RowKind, String, String) {
        match item {
            QjItem::Page(h) => {
                let kind = if h.daily {
                    RowKind::Day
                } else if self.tags.contains(&h.page) {
                    RowKind::Tag
                } else {
                    RowKind::Page
                };
                // An alias hit says why it matched; the icon still says what it is.
                let detail = if h.alias.is_some() {
                    "Alias".to_string()
                } else {
                    kind.label().to_string()
                };
                (kind, h.title.clone(), detail)
            }
            QjItem::Day { title, label, .. } => (RowKind::Day, title.clone(), label.to_string()),
            QjItem::Create(t) => (RowKind::Create, format!("Create \"{t}\""), String::new()),
            QjItem::Block(b) => (
                RowKind::Block,
                utopia_core::inline::plain_text(&b.text)
                    .chars()
                    .take(80)
                    .collect(),
                b.page_title.clone(),
            ),
            QjItem::Command(d) => (
                RowKind::Command,
                d.label.to_string(),
                pretty_keys(&platform_keys(d.keys)),
            ),
        }
    }

    /// One result row: icon, label with the match in semibold (QJ-4), type label.
    fn row(&self, i: usize, item: &QjItem, needle: &str, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let (kind, label, detail) = self.row_parts(item);
        let selected = i == self.selected;
        let marks: Vec<(Range<usize>, HighlightStyle)> = if kind == RowKind::Create {
            Vec::new()
        } else {
            highlight_ranges(&label, needle)
                .into_iter()
                .map(|r| {
                    (
                        r,
                        HighlightStyle {
                            font_weight: Some(FontWeight::SEMIBOLD),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        };
        let text: AnyElement = if marks.is_empty() {
            div().child(label).into_any_element()
        } else {
            div()
                .child(StyledText::new(label).with_highlights(marks))
                .into_any_element()
        };
        let accent = kind == RowKind::Create;
        div()
            .id(("qj", i))
            .debug_selector(move || format!("qj-row-{i}"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .when(accent, |d| d.text_color(th.accent))
            .hover(|d| d.bg(th.hover))
            .when(selected, |d| d.bg(th.chip_bg))
            .child(row_icon(i, kind, &th))
            .child(
                div()
                    .debug_selector(move || format!("qj-label-{i}"))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .child(text),
            )
            .child(
                div()
                    .debug_selector(move || format!("qj-detail-{i}"))
                    .flex_none()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(if accent { th.accent } else { th.muted })
                    .child(detail),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                    let beside = e.modifiers.shift || e.modifiers.platform;
                    this.choose(i, beside, w, cx)
                }),
            )
            .into_any_element()
    }
}

/// The one glyph per row type (QJ-2). Drawn rather than set in a font, except `#` and
/// `+`, so nothing depends on a symbol font being installed.
fn row_icon(i: usize, kind: RowKind, th: &Theme) -> AnyElement {
    let slot = div()
        .id(("qj-icon", i))
        .debug_selector(move || format!("qj-icon-{i}-{}", kind.key()))
        .w(px(14.))
        .flex_none()
        .flex()
        .justify_center()
        .items_center();
    match kind {
        RowKind::Page => slot
            .child(
                div()
                    .w(px(8.))
                    .h(px(10.))
                    .rounded_xs()
                    .border_1()
                    .border_color(th.faint),
            )
            .into_any_element(),
        RowKind::Day => slot
            .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(th.muted))
            .into_any_element(),
        RowKind::Tag => slot
            .text_size(px(CONTROL_SIZE))
            .text_color(th.tag)
            .font_weight(FontWeight::SEMIBOLD)
            .child("#")
            .into_any_element(),
        RowKind::Block => slot
            .child(div().w(px(7.)).h(px(1.)).bg(th.faint))
            .into_any_element(),
        RowKind::Command => slot
            .text_size(px(CAPTION_SIZE))
            .text_color(th.muted)
            .child("⌘")
            .into_any_element(),
        RowKind::Create => slot
            .text_size(px(CONTROL_SIZE))
            .text_color(th.accent)
            .font_weight(FontWeight::SEMIBOLD)
            .child("+")
            .into_any_element(),
    }
}

impl Render for JumpPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let query = self.input.read(cx).text().to_string();
        let needle = query.strip_prefix('>').unwrap_or(&query).to_string();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut create: Option<AnyElement> = None;
        for (i, item) in self.results.iter().enumerate() {
            let el = self.row(i, item, &needle, cx);
            if matches!(item, QjItem::Create(_)) {
                create = Some(el);
            } else {
                rows.push(el);
            }
        }
        // "Create page" is last, above a hairline, in the accent colour (QJ-2).
        let create_row = create.map(|el| {
            div()
                .debug_selector(|| "jump-create".into())
                .mt_1()
                .pt_1()
                .border_t_1()
                .border_color(th.line)
                .child(el)
        });
        let hints = match self.mode {
            PaletteMode::Pages => "↑↓ move · ↩ open · ⌘↩ open in split · esc",
            PaletteMode::Search => "↑↓ move · ↩ open · ⌘↩ open beside · esc",
            PaletteMode::Capture => "↩ save to today · esc",
            PaletteMode::Text => "↩ confirm · esc",
        };
        div()
            .debug_selector(|| "jump-palette".into())
            .w(px(520.))
            .max_h(px(440.))
            .flex()
            .flex_col()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_lg()
            .shadow_lg()
            .p_2()
            .gap_1()
            .child(
                div()
                    .px_1()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(self.title),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(th.line)
                    .rounded_md()
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("jump-results")
                    .debug_selector(|| "jump-results".into())
                    .flex()
                    .flex_col()
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .children(rows)
                    .children(create_row),
            )
            .child(
                div()
                    .debug_selector(|| "jump-footer".into())
                    .border_t_1()
                    .border_color(th.line)
                    .pt_1()
                    .px_1()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.faint)
                    .child(hints),
            )
    }
}

/// The commands a `>` query lists (QJ-5), in catalog order so an empty query reads as a
/// menu grouped by category. Matches the label, the category, and the keys either as
/// `keymap.json` spells them or as the row shows them.
fn commands(query: &str) -> Vec<&'static ActionDoc> {
    let q = query.trim().to_lowercase();
    CATALOG
        .iter()
        .filter(|d| {
            q.is_empty()
                || d.label.to_lowercase().contains(&q)
                || d.category.label().to_lowercase().contains(&q)
                || d.keys.to_lowercase().contains(&q)
                || pretty_keys(&platform_keys(d.keys))
                    .to_lowercase()
                    .contains(&q)
        })
        .collect()
}

/// Byte ranges of the typed letters inside `label`, for the semibold match (QJ-4). A
/// contiguous, case-insensitive match wins, so `ana` bolds just the `ana` of `Dana`;
/// otherwise the letters of the fuzzy subsequence are marked one at a time, which is why
/// `dn` still shows what made `Dana` a result. Case folding can change byte lengths, so
/// the contiguous pass is only trusted when the folded text has the same length.
fn highlight_ranges(label: &str, needle: &str) -> Vec<Range<usize>> {
    let needle = needle.trim();
    if needle.is_empty() || label.is_empty() {
        return Vec::new();
    }
    let lower_label = label.to_lowercase();
    let lower_needle = needle.to_lowercase();
    if lower_label.len() == label.len() && lower_needle.len() == needle.len() {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = lower_label[from..].find(&lower_needle) {
            let start = from + i;
            out.push(start..start + needle.len());
            from = start + needle.len();
        }
        if !out.is_empty() {
            return out;
        }
    }
    let mut out = Vec::new();
    let mut it = label.char_indices();
    for want in lower_needle.chars() {
        let mut found = None;
        for (i, c) in it.by_ref() {
            let mut lower = c.to_lowercase();
            if lower.next() == Some(want) && lower.next().is_none() {
                found = Some((i, c));
                break;
            }
        }
        let Some((i, c)) = found else {
            return Vec::new();
        };
        out.push(i..i + c.len_utf8());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QJ-4: the typed letters are marked inside the result, and nothing else is.
    #[test]
    fn the_match_is_marked() {
        assert_eq!(highlight_ranges("Dana", "dan"), vec![0..3]);
        assert_eq!(highlight_ranges("Dana", "ANA"), vec![1..4]);
        // Every occurrence is marked, not just the first.
        assert_eq!(
            highlight_ranges("Atlas launch", "a"),
            vec![0..1, 3..4, 7..8]
        );
        // A fuzzy match marks the letters it matched.
        assert_eq!(highlight_ranges("Dana", "dn"), vec![0..1, 2..3]);
        assert!(highlight_ranges("Dana", "x").is_empty());
        assert!(highlight_ranges("Dana", "").is_empty());
        // Ranges are byte ranges into the label, so a non-ASCII title stays intact.
        assert_eq!(highlight_ranges("Café notes", "é"), vec![3..5]);
        assert_eq!(highlight_ranges("Café notes", "notes"), vec![6..11]);
    }

    /// QJ-5: `>` lists the catalog, filtered by label, category or key.
    #[test]
    fn commands_come_from_the_catalog() {
        assert_eq!(commands("").len(), CATALOG.len());
        let today = commands("today");
        assert!(
            today.iter().any(|d| d.action == "GoToday"),
            "{:?}",
            today.iter().map(|d| d.action).collect::<Vec<_>>()
        );
        assert!(commands("navigation")
            .iter()
            .all(|d| d.category.label() == "Navigation"));
        assert!(commands("cmd-shift-g")
            .iter()
            .any(|d| d.action == "ShowGraph"));
        assert!(commands("⇧⌘G").iter().any(|d| d.action == "ShowGraph"));
        assert!(commands("nonsense-nothing").is_empty());
    }
}
