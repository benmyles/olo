//! The GPUI kit's BeOS controls in one window, laid out exactly as
//! olo-kit's `controls` example lays out its own: the two are compared
//! pixel for pixel (`tools/dev/kit-compare`) while apps move from this
//! kit to that one.

use olo_ui::{
    Menu, MenuBar, WindowOptions,
    gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{BBox, Button, CheckBox, LinkButton, MenuField, RadioButton, StringView, TextControl, TextField, TextFieldEvent, heading, separator, text_field},
};

struct Controls {
    wrap: bool,
    sound: bool,
    radio: usize,
    clicks: u32,
    speed: usize,
    name: Entity<TextField>,
    hinted: Entity<TextField>,
    off: Entity<TextField>,
    notes: Entity<TextField>,
    said: String,
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
        let menus = vec![
            Menu::new("File").item("Count", "count").key("k").separator().item("Quit", "quit").key("q"),
            Menu::new("Edit").item("Reset", "reset").item("Disabled", "none").enabled(false).separator().submenu(Menu::new("More").item("One", "one").item("Two", "two").checked(true)),
        ];
        let content = div()
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
            .child(div().flex().gap(px(m::SPACING)).child(StringView::new(format!("Clicked {} times.{}", self.clicks, self.said))).child(LinkButton::new("Reset")))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(TextControl::new(self.name.clone()).label("Name:").width(200.0))
                    .child(TextControl::new(self.hinted.clone()).width(120.0))
                    .child(MenuField::new("speed", vec!["Slow".into(), "Medium".into(), "Fast".into()], self.speed).label("Speed:")),
            )
            .child(
                div().flex().gap(px(m::SPACING)).child(TextControl::new(self.off.clone()).label("Off:").width(160.0)).child(MenuField::new("none", vec!["Nothing to choose".into()], 0).enabled(false)),
            )
            .child(div().w(px(300.0)).h(px(54.0)).child(self.notes.clone()))
            .child(div().w(px(300.0)).child(StringView::new("A paragraph of text that is long enough to wrap onto a second line in the three hundred points it is given here.").wrap(true)))
            .child(div().w(px(160.0)).child(StringView::new("A long line cut short to fit the box it is in").cut(true)));
        div().size_full().flex().flex_col().child(MenuBar::new(menus, |_, _, _| {})).child(content)
    }
}

fn main() {
    olo_ui::run("olo.controls", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Controls", 540.0, 640.0), |_, cx| {
            cx.new(|cx| {
                let name = text_field(cx, |f| f.name("Name"));
                name.update(cx, |f, cx| f.set_text("Olo", cx));
                cx.subscribe(&name, |c: &mut Controls, _, e: &TextFieldEvent, cx| {
                    c.said = match e {
                        TextFieldEvent::Changed => " Changed.".into(),
                        TextFieldEvent::Entered => " Entered.".into(),
                        _ => return,
                    };
                    cx.notify();
                })
                .detach();
                let off = text_field(cx, |f| f.name("Off"));
                off.update(cx, |f, cx| {
                    f.set_text("Can't be typed in", cx);
                    f.set_enabled(false, cx);
                });
                let notes = text_field(cx, |f| f.name("Notes").multiline(false));
                notes.update(cx, |f, cx| f.set_text("Several lines of text in a field that wraps what is typed to its width.\nA second paragraph.", cx));
                Controls { wrap: true, sound: false, radio: 0, clicks: 0, speed: 1, name, hinted: text_field(cx, |f| f.name("Search").hint("Search")), off, notes, said: String::new() }
            })
        });
    });
}
