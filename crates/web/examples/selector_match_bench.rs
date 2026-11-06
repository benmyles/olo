use olo_web::{
    css::{
        self, Syntax,
        selectors::{DomTree, Selectors},
    },
    html,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn compiled(s: &str) -> Selectors {
    let s = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
    Selectors::compile(s.components(), Default::default(), Default::default(), None).unwrap()
}
fn main() {
    let iterations = std::env::args().nth(1).map(|s| s.parse::<usize>().unwrap()).unwrap_or(200000);
    let parsed = html::parse(Arc::from("<!doctype html><main><p id=target class=a>x</p> <p>y</p></main>"), Default::default(), None).unwrap();
    let tree = DomTree::new(&parsed.document);
    let mut target = None;
    compiled("#target")
        .query(&tree, parsed.document.root(), false, Default::default(), Default::default(), None, |n, _| {
            target = Some(n);
            Ok(())
        })
        .unwrap();
    let target = target.unwrap();
    let mut cases = Vec::new();
    for (source, expected) in [("#target", true), (".a", true), ("main p.a", true), (":is(.a,#absent)", true), (":nth-child(2n+1)", true), ("p + p", false), ("main:has(> p.a) p", true)] {
        let selector = compiled(source);
        let start = Instant::now();
        let first = selector.matches(&tree, target, Default::default(), Default::default(), None).unwrap();
        let first_us = start.elapsed().as_secs_f64() * 1e6;
        assert_eq!(first.is_some(), expected);
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(selector.matches(&tree, target, Default::default(), Default::default(), None).unwrap());
        }
        let match_ns = start.elapsed().as_secs_f64() * 1e9 / iterations as f64;
        cases.push(json!({"source":source,"matched":expected,"first_match_us":first_us,"match_ns":match_ns}));
    }
    println!(
        "{}",
        json!({"scope":"Unchanged public selector matches API over a fixed inert DOM. No styling, layout, rendering, JS, UI or browser evidence.","method":"Each independently expected match checked before timed loop, mean over iterations; parser/DOM construction excluded. First-call sample recorded separately.","iterations":iterations,"cases":cases})
    );
}
