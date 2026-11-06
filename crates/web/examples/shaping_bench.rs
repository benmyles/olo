use olo_web::{
    css::{
        Syntax,
        properties::{self as p, Declaration},
        style::{Axes, FontRequest, FontSlant},
    },
    font::{Builder, FontSet, GenericPolicy, MatchOptions},
    text::shaping::{self as s, Analysis, BaseDirection, Direction, Engine, RunOptions, Script},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, path::Path, sync::Arc, time::Instant};
fn request(family: &str) -> FontRequest {
    let syntax = Syntax::parse(Arc::from(format!("'{family}'")), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let p::Value::Families(f) = &values[0].value else { panic!() };
    FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families: f.clone(), axes: Axes::default() }
}
fn digest(run: &s::Run) -> String {
    let mut h = Sha256::new();
    for g in &run.glyphs {
        h.update(g.id.to_le_bytes());
        h.update(g.cluster.to_le_bytes());
        for x in [g.x_advance, g.y_advance, g.x_offset, g.y_offset] {
            h.update(x.to_le_bytes());
        }
        h.update([u8::from(g.unsafe_to_break)]);
    }
    format!("{:x}", h.finalize())
}
fn build(engine: &mut Engine, scope: &olo_web::font::Scope, r: FontRequest, text: Arc<str>) -> (s::Paragraph, f64, f64) {
    let start = Instant::now();
    let mut job = s::ParagraphJob::new(engine, scope, r, text, BaseDirection::Auto, Default::default(), None).unwrap();
    let prepare_us = start.elapsed().as_secs_f64() * 1e6;
    let mut maximum = 0f64;
    loop {
        let start = Instant::now();
        let p = job.step(128).unwrap();
        maximum = maximum.max(start.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    (job.finish().unwrap(), prepare_us, maximum)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "50".into()).parse().unwrap();
    assert!(rounds > 0);
    let reversed = std::env::args().nth(2).as_deref() == Some("reverse");
    let mut modes = [false, true];
    if reversed {
        modes.reverse();
    }
    let refs: serde_json::Value = serde_json::from_str(include_str!("../tests/shaping-fixtures/harfbuzz.json")).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut b = Builder::new(Default::default());
    let mut seen = std::collections::HashSet::new();
    for c in refs["benchmarks"].as_array().unwrap() {
        let path = c["font"].as_str().unwrap();
        if seen.insert(path) {
            b.load_file(&repo.join(path), |_| {}, None).unwrap();
        }
    }
    let catalog = b.finish(None).unwrap();
    let mut cases = Vec::new();
    for case in refs["benchmarks"].as_array().unwrap() {
        let family = case["family"].as_str().unwrap();
        let r = request(family);
        let selected = catalog.select_family(family, &r, Default::default(), None).unwrap().unwrap();
        let scope = FontSet::new(catalog.clone(), GenericPolicy::new(&[family]).unwrap(), MatchOptions::default()).unwrap().scope(case["language"].as_str().unwrap()).unwrap();
        let text: Arc<str> = Arc::from(case["text"].as_str().unwrap());
        let options = RunOptions {
            direction: if case["direction"] == "rtl" { Direction::RightToLeft } else { Direction::LeftToRight },
            script: Script::from_iso15924_tag(harfrust::Tag::new(case["script"].as_str().unwrap().as_bytes().try_into().unwrap())).unwrap(),
            language: Some(case["language"].as_str().unwrap()),
            ..Default::default()
        };
        for cache in modes {
            let limits = if cache { s::Limits::default() } else { s::Limits { cached_faces: 0, cached_instances: 0, cached_plans: 0, ..Default::default() } };
            let mut engine = Engine::new(limits);
            let start = Instant::now();
            let run = engine.shape_run(&selected, text.clone(), 0..text.len(), options, None).unwrap();
            let first_run_us = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!(digest(&run), case["glyph_sha256"].as_str().unwrap());
            let glyphs = run.glyphs.len();
            let glyph_bytes = run.glyph_payload_bytes();
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(engine.shape_run(&selected, text.clone(), 0..text.len(), options, None).unwrap());
            }
            let run_shape_drop_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let mut engine = Engine::new(limits);
            let start = Instant::now();
            let (paragraph, first_prepare_us, first_step_us) = build(&mut engine, &scope, r.clone(), text.clone());
            let first_paragraph_us = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!(paragraph.runs.len(), 1);
            assert_eq!(digest(&paragraph.runs[0]), case["glyph_sha256"].as_str().unwrap());
            assert_eq!(paragraph.report, s::ParagraphReport::default());
            let start = Instant::now();
            let (mut prepare, mut maximum) = (0f64, 0f64);
            for _ in 0..rounds {
                let (p, prep, step) = build(&mut engine, &scope, r.clone(), text.clone());
                prepare = prepare.max(prep);
                maximum = maximum.max(step);
                black_box(p);
            }
            let paragraph_build_drop_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let stats = engine.cache_stats();
            cases.push(json!({"name":case["name"],"bytes":text.len(),"glyphs":glyphs,"glyph_payload_bytes":glyph_bytes,"cache":cache,"first_run_us":first_run_us,"run_shape_drop_us":run_shape_drop_us,"first_paragraph_us":first_paragraph_us,"first_prepare_us":first_prepare_us,"first_max_step_us":first_step_us,"paragraph_build_drop_us":paragraph_build_drop_us,"max_prepare_us":prepare,"max_step_us":maximum,"cached_faces":stats.faces,"cached_instances":stats.instances,"cached_plans":stats.plans,"cached_source_bytes":stats.source_bytes}));
        }
    }
    let mut text_ops = Vec::new();
    for (name, unit) in [("ascii", "office AV 123 ()! "), ("mixed", "abc שלום سلام हिन्दी ภาษาไทย "), ("emoji", "👩🏽\u{200d}💻 🇺🇸 \r\n क्ष ")] {
        let text: Arc<str> = Arc::from(unit.repeat(100));
        let start = Instant::now();
        let a = Analysis::new(text.clone(), BaseDirection::Auto, 64 * 1024, None).unwrap();
        let first_analysis_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(Analysis::new(text.clone(), BaseDirection::Auto, 64 * 1024, None).unwrap());
        }
        let analysis_drop_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        let start = Instant::now();
        let mut count = 0;
        for _ in 0..rounds {
            count = black_box(s::graphemes(black_box(&text)).count());
        }
        let grapheme_scan_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
        text_ops.push(json!({"name":name,"bytes":text.len(),"clusters":count,"paragraphs":a.paragraphs().count(),"first_analysis_us":first_analysis_us,"analysis_drop_us":analysis_drop_us,"grapheme_scan_us":grapheme_scan_us}));
    }
    println!(
        "{}",
        json!({"scope":"Native font-unit shaping and unwrapped uniform-font paragraph preparation, five scripts and three sizes. Independent native HarfBuzz 15 packed glyph hashes checked outside timings. Preloaded/shared fonts; no disk, CSS whitespace/line wrapping, styled inline spans, layout, rasterization, GPU, V8/network/UI or website/Chrome performance evidence.","method":"Mean full shape/build/drop; cold calls and maximum 128-grapheme worker slice/constructor separately. Same binary font/instance/plan caches on/off, alternating order. Glyph payload is dense font-unit records, source-byte counter is conservative per cached base face and excludes opaque parser/shaper caches/allocator overhead. Process RSS is measured separately.","rounds":rounds,"order_reversed":reversed,"baseline_text_ascii":cfg!(feature="baseline-text-ascii"),"glyph_record_bytes":size_of::<s::Glyph>(),"catalog_source_count":catalog.source_count(),"catalog_payload_bytes":catalog.reserved_payload_bytes(),"cases":cases,"text_ops":text_ops})
    );
}
