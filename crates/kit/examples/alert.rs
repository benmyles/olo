//! A window that asks with an alert, the same as olo-ui's `alert_gpui`
//! example: the two are compared pixel for pixel
//! (`tools/dev/kit-compare … --window Erase`).

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{Button, StringView, open_alert},
};

struct Asks {
    said: String,
}

#[derive(Clone)]
enum Msg {
    Ask,
    Answered(Option<usize>),
}

impl View for Asks {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Ask => {
                let answered = cx.msg_with(|choice: &Option<usize>| Msg::Answered(*choice));
                open_alert(cx, "Erase", "warning", "Erase “Backup”? Everything on it will be lost, and this can't be undone.", &["Cancel", "Erase"], answered);
            }
            Msg::Answered(choice) => {
                self.said = match choice {
                    Some(1) => "Erased.".into(),
                    Some(_) => "Cancelled.".into(),
                    None => "Closed.".into(),
                };
                cx.invalidate();
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(Button::new("Ask").on_click(cx.msg(Msg::Ask))).child(StringView::new(self.said.clone()))
    }
}

fn main() {
    olo_kit::run("olo.alert-example", |_| {
        olo_kit::open_window(WindowOptions::new("Asks", 240.0, 100.0), |cx| {
            // (Once the window is up and has the keyboard: the alert
            // opens over it.)
            cx.after(std::time::Duration::from_millis(400), Msg::Ask);
            Asks { said: "Nothing asked yet.".into() }
        });
    });
}
