//! Lines drawn by the GPU's segment primitive (`Window::paint_segment`):
//! a fan at many angles, widths from a hairline to six points, butt and
//! round ends, and a graph's polyline, to look at closely at every scale.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, canvas, div, point, px},
    style, theme,
};

struct Lines;

impl Render for Lines {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            canvas(
                |_, _, _| {},
                |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let ink = style::c(p.panel_text);
                    let o = b.origin;
                    // A fan: 24 angles from one point.
                    let c = point(o.x + px(110.0), o.y + px(110.0));
                    for i in 0..24 {
                        let a = i as f32 / 24.0 * std::f32::consts::TAU;
                        let to = point(c.x + px(a.cos() * 96.0), c.y + px(a.sin() * 96.0));
                        window.paint_segment(c, to, px(1.0), false, ink);
                    }
                    // Widths, butt then round.
                    for (i, w) in [0.5f32, 1.0, 1.5, 2.0, 3.0, 4.5, 6.0].into_iter().enumerate() {
                        let y = o.y + px(20.0 + i as f32 * 22.0);
                        window.paint_segment(point(o.x + px(240.0), y), point(o.x + px(330.0), y + px(10.0)), px(w), false, ink);
                        window.paint_segment(point(o.x + px(350.0), y), point(o.x + px(440.0), y + px(10.0)), px(w), true, ink);
                    }
                    // A graph's polyline.
                    let mut last = None;
                    for i in 0..=100 {
                        let x = o.x + px(20.0 + i as f32 * 4.2);
                        let y = o.y + px(300.0 - (i as f32 * 0.21).sin() * 40.0 - (i as f32 * 0.05).cos() * 20.0);
                        if let Some(prev) = last {
                            window.paint_segment(prev, point(x, y), px(1.5), true, style::c(p.control_highlight));
                        }
                        last = Some(point(x, y));
                    }
                },
            )
            .size_full(),
        )
    }
}

fn main() {
    olo_ui::run("olo.lines-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Lines", 460.0, 360.0), |_, cx| cx.new(|_| Lines));
    });
}
