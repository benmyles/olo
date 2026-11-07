#[path = "../tests/support/style_host.rs"]
mod fixtures;
use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        properties::{Keyword, Property as P},
        style::{self, Defaults, StyleTree, Value},
    },
    dom::{Document, NodeId, NodeType},
    html,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn build(document: &Document, rules: &olo_web::css::cascade::RuleSet, defaults: &Arc<Defaults>) -> (StyleTree, f64) {
    let mut job = style::Job::new(document, rules, defaults.clone(), style::JobOptions { generation: 1, ..Default::default() }, Default::default(), None).unwrap();
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
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "3".into()).parse().unwrap();
    let sizes: Vec<usize> = std::env::args().nth(2).map_or_else(|| vec![1000, 10000], |s| vec![s.parse().unwrap()]);
    let selected = std::env::args().nth(3);
    let mut cases = Vec::new();
    for n in sizes {
        for scene in ["bare", "uniform", "varying", "inset"] {
            if selected.as_ref().is_some_and(|s| s != scene) {
                continue;
            }
            let mut source = String::from("<!doctype html><main class=host>");
            for i in 0..n {
                if scene == "varying" {
                    source.push_str(&format!("<div class=item id=item{i} style='width:{}px'></div>", i % 100 + 1));
                } else {
                    source.push_str(&format!("<div class=item id=item{i}></div>"));
                }
            }
            source.push_str("</main>");
            let document = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
            let css = match scene {
                "bare" => "",
                "uniform" => ".host{font-size:20px}.item{width:40px;padding:2px;color:red}",
                "inset" => ".host{font-size:20px}.item{width:40px;padding:2px;color:red;clip-path:inset(calc(10% + 2em) round calc(20% + 1em))}",
                _ => ".host{font-size:20px}.item{width:40px;padding:2em;opacity:0.5}",
            };
            let syntax = Syntax::parse(Arc::from(css), Default::default(), None).unwrap();
            let mut builder = SheetBuilder::new(Default::default());
            builder.add_sheet(&syntax, Origin::Author, None).unwrap();
            let rules = builder.finish(None).unwrap();
            assert!(rules.issues().is_empty());
            let start = Instant::now();
            let defaults = Defaults::new(&fixtures::FixtureHost, Default::default(), None).unwrap();
            let defaults_us = start.elapsed().as_secs_f64() * 1e6;
            let start = Instant::now();
            let (tree, first_max_step_us) = build(&document, &rules, &defaults);
            let first_build_ms = start.elapsed().as_secs_f64() * 1e3;
            let targets: Vec<NodeId> =
                tree.iter().filter(|(node, _)| document.node_type(*node).unwrap() == NodeType::Element && document.attribute(*node, "class").unwrap() == Some("item")).map(|(node, _)| node).collect();
            assert_eq!(targets.len(), n);
            // Independent generated scalars, checked outside all timings.
            for &node in &targets {
                let style = tree.get(&document, node, 1).unwrap().unwrap();
                let index: usize = document.attribute(node, "id").unwrap().unwrap().strip_prefix("item").unwrap().parse().unwrap();
                assert_eq!(style.value(P::FontSize).scalar().unwrap(), if scene == "bare" { 16. } else { 20. });
                if scene == "bare" {
                    assert!(matches!(style.value(P::Width), Value::Keyword(Keyword::Auto)));
                } else {
                    assert_eq!(style.value(P::Width).scalar().unwrap(), if matches!(scene, "uniform" | "inset") { 40. } else { (index % 100 + 1) as f64 });
                }
                if scene == "inset" {
                    struct Basis;
                    impl olo_web::css::math::Context for Basis {
                        fn factor(&self, unit: olo_web::css::math::RelativeUnit) -> olo_web::css::math::Result<f64> {
                            if unit == olo_web::css::math::RelativeUnit::Percentage { Ok(2.) } else { Err(olo_web::css::math::Error::ContextUnavailable) }
                        }
                    }
                    let Value::Inset(shape) = style.value(P::ClipPath) else { panic!("missing inset") };
                    for n in shape.offsets() {
                        assert_eq!(n.resolve(&Basis, &mut Default::default(), Default::default(), None).unwrap(), 60.);
                    }
                    for n in shape.radii().unwrap() {
                        assert_eq!(n.resolve(&Basis, &mut Default::default(), Default::default(), None).unwrap(), 60.);
                    }
                }
                assert_eq!(
                    style.value(P::PaddingTop).scalar().unwrap(),
                    if scene == "bare" {
                        0.
                    } else if matches!(scene, "uniform" | "inset") {
                        2.
                    } else {
                        40.
                    }
                );
            }
            let memory = style::Memory::for_styles(tree.iter().map(|(_, s)| s));
            let container_bytes = tree.storage_payload_bytes() - memory.styles_bytes;
            let start = Instant::now();
            let mut max_step_us = 0f64;
            for _ in 0..rounds {
                let (value, maximum) = build(&document, &rules, &defaults);
                max_step_us = max_step_us.max(maximum);
                black_box(value);
            }
            let build_drop_ms = start.elapsed().as_secs_f64() * 1e3 / rounds as f64;
            let start = Instant::now();
            for _ in 0..rounds * 100 {
                for &node in &targets {
                    let style = tree.get(black_box(&document), black_box(node), 1).unwrap().unwrap();
                    black_box(style.value(black_box(P::Width)));
                    black_box(style.value(black_box(P::PaddingTop)));
                    black_box(style.value(black_box(P::FontSize)));
                }
            }
            let three_queries_ns = start.elapsed().as_secs_f64() * 1e9 / (rounds * 100 * n) as f64;
            cases.push(json!({"scene":scene,"elements":n,"styled_nodes":memory.styles,"defaults_us":defaults_us,"first_build_ms":first_build_ms,"first_max_step_us":first_max_step_us,"build_drop_ms":build_drop_ms,"max_step_us":max_step_us,"three_queries_ns":three_queries_ns,"style_headers_bytes":memory.styles_bytes,"shared_defaults_bytes":memory.defaults_bytes,"override_bytes":memory.overrides_bytes,"font_bytes":memory.fonts_bytes,"families_bytes":memory.families_bytes,"custom_bytes":memory.customs_bytes,"numeric_bytes":memory.numeric_bytes,"clip_shape_bytes":memory.clip_shape_bytes,"accounted_style_payload_bytes":memory.accounted_reserved_bytes,"tree_container_bytes":container_bytes,"accounted_style_tree_bytes":memory.accounted_reserved_bytes+container_bytes}));
        }
    }
    println!(
        "{}",
        json!({"scope":"Original document style jobs from prebuilt HTML/RuleSet/Defaults; inline declaration parsing included. Supplied fixed numeric font/palette context, no font selection/shaping, UA sheet, layout, rasterization, JS/network/UI/website/Chrome evidence.","method":"Mean full build/drop, retained-style three-property lookup mean, cold first calls and maximum observed 128-node worker step. Literal generated scalars verified outside timings. Shared allocations counted once; Arc controls included; allocator/hash controls and borrowed DOM/rules excluded. Tree container capacities included. Not process RSS.","rounds":rounds,"baseline_style_dense":cfg!(feature="baseline-style-dense"),"cases":cases})
    );
}
