//! Quick jump (Cmd+K, §9.1): fuzzy titles and aliases; Enter opens, Shift/Cmd+Enter opens
//! in the sidebar, and a novel title offers "Create". Dates resolve to daily pages.
//!
//! With nothing typed the palette offers today, yesterday and the pages opened last
//! (QJ-1). That list is local UI state the workspace keeps in `local_state` (rule 9); the
//! rest of the rows still come from the index when the query changes, never while
//! painting (rule 3).

use chrono::NaiveDate;
use gpui::{
    div, prelude::*, px, Context, Entity, EventEmitter, MouseButton, MouseDownEvent, Subscription,
    Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockHit, TitleHit};

use crate::app_state::AppState;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::theme;

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
        /// "Today" or "Yesterday": the pinned day's label.
        label: &'static str,
    },
    Create(String),
    Block(BlockHit),
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
    Text(String),
    Dismissed,
}

pub struct JumpPalette {
    app: Entity<AppState>,
    pub input: Entity<LineInput>,
    results: Vec<QjItem>,
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
            PaletteMode::Pages => "Go to page or date…",
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
                self.selected = 0;
                cx.notify();
                return;
            }
            PaletteMode::Pages => {}
        }
        // QJ-1: with nothing typed, offer today, yesterday and the pages opened last.
        if q.trim().is_empty() {
            self.results = self.empty_state(cx);
            self.selected = 0;
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
            if q.trim().is_empty() {
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
            QjItem::Block(b) => cx.emit(QuickJumpEvent::Chosen {
                page: b.page,
                sidebar,
                focus: Some(b.block),
            }),
            QjItem::Day { date, .. } => cx.emit(QuickJumpEvent::ChosenDay { date, sidebar }),
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
}

impl Render for JumpPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let rows = self.results.iter().enumerate().map(|(i, r)| {
            let (label, detail) = match r {
                QjItem::Page(h) => (
                    h.title.clone(),
                    match (&h.alias, h.daily) {
                        (Some(a), _) => format!("alias: {a}"),
                        (None, true) => "journal".to_string(),
                        _ => String::new(),
                    },
                ),
                QjItem::Day { title, label, .. } => (title.clone(), label.to_string()),
                QjItem::Create(t) => (format!("Create page \"{t}\""), String::new()),
                QjItem::Block(b) => (
                    utopia_core::inline::plain_text(&b.text)
                        .chars()
                        .take(80)
                        .collect(),
                    b.page_title.clone(),
                ),
            };
            div()
                .id(("qj", i))
                .debug_selector(move || format!("qj-row-{i}"))
                .flex()
                .justify_between()
                .px_3()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .hover(|d| d.bg(th.hover))
                .when(i == self.selected, |d| d.bg(th.hover))
                .child(
                    div()
                        .debug_selector(move || format!("qj-label-{i}"))
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(th.text)
                        .child(label),
                )
                .child(
                    div()
                        .debug_selector(move || format!("qj-detail-{i}"))
                        .flex_none()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .child(detail),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                        this.choose(i, e.modifiers.shift, w, cx)
                    }),
                )
        });
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
                    .text_size(px(crate::theme::CAPTION_SIZE))
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
                    .children(rows),
            )
    }
}
