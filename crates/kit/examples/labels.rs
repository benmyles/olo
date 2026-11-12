//! Labels at whole and fractional places (to compare with a build from
//! before a change, glyph by glyph: `tools/dev/kit-compare`).

use olo_kit::{Cx, Element, FontWeight, Styled as _, View, WindowOptions, canvas, px, style};

struct Labels;

impl View for Labels {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        canvas(|g, b| {
            let p = olo_kit::theme::palette();
            let ink = style::c(p.panel_text);
            let mut y = b.top() + px(20.0);
            for (i, (text, size, weight)) in
                [("The Tester", 16.0, FontWeight::BOLD), ("ben", 13.0, FontWeight::NORMAL), ("Tuesday, November 10", 20.0, FontWeight::BOLD), ("Enter your password", 13.0, FontWeight::NORMAL)]
                    .into_iter()
                    .enumerate()
            {
                for (k, x) in [16.0, 96.0, 203.0, 406.0].into_iter().enumerate() {
                    style::label(g, b.left() + px(x + 150.0 * (k / 2) as f32), y, style::Align::Left, text, size, style::ui_font(weight), ink);
                }
                y += px(30.0 + 2.0 * i as f32);
            }
        })
        .size_full()
    }
}

fn main() {
    olo_kit::run("olo.labels-example", |_| {
        olo_kit::open_window(WindowOptions::new("Labels", 760.0, 170.0), |_| Labels);
    });
}
