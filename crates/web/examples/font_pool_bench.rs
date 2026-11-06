#[path = "../tests/support/style_host.rs"]
mod fixtures;
use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, RuleSet, SheetBuilder},
        properties::Property as P,
        style::{self, Defaults, FontCacheStats, StyleTree},
    },
    dom::Document,
    html,
};
use serde_json::json;
use std::{collections::HashSet, hint::black_box, sync::Arc, time::Instant};
fn build(document: &Document, rules: &RuleSet, defaults: &Arc<Defaults>, entries: usize) -> (StyleTree, f64, FontCacheStats) {
    let mut job = style::Job::new(document, rules, defaults.clone(), Default::default(), style::JobLimits { font_cache_entries: entries, ..Default::default() }, None).unwrap();
    let mut max = 0f64;
    loop {
        let start = Instant::now();
        let p = job.step(128, |_| fixtures::FixtureHost).unwrap();
        max = max.max(start.elapsed().as_secs_f64() * 1e6);
        if p.finished {
            break;
        }
    }
    let stats = job.font_cache_stats();
    (job.finish().unwrap(), max, stats)
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "10".into()).parse().unwrap();
    assert!(rounds > 0);
    let reversed = std::env::args().nth(2).as_deref() == Some("reverse");
    let mut entry_limits = [0, 4096];
    if reversed {
        entry_limits.reverse();
    }
    let mut cases = Vec::new();
    for n in [1000, 10000] {
        for scene in ["repeated", "inline-families", "unique"] {
            let mut source = String::from("<!doctype html><main>");
            for i in 0..n {
                match scene {
                    "repeated" => source.push_str(&format!("<i id=n{i} style='font-size:{}px'></i>", 20 + i % 100)),
                    "inline-families" => source.push_str(&format!("<i id=n{i} style='font-family:Face,serif;font-size:{}px'></i>", 20 + i % 100)),
                    _ => source.push_str(&format!("<i id=n{i} style='font-size:{}px'></i>", 20 + i)),
                }
            }
            source.push_str("</main>");
            let document = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
            let syntax = Syntax::parse(Arc::from("i{font-family:Face,serif;width:1ex}"), Default::default(), None).unwrap();
            let mut builder = SheetBuilder::new(Default::default());
            builder.add_sheet(&syntax, Origin::Author, None).unwrap();
            let rules = builder.finish(None).unwrap();
            assert!(rules.issues().is_empty());
            let defaults = Defaults::new(&fixtures::FixtureHost, Default::default(), None).unwrap();
            for entries in entry_limits {
                let start = Instant::now();
                let (tree, first_max_step_us, stats) = build(&document, &rules, &defaults, entries);
                let first_build_ms = start.elapsed().as_secs_f64() * 1e3;
                let mut fonts = HashSet::new();
                let mut count = 0;
                for (node, style) in tree.iter() {
                    if let Some(id) = document.attribute(node, "id").unwrap() {
                        let i: usize = id.strip_prefix('n').unwrap().parse().unwrap();
                        let size = 20 + if scene == "unique" { i } else { i % 100 };
                        assert_eq!(style.font().request.size, size as f64);
                        assert_eq!(style.value(P::Width).scalar(), Some(size as f64 * 0.5));
                        fonts.insert(Arc::as_ptr(style.font()));
                        count += 1;
                    }
                }
                assert_eq!(count, n);
                let memory = style::Memory::for_styles(tree.iter().map(|(_, s)| s));
                let tree_container_bytes = tree.storage_payload_bytes() - memory.styles_bytes;
                let start = Instant::now();
                let mut maximum = 0f64;
                for _ in 0..rounds {
                    let (tree, max, _) = build(&document, &rules, &defaults, entries);
                    maximum = maximum.max(max);
                    black_box(tree);
                }
                let build_drop_ms = start.elapsed().as_secs_f64() * 1e3 / rounds as f64;
                cases.push(json!({"elements":n,"scene":scene,"entry_limit":entries,"first_build_ms":first_build_ms,"first_max_step_us":first_max_step_us,"build_drop_ms":build_drop_ms,"max_step_us":maximum,"subject_font_objects":fonts.len(),"font_bytes":memory.fonts_bytes,"families_bytes":memory.families_bytes,"style_tree_payload_bytes":memory.accounted_reserved_bytes+tree_container_bytes,"pool_hits":stats.hits,"pool_misses":stats.misses,"pool_entries":stats.entries,"pool_slot_bytes":stats.reserved_bytes}));
            }
        }
    }
    println!(
        "{}",
        json!({"scope":"Synthetic original static CSS jobs with supplied fixed font metrics. Same binary, font_cache_entries=0/4096; repeated requests, equal independently parsed family lists and unique requests. No native font selection, shaping, layout, JS/network/UI or website evidence.","method":"Full build/drop means, first calls and maximum 128-node worker slice separately. Literal generated sizes/unit values checked outside timings. Pool slot payload is worker scratch, dropped at finish; Fonts/family allocations are counted once in retained style payload. Allocator/hash controls and borrowed DOM/rules excluded; not process RSS.","rounds":rounds,"order_reversed":reversed,"cases":cases})
    );
}
