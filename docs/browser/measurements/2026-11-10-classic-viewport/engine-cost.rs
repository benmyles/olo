use olo_web::{
    css::{media::Media, style::ScrollbarWidths},
    dom::{Document, NodeId},
    layout, page, paint,
    text::shaping,
};
use std::{hint::black_box, sync::Arc, time::Instant};
#[path = "/home/ben/worktrees/olo/browser/crates/web/tests/support/classic_host.rs"]
mod classic_host;
fn find(d: &Document, name: &str) -> NodeId {
    let mut stack = vec![d.root()];
    while let Some(n) = stack.pop() {
        if d.name(n).ok().flatten() == Some(name) {
            return n;
        }
        stack.extend(d.children(n).unwrap());
    }
    panic!("missing {name}")
}
fn main() {
    println!("[");
    let mut first = true;
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        for classic in [false, true, true, false] {
            let scope = classic_host::scope();
            let view = layout::Viewport { width: 1000., height: 700., scale };
            let p = page::prepare(
                Arc::from("<!doctype html><style>html,body{margin:0}div{height:2000px;width:100vw}</style><div></div>"),
                &scope,
                Media::screen(1000., 700., scale, false),
                false,
                None,
                |_, _| {},
                |_, _, _| Ok(None),
            )
            .unwrap();
            let mut host = classic_host::Classic::new(p.scope().clone(), view);
            if !classic {
                host.widths = ScrollbarWidths::default()
            }
            let measurer = p.measurer();
            let mut shaper = shaping::Engine::default();
            let mut painter = paint::Engine::default();
            let mut page = page::finish(p, &host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
            let mut prev = None;
            let mut measurement = Vec::new();
            let mut rendering = Vec::new();
            let html = find(&page.document, "html");
            let div = find(&page.document, "div");
            for i in 0..1020 {
                page.document.set_attribute(html, "style", ["", "overflow:scroll", "scrollbar-gutter:stable both-edges", "scrollbar-width:thin"][i % 4]).unwrap();
                page.document.set_attribute(div, "style", if i % 3 == 0 { "height:200px" } else { "height:2000px" }).unwrap();
                let t = Instant::now();
                let m = measurer.measure(&page.document, &host, &mut shaper, view, prev, None).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.;
                if i >= 20 {
                    measurement.push(ms)
                }
                let t = Instant::now();
                page.restyle(&host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.;
                if i >= 20 {
                    rendering.push(ms)
                }
                assert_eq!(m.layout().viewport_geometry(), page.scene.layout().viewport_geometry());
                prev = Some(m);
            }
            let m = prev.unwrap();
            let t = Instant::now();
            for _ in 0..100000 {
                black_box(m.document_size());
                black_box(m.layout().viewport_geometry());
            }
            let reads = t.elapsed().as_secs_f64() * 1000.;
            measurement.sort_by(f64::total_cmp);
            rendering.sort_by(f64::total_cmp);
            if !first {
                println!(",");
            }
            first = false;
            print!(
                "{{\"scale\":{scale},\"classic\":{classic},\"measure_p50_ms\":{},\"measure_p95_ms\":{},\"render_p50_ms\":{},\"render_p95_ms\":{},\"cached_100000_ms\":{reads}}}",
                measurement[500], measurement[950], rendering[500], rendering[950]
            );
        }
    }
    println!("\n]");
}
