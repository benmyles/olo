//! The Interface Kit's first window: a view with some state, a message
//! type, a few elements and a drawing of its own.
//!
//!     cargo run -p olo-interface --example hello

use olo_interface::{Bounds, ControlState, Cx, Element, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, Styled as _, View, WindowOptions, control_canvas, div, fill, px, rgb};

struct Hello {
    count: u32,
}

#[derive(Clone)]
enum Msg {
    Add,
    Reset,
}

impl View for Hello {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Add => self.count += 1,
            Msg::Reset => self.count = 0,
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let button = |label: &'static str| {
            div().w(px(96.)).h(px(28.)).child(
                control_canvas(move |p, bounds: Bounds<Pixels>, state: ControlState| {
                    let ground = if state.pressed {
                        rgb(0x9a9a9a)
                    } else if state.hovered {
                        rgb(0xf4f4f4)
                    } else {
                        rgb(0xd8d8d8)
                    };
                    p.paint_quad(fill(bounds, ground));
                    let _ = label;
                })
                .size_full(),
            )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .bg(rgb(0xd8d8d8))
            .text_color(rgb(0x000000))
            .child(format!("Pressed {} times", self.count).into_element())
            .child(div().flex().gap(px(8.)).child(button("Add").on_click(cx.msg(Msg::Add))).child(button("Reset").on_click(cx.msg(Msg::Reset))))
    }
}

fn main() {
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).try_init();
    olo_interface::run("olo.hello", |app| {
        app.open_window(WindowOptions::new("Hello", 320., 160.), |_| Hello { count: 0 });
    });
}
