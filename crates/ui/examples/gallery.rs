//! The GPUI kit's widgets in one window, laid out as the egui kit's demo
//! (crates/kit/examples/demo.rs, its Controls tab), for comparing the
//! two closely (`tsession` at every scale and theme).

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div, px},
    metrics as m,
    widgets::{BBox, Button, CheckBox, MenuField, RadioButton, StringView, TextControl, TextField, text_field},
};

const FONTS: &[&str] = &["Nimbus Sans", "DejaVu Sans", "Noto Sans", "Liberation Sans"];

struct Gallery {
    name: Entity<TextField>,
    wrap: bool,
    sound: bool,
    radio: usize,
    choice: usize,
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let set = move |f: fn(&mut Gallery, bool)| {
            let view = view.clone();
            move |on: bool, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    f(g, on);
                    cx.notify();
                })
            }
        };
        let radio = |i: usize| {
            let set = set(|_, _| {});
            let view = cx.entity();
            move |window: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, _| g.radio = i);
                set(true, window, cx);
            }
        };
        let choose = {
            let view = cx.entity();
            move |i: usize, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    g.choice = i;
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
            .child(TextControl::new(self.name.clone()).label("Name:").divider(60.0))
            .child(MenuField::new("font", FONTS.iter().map(|&f| f.into()).collect(), self.choice).label("Font:").divider(60.0).on_change(choose))
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
                            .child(CheckBox::new("Disabled", true).enabled(false)),
                    )
                    .child(
                        BBox::new("Mode")
                            .child(RadioButton::new("Light", self.radio == 0).on_select(radio(0)))
                            .child(RadioButton::new("Dark", self.radio == 1).on_select(radio(1)))
                            .child(RadioButton::new("Automatic", self.radio == 2).on_select(radio(2))),
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Copy Name"))
                    .child(Button::new("Disabled").enabled(false))
                    .child(Button::new("OK").default(true)),
            )
            .child(StringView::new("Dim text for hints.").dim(true))
            .child(div().id("rows").h(px(100.0)).w(px(200.0)).overflow_y_scroll().children((1..=40).map(|i| StringView::new(format!("Row {i}")))))
    }
}

fn main() {
    olo_ui::run("olo.gallery", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Gallery", 460.0, 360.0), |_, cx| {
            let name = text_field(cx, |f| f.name("Name"));
            name.update(cx, |f, cx| f.set_text("Hello, BeOS", cx));
            cx.new(|_| Gallery { name, wrap: true, sound: false, radio: 0, choice: 0 })
        });
    });
}
