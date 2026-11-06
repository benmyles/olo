//! New inline-box path; no before counterpart. No pixels or website acceptance.
#[path = "support/features.rs"]
mod build_features;
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
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
        let source: Arc<str> = format!(
            "<!doctype html><style>body{{margin:0}}div{{width:100px}}span{{margin-left:3px;padding-left:5%;padding-right:7px;margin-right:5px}}</style>{}",
            "<div>A <span>B C D</span> E</div>".repeat(count)
        )
        .into();
        let mut run = || page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {}).unwrap();
        let warm = run();
        let commands = warm.scene.commands().len();
        let inlines: usize = warm
            .scene
            .layout()
            .cache()
            .tree()
            .iter()
            .filter_map(|(_, b)| match b.kind {
                layout::BoxKind::InlineContent { source } => Some(warm.scene.layout().cache().tree().inline_boxes(source).unwrap().len()),
                _ => None,
            })
            .sum();
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(run());
        }
        rows.push(serde_json::json!({"elements":count,"inline_boxes":inlines,"commands":commands,"pipeline_us":start.elapsed().as_secs_f64()*1e6/rounds as f64}));
    }
    println!("{}", serde_json::json!({"scope":"new inline spacing path; excludes raster, GPU and presentation", "features":build_features::enabled(),"rounds":rounds,"rows":rows}));
}
