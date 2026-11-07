//! Diagnostic phase timing via existing worker progress callbacks. Adds timing
//! overhead; use page_bench for uninstrumented end-to-end comparisons.
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
use std::{
    hint::black_box,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn phase(p: page::Phase) -> usize {
    match p {
        page::Phase::Html | page::Phase::Scripts => 0,
        page::Phase::Styles => 1,
        page::Phase::Boxes => 2,
        page::Phase::Text => 3,
        page::Phase::Layout => 4,
        page::Phase::Paint => 5,
    }
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "100".into()).parse().unwrap();
    assert!(rounds > 0);
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut rows = Vec::new();
    for count in [100, 1000] {
        let source: Arc<str> = Arc::from(format!(
            "<!doctype html><title>Pipeline</title><style>body{{margin:0}}div{{display:block;border-width:1px;border-style:solid;border-color:red;padding:3px}}</style>{}",
            "<div>office alpha beta gamma</div>".repeat(count)
        ));
        let viewport = layout::Viewport { width: 800., height: 600., scale: 1. };
        let warm = page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        let mut geometry = Sha256::new();
        for c in warm.scene.commands() {
            for n in [c.bounds.x, c.bounds.y, c.bounds.width, c.bounds.height] {
                geometry.update(n.to_le_bytes());
            }
        }
        let mut elapsed = [Duration::ZERO; 7];
        let started = Instant::now();
        for _ in 0..rounds {
            let mut at = Instant::now();
            let mut previous = 0;
            let loaded = page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, viewport, None, |p, _| {
                let next = phase(p);
                if next != previous {
                    let now = Instant::now();
                    elapsed[previous] += now - at;
                    previous = next;
                    at = now;
                }
            })
            .unwrap();
            let now = Instant::now();
            elapsed[previous] += now - at;
            drop(black_box(loaded));
            elapsed[6] += now.elapsed();
        }
        let total = started.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let phases: serde_json::Map<_, _> = ["html", "styles", "boxes", "text", "layout", "paint", "dispose"]
            .into_iter()
            .zip(elapsed)
            .map(|(name, elapsed)| (name.to_owned(), json!(elapsed.as_secs_f64() * 1e6 / rounds as f64)))
            .collect();
        rows.push(json!({"elements":count,"total_us":total,"phase_us":phases,"commands":warm.scene.commands().len(),"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes())),"geometry_sha256":format!("{:x}",geometry.finalize())}));
    }
    println!(
        "{}",
        json!({"scope":"Instrumented phase diagnostic; excludes raster/GPU/presentation. Styles includes stylesheet parsing and cascade. Dispose includes all page destruction. Callback instrumentation changes code generation; confirm conclusions with page_bench.","features":build_features::enabled(),"rounds":rounds,"rows":rows})
    );
}
