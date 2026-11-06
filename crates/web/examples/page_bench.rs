//! Complete static worker pipeline benchmark shared with the parent revision.
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
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "50".into()).parse().unwrap();
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
        let mut run = || page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {}).unwrap();
        let warm = run();
        let mut geometry = Sha256::new();
        for c in warm.scene.commands() {
            for n in [c.bounds.x, c.bounds.y, c.bounds.width, c.bounds.height] {
                geometry.update(n.to_le_bytes());
            }
        }
        let commands = warm.scene.commands().len();
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(run());
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        rows.push(
            json!({"elements":count,"pipeline_us":micros,"commands":commands,"geometry_sha256":format!("{:x}",geometry.finalize()),"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes()))}),
        );
    }
    println!("{}", json!({"features":build_features::enabled(),"rounds":rounds,"rows":rows}));
}
