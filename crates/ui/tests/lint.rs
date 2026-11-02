//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale, and controls put over each other are found.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    lint::{Lint, assert_clean},
    metrics as m,
    widgets::{Button, CheckBox, RadioButton, TextControl, text_field},
};

/// A form: a labelled field, choices, and buttons along the bottom.
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
            .child(RadioButton::new("Automatically", true))
            .child(RadioButton::new("By hand", false))
            .child(buttons)
    }
}

fn form(stacked: bool) -> Vec<olo_ui::lint::Found> {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Form", 360.0, 200.0);
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
