//! More of the GPUI kit's widgets in one window, laid out exactly as
//! olo-kit's `widgets` example lays out its own: the two are compared
//! pixel for pixel (`tools/dev/kit-compare`).

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, hsla, px, rgb},
    metrics as m,
    widgets::{
        Banner, Button, ColorControl, ColorWell, KeyHint, Sidebar, Slider, StatusBar, StringView, TabView, TextField, Thumbnail, ThumbnailPicker, Tone, find_bar, find_said, sidebar::Row, text_field,
    },
};

struct Widgets {
    find: Entity<TextField>,
    section: usize,
    tab: usize,
    volume: f32,
    rgb: [u8; 3],
    picked: usize,
    said: String,
}

impl Render for Widgets {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let set = move |f: fn(&mut Widgets, usize)| {
            let me = me.clone();
            move |i: usize, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                me.update(cx, |w, cx| {
                    f(w, i);
                    cx.notify();
                })
            }
        };
        let rows = vec![Row::heading("Monitor"), Row::new("", "Processor").value("12%"), Row::new("", "Memory").value("48%"), Row::heading("Storage"), Row::new("", "Disks"), Row::new("", "Backups")];
        let sidebar = Sidebar::new("sections", rows, self.section).on_choose(set(|w, i| w.section = i));
        let (volume, colour, step, close) = (cx.entity(), cx.entity(), cx.entity(), cx.entity());
        let [r, g, b] = self.rgb;
        let thumbs: Vec<Thumbnail> =
            [("Dawn", 0.08), ("Noon", 0.55), ("Dusk", 0.75)].into_iter().map(|(label, hue)| Thumbnail { picture: None, color: Some(hsla(hue, 0.6, 0.6, 1.0)), label: label.into() }).collect();
        let found = find_said("row", 90, Some(2), None);
        let bar = find_bar(
            &self.find,
            found,
            true,
            move |back, _, cx| {
                step.update(cx, |w, cx| {
                    w.said = if back { "Previous.".into() } else { "Next.".into() };
                    cx.notify();
                })
            },
            move |_, cx| {
                close.update(cx, |w, cx| {
                    w.said = "Closed.".into();
                    cx.notify();
                })
            },
        );
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(StatusBar::new(0.4).label("Copying").trailing("3 of 7"))
            .child(Slider::new("volume", self.volume, 0.0..=100.0).label("Volume:").value_text(format!("{:.0} %", self.volume)).limits("Off", "Loud").on_change(move |v, _, cx| {
                volume.update(cx, |w, cx| {
                    w.volume = v;
                    cx.notify();
                })
            }))
            .child(Slider::new("off", 30.0, 0.0..=100.0).label("Balance:").enabled(false))
            .child(div().flex().gap(px(m::SPACING * 2.0)).child(KeyHint::new("Space", "Next")).child(KeyHint::new("Esc", "Stop").enabled(false)))
            .child(Banner::new(if self.tab == 0 { Tone::Success } else { Tone::Failure }, "This step is done", "Everything was copied.").trailing(Button::new("Next")))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(ColorControl::new("accent", self.rgb).on_change(move |rgb, _, cx| {
                        colour.update(cx, |w, cx| {
                            w.rgb = rgb;
                            cx.notify();
                        })
                    }))
                    .child(ColorWell::new(rgb(u32::from_be_bytes([0, r, g, b])).into(), 40.0)),
            )
            .child(ThumbnailPicker::new("skies", thumbs, Some(self.picked)).picture_size(96.0, 54.0).width(420.0).on_select(set(|w, i| w.picked = i)))
            .child(StringView::new(self.said.clone()).dim(true));
        let tabs: Vec<SharedString> = vec!["General".into(), "Sound".into(), "Display".into()];
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().w(px(150.0)).h_full().child(sidebar))
                .child(div().flex_1().min_w_0().p(px(m::WINDOW_INSET)).child(TabView::new("tabs", tabs, self.tab).on_select(set(|w, i| w.tab = i)).child(panel))),
        )
    }
}

fn main() {
    olo_ui::run("olo.widgets-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Widgets", 680.0, 640.0), |_, cx| {
            cx.new(|cx| {
                let find = text_field(cx, |f| f.name("Find"));
                find.update(cx, |f, cx| f.set_text("row", cx));
                Widgets { find, section: 1, tab: 0, volume: 60.0, rgb: [200, 120, 40], picked: 1, said: "Nothing yet.".into() }
            })
        });
    });
}
