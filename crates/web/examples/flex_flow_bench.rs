//! Initial page flex formatter cost: identical geometry with first intrinsic
//! measurement versus retained measurements. Excludes parse/style/preparation,
//! paint/raster, network, JS, UI and presentation; not a before-engine speedup.
#[path = "support/features.rs"]
mod build_features;
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy, Scope},
    layout, page, paint,
    text::shaping::Engine,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
fn fit(engine: &mut Engine, scope: &Scope, cache: Arc<layout::Cache>, width: f64, budget: usize) -> (layout::Layout, f64, f64, usize) {
    let at = Instant::now();
    let mut job = layout::Job::new(engine, scope, cache, layout::Viewport { width, height: 600., scale: 1. }, Default::default(), None).unwrap();
    let ctor = at.elapsed().as_secs_f64() * 1e6;
    let mut maximum = 0f64;
    loop {
        let at = Instant::now();
        let p = job.step(budget).unwrap();
        maximum = maximum.max(at.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    let work = job.progress().work;
    (job.finish().unwrap(), ctor, maximum, work)
}
fn digest(l: &layout::Layout) -> String {
    let mut hash = Sha256::new();
    for (id, _) in l.cache().tree().iter() {
        let r = l.bounds(id).unwrap();
        for value in [r.x, r.y, r.width, r.height] {
            hash.update(value.to_le_bytes());
        }
        if let Some(text) = l.text_layout(id) {
            for line in text.lines() {
                hash.update(line.advance.to_le_bytes());
                hash.update(line.source.start.to_le_bytes());
                hash.update(line.source.end.to_le_bytes());
            }
        }
    }
    format!("{:x}", hash.finalize())
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "50".into()).parse().unwrap();
    assert!(rounds > 0);
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut engine = Engine::default();
    let mut painter = paint::Engine::default();
    let mut rows = Vec::new();
    for (count, words) in [(4, 2), (128, 2), (1024, 2), (4, 500)] {
        let unit = format!("<div>{}</div>", "AB CD ".repeat(words));
        let source: Arc<str> = Arc::from(format!("<!doctype html><style>body{{margin:0;display:flex;gap:3px;font-size:20px;line-height:20px}}body>div{{flex:1 1 auto}}</style>{}", unit.repeat(count)));
        let warm = page::load(source.clone(), &host, &scope, &mut engine, &mut painter, layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {}).unwrap();
        let mut prepare = layout::PrepareJob::new(&mut engine, &scope, warm.scene.layout().cache().tree().clone(), Default::default(), None).unwrap();
        while !prepare.step(1).unwrap().finished {}
        let original = prepare.finish().unwrap();
        assert_eq!(original.intrinsic_measurements(), 0);
        let (initial, _, _, _) = fit(&mut engine, &scope, original.clone(), 800., 1);
        assert_eq!(initial.cache().intrinsic_measurements(), 1, "identical retained Text must be measured only once");
        for (mode, cache) in [("measure", original.clone()), ("retained", initial.cache().clone())] {
            let (check, ctor, worst, work) = fit(&mut engine, &scope, cache.clone(), 733., 1);
            let hash = digest(&check);
            let payload = check.geometry_payload_bytes();
            let intrinsic = check.cache().intrinsic_payload_bytes();
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(fit(&mut engine, &scope, cache.clone(), 733., 128));
            }
            rows.push(json!({"items":count,"word_pairs_per_item":words,"mode":mode,"layout_us":start.elapsed().as_secs_f64()*1e6/rounds as f64,"ctor_us":ctor,"maximum_single_step_us":worst,"work":work,"geometry_payload_bytes":payload,"intrinsic_entry_payload_bytes":intrinsic,"geometry_sha256":hash,"source_sha256":format!("{:x}",Sha256::digest(source.as_bytes()))}));
        }
    }
    println!("{}", json!({"features":build_features::enabled(),"rounds":rounds,"job_bytes":std::mem::size_of::<layout::Job<'_>>(),"cache_bytes":std::mem::size_of::<layout::Cache>(),"rows":rows}));
}
