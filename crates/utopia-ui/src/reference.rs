//! One reference row (FR-3.1): the path above it, a bullet, and the referencing block
//! rendered the way an outline row is — marks, page chips and `((block))` chips inside one
//! shaped text (§13.4). Linked References and the right sidebar's References tab share it
//! (RS-1), so a reference reads the same wherever it is shown and the two cannot drift.

use std::ops::Range;

use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Entity, MouseButton, StyledText, Window,
};
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockRefStatus, RefHit, Store};

use crate::rich::{self, ChipState, LinkTarget};
use crate::theme::{Theme, CAPTION_SIZE};

/// How a `((uid))` chip should look in a row of `store` (FR-4.3, FR-4.7, AC-4.5).
pub fn chip_state(store: &Store, uid: &str, row_block: &BlockId) -> ChipState {
    if uid == row_block.as_str() {
        return ChipState::Cyclic;
    }
    match store.block_ref_status(uid) {
        Ok(BlockRefStatus::Live { text, .. }) => ChipState::Live(text),
        Ok(BlockRefStatus::Deleted { .. }) => ChipState::Deleted,
        _ => ChipState::Missing,
    }
}

/// What a row's clicks do. A page view and the workspace each have their own idea of
/// "open this page here" (a `PageView` emits an event, the workspace switches panes), so the
/// row calls back into the view that hosts it instead of hard-coding one.
pub trait RefRowHost: Sized {
    /// The row itself was clicked: show `page` with `block` focused.
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// A chip inside the row was clicked: follow the page/block it names.
    fn follow_reference(
        &mut self,
        target: LinkTarget,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
}

/// A reference row. `prefix` + `ix` name it for `debug_selector` (tests read its bounds);
/// `pad_x` is the row's inset from the panel edge, so the two call sites can keep their own
/// measure without a second renderer.
pub fn reference_row<H: RefRowHost + 'static>(
    id: impl Into<ElementId>,
    prefix: &'static str,
    ix: usize,
    hit: &RefHit,
    theme: &Theme,
    store: &Store,
    pad_x: f32,
    host: Entity<H>,
) -> AnyElement {
    let th = theme.clone();
    let crumbs = hit
        .breadcrumb
        .iter()
        .map(|(_, t)| t.chars().take(30).collect::<String>())
        .collect::<Vec<_>>()
        .join(" › ");
    let rendered = rich::render(&hit.text, &th, &mut |uid| {
        chip_state(store, uid, &hit.block)
    });
    let styled =
        StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
    // Bound to the shaped text, so a click can tell a chip from the block body.
    let layout = styled.layout().clone();
    let links: Vec<Range<usize>> = rendered.links.iter().map(|(r, _)| r.clone()).collect();
    let (page, block) = (hit.page.clone(), hit.block.clone());
    let (a, b) = (host.clone(), host);
    div()
        .id(id)
        .debug_selector(move || format!("{prefix}-{ix}"))
        .px(px(pad_x))
        .py_0p5()
        .rounded_sm()
        .flex()
        .flex_col()
        .cursor_pointer()
        .hover(|d| d.bg(th.hover))
        .when(!crumbs.is_empty(), |d| {
            d.child(
                div()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.faint)
                    .child(format!("› {crumbs}")),
            )
        })
        .child(
            div()
                .flex()
                .child(div().w(px(14.)).text_color(th.bullet).child("•"))
                // A flex item's minimum width is its unwrapped text unless `min_w_0`, so
                // the body would run wide instead of wrapping at the panel's edge.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(rich::LinkText::new(styled, links)),
                ),
        )
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            if let Ok(ix) = layout.index_for_position(e.position) {
                if let Some(target) = rendered.link_at(ix).cloned() {
                    cx.stop_propagation();
                    b.update(cx, |this, cx| {
                        this.follow_reference(target, e.modifiers.shift, window, cx)
                    });
                    return;
                }
            }
            a.update(cx, |this, cx| {
                this.open_reference(page.clone(), block.clone(), e.modifiers.shift, window, cx)
            });
        })
        .into_any_element()
}
