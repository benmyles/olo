//! Static CSS syntax and generic block construction, no style/layout.
use olo_web::css::{Limits, Syntax, Tokenizer};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(5);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for (case, input, mixed) in [
        ("stylesheet", ".card > a:hover { color: #abc; margin: 12.5px 0; background: url(\"image.png\"); }".repeat(8192), false),
        ("literals", format!("/*{}*/a {{ content: \"{}\"; }}", "x".repeat(1024 * 1024), "ab🙂".repeat(180000)), false),
        ("decoded", format!("a {{ content: \"{}\" }}", "ab\\61".repeat(100000)), false),
        ("declaration_values", format!("--values:{} !important;", "1px ".repeat(100000)), true),
        ("mixed_2000", "a:hover {color:red}".repeat(2000), true),
        ("mixed_4000", "a:hover {color:red}".repeat(4000), true),
        ("mixed_8000", "a:hover {color:red}".repeat(8000), true),
    ] {
        let source: Arc<str> = Arc::from(input);
        let first = Instant::now();
        let tree = Syntax::parse(source.clone(), Limits::default(), None).unwrap();
        let items = if mixed { tree.block_contents(None).unwrap() } else { tree.stylesheet(None).unwrap() };
        let first_ms = first.elapsed().as_secs_f64() * 1000.;
        let count = items.len();
        let memory = tree.memory();
        drop(items);
        drop(tree);
        let start = Instant::now();
        for _ in 0..rounds {
            let tree = Syntax::parse(source.clone(), Limits::default(), None).unwrap();
            let items = if mixed { tree.block_contents(None).unwrap() } else { tree.stylesheet(None).unwrap() };
            black_box((tree.memory(), items.len()));
        }
        let parse_ms = start.elapsed().as_secs_f64() * 1000. / rounds as f64;
        let start = Instant::now();
        let mut tokens = 0;
        for _ in 0..rounds {
            let mut tokenizer = Tokenizer::new(&source, Limits::default(), None).unwrap();
            while let Some(token) = tokenizer.next_token().unwrap() {
                black_box(token);
                tokens += 1;
            }
        }
        results.push(json!({"case":case,"source_bytes":source.len(),"rounds":rounds,"items":count,"tokens":tokens/rounds,"first_parse_ms":first_ms,"parse_and_drop_ms":parse_ms,"tokenize_ms":start.elapsed().as_secs_f64()*1000./rounds as f64,"nodes":memory.nodes,"node_size":memory.node_size,"node_reserved_bytes":memory.node_reserved_bytes,"owned_text_bytes":memory.decoded_bytes,"decoded_slot_reserved_bytes":memory.decoded_slot_reserved_bytes,"dimension_reserved_bytes":memory.dimension_reserved_bytes}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"scope":"Static CSS syntax only; no selectors, property grammar, style, layout, network, UI or Chrome comparison","scalar_scans":cfg!(feature="scalar-scans"),"baseline_css_parser":cfg!(feature="baseline-css-parser"),"results":results})
        )
        .unwrap()
    );
}
