//! A window drawing in HDR10 (`Window::set_gamut(Gamut::Hdr10)`): an
//! sRGB white, then two HDR10 pictures (`RenderImage::deep`, half floats
//! of PQ in BT.2020) at 1,000 and 100 cd/m², side by side, each a third
//! of it. Read back from a pretend HDR screen as a display would be sent
//! it (`tools/dev/hdr-check`'s `--gpui`): PQ's codes for 203 cd/m² (SDR's
//! white), 1,000 and 100.
//!
//! `cargo run --release -p olo-ui --example hdr_probe`

use std::sync::Arc;

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Bounds, Context, Corners, Gamut, IntoElement, ParentElement as _, Render, RenderImage, Styled as _, Window, canvas, div, fill, point, px, rgb, size},
};

/// PQ's signal for `nits`.
fn pq(nits: f64) -> f32 {
    let y = (nits / 10_000.0).powf(2610.0 / 16384.0);
    (((3424.0 / 4096.0) + (2413.0 / 128.0) * y) / (1.0 + (2392.0 / 128.0) * y)).powf(2523.0 / 32.0) as f32
}

/// A half float's bits for `v` in 0–1 (normal numbers: enough here).
fn half(v: f32) -> u16 {
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

struct Probe {
    pictures: [Arc<RenderImage>; 2],
    asked: bool,
    /// Said once it's on an HDR display (it enters its display after
    /// its first frame).
    told: bool,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.asked {
            self.asked = true;
            let drawn = window.set_gamut(Gamut::Hdr10);
            eprintln!("hdr_probe: drawing in HDR10: {drawn}");
        }
        if !self.told && window.on_hdr_display() {
            self.told = true;
            eprintln!("hdr_probe: on an HDR display");
        }
        if !self.told {
            window.request_animation_frame();
        }
        let pictures = self.pictures.clone();
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let third = b.size.width / 3.0;
                    let cell = |i: f32| Bounds::new(point(b.origin.x + third * i, b.origin.y), size(third, b.size.height));
                    window.paint_quad(fill(cell(0.0), rgb(0xFFFFFF)));
                    for (i, p) in pictures.iter().enumerate() {
                        let _ = window.paint_image(cell(1.0 + i as f32), Corners::default(), p.clone(), 0, false);
                    }
                },
            )
            .size_full(),
        )
    }
}

fn main() {
    olo_ui::run("olo.hdr-probe", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("HDR Probe", 360.0, 120.0), |_, cx| cx.new(|_| Probe { pictures: [flat(1000.0), flat(100.0)], asked: false, told: false }));
    });
}
