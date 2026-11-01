//! A layer surface: a bar along the top of the screen (keeping windows
//! below it) with a button opening a menu over it, to try the GPUI kit's
//! layer shell and popups over layer surfaces.

use olo_ui::{
    Menu, WindowOptions,
    gpui::{AppContext as _, Context, Corner, IntoElement, LayerShellLayer, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m, menu,
    widgets::{Button, StringView},
};

struct Bar {
    chosen: String,
}

impl Render for Bar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .px(px(m::TIGHT))
            .child(Button::new("Menu").small().on_click(move |window, cx| {
                let view = view.clone();
                let anchor = olo_ui::gpui::Bounds { origin: olo_ui::gpui::point(px(4.0), px(2.0)), size: olo_ui::gpui::size(px(48.0), px(20.0)) };
                let m = Menu::new("Bar").item("First", "first").item("Second", "second").separator().submenu(Menu::new("More").item("Third", "third"));
                menu::popup(window, cx, m, anchor, Corner::BottomLeft, move |command, _, cx| {
                    let command = command.to_owned();
                    view.update(cx, |b, cx| {
                        b.chosen = command;
                        cx.notify();
                    });
                });
            }))
            .child(StringView::new(format!("Chosen: {}", self.chosen)))
    }
}

fn main() {
    olo_ui::run("olo.layer-example", |cx| {
        let options = WindowOptions::new("Bar", 400.0, 24.0).layer("example-bar", LayerShellLayer::Top, (true, false, true, true)).layer_options(|l| l.exclusive_zone = 24);
        olo_ui::open_window(cx, options, |_, cx| cx.new(|_| Bar { chosen: "nothing".into() }));
    });
}
