//! Find's bar: a band over what it searches (a document's pages, a
//! text), as a tool bar is (the panel's ground, a line under it), with
//! the field, how many it found ("3 of 90", "Not found", still looking),
//! Previous, Next and Close. Return goes to the next match, ⇧Return the
//! one before, Escape closes it. The view owns the field and the matches;
//! this is how they're shown, the same in every app (Viewer's PDFs and
//! texts).

use std::rc::Rc;

use olo_interface::{Child, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, Styled as _, WindowCx, canvas, div, px};
use olo_kit_base::metrics as m;

use super::{Button, StringView, TextControl, TextField};
use crate::{style, theme};

/// What a find says: nothing before anything's typed, how many it
/// found and which is shown, or that it's still looking (`looking`, its
/// progress: "12 of 38 pages").
pub fn find_said(query: &str, found: usize, current: Option<usize>, looking: Option<String>) -> String {
    match (query.is_empty(), found, current, looking) {
        (true, ..) => String::new(),
        (false, 0, _, Some(l)) => format!("Looking… ({l})"),
        (false, 0, _, None) => "Not found".into(),
        (false, n, Some(c), l) => format!("{} of {n}{}", c + 1, if l.is_some() { "…" } else { "" }),
        (false, n, None, _) => format!("{n} found"),
    }
}

/// The least width the bar shows whole at: its field, what it found and
/// its three buttons. A window that may be narrower (Tracker's Quick
/// Look, as wide as what it shows) widens to it when Find opens; in
/// less, what it found gives way first, then the field.
pub const MIN_WIDTH: f32 = 640.0;

/// The bar: `field` (the view's, its Return and ⇧Return its own to
/// handle), `said` ([`find_said`]), Previous and Next (`step`, told
/// whether it's back: `cx.msg_with(|back: &bool| ..)`) while anything's
/// found, Close.
pub fn find_bar(cx: &WindowCx, field: &Child<TextField>, said: String, found: bool, step: impl Fn(&bool, &mut WindowCx) + 'static, close: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let step = Rc::new(step);
    let close = Rc::new(close);
    let (back, next, before, esc, done) = (step.clone(), step.clone(), step, close.clone(), close);
    // The tool bar's band: the panel's ground, a line under it.
    let ground = canvas(|g, b| {
        let p = theme::palette();
        // (`style`'s fill, as the tool bar's: the layout review knows
        // a band along the window's edge by it, and leaves the band's
        // own insets to it.)
        style::fill_rect(g, b, style::c(p.panel_background));
        style::hline_above(g, b.left(), b.right(), b.bottom(), style::shade(&p, p.panel_background, 1.3));
    })
    .paint_key(0)
    .absolute()
    .size_full();
    div()
        .relative()
        .h(px(m::TOOLBAR_HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        // The field's text at the window's inset, under the menu bar's
        // first title (a field's text is `TEXT_INSET` and 4 more in).
        .pl(px(m::WINDOW_INSET - m::TEXT_INSET - 4.0))
        .pr(px(m::SPACING))
        .child(ground)
        .on_key_down(move |e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                esc(&(), cx);
                cx.stop_propagation();
            } else if e.keystroke.key == "enter" && e.keystroke.modifiers.shift {
                // ⇧Return, which a one-line field lets go on (Return alone
                // is its `Entered`): the match before. Here, for every
                // find bar: each view handled `Entered` with ⇧ held,
                // which never came.
                before(&true, cx);
                cx.stop_propagation();
            }
        })
        .child(div().w(px(260.0)).child(TextControl::new(cx, field)))
        .child(div().flex_1().min_w_0().child(StringView::new(said).cut(true)))
        .child(Button::new("Previous").enabled(found).on_click(move |_, cx| back(&true, cx)))
        .child(Button::new("Next").enabled(found).on_click(move |_, cx| next(&false, cx)))
        .child(Button::new("Close").on_click(move |_, cx| done(&(), cx)))
}

#[cfg(test)]
mod tests {
    use super::find_said;

    #[test]
    fn what_it_says() {
        assert_eq!(find_said("", 4, Some(0), None), "");
        assert_eq!(find_said("x", 0, None, None), "Not found");
        assert_eq!(find_said("x", 0, None, Some("3 of 38 pages".into())), "Looking… (3 of 38 pages)");
        assert_eq!(find_said("x", 90, Some(2), None), "3 of 90");
        assert_eq!(find_said("x", 90, Some(2), Some("…".into())), "3 of 90…");
    }
}
