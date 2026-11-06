//! The Settings window (§9.1, `OpenSettings` / Cmd+,). Four tabs:
//!
//! - **Keybindings** — every action in [`crate::actions::CATALOG`] with the keys in force,
//!   rebindable by pressing the combination you want. Changes apply at once and are written
//!   to `keymap.json`; only the actions you changed are stored.
//! - **Shortcuts** — the cheat sheet of everything currently bound, which is also what the
//!   Help menu's "Keyboard Shortcuts" opens.
//! - **Appearance** — the font notes and the interface are set in (System or the bundled
//!   IBM Plex Sans) and how densely blocks are set (Comfortable or Compact). Applies to
//!   every window at once; kept in `settings.json` ([`crate::appearance`]), not in
//!   `keymap.json`.
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
use crate::appearance::{self, Density, UiFont};
use crate::help::{shot_image, TOPICS};
use crate::theme::{quiet_control, theme};

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
    Appearance,
    Help,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Keybindings => "Keybindings",
            Tab::Shortcuts => "Shortcuts",
            Tab::Appearance => "Appearance",
            Tab::Help => "Help",
        }
    }

    fn all() -> &'static [Tab] {
        &[Tab::Keybindings, Tab::Shortcuts, Tab::Appearance, Tab::Help]
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
        // The Appearance tab shows a sample of every font it offers.
        for &f in UiFont::ALL {
            appearance::load_font(cx, f);
        }
        let (overrides, mut warnings) = Overrides::load(&config_dir);
        for w in &warnings {
            eprintln!("utopia: {w}");
        }
        // Already logged at startup; repeated here so the user sees it.
        warnings.extend(appearance::Prefs::load(&config_dir).1);
        Self {
            config_dir,
            overrides,
            tab,
            capturing: None,
            capture: None,
            window: None,
            // A malformed keymap.json or settings.json is worth saying out loud: what is in
            // force is not what the user wrote (rule 11: degrade, never panic).
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

    /// Settings → Appearance: put `font` in force in every window and remember it.
    pub fn choose_font(&mut self, font: UiFont, cx: &mut Context<Self>) {
        let before = appearance::ui_font(cx);
        self.status = Some(match appearance::set_font(cx, font, &self.config_dir) {
            Err(e) => format!(
                "{} applies now, but could not save {}: {e}",
                font.label(),
                appearance::FILE
            )
            .into(),
            Ok(()) if before == font => format!("The font is already {}.", font.label()).into(),
            Ok(()) => format!("The font is now {}.", font.label()).into(),
        });
        cx.notify();
    }

    /// Settings → Appearance: switch density in every window and remember it (ALL-5).
    pub fn choose_density(&mut self, density: Density, cx: &mut Context<Self>) {
        let before = appearance::density(cx);
        self.status = Some(
            match appearance::set_density(cx, density, &self.config_dir) {
                Err(e) => format!(
                    "{} applies now, but could not save {}: {e}",
                    density.label(),
                    appearance::FILE
                )
                .into(),
                Ok(()) if before == density => {
                    format!("The density is already {}.", density.label()).into()
                }
                Ok(()) => format!("The density is now {}.", density.label()).into(),
            },
        );
        cx.notify();
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
        let density = appearance::density(cx);
        let tabs = Tab::all().iter().map(|&t| {
            let on = t == self.tab;
            div()
                .id(SharedString::from(format!("tab-{}", t.label())))
                .debug_selector(move || format!("settings-tab-{}", t.label()))
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(crate::theme::BODY_SIZE))
                .when(on, |d| d.bg(th.chip_bg).text_color(th.accent))
                .when(!on, |d| d.text_color(th.muted))
                .child(t.label())
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
        });
        let tabs: Vec<_> = tabs.collect();
        let font = appearance::ui_font(cx);

        let body = match self.tab {
            Tab::Keybindings => self.render_keybindings(cx).into_any_element(),
            Tab::Shortcuts => self.render_shortcuts(cx).into_any_element(),
            Tab::Appearance => self.render_appearance(cx).into_any_element(),
            Tab::Help => self.render_help(cx).into_any_element(),
        };
        // One debug selector per element: the body names the font, the header the density.
        let body = div()
            .debug_selector(move || format!("settings-ui-font-{}", font.key()))
            .flex()
            .flex_col()
            .flex_1()
            .child(body);

        div()
            .debug_selector(|| "settings".into())
            .track_focus(&self.focus)
            .key_context("Settings")
            .size_full()
            .flex()
            .flex_col()
            .bg(th.bg)
            .text_color(th.text)
            .text_size(px(density.text_size()))
            .line_height(px(density.line_height()))
            .font_family(appearance::font_family(cx))
            .child(
                div()
                    // Names the density in force, for tests (the root names the font; an
                    // element carries only one debug selector).
                    .debug_selector(move || format!("settings-ui-density-{}", density.key()))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(th.line)
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
                    .border_color(th.line)
                    .bg(th.surface)
                    .text_size(px(crate::theme::CAPTION_SIZE))
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
                .child(
                    div()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .child(if changed == 0 {
                            "Pick Change, then press the keys you want.".to_string()
                        } else {
                            format!(
                                "Pick Change, then press the keys you want. {changed} changed \
                                 from the default."
                            )
                        }),
                )
                .child(
                    quiet_control("reset-all", "Reset All", &th)
                        .debug_selector(|| "settings-reset-all".into())
                        .on_click(cx.listener(|this, _, _, cx| this.reset_all(cx))),
                ),
        );

        for (a, b) in &conflicts {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(th.danger_bg)
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.danger)
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
                    .text_size(px(crate::theme::CAPTION_SIZE))
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
                .text_size(px(crate::theme::CAPTION_SIZE))
                .child("Press keys… (esc to keep)")
        } else {
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(th.chip_bg)
                .text_size(px(crate::theme::CAPTION_SIZE))
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
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(d.help),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(chip)
                    .child(
                        quiet_control(
                            SharedString::from(format!("change-{}", d.action)),
                            if capturing { "Cancel" } else { "Change" },
                            &th,
                        )
                        .debug_selector(move || format!("settings-change-{}", d.action))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.capturing.map(|c| c.action) == Some(d.action) {
                                    this.cancel_capture();
                                    cx.notify();
                                } else {
                                    this.start_capture(d, window, cx);
                                }
                            },
                        )),
                    )
                    .children(changed.then(|| {
                        div()
                            .id(SharedString::from(format!("reset-{}", d.action)))
                            .debug_selector(move || format!("settings-reset-{}", d.action))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .text_size(px(crate::theme::CAPTION_SIZE))
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
                .text_size(px(crate::theme::CAPTION_SIZE))
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
                    .child(div().text_size(px(crate::theme::BODY_SIZE)).child(d.label))
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
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
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.label()),
                    )
                    .children(rows),
            );
        }
        list
    }

    fn render_appearance(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let current = appearance::ui_font(cx);
        let options = UiFont::ALL.iter().map(|&f| {
            let on = f == current;
            div()
                .id(SharedString::from(format!("font-{}", f.key())))
                .debug_selector(move || format!("settings-font-{}", f.key()))
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(if on { th.accent } else { th.line })
                .when(on, |d| d.bg(th.chip_bg))
                .cursor_pointer()
                .hover(|x| x.bg(th.hover))
                // A drawn radio mark: "●"/"○" would come from whichever font is in force,
                // and IBM Plex Sans has neither, so the fallback drew them at another size.
                .child(
                    div()
                        .size(px(14.))
                        .flex_none()
                        .rounded_full()
                        .border_1()
                        .border_color(if on { th.accent } else { th.faint })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |d| {
                            d.child(div().size(px(8.)).rounded_full().bg(th.accent))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(div().child(f.label()))
                        // Each sample in its own font, whichever one is in force.
                        .child(
                            div()
                                .font_family(f.family())
                                .text_color(th.muted)
                                .child("Bob dug a big bog by the pond, quickly."),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.choose_font(f, cx)))
        });
        let options: Vec<_> = options.collect();
        let current_density = appearance::density(cx);
        let densities: Vec<_> = Density::ALL
            .iter()
            .map(|&d| {
                let on = d == current_density;
                let th = th.clone();
                div()
                    .id(SharedString::from(format!("density-{}", d.key())))
                    .debug_selector(move || format!("settings-density-{}", d.key()))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if on { th.accent } else { th.line })
                    .when(on, |x| x.bg(th.chip_bg))
                    .cursor_pointer()
                    .hover(|x| x.bg(th.hover))
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .border_color(if on { th.accent } else { th.faint })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |x| {
                                x.child(div().size(px(8.)).rounded_full().bg(th.accent))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().child(d.label()))
                            .child(
                                div()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.muted)
                                    .child(match d {
                                        Density::Comfortable => {
                                            "15 px body on 24 px lines, as before."
                                        }
                                        Density::Compact => "13.5 px body and 4 px between blocks.",
                                    }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_density(d, cx)))
            })
            .collect();
        div()
            .id("settings-appearance")
            .debug_selector(|| "settings-appearance".into())
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(HELP_WIDTH))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .pt_3()
                            .pb_1()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Font"),
                    )
                    .child(div().text_size(px(crate::theme::CAPTION_SIZE)).text_color(th.muted).child(
                        "Used for your notes and the interface, in every window, as soon as \
                         you pick it. Code blocks stay in Menlo.",
                    ))
                    .children(options)
                    .child(
                        div()
                            .pt_4()
                            .pb_1()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Density"),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(
                                "How much space your notes take. Local to this machine, \
                                 kept in settings.json, never synced.",
                            ),
                    )
                    .children(densities),
            )
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
                .text_size(px(crate::theme::BODY_SIZE))
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
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .child(pretty_keys(&self.overrides.keys_for(d))),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(d.label),
                    )
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
                    .text_size(px(crate::theme::H1_SIZE))
                    .font_weight(FontWeight::BOLD)
                    .child(t.title),
            )
            .child(div().text_color(th.muted).child(t.summary));
        for p in t.body {
            column = column.child(div().text_size(px(crate::theme::BODY_SIZE)).child(*p));
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
                        .border_color(th.line),
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
                    .border_color(th.line)
                    .bg(th.sidebar_bg)
                    .px_2()
                    .py_3()
                    .gap_0p5()
                    .children(nav),
            )
            .child(detail)
    }
}
