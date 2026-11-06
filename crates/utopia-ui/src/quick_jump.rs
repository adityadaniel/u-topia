//! Quick jump (Cmd+K) and full-text search (Cmd+Shift+F) share one palette (SR-5): fuzzy
//! page titles first, then the matching blocks grouped under their page or day (SR-2).
//! Enter opens, Shift/Cmd+Enter opens in the sidebar, and a novel title offers "Create".
//! Dates resolve to daily pages. Matched words are washed in the accent colour (SR-1) and
//! the selected block is previewed beside the list (SR-4).

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Context, Corner, Entity, EventEmitter,
    HighlightStyle, MouseButton, MouseDownEvent, StyledText, Subscription, Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockGroup, BlockHit, BlockPreview, SearchAge, SearchScope, TitleHit};

use crate::app_state::AppState;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::{quiet_control, quiet_menu, theme, Theme};

/// What the palette is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Cmd+K: pages first, then blocks; the selection starts on the first page.
    Pages,
    /// Cmd+Shift+F: the same list over block bodies (P1, FTS5); starts on the first block.
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
    scope: SearchScope,
    age: SearchAge,
    age_menu: bool,
    /// The selected result in full (SR-4), read when the selection moves, not at paint.
    preview: Option<BlockPreview>,
    _sub: Subscription,
}

/// Whether the selected scope may list a page title (SR-3): Journal is days only, Pages
/// is ordinary pages only, To-dos is blocks only, All is everything.
fn scope_keeps_page(scope: SearchScope, daily: bool) -> bool {
    match scope {
        SearchScope::All => true,
        SearchScope::Journal => daily,
        SearchScope::Pages => !daily,
        SearchScope::Todos => false,
    }
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
            scope: SearchScope::default(),
            age: SearchAge::default(),
            age_menu: false,
            preview: None,
            _sub: sub,
        };
        me.search("", cx);
        me
    }

    fn search(&mut self, q: &str, cx: &mut Context<Self>) {
        self.rows.clear();
        self.groups.clear();
        self.selected = 0;
        if matches!(self.mode, PaletteMode::Capture | PaletteMode::Text) {
            cx.notify();
            return;
        }
        // One palette, two modes (SR-5): pages first, then their blocks. Cmd+K starts on
        // the first page, Cmd+Shift+F on the first block.
        self.search_pages(q, cx);
        self.append_block_groups(q, cx);
        let first_page = self.rows.iter().position(Row::selectable);
        let first_block = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Block { .. }));
        self.selected = if self.mode == PaletteMode::Search {
            first_block.or(first_page)
        } else {
            first_page
        }
        .unwrap_or_default();
        self.load_preview(cx);
        cx.notify();
    }

    fn search_pages(&mut self, q: &str, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let store = &app.store;
        let all_hits = store.search_titles(q, 12).unwrap_or_default();
        let key = normalize_title(q);
        let exact = all_hits.iter().any(|h| {
            normalize_title(&h.title) == key
                || h.alias.as_deref().map(normalize_title) == Some(key.clone())
        }) || store.find_page(q).ok().flatten().is_some();
        // The chips filter page rows as well as block groups (SR-3).
        let mut results: Vec<Row> = all_hits
            .into_iter()
            .filter(|h| scope_keeps_page(self.scope, h.daily))
            .map(Row::Page)
            .collect();
        // Block search lists existing blocks; only quick jump creates a page (SR-5).
        // Journal and To-dos cannot create a page, and Pages must not create a daily one.
        let date_like =
            utopia_core::date::parse_unambiguous_date(&key, store.date_format()).is_some();
        let can_create = self.mode == PaletteMode::Pages
            && matches!(self.scope, SearchScope::All | SearchScope::Pages)
            && (self.scope == SearchScope::All || !date_like);
        if can_create && !key.is_empty() && !exact {
            results.push(Row::Create(q.trim().to_string()));
        }
        // A date typed in any accepted form jumps straight to its day (if in scope).
        if let Ok(Some(p)) = store.find_page(q) {
            if !results
                .iter()
                .any(|r| matches!(r, Row::Page(h) if h.page == p))
            {
                if let Ok(Some(row)) = store.page_row(&p) {
                    let daily = row.is_daily();
                    if scope_keeps_page(self.scope, daily) {
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
        }
        self.rows = results;
    }

    /// Append the block hits, grouped, after the page rows (SR-2, SR-5).
    fn append_block_groups(&mut self, q: &str, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        let groups = self
            .app
            .read(cx)
            .store
            .search_groups_filtered(q, self.scope, self.age, 30)
            .unwrap_or_default();
        let base = self.groups.len();
        self.groups.extend(groups);
        for gi in base..self.groups.len() {
            self.rows.push(Row::Group { group: gi });
            for hi in 0..self.groups[gi].hits.len() {
                self.rows.push(Row::Block { group: gi, hit: hi });
            }
        }
    }

    /// Results by page, for the scope tests and the preview (SR-2, SR-3).
    pub fn groups(&self) -> &[BlockGroup] {
        &self.groups
    }

    /// The selected block's preview (SR-4), `None` when a page row is selected.
    pub fn preview(&self) -> Option<&BlockPreview> {
        self.preview.as_ref()
    }

    /// Whether the list offers "Create page" (quick jump only; SR-5).
    pub fn offers_create(&self) -> bool {
        self.rows.iter().any(|r| matches!(r, Row::Create(_)))
    }

    /// Page titles currently listed, for the scope tests (SR-3).
    pub fn page_titles(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Page(h) => Some(h.title.clone()),
                _ => None,
            })
            .collect()
    }

    fn selected_hit(&self) -> Option<&BlockHit> {
        match self.rows.get(self.selected) {
            Some(Row::Block { group, hit }) => self.groups.get(*group)?.hits.get(*hit),
            _ => None,
        }
    }

    /// Read the preview for the current selection. Store work stays out of `render` (rule 10).
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let block = self.selected_hit().map(|h| h.block.clone());
        self.preview = block.and_then(|b| {
            let app = self.app.read(cx);
            match app.store.preview_block(&b) {
                Ok(p) => p,
                Err(e) => {
                    // Rule 11: a preview that cannot be read logs and leaves the pane empty.
                    eprintln!("utopia: {e}");
                    None
                }
            }
        });
    }

    pub fn scope(&self) -> SearchScope {
        self.scope
    }

    pub fn age(&self) -> SearchAge {
        self.age
    }

    /// Re-run the same query under a new chip (SR-3). The typed text stays in the field.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text().to_string();
        self.search(&q, cx);
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
                self.load_preview(cx);
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

impl JumpPalette {
    /// The state of a previewed block as a short label (SR-4): its type, or Done for a
    /// checked to-do.
    fn state_label(kind: utopia_core::BlockKind, checked: bool) -> &'static str {
        use utopia_core::BlockKind::*;
        match kind {
            Todo if checked => "Done",
            Todo => "To-do",
            Heading(1) => "Heading 1",
            Heading(2) => "Heading 2",
            Heading(_) => "Heading 3",
            Quote => "Quote",
            Code => "Code",
            Numbered => "Numbered",
            Bullet => "Bullet",
        }
    }

    /// The right half: the selected block's path, text, state and first children (SR-4).
    fn preview_pane(&self, p: &BlockPreview, th: &Theme) -> gpui::Stateful<gpui::Div> {
        let mut crumbs: Vec<String> = vec![p.page_title.clone()];
        crumbs.extend(p.path.iter().map(|(_, t)| {
            let mut s: String = t.chars().take(40).collect();
            if t.chars().count() > 40 {
                s.push('…');
            }
            s
        }));
        let body = utopia_core::inline::plain_text(&p.text);
        let anchors = self
            .selected_hit()
            .map(|h| h.anchors.as_slice())
            .unwrap_or(&[]);
        let mut pane = div()
            .id("search-preview")
            .debug_selector(|| "search-preview".into())
            .w(px(340.))
            .flex_none()
            .max_h(px(340.))
            .overflow_y_scroll()
            .pl_3()
            .border_l_1()
            .border_color(th.line)
            .flex()
            .flex_col()
            .gap_2();
        pane = pane.child(
            div()
                .debug_selector(|| "search-preview-path".into())
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .line_clamp(2)
                .child(crumbs.join(" › ")),
        );
        let label = Self::state_label(p.kind, p.checked);
        let muted_state = p.kind == utopia_core::BlockKind::Bullet
            || (p.kind == utopia_core::BlockKind::Todo && p.checked);
        let fg = if muted_state { th.muted } else { th.accent };
        let mut state = div().flex().items_center().gap_2().child(
            div()
                .debug_selector(|| "search-preview-state".into())
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(th.chip_bg)
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(fg)
                .child(label),
        );
        if let Some(when) = chrono::DateTime::from_timestamp_millis(p.updated_at) {
            state = state.child(
                div()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("Edited {}", when.format("%b %-d, %Y"))),
            );
        }
        pane = pane
            .child(state)
            .child(
                div()
                    .text_color(th.text)
                    .line_clamp(6)
                    .child(marked(&body, anchors, th)),
            )
            .child(
                div()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(match p.child_count {
                        0 => "No children".to_string(),
                        1 => "1 child".to_string(),
                        n => format!("{n} children"),
                    }),
            );
        for (i, c) in p.children.iter().enumerate() {
            let lead = match (c.kind, c.checked) {
                (utopia_core::BlockKind::Todo, true) => "☑",
                (utopia_core::BlockKind::Todo, false) => "☐",
                _ => "•",
            };
            pane = pane.child(
                div()
                    .debug_selector(move || format!("search-preview-child-{i}"))
                    .flex()
                    .gap_2()
                    .items_start()
                    .child(div().flex_none().text_color(th.muted).child(lead))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if c.checked { th.muted } else { th.text })
                            .line_clamp(2)
                            .child(utopia_core::inline::plain_text(&c.text)),
                    ),
            );
        }
        if p.child_count > p.children.len() {
            pane = pane.child(
                div()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("+{} more", p.child_count - p.children.len())),
            );
        }
        pane
    }

    /// The scope and age chips under the field (SR-3). Only search has scopes; quick jump
    /// still searches titles.
    fn scope_bar(&self, th: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut bar = div()
            .id("search-scopes")
            .debug_selector(|| "search-scopes".into())
            .flex()
            .items_center()
            .gap_1()
            .px_1();
        for &s in SearchScope::ALL {
            let selected = s == self.scope;
            let key = s.label().to_lowercase().replace(' ', "-");
            bar = bar.child(
                quiet_control(
                    gpui::ElementId::Name(format!("search-scope-{key}").into()),
                    s.label(),
                    th,
                )
                .debug_selector(move || format!("search-scope-{key}"))
                .when(selected, |d| d.bg(th.chip_bg).text_color(th.accent))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.scope = s;
                    this.age_menu = false;
                    this.refilter(cx);
                })),
            );
        }
        let menu_open = self.age_menu;
        bar.child(div().flex_1()).child(
            div()
                .relative()
                .child(
                    quiet_menu("search-age", self.age.label(), th)
                        .debug_selector(|| "search-age".into())
                        .when(menu_open, |d| d.bg(th.chip_bg).text_color(th.accent))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.age_menu = !this.age_menu;
                            cx.notify();
                        })),
                )
                .children(menu_open.then(|| {
                    deferred(
                        anchored()
                            .anchor(Corner::TopRight)
                            .offset(point(px(0.), px(6.)))
                            .child(self.age_popup(th, cx)),
                    )
                    .with_priority(1)
                })),
        )
    }

    /// The "Any time" menu, drawn deferred so opening it never moves the chips or the
    /// results (the lesson from screen 00's page menu).
    fn age_popup(&self, th: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut menu = div()
            .id("search-age-popup")
            .debug_selector(|| "search-age-popup".into())
            .w(px(170.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .shadow_md()
            // The results list is underneath: stop the mouse-down here or a row would
            // choose itself before the menu item's click runs (found by the scope test).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for &a in SearchAge::ALL {
            let selected = a == self.age;
            let key = a.label().to_lowercase().replace(' ', "-");
            menu = menu.child(
                quiet_control(
                    gpui::ElementId::Name(format!("search-age-item-{key}").into()),
                    a.label(),
                    th,
                )
                .debug_selector(move || format!("search-age-{key}"))
                .w_full()
                .justify_start()
                .when(selected, |d| d.bg(th.chip_bg).text_color(th.accent))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.age = a;
                    this.age_menu = false;
                    this.refilter(cx);
                })),
            );
        }
        menu
    }
}

impl Render for JumpPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let selected = self.selected;
        // Build the chips before borrowing the rows, because their click listeners need a
        // mutable context.
        let scopes = matches!(self.mode, PaletteMode::Pages | PaletteMode::Search)
            .then(|| self.scope_bar(&th, cx));
        let preview = self.preview.as_ref().map(|p| self.preview_pane(p, &th));
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
                    .debug_selector(move || format!("search-page-{i}"))
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
                .debug_selector(|| "search-create".into())
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
            // One palette, two modes (SR-5): the merged list can show a preview, so it is
            // wide; the small prompts stay narrow.
            .w(px(
                if matches!(self.mode, PaletteMode::Pages | PaletteMode::Search) {
                    860.
                } else {
                    520.
                },
            ))
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
            .children(scopes)
            .child(
                div()
                    .id("jump-results-row")
                    .flex()
                    .gap_2()
                    .max_h(px(340.))
                    .child(
                        div()
                            .id("jump-results")
                            .debug_selector(|| "jump-results".into())
                            .flex_1()
                            .min_w_0()
                            .max_h(px(340.))
                            .overflow_y_scroll()
                            .child(div().flex().flex_col().children(rows)),
                    )
                    .children(preview),
            )
    }
}
