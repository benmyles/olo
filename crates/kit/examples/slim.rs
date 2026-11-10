//! The kit's slim scroll bar over a view of numbered lines, the same as
//! olo-ui's `slim_gpui` example draws it on GPUI: the two are compared
//! pixel for pixel (`tools/dev/kit-compare`).

use olo_kit::{
    Cx, Element, FontWeight, InteractiveElement as _, ParentElement as _, ScrollWheelEvent, Styled as _, View, WindowOptions, canvas, div, metrics as m, px, style,
    widgets::{SlimScrollBar, StringView},
};

const LINES: f32 = 200.0;
const SHOWN: f32 = 12.0;
const ROW: f32 = 18.0;

struct Slim {
    top: f32,
}

#[derive(Clone)]
enum Msg {
    To(f32),
    By(f32),
}

impl View for Slim {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        self.top = match message {
            Msg::To(top) => top,
            // (To an eighth of a line: three notches of nearly three lines are nine.)
            Msg::By(lines) => (((self.top - lines) * 8.0).round() / 8.0).clamp(0.0, LINES - SHOWN),
        };
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let top = self.top;
        let lines = canvas(move |g, b| {
            let p = olo_kit::theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            let first = top.floor() as usize;
            for i in 0..SHOWN as usize {
                let mid = b.top() + px((i as f32 + 0.5) * ROW);
                style::label(g, b.left() + px(6.0), mid, style::Align::Left, format!("Line {}", first + i + 1), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.document_text));
            }
        })
        .paint_key(top.floor() as u64)
        .size_full();
        let bar = SlimScrollBar::new("slim", top, SHOWN, LINES).on_scroll(cx.msg_with(|to: &f32| Msg::To(*to)));
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(
                div()
                    .id("view")
                    .relative()
                    .w_full()
                    .h(px(SHOWN * ROW))
                    .flex_shrink_0()
                    .on_scroll_wheel(cx.msg_with(|e: &ScrollWheelEvent| Msg::By(f32::from(e.delta.pixel_delta(px(ROW)).y) / ROW)))
                    .child(lines)
                    .child(bar),
            )
            .child(StringView::new(format!("At line {:.0}.", top.floor() + 1.0)).dim(true))
    }
}

fn main() {
    olo_kit::run("olo.slim-example", |_| {
        let top = std::env::var("SLIM_TOP").ok().and_then(|v| v.parse().ok()).unwrap_or(40.0);
        olo_kit::open_window(WindowOptions::new("Slim", 320.0, 290.0), move |_| Slim { top });
    });
}
