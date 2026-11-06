use olo_web::{
    css::{
        self, Syntax,
        cascade::{self, Defaulted, Origin, SheetBuilder},
        properties::{Property, Value},
        selectors::{self, Tree},
    },
    html,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn compile(s: &Syntax) -> cascade::RuleSet {
    let mut b = SheetBuilder::new(Default::default());
    b.add_sheet(s, Origin::Author, None).unwrap();
    b.finish(None).unwrap()
}
fn main() {
    let rounds = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(3);
    let mut cases = Vec::new();
    for n in [1000, 2000, 4000] {
        let mut source = String::new();
        let mut doc = String::from("<!doctype html><main class=app>");
        for i in 0..n {
            source.push_str(&format!(".app .item{i}{{width:{}px}}", i % 100 + 1));
            doc.push_str(&format!("<div class=item{i}></div>"));
        }
        doc.push_str("</main>");
        let syntax = Syntax::parse(Arc::from(source), css::Limits::default(), None).unwrap();
        let start = Instant::now();
        let rules = compile(&syntax);
        let first_compile_ms = start.elapsed().as_secs_f64() * 1e3;
        assert!(rules.issues().is_empty());
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(compile(&syntax));
        }
        let compile_drop_ms = start.elapsed().as_secs_f64() * 1e3 / rounds as f64;
        let parsed = html::parse(Arc::from(doc), Default::default(), None).unwrap();
        let tree = selectors::DomTree::new(&parsed.document);
        let mut elements = Vec::new();
        let mut pending = vec![parsed.document.root()];
        while let Some(node) = pending.pop() {
            if matches!(tree.content(node).unwrap(),selectors::Content::Element(name) if name.local_name=="div") {
                elements.push(node);
            }
            let mut child = tree.first_child(node).unwrap();
            while let Some(c) = child {
                pending.push(c);
                child = tree.next_sibling(c).unwrap();
            }
        }
        let mut scratch = cascade::MatchScratch::default();
        let mut selected = 0;
        let mut snapshot_bytes = 0;
        let start = Instant::now();
        let first = rules.match_element(&tree, elements[0], Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None).unwrap();
        black_box(first.resolver().property(Property::Width, true).unwrap());
        let first_match_us = start.elapsed().as_secs_f64() * 1e6;
        drop(first);
        // Verify every generated width independently of matcher/arena storage.
        // This pass is outside timings and deliberately warms rule/node data.
        for &node in &elements {
            let class = parsed.document.attribute(node, "class").unwrap().unwrap();
            let index: usize = class.strip_prefix("item").unwrap().parse().unwrap();
            let expected = (index % 100 + 1) as f64;
            let cascade = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None).unwrap();
            let Defaulted::Specified(Value::Numeric(value)) = cascade.resolver().property(Property::Width, true).unwrap() else { panic!() };
            assert_eq!(value.resolve(&css::math::NoContext, &mut Default::default(), Default::default(), None).unwrap(), expected);
        }
        let start = Instant::now();
        let mut sum = 0.;
        for _ in 0..rounds {
            for &node in &elements {
                let cascade = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None).unwrap();
                selected += scratch.candidate_count();
                snapshot_bytes = cascade.reserved_payload_bytes();
                let Defaulted::Specified(Value::Numeric(value)) = cascade.resolver().property(Property::Width, true).unwrap() else { panic!() };
                sum += value.resolve(&css::math::NoContext, &mut Default::default(), Default::default(), None).unwrap();
            }
        }
        let match_resolve_ms = start.elapsed().as_secs_f64() * 1e3 / rounds as f64;
        black_box(sum);
        let m = rules.memory();
        cases.push(json!({"rules":n,"elements":elements.len(),"source_bytes":syntax.source().len(),"compile_drop_ms":compile_drop_ms,"first_compile_ms":first_compile_ms,"match_resolve_ms":match_resolve_ms,"first_match_us":first_match_us,"selected_rules_per_round":selected/rounds,"checksum_per_round":sum/rounds as f64,"snapshot_payload_bytes":snapshot_bytes,"rule_array_bytes":m.rule_array_bytes,"declaration_array_bytes":m.declaration_array_bytes,"selector_reserved_bytes":m.selector_reserved_bytes,"index_entries":m.index_entries,"index_vector_bytes":m.index_vector_bytes,"index_key_value_capacity_bytes":m.index_key_value_capacity_bytes,"accounted_rule_payload_bytes":m.rule_array_bytes+m.declaration_array_bytes+m.selector_reserved_bytes+m.index_vector_bytes+m.index_key_value_capacity_bytes}));
    }
    println!(
        "{}",
        json!({"scope":"Prebuilt Syntax stylesheet compilation and native DOM selector/cascade/numeric-width resolution. No layout, rendering, JS, network, UI, browser or Chrome evidence.","method":"Mean compile/drop or all-element match/default/numeric resolve over rounds; source/HTML parsing excluded. RuleSet retained for resolution and matching scratch reused; every generated width independently checked in an untimed warm-up. Payload counts capacities, not total allocator/RSS/borrowed Syntax/DOM/index hash controls/custom maps/typed heaps. All benchmark values are independent scalar widths with no typed heap programs.","baseline_rule_scan":cfg!(feature="baseline-cascade-index"),"rounds":rounds,"cases":cases})
    );
}
