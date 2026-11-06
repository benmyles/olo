#[path = "../tests/support/native_font_host.rs"]
mod adapter;
use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        properties::{self as p, Declaration},
        style::{self, Axes, Defaults, FontRequest, FontSlant},
    },
    font::{Builder, FontSet, GenericPolicy, Limits, MatchOptions},
};
use serde_json::json;
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
fn request() -> FontRequest {
    let syntax = Syntax::parse(Arc::from("'Noto Sans',sans-serif"), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let p::Value::Families(f) = &values[0].value else { panic!() };
    FontRequest { size: 20., weight: 650.5, width: 100., slant: FontSlant::Normal, families: f.clone(), axes: Axes::default() }
}
fn build_catalog(sources: &[Arc<[u8]>], cache: bool) -> Arc<olo_web::font::Catalog> {
    let mut b = Builder::new(Limits { cache_entries: if cache { 4096 } else { 0 }, ..Default::default() });
    for source in sources {
        b.add_shared(source.clone(), None).unwrap();
    }
    b.finish(None).unwrap()
}
fn set(catalog: Arc<olo_web::font::Catalog>) -> Arc<FontSet> {
    FontSet::new(catalog, GenericPolicy::new(&["Noto Sans"]).unwrap(), MatchOptions::default()).unwrap()
}
fn build(document: &olo_web::dom::Document, rules: &olo_web::css::cascade::RuleSet, defaults: &Arc<Defaults>, host: &adapter::NativeHost) -> (style::StyleTree, f64) {
    let mut job = style::Job::new(document, rules, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    let mut maximum = 0f64;
    loop {
        let start = Instant::now();
        let p = job.step(128, |_| host).unwrap();
        maximum = maximum.max(start.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (job.finish().unwrap(), maximum)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "10".into()).parse().unwrap();
    assert!(rounds > 0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts");
    let names = ["NotoSans[wght].ttf", "NotoSans-Italic[wght].ttf", "NotoSansMono[wght].ttf", "NotoSansSymbols[wght].ttf", "NotoSansSymbols2-Regular.ttf", "NotoSansMath-Regular.ttf"];
    let sources: Vec<Arc<[u8]>> = names.iter().map(|name| std::fs::read(root.join(name)).unwrap().into()).collect();
    let source_bytes: usize = sources.iter().map(|s| s.len()).sum();
    let start = Instant::now();
    let catalog = build_catalog(&sources, true);
    let first_catalog_us = start.elapsed().as_secs_f64() * 1e6;
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(build_catalog(&sources, true));
    }
    let catalog_build_drop_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
    let r = request();
    let host = adapter::NativeHost(set(catalog.clone()).scope("en").unwrap());
    let uncached = adapter::NativeHost(set(build_catalog(&sources, false)).scope("en").unwrap());
    let iterations = rounds * 1000;
    let start = Instant::now();
    black_box(host.0.metrics(&r, None).unwrap());
    let first_metrics_us = start.elapsed().as_secs_f64() * 1e6;
    let mut metrics = Vec::new();
    for (label, h) in [("cache_off", &uncached), ("cache_on", &host)] {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(h.0.metrics(black_box(&r), None).unwrap());
        }
        let ns = start.elapsed().as_secs_f64() * 1e9 / iterations as f64;
        let m = h.0.metrics(&r, None).unwrap();
        let references: serde_json::Value = serde_json::from_str(include_str!("../tests/font-native-fixtures/raw-references.json")).unwrap();
        let case = &references["noto"][1];
        let expected = case["units"]["ch"].as_f64().unwrap() * r.size / case["upem"].as_f64().unwrap();
        assert!((m.style.ch - expected).abs() < 1e-5);
        metrics.push(json!({"label":label,"metrics_ns":ns,"ch":m.style.ch,"ex":m.style.ex,"cap":m.style.cap,"cache_entries":h.0.cache_stats().entries,"cache_conservative_bytes":h.0.cache_stats().conservative_bytes}));
    }
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(catalog.select_family(black_box("Noto Sans"), black_box(&r), Default::default(), None).unwrap().unwrap());
    }
    let select_family_ns = start.elapsed().as_secs_f64() * 1e9 / iterations as f64;
    let mut jobs = Vec::new();
    for n in [1000, 10000] {
        let mut source = String::from("<!doctype html><main>");
        for i in 0..n {
            source.push_str(&format!("<div id=i{i} style='font-weight:{}'></div>", 400 + i % 301));
        }
        source.push_str("</main>");
        let document = olo_web::html::parse(Arc::from(source), Default::default(), None).unwrap().document;
        let syntax = Syntax::parse(Arc::from("main{font-family:'Noto Sans';font-size:20px}div{width:1ch;height:1ex}"), Default::default(), None).unwrap();
        let mut b = SheetBuilder::new(Default::default());
        b.add_sheet(&syntax, Origin::Author, None).unwrap();
        let rules = b.finish(None).unwrap();
        assert!(rules.issues().is_empty());
        for (label, h) in [("cache_off", &uncached), ("cache_on", &host)] {
            let defaults = Defaults::new(h, Default::default(), None).unwrap();
            let start = Instant::now();
            let (tree, first_max_step_us) = build(&document, &rules, &defaults, h);
            let first_build_ms = start.elapsed().as_secs_f64() * 1e3;
            let memory = style::Memory::for_styles(tree.iter().map(|(_, s)| s));
            let mut count = 0;
            for (node, s) in tree.iter() {
                if document.attribute(node, "id").unwrap().is_some() {
                    assert!((s.value(p::Property::Width).scalar().unwrap() - s.font().metrics.ch).abs() < 1e-9);
                    assert!((s.value(p::Property::Height).scalar().unwrap() - s.font().metrics.ex).abs() < 1e-9);
                    count += 1;
                }
            }
            assert_eq!(count, n);
            let start = Instant::now();
            let mut maximum = 0f64;
            for _ in 0..rounds {
                let (tree, m) = build(&document, &rules, &defaults, h);
                maximum = maximum.max(m);
                black_box(tree);
            }
            let build_drop_ms = start.elapsed().as_secs_f64() * 1e3 / rounds as f64;
            jobs.push(json!({"label":label,"elements":n,"first_build_ms":first_build_ms,"first_max_step_us":first_max_step_us,"build_drop_ms":build_drop_ms,"max_step_us":maximum,"font_bytes":memory.fonts_bytes,"style_payload_bytes":memory.accounted_reserved_bytes,"cache_entries":h.0.cache_stats().entries,"cache_conservative_bytes":h.0.cache_stats().conservative_bytes}));
        }
    }
    println!(
        "{}",
        json!({"scope":"Native installed-file table loading/matching/metric lookup and original static CSS jobs. Six repository Noto sources, pre-read/shared source bytes. No disk I/O in timings, shaping/rasterization/layout/network/V8/UI or website claim.","method":"Means over repetitions; cold first calls; maximum observed 128-node worker slice. Same binary/cache contract with cache_entries=0 control. Source/catalog/cache payload counts exclude allocator controls/RSS; style job payload excludes borrowed source/catalog/cache. No Font interner yet.","rounds":rounds,"source_bytes":source_bytes,"catalog_payload_bytes":catalog.reserved_payload_bytes(),"faces":catalog.faces().count(),"first_catalog_us":first_catalog_us,"catalog_build_drop_us":catalog_build_drop_us,"first_metrics_us":first_metrics_us,"select_family_ns":select_family_ns,"metrics":metrics,"jobs":jobs})
    );
}
