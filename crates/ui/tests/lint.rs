//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale (a box with an action in its top line too), and controls put
//! over each other are found.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    lint::{Lint, assert_clean},
    metrics as m,
    widgets::{BBox, Button, CheckBox, RadioButton, TextControl, text_field},
};

/// A form: a labelled field, choices in a box with Reset, and buttons
/// along the bottom.
struct Form {
    field: olo_ui::gpui::Entity<olo_ui::widgets::TextField>,
    /// The buttons put over each other (to be found).
    stacked: bool,
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let buttons = if self.stacked {
            div().relative().h(px(m::CONTROL_HEIGHT)).child(div().absolute().left_0().child(Button::new("Revert"))).child(div().absolute().left(px(20.0)).child(Button::new("Defaults")))
        } else {
            div().flex().flex_row().gap(px(m::SPACING)).child(Button::new("Revert")).child(Button::new("Defaults"))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(TextControl::new(self.field.clone()).label("Name:").divider(60.0))
            .child(CheckBox::new("Show the date in the menu bar", true))
            .child(BBox::new("Set the Time").action("Reset", |_, _| {}).child(RadioButton::new("Automatically", true)).child(RadioButton::new("By hand", false)))
            .child(buttons)
    }
}

fn form(stacked: bool) -> Vec<olo_ui::lint::Found> {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Form", 360.0, 226.0);
    lint.window(&options, move |_, cx| {
        let field = text_field(cx, |f| f);
        cx.new(|_| Form { field, stacked })
    })
}

#[test]
fn ui_lint_a_kit_form_is_clean() {
    assert_clean("the form", &form(false));
}

#[test]
fn ui_lint_finds_controls_over_each_other() {
    let found = form(true);
    assert!(found.iter().any(|f| f.kind == "overlap" && f.text.contains("Revert") && f.text.contains("Defaults")), "{found:#?}");
}

/// GPUI's own text elements, some cut: one-line text running past its
/// box, text truncated with "…", a paragraph clamped to two lines; and
/// some not: a line that fits, a paragraph that wraps in full.
struct Texts;

const PAST: &str = "A long line that runs past the side of its box";
const TRUNCATED: &str = "Truncated to the width of its box with an ellipsis";
const CLAMPED: &str = "A paragraph wrapped to its box and clamped to two lines, so the rest of it never shows at all";
const FITS: &str = "Fits";
const WRAPS: &str = "A paragraph that wraps and shows in full";

impl Render for Texts {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
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

/// What GPUI's text elements cut is read whole (and shown whole in a tip,
/// `olo_ui::cut`): agents find each as a label by its whole text; what
/// shows in full isn't one.
#[test]
fn ui_lint_cut_gpui_text_is_read_whole() {
    let mut lint = Lint::new();
    let trees = lint.trees(&WindowOptions::new("Texts", 360.0, 300.0), |_, cx| cx.new(|_| Texts));
    let labels: Vec<String> = trees[0].1.iter().filter(|n| n.role == "Label").filter_map(|n| n.name.clone()).collect();
    for cut in [PAST, TRUNCATED, CLAMPED] {
        assert!(labels.iter().any(|l| l == cut), "{cut:?} isn't read whole: {labels:#?}");
    }
    for whole in [FITS, WRAPS] {
        assert!(!labels.iter().any(|l| l == whole), "{whole:?} shows in full but is read as cut: {labels:#?}");
    }
}
