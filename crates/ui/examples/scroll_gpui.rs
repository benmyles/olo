//! The GPUI kit's scroll views in one window, laid out exactly as
//! olo-kit's `scroll` example lays out its own: the two are compared
//! pixel for pixel (`tools/dev/kit-compare`) while apps move from this
//! kit to that one.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, ScrollHandle, Styled as _, Window, div, px},
    metrics as m,
    widgets::{Button, CheckBox, ScrollView, StringView},
};

struct Scrolls {
    rows: ScrollHandle,
    wide: ScrollHandle,
    few: ScrollHandle,
    checked: Vec<bool>,
    count: usize,
}

impl Render for Scrolls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let rows = div().flex().flex_col().gap(px(m::TIGHT)).p(px(m::SPACING)).children((0..self.checked.len()).map(|i| {
            let view = view.clone();
            CheckBox::new(format!("Row {} of the list", i + 1), self.checked[i]).on_toggle(move |on, _, cx| {
                view.update(cx, |s, cx| {
                    s.checked[i] = on;
                    cx.notify();
                })
            })
        }));
        let wide = div()
            .w(px(520.0))
            .p(px(m::SPACING))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .children((0..14).map(|i| StringView::new(format!("Line {} is a good deal wider than the view that shows it, so it scrolls across.", i + 1))));
        let few = div().flex().flex_col().p(px(m::SPACING)).children((0..self.count).map(|i| StringView::new(format!("Item {}", i + 1))));
        let more = view.clone();
        let bottom = view.clone();
        div().size_full().flex().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().w(px(230.0)).h_full().child(ScrollView::new("rows", &self.rows).child(rows))).child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(div().w(px(240.0)).h(px(160.0)).child(ScrollView::new("wide", &self.wide).horizontal(true).child(wide)))
                .child(div().w(px(240.0)).h(px(100.0)).child(ScrollView::new("few", &self.few).auto_hide(true).child(few)))
                .child(
                    div()
                        .flex()
                        .gap(px(m::SPACING))
                        .child(Button::new("More").on_click(move |_, cx| {
                            more.update(cx, |s, cx| {
                                s.count += 3;
                                cx.notify();
                            })
                        }))
                        .child(Button::new("Bottom").on_click(move |_, cx| {
                            bottom.update(cx, |s, cx| {
                                s.rows.scroll_to_bottom();
                                cx.notify();
                            })
                        })),
                ),
        )
    }
}

fn main() {
    olo_ui::run("olo.scroll", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Scroll", 530.0, 380.0), |_, cx| {
            cx.new(|_| Scrolls { rows: ScrollHandle::new(), wide: ScrollHandle::new(), few: ScrollHandle::new(), checked: (0..60).map(|i| i % 3 == 0).collect(), count: 3 })
        });
    });
}
