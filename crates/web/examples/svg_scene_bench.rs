//! Retained SVG scene and CPU tile cost; clipped baseline and visible-overflow mode.
#[path = "support/features.rs"]
mod build_features;
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "200".into()).parse().unwrap();
    assert!(rounds > 0);
    let visible = std::env::args().nth(2).as_deref() == Some("visible");
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut rows = Vec::new();
    for count in [100, 1000] {
        let overflow = if visible { "visible" } else { "hidden" };
        let body = "<svg width=128 height=20 viewBox='0 0 128 20'><path fill='#6050ff' d='M-8 -4H136V24H-8Z'/><path fill='#00a48e' d='M8 16C8 0 64 0 64 16Z'/></svg>";
        let source: Arc<str> = Arc::from(format!("<!doctype html><style>body{{margin:0}}svg{{display:block;overflow:{overflow}}}</style>{}", body.repeat(count)));
        let page = page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {}).unwrap();
        let mut geometry = Sha256::new();
        for c in page.scene.commands() {
            for n in [c.bounds.x, c.bounds.y, c.bounds.width, c.bounds.height] {
                geometry.update(n.to_le_bytes());
            }
        }
        let tile = painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 256, height: 256 }, None).unwrap();
        let pixels = format!("{:x}", Sha256::digest(tile.rgba()));
        let at = Instant::now();
        for _ in 0..rounds {
            black_box(page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {}).unwrap());
        }
        let pipeline_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let at = Instant::now();
        for _ in 0..rounds {
            let mut j = paint::BuildJob::new(&page.document, page.scene.layout().clone(), &mut painter, Default::default(), Default::default(), None).unwrap();
            while !j.step(32).unwrap().finished {}
            black_box(j.finish().unwrap());
        }
        let scene_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let at = Instant::now();
        for _ in 0..rounds {
            black_box(painter.raster(&page.scene, paint::DeviceRect { x: 0, y: 0, width: 256, height: 256 }, None).unwrap());
        }
        let raster_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let vector_bytes: usize = page
            .scene
            .layout()
            .cache()
            .tree()
            .iter()
            .filter_map(|(_, b)| match b.kind {
                layout::BoxKind::BlockVector { vector } => page.scene.layout().cache().tree().vector(vector),
                _ => None,
            })
            .map(|v| std::mem::size_of_val(v) + v.payload_bytes())
            .sum();
        rows.push(json!({"pipeline_us":pipeline_us,"raster_us":raster_us,"vector_bytes":vector_bytes,"command_bytes":page.scene.command_payload_bytes(),"elements":count,"scene_us":scene_us,"commands":page.scene.commands().len(),"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes())),"geometry_sha256":format!("{:x}",geometry.finalize()),"pixels_sha256":pixels}));
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"rounds":rounds,"visible":visible,"rows":rows,"scope":"SVG page pipeline, retained scene construction/destruction and 256x256 CPU tile raster. No network, GPU/presentation or website acceptance."})
    );
}
