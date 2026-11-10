//! The kit's graphs, spectra, cell grids, cards and meters in one
//! window, the same as olo-ui's `charts_gpui` example draws them on
//! GPUI: the two are compared pixel for pixel (`tools/dev/kit-compare`).
//! Still, at time 60, unless CHARTS_LIVE is set: then time runs and the
//! graphs scroll (for `tools/dev/cost`).

use std::{collections::VecDeque, rc::Rc, time::Instant};

use olo_kit::{
    Cx, Element, Hsla, ParentElement as _, SharedString, Styled as _, View, WindowOptions, canvas, div, metrics as m,
    palette::Hue,
    px, style,
    widgets::{
        CellGrid, Graph, GridCell, LegendItem, Series, Spectrum, SpectrumSeries, StringView, charts,
        dashboard::{self, Card, Facts},
        legend, legend_height,
    },
};

struct Charts {
    started: Instant,
    live: bool,
    picked: Option<usize>,
    said: String,
}

#[derive(Clone)]
enum Msg {
    Said(String),
    Picked(usize),
}

/// Samples every half second from 0 to `until`.
fn wave(f: f64, k: f64, until: f64) -> Rc<VecDeque<(f64, f64)>> {
    Rc::new((0..=(until * 2.0) as usize).map(|i| (i as f64 * 0.5, k * (1.2 + (i as f64 * f).sin() + 0.3 * (i as f64 * f * 3.1).cos()))).collect())
}

fn levels(n: usize, f: f32) -> Vec<f32> {
    (0..n).map(|i| 0.5 + 0.5 * (i as f32 * f).sin()).collect()
}

impl View for Charts {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Said(said) => self.said = said,
            Msg::Picked(i) => {
                self.picked = Some(i);
                self.said = format!("Picked cell {i}.");
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = olo_kit::theme::palette();
        let hue = |h: Hue| style::c(p.hue(h));
        let (blue, red, green) = (hue(Hue::Blue), hue(Hue::Red), hue(Hue::Green));
        let until = if self.live { 600.0 } else { 60.0 };
        let series = [Series::new("In", blue, wave(0.11, 40.0, until)).fill(true), Series::new("Out", red, wave(0.07, 25.0, until))];
        // (Live: each graph reads the time as it draws itself, and only
        // the graphs are drawn as it passes.)
        let (live, started) = (self.live, self.started);
        let graph_of = move |id: &'static str, series: [Series; 2]| if live { Graph::live(id, series, move || 60.0 + started.elapsed().as_secs_f64()) } else { Graph::new(id, series, 60.0) };
        let graph = graph_of("graph", series.clone()).span(60.0).grid(15.0).unit(|v| format!("{v:.0} KB/s")).on_click(cx.msg_with(|t: &f64| Msg::Said(format!("Clicked at {t:.0} s."))));
        let spectrum = Spectrum::new(
            [
                SpectrumSeries::new("Original", blue, (0..48).map(|i| -70.0 + 50.0 * (i as f32 * 0.21).sin().abs()).collect::<Vec<f32>>()).fill(true),
                SpectrumSeries::new("Processed", red, (0..48).map(|i| -80.0 + 40.0 * (i as f32 * 0.13).cos().abs()).collect::<Vec<f32>>()),
            ],
            50.0,
            16_000.0,
        );
        let items = vec![
            LegendItem { color: blue, name: "In".into(), value: "48 KB/s".into() },
            LegendItem { color: red, name: "Out".into(), value: "12 KB/s".into() },
            LegendItem { color: Hsla::default(), name: "Since it started".into(), value: "1.2 GB".into() },
        ];
        let legend_h = legend_height(268.0, &items);
        let legend_el = canvas(move |g, b| legend(g, b, &items)).w_full().h(px(legend_h)).flex_shrink_0();
        let parts = vec![(blue, 5.2), (green, 2.1), (red, 0.6)];
        let meter = canvas(move |g, b| dashboard::meter(g, b, &parts, 16.0)).w_full().h(px(8.0)).flex_shrink_0();
        let facts = |rows: &[(&'static str, &'static str)]| Facts::new(rows.iter().map(|(a, b)| (SharedString::from(*a), SharedString::from(*b))).collect::<Vec<_>>());
        let network = Card::new("network", "Network")
            .figure("48 KB/s")
            .caption("Wi-Fi, 866 Mb/s")
            .child(div().h(px(70.0)).flex_shrink_0().child(graph_of("card-graph", series).span(60.0).grid(15.0).plain()))
            .child(legend_el);
        let memory = Card::new("memory", "Memory").figure("49%").figure_ink(style::c(p.warning)).on_click(cx.msg(Msg::Said("Opened Memory.".into()))).child(meter).child(facts(&[
            ("Used", "7.9 GB"),
            ("Cached", "2.1 GB"),
            ("A long label here", "A value long enough that it is cut short where the card ends"),
        ]));
        let disks = Card::new("disks", "Disks").figure("3").selected(true).child(facts(&[("Olo", "212 GB free")]));
        let cells: Vec<GridCell> = (0..20)
            .map(|i| GridCell {
                color: (i % 3 != 2).then(|| if i % 2 == 0 { blue } else { green }),
                mark: (i % 5 == 0).then(|| "✦".into()),
                name: format!("Cell {i}").into(),
                selected: self.picked == Some(i),
            })
            .collect();
        let grid = CellGrid::new(cells.clone()).rows(2).cell_size(14.0).fill().on_click(cx.msg_with(|i: &usize| Msg::Picked(*i)));
        let strip = CellGrid::new(cells[..7].to_vec()).cell_size(12.0).slots(25);
        let small = |kind: usize| {
            canvas(move |g, b| {
                let p = olo_kit::theme::palette();
                let inner = charts::paint_well(g, b, &p);
                let ink = style::c(p.hue(Hue::Blue));
                match kind {
                    0 => {
                        let points: Vec<(f64, f32)> = (0..=120).map(|i| (i as f64 * 0.5, 0.5 + 0.4 * (i as f32 * 0.17).sin())).collect();
                        charts::paint_trace(g, inner, &points, 60.0, 30.0, ink, true)
                    }
                    1 => charts::paint_bars(g, inner, &levels(8, 0.9), ink),
                    2 => charts::paint_heatmap(g, inner, &levels(16, 0.7), p.document_background, p.hue(Hue::Red)),
                    _ => charts::paint_meter(g, inner, &levels(3, 1.3), ink),
                }
            })
            .paint_key(kind as u64)
            .w(px(64.0))
            .h(px(26.0))
            .flex_shrink_0()
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().flex().gap(px(m::SPACING)).h(px(120.0)).flex_shrink_0().child(div().flex_1().min_w_0().child(graph)).child(div().flex_1().min_w_0().child(spectrum)))
            .child(
                div()
                    .flex()
                    .gap(px(dashboard::GAP))
                    .p(px(dashboard::GAP))
                    .bg(style::c(dashboard::gutter(&p)))
                    .child(div().w(px(300.0)).flex().child(network))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(dashboard::GAP)).child(memory).child(disks)),
            )
            .child(grid)
            .child(strip)
            .child(div().flex().gap(px(m::SPACING)).child(small(0)).child(small(1)).child(small(2)).child(small(3)))
            .child(StringView::new(self.said.clone()).dim(true))
    }
}

fn main() {
    olo_kit::run("olo.charts-example", |_| {
        olo_kit::open_window(WindowOptions::new("Charts", 660.0, 560.0), |_| Charts {
            started: Instant::now(),
            live: std::env::var_os("CHARTS_LIVE").is_some(),
            picked: Some(3),
            said: "Nothing yet.".into(),
        });
    });
}
