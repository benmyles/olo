//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale (a box with an action in its top line too), and controls put
//! over each other are found; text the interface cuts is read whole.

use olo_kit::{
    Child, Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div,
    lint::{Found, Lint, assert_clean},
    metrics as m, px,
    widgets::{BBox, Button, CheckBox, RadioButton, TextControl, TextField, text_field},
};

/// A form: a labelled field, choices in a box with Reset, and buttons
/// along the bottom.
struct Form {
    field: Child<TextField>,
    /// The buttons put over each other (to be found).
    stacked: bool,
}

impl View for Form {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let buttons = if self.stacked {
            div().relative().h(px(m::CONTROL_HEIGHT)).child(div().absolute().left(px(0.0)).child(Button::new("Revert"))).child(div().absolute().left(px(20.0)).child(Button::new("Defaults")))
        } else {
            div().flex().flex_row().gap(px(m::SPACING)).child(Button::new("Revert")).child(Button::new("Defaults"))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(TextControl::new(cx, &self.field).label("Name:").divider(60.0))
            .child(CheckBox::new("Show the date in the menu bar", true))
            .child(BBox::new("Set the Time").action("Reset", |_, _| {}).child(RadioButton::new("Automatically", true)).child(RadioButton::new("By hand", false)))
            .child(buttons)
    }
}

fn form(stacked: bool) -> Vec<Found> {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Form", 360.0, 226.0);
    lint.window(&options, move |cx| Form { field: text_field(cx, |f| f), stacked })
}

#[test]
fn ui_lint_a_kit_form_is_clean() {
    assert_clean("the form", &form(false));
}

#[test]
fn ui_lint_finds_controls_over_each_other() {
    let found = form(true);
    assert!(found.iter().any(|f| f.kind == "overlap" && f.text.contains("Revert") && f.text.contains("Defaults")), "{found:#?}");
    // In every look at every scale: the window with no display took each.
    let mut where_found: Vec<(&str, u32)> = found.iter().filter(|f| f.kind == "overlap").map(|f| (f.look, (f.scale * 100.0) as u32)).collect();
    where_found.sort();
    where_found.dedup();
    assert_eq!(where_found, [("dark", 100), ("dark", 150), ("dark", 200), ("light", 100), ("light", 150), ("light", 200)]);
}

/// The interface's own text, some cut: one-line text running past its
/// box, text truncated with "…", a paragraph clamped to two lines; and
/// some not: a line that fits, a paragraph that wraps in full.
struct Texts;

const PAST: &str = "A long line that runs past the side of its box";
const TRUNCATED: &str = "Truncated to the width of its box with an ellipsis";
const CLAMPED: &str = "A paragraph wrapped to its box and clamped to two lines, so the rest of it never shows at all";
const FITS: &str = "Fits";
const WRAPS: &str = "A paragraph that wraps and shows in full";

impl View for Texts {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().w(px(90.0)).overflow_hidden().whitespace_nowrap().child(PAST))
            .child(div().w(px(90.0)).truncate().child(TRUNCATED))
            .child(div().w(px(90.0)).line_clamp(2).child(CLAMPED))
            .child(div().w(px(200.0)).child(FITS))
            .child(div().w(px(200.0)).child(WRAPS))
    }
}

/// Text is read whole whether or not all of it shows (agents find each
/// as a label by its whole text; what's cut is shown whole in a tip,
/// `olo_kit::cut`): the interface's own text is in the window's tree,
/// where olo-ui's lint read only what GPUI's text elements cut.
#[test]
fn ui_lint_text_is_read_whole() {
    let mut lint = Lint::new();
    let trees = lint.trees(&WindowOptions::new("Texts", 360.0, 300.0), |_| Texts);
    let labels: Vec<String> = trees[0].1.iter().filter(|n| n.role == "Label").filter_map(|n| n.name.clone()).collect();
    for text in [PAST, TRUNCATED, CLAMPED, FITS, WRAPS] {
        assert!(labels.iter().any(|l| l == text), "{text:?} isn't read whole: {labels:#?}");
    }
}
