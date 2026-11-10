//! The kit's BeOS controls in one window, the same as olo-ui's
//! `controls` example draws them on GPUI: the two are compared pixel
//! for pixel (`tools/dev/kit-compare`).

use olo_kit::{
    Cx, Element, IntoElement as _, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, LinkButton, RadioButton, StringView, heading, separator},
};

struct Controls {
    wrap: bool,
    sound: bool,
    radio: usize,
    clicks: u32,
}

#[derive(Clone)]
enum Msg {
    Wrap(bool),
    Sound(bool),
    Radio(usize),
    Click,
}

impl View for Controls {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Wrap(on) => self.wrap = on,
            Msg::Sound(on) => self.sound = on,
            Msg::Radio(i) => self.radio = i,
            Msg::Click => self.clicks += 1,
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
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
                            .child(CheckBox::new("Wrap lines", self.wrap).on_toggle(cx.msg_with(|on: &bool| Msg::Wrap(*on))))
                            .child(CheckBox::new("Play sounds", self.sound).on_toggle(cx.msg_with(|on: &bool| Msg::Sound(*on))))
                            .child(CheckBox::new("Mixed", false).mixed(true))
                            .child(CheckBox::new("Disabled", true).enabled(false)),
                    )
                    .child(
                        BBox::new("Mode")
                            .child(RadioButton::new("Light", self.radio == 0).on_select(cx.msg(Msg::Radio(0))))
                            .child(RadioButton::new("Dark", self.radio == 1).on_select(cx.msg(Msg::Radio(1))))
                            .child(RadioButton::new("Automatic", self.radio == 2).on_select(cx.msg(Msg::Radio(2)))),
                    ),
            )
            .child(separator())
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Count").on_click(cx.msg(Msg::Click)))
                    .child(Button::new("Small").small())
                    .child(Button::new("Pushed").pushed(true))
                    .child(Button::new("Disabled").enabled(false))
                    .child(Button::new("OK").default(true).on_click(cx.msg(Msg::Click))),
            )
            .child(div().flex().gap(px(m::SPACING)).child(StringView::new(format!("Clicked {} times.", self.clicks))).child(LinkButton::new("Reset")))
            .child(div().w(px(300.0)).child(StringView::new("A paragraph of text that is long enough to wrap onto a second line in the three hundred points it is given here.").wrap(true)))
    }
}

fn main() {
    olo_kit::run("olo.controls", |_| {
        olo_kit::open_window(WindowOptions::new("Controls", 480.0, 420.0), |_| Controls { wrap: true, sound: false, radio: 0, clicks: 0 });
    });
}
