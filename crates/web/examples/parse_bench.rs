//! Complete-buffer static tree construction only, never a page-load claim.
use olo_web::html::{ParseOptions, parse};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(3);
    assert!(rounds > 0);
    let attributes = (0..4096).map(|n| format!("data-{n}='value' ")).collect::<String>();
    let cases = [
        (
            "article",
            format!(
                "<!doctype html><title>Article</title><main>{}</main>",
                "<section class=article><h2>Headline</h2><p>Text with <a href='/story'>a link</a> and more text.</p></section>".repeat(8192)
            ),
        ),
        ("fragmented_text", format!("<!doctype html>{}", "<".repeat(100_000))),
        ("attributes", format!("<!doctype html><div {attributes}>")),
        ("formatting_attributes", format!("<!doctype html>{}", format!("<b {attributes}>").repeat(4))),
    ];
    let mut samples = Vec::new();
    for (case, input) in cases {
        let input: Arc<str> = Arc::from(input);
        let first = Instant::now();
        let doc = parse(input.clone(), ParseOptions::default(), None).unwrap();
        let first_ms = first.elapsed().as_secs_f64() * 1000.0;
        let memory = doc.document.memory();
        drop(doc);
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(parse(input.clone(), ParseOptions::default(), None).unwrap());
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        samples.push(json!({"case":case,"rounds":rounds,"source_bytes":input.len(),"nodes":memory.live_nodes,"attributes":memory.live_attributes,"owned_text_capacity_bytes":memory.owned_text_bytes,"node_reserved_bytes":memory.node_reserved_bytes,"attribute_reserved_bytes":memory.attribute_reserved_bytes,"first_parse_ms":first_ms,"parse_and_drop_ms":ms}));
    }
    println!("{}",serde_json::to_string_pretty(&json!({"baseline_parser":cfg!(feature="baseline-parser"),"scope":"Static complete-buffer tree construction only; no network, JS execution, layout, frames, UI or Chrome comparison","samples":samples})).unwrap());
}
