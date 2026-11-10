//! The GPUI kit's slim scroll bar over a view of numbered lines, laid
//! out exactly as olo-kit's `slim` example lays out its own: the two
//! are compared pixel for pixel (`tools/dev/kit-compare`).

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollWheelEvent, Styled as _, Window, canvas, div, px},
    metrics as m, style,
    widgets::{SlimScrollBar, StringView},
};

const LINES: f32 = 200.0;
const SHOWN: f32 = 12.0;
const ROW: f32 = 18.0;

struct Slim {
    top: f32,
}

impl Render for Slim {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let top = self.top;
        let lines = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = olo_ui::theme::palette(cx);
                style::fill_rect(window, b, style::c(p.document_background));
                let first = top.floor() as usize;
                for i in 0..SHOWN as usize {
                    let mid = b.top() + px((i as f32 + 0.5) * ROW);
                    style::label(
                        window,
                        cx,
                        b.left() + px(6.0),
                        mid,
                        style::Align::Left,
                        format!("Line {}", first + i + 1),
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::c(p.document_text),
                    );
                }
            },
        )
        .size_full();
        let (me, wheel) = (cx.entity(), cx.entity());
        let bar = SlimScrollBar::new("slim", top, SHOWN, LINES).on_scroll(move |to, _, cx| {
            me.update(cx, |v, cx| {
                v.top = to;
                cx.notify();
            })
        });
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
                    .on_scroll_wheel(move |e: &ScrollWheelEvent, _, cx| {
                        let dy = f32::from(e.delta.pixel_delta(px(ROW)).y) / ROW;
                        wheel.update(cx, |v, cx| {
                            // (To an eighth of a line: three notches of nearly three lines are nine.)
                            v.top = (((v.top - dy) * 8.0).round() / 8.0).clamp(0.0, LINES - SHOWN);
                            cx.notify();
                        })
                    })
                    .child(lines)
                    // (olo-ui's fills its parent in the flow: a box of its own over the view.)
                    .child(div().absolute().inset_0().child(bar)),
            )
            .child(StringView::new(format!("At line {:.0}.", top.floor() + 1.0)).dim(true))
    }
}

fn main() {
    olo_ui::run("olo.slim-example", |cx| {
        let top = std::env::var("SLIM_TOP").ok().and_then(|v| v.parse().ok()).unwrap_or(40.0);
        olo_ui::open_window(cx, WindowOptions::new("Slim", 320.0, 290.0), |_, cx| cx.new(|_| Slim { top }));
    });
}
