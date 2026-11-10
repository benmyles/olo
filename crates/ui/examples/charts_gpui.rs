//! The GPUI kit's graphs, spectra, cell grids, cards and meters in one
//! window, laid out exactly as olo-kit's `charts` example lays out its
//! own: the two are compared pixel for pixel (`tools/dev/kit-compare`).
//! Still, at time 60, unless CHARTS_LIVE is set: then time runs and the
//! graphs scroll (for `tools/dev/cost`).

use std::{collections::VecDeque, rc::Rc, time::Instant};

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, canvas, div, px},
    metrics as m,
    palette::Hue,
    style,
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

/// Samples every half second from 0 to `until`.
fn wave(f: f64, k: f64, until: f64) -> Rc<VecDeque<(f64, f64)>> {
    Rc::new((0..=(until * 2.0) as usize).map(|i| (i as f64 * 0.5, k * (1.2 + (i as f64 * f).sin() + 0.3 * (i as f64 * f * 3.1).cos()))).collect())
}

fn levels(n: usize, f: f32) -> Vec<f32> {
    (0..n).map(|i| 0.5 + 0.5 * (i as f32 * f).sin()).collect()
}

impl Render for Charts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = olo_ui::theme::palette(cx);
        let hue = |h: Hue| style::c(p.hue(h));
        let (blue, red, green) = (hue(Hue::Blue), hue(Hue::Red), hue(Hue::Green));
        let now = if self.live { 60.0 + self.started.elapsed().as_secs_f64() } else { 60.0 };
        let until = if self.live { 600.0 } else { 60.0 };
        let series = [Series::new("In", blue, wave(0.11, 40.0, until)).fill(true), Series::new("Out", red, wave(0.07, 25.0, until))];
        let me = cx.entity();
        let clicked = move |what: &'static str| {
            let me = me.clone();
            move |t: f64, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                me.update(cx, |v, cx| {
                    v.said = format!("{what} at {t:.0} s.");
                    cx.notify();
                })
            }
        };
        let graph = Graph::new("graph", series.clone(), now).span(60.0).grid(15.0).unit(|v| format!("{v:.0} KB/s")).on_click(clicked("Clicked"));
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
        let legend_h = legend_height(cx, 268.0, &items);
        let legend_el = canvas(|_, _, _| {}, move |b, _, window, cx| legend(window, cx, b, &items)).w_full().h(px(legend_h)).flex_shrink_0();
        let parts = vec![(blue, 5.2), (green, 2.1), (red, 0.6)];
        let meter = canvas(|_, _, _| {}, move |b, _, window, cx| dashboard::meter(window, cx, b, &parts, 16.0)).w_full().h(px(8.0)).flex_shrink_0();
        let facts = |rows: &[(&'static str, &'static str)]| Facts::new(rows.iter().map(|(a, b)| (SharedString::from(*a), SharedString::from(*b))).collect::<Vec<_>>());
        let me = cx.entity();
        let network = Card::new("network", "Network")
            .figure("48 KB/s")
            .caption("Wi-Fi, 866 Mb/s")
            .child(div().h(px(70.0)).flex_shrink_0().child(Graph::new("card-graph", series, now).span(60.0).grid(15.0).plain()))
            .child(legend_el);
        let memory = Card::new("memory", "Memory")
            .figure("49%")
            .figure_ink(style::c(p.warning))
            .on_click(move |_, cx| {
                me.update(cx, |v, cx| {
                    v.said = "Opened Memory.".into();
                    cx.notify();
                })
            })
            .child(meter)
            .child(facts(&[("Used", "7.9 GB"), ("Cached", "2.1 GB"), ("A long label here", "A value long enough that it is cut short where the card ends")]));
        let disks = Card::new("disks", "Disks").figure("3").selected(true).child(facts(&[("Olo", "212 GB free")]));
        let cells: Vec<GridCell> = (0..20)
            .map(|i| GridCell {
                color: (i % 3 != 2).then(|| if i % 2 == 0 { blue } else { green }),
                mark: (i % 5 == 0).then(|| "✦".into()),
                name: format!("Cell {i}").into(),
                selected: self.picked == Some(i),
            })
            .collect();
        let me = cx.entity();
        let grid = CellGrid::new(cells.clone()).rows(2).cell_size(14.0).fill().on_click(move |i, _, cx| {
            me.update(cx, |v, cx| {
                v.picked = Some(i);
                v.said = format!("Picked cell {i}.");
                cx.notify();
            })
        });
        let strip = CellGrid::new(cells[..7].to_vec()).cell_size(12.0).slots(25);
        let small = |kind: usize| {
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = olo_ui::theme::palette(cx);
                    let inner = charts::paint_well(window, b, &p);
                    let ink = style::c(p.hue(Hue::Blue));
                    match kind {
                        0 => {
                            let points: Vec<(f64, f32)> = (0..=120).map(|i| (i as f64 * 0.5, 0.5 + 0.4 * (i as f32 * 0.17).sin())).collect();
                            charts::paint_trace(window, inner, &points, 60.0, 30.0, ink, true)
                        }
                        1 => charts::paint_bars(window, inner, &levels(8, 0.9), ink),
                        2 => charts::paint_heatmap(window, inner, &levels(16, 0.7), p.document_background, p.hue(Hue::Red)),
                        _ => charts::paint_meter(window, inner, &levels(3, 1.3), ink),
                    }
                },
            )
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
    olo_ui::run("olo.charts-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Charts", 660.0, 560.0), |_, cx| {
            cx.new(|_| Charts { started: Instant::now(), live: std::env::var_os("CHARTS_LIVE").is_some(), picked: Some(3), said: "Nothing yet.".into() })
        });
    });
}
