//! Labels at whole and fractional places on the GPUI kit, laid out
//! exactly as olo-kit's `labels` example lays out its own.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, FontWeight, IntoElement, Render, Styled as _, Window, canvas, px},
    style,
};

struct Labels;

impl Render for Labels {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |b, _, window, cx| {
                let p = olo_ui::theme::palette(cx);
                let ink = style::c(p.panel_text);
                let mut y = b.top() + px(20.0);
                for (i, (text, size, weight)) in
                    [("The Tester", 16.0, FontWeight::BOLD), ("ben", 13.0, FontWeight::NORMAL), ("Tuesday, November 10", 20.0, FontWeight::BOLD), ("Enter your password", 13.0, FontWeight::NORMAL)]
                        .into_iter()
                        .enumerate()
                {
                    for (k, x) in [16.0, 96.0, 203.0, 406.0].into_iter().enumerate() {
                        style::label(window, cx, b.left() + px(x + 150.0 * (k / 2) as f32), y, style::Align::Left, text, size, style::ui_font(weight), ink);
                    }
                    y += px(30.0 + 2.0 * i as f32);
                }
            },
        )
        .size_full()
    }
}

fn main() {
    olo_ui::run("olo.labels-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Labels", 760.0, 170.0), |_, cx| cx.new(|_| Labels));
    });
}
