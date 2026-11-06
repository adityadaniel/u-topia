//! The main window (§9.2): left sidebar (journal, date picker, pages, storage status),
//! the main page, the right sidebar of stacked pages plus the reference inspector, quick
//! jump, and the new-day banner (FR-1.7).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::NaiveDate;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, EntityId, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, SharedString, Subscription, Task, Window,
};
use utopia_core::inline::{extract_refs, plain_text};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{JournalEntry, OutgoingRef, PageRow, RefTarget};

use crate::actions::*;
use crate::app_state::AppState;
use crate::editor::{BlockEditor, EditorEvent};
use crate::graph_view::{GraphEvent, GraphView};
use crate::page_view::{PageView, PageViewEvent};
use crate::pdf_view::{PdfEvent, PdfView};
use crate::quick_jump::{JumpPalette, PaletteMode, QuickJumpEvent};
use crate::sync_runner::{exchange_owned, status_line, BoxTransport, SyncRunner};
use crate::theme::{quiet_control, theme};

/// The reference panel's sub-tabs: which way the edges are looked at (FR-4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Incoming,
    Outgoing,
    Unresolved,
}

/// What the right sidebar's own tabs show (RS-3): the page's references, its headings, or
/// the pages pinned to stay reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelTab {
    References,
    Outline,
    Pinned,
}

impl PanelTab {
    /// Spelled in a `debug_selector`, so tests can name the panel they are looking at.
    fn id(self) -> &'static str {
        match self {
            PanelTab::References => "references",
            PanelTab::Outline => "outline",
            PanelTab::Pinned => "pinned",
        }
    }
}

/// One heading of the page in the main pane, for the Outline tab (RS-3).
#[derive(Debug, Clone)]
struct OutlineEntry {
    block: BlockId,
    /// `BlockKind::Heading(level)`: 1–3.
    level: u8,
    depth: usize,
    text: String,
}

#[derive(Debug, Clone)]
enum JumpMode {
    Open,
    Redirect { block: BlockId, key: String },
    Search,
    Capture,
    Extract { page: PageId, block: BlockId },
}

/// A page stacked in the right sidebar (RS-5): its view, whether its card is folded to the
/// header, and the title the header shows.
struct SidePane {
    view: Entity<PageView>,
    collapsed: bool,
    title: SharedString,
}

#[derive(Default)]
struct SidebarData {
    journal: Vec<JournalEntry>,
    pages: Vec<PageRow>,
    days: HashSet<NaiveDate>,
}

#[derive(Default)]
struct InspectorData {
    /// Linked References, grouped by source page; each group carries its blocks (RS-1).
    incoming: Vec<utopia_store::RefGroup>,
    outgoing: Vec<OutgoingRef>,
    /// Unresolved targets with a display title recovered from the source text.
    unresolved: Vec<(OutgoingRef, String)>,
    /// The page's headings in document order (RS-3).
    outline: Vec<OutlineEntry>,
}

pub struct Workspace {
    app: Entity<AppState>,
    editor: Entity<BlockEditor>,
    main: Entity<PageView>,
    panes: Vec<SidePane>,
    splits: Vec<Entity<PageView>>,
    right_open: bool,
    /// The right sidebar's width in points (RS-4), kept in `settings.json` (rule 9).
    right_width: f32,
    /// A drag of the sidebar's left edge: where it started and the width then.
    resize_from: Option<(f32, f32)>,
    /// Left sidebar and its month grid; local UI state (rule 9), kept in `local_state`.
    left_open: bool,
    calendar_open: bool,
    inspector_open: bool,
    tab: InspectorTab,
    /// Which of the sidebar's own tabs is showing (RS-3).
    panel_tab: PanelTab,
    /// Pages pinned in the Pinned tab, resolved to titles, and the ids they came from.
    pinned: Vec<(PageId, String)>,
    pinned_ids: Vec<String>,
    jump: Option<(Entity<JumpPalette>, JumpMode)>,
    /// The graph view, shown in place of the main page while open (P2).
    graph: Option<Entity<GraphView>>,
    /// The PDF viewer, shown in place of the main page while open (P2).
    pdf: Option<Entity<PdfView>>,
    sidebar: SidebarData,
    sidebar_seen: Option<u64>,
    inspector: InspectorData,
    inspector_seen: Option<(PageId, u64)>,
    month: NaiveDate,
    today: NaiveDate,
    new_day: bool,
    backup_dir: Option<PathBuf>,
    sync: Option<SyncRunner>,
    focus: FocusHandle,
    subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl crate::reference::RefRowHost for Workspace {
    /// A reference row was clicked: open its page with that block focused (§7). Shift
    /// stacks the page in this sidebar instead of replacing the main pane.
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(page, None, Some(block), shift, window, cx);
    }

    fn follow_reference(
        &mut self,
        target: crate::rich::LinkTarget,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_link(target, shift, window, cx);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| BlockEditor::new(app.clone(), cx));
        let main =
            cx.new(|cx| PageView::new(app.clone(), editor.clone(), false, false, window, cx));
        let today = app.read(cx).store.today();
        let backup_dir = app
            .read(cx)
            .store
            .path()
            .and_then(|p| p.parent().map(|d| d.join("backup")));
        let subs = vec![
            cx.subscribe_in(&main, window, Self::on_page_event),
            cx.subscribe_in(
                &editor,
                window,
                |this, editor, e: &EditorEvent, window, cx| {
                    // Escape, or the edited block disappearing, must not strand focus on an
                    // element that is no longer rendered (global shortcuts would stop working).
                    if matches!(e, EditorEvent::Blurred) && editor.read(cx).target.is_none() {
                        window.focus(&this.focus);
                    }
                },
            ),
            cx.observe(&app, |_, _, cx| cx.notify()),
        ];
        app.update(cx, |a, cx| a.install_quit_hook(cx));
        let flag = |key: &str, default: bool| {
            app.read(cx)
                .store
                .local_get(key)
                .ok()
                .flatten()
                .map_or(default, |v| v == "1")
        };
        let (left_open, calendar_open) = (flag("left_sidebar", true), flag("calendar", false));
        let mut me = Self {
            app,
            editor,
            main,
            panes: Vec::new(),
            splits: Vec::new(),
            right_open: false,
            right_width: crate::appearance::right_sidebar_width(cx),
            resize_from: None,
            left_open,
            calendar_open,
            inspector_open: true,
            tab: InspectorTab::Incoming,
            panel_tab: PanelTab::References,
            pinned: Vec::new(),
            pinned_ids: Vec::new(),
            jump: None,
            graph: None,
            pdf: None,
            sidebar: SidebarData::default(),
            sidebar_seen: None,
            inspector: InspectorData::default(),
            inspector_seen: None,
            month: today,
            today,
            new_day: false,
            backup_dir,
            sync: None,
            focus: cx.focus_handle(),
            subs,
            _tasks: Vec::new(),
        };
        me.spawn_timers(cx);
        me
    }

    fn spawn_timers(&mut self, cx: &mut Context<Self>) {
        // FR-1.7: today recomputes on rollover and time zone change; never auto-navigate.
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            let alive = this.update(cx, |this, cx| {
                let today = this.app.read(cx).store.today();
                if today != this.today {
                    this.today = today;
                    this.new_day = true;
                    this.sidebar_seen = None;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        }));
        // §15.3: rolling snapshots every 5 minutes, 6 generations.
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(300))
                .await;
            let alive = this.update(cx, |this, cx| {
                if let Some(dir) = this.backup_dir.clone() {
                    this.app.update(cx, |app, _| {
                        let r = app.store.backup(&dir, 6);
                        app.report(r);
                    });
                }
            });
            if alive.is_err() {
                break;
            }
        }));
    }

    /// Start background sync for `cfg`: relay or folder, optionally end-to-end encrypted,
    /// with realtime notices from a relay (§12). Local editing never waits on it.
    pub fn enable_sync(&mut self, cfg: crate::sync_runner::SyncConfig, cx: &mut Context<Self>) {
        let client = self.app.update(cx, |app, _| {
            utopia_sync::SyncEngine::new(&app.store).map(|e| e.client_id().to_string())
        });
        let transport = client.and_then(|c| crate::sync_runner::build_transport(&cfg, &c));
        match transport {
            Ok(t) => {
                let nudge = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                if let crate::sync_runner::SyncTarget::Relay { url, token } = &cfg.target {
                    crate::sync_runner::spawn_realtime(
                        url.clone(),
                        cfg.graph.clone(),
                        token.clone(),
                        nudge.clone(),
                    );
                }
                self.enable_sync_with(t, Some(nudge), cx);
            }
            Err(e) => {
                eprintln!("utopia: sync disabled: {e}");
                self.app.update(cx, |app, _| {
                    app.last_error = Some(format!("sync disabled: {e}"))
                });
            }
        }
    }

    /// Start background sync over an already-built transport (tests, custom transports).
    pub fn enable_sync_with(
        &mut self,
        transport: BoxTransport,
        nudge: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        cx: &mut Context<Self>,
    ) {
        let engine = self
            .app
            .update(cx, |app, _| utopia_sync::SyncEngine::new(&app.store));
        let engine = match engine {
            Ok(e) => e,
            Err(e) => {
                eprintln!("utopia: sync disabled: {e}");
                return;
            }
        };
        let mut runner = SyncRunner::new(engine, transport);
        if let Some(n) = nudge {
            runner.nudge = n;
        }
        self.sync = Some(runner);
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            let wait = this.update(cx, |this, _| {
                this.sync.as_ref().map(|s| (s.interval, s.nudge.clone()))
            });
            let Ok(Some((interval, nudge))) = wait else {
                break;
            };
            // Poll interval, cut short by a realtime notice.
            let mut waited = Duration::ZERO;
            while waited < interval {
                let step = Duration::from_millis(250).min(interval - waited);
                cx.background_executor().timer(step).await;
                waited += step;
                if nudge.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
            }
            // 1. Gather pushes on the UI thread (short store reads).
            let prepared = this.update(cx, |this, cx| {
                let Workspace { app, sync, .. } = this;
                let runner = sync.as_mut()?;
                let transport = runner.transport.take()?;
                let out = app.update(cx, |app, _| runner.engine.prepare(&mut app.store));
                match out {
                    Ok(out) => Some((out, transport)),
                    Err(e) => {
                        runner.transport = Some(transport);
                        runner.status = Some(utopia_sync::SyncStatus::from_result(
                            &Err(e),
                            &app.read(cx).store,
                        ));
                        None
                    }
                }
            });
            let Ok(prepared) = prepared else { break };
            let Some((out, transport)) = prepared else {
                continue;
            };
            // 2. Network on a background thread.
            let (result, transport) = cx
                .background_executor()
                .spawn(async move { exchange_owned(transport, out) })
                .await;
            // 3. Import on the UI thread and announce merges.
            let applied = this.update(cx, |this, cx| {
                let Workspace { app, sync, .. } = this;
                let Some(runner) = sync.as_mut() else { return };
                runner.transport = Some(transport);
                let report = match result {
                    Ok(inc) => app.update(cx, |app, cx| {
                        let r = runner.engine.apply(&mut app.store, inc);
                        if let Ok(rep) = &r {
                            if !rep.imported.is_empty() {
                                app.changed(rep.imported.clone(), true, cx);
                            }
                        }
                        r
                    }),
                    Err(e) => Err(e),
                };
                if let Ok(rep) = &report {
                    if !rep.imported.is_empty() {
                        let now = app.read(cx).store.now();
                        let n = rep.imported.len();
                        let pages = if n == 1 { "page" } else { "pages" };
                        runner.notice = Some((
                            format!("Merged changes from the relay into {n} {pages}"),
                            now,
                        ));
                    }
                }
                runner.interval = runner.next_interval(&report);
                runner.status = Some(utopia_sync::SyncStatus::from_result(
                    &report,
                    &app.read(cx).store,
                ));
                cx.notify();
            });
            if applied.is_err() {
                break;
            }
        }));
        cx.notify();
    }

    /// Split panes (P1): an editable page beside the main one (Cmd+Shift+Click). At most
    /// two splits; the oldest closes first.
    pub fn open_split(&mut self, page: PageId, window: &mut Window, cx: &mut Context<Self>) {
        let v = cx.new(|cx| {
            PageView::new(
                self.app.clone(),
                self.editor.clone(),
                true,
                true,
                window,
                cx,
            )
        });
        self.subs
            .push(cx.subscribe_in(&v, window, Self::on_page_event));
        v.update(cx, |v, cx| v.open(page, None, cx));
        if self.splits.len() >= 2 {
            self.splits.remove(0);
        }
        self.splits.push(v);
        cx.notify();
    }

    /// Current sync status, if sync is enabled.
    pub fn sync_status(&self) -> Option<utopia_sync::SyncStatus> {
        self.sync.as_ref().and_then(|s| s.status.clone())
    }

    /// The shared block editor (startup/latency probe).
    pub fn editor(&self) -> Entity<BlockEditor> {
        self.editor.clone()
    }

    /// The page shown in the main pane.
    pub fn main_page(&self, cx: &gpui::App) -> Option<PageId> {
        self.main.read(cx).page.clone()
    }

    /// The block the main pane is zoomed into, if any.
    pub fn main_zoom(&self, cx: &gpui::App) -> Option<BlockId> {
        self.main.read(cx).zoom.clone()
    }

    /// Pages open in split panes beside the main one.
    pub fn split_pages(&self, cx: &gpui::App) -> Vec<PageId> {
        self.splits
            .iter()
            .filter_map(|p| p.read(cx).page.clone())
            .collect()
    }

    /// The title a sidebar card shows for `page` (RS-5): the display title, from the shard,
    /// so a draft day and a renamed page both read right.
    fn pane_title(&self, page: &PageId, cx: &mut Context<Self>) -> SharedString {
        self.app
            .update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                app.store
                    .page_state(page)
                    .map(|st| utopia_store::display_title(st, &fmt))
                    .unwrap_or_default()
            })
            .into()
    }

    /// Pages pinned in the sidebar's Pinned tab, in the order they show (RS-3).
    pub fn pinned_pages(&self, _cx: &gpui::App) -> Vec<PageId> {
        self.pinned.iter().map(|(p, _)| p.clone()).collect()
    }

    /// Pages stacked in the right sidebar, top first.
    pub fn sidebar_pages(&self, cx: &gpui::App) -> Vec<PageId> {
        self.panes
            .iter()
            .filter_map(|p| p.view.read(cx).page.clone())
            .collect()
    }

    /// Open the configured launch page (FR-1.1): today by default, or the last page.
    pub fn open_launch_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (mode, last) = {
            let s = &self.app.read(cx).store;
            (
                s.local_get("launch")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "today".into()),
                s.local_get("last_page").ok().flatten(),
            )
        };
        if mode == "last" {
            if let Some(p) = last.map(PageId) {
                if self
                    .app
                    .read(cx)
                    .store
                    .page_row(&p)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.is_live())
                {
                    self.open(p, None, None, false, window, cx);
                    return;
                }
            }
        }
        self.open_journal(window, cx);
    }

    fn on_page_event(
        &mut self,
        view: &Entity<PageView>,
        e: &PageViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            PageViewEvent::Open {
                page,
                zoom,
                focus,
                sidebar,
            } => self.open(
                page.clone(),
                zoom.clone(),
                focus.clone(),
                *sidebar,
                window,
                cx,
            ),
            PageViewEvent::Close => {
                self.panes.retain(|p| &p.view != view);
                self.splits.retain(|p| p != view);
                cx.notify();
            }
            PageViewEvent::OpenSplit { page } => self.open_split(page.clone(), window, cx),
            PageViewEvent::OpenPdf {
                hash,
                name,
                page,
                host,
            } => self.open_pdf(hash.clone(), name.clone(), *page, host.clone(), window, cx),
            PageViewEvent::RequestExtract { page, block } => {
                self.start_jump(
                    JumpMode::Extract {
                        page: page.clone(),
                        block: block.clone(),
                    },
                    window,
                    cx,
                );
            }
        }
    }

    pub fn open(
        &mut self,
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        if sidebar {
            // Shift-click stacks the page beside the current one (§0, W6).
            let existing = self.panes.iter().position(|p| {
                p.view.read(cx).page.as_ref() == Some(&page) && p.view.read(cx).zoom == zoom
            });
            let pane = match existing {
                // Re-opening moves the card back to the top but keeps it folded if it was.
                Some(i) => self.panes.remove(i),
                None => {
                    let v = cx.new(|cx| {
                        PageView::new(
                            self.app.clone(),
                            self.editor.clone(),
                            true,
                            false,
                            window,
                            cx,
                        )
                    });
                    self.subs
                        .push(cx.subscribe_in(&v, window, Self::on_page_event));
                    v.update(cx, |v, cx| v.open(page.clone(), zoom.clone(), cx));
                    SidePane {
                        view: v,
                        collapsed: false,
                        title: self.pane_title(&page, cx),
                    }
                }
            };
            self.panes.insert(0, pane);
            self.right_open = true;
        } else {
            self.pdf = None;
            self.main
                .update(cx, |m, cx| m.open(page.clone(), zoom.clone(), cx));
            let _ = self
                .app
                .read(cx)
                .store
                .local_set("last_page", page.as_str());
            // From the shard: a draft day has no index row yet.
            let title = self.app.update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                app.store
                    .page_state(&page)
                    .map(|st| utopia_store::display_title(st, &fmt))
                    .unwrap_or_default()
            });
            window.set_window_title(&format!("{title} — Utopia"));
            match focus {
                Some(b) => self
                    .main
                    .update(cx, |m, cx| m.focus_block(&b, None, window, cx)),
                None => window.focus(&self.focus),
            }
        }
        cx.notify();
    }

    // ---------------------------------------------------------------- actions

    fn quick_jump(&mut self, _: &QuickJump, window: &mut Window, cx: &mut Context<Self>) {
        self.start_jump(JumpMode::Open, window, cx);
    }

    fn start_jump(&mut self, mode: JumpMode, window: &mut Window, cx: &mut Context<Self>) {
        let (title, palette) = match mode {
            JumpMode::Open => ("Quick jump", PaletteMode::Pages),
            JumpMode::Redirect { .. } => ("Redirect link to…", PaletteMode::Pages),
            JumpMode::Search => ("Search", PaletteMode::Search),
            JumpMode::Capture => ("Quick capture → today", PaletteMode::Capture),
            JumpMode::Extract { .. } => ("Extract block to page", PaletteMode::Text),
        };
        let app = self.app.clone();
        let jump = cx.new(|cx| JumpPalette::with_mode(app, title, palette, window, cx));
        self.subs.push(cx.subscribe_in(
            &jump,
            window,
            |this, _, e: &QuickJumpEvent, window, cx| {
                let mode = this.jump.take().map(|(_, m)| m);
                match (e, mode) {
                    (
                        QuickJumpEvent::Chosen {
                            page,
                            sidebar,
                            focus,
                        },
                        Some(JumpMode::Open | JumpMode::Search),
                    ) => this.open(page.clone(), None, focus.clone(), *sidebar, window, cx),
                    (
                        QuickJumpEvent::Chosen { page, .. },
                        Some(JumpMode::Redirect { block, key }),
                    ) => {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.redirect_intent(&block, &key, page);
                            app.report(r);
                            app.changed(Vec::new(), true, cx);
                        });
                    }
                    (QuickJumpEvent::Text(text), Some(JumpMode::Capture)) => {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.capture(text);
                            if let Some((page, _)) = app.report(r) {
                                app.changed(vec![page], true, cx);
                            }
                        });
                    }
                    (QuickJumpEvent::Text(text), Some(JumpMode::Extract { page, block })) => {
                        let dst = this.app.update(cx, |app, cx| {
                            let r = app.store.extract_block_to_page(&page, &block, text);
                            let d = app.report(r);
                            app.changed(vec![page.clone()], true, cx);
                            d
                        });
                        if let Some(dst) = dst {
                            this.open(dst, None, None, true, window, cx);
                        }
                    }
                    _ => {}
                }
                if this.editor.read(cx).target.is_none() {
                    window.focus(&this.focus);
                }
                cx.notify();
            },
        ));
        self.jump = Some((jump, mode));
        cx.notify();
    }

    fn full_search(&mut self, _: &FullSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.start_jump(JumpMode::Search, window, cx);
    }

    /// Cmd+Shift+G: show or hide the graph view (P2).
    pub fn show_graph(&mut self, _: &ShowGraph, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        self.editor.update(cx, |e, cx| e.end(cx));
        let origin = self.main.read(cx).page.clone();
        let view = cx.new(|cx| GraphView::new(self.app.clone(), origin, cx));
        self.subs.push(
            cx.subscribe_in(&view, window, |this, _, e: &GraphEvent, window, cx| {
                this.graph = None;
                match e {
                    GraphEvent::Open { page, sidebar } => {
                        this.open(page.clone(), None, None, *sidebar, window, cx)
                    }
                    GraphEvent::Close => window.focus(&this.focus),
                }
                cx.notify();
            }),
        );
        window.focus(&view.focus_handle(cx));
        self.graph = Some(view);
        cx.notify();
    }

    /// Open a PDF attachment in the viewer, optionally at a page (0-based).
    pub fn open_pdf(
        &mut self,
        hash: String,
        name: String,
        page: Option<usize>,
        host: Option<BlockId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.graph = None;
        let view = cx.new(|cx| PdfView::new(self.app.clone(), hash, name, page, host, cx));
        self.subs.push(
            cx.subscribe_in(&view, window, |this, _, e: &PdfEvent, window, cx| {
                match e {
                    PdfEvent::Close => {
                        this.pdf = None;
                        window.focus(&this.focus);
                    }
                    // The PDF stays open; the highlight's block opens beside it.
                    PdfEvent::OpenBlock { page, block } => {
                        this.open(page.clone(), Some(block.clone()), None, true, window, cx)
                    }
                }
                cx.notify();
            }),
        );
        window.focus(&view.focus_handle(cx));
        self.pdf = Some(view);
        cx.notify();
    }

    pub fn pdf_view(&self) -> Option<Entity<PdfView>> {
        self.pdf.clone()
    }

    /// Files dropped on the window become attachments: at the caret when editing, else a
    /// new block at the end of the open page.
    pub fn drop_files(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .editor
            .update(cx, |e, cx| e.attach_paths(paths, window, cx))
        {
            return;
        }
        let Some(page) = self.main.read(cx).page.clone() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let markup: Vec<String> = paths
                .iter()
                .filter_map(|p| {
                    let r = app.store.attach_file(p);
                    app.report(r)
                })
                .map(|a| utopia_core::attach::markup(&a.name, &a.hash, &a.mime))
                .collect();
            if markup.is_empty() {
                return;
            }
            let r = (|| {
                let st = app.store.page_state(&page)?;
                let roots = st.live_roots();
                let reuse = match roots.as_slice() {
                    [only] => st.block(only).is_some_and(|b| b.text.is_empty()),
                    _ => false,
                };
                let block = if reuse {
                    roots[0].clone()
                } else {
                    let plan = utopia_core::outline::append_block(st, None);
                    match app.store.execute_plan(&page, plan, None)? {
                        Some(f) => f.block,
                        None => return Ok(()),
                    }
                };
                app.store
                    .set_block_text(&page, &block, &markup.join(" "), None, None)?;
                app.store.commit_block(&page, &block).map(|_| ())
            })();
            app.report(r);
            app.changed(vec![page.clone()], true, cx);
        });
    }

    pub fn graph_view(&self) -> Option<Entity<GraphView>> {
        self.graph.clone()
    }

    /// Full-text search with `text` already typed (`utopia://search`).
    pub fn search_for(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.full_search(&FullSearch, window, cx);
        if let Some((jump, _)) = &self.jump {
            let input = jump.read(cx).input.clone();
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }

    /// Open the page a title names (dates and aliases resolve like links), creating it if
    /// it does not exist (`utopia://page`).
    pub fn open_title(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.app.update(cx, |app, cx| {
            let s = &mut app.store;
            let found =
                utopia_store::resolve_page_key(s.conn(), &normalize_title(title), s.date_format());
            let r = match found {
                Ok(Some(p)) => Ok(p),
                Ok(None) => {
                    let r = s.create_page(title);
                    app.changed(Vec::new(), true, cx);
                    r
                }
                Err(e) => Err(e),
            };
            app.report(r)
        });
        if let Some(page) = page {
            self.open(page, None, None, false, window, cx);
        }
    }

    pub fn app(&self) -> &Entity<AppState> {
        &self.app
    }

    /// Append a line to today's page (`utopia://capture`).
    pub fn capture_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let r = app.store.capture(text);
            if let Some((page, _)) = app.report(r) {
                app.changed(vec![page], true, cx);
            }
        });
    }

    /// Open the page holding a block, zoomed into it (`utopia://block`).
    pub fn open_block(&mut self, uid: &str, window: &mut Window, cx: &mut Context<Self>) {
        let block = BlockId(uid.to_string());
        let page = self.app.update(cx, |app, _| {
            let r = app.store.block_row(&block);
            app.report(r).flatten().map(|b| b.page_id)
        });
        if let Some(page) = page {
            self.open(page, Some(block), None, false, window, cx);
        }
    }

    /// Quick capture (FR-2.19): from anywhere in the app, and from the system-wide hotkey.
    pub fn quick_capture(&mut self, _: &QuickCapture, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.start_jump(JumpMode::Capture, window, cx);
    }

    /// Daily Note (§9.1): the whole feed, which is also the launch view.
    fn go_daily_notes(&mut self, _: &GoDailyNotes, window: &mut Window, cx: &mut Context<Self>) {
        self.open_journal(window, cx);
    }

    /// Today (§9.1): today's page on its own, not the feed. A day stays a draft until it is
    /// written (FR-1.2), so this opens the draft rather than creating the page.
    fn go_today(&mut self, _: &GoToday, window: &mut Window, cx: &mut Context<Self>) {
        let today = self.app.read(cx).store.today();
        self.today = today;
        self.open_date(today, false, true, window, cx);
    }

    /// Yesterday (§9.1). `pred_opt` only returns `None` at the start of the proleptic
    /// calendar, where staying put is the sane answer.
    fn go_yesterday(&mut self, _: &GoYesterday, window: &mut Window, cx: &mut Context<Self>) {
        let today = self.app.read(cx).store.today();
        self.today = today;
        let d = today.pred_opt().unwrap_or(today);
        self.open_date(d, false, true, window, cx);
    }

    /// The Daily Notes feed in the main pane, caret in today's first block (FR-1.1).
    /// Today is a draft until it is written (FR-1.2); older days only if they exist.
    pub fn open_journal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_day = false;
        let today = self.app.read(cx).store.today();
        self.today = today;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(today);
            let p = app.report(r);
            app.changed(Vec::new(), true, cx);
            p
        });
        let Some(page) = page else { return };
        self.editor.update(cx, |e, cx| e.end(cx));
        self.graph = None;
        self.pdf = None;
        self.main
            .update(cx, |m, cx| m.open_journal(page.clone(), cx));
        let _ = self
            .app
            .read(cx)
            .store
            .local_set("last_page", page.as_str());
        window.set_window_title("Daily Notes — Utopia");
        self.main.update(cx, |m, cx| m.focus_first(window, cx));
        cx.notify();
    }

    /// Days shown in the main pane's Daily Notes feed, newest first (empty otherwise).
    pub fn journal_days(&self, cx: &gpui::App) -> Vec<PageId> {
        self.main.read(cx).journal_days()
    }

    pub fn open_date(
        &mut self,
        d: NaiveDate,
        sidebar: bool,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(d);
            let p = app.report(r);
            app.changed(Vec::new(), true, cx);
            p
        });
        if let Some(page) = page {
            self.open(page, None, None, sidebar, window, cx);
            if focus && !sidebar {
                self.main.update(cx, |m, cx| m.focus_first(window, cx));
            }
        }
    }

    fn current_date(&self, cx: &mut Context<Self>) -> NaiveDate {
        let page = self.main.read(cx).page.clone();
        page.and_then(|p| {
            self.app
                .update(cx, |app, _| app.store.page_state(&p).ok()?.meta.daily_date)
        })
        .unwrap_or(self.today)
    }

    fn prev_day(&mut self, _: &PrevDay, window: &mut Window, cx: &mut Context<Self>) {
        let d = self.current_date(cx);
        self.open_date(d.pred_opt().unwrap_or(d), false, false, window, cx);
    }

    fn next_day(&mut self, _: &NextDay, window: &mut Window, cx: &mut Context<Self>) {
        let d = self.current_date(cx);
        self.open_date(d.succ_opt().unwrap_or(d), false, false, window, cx);
    }

    fn toggle_right(
        &mut self,
        _: &ToggleRightSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_open {
            self.hide_right(window, cx);
        } else {
            self.right_open = true;
            cx.notify();
        }
    }

    /// End the edit if `host` owns it and hand the keyboard back to the workspace. Hiding a
    /// card removes its `PageView` from the window, and a focused element that is not
    /// rendered gets no keystrokes, so every shortcut would die (AGENTS.md, §13.4).
    fn end_edit_in(&mut self, host: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let owned = self
            .editor
            .read(cx)
            .target
            .as_ref()
            .is_some_and(|t| t.host == host);
        if owned {
            self.editor.update(cx, |e, cx| e.end(cx));
            window.focus(&self.focus);
        }
    }

    /// Close the sidebar, first ending any edit a stacked card hosts — the same invariant as
    /// folding one card (RS-4, RS-5).
    fn hide_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosts: Vec<EntityId> = self.panes.iter().map(|p| p.view.entity_id()).collect();
        for host in hosts {
            self.end_edit_in(host, window, cx);
        }
        self.right_open = false;
        cx.notify();
    }

    /// Dragging the sidebar's left edge (RS-4). The window-level move listener runs wherever
    /// the pointer is, so dragging past the edge keeps the width following the pointer.
    fn resize_move(&mut self, e: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((start_x, start_width)) = self.resize_from else {
            return;
        };
        if e.pressed_button.is_none() {
            self.resize_from = None;
            return;
        }
        let width = crate::appearance::clamp_sidebar_width(
            start_width + (start_x - f32::from(e.position.x)),
        );
        if (width - self.right_width).abs() >= 1. {
            self.right_width = width;
            crate::appearance::set_right_sidebar_width(cx, width);
        }
    }

    /// The drag ended: remember the width in `settings.json` (rule 9). One write per drag,
    /// not one per pixel.
    fn resize_done(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resize_from.take().is_none() {
            return;
        }
        self.right_width = crate::appearance::right_sidebar_width(cx);
        if let Err(e) = crate::appearance::remember(cx) {
            eprintln!("utopia: settings.json: {e}");
        }
        cx.notify();
    }

    fn toggle_left(&mut self, _: &ToggleLeftSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.left_open = !self.left_open;
        let _ = (self.app.read(cx).store).local_set("left_sidebar", flag(self.left_open));
        cx.notify();
    }

    fn toggle_calendar(&mut self, cx: &mut Context<Self>) {
        self.calendar_open = !self.calendar_open;
        let _ = (self.app.read(cx).store).local_set("calendar", flag(self.calendar_open));
        cx.notify();
    }

    pub fn left_sidebar_open(&self) -> bool {
        self.left_open
    }

    pub fn calendar_open(&self) -> bool {
        self.calendar_open
    }

    fn toggle_refs(&mut self, _: &ToggleReferences, _: &mut Window, cx: &mut Context<Self>) {
        if self.right_open && self.inspector_open {
            self.inspector_open = false;
        } else {
            self.right_open = true;
            self.inspector_open = true;
        }
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.history(true, window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.history(false, window, cx);
    }

    /// Undo/redo while no block is being edited; the caret goes back into the main pane.
    fn history(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        if let Some(c) = caret {
            self.main
                .update(cx, |m, cx| m.restore_caret(&c, window, cx));
        }
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.app.update(cx, |app, _| {
            let r = app.store.sync_index();
            app.report(r);
        });
        cx.quit();
    }

    fn pick_date(
        &mut self,
        d: NaiveDate,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_date(d, sidebar, !sidebar, window, cx);
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.month = crate::calendar::shift_month(self.month, delta);
        cx.notify();
    }

    // ---------------------------------------------------------------- data

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let v = self.app.read(cx).refs_version;
        // RS-4: the width lives in the preferences, so a reset from Settings (or another
        // window's drag) reaches this sidebar too. Never while this one is dragging.
        if self.resize_from.is_none() {
            self.right_width = crate::appearance::right_sidebar_width(cx);
        }
        // Pinned pages are a local preference (rule 9); re-read when it changes.
        if crate::appearance::pinned_pages(cx) != self.pinned_ids {
            self.load_pinned(cx);
        }
        if self.sidebar_seen != Some(v) {
            self.sidebar_seen = Some(v);
            let store = &self.app.read(cx).store;
            let all = store.list_pages(true).unwrap_or_default();
            self.sidebar = SidebarData {
                journal: store.journal(14).unwrap_or_default(),
                days: all.iter().filter_map(|p| p.daily_date).collect(),
                pages: all.into_iter().filter(|p| !p.is_daily()).collect(),
            };
            // Card headers name their page (RS-5); a rename must reach them.
            let pages: Vec<Option<PageId>> = self
                .panes
                .iter()
                .map(|p| p.view.read(cx).page.clone())
                .collect();
            let titles: Vec<SharedString> = self.app.update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                pages
                    .into_iter()
                    .map(|page| {
                        page.and_then(|p| {
                            app.store
                                .page_state(&p)
                                .ok()
                                .map(|st| utopia_store::display_title(st, &fmt))
                        })
                        .unwrap_or_default()
                        .into()
                    })
                    .collect()
            });
            for (pane, title) in self.panes.iter_mut().zip(titles) {
                pane.title = title;
            }
        }
        let Some(page) = self.main.read(cx).page.clone() else {
            return;
        };
        if self.inspector_seen.as_ref() != Some(&(page.clone(), v)) {
            self.inspector_seen = Some((page.clone(), v));
            // RS-3's Outline: the page's headings in document order, so a long page is
            // navigable from the panel. Read from the shard, never resolved at paint time.
            let outline = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&page)
                    .map(|st| {
                        st.live_preorder()
                            .into_iter()
                            .filter_map(|(id, depth)| {
                                let b = st.block(&id)?;
                                match b.kind {
                                    utopia_core::BlockKind::Heading(level) => Some(OutlineEntry {
                                        block: id,
                                        level: level.clamp(1, 3),
                                        depth,
                                        text: plain_text(&b.text),
                                    }),
                                    _ => None,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
            let store = &self.app.read(cx).store;
            let incoming = store
                .linked_references(&page, &Default::default())
                .unwrap_or_default()
                .groups;
            let outgoing = store.outgoing_refs(&page, None).unwrap_or_default();
            let unresolved = store
                .unresolved_refs(&page)
                .unwrap_or_default()
                .into_iter()
                .map(|o| {
                    let label = match &o.target {
                        RefTarget::UnresolvedPage { key } => store
                            .block_row(&o.from_block)
                            .ok()
                            .flatten()
                            .and_then(|b| {
                                extract_refs(&b.text)
                                    .into_iter()
                                    .find(|r| {
                                        r.is_page_like() && normalize_title(&r.target) == *key
                                    })
                                    .map(|r| r.target)
                            })
                            .unwrap_or_else(|| key.clone()),
                        RefTarget::Block { id, .. } => format!("(({id}))"),
                        RefTarget::Page { title, .. } => title.clone(),
                    };
                    (o, label)
                })
                .collect();
            self.inspector = InspectorData {
                incoming,
                outgoing,
                unresolved,
                outline,
            };
        }
    }

    // ---------------------------------------------------------------- rendering

    fn nav_item(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        page: Option<PageId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let main = self.main.read(cx);
        let active = page.is_some() && !main.is_journal() && main.page == page;
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(crate::theme::BODY_SIZE))
            .text_color(if active { th.accent } else { th.text })
            .when(active, |d| d.bg(th.hover))
            .hover(|s| s.bg(th.hover))
            .child(label.into())
            .when_some(page, |d, page| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        this.open(page.clone(), None, None, e.modifiers.shift, window, cx)
                    }),
                )
            })
    }

    fn render_left(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let today = self.today;
        let section = |title: &'static str| {
            div()
                .mt_3()
                .px_2()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.muted)
                .child(title)
        };
        let day_link = |this: &Self,
                        id: &'static str,
                        label: &'static str,
                        d: NaiveDate,
                        cx: &mut Context<Self>| {
            this.nav_item(id, label, None, cx).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.new_day = false;
                    this.open_date(d, e.modifiers.shift, !e.modifiers.shift, window, cx)
                }),
            )
        };
        let mut col = div()
            .debug_selector(|| "left-sidebar".into())
            .w(px(250.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(th.sidebar_bg)
            .border_r_1()
            .border_color(th.line)
            .p_2()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_2()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Utopia"))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(quiet_control("open-jump", "⌘K", &th).on_click(cx.listener(
                                |this, _, window, cx| this.start_jump(JumpMode::Open, window, cx),
                            )))
                            .child(
                                quiet_control("collapse-left", "«", &th)
                                    .debug_selector(|| "collapse-left".into())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_left(&ToggleLeftSidebar, window, cx)
                                    })),
                            ),
                    ),
            )
            .child(section("JOURNAL"))
            .child({
                let active = self.main.read(cx).is_journal();
                self.nav_item("daily-notes", "Daily Notes", None, cx)
                    .when(active, |d| d.text_color(th.accent).bg(th.hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_journal(window, cx)),
                    )
            })
            .child(day_link(self, "today", "Today", today, cx))
            .child(day_link(
                self,
                "yesterday",
                "Yesterday",
                today.pred_opt().unwrap_or(today),
                cx,
            ))
            .child(day_link(
                self,
                "tomorrow",
                "Tomorrow",
                today.succ_opt().unwrap_or(today),
                cx,
            ))
            .child(
                self.nav_item(
                    "toggle-calendar",
                    if self.calendar_open {
                        "▾ Calendar"
                    } else {
                        "▸ Calendar"
                    },
                    None,
                    cx,
                )
                .text_color(th.muted)
                .debug_selector(|| "toggle-calendar".into())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_calendar(cx))),
            )
            .when(self.calendar_open, |col| {
                col.child(
                    div()
                        .debug_selector(|| "left-calendar".into())
                        .px_1()
                        .py_2()
                        .child(crate::calendar::render(
                            cx,
                            self.month,
                            today,
                            &self.sidebar.days,
                            Self::pick_date,
                            Self::shift_month,
                        )),
                )
            });
        let mut scroll = div()
            .id("left-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        scroll = scroll.child(section("RECENT DAYS"));
        let journal: Vec<(PageId, String, Vec<String>)> = self
            .sidebar
            .journal
            .iter()
            .map(|j| (j.page.id.clone(), j.page.title.clone(), j.preview.clone()))
            .collect();
        for (i, (id, title, preview)) in journal.into_iter().enumerate() {
            let item = self.nav_item(("journal", i), title, Some(id), cx).child(
                div()
                    .flex()
                    .flex_col()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .children(
                        preview
                            .into_iter()
                            .take(2)
                            .map(|p| p.chars().take(48).collect::<String>()),
                    ),
            );
            scroll = scroll.child(item);
        }
        scroll = scroll.child(section("PAGES"));
        let pages: Vec<(PageId, String)> = self
            .sidebar
            .pages
            .iter()
            .map(|p| (p.id.clone(), p.title.clone()))
            .collect();
        if pages.is_empty() {
            scroll = scroll.child(
                div()
                    .px_2()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.faint)
                    .child("Type [[ or # to make a page."),
            );
        }
        for (i, (id, title)) in pages.into_iter().enumerate() {
            scroll = scroll.child(self.nav_item(("page", i), title, Some(id), cx));
        }
        col = col.child(scroll);
        let (file, err, notice) = {
            let app = self.app.read(cx);
            (
                app.store
                    .path()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "in-memory graph".into()),
                app.last_error.clone(),
                app.notice.clone(),
            )
        };
        col.child(
            div()
                .pt_2()
                .px_2()
                .border_t_1()
                .border_color(th.line)
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child(format!("Local · {file}"))
                .child(match &self.sync {
                    None => div().child("Sync: off (local only)"),
                    Some(runner) => {
                        let (line, warn) =
                            status_line(runner.status.as_ref(), self.app.read(cx).store.now());
                        div().when(warn, |d| d.text_color(th.warning)).child(line)
                    }
                })
                .when_some(notice, |d, n| d.child(div().text_color(th.text).child(n)))
                .when_some(err, |d, e| d.child(div().text_color(th.danger).child(e))),
        )
        .into_any_element()
    }

    /// The muted line a panel shows when it has nothing to list (RS-2, RS-3). `id` names
    /// it for `debug_selector`, so a test can tell the empty state from a missing panel.
    fn panel_empty(
        &self,
        id: &'static str,
        text: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        div()
            .debug_selector(move || id.to_string())
            .px_2()
            .py_1()
            .text_size(px(crate::theme::CAPTION_SIZE))
            .text_color(th.faint)
            .child(text)
            .into_any_element()
    }

    /// Follow a page or block chip clicked in the right sidebar: a page opens in the main
    /// pane, a block opens its page zoomed to it. An unresolved title is created, exactly as
    /// following it from the outline does (FR-4.14). Local UI state only — nothing here
    /// writes a shard beyond that page creation.
    fn follow_link(
        &mut self,
        target: crate::rich::LinkTarget,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            crate::rich::LinkTarget::Page(title) => {
                let page = self.app.update(cx, |app, cx| {
                    let found = app.store.find_page(&title).ok().flatten();
                    match found {
                        Some(p) => Some(p),
                        None => {
                            let r = app.store.create_page(&title);
                            let p = app.report(r);
                            app.changed(Vec::new(), true, cx);
                            p
                        }
                    }
                });
                if let Some(page) = page {
                    self.open(page, None, None, sidebar, window, cx);
                }
            }
            crate::rich::LinkTarget::Block(uid) => {
                if let Ok(Some(b)) = self.app.read(cx).store.block_row(&BlockId(uid)) {
                    self.open(b.page_id, Some(b.id), None, sidebar, window, cx);
                }
            }
            // Attachments and URLs are the page's business, not the panel's.
            crate::rich::LinkTarget::Url(_) => {}
        }
    }

    /// The right sidebar's reference panel: three tabs (RS-3) over the page's references,
    /// its headings, or the pages pinned to stay reachable.
    fn render_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let panel_tab = |this: &Self,
                         id: &'static str,
                         t: PanelTab,
                         label: &'static str,
                         cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_size(px(crate::theme::CONTROL_SIZE))
                .cursor_pointer()
                .text_color(if this.panel_tab == t {
                    th.accent
                } else {
                    th.muted
                })
                .when(this.panel_tab == t, |d| d.bg(th.hover))
                .hover(|d| d.bg(th.hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel_tab = t;
                    cx.notify();
                }))
        };
        let header = div()
            .flex()
            .items_center()
            .gap_1()
            .child(panel_tab(
                self,
                "panel-references",
                PanelTab::References,
                "References",
                cx,
            ))
            .child(panel_tab(
                self,
                "panel-outline",
                PanelTab::Outline,
                "Outline",
                cx,
            ))
            .child(panel_tab(
                self,
                "panel-pinned",
                PanelTab::Pinned,
                "Pinned",
                cx,
            ));
        let body = match self.panel_tab {
            PanelTab::References => self.render_references(cx),
            PanelTab::Outline => self.render_outline(cx),
            PanelTab::Pinned => self.render_pinned(cx),
        };
        // With no pages stacked above it, the panel is the sidebar and fills it. With
        // pages, it keeps a fixed share at the bottom so both stay usable.
        let only_panel = self.panes.is_empty();
        div()
            .debug_selector(|| format!("panel-{}", self.panel_tab.id()))
            .when(only_panel, |d| d.flex_1().min_h_0())
            .when(!only_panel, |d| d.h(px(260.)).flex_none())
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(th.line)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_references(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let tab = |this: &Self,
                   id: &'static str,
                   t: InspectorTab,
                   label: String,
                   cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px_2()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .rounded_sm()
                .cursor_pointer()
                .text_color(if this.tab == t { th.accent } else { th.muted })
                .when(this.tab == t, |d| d.bg(th.hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    cx.notify();
                }))
        };
        let n_in: usize = self.inspector.incoming.iter().map(|g| g.hits.len()).sum();
        let tabs = div()
            .flex()
            .gap_1()
            .child(tab(
                self,
                "tab-in",
                InspectorTab::Incoming,
                format!("Incoming {n_in}"),
                cx,
            ))
            .child(tab(
                self,
                "tab-out",
                InspectorTab::Outgoing,
                format!("Outgoing {}", self.inspector.outgoing.len()),
                cx,
            ))
            .child(tab(
                self,
                "tab-un",
                InspectorTab::Unresolved,
                format!("Unresolved {}", self.inspector.unresolved.len()),
                cx,
            ));
        let mut body = div()
            .id("inspector-body")
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1();
        match self.tab {
            InspectorTab::Incoming => {
                // One group per source page, its count next to the name (LR-2), and below
                // it each referencing block drawn the way Linked References draws it (RS-1).
                let groups: Vec<utopia_store::RefGroup> = self.inspector.incoming.clone();
                let me = cx.entity();
                if groups.is_empty() {
                    body =
                        body.child(self.panel_empty("references-empty", "No references yet.", cx));
                }
                let mut ix = 0usize;
                for (gi, group) in groups.iter().enumerate() {
                    let page = group.page.id.clone();
                    let header = div()
                        .id(("ref-group", gi))
                        .debug_selector(move || format!("ref-group-{gi}"))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .pt_2()
                        .pb_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|d| d.bg(th.hover))
                        .child(
                            div()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(th.muted)
                                .child(group.page.title.clone()),
                        )
                        .child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(th.chip_bg)
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .text_color(th.muted)
                                .child(group.hits.len().to_string()),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                this.open(page.clone(), None, None, e.modifiers.shift, window, cx)
                            }),
                        );
                    body = body.child(header);
                    for hit in &group.hits {
                        body = body.child(crate::reference::reference_row(
                            "ref-block",
                            ix,
                            hit,
                            &th,
                            &self.app.read(cx).store,
                            8.,
                            me.clone(),
                        ));
                        ix += 1;
                    }
                }
            }
            InspectorTab::Outgoing => {
                let rows: Vec<OutgoingRef> = self.inspector.outgoing.clone();
                for (i, o) in rows.into_iter().enumerate() {
                    let (label, page): (String, Option<PageId>) = match &o.target {
                        RefTarget::Page { id, title } => (title.clone(), Some(id.clone())),
                        RefTarget::UnresolvedPage { key } => (format!("{key} (unresolved)"), None),
                        RefTarget::Block { status, id } => match status {
                            utopia_store::BlockRefStatus::Live { text, page, .. } => (
                                format!(
                                    "(( {} ))",
                                    plain_text(text).chars().take(40).collect::<String>()
                                ),
                                Some(page.clone()),
                            ),
                            utopia_store::BlockRefStatus::Deleted { .. } => {
                                ("deleted block".into(), None)
                            }
                            utopia_store::BlockRefStatus::Missing => {
                                (format!("Missing block (({id}))"), None)
                            }
                        },
                    };
                    body = body.child(self.nav_item(
                        ("out", i),
                        format!("{label}  ·  {}", o.count),
                        page,
                        cx,
                    ));
                }
            }
            InspectorTab::Unresolved => {
                // RS-2: every dangling reference is a row you can act on — the title becomes
                // a page, or the intent is pointed elsewhere or turned back into text
                // (FR-4.14). A missing `((block))` has no page to create, only these choices.
                let rows = self.inspector.unresolved.clone();
                if rows.is_empty() {
                    body = body.child(self.panel_empty(
                        "unresolved-empty",
                        "No unresolved references",
                        cx,
                    ));
                }
                for (i, (o, label)) in rows.into_iter().enumerate() {
                    let block = o.from_block.clone();
                    let mut row = div().flex().flex_col().gap_1().px_2().py_1().child(
                        div()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.danger)
                            .child(label.clone()),
                    );
                    if let RefTarget::UnresolvedPage { key } = &o.target {
                        let (k2, k3) = (key.clone(), key.clone());
                        let (b2, b3) = (block.clone(), block.clone());
                        let title = label.clone();
                        let action =
                            |id: &'static str, text: &'static str, th: &crate::theme::Theme| {
                                quiet_control((id, i), text, th)
                                    .debug_selector(move || format!("unresolved-{id}-{i}"))
                            };
                        row = row.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .child(action("create", "Create page", &th).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        // The row's title is the page title, exactly as
                                        // following the intent from the outline would
                                        // create it (FR-4.14).
                                        let page = this.app.update(cx, |app, cx| {
                                            let r = app.store.create_page(&title);
                                            let page = app.report(r);
                                            app.changed(Vec::new(), true, cx);
                                            page
                                        });
                                        let _ = page;
                                    },
                                )))
                                .child(action("redirect", "Redirect…", &th).on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start_jump(
                                            JumpMode::Redirect {
                                                block: b2.clone(),
                                                key: k2.clone(),
                                            },
                                            window,
                                            cx,
                                        )
                                    },
                                )))
                                .child(action("plain", "Show as text", &th).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.app.update(cx, |app, cx| {
                                            let r = app.store.unlink_intent(&b3, &k3);
                                            app.report(r);
                                            app.changed(Vec::new(), true, cx);
                                        });
                                    },
                                ))),
                        );
                    }
                    body = body.child(row);
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_h_0()
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    /// RS-3: the headings of the page in the main pane, indented as they are in the outline
    /// and drawn at the type scale's heading sizes. Clicking one jumps to it (caret at its
    /// start, scrolled into view).
    fn render_outline(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let entries = self.inspector.outline.clone();
        let mut body = div()
            .id("outline-body")
            .debug_selector(|| "outline-body".into())
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        if entries.is_empty() {
            body = body.child(self.panel_empty("outline-empty", "No headings in this page.", cx));
        }
        for (i, e) in entries.iter().enumerate() {
            let block = e.block.clone();
            body = body.child(
                div()
                    .id(("outline", i))
                    .debug_selector(move || format!("outline-{i}"))
                    .flex()
                    .items_center()
                    .pr_2()
                    .pl(px(8. + e.depth.min(6) as f32 * 10.))
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_size(px(match e.level {
                        1 => crate::theme::H1_SIZE,
                        2 => crate::theme::H2_SIZE,
                        _ => crate::theme::H3_SIZE,
                    }))
                    .text_color(th.text)
                    .hover(|d| d.bg(th.hover))
                    .child(div().flex_1().min_w_0().child(e.text.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.main
                            .update(cx, |m, cx| m.focus_block(&block, None, window, cx));
                    })),
            );
        }
        body.into_any_element()
    }

    /// RS-3: pages pinned to the sidebar, and the control that pins the page being read.
    /// Pinning is local UI state, kept in `settings.json` (rule 9) and never synced.
    fn render_pinned(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let current = self.main.read(cx).page.clone();
        let pinned_here = current
            .as_ref()
            .is_some_and(|p| self.pinned.iter().any(|(q, _)| q == p));
        let mut body = div()
            .id("pinned-body")
            .debug_selector(|| "pinned-body".into())
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        if let Some(page) = current {
            let id = page.as_str().to_string();
            body = body.child(
                quiet_control(
                    "pin-current",
                    if pinned_here {
                        "Unpin this page"
                    } else {
                        "Pin this page"
                    },
                    &th,
                )
                .debug_selector(|| "pin-current".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    crate::appearance::toggle_pinned_page(cx, &id);
                    this.reload_pinned(cx);
                    cx.notify();
                })),
            );
        }
        if self.pinned.is_empty() {
            body = body.child(self.panel_empty(
                "pinned-empty",
                "No pinned pages yet. Pin the page you are reading to keep it one click away.",
                cx,
            ));
        }
        for (i, (page, title)) in self.pinned.clone().into_iter().enumerate() {
            let open_page = page.clone();
            let unpin = page.as_str().to_string();
            body = body.child(
                div()
                    .id(("pinned", i))
                    .debug_selector(move || format!("pinned-{i}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover))
                    .child(div().flex_1().min_w_0().child(title))
                    .child(
                        div()
                            .id(("unpin", i))
                            .debug_selector(move || format!("unpin-{i}"))
                            .px_1()
                            .rounded_sm()
                            .text_color(th.muted)
                            .cursor_pointer()
                            .hover(|d| d.bg(th.hover).text_color(th.text))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                crate::appearance::toggle_pinned_page(cx, &unpin);
                                this.reload_pinned(cx);
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        this.open(
                            open_page.clone(),
                            None,
                            None,
                            e.modifiers().shift,
                            window,
                            cx,
                        )
                    })),
            );
        }
        body.into_any_element()
    }

    /// Read the pinned ids from the preferences and resolve each to its current title, so a
    /// rename shows up without a restart and an id from another graph is skipped.
    fn load_pinned(&mut self, cx: &mut Context<Self>) {
        let ids = crate::appearance::pinned_pages(cx);
        let resolved = self.app.update(cx, |app, _| {
            let fmt = app.store.date_format().clone();
            ids.iter()
                .filter_map(|id| {
                    let page = PageId(id.clone());
                    let title = app
                        .store
                        .page_state(&page)
                        .map(|st| utopia_store::display_title(st, &fmt))
                        .ok()
                        .filter(|t| !t.is_empty())?;
                    Some((page, title))
                })
                .collect::<Vec<_>>()
        });
        self.pinned = resolved;
        self.pinned_ids = ids;
    }

    /// Re-read the pinned list after the user changed it from the panel.
    fn reload_pinned(&mut self, cx: &mut Context<Self>) {
        self.load_pinned(cx);
        self.refresh(cx);
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(cx);
        let th = theme(cx).clone();
        let density = crate::appearance::density(cx);
        let left = if self.left_open {
            self.render_left(cx)
        } else {
            // Collapsed: a thin rail that brings the sidebar back (Cmd+\).
            div()
                .debug_selector(|| "left-rail".into())
                .w(px(32.))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .pt_2()
                .bg(th.sidebar_bg)
                .border_r_1()
                .border_color(th.line)
                .child(
                    div()
                        .id("expand-left")
                        .px_1()
                        .rounded_sm()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(th.hover))
                        .child("»")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_left(&ToggleLeftSidebar, window, cx)
                        })),
                )
                .into_any_element()
        };
        let mut main_col = div().flex_1().min_w_0().h_full().flex().flex_col();
        if self.new_day {
            main_col = main_col.child(
                div()
                    .id("new-day")
                    .flex()
                    .justify_center()
                    .py_1()
                    .bg(th.banner_bg)
                    .text_size(px(crate::theme::BODY_SIZE))
                    .cursor_pointer()
                    .child("It's a new day → open today")
                    .on_click(cx.listener(|this, _, window, cx| this.open_journal(window, cx))),
            );
        }
        let notice = self.sync.as_ref().and_then(|s| s.notice.clone());
        if let Some((text, at)) = notice {
            // Shown for five seconds after a merge (§9.3 "Merged N changes").
            if self.app.read(cx).store.now() - at < 5_000 {
                main_col = main_col.child(
                    div()
                        .flex()
                        .justify_center()
                        .py_1()
                        .bg(th.banner_bg)
                        .text_size(px(crate::theme::BODY_SIZE))
                        .child(text),
                );
            }
        }
        let mut panes_row = div()
            .debug_selector(move || format!("ui-density-{}", density.key()))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .map(|d| match (&self.graph, &self.pdf) {
                        (Some(g), _) => d.child(g.clone()),
                        (None, Some(p)) => d.child(p.clone()),
                        (None, None) => d.child(self.main.clone()),
                    }),
            );
        for (i, sp) in self.splits.iter().enumerate() {
            panes_row = panes_row.child(
                div()
                    .debug_selector(move || format!("split-{i}"))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(th.line)
                    .child(sp.clone()),
            );
        }
        main_col = main_col.child(panes_row);
        let main_col = main_col.debug_selector(|| "main-pane".into());
        let right = self.right_open.then(|| {
            let mut col = div()
                .debug_selector(|| "right-sidebar".into())
                .w(px(self.right_width))
                .h_full()
                .flex_none()
                .flex()
                .flex_row()
                .bg(th.surface)
                .border_l_1()
                .border_color(th.line)
                // RS-4: a 4 px handle on the left edge. The drag is tracked from here and
                // read by the window-level move handler below, so it survives the pointer
                // leaving the sidebar.
                .child(
                    div()
                        .id("right-resize")
                        .debug_selector(|| "right-resize".into())
                        .w(px(4.))
                        .h_full()
                        .flex_none()
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .when(self.resize_from.is_some(), |d| d.bg(th.accent))
                        .hover(|d| d.bg(th.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                this.resize_from =
                                    Some((f32::from(e.position.x), this.right_width));
                                cx.notify();
                            }),
                        ),
                );
            let mut panel = div().flex().flex_col().flex_1().min_w_0().h_full().child(
                // The sidebar's own header: it collapses here (RS-4).
                div().flex().items_center().justify_end().px_1().child(
                    div()
                        .id("right-close")
                        .debug_selector(|| "right-close".into())
                        .px_1()
                        .rounded_sm()
                        .text_size(px(crate::theme::CONTROL_SIZE))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|d| d.bg(th.hover).text_color(th.text))
                        .child("×")
                        .on_click(cx.listener(|this, _, window, cx| this.hide_right(window, cx))),
                ),
            );
            if self.panes.is_empty() && !self.inspector_open {
                panel = panel.child(
                    div()
                        .p_4()
                        .text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.faint)
                        .child("Shift-click a link to open it here."),
                );
            }
            for (i, pane) in self.panes.iter().enumerate() {
                // RS-5: a card per page, its header carrying the fold arrow and the close ×,
                // like Roam's right sidebar. A folded card is its header and nothing more.
                let collapsed = pane.collapsed;
                let title = pane.title.clone();
                let mut card = div()
                    .id(("pane-card", i))
                    .debug_selector(move || format!("pane-{i}"))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(th.line)
                    .when(collapsed, |d| d.flex_none())
                    .when(!collapsed, |d| d.flex_1().min_h(px(140.)));
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .py_0p5()
                        .hover(|d| d.bg(th.hover))
                        .child(
                            div()
                                .id(("pane-collapse", i))
                                .debug_selector(move || format!("pane-collapse-{i}"))
                                .px_1()
                                .rounded_sm()
                                .text_size(px(crate::theme::CONTROL_SIZE))
                                .text_color(th.muted)
                                .cursor_pointer()
                                .hover(|d| d.text_color(th.text))
                                .child(if collapsed { "▸" } else { "▾" })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let host = this.panes.get(i).map(|p| p.view.entity_id());
                                    let folding = this.panes.get(i).is_some_and(|p| !p.collapsed);
                                    if let (true, Some(host)) = (folding, host) {
                                        this.end_edit_in(host, window, cx);
                                    }
                                    if let Some(pane) = this.panes.get_mut(i) {
                                        pane.collapsed = !pane.collapsed;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("pane-title-{i}"))
                                .flex_1()
                                .min_w_0()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(th.muted)
                                .child(title),
                        )
                        .child(
                            div()
                                .id(("pane-close", i))
                                .debug_selector(move || format!("pane-close-{i}"))
                                .px_1()
                                .rounded_sm()
                                .text_size(px(crate::theme::CONTROL_SIZE))
                                .text_color(th.muted)
                                .cursor_pointer()
                                .hover(|d| d.bg(th.hover).text_color(th.text))
                                .child("×")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if i < this.panes.len() {
                                        let host = this.panes[i].view.entity_id();
                                        this.end_edit_in(host, window, cx);
                                        this.panes.remove(i);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
                if !collapsed {
                    card = card.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .border_t_1()
                            .border_color(th.line)
                            .child(pane.view.clone()),
                    );
                }
                panel = panel.child(card);
            }
            if self.inspector_open {
                panel = panel.child(self.render_inspector(cx));
            }
            col = col.child(panel);
            col
        });
        let overlay = self.jump.as_ref().map(|(j, _)| {
            div()
                .id("jump-overlay")
                .absolute()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(90.))
                .bg(th.scrim)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.jump = None;
                        window.focus(&this.focus);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(j.clone()),
                )
        });
        let _ = window;
        let font = crate::appearance::ui_font(cx);
        div()
            // Names the font in force, for tests (Settings → Appearance).
            .debug_selector(move || format!("ui-font-{}", font.key()))
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::quick_jump))
            .on_action(cx.listener(Self::full_search))
            .on_action(cx.listener(Self::show_graph))
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                    this.drop_files(paths.paths(), window, cx)
                }),
            )
            .on_action(cx.listener(Self::quick_capture))
            .on_action(cx.listener(Self::go_daily_notes))
            .on_action(cx.listener(Self::go_today))
            .on_action(cx.listener(Self::go_yesterday))
            .on_action(cx.listener(Self::prev_day))
            .on_action(cx.listener(Self::next_day))
            .on_action(cx.listener(Self::toggle_right))
            .on_mouse_move(cx.listener(Self::resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::resize_done))
            .on_action(cx.listener(Self::toggle_left))
            .on_action(cx.listener(Self::toggle_refs))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::quit))
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(th.bg)
            .text_color(th.text)
            .text_size(px(density.text_size()))
            .line_height(px(density.line_height()))
            .font_family(font.family())
            .child(left)
            .child(main_col)
            .children(right)
            .children(overlay)
    }
}

fn flag(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}
