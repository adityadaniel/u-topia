//! Quick jump (Cmd+K, §9.1): fuzzy titles and aliases; Enter opens, Shift/Cmd+Enter opens
//! in the sidebar, and a novel title offers "Create". Dates resolve to daily pages.
//!
//! The same palette is full-text search (Cmd+Shift+F) and the small capture/answer prompts.
//! Search results are grouped under their page or day (SR-2) and the matched words are
//! washed with the accent colour (SR-1); anchors come from the store, which computed them
//! against the plain text the row draws.

use gpui::{
    div, prelude::*, px, Context, Entity, EventEmitter, HighlightStyle, MouseButton,
    MouseDownEvent, StyledText, Subscription, Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockGroup, TitleHit};

use crate::app_state::AppState;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::{theme, Theme};

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
    Text(String),
    Dismissed,
}

/// One drawn line in the result list. Group headers name the page or day their blocks
/// belong to; they are not selectable, so arrow keys step over them.
#[derive(Debug, Clone)]
enum Row {
    Page(TitleHit),
    Create(String),
    Group { group: usize },
    Block { group: usize, hit: usize },
}

impl Row {
    fn selectable(&self) -> bool {
        !matches!(self, Row::Group { .. })
    }
}

pub struct JumpPalette {
    app: Entity<AppState>,
    pub input: Entity<LineInput>,
    rows: Vec<Row>,
    groups: Vec<BlockGroup>,
    selected: usize,
    title: &'static str,
    mode: PaletteMode,
    _sub: Subscription,
}

/// A result row's text with the matched terms on a soft accent wash (SR-1). The anchors
/// come from the store, which computed them against this same plain text.
fn marked(text: &str, anchors: &[(usize, usize)], th: &Theme) -> StyledText {
    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = anchors
        .iter()
        .filter(|(a, b)| {
            a < b && *b <= text.len() && text.is_char_boundary(*a) && text.is_char_boundary(*b)
        })
        .map(|&(a, b)| {
            (
                a..b,
                HighlightStyle {
                    background_color: Some(th.accent.opacity(0.16)),
                    ..Default::default()
                },
            )
        })
        .collect();
    StyledText::new(text.to_string()).with_highlights(highlights)
}

/// The page/day header of a result group: an icon on the top line, the name and the block
/// count. `items_start` keeps the icon with the first line when a long title wraps (SR-2).
fn group_header(group: usize, g: &BlockGroup, th: &Theme) -> gpui::Stateful<gpui::Div> {
    let icon = if g.daily { "◷" } else { "▤" };
    div()
        .id(("search-group", group))
        .debug_selector(move || format!("search-group-{group}"))
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .pt_2()
        .pb_1()
        .child(
            div()
                .debug_selector(move || format!("search-group-icon-{group}"))
                .w(px(14.))
                .flex_none()
                .text_color(th.muted)
                .child(icon),
        )
        .child(
            div()
                .debug_selector(move || format!("search-group-title-{group}"))
                .flex_1()
                .min_w_0()
                .text_color(th.text)
                .child(g.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .px_2()
                .rounded_md()
                .bg(th.chip_bg)
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child(format!("{}", g.hits.len())),
        )
}

impl EventEmitter<QuickJumpEvent> for JumpPalette {}

impl JumpPalette {
    pub fn new(
        app: Entity<AppState>,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_mode(app, title, PaletteMode::Pages, window, cx)
    }

    pub fn with_mode(
        app: Entity<AppState>,
        title: &'static str,
        mode: PaletteMode,
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
            rows: Vec::new(),
            groups: Vec::new(),
            selected: 0,
            title,
            mode,
            _sub: sub,
        };
        me.search("", cx);
        me
    }

    fn search(&mut self, q: &str, cx: &mut Context<Self>) {
        self.rows.clear();
        self.groups.clear();
        self.selected = 0;
        match self.mode {
            PaletteMode::Capture | PaletteMode::Text => {
                cx.notify();
                return;
            }
            PaletteMode::Search => self.search_blocks(q, cx),
            PaletteMode::Pages => self.search_pages(q, cx),
        }
        self.selected = self
            .rows
            .iter()
            .position(Row::selectable)
            .unwrap_or_default();
        cx.notify();
    }

    fn search_pages(&mut self, q: &str, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let store = &app.store;
        let hits = store.search_titles(q, 12).unwrap_or_default();
        let key = normalize_title(q);
        let exact = hits.iter().any(|h| {
            normalize_title(&h.title) == key
                || h.alias.as_deref().map(normalize_title) == Some(key.clone())
        }) || store.find_page(q).ok().flatten().is_some();
        let mut results: Vec<Row> = hits.into_iter().map(Row::Page).collect();
        if !key.is_empty() && !exact {
            results.push(Row::Create(q.trim().to_string()));
        }
        // A date typed in any accepted form jumps straight to its day.
        if let Ok(Some(p)) = store.find_page(q) {
            if !results
                .iter()
                .any(|r| matches!(r, Row::Page(h) if h.page == p))
            {
                if let Ok(Some(row)) = store.page_row(&p) {
                    let daily = row.is_daily();
                    results.insert(
                        0,
                        Row::Page(TitleHit {
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
        self.rows = results;
    }

    fn search_blocks(&mut self, q: &str, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        let groups = self
            .app
            .read(cx)
            .store
            .search_groups(q, 30)
            .unwrap_or_default();
        for (gi, g) in groups.iter().enumerate() {
            self.rows.push(Row::Group { group: gi });
            for hi in 0..g.hits.len() {
                self.rows.push(Row::Block { group: gi, hit: hi });
            }
        }
        self.groups = groups;
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected;
        for _ in 0..n {
            i = (i as isize + d).rem_euclid(n as isize) as usize;
            if self.rows[i].selectable() {
                self.selected = i;
                cx.notify();
                return;
            }
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
        let Some(row) = self.rows.get(ix).cloned() else {
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
        match row {
            Row::Page(h) => cx.emit(QuickJumpEvent::Chosen {
                page: h.page,
                sidebar,
                focus: None,
            }),
            Row::Block { group, hit } => {
                let Some(b) = self.groups.get(group).and_then(|g| g.hits.get(hit)) else {
                    return;
                };
                cx.emit(QuickJumpEvent::Chosen {
                    page: b.page.clone(),
                    sidebar,
                    focus: Some(b.block.clone()),
                });
            }
            Row::Create(t) => self.create(&t, sidebar, cx),
            Row::Group { .. } => {}
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
        let selected = self.selected;
        let rows = self.rows.iter().enumerate().map(|(i, r)| match r {
            Row::Group { group } => group_header(*group, &self.groups[*group], &th),
            Row::Page(h) => {
                let detail = match (&h.alias, h.daily) {
                    (Some(a), _) => format!("alias: {a}"),
                    (None, true) => "journal".to_string(),
                    _ => String::new(),
                };
                div()
                    .id(("qj", i))
                    .flex()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover))
                    .when(i == selected, |d| d.bg(th.hover))
                    .child(div().text_color(th.text).child(h.title.clone()))
                    .child(
                        div()
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
            }
            Row::Create(t) => div()
                .id(("qj", i))
                .px_3()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .hover(|d| d.bg(th.hover))
                .when(i == selected, |d| d.bg(th.hover))
                .child(
                    div()
                        .text_color(th.accent)
                        .child(format!("Create page \"{t}\"")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                        this.choose(i, e.modifiers.shift, w, cx)
                    }),
                ),
            Row::Block { group, hit } => {
                let b = &self.groups[*group].hits[*hit];
                let text = utopia_core::inline::plain_text(&b.text);
                let (group, hit) = (*group, *hit);
                div()
                    .id(("qj", i))
                    .debug_selector(move || format!("search-hit-{group}-{hit}"))
                    .px_3()
                    .py_1()
                    .pl(px(46.))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover))
                    .when(i == selected, |d| d.bg(th.hover))
                    .child(
                        div()
                            .debug_selector(move || format!("search-hit-text-{group}-{hit}"))
                            .text_color(th.text)
                            .line_clamp(2)
                            .child(marked(&text, &b.anchors, &th)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                            this.choose(i, e.modifiers.shift, w, cx)
                        }),
                    )
            }
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
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .child(div().flex().flex_col().children(rows)),
            )
    }
}
