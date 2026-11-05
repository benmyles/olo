//! Static SVG/MathML construction and mixed-namespace token dispatch only.
use olo_web::html::{ParseOptions, parse};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(10);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for count in [10_000, 20_000, 40_000] {
        for (case, markup) in [
            ("svg_icons", "<svg viewBox='0 0 16 16'><path d='M0 0h16v16z'/><use xlink:href='#icon' xml:lang='en'/><foreignObject><p>Label</p></foreignObject></svg>".repeat(count)),
            ("math", "<math><mrow><mi>x</mi><mo>+</mo><mn>2</mn></mrow><annotation-xml encoding='text/html'><p>Label</p></annotation-xml></math>".repeat(count)),
            ("html_fragmented_text", "<".repeat(count)),
            ("html_after_svg_fragmented_text", format!("<svg/>{}", "<".repeat(count))),
            ("svg_cdata", format!("<svg><![CDATA[{}]]></svg>", "<".repeat(count))),
        ] {
            let source: Arc<str> = Arc::from(format!("<!doctype html>{markup}"));
            let first = Instant::now();
            let parsed = parse(source.clone(), ParseOptions::default(), None).unwrap();
            let first_ms = first.elapsed().as_secs_f64() * 1000.0;
            let memory = parsed.document.memory();
            assert_eq!(memory.owned_text_bytes, 0);
            drop(parsed);
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(parse(source.clone(), ParseOptions::default(), None).unwrap());
            }
            results.push(json!({"case":case,"count":count,"rounds":rounds,"source_bytes":source.len(),"first_parse_ms":first_ms,"parse_and_drop_ms":start.elapsed().as_secs_f64()*1000.0/rounds as f64,"nodes":memory.live_nodes,"attributes":memory.live_attributes,"node_size":memory.node_size,"node_reserved_bytes":memory.node_reserved_bytes,"attribute_reserved_bytes":memory.attribute_reserved_bytes,"owned_text_capacity_bytes":memory.owned_text_bytes}));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"scope":"Static tree construction/storage only, no SVG/MathML rendering, network, JS, layout, UI or Chrome comparison","results":results})).unwrap()
    );
}
