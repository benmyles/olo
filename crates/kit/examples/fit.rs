//! A window that follows what it shows (`WindowOptions::fits_content`):
//! as tall as its rows need, growing and shrinking with them, and
//! scrolled once they need more than the screen has room for.

use olo_kit::{
    Cx, Element, IntoElement as _, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{Button, StringView},
};

struct Fit {
    rows: usize,
}

#[derive(Clone)]
enum Msg {
    More,
    Fewer,
}

impl View for Fit {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::More => self.rows += 4,
            Msg::Fewer => self.rows = self.rows.saturating_sub(4),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .p(px(m::WINDOW_INSET))
            .children((0..self.rows).map(|i| StringView::new(format!("Row {} of what the window shows", i + 1))))
            .child(div().flex().gap(px(m::SPACING)).child(Button::new("More").on_click(cx.msg(Msg::More))).child(Button::new("Fewer").on_click(cx.msg(Msg::Fewer))))
    }
}

fn main() {
    olo_kit::run("olo.fit-example", |_| {
        let mut options = WindowOptions::new("Fit", 300.0, 80.0).fits_content();
        options.fixed = true;
        olo_kit::open_window(options, |_| Fit { rows: 3 });
    });
}
