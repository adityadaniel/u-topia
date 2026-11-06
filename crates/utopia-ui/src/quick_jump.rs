//! Quick jump (Cmd+K, §9.1): fuzzy titles and aliases; Enter opens, Shift/Cmd+Enter opens
//! in the sidebar, and a novel title offers "Create". Dates resolve to daily pages.

use gpui::{
    div, prelude::*, px, Context, Entity, EventEmitter, HighlightStyle, MouseButton,
    MouseDownEvent, StyledText, Subscription, Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockHit, TitleHit};

use crate::app_state::AppState;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::theme;

/// A result row's text with the matched terms on a soft accent wash (SR-1). The anchors
/// come from the store, which computed them against this same plain text.
fn marked(text: &str, anchors: &[(usize, usize)], th: &crate::theme::Theme) -> StyledText {
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

#[derive(Debug, Clone)]
pub enum QjItem {
    Page(TitleHit),
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
    Text(String),
    Dismissed,
}

pub struct JumpPalette {
    app: Entity<AppState>,
    pub input: Entity<LineInput>,
    results: Vec<QjItem>,
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
            results: Vec::new(),
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
                QjItem::Create(t) => (format!("Create page \"{t}\""), String::new()),
                QjItem::Block(b) => (
                    utopia_core::inline::plain_text(&b.text),
                    b.page_title.clone(),
                ),
            };
            let label: StyledText = match r {
                QjItem::Block(b) => marked(&label, &b.anchors, &th),
                _ => StyledText::new(label),
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
                .when(i == self.selected, |d| d.bg(th.hover))
                .child(div().text_color(th.text).line_clamp(2).child(label))
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
            .child(div().flex().flex_col().overflow_hidden().children(rows))
    }
}
