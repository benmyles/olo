//! The GPUI kit's alert, opened by a window as olo-kit's `alert` example
//! opens its own: the two are compared pixel for pixel
//! (`tools/dev/kit-compare … --window Erase`).

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{Button, StringView, open_alert},
};

struct Asks {
    said: String,
}

impl Asks {
    fn ask(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity();
        open_alert(cx, "Erase", "warning", "Erase “Backup”? Everything on it will be lost, and this can't be undone.", &["Cancel", "Erase"], move |choice, cx| {
            me.update(cx, |a, cx| {
                a.said = match choice {
                    Some(1) => "Erased.".into(),
                    Some(_) => "Cancelled.".into(),
                    None => "Closed.".into(),
                };
                cx.notify();
            })
        });
    }
}

impl Render for Asks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(Button::new("Ask").on_click(move |_, cx| me.update(cx, |a, cx| a.ask(cx))))
            .child(StringView::new(self.said.clone()))
    }
}

fn main() {
    olo_ui::run("olo.alert-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Asks", 240.0, 100.0), |_, cx| {
            cx.new(|cx| {
                // (Once the window is up and has the keyboard: the alert
                // opens over it.)
                cx.spawn(async move |asks, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    let _ = asks.update(cx, |asks: &mut Asks, cx| asks.ask(cx));
                })
                .detach();
                Asks { said: "Nothing asked yet.".into() }
            })
        });
    });
}
