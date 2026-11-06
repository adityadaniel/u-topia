//! PDF viewer with highlights (P2). Pages scroll in a virtualized column; drag a
//! rectangle over a page to highlight it. Each highlight becomes a quote block (with the
//! text under the rectangle) beneath the block that embeds the PDF, so highlights are
//! searchable, referenceable and synced like everything else. Click a highlight to jump
//! to its block.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    canvas, div, list, prelude::*, px, Bounds, Context, EventEmitter, FocusHandle, Focusable,
    ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Window,
};
use utopia_core::{BlockId, PageId};
use utopia_store::PdfHighlight;

use crate::app_state::AppState;
use crate::pdf::{page_cache_path, PAGE_WIDTH};
use crate::theme::theme;

pub enum PdfEvent {
    Close,
    OpenBlock { page: PageId, block: BlockId },
}

pub struct PdfView {
    app: gpui::Entity<AppState>,
    hash: String,
    name: String,
    /// The block the PDF was opened from (new highlights go under it).
    host: Option<BlockId>,
    path: Option<PathBuf>,
    blob_dir: Option<PathBuf>,
    #[cfg(target_os = "macos")]
    doc: Option<crate::pdf::PdfDoc>,
    /// Page sizes in points.
    pages: Vec<(f32, f32)>,
    highlights: Vec<PdfHighlight>,
    list: ListState,
    /// `(page, start, current)` in window coordinates while dragging.
    drag: Option<(usize, Point<Pixels>, Point<Pixels>)>,
    /// Option-drag: highlight an area (a figure) instead of selecting text.
    area_drag: bool,
    /// Line rectangles of the text the current drag selects (live preview).
    preview: Vec<[f32; 4]>,
    page_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// Decoded page images, kept while the viewer is open.
    images: HashMap<usize, std::sync::Arc<gpui::RenderImage>>,
    focus: FocusHandle,
}

impl EventEmitter<PdfEvent> for PdfView {}

impl Focusable for PdfView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Retina-sharp page images.
const RENDER_SCALE: u32 = 2;

impl PdfView {
    pub fn new(
        app: gpui::Entity<AppState>,
        hash: String,
        name: String,
        start_page: Option<usize>,
        host: Option<BlockId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = app.read(cx).store.blob_path(&hash);
        let blob_dir = app.read(cx).store.blob_dir();
        #[cfg(target_os = "macos")]
        let doc = path.as_deref().and_then(crate::pdf::PdfDoc::open);
        #[cfg(target_os = "macos")]
        let pages: Vec<(f32, f32)> = doc
            .as_ref()
            .map(|d| (0..d.page_count()).filter_map(|i| d.page_size(i)).collect())
            .unwrap_or_default();
        #[cfg(not(target_os = "macos"))]
        let pages: Vec<(f32, f32)> = Vec::new();
        let list = ListState::new(pages.len(), ListAlignment::Top, px(1200.));
        if let Some(p) = start_page.filter(|p| *p < pages.len()) {
            list.scroll_to_reveal_item(p);
        }
        let mut v = Self {
            app,
            hash,
            name,
            host,
            path,
            blob_dir,
            #[cfg(target_os = "macos")]
            doc,
            pages,
            highlights: Vec::new(),
            list,
            drag: None,
            area_drag: false,
            preview: Vec::new(),
            page_bounds: Rc::default(),
            images: HashMap::new(),
            focus: cx.focus_handle(),
        };
        v.reload_highlights(cx);
        v
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn highlights(&self) -> &[PdfHighlight] {
        &self.highlights
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Where a page is drawn (after a frame), for tests.
    pub fn page_bounds(&self, page: usize) -> Option<Bounds<Pixels>> {
        self.page_bounds.borrow().get(&page).copied()
    }

    fn reload_highlights(&mut self, cx: &mut Context<Self>) {
        let hash = self.hash.clone();
        self.highlights = self.app.update(cx, |app, _| {
            let r = app.store.pdf_highlights(&hash);
            app.report(r).unwrap_or_default()
        });
        cx.notify();
    }

    /// A page's image: decoded from the PNG cache beside the blobs, rendering the page
    /// into it on first use.
    fn page_image(&mut self, i: usize) -> Option<std::sync::Arc<gpui::RenderImage>> {
        if let Some(img) = self.images.get(&i) {
            return Some(img.clone());
        }
        #[cfg(target_os = "macos")]
        {
            let dir = self.blob_dir.clone()?;
            let out = page_cache_path(&dir, &self.hash, i, RENDER_SCALE);
            let mut img = out
                .exists()
                .then(|| crate::pdf::load_page_image(&out))
                .flatten();
            if img.is_none() {
                if let Some(doc) = &self.doc {
                    if doc.render_png(i, RENDER_SCALE as f32, &out).is_ok() {
                        img = crate::pdf::load_page_image(&out);
                    }
                }
            }
            let img = img?;
            self.images.insert(i, img.clone());
            Some(img)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = i;
            None
        }
    }

    /// The text a drag from `from` to `to` selects, with one rectangle per line.
    fn selection(
        &self,
        page: usize,
        from: (f32, f32),
        to: (f32, f32),
    ) -> Option<(String, Vec<[f32; 4]>)> {
        #[cfg(target_os = "macos")]
        {
            self.doc.as_ref()?.select_text(page, from, to)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (page, from, to);
            None
        }
    }

    fn add(&mut self, page: usize, rects: &[[f32; 4]], quote: &str, cx: &mut Context<Self>) {
        let (hash, host) = (self.hash.clone(), self.host.clone());
        self.app.update(cx, |app, cx| {
            let r = app
                .store
                .add_pdf_highlight(&hash, host.as_ref(), page, rects, quote);
            if let Some((p, _)) = app.report(r) {
                app.changed(vec![p], true, cx);
            }
        });
        self.reload_highlights(cx);
    }

    /// Highlight the text selected from `from` to `to` (page-relative), like dragging
    /// across it in a PDF reader. Returns whether any text was there.
    pub fn highlight_text(
        &mut self,
        page: usize,
        from: (f32, f32),
        to: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        match self.selection(page, from, to) {
            Some((quote, rects)) => {
                self.add(page, &rects, &quote, cx);
                true
            }
            None => false,
        }
    }

    /// Highlight an area (a figure, or a scan without text); its block quotes whatever
    /// text lies inside.
    pub fn highlight_area(&mut self, page: usize, rect: [f32; 4], cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        let quote = self
            .doc
            .as_ref()
            .map(|d| d.text_in(page, rect))
            .unwrap_or_default();
        #[cfg(not(target_os = "macos"))]
        let quote = String::new();
        self.add(page, &[rect], &quote, cx);
    }

    fn rel(&self, page: usize, at: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.page_bounds.borrow().get(&page).copied()?;
        let x = f32::from(at.x - b.origin.x) / f32::from(b.size.width);
        let y = f32::from(at.y - b.origin.y) / f32::from(b.size.height);
        Some((x.clamp(0., 1.), y.clamp(0., 1.)))
    }

    fn finish_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.preview.clear();
        let Some((page, start, _)) = self.drag.take() else {
            return;
        };
        let (Some(a), Some(b)) = (self.rel(page, start), self.rel(page, at)) else {
            return;
        };
        let rect = [
            a.0.min(b.0),
            a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        ];
        // A click, not a drag.
        if rect[2] < 0.01 && rect[3] < 0.005 {
            cx.notify();
            return;
        }
        if !self.area_drag && self.highlight_text(page, a, b, cx) {
            return;
        }
        // No text under the drag (or Option held): an area highlight.
        if rect[2] >= 0.01 && rect[3] >= 0.005 {
            self.highlight_area(page, rect, cx);
        }
        cx.notify();
    }

    fn render_page(&mut self, i: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let th = theme(cx).clone();
        let (w, h) = self.pages[i];
        let height = PAGE_WIDTH * h / w.max(1.);
        let image = self.page_image(i);
        let bounds_map = self.page_bounds.clone();
        let mut page = div()
            .id(("pdf-page", i))
            .debug_selector(move || format!("pdf-page-{i}"))
            .relative()
            .w(px(PAGE_WIDTH))
            .h(px(height))
            .bg(gpui::white())
            .border_1()
            .border_color(th.line)
            .cursor_crosshair()
            .child(
                canvas(
                    move |b, _, _| {
                        bounds_map.borrow_mut().insert(i, b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when_some(image, |d, img| {
                d.child(gpui::img(gpui::ImageSource::Render(img)).size_full())
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.area_drag = e.modifiers.alt;
                    this.preview.clear();
                    this.drag = Some((i, e.position, e.position));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if let Some((p, start, _)) = this.drag {
                    if p == i && e.pressed_button == Some(MouseButton::Left) {
                        this.drag = Some((p, start, e.position));
                        if !this.area_drag {
                            this.preview = match (this.rel(p, start), this.rel(p, e.position)) {
                                (Some(a), Some(b)) => {
                                    this.selection(p, a, b).map(|s| s.1).unwrap_or_default()
                                }
                                _ => Vec::new(),
                            };
                        }
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, cx| this.finish_drag(e.position, cx)),
            );
        let yellow = th.pdf_highlight;
        for (k, hl) in self
            .highlights
            .iter()
            .enumerate()
            .filter(|(_, h)| h.pdf_page == i)
        {
            for (n, r) in hl.rects.iter().enumerate() {
                let (page_id, block) = (hl.page_id.clone(), hl.block.clone());
                page = page.child(
                    div()
                        .id(gpui::ElementId::NamedInteger(
                            "pdf-hl".into(),
                            (k * 1000 + n) as u64,
                        ))
                        .debug_selector(move || format!("pdf-hl-{k}-{n}"))
                        .absolute()
                        .left(px(r[0] * PAGE_WIDTH))
                        .top(px(r[1] * height))
                        .w(px(r[2] * PAGE_WIDTH))
                        .h(px(r[3] * height))
                        .bg(yellow)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(PdfEvent::OpenBlock {
                                page: page_id.clone(),
                                block: block.clone(),
                            })
                        })),
                );
            }
        }
        let previewing = self.drag.is_some_and(|d| d.0 == i) && !self.preview.is_empty();
        if previewing {
            for r in &self.preview {
                page = page.child(
                    div()
                        .absolute()
                        .left(px(r[0] * PAGE_WIDTH))
                        .top(px(r[1] * height))
                        .w(px(r[2] * PAGE_WIDTH))
                        .h(px(r[3] * height))
                        .bg(th.pdf_selection),
                );
            }
        }
        if let (Some((p, start, cur)), false) = (self.drag, previewing) {
            if p == i {
                if let Some(b) = self.page_bounds.borrow().get(&i).copied() {
                    let x0 = f32::from(start.x.min(cur.x) - b.origin.x);
                    let y0 = f32::from(start.y.min(cur.y) - b.origin.y);
                    page = page.child(
                        div()
                            .absolute()
                            .left(px(x0))
                            .top(px(y0))
                            .w(px(f32::from((start.x - cur.x).abs())))
                            .h(px(f32::from((start.y - cur.y).abs())))
                            .border_1()
                            .border_color(th.accent)
                            .bg(yellow),
                    );
                }
            }
        }
        div()
            .w_full()
            .flex()
            .justify_center()
            .py_2()
            .child(page)
            .into_any_element()
    }
}

impl Render for PdfView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let count = self.pages.len();
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(th.line)
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.faint)
                    .child(format!(
                        "{count} pages · {} highlights · drag over a page to highlight",
                        self.highlights.len()
                    )),
            )
            .child(div().flex_1())
            .when_some(self.path.clone(), |d, p| {
                d.child(
                    div()
                        .id("pdf-external")
                        .px_2()
                        .text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(th.accent))
                        .child("Open in Preview")
                        .on_click(move |_, _, cx| cx.open_with_system(&p)),
                )
            })
            .child(
                div()
                    .id("pdf-close")
                    .px_2()
                    .text_size(px(crate::theme::BODY_SIZE))
                    .cursor_pointer()
                    .hover(|s| s.text_color(th.accent))
                    .child("Close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PdfEvent::Close))),
            );
        let view = cx.entity();
        let body = if count == 0 {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .items_center()
                .text_color(th.faint)
                .child(if self.path.is_none() {
                    "This PDF has not synced to this device yet."
                } else {
                    "This PDF cannot be shown here; use Open in Preview."
                })
                .into_any_element()
        } else {
            list(self.list.clone(), move |i, _, cx| {
                view.update(cx, |this, cx| this.render_page(i, cx))
            })
            .flex_1()
            .into_any_element()
        };
        div()
            .debug_selector(|| "pdf-view".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(PdfEvent::Close);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(th.surface)
            .child(header)
            .child(body)
    }
}
