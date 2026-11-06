//! Graph view (P2): pages as nodes, links as edges, laid out force-directed off the main
//! thread. Drag to pan, scroll to zoom, click a node to open its page (shift-click opens
//! it in the right sidebar). "Local" limits the graph to two links around the page that
//! was open.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, point, prelude::*, px, Bounds, Context, EventEmitter, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    ScrollWheelEvent, SharedString, Task, TextRun, Window,
};
use utopia_core::PageId;
use utopia_store::{GraphOptions, PageGraph};

use crate::app_state::AppState;
use crate::theme::theme;

const LAYOUT_ITERATIONS: usize = 160;
const MARGIN: f32 = 48.;

pub enum GraphEvent {
    Open { page: PageId, sidebar: bool },
    Close,
}

pub struct GraphView {
    app: gpui::Entity<AppState>,
    /// The page that was open when the graph was shown (the local graph's center).
    origin: Option<PageId>,
    local: bool,
    include_daily: bool,
    graph: PageGraph,
    pos: Vec<(f32, f32)>,
    laid_out: bool,
    pan: Point<Pixels>,
    zoom: f32,
    press: Option<(Point<Pixels>, Point<Pixels>)>,
    hover: Option<usize>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    _layout: Option<Task<()>>,
}

impl EventEmitter<GraphEvent> for GraphView {}

impl Focusable for GraphView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GraphView {
    pub fn new(
        app: gpui::Entity<AppState>,
        origin: Option<PageId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut v = Self {
            app,
            origin,
            local: false,
            // Most links in a daily-notes graph come from journal pages; hiding them by
            // default leaves the picture nearly empty.
            include_daily: true,
            graph: PageGraph::default(),
            pos: Vec::new(),
            laid_out: false,
            pan: point(px(0.), px(0.)),
            zoom: 1.0,
            press: None,
            hover: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            focus: cx.focus_handle(),
            _layout: None,
        };
        v.reload(cx);
        v
    }

    pub fn node_count(&self) -> usize {
        self.graph.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edges.len()
    }

    pub fn is_laid_out(&self) -> bool {
        self.laid_out
    }

    pub fn set_local(&mut self, local: bool, cx: &mut Context<Self>) {
        self.local = local;
        self.reload(cx);
    }

    /// Screen position of a node, for tests and hit-testing.
    pub fn node_screen_pos(&self, i: usize) -> Option<Point<Pixels>> {
        let b = self.bounds.get();
        self.pos.get(i).map(|p| self.to_screen(*p, b))
    }

    pub fn node_page(&self, i: usize) -> Option<PageId> {
        self.graph.nodes.get(i).map(|n| n.page.clone())
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let opts = GraphOptions {
            center: self.local.then(|| self.origin.clone()).flatten(),
            depth: 2,
            include_daily: self.include_daily,
            limit: 400,
        };
        let graph = self.app.update(cx, |app, _| {
            let r = app.store.page_graph(&opts);
            app.report(r).unwrap_or_default()
        });
        let n = graph.nodes.len();
        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.0, e.1)).collect();
        self.graph = graph;
        self.laid_out = false;
        self.hover = None;
        self.pan = point(px(0.), px(0.));
        self.zoom = 1.0;
        let task = cx
            .background_executor()
            .spawn(async move { utopia_core::graph::layout(n, &edges, LAYOUT_ITERATIONS, 0x7e11) });
        self._layout = Some(cx.spawn(async move |this, cx| {
            let pos = task.await;
            let _ = this.update(cx, |this, cx| {
                if pos.len() == this.graph.nodes.len() {
                    this.pos = pos;
                    this.laid_out = true;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn scale(b: Bounds<Pixels>) -> f32 {
        (f32::from(b.size.width).min(f32::from(b.size.height)) / 2. - MARGIN).max(20.)
    }

    fn to_screen(&self, p: (f32, f32), b: Bounds<Pixels>) -> Point<Pixels> {
        let s = Self::scale(b) * self.zoom;
        let c = b.center();
        point(
            c.x + px(p.0 * s) + self.pan.x,
            c.y + px(p.1 * s) + self.pan.y,
        )
    }

    fn radius(degree: usize) -> f32 {
        (3. + (degree as f32).sqrt() * 1.6).min(14.)
    }

    fn hit(&self, at: Point<Pixels>) -> Option<usize> {
        if !self.laid_out {
            return None;
        }
        let b = self.bounds.get();
        let mut best: Option<(usize, f32)> = None;
        for (i, n) in self.graph.nodes.iter().enumerate() {
            let p = self.to_screen(self.pos[i], b);
            let d = f32::from((p.x - at.x).abs()).hypot(f32::from((p.y - at.y).abs()));
            if d <= Self::radius(n.degree) + 4. && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.press = Some((e.position, e.position));
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((start, last)) = self.press {
            if e.pressed_button == Some(MouseButton::Left) {
                self.pan += e.position - last;
                self.press = Some((start, e.position));
                cx.notify();
                return;
            }
            self.press = None;
        }
        let h = self.hit(e.position);
        if h != self.hover {
            self.hover = h;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((start, _)) = self.press.take() else {
            return;
        };
        let moved =
            f32::from((e.position.x - start.x).abs()) + f32::from((e.position.y - start.y).abs());
        if moved < 4. {
            if let Some(i) = self.hit(e.position) {
                cx.emit(GraphEvent::Open {
                    page: self.graph.nodes[i].page.clone(),
                    sidebar: e.modifiers.shift,
                });
            }
        }
        cx.notify();
    }

    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(e.delta.pixel_delta(px(16.)).y);
        let old = self.zoom;
        self.zoom = (self.zoom * (1. + dy * 0.004)).clamp(0.25, 8.);
        // Zoom about the cursor: keep the point under it fixed.
        let c = self.bounds.get().center();
        let k = self.zoom / old;
        let rel = e.position - c - self.pan;
        self.pan += point(rel.x * (1. - k), rel.y * (1. - k));
        cx.notify();
    }

    fn chip(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        on: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(on, |d| d.bg(th.chip_bg).text_color(th.accent))
            .when(!on, |d| d.text_color(th.muted).hover(|s| s.bg(th.hover)))
            .child(label.into())
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let origin_title = self.origin.as_ref().and_then(|p| {
            self.app
                .read(cx)
                .store
                .page_row(p)
                .ok()
                .flatten()
                .map(|r| r.title)
        });
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(th.line)
            .child(div().font_weight(gpui::FontWeight::BOLD).child("Graph"))
            .child(
                self.chip("graph-all", "All pages", !self.local, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.set_local(false, cx))),
            )
            .when_some(origin_title, |d, t| {
                d.child(
                    self.chip("graph-local", format!("Around {t}"), self.local, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.set_local(true, cx))),
                )
            })
            .child(
                self.chip("graph-daily", "Journal pages", self.include_daily, cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.include_daily = !this.include_daily;
                        this.reload(cx);
                    })),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(th.faint).child(format!(
                "{} pages · {} links",
                self.graph.nodes.len(),
                self.graph.edges.len()
            )))
            .child(
                self.chip("graph-close", "Close", false, cx)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::Close))),
            );

        // Snapshot for the paint closure.
        let laid_out = self.laid_out;
        let screen: Vec<(f32, f32)> = self.pos.clone();
        let nodes = self.graph.nodes.clone();
        let edges = self.graph.edges.clone();
        let (pan, zoom, hover) = (self.pan, self.zoom, self.hover);
        let bounds_cell = self.bounds.clone();
        let center_page = self.local.then(|| self.origin.clone()).flatten();
        let font = window.text_style().font();
        let empty = nodes.is_empty();
        let body = div()
            .id("graph-canvas")
            .debug_selector(|| "graph-canvas".into())
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .when(self.hover.is_some(), |d| d.cursor_pointer())
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        bounds_cell.set(b);
                        if !laid_out {
                            return;
                        }
                        let s = GraphView::scale(b) * zoom;
                        let c = b.center();
                        let at = |p: (f32, f32)| {
                            point(c.x + px(p.0 * s) + pan.x, c.y + px(p.1 * s) + pan.y)
                        };
                        // Edges first, neighbours of the hovered node highlighted.
                        let mut faint = PathBuilder::stroke(px(1.));
                        let mut strong = PathBuilder::stroke(px(1.5));
                        let (mut any_faint, mut any_strong) = (false, false);
                        for &(a, bnode, _) in &edges {
                            let lit = hover.is_some_and(|h| h == a || h == bnode);
                            let pb = if lit { &mut strong } else { &mut faint };
                            pb.move_to(at(screen[a]));
                            pb.line_to(at(screen[bnode]));
                            if lit {
                                any_strong = true;
                            } else {
                                any_faint = true;
                            }
                        }
                        if any_faint {
                            if let Ok(p) = faint.build() {
                                window.paint_path(p, th.line);
                            }
                        }
                        if any_strong {
                            if let Ok(p) = strong.build() {
                                window.paint_path(p, th.accent);
                            }
                        }
                        // Nodes, then labels for the hovered node, the center and the
                        // best-connected pages (more as you zoom in).
                        let label_budget = ((30. * zoom * zoom) as usize).max(12);
                        for (i, n) in nodes.iter().enumerate() {
                            let r = GraphView::radius(n.degree);
                            let p = at(screen[i]);
                            let is_center = center_page.as_ref() == Some(&n.page);
                            let color = if hover == Some(i) || is_center {
                                th.accent
                            } else if n.daily {
                                th.tag
                            } else {
                                th.bullet
                            };
                            let nb = Bounds::centered_at(p, gpui::size(px(r * 2.), px(r * 2.)));
                            window.paint_quad(gpui::fill(nb, color).corner_radii(px(r)));
                            if hover == Some(i) || is_center || i < label_budget {
                                let text: SharedString = n.title.clone().into();
                                let run = TextRun {
                                    len: text.len(),
                                    font: font.clone(),
                                    color: if hover == Some(i) { th.text } else { th.muted },
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                let line =
                                    window.text_system().shape_line(text, px(12.), &[run], None);
                                let origin = point(p.x - line.width / 2., p.y + px(r + 2.));
                                let _ = line.paint(origin, px(16.), window, cx);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_1_2()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_color(th.faint)
                        .child("No links yet. Type [[ to link pages and they appear here."),
                )
            });
        div()
            .debug_selector(|| "graph-view".into())
            .key_context("GraphView")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(GraphEvent::Close);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(th.bg)
            .child(header)
            .child(body)
    }
}
