//! Find's bar: a band over what it searches (a document's pages, a
//! text), as a tool bar is (the panel's ground, a line under it), with
//! the field, how many it found ("3 of 90", "Not found", still looking),
//! Previous, Next and Close. Return goes to the next match, ⇧Return the
//! one before, Escape closes it. The view owns the field and the matches;
//! this is how they're shown, the same in every app (Viewer's PDFs and
//! texts).

use gpui::{AnyElement, App, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Styled as _, Window, canvas, div, px};
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

/// The bar: `field` (the view's, its Return and ⇧Return its own to
/// handle), `said` ([`find_said`]), Previous and Next (`step(back)`)
/// while anything's found, Close.
pub fn find_bar(field: &Entity<TextField>, said: String, found: bool, step: impl Fn(bool, &mut Window, &mut App) + 'static, close: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let step = std::rc::Rc::new(step);
    let close = std::rc::Rc::new(close);
    let (back, next, esc, done) = (step.clone(), step, close.clone(), close);
    // The tool bar's band: the panel's ground, a line under it.
    let ground = canvas(
        |_, _, _| {},
        |b, _, window, cx| {
            let p = theme::palette(cx);
            window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
            style::hline_above(window, b.left(), b.right(), b.bottom(), style::shade(&p, p.panel_background, 1.3));
        },
    )
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
        .on_key_down(move |e: &KeyDownEvent, window, cx| {
            if e.keystroke.key == "escape" {
                esc(window, cx);
                cx.stop_propagation();
            }
        })
        .child(div().w(px(260.0)).child(TextControl::new(field.clone())))
        .child(div().flex_1().child(StringView::new(said)))
        .child(Button::new("Previous").enabled(found).on_click(move |window, cx| back(true, window, cx)))
        .child(Button::new("Next").enabled(found).on_click(move |window, cx| next(false, window, cx)))
        .child(Button::new("Close").on_click(move |window, cx| done(window, cx)))
        .into_any_element()
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
