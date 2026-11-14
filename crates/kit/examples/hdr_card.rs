//! An HDR test card for looking at a display by eye: a window drawing in
//! HDR10 (`WindowCx::set_gamut(Gamut::Hdr10)`), each patch labelled.
//!
//! Top row: an sRGB white (SDR's white), then HDR10 patches at 400, 1,000
//! and 1,600 cd/m². On a display decoding PQ they're much brighter than
//! the white; shown without decoding (PQ's codes taken as gamma) they're
//! only a little brighter.
//!
//! Bottom row: dark greys at 0.1, 0.5, 2 and 10 cd/m². Decoded, the first
//! two are next to black; undecoded they're all plainly grey (PQ gives
//! dark tones high codes), the picture washed out.
//!
//! `cargo run --release -p olo-kit --example hdr_card`

use std::{sync::Arc, time::Duration};

use olo_kit::{
    Bounds, Corners, Cx, Element, FontWeight, Gamut, Painter, ParentElement as _, Pixels, RenderImage, Styled as _, View, WindowOptions, canvas, div, fill, hsla, point, px, rgb, size,
    style::{self, Align},
};

/// PQ's signal for `nits`.
fn pq(nits: f64) -> f32 {
    let y = (nits / 10_000.0).powf(2610.0 / 16384.0);
    (((3424.0 / 4096.0) + (2413.0 / 128.0) * y) / (1.0 + (2392.0 / 128.0) * y)).powf(2523.0 / 32.0) as f32
}

/// A half float's bits for `v` in 0–1 (0 and normal numbers).
fn half(v: f32) -> u16 {
    if v <= 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    (((exp.max(0) as u32) << 10) | ((bits & 0x7F_FFFF) + 0x1000) >> 13) as u16
}

/// A flat HDR10 picture of `nits`.
fn flat(nits: f64) -> Arc<RenderImage> {
    let v = half(pq(nits));
    let rgba: Vec<u16> = (0..16 * 16).flat_map(|_| [v, v, v, half(1.0)]).collect();
    Arc::new(RenderImage::deep(16, 16, &rgba, Gamut::Hdr10))
}

const BRIGHT: [f64; 3] = [400.0, 1000.0, 1600.0];
const DARK: [f64; 4] = [0.1, 0.5, 2.0, 10.0];

struct Card {
    bright: Vec<Arc<RenderImage>>,
    dark: Vec<Arc<RenderImage>>,
}

/// Ask for HDR10 (once the window is up: the compositor is asked).
struct Ask;

impl View for Card {
    type Message = Ask;

    fn message(&mut self, _: Ask, cx: &mut Cx<Self>) {
        let drawn = cx.set_gamut(Gamut::Hdr10, false);
        eprintln!("hdr_card: drawing in HDR10: {drawn}");
        cx.invalidate();
    }

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let (bright, dark) = (self.bright.clone(), self.dark.clone());
        div().size_full().bg(rgb(0x000000)).child(
            canvas(move |g, b| {
                let pad = px(12.0);
                let label_h = px(26.0);
                let row_h = (b.size.height - pad * 3.0 - label_h * 2.0) / 2.0;
                let ink = hsla(0.0, 0.0, 0.85, 1.0);
                let font = style::ui_font(FontWeight::NORMAL);
                let row = |g: &mut Painter, y: Pixels, names: &[String], paint: &dyn Fn(&mut Painter, usize, Bounds<Pixels>)| {
                    let n = names.len();
                    let w = (b.size.width - pad * (n as f32 + 1.0)) / n as f32;
                    for (i, name) in names.iter().enumerate() {
                        let cell = Bounds::new(point(b.origin.x + pad + (w + pad) * i as f32, y), size(w, row_h));
                        paint(g, i, cell);
                        style::label(g, cell.center().x, cell.bottom() + label_h / 2.0, Align::Center, name.clone(), 13.0, font.clone(), ink);
                    }
                };
                let top = b.origin.y + pad;
                let names: Vec<String> = std::iter::once("SDR white".to_owned()).chain(BRIGHT.iter().map(|n| format!("{n} cd/m²"))).collect();
                row(g, top, &names, &|g, i, cell| {
                    if i == 0 {
                        g.paint_quad(fill(cell, rgb(0xFFFFFF)));
                    } else {
                        let _ = g.paint_image(cell, Corners::default(), bright[i - 1].clone(), 0, false);
                    }
                });
                let names: Vec<String> = DARK.iter().map(|n| format!("{n} cd/m²")).collect();
                row(g, top + row_h + label_h + pad, &names, &|g, i, cell| {
                    let _ = g.paint_image(cell, Corners::default(), dark[i].clone(), 0, false);
                });
            })
            .size_full(),
        )
    }
}

fn main() {
    olo_kit::run("olo.hdr-card", |_| {
        olo_kit::open_window(WindowOptions::new("HDR Test Card", 640.0, 360.0), |cx| {
            cx.after(Duration::from_millis(200), Ask);
            Card { bright: BRIGHT.iter().map(|&n| flat(n)).collect(), dark: DARK.iter().map(|&n| flat(n)).collect() }
        });
    });
}
