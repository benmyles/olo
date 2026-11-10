//! The GPUI kit's BeOS controls in one window, laid out exactly as
//! olo-kit's `controls` example lays out its own: the two are compared
//! pixel for pixel (`tools/dev/kit-compare`) while apps move from this
//! kit to that one.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{BBox, Button, CheckBox, LinkButton, RadioButton, StringView, heading, separator},
};

struct Controls {
    wrap: bool,
    sound: bool,
    radio: usize,
    clicks: u32,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let set = move |f: fn(&mut Controls, bool)| {
            let view = view.clone();
            move |on: bool, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    f(g, on);
                    cx.notify();
                })
            }
        };
        let radio = |i: usize| {
            let view = cx.entity();
            move |_: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    g.radio = i;
                    cx.notify();
                })
            }
        };
        let click = || {
            let view = cx.entity();
            move |_: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    g.clicks += 1;
                    cx.notify();
                })
            }
        };
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(heading("Controls"))
            .child(StringView::new("A line of plain text, as tall as a control."))
            .child(StringView::new("Secondary text, dimmed.").dim(true))
            .child(
                div()
                    .mt(px(4.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(
                        BBox::new("Options")
                            .content_width(180.0)
                            .child(CheckBox::new("Wrap lines", self.wrap).on_toggle(set(|g, on| g.wrap = on)))
                            .child(CheckBox::new("Play sounds", self.sound).on_toggle(set(|g, on| g.sound = on)))
                            .child(CheckBox::new("Mixed", false).mixed(true))
                            .child(CheckBox::new("Disabled", true).enabled(false)),
                    )
                    .child(
                        BBox::new("Mode")
                            .child(RadioButton::new("Light", self.radio == 0).on_select(radio(0)))
                            .child(RadioButton::new("Dark", self.radio == 1).on_select(radio(1)))
                            .child(RadioButton::new("Automatic", self.radio == 2).on_select(radio(2))),
                    ),
            )
            .child(separator())
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Count").on_click(click()))
                    .child(Button::new("Small").small())
                    .child(Button::new("Pushed").pushed(true))
                    .child(Button::new("Disabled").enabled(false))
                    .child(Button::new("OK").default(true).on_click(click())),
            )
            .child(div().flex().gap(px(m::SPACING)).child(StringView::new(format!("Clicked {} times.", self.clicks))).child(LinkButton::new("Reset")))
            .child(div().w(px(300.0)).child(StringView::new("A paragraph of text that is long enough to wrap onto a second line in the three hundred points it is given here.").wrap(true)))
            .child(div().w(px(160.0)).child(StringView::new("A long line cut short to fit the box it is in").cut(true)))
    }
}

fn main() {
    olo_ui::run("olo.controls", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Controls", 480.0, 460.0), |_, cx| cx.new(|_| Controls { wrap: true, sound: false, radio: 0, clicks: 0 }));
    });
}
