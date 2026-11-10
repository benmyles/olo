//! A window full of cards of text, each with a button: something to
//! measure the kit's frames on (`OLO_FRAME_PHASES=1`), a small change in
//! a big window.
//!
//!     cargo run --release -p olo-interface --example cards -- [COUNT]

use olo_interface::{Bounds, ControlState, Cx, Element, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, Styled as _, View, WindowOptions, control_canvas, div, fill, px, rgb};

struct Cards {
    count: usize,
    pressed: Vec<u32>,
}

#[derive(Clone)]
enum Msg {
    Press(usize),
}

const WORDS: &str = "The quick brown fox jumps over the lazy dog while a looper takes its messages one at a time and sleeps when there are none";

impl View for Cards {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Press(i) => self.pressed[i] += 1,
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let card = |i: usize, pressed: u32| {
            div()
                .id(i)
                .w(px(220.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0x808080))
                .child(format!("Card {i}: pressed {pressed}").into_element())
                .child(WORDS.into_element())
                .child(
                    div().w(px(80.)).h(px(20.)).child(
                        control_canvas(move |p, bounds: Bounds<Pixels>, state: ControlState| {
                            let ground = if state.pressed {
                                rgb(0x9a9a9a)
                            } else if state.hovered {
                                rgb(0xffcb00)
                            } else {
                                rgb(0xd8d8d8)
                            };
                            p.paint_quad(fill(bounds, ground));
                        })
                        .size_full()
                        .on_click(cx.msg(Msg::Press(i))),
                    ),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(8.))
            .p(px(8.))
            .bg(rgb(0xd8d8d8))
            .text_color(rgb(0x000000))
            .text_size(px(12.))
            .children((0..self.count).map(|i| card(i, self.pressed[i])))
    }
}

fn main() {
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).try_init();
    let count = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(60);
    olo_interface::run("olo.cards", |app| {
        app.open_window(WindowOptions::new("Cards", 1200., 800.), move |_| Cards { count, pressed: vec![0; count] });
    });
}
