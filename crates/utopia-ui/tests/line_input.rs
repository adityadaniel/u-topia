//! The single-line input's own side of §13.5: a host that writes the whole text is told
//! about it, a write of the text already shown is not a change (the reference filter is
//! cleared on every page change, §13.4), and a composition in it is never mistaken for a
//! committed query.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{EntityInputHandler as _, TestAppContext};
use utopia_ui::line_input::{LineInput, LineInputEvent};
use utopia_ui::theme::Theme;

#[gpui::test]
fn set_text_notifies_only_a_change(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::light()));
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("search", cx));
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = seen.clone();
    let _sub = cx.cx.update(|cx| {
        cx.subscribe(&input, move |_, e: &LineInputEvent, _| {
            if let LineInputEvent::Changed(t) = e {
                sink.borrow_mut().push(t.clone());
            }
        })
    });

    input.update(cx, |i, cx| i.set_text("november", cx));
    assert_eq!(seen.borrow().as_slice(), ["november"]);
    // Clearing a filter that is already empty must not make its listener rebuild.
    input.update(cx, |i, cx| i.set_text("november", cx));
    assert_eq!(
        seen.borrow().as_slice(),
        ["november"],
        "writing the text it already shows changes nothing"
    );

    // A composition is provisional: the host hears about the commit, not the glyphs.
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    assert_eq!(seen.borrow().len(), 1, "a composition is not a query");
    // A host write still wins over a live composition and is announced once.
    input.update(cx, |i, cx| i.set_text("zebra", cx));
    assert_eq!(seen.borrow().as_slice(), ["november", "zebra"]);
    assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "zebra");
    assert_eq!(
        input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx)),
        None,
        "the field is the host's again"
    );
}
