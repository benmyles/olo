//! Selector compilation and static DOM queries only; no cascade/layout.
use olo_web::{
    css::{
        self, Syntax,
        selectors::{DomTree, MatchLimits, Selectors},
    },
    html,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn main() {
    let rounds = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(5);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for count in [1000, 2000, 4000] {
        let source = format!("<!doctype html><ul>{}</ul>", "<li class='item selected' data-x='AbAbAbAbC'><a href='x'>text</a></li>".repeat(count));
        let doc = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
        for selector in ["li.item > a", "li:nth-child(2n+1)", "li:nth-of-type(2n+1)", "li:nth-last-child(2n of .selected)", "li:has(> a)", "[data-x*='aBABc' i]"] {
            let source = Syntax::parse(Arc::from(selector), css::Limits::default(), None).unwrap();
            let compiled = Selectors::compile(source.components(), Default::default(), Default::default(), None).unwrap();
            let memory = compiled.memory();
            let first = Instant::now();
            let mut found = 0;
            let limits = MatchLimits { work: usize::MAX, results: 100000, ..Default::default() };
            let p = compiled
                .query(&DomTree::new(&doc), doc.root(), false, Default::default(), limits, None, |n, _| {
                    black_box(n);
                    found += 1;
                    Ok(())
                })
                .unwrap();
            let first_ms = first.elapsed().as_secs_f64() * 1000.;
            let start = Instant::now();
            for _ in 0..rounds {
                compiled
                    .query(&DomTree::new(&doc), doc.root(), false, Default::default(), limits, None, |n, _| {
                        black_box(n);
                        Ok(())
                    })
                    .unwrap();
            }
            let ms = start.elapsed().as_secs_f64() * 1000. / rounds as f64;
            results.push(json!({"count":count,"selector":selector,"first_ms":first_ms,"query_ms":ms,"matched":found,"work":p.work,"peak_frames":p.peak_backtracking_frames,"index_entries":p.index_entries,"index_capacity":p.index_capacity,"peak_pending_indices":p.peak_pending_indices,"tests":memory.tests,"test_size":memory.test_size,"accounted_reserved_bytes":memory.accounted_reserved_bytes}));
        }
    }
    let source = Syntax::parse(Arc::from(".item[data-x*=\"ABabc\" i] > a:is(:not(.off),#a),section:has(> .child),:nth-child(2n+1 of .item)"), css::Limits::default(), None).unwrap();
    let start = Instant::now();
    for _ in 0..rounds * 1000 {
        black_box(Selectors::compile(source.components(), Default::default(), Default::default(), None).unwrap());
    }
    let compile_us = start.elapsed().as_secs_f64() * 1e6 / (rounds * 1000) as f64;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"scope":"Compiled selectors and static DOM queries; no style, layout, UI, network, JS or Chrome comparison","rounds":rounds,"baseline_selector_nth":cfg!(feature="baseline-selector-nth"),"compile_us":compile_us,"results":results})
        )
        .unwrap()
    );
}
