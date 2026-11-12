//! The kit's BeOS controls in one window (to compare with a build from
//! before a change, pixel for pixel: `tools/dev/kit-compare`).

use olo_kit::{
    Child, Cx, Element, InteractiveElement as _, Menu, MenuBar, MouseButton, ParentElement as _, Styled as _, View, WindowOptions, div, menu, metrics as m, px,
    widgets::{BBox, Button, CheckBox, LinkButton, MenuField, RadioButton, StringView, TextControl, TextField, TextFieldEvent, heading, separator, text_field},
};

struct Controls {
    wrap: bool,
    sound: bool,
    radio: usize,
    clicks: u32,
    speed: usize,
    name: Child<TextField>,
    hinted: Child<TextField>,
    off: Child<TextField>,
    notes: Child<TextField>,
    /// What the name field last said.
    said: String,
}

#[derive(Clone)]
enum Msg {
    Wrap(bool),
    Sound(bool),
    Radio(usize),
    Speed(usize),
    Click,
}

impl View for Controls {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Wrap(on) => self.wrap = on,
            Msg::Sound(on) => self.sound = on,
            Msg::Radio(i) => self.radio = i,
            Msg::Speed(i) => self.speed = i,
            Msg::Click => self.clicks += 1,
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "count" => self.clicks += 1,
            "reset" => self.clicks = 0,
            "quit" => olo_kit::quit(),
            _ => return false,
        }
        cx.invalidate();
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let content = self.content(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Right, |e, cx| {
                menu::context_menu(cx, menus().remove(1), e.position, menu::on_command);
            })
            .child(MenuBar::new(cx, menus()))
            .child(content)
    }
}

/// The window's menus.
fn menus() -> Vec<Menu> {
    vec![
        Menu::new("File").item("Count", "count").key("k").separator().item("Quit", "quit").key("q"),
        Menu::new("Edit").item("Reset", "reset").item("Disabled", "none").enabled(false).separator().submenu(Menu::new("More").item("One", "one").item("Two", "two").checked(true)),
    ]
}

impl Controls {
    fn content(&mut self, cx: &mut Cx<Self>) -> Element {
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
            .child(div().flex().gap(px(m::SPACING)).child(StringView::new(format!("Clicked {} times.{}", self.clicks, self.said))).child(LinkButton::new("Reset")))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(TextControl::new(cx, &self.name).label("Name:").width(200.0))
                    .child(TextControl::new(cx, &self.hinted).width(120.0))
                    .child(MenuField::new("speed", vec!["Slow".into(), "Medium".into(), "Fast".into()], self.speed).label("Speed:").on_change(cx.msg_with(|i: &usize| Msg::Speed(*i)))),
            )
            .child(div().flex().gap(px(m::SPACING)).child(TextControl::new(cx, &self.off).label("Off:").width(160.0)).child(MenuField::new("none", vec!["Nothing to choose".into()], 0).enabled(false)))
            .child(div().w(px(300.0)).h(px(54.0)).child(&self.notes))
            .child(div().w(px(300.0)).child(StringView::new("A paragraph of text that is long enough to wrap onto a second line in the three hundred points it is given here.").wrap(true)))
            .child(div().w(px(160.0)).child(StringView::new("A long line cut short to fit the box it is in").cut(true)))
    }
}

fn main() {
    olo_kit::run("olo.controls", |_| {
        olo_kit::open_window(WindowOptions::new("Controls", 540.0, 640.0), |cx| {
            let name = text_field(cx, |f| f.name("Name").with_text("Olo"));
            cx.subscribe(&name, |c: &mut Controls, e: &TextFieldEvent, cx| {
                c.said = match e {
                    TextFieldEvent::Changed => " Changed.".into(),
                    TextFieldEvent::Entered => " Entered.".into(),
                    _ => return,
                };
                cx.invalidate();
            });
            let off = text_field(cx, |f| f.name("Off").with_text("Can't be typed in"));
            off.update(cx, |f, cx| f.set_enabled(false, cx));
            Controls {
                wrap: true,
                sound: false,
                radio: 0,
                clicks: 0,
                speed: 1,
                name,
                hinted: text_field(cx, |f| f.name("Search").hint("Search")),
                off,
                notes: text_field(cx, |f| f.name("Notes").multiline(false).with_text("Several lines of text in a field that wraps what is typed to its width.\nA second paragraph.")),
                said: String::new(),
            }
        });
    });
}
