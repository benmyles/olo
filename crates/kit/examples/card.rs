//! A tip's card (`olo_kit::card`): one painted in the window, and the
//! same as the tip of the box beside it (rest the pointer on it).

use std::rc::Rc;

use olo_kit::{
    Element, InteractiveElement as _, ParentElement as _, Styled as _, View, WindowOptions, canvas,
    card::{CARD_INNER, Card, CardPaint, Part, Row},
    div, metrics as m,
    palette::Hue,
    px, style, theme, tip,
    widgets::{StringView, charts},
};

struct Cards;

/// A card like Deskbar's processor widget's.
fn card() -> Card {
    let p = theme::palette();
    let blue = style::c(p.hue(Hue::Blue));
    let graph: CardPaint = Rc::new(move |g, b| {
        let p = theme::palette();
        let inner = charts::paint_well(g, b, &p);
        let points: Vec<(f64, f32)> = (0..=120).map(|i| (i as f64 * 0.5, 0.3 + 0.25 * (i as f32 * 0.17).sin())).collect();
        charts::paint_trace(g, inner, &points, 60.0, 30.0, style::c(p.hue(Hue::Blue)), true);
    });
    let bars: CardPaint = Rc::new(move |g, b| {
        let p = theme::palette();
        let inner = charts::paint_well(g, b, &p);
        let levels: Vec<f32> = (0..16).map(|i| 0.5 + 0.5 * (i as f32 * 0.9).sin()).collect();
        charts::paint_bars(g, inner, &levels, style::c(p.hue(Hue::Blue)));
    });
    Card::new("CPU")
        .value("13%", None)
        .caption("Each core · over the last minute")
        .part(Part::Drawing { heading: None, height: 64.0, paint: graph, text: "From 3% to 31%".into() })
        .part(Part::Drawing { heading: Some("Each core now".into()), height: 32.0, paint: bars, text: "16 cores".into() })
        .part(Part::Rows {
            heading: Some("Using the most".into()),
            rows: vec![
                Row::new("rustc", "6.2%").level(0.062, Some(blue)),
                Row::new("olo-app-server", "0.7%").level(0.007, Some(blue)),
                Row::new("A process whose name is long enough to be cut short", "0.2%").level(0.002, Some(blue)).note("and a note under it"),
            ],
        })
        .part(Part::Notes(vec!["Notes are dim, one line each".into()]))
}

impl View for Cards {
    type Message = ();

    fn body(&mut self, _cx: &mut olo_kit::Cx<Self>) -> Element {
        let shown = Rc::new(card());
        let (w, h) = shown.size();
        let painted = shown.clone();
        let tipped = shown.clone();
        div().size_full().p(px(m::WINDOW_INSET)).flex().gap(px(m::SPACING)).child(canvas(move |g, b| painted.paint(g, b)).w(px(w)).h(px(h)).flex_shrink_0()).child(
            div()
                .id("rest-here")
                .w(px(CARD_INNER / 2.0))
                .h(px(60.0))
                .p(px(m::TIGHT))
                .border_1()
                .border_color(style::c(theme::palette().panel_text))
                .child(StringView::new("Rest the pointer here").wrap(true))
                .on_hover(move |on: &bool, cx| {
                    let at = cx.listener_bounds();
                    tip::hover_card(cx, "rest-here", tipped.clone(), at, *on);
                }),
        )
    }
}

fn main() {
    olo_kit::run("olo.card-example", |_| {
        olo_kit::open_window(WindowOptions::new("Cards", 480.0, 420.0).fits_content(), |_| Cards);
    });
}
