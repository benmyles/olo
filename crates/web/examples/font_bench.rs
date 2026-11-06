#[path = "../tests/support/style_host.rs"]
mod fixtures;
use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        properties::{Declaration, Property as P},
        style::{self, Defaults, FontSlant, StyleTree},
    },
    dom::Document,
    html,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn build(document: &Document, rules: &olo_web::css::cascade::RuleSet, defaults: &Arc<Defaults>) -> (StyleTree, f64) {
    let mut job = style::Job::new(document, rules, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    let mut maximum = 0f64;
    loop {
        let start = Instant::now();
        let progress = job.step(128, |_| fixtures::FixtureHost).unwrap();
        maximum = maximum.max(start.elapsed().as_secs_f64() * 1e6);
        if progress.finished {
            break;
        }
    }
    (job.finish().unwrap(), maximum)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "10".into()).parse().unwrap();
    assert!(rounds > 0);
    let mut parsing = Vec::new();
    for (property, value) in [
        ("font-family", r#"New Century Schoolbook, "serif", monospace, generic(kai)"#),
        ("font-family", "a b c d e f, one, two, three, four, five, serif"),
        ("font-style", "oblique 10grad"),
        ("font-style", "oblique calc(sign(1ex - 10px) * 10deg)"),
        ("font-width", "calc(100% + sign(1ex - 10px) * 5%)"),
        ("font-stretch", "semi-condensed"),
    ] {
        let syntax = Syntax::parse(Arc::from(value), Default::default(), None).unwrap();
        let compile = || Declaration::parse(property, syntax.components(), false, Default::default(), None).unwrap();
        let start = Instant::now();
        black_box(compile());
        let first_compile_us = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        for _ in 0..rounds * 1000 {
            black_box(compile());
        }
        let compile_drop_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 1000) as f64;
        parsing.push(json!({"property":property,"value":value,"first_compile_us":first_compile_us,"compile_drop_us":compile_drop_us}));
    }
    let mut cases = Vec::new();
    for n in [1000, 10000] {
        let mut html = String::from("<!doctype html><main>");
        for i in 0..n {
            html.push_str(&format!("<div id=item{i} class=item style='font-width:{}%;font-style:oblique {}deg'></div>", 50 + i % 100, i % 30));
        }
        html.push_str("</main>");
        let document = html::parse(Arc::from(html), Default::default(), None).unwrap().document;
        let syntax = Syntax::parse(Arc::from(".item{font-family:Named Face,serif;font-size:20px;width:2ex}"), Default::default(), None).unwrap();
        let mut builder = SheetBuilder::new(Default::default());
        builder.add_sheet(&syntax, Origin::Author, None).unwrap();
        let rules = builder.finish(None).unwrap();
        assert!(rules.issues().is_empty());
        let defaults = Defaults::new(&fixtures::FixtureHost, Default::default(), None).unwrap();
        let start = Instant::now();
        let (tree, first_max_step_us) = build(&document, &rules, &defaults);
        let first_build_ms = start.elapsed().as_secs_f64() * 1e3;
        // Independently generated expectations, outside timed regions. The
        // supplied metrics are fixed test data, not real selected face metrics.
        let mut count = 0;
        for (node, style) in tree.iter() {
            if let Some(id) = document.attribute(node, "id").unwrap() {
                let i: usize = id.strip_prefix("item").unwrap().parse().unwrap();
                count += 1;
                assert_eq!(style.font().request.size, 20.);
                assert_eq!(style.font().request.width, (50 + i % 100) as f64);
                assert_eq!(style.font().request.slant, if i.is_multiple_of(30) { FontSlant::Normal } else { FontSlant::Oblique((i % 30) as f64) });
                assert_eq!(style.value(P::Width).scalar(), Some(20.));
            }
        }
        assert_eq!(count, n);
        assert_eq!(tree.report().inline_invalid, 0);
        assert_eq!(tree.report().inline_unsupported, 0);
        let memory = style::Memory::for_styles(tree.iter().map(|(_, s)| s));
        let start = Instant::now();
        let mut max_step_us = 0f64;
        for _ in 0..rounds {
            let (tree, maximum) = build(&document, &rules, &defaults);
            max_step_us = max_step_us.max(maximum);
            black_box(tree);
        }
        cases.push(json!({"elements":n,"first_build_ms":first_build_ms,"first_max_step_us":first_max_step_us,"max_step_us":max_step_us,"build_drop_ms":start.elapsed().as_secs_f64()*1e3/rounds as f64,"accounted_style_tree_bytes":memory.accounted_reserved_bytes+tree.storage_payload_bytes()-memory.styles_bytes,"font_bytes":memory.fonts_bytes,"families_bytes":memory.families_bytes}));
    }
    println!(
        "{}",
        json!({"scope":"Original font longhand compilation and initial style document jobs with generated varying face requests. Supplied fixed numeric metrics, no font loading/matching/shaping, UA sheet, CSSOM, layout/rasterization, network/JS/UI or website/Chrome performance evidence.","method":"Release compile/drop and full build/drop means, first calls and maximum observed 128-node steps separate. Referenced payload deduplicates shared arrays/fonts and includes Arc controls and tree capacities, excludes allocator/hash controls and borrowed DOM/rules; not process RSS. All generated scalars checked outside timings.","rounds":rounds,"parsing":parsing,"cases":cases})
    );
}
