//! The main window (§9.2): left sidebar (journal, date picker, pages, storage status),
//! the main page, the right sidebar of stacked pages plus the reference inspector, quick
//! jump, and the new-day banner (FR-1.7).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::NaiveDate;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, SharedString, Subscription, Task, Window,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Incoming,
    Outgoing,
    Unresolved,
}

#[derive(Debug, Clone)]
enum JumpMode {
    Open,
    Redirect { block: BlockId, key: String },
    Search,
    Capture,
    Extract { page: PageId, block: BlockId },
}

#[derive(Default)]
struct SidebarData {
    journal: Vec<JournalEntry>,
    pages: Vec<PageRow>,
    days: HashSet<NaiveDate>,
}

#[derive(Default)]
struct InspectorData {
    incoming: Vec<(PageRow, usize)>,
    outgoing: Vec<OutgoingRef>,
    /// Unresolved targets with a display title recovered from the source text.
    unresolved: Vec<(OutgoingRef, String)>,
}

/// `local_state` key holding the pages opened last, newest first (QJ-1). Device-local UI
/// state like collapse, so it never syncs and never enters a shard (rule 9).
const RECENT_KEY: &str = "recent_pages";
/// How many ids are kept: Quick jump shows eight after today and yesterday, and pages
/// deleted since the list was written are skipped, so a few spares are kept.
const RECENT_KEEP: usize = 24;

pub struct Workspace {
    app: Entity<AppState>,
    editor: Entity<BlockEditor>,
    main: Entity<PageView>,
    panes: Vec<Entity<PageView>>,
    splits: Vec<Entity<PageView>>,
    right_open: bool,
    /// Left sidebar and its month grid; local UI state (rule 9), kept in `local_state`.
    left_open: bool,
    calendar_open: bool,
    inspector_open: bool,
    tab: InspectorTab,
    jump: Option<(Entity<JumpPalette>, JumpMode)>,
    /// The graph view, shown in place of the main page while open (P2).
    graph: Option<Entity<GraphView>>,
    /// The PDF viewer, shown in place of the main page while open (P2).
    pdf: Option<Entity<PdfView>>,
    sidebar: SidebarData,
    sidebar_seen: Option<u64>,
    inspector: InspectorData,
    inspector_seen: Option<(PageId, u64)>,
    /// Pages opened last, newest first, for the Quick jump empty state (QJ-1). Read from
    /// and written back to `local_state`, never to a shard (rule 9).
    recent: Vec<PageId>,
    month: NaiveDate,
    today: NaiveDate,
    new_day: bool,
    backup_dir: Option<PathBuf>,
    sync: Option<SyncRunner>,
    focus: FocusHandle,
    subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| BlockEditor::new(app.clone(), cx));
        let main = cx.new(|cx| PageView::new(app.clone(), editor.clone(), false, window, cx));
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
        let recent = app
            .read(cx)
            .store
            .local_get(RECENT_KEY)
            .ok()
            .flatten()
            .map(|v| {
                v.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(RECENT_KEEP)
                    .map(|l| PageId(l.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let mut me = Self {
            app,
            editor,
            main,
            panes: Vec::new(),
            splits: Vec::new(),
            right_open: false,
            left_open,
            calendar_open,
            inspector_open: true,
            tab: InspectorTab::Incoming,
            jump: None,
            graph: None,
            pdf: None,
            sidebar: SidebarData::default(),
            sidebar_seen: None,
            inspector: InspectorData::default(),
            inspector_seen: None,
            recent,
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
        let v = cx.new(|cx| PageView::new(self.app.clone(), self.editor.clone(), true, window, cx));
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

    /// Pages stacked in the right sidebar, top first.
    pub fn sidebar_pages(&self, cx: &gpui::App) -> Vec<PageId> {
        self.panes
            .iter()
            .filter_map(|p| p.read(cx).page.clone())
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

    /// Remember that `page` was opened, for the Quick jump empty state (QJ-1). Local UI
    /// state in `local_state` (rule 9): a page opened on another device is not reordered
    /// here, and a write failure costs the ordering, never the page.
    fn note_opened(&mut self, page: &PageId, cx: &mut Context<Self>) {
        if self.recent.first() == Some(page) {
            return;
        }
        self.recent.retain(|p| p != page);
        self.recent.insert(0, page.clone());
        self.recent.truncate(RECENT_KEEP);
        let joined = self
            .recent
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(e) = self.app.read(cx).store.local_set(RECENT_KEY, &joined) {
            eprintln!("utopia: {e}");
        }
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
                self.panes.retain(|p| p != view);
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
        self.note_opened(&page, cx);
        if sidebar {
            // Shift-click stacks the page beside the current one (§0, W6).
            let existing = self
                .panes
                .iter()
                .position(|p| p.read(cx).page.as_ref() == Some(&page) && p.read(cx).zoom == zoom);
            let view = match existing {
                Some(i) => self.panes.remove(i),
                None => {
                    let v = cx.new(|cx| {
                        PageView::new(self.app.clone(), self.editor.clone(), true, window, cx)
                    });
                    self.subs
                        .push(cx.subscribe_in(&v, window, Self::on_page_event));
                    v.update(cx, |v, cx| v.open(page.clone(), zoom.clone(), cx));
                    v
                }
            };
            self.panes.insert(0, view);
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
        // QJ-1: only "go to page" has an empty state worth having — today, yesterday and
        // the pages opened last. Redirecting a link or naming an extracted page has
        // nothing to do with where you have been.
        let recent = match mode {
            JumpMode::Open => Some(self.recent.clone()),
            _ => None,
        };
        let app = self.app.clone();
        let jump = cx.new(|cx| JumpPalette::with_mode(app, title, palette, recent, window, cx));
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
                    (QuickJumpEvent::ChosenDay { date, sidebar }, Some(JumpMode::Open)) => {
                        this.open_date(*date, *sidebar, !*sidebar, window, cx)
                    }
                    // QJ-5: run the command exactly as its keystroke would, once the
                    // palette is gone and focus is back on the workspace.
                    (QuickJumpEvent::RunCommand(name), _) => {
                        window.focus(&this.focus);
                        match action_for(name) {
                            Some(action) => window.dispatch_action(action, cx),
                            None => eprintln!("utopia: no action named {name}"),
                        }
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

    fn toggle_right(&mut self, _: &ToggleRightSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.right_open = !self.right_open;
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
        if self.sidebar_seen != Some(v) {
            self.sidebar_seen = Some(v);
            let store = &self.app.read(cx).store;
            let all = store.list_pages(true).unwrap_or_default();
            self.sidebar = SidebarData {
                journal: store.journal(14).unwrap_or_default(),
                days: all.iter().filter_map(|p| p.daily_date).collect(),
                pages: all.into_iter().filter(|p| !p.is_daily()).collect(),
            };
        }
        let Some(page) = self.main.read(cx).page.clone() else {
            return;
        };
        if self.inspector_seen.as_ref() != Some(&(page.clone(), v)) {
            self.inspector_seen = Some((page.clone(), v));
            let store = &self.app.read(cx).store;
            let refs = store
                .linked_references(&page, &Default::default())
                .unwrap_or_default();
            let incoming = refs
                .groups
                .into_iter()
                .map(|g| (g.page, g.hits.len()))
                .collect();
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

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let tab = |this: &Self,
                   id: &'static str,
                   t: InspectorTab,
                   label: String,
                   cx: &mut Context<Self>| {
            div()
                .id(id)
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
        let n_in: usize = self.inspector.incoming.iter().map(|(_, n)| n).sum();
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
                let rows: Vec<(PageId, String, usize)> = self
                    .inspector
                    .incoming
                    .iter()
                    .map(|(p, n)| (p.id.clone(), p.title.clone(), *n))
                    .collect();
                for (i, (id, title, n)) in rows.into_iter().enumerate() {
                    body = body.child(self.nav_item(
                        ("in", i),
                        format!("{title}  ·  {n}"),
                        Some(id),
                        cx,
                    ));
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
                let rows = self.inspector.unresolved.clone();
                if rows.is_empty() {
                    body = body.child(
                        div()
                            .px_2()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .child("Every reference resolves."),
                    );
                }
                for (i, (o, label)) in rows.into_iter().enumerate() {
                    let block = o.from_block.clone();
                    let mut row = div().flex().flex_col().px_2().py_1().child(
                        div()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.danger)
                            .child(label.clone()),
                    );
                    if let RefTarget::UnresolvedPage { key } = &o.target {
                        let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                        let (b2, b3) = (block.clone(), block.clone());
                        let title = label.clone();
                        let action = |id: (&'static str, usize), text: &'static str| {
                            div()
                                .id(id)
                                .px_1()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .rounded_sm()
                                .text_color(th.accent)
                                .cursor_pointer()
                                .hover(|s| s.bg(th.hover))
                                .child(text)
                        };
                        row = row.child(
                            div()
                                .flex()
                                .gap_1()
                                .child(action(("create", i), "Create").on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let _ = &k1;
                                        this.app.update(cx, |app, cx| {
                                            let r = app.store.create_page(&title);
                                            app.report(r);
                                            app.changed(Vec::new(), true, cx);
                                        });
                                    },
                                )))
                                .child(action(("redirect", i), "Redirect…").on_click(cx.listener(
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
                                .child(action(("plain", i), "Show as text").on_click(cx.listener(
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
            .h(px(260.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(th.line)
            .child(
                div()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(th.muted)
                    .child("REFERENCES"),
            )
            .child(tabs)
            .child(body)
            .into_any_element()
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
                .w(px(380.))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .bg(th.surface)
                .border_l_1()
                .border_color(th.line);
            if self.panes.is_empty() && !self.inspector_open {
                col = col.child(
                    div()
                        .p_4()
                        .text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.faint)
                        .child("Shift-click a link to open it here."),
                );
            }
            for p in &self.panes {
                col = col.child(
                    div()
                        .flex_1()
                        .min_h(px(140.))
                        .border_b_1()
                        .border_color(th.line)
                        .child(p.clone()),
                );
            }
            if self.inspector_open {
                col = col.child(self.render_inspector(cx));
            }
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
