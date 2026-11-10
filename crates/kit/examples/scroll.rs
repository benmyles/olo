//! Scroll views in one window, the same as olo-ui's `scroll_gpui`
//! example draws them on GPUI: the two are compared pixel for pixel
//! (`tools/dev/kit-compare`), and this one is what scrolling is measured
//! with.

use olo_kit::{
    Cx, Element, IntoElement as _, ParentElement as _, ScrollHandle, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{Button, CheckBox, ScrollView, StringView},
};

struct Scrolls {
    rows: ScrollHandle,
    wide: ScrollHandle,
    few: ScrollHandle,
    checked: Vec<bool>,
    count: usize,
}

#[derive(Clone)]
enum Msg {
    Check(usize, bool),
    More,
    Bottom,
}

impl View for Scrolls {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Check(i, on) => self.checked[i] = on,
            Msg::More => self.count += 3,
            Msg::Bottom => self.rows.scroll_to_bottom(cx),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let rows = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .p(px(m::SPACING))
            .children((0..self.checked.len()).map(|i| CheckBox::new(format!("Row {} of the list", i + 1), self.checked[i]).on_toggle(cx.msg_with(move |on: &bool| Msg::Check(i, *on)))));
        let wide = div()
            .w(px(520.0))
            .p(px(m::SPACING))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .children((0..14).map(|i| StringView::new(format!("Line {} is a good deal wider than the view that shows it, so it scrolls across.", i + 1))));
        let few = div().flex().flex_col().p(px(m::SPACING)).children((0..self.count).map(|i| StringView::new(format!("Item {}", i + 1))));
        div().size_full().flex().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().w(px(230.0)).h_full().child(ScrollView::new(&self.rows).child(rows))).child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(div().w(px(240.0)).h(px(160.0)).child(ScrollView::new(&self.wide).horizontal(true).child(wide)))
                .child(div().w(px(240.0)).h(px(100.0)).child(ScrollView::new(&self.few).auto_hide(true).child(few)))
                .child(div().flex().gap(px(m::SPACING)).child(Button::new("More").on_click(cx.msg(Msg::More))).child(Button::new("Bottom").on_click(cx.msg(Msg::Bottom)))),
        )
    }
}

fn main() {
    olo_kit::run("olo.scroll", |_| {
        olo_kit::open_window(WindowOptions::new("Scroll", 530.0, 380.0), |_| Scrolls {
            rows: ScrollHandle::new(),
            wide: ScrollHandle::new(),
            few: ScrollHandle::new(),
            checked: (0..60).map(|i| i % 3 == 0).collect(),
            count: 3,
        });
    });
}
