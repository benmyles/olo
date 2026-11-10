//! More of the kit's widgets in one window (a find bar, a sidebar, tabs,
//! a status bar, sliders, key hints, a banner, a colour control,
//! thumbnails), the same as olo-ui's `widgets_gpui` example draws them
//! on GPUI: the two are compared pixel for pixel (`tools/dev/kit-compare`).

use olo_kit::{
    Child, Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div, hsla, metrics as m, px,
    widgets::{
        Banner, Button, ColorControl, ColorWell, KeyHint, Sidebar, Slider, StatusBar, StringView, TabView, TextField, Thumbnail, ThumbnailPicker, Tone, find_bar, find_said, sidebar::Row, text_field,
    },
};

struct Widgets {
    find: Child<TextField>,
    section: usize,
    tab: usize,
    volume: f32,
    rgb: [u8; 3],
    picked: usize,
    said: String,
}

#[derive(Clone)]
enum Msg {
    Section(usize),
    Tab(usize),
    Volume(f32),
    Colour([u8; 3]),
    Picked(usize),
    Step(bool),
    Close,
}

impl View for Widgets {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Section(i) => self.section = i,
            Msg::Tab(i) => self.tab = i,
            Msg::Volume(v) => self.volume = v,
            Msg::Colour(rgb) => self.rgb = rgb,
            Msg::Picked(i) => self.picked = i,
            Msg::Step(back) => self.said = if back { "Previous.".into() } else { "Next.".into() },
            Msg::Close => self.said = "Closed.".into(),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let rows = vec![Row::heading("Monitor"), Row::new("", "Processor").value("12%"), Row::new("", "Memory").value("48%"), Row::heading("Storage"), Row::new("", "Disks"), Row::new("", "Backups")];
        let sidebar = Sidebar::new("sections", rows, self.section).on_choose(cx.msg_with(|i: &usize| Msg::Section(*i))).build(cx);
        let colour = ColorControl::new("accent", self.rgb).on_change(cx.msg_with(|rgb: &[u8; 3]| Msg::Colour(*rgb))).build(cx);
        let [r, g, b] = self.rgb;
        let thumbs: Vec<Thumbnail> =
            [("Dawn", 0.08), ("Noon", 0.55), ("Dusk", 0.75)].into_iter().map(|(label, hue)| Thumbnail { picture: None, color: Some(hsla(hue, 0.6, 0.6, 1.0)), label: label.into() }).collect();
        let found = find_said("row", 90, Some(2), None);
        let bar = find_bar(cx, &self.find, found, true, cx.msg_with(|back: &bool| Msg::Step(*back)), cx.msg(Msg::Close));
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(StatusBar::new(0.4).label("Copying").trailing("3 of 7"))
            .child(Slider::new("volume", self.volume, 0.0..=100.0).label("Volume:").value_text(format!("{:.0} %", self.volume)).limits("Off", "Loud").on_change(cx.msg_with(|v: &f32| Msg::Volume(*v))))
            .child(Slider::new("off", 30.0, 0.0..=100.0).label("Balance:").enabled(false))
            .child(div().flex().gap(px(m::SPACING * 2.0)).child(KeyHint::new("Space", "Next")).child(KeyHint::new("Esc", "Stop").enabled(false)))
            .child(Banner::new(if self.tab == 0 { Tone::Success } else { Tone::Failure }, "This step is done", "Everything was copied.").trailing(Button::new("Next")))
            .child(div().flex().gap(px(m::SPACING)).child(colour).child(ColorWell::new(olo_kit::rgb(u32::from_be_bytes([0, r, g, b])).into(), 40.0)))
            .child(ThumbnailPicker::new("skies", thumbs, Some(self.picked)).picture_size(96.0, 54.0).width(420.0).on_select(cx.msg_with(|i: &usize| Msg::Picked(*i))))
            .child(StringView::new(self.said.clone()).dim(true));
        let tabs: Vec<olo_kit::SharedString> = vec!["General".into(), "Sound".into(), "Display".into()];
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(150.0)).h_full().child(sidebar))
                .child(div().flex_1().min_w_0().p(px(m::WINDOW_INSET)).child(TabView::new("tabs", tabs, self.tab).on_select(cx.msg_with(|i: &usize| Msg::Tab(*i))).child(panel))),
        )
    }
}

fn main() {
    olo_kit::run("olo.widgets-example", |_| {
        olo_kit::open_window(WindowOptions::new("Widgets", 680.0, 640.0), |cx| Widgets {
            find: text_field(cx, |f| f.name("Find").with_text("row")),
            section: 1,
            tab: 0,
            volume: 60.0,
            rgb: [200, 120, 40],
            picked: 1,
            said: "Nothing yet.".into(),
        });
    });
}
