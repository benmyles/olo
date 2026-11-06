//! New-capability cost: retained-document full restyles, with no HTML or I/O.
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
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
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
        let source: Arc<str> =
            Arc::from(format!("<!doctype html><style>body{{margin:0}}a{{color:red}}a:hover{{color:green}}</style>{}", "<div><a href=x>office alpha beta gamma</a></div>".repeat(count)));
        let viewport = layout::Viewport { width: 800., height: 600., scale: 1. };
        let mut page = page::load(source, &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        let node = page.scene.commands().iter().enumerate().find_map(|(i, _)| page.scene.text_node(i as u32)).unwrap();
        let identity = page.document.id();
        let start = Instant::now();
        for r in 0..rounds {
            page.interaction.hover = (r % 2 == 0).then_some(node);
            page.restyle(&host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
            black_box(&page.scene);
        }
        assert_eq!(page.document.id(), identity);
        rows.push(json!({"link_rows":count,"full_restyle_us":start.elapsed().as_secs_f64()*1e6/rounds as f64,"commands":page.scene.commands().len(),"command_payload_bytes":page.scene.command_payload_bytes()}));
    }
    println!("{}", json!({"features":build_features::enabled(),"rounds":rounds,"command_bytes":std::mem::size_of::<paint::Command>(),"rows":rows}));
}
