//! The Settings window (§9.1, `OpenSettings` / Cmd+,). Three tabs:
//!
//! - **Keybindings** — every action in [`crate::actions::CATALOG`] with the keys in force,
//!   rebindable by pressing the combination you want. Changes apply at once and are written
//!   to `keymap.json`; only the actions you changed are stored.
//! - **Shortcuts** — the cheat sheet of everything currently bound, which is also what the
//!   Help menu's "Keyboard Shortcuts" opens.
//! - **Help** — a tour of the app's features, with screenshots taken from the app itself.
//!
//! Capture uses a gpui *keystroke interceptor* rather than a key-down handler on purpose:
//! interceptors run before the keymap is consulted, so stopping propagation there means the
//! keys being recorded cannot also fire their own action. Without that, rebinding onto
//! Cmd+Q would quit the app mid-edit.

use gpui::{
    div, img, prelude::*, px, AnyWindowHandle, App, Context, FocusHandle, Focusable, FontWeight,
    Keystroke, SharedString, Subscription, Window,
};
use std::path::PathBuf;

use crate::actions::{
    by_category, canonical_keys, doc, pretty_keys, reinstall, ActionDoc, Category, Overrides,
};
use crate::help::{shot_image, TOPICS};
use crate::theme::theme;

/// Help prose stops at a readable line length, like the page column does.
const HELP_WIDTH: f32 = 720.;
/// Screenshots are shown at a fixed size. gpui derives an image's aspect ratio from the
/// decoded bytes, but that lands a frame after layout, so a width alone leaves the element
/// at its intrinsic height; giving both keeps the box the right shape from the first frame.
/// Every capture is the same window, so one ratio covers them all (see `xtask help-shots`).
const HELP_SHOT_WIDTH: f32 = 640.;
const HELP_SHOT_HEIGHT: f32 = HELP_SHOT_WIDTH * 798. / 1200.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Keybindings,
    Shortcuts,
    Help,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Keybindings => "Keybindings",
            Tab::Shortcuts => "Shortcuts",
            Tab::Help => "Help",
        }
    }

    fn all() -> &'static [Tab] {
        &[Tab::Keybindings, Tab::Shortcuts, Tab::Help]
    }
}

pub struct SettingsView {
    config_dir: PathBuf,
    overrides: Overrides,
    tab: Tab,
    /// The action currently waiting for a keystroke, if any.
    capturing: Option<&'static ActionDoc>,
    /// Dropped to stop intercepting keystrokes.
    capture: Option<Subscription>,
    /// Only accept captured keys that arrive in this window.
    window: Option<AnyWindowHandle>,
    status: Option<SharedString>,
    topic: usize,
    focus: FocusHandle,
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsView {
    pub fn new(
        config_dir: PathBuf,
        tab: Tab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (overrides, warnings) = Overrides::load(&config_dir);
        for w in &warnings {
            eprintln!("utopia: {w}");
        }
        Self {
            config_dir,
            overrides,
            tab,
            capturing: None,
            capture: None,
            window: None,
            // A malformed keymap.json is worth saying out loud: the user's bindings are not
            // the ones they wrote (rule 11: degrade, never panic).
            status: warnings.first().map(|w| SharedString::from(w.clone())),
            topic: 0,
            focus: cx.focus_handle(),
        }
    }

    pub fn show(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.cancel_capture();
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.cancel_capture();
        cx.notify();
    }

    fn cancel_capture(&mut self) {
        self.capturing = None;
        self.capture = None;
    }

    /// Start listening for the next keystroke and bind it to `d`.
    pub fn start_capture(
        &mut self,
        d: &'static ActionDoc,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capturing = Some(d);
        self.status = None;
        let handle = window.window_handle();
        self.window = Some(handle);
        let me = cx.entity().downgrade();
        self.capture = Some(cx.intercept_keystrokes(move |ev, window, cx| {
            // Another window's typing is none of our business.
            if window.window_handle() != handle {
                return;
            }
            // Swallow it so the keys cannot also run their own action while we record them.
            cx.stop_propagation();
            let ks = ev.keystroke.clone();
            let _ = me.update(cx, |this, cx| this.captured(ks, cx));
        }));
        cx.notify();
    }

    /// A keystroke arrived while capturing.
    fn captured(&mut self, ks: Keystroke, cx: &mut Context<Self>) {
        let Some(d) = self.capturing else { return };
        if is_modifier_only(&ks) {
            return; // still waiting for a real key
        }
        self.cancel_capture();
        if ks.key == "escape" && !has_modifier(&ks) {
            self.status = Some("Left unchanged.".into());
            cx.notify();
            return;
        }
        let keys = ks.unparse();
        // Round-trip through the catalog's own validation: whatever gpui hands us has to be
        // something `binding` will accept later, or the override would silently do nothing.
        if crate::actions::binding(d.action, &keys, d.context).is_none() {
            self.status =
                Some(format!("{} cannot be used as a shortcut.", pretty_keys(&keys)).into());
            cx.notify();
            return;
        }
        self.rebind(d, keys, cx);
    }

    fn rebind(&mut self, d: &'static ActionDoc, keys: String, cx: &mut Context<Self>) {
        let pretty = pretty_keys(&keys);
        if canonical_keys(&keys) == canonical_keys(&crate::actions::platform_keys(d.keys)) {
            // Back to the default: drop the override instead of storing a copy of it.
            self.overrides.reset(d.action);
        } else {
            self.overrides.set(d.action, &keys);
        }
        self.apply(cx);
        let taken: Vec<&str> = self
            .overrides
            .conflicts()
            .into_iter()
            .filter(|(a, b)| a.action == d.action || b.action == d.action)
            .map(|(a, b)| {
                if a.action == d.action {
                    b.label
                } else {
                    a.label
                }
            })
            .collect();
        self.status = Some(if taken.is_empty() {
            format!("{} is now {pretty}.", d.label).into()
        } else {
            format!(
                "{} is now {pretty}, which {} also uses.",
                d.label,
                taken.join(" and ")
            )
            .into()
        });
        cx.notify();
    }

    pub fn reset_one(&mut self, d: &'static ActionDoc, cx: &mut Context<Self>) {
        self.overrides.reset(d.action);
        self.apply(cx);
        self.status = Some(
            format!(
                "{} is back to {}.",
                d.label,
                pretty_keys(&crate::actions::platform_keys(d.keys))
            )
            .into(),
        );
        cx.notify();
    }

    pub fn reset_all(&mut self, cx: &mut Context<Self>) {
        if self.overrides.0.is_empty() {
            self.status = Some("Nothing to reset: these are the defaults.".into());
            cx.notify();
            return;
        }
        self.overrides = Overrides::default();
        self.apply(cx);
        self.status = Some("Every shortcut is back to its default.".into());
        cx.notify();
    }

    /// Put the bindings live and write them down. A failed write is reported, not fatal:
    /// the bindings still work for this session.
    fn apply(&mut self, cx: &mut Context<Self>) {
        reinstall(cx, &self.overrides);
        if let Err(e) = self.overrides.save(&self.config_dir) {
            self.status = Some(format!("Could not save keymap.json: {e}").into());
        }
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Whether a keystroke is being recorded right now.
    pub fn capturing(&self) -> Option<&'static ActionDoc> {
        self.capturing
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref().map(|v| &**v)
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }
}

/// gpui sends modifier presses through as keystrokes with no real key on some layouts.
fn is_modifier_only(ks: &Keystroke) -> bool {
    matches!(
        ks.key.as_str(),
        "" | "cmd" | "ctrl" | "alt" | "shift" | "function" | "fn" | "capslock"
    )
}

fn has_modifier(ks: &Keystroke) -> bool {
    let m = &ks.modifiers;
    m.control || m.alt || m.shift || m.platform || m.function
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let tabs = Tab::all().iter().map(|&t| {
            let on = t == self.tab;
            div()
                .id(SharedString::from(format!("tab-{}", t.label())))
                .debug_selector(move || format!("settings-tab-{}", t.label()))
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .when(on, |d| d.bg(th.chip_bg).text_color(th.accent))
                .when(!on, |d| d.text_color(th.muted))
                .child(t.label())
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
        });
        let tabs: Vec<_> = tabs.collect();

        let body = match self.tab {
            Tab::Keybindings => self.render_keybindings(cx).into_any_element(),
            Tab::Shortcuts => self.render_shortcuts(cx).into_any_element(),
            Tab::Help => self.render_help(cx).into_any_element(),
        };

        div()
            .debug_selector(|| "settings".into())
            .track_focus(&self.focus)
            .key_context("Settings")
            .size_full()
            .flex()
            .flex_col()
            .bg(th.bg)
            .text_color(th.text)
            .text_size(px(crate::theme::TEXT_SIZE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(th.border)
                    .child(div().font_weight(FontWeight::BOLD).mr_2().child("Settings"))
                    .children(tabs),
            )
            .child(body)
            .children(self.status.clone().map(|s| {
                div()
                    .debug_selector(|| "settings-status".into())
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(th.border)
                    .bg(th.panel_bg)
                    .text_xs()
                    .text_color(th.muted)
                    .child(s)
            }))
    }
}

impl SettingsView {
    fn render_keybindings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let conflicts = self.overrides.conflicts();
        let changed = self.overrides.0.len();

        let mut list = div()
            .id("settings-keys")
            .debug_selector(|| "settings-keys".into())
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .gap_1();

        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pb_2()
                .child(div().text_xs().text_color(th.muted).child(if changed == 0 {
                    "Pick Change, then press the keys you want.".to_string()
                } else {
                    format!(
                        "Pick Change, then press the keys you want. {changed} changed \
                                 from the default."
                    )
                }))
                .child(
                    div()
                        .id("reset-all")
                        .debug_selector(|| "settings-reset-all".into())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(th.border)
                        .text_xs()
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|d| d.bg(th.hover))
                        .child("Reset All")
                        .on_click(cx.listener(|this, _, _, cx| this.reset_all(cx))),
                ),
        );

        for (a, b) in &conflicts {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(th.missing_bg)
                    .text_xs()
                    .text_color(th.missing_text)
                    .child(format!(
                        "{} and {} are both on {}.",
                        a.label,
                        b.label,
                        pretty_keys(&self.overrides.keys_for(a))
                    )),
            );
        }

        for &c in Category::all() {
            let mut any = false;
            let mut section = div().flex().flex_col().pt_3().gap_0p5();
            section = section.child(
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(th.faint)
                    .font_weight(FontWeight::MEDIUM)
                    .child(c.label()),
            );
            for d in by_category(c) {
                any = true;
                section = section.child(self.render_key_row(d, cx));
            }
            if any {
                list = list.child(section);
            }
        }
        list
    }

    fn render_key_row(&self, d: &'static ActionDoc, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let capturing = self.capturing.map(|c| c.action) == Some(d.action);
        let keys = self.overrides.keys_for(d);
        let changed = self.overrides.is_changed(d.action);
        let chip = if capturing {
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(th.banner_bg)
                .text_color(th.accent)
                .text_xs()
                .child("Press keys… (esc to keep)")
        } else {
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(th.chip_bg)
                .text_xs()
                .text_color(th.text)
                .child(pretty_keys(&keys))
        };
        div()
            .debug_selector(move || format!("settings-row-{}", d.action))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_3()
            .py_1p5()
            .rounded_md()
            .hover(|x| x.bg(th.hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(div().child(d.label))
                    .child(div().text_xs().text_color(th.muted).child(d.help)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(chip)
                    .child(
                        div()
                            .id(SharedString::from(format!("change-{}", d.action)))
                            .debug_selector(move || format!("settings-change-{}", d.action))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .border_1()
                            .border_color(th.border)
                            .text_xs()
                            .text_color(th.muted)
                            .cursor_pointer()
                            .hover(|x| x.bg(th.hover))
                            .child(if capturing { "Cancel" } else { "Change" })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.capturing.map(|c| c.action) == Some(d.action) {
                                    this.cancel_capture();
                                    cx.notify();
                                } else {
                                    this.start_capture(d, window, cx);
                                }
                            })),
                    )
                    .children(changed.then(|| {
                        div()
                            .id(SharedString::from(format!("reset-{}", d.action)))
                            .debug_selector(move || format!("settings-reset-{}", d.action))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .text_color(th.accent)
                            .cursor_pointer()
                            .hover(|x| x.bg(th.hover))
                            .child("Reset")
                            .on_click(cx.listener(move |this, _, _, cx| this.reset_one(d, cx)))
                    })),
            )
    }

    /// The cheat sheet: everything bound right now, grouped the same way as the editor.
    fn render_shortcuts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let mut list = div()
            .id("settings-shortcuts")
            .debug_selector(|| "settings-shortcuts".into())
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .px_4()
            .py_3();
        list = list.child(
            div()
                .pb_2()
                .text_xs()
                .text_color(th.muted)
                .child("Every shortcut in the app, as it is bound now."),
        );
        for &c in Category::all() {
            let rows = by_category(c).map(|d| {
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap_4()
                    .py_0p5()
                    .child(div().text_sm().child(d.label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(th.muted)
                            .child(pretty_keys(&self.overrides.keys_for(d))),
                    )
            });
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .pt_3()
                    .child(
                        div()
                            .pb_1()
                            .text_xs()
                            .text_color(th.faint)
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.label()),
                    )
                    .children(rows),
            );
        }
        list
    }

    fn render_help(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let selected = self.topic.min(TOPICS.len().saturating_sub(1));
        let nav = TOPICS.iter().enumerate().map(|(i, t)| {
            let on = i == selected;
            div()
                .id(SharedString::from(format!("help-nav-{i}")))
                .debug_selector(move || format!("help-nav-{i}"))
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .when(on, |d| d.bg(th.chip_bg).text_color(th.accent))
                .when(!on, |d| d.text_color(th.text).hover(|x| x.bg(th.hover)))
                .child(t.title)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.topic = i;
                    cx.notify();
                }))
        });
        let nav: Vec<_> = nav.collect();

        let t = &TOPICS[selected];
        let shortcuts: Vec<_> = t
            .actions
            .iter()
            .filter_map(|a| doc(a))
            .map(|d| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(th.chip_bg)
                            .text_xs()
                            .child(pretty_keys(&self.overrides.keys_for(d))),
                    )
                    .child(div().text_xs().text_color(th.muted).child(d.label))
            })
            .collect();

        // `w_full` first, then `max_w`: a flex child's width is only definite once it is
        // told to fill its parent, and without a definite width the prose does not wrap.
        let mut column = div()
            .w_full()
            .max_w(px(HELP_WIDTH))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::BOLD)
                    .child(t.title),
            )
            .child(div().text_color(th.muted).child(t.summary));
        for p in t.body {
            column = column.child(div().text_sm().child(*p));
        }
        if !shortcuts.is_empty() {
            column = column.child(div().flex().flex_wrap().gap_3().pt_1().children(shortcuts));
        }
        if let Some(image) = shot_image(t) {
            column = column.child(
                div().debug_selector(|| "help-shot".into()).pt_2().child(
                    img(image)
                        .w(px(HELP_SHOT_WIDTH))
                        .h(px(HELP_SHOT_HEIGHT))
                        .rounded_md()
                        .border_1()
                        .border_color(th.border),
                ),
            );
        }
        let detail = div()
            .id("help-detail")
            .debug_selector(|| "help-detail".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .overflow_y_scroll()
            .px_6()
            .py_4()
            .child(column);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .overflow_hidden()
            .child(
                div()
                    .id("help-nav")
                    .flex()
                    .flex_col()
                    .w(px(260.))
                    .flex_none()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(th.border)
                    .bg(th.sidebar_bg)
                    .px_2()
                    .py_3()
                    .gap_0p5()
                    .children(nav),
            )
            .child(detail)
    }
}
