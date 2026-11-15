//! A forecast's `Timeline`: two days by the hour from a fixed now, made
//! up (temperature with dew point and a freezing guide, cloud over its
//! base, rain's columns by chance, wind arrows, a sky strip), the nights
//! shaded and one span named. Hover it for the readout. Weather's charts
//! are this widget (apps/weather/src/view.rs).

use std::rc::Rc;

use olo_kit::{
    Cx, Element, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, WindowOptions, div, metrics as m,
    palette::Hue,
    px, style,
    widgets::{
        ScrollView, StringView,
        timeline::{Panel, Scale, Span, Timeline, Track},
    },
};

/// Saturday 14 November 2026, 09:00 UTC (01:00 at the place, UTC−8):
/// the example's now, so it draws the same every time.
const NOW: f64 = 1_794_646_800.0;
const HOUR: f64 = 3600.0;
const HOURS: usize = 48;

struct Example {
    scroll: ScrollHandle,
}

fn hourly(f: impl Fn(f64) -> f64) -> Rc<[(f64, f64)]> {
    (0..=HOURS).map(|i| (NOW + i as f64 * HOUR, f(i as f64))).collect()
}

fn panels() -> Vec<Panel> {
    let p = olo_kit::theme::palette();
    let hue = |h: Hue| style::c(p.hue(h));
    let (orange, teal, blue, grey) = (hue(Hue::Orange), hue(Hue::Teal), hue(Hue::Blue), hue(Hue::Gray));
    let temp = hourly(|h| 4.0 + 5.0 * ((h - 7.0) / 24.0 * std::f64::consts::TAU).sin());
    let dew = hourly(|h| 1.5 + 3.0 * ((h - 9.0) / 24.0 * std::f64::consts::TAU).sin());
    let base = hourly(|h| if (9.0..15.0).contains(&h) { 1500.0 + 120.0 * (h - 8.0) } else { 400.0 + 30.0 * (h % 7.0) });
    let rain = hourly(|h| if (2.0..8.0).contains(&h) { 0.4 + 0.2 * (h % 3.0) } else { 0.0 });
    let chance: Rc<[f32]> = (0..=HOURS).map(|i| if (2..8).contains(&i) { 0.9 } else { 0.25 }).collect();
    let wind = hourly(|h| 200.0 + 4.0 * h);
    let skies: Rc<[(f64, SharedString)]> = (0..=HOURS)
        .step_by(3)
        .map(|i| {
            (
                NOW + i as f64 * HOUR,
                SharedString::from(if (2..8).contains(&i) {
                    "weather-rain"
                } else if (9..15).contains(&i) {
                    "weather-partly-cloudy-day"
                } else {
                    "weather-cloudy"
                }),
            )
        })
        .collect();
    vec![
        Panel::icons(skies),
        Panel::plot("Temperature (°C)", 110.0, Scale::new(|v| format!("{v:.0}°")))
            .track(Track::line("Temperature", orange, temp))
            .track(Track::line("Dew point", teal, dew).thin())
            .guide(0.0, "Freezing"),
        Panel::plot("Cloud base (m)", 100.0, Scale::new(|v| format!("{v:.0}")).from_zero().at_least(3000.0)).track(Track::above("Cloud base", grey, base)),
        Panel::plot("Rain (mm/h)", 80.0, Scale::new(|v| format!("{v:.1}")).from_zero().at_least(1.0)).track(Track::columns("Rain", blue, rain).alpha(chance)),
        Panel::arrows(wind, grey),
    ]
}

impl View for Example {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let nights: Rc<[(f64, f64)]> = Rc::new([(NOW - HOUR, NOW + 6.25 * HOUR), (NOW + 15.5 * HOUR, NOW + 30.25 * HOUR), (NOW + 39.5 * HOUR, NOW + 49.0 * HOUR)]);
        let spans: Rc<[Span]> = Rc::new([Span { start: NOW + 9.0 * HOUR, end: NOW + 14.0 * HOUR, name: "Bases over 1,500 m".into() }]);
        // The place's clock: UTC−8 (Seattle's, in November).
        let chart = Timeline::new("example-timeline", NOW, NOW + HOURS as f64 * HOUR, HOUR, panels(), 1).now(NOW + 0.5 * HOUR).nights(nights).spans(spans).local(|t| t as i64 - 8 * 3600);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::SPACING))
            .child(StringView::new("Two days, made up. Hover the chart for every value at an hour.").dim(true))
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).child(div().p(px(m::SPACING)).child(chart))))
    }
}

fn main() {
    olo_kit::run("olo.timeline-example", |_| {
        olo_kit::open_window(WindowOptions::new("Timeline", 760.0, 540.0), |_| Example { scroll: ScrollHandle::new() });
    });
}
