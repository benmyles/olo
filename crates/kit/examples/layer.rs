//! A layer surface: a bar along the top of the screen (keeping windows
//! below it) with a button opening a menu over it, to try the kit's
//! layer shell and popups over layer surfaces.

use olo_kit::{
    Bounds, Corner, Cx, Element, LayerShellLayer, Menu, ParentElement as _, Styled as _, View, WindowOptions, div, menu, metrics as m, point, px, size,
    widgets::{Button, StringView},
};

struct Bar {
    chosen: String,
}

#[derive(Clone)]
enum Msg {
    Menu,
    Chose(String),
}

impl View for Bar {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Menu => {
                let anchor = Bounds { origin: point(px(4.0), px(2.0)), size: size(px(48.0), px(20.0)) };
                let m = Menu::new("Bar").item("First", "first").item("Second", "second").separator().submenu(Menu::new("More").item("Third", "third"));
                let chose = cx.msg_with(|command: &str| Msg::Chose(command.to_owned()));
                menu::popup(cx, m, anchor, Corner::BottomLeft, chose);
            }
            Msg::Chose(command) => {
                self.chosen = command;
                cx.invalidate();
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .px(px(m::TIGHT))
            .child(Button::new("Menu").small().on_click(cx.msg(Msg::Menu)))
            .child(StringView::new(format!("Chosen: {}", self.chosen)))
    }
}

fn main() {
    olo_kit::run("olo.layer-example", |_| {
        let options = WindowOptions::new("Bar", 400.0, 24.0).layer("example-bar", LayerShellLayer::Top, (true, false, true, true)).layer_options(|l| l.exclusive_zone = 24);
        olo_kit::open_window(options, |_| Bar { chosen: "nothing".into() });
    });
}
