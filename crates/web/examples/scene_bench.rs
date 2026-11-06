//! Repeated retained scene construction, isolated from parse/style/layout.
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
    let controls = std::env::args().nth(2).as_deref() == Some("controls");
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut rows = Vec::new();
    for count in [100, 1000] {
        let body = if controls { "<div>office<wbr> alpha<br> beta gamma</div>" } else { "<div>office alpha beta gamma</div>" };
        let source: Arc<str> = Arc::from(format!(
            "<!doctype html><title>Pipeline</title><style>body{{margin:0}}div{{display:block;border-width:1px;border-style:solid;border-color:red;padding:3px}}</style>{}",
            body.repeat(count)
        ));
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
            let mut j = paint::BuildJob::new(&page.document, page.scene.layout().clone(), &mut painter, Default::default(), Default::default(), None).unwrap();
            while !j.step(32).unwrap().finished {}
            black_box(j.finish().unwrap());
        }
        rows.push(json!({"elements":count,"scene_us":at.elapsed().as_secs_f64()*1e6/rounds as f64,"commands":page.scene.commands().len(),"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes())),"geometry_sha256":format!("{:x}",geometry.finalize()),"pixels_sha256":pixels}));
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"rounds":rounds,"controls":controls,"rows":rows,"scope":"Repeated retained scene construction and destruction. Raster hash is untimed validation only; excludes live updates/GPU/presentation/websites."})
    );
}
