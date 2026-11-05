//! Static table construction and malformed-table text buffering. No layout.
use olo_web::{
    dom::NodeType,
    html::{ParseOptions, parse},
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(5);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for count in [10_000, 20_000, 40_000] {
        for (case, body) in [
            ("rows", "<tr><td>one<td>two".repeat(count)),
            ("fostered_rows", "x<tr><td>cell</tr>".repeat(count)),
            ("fragmented_text", "<".repeat(count)),
            ("decoded_fragmented_text", format!("&amp;{}", "<".repeat(count))),
        ] {
            let source: Arc<str> = Arc::from(format!("<!doctype html><table>{body}</table>"));
            let first = Instant::now();
            let parsed = parse(source.clone(), ParseOptions::default(), None).unwrap();
            let first_ms = first.elapsed().as_secs_f64() * 1000.0;
            let memory = parsed.document.memory();
            let html = parsed.document.children(parsed.document.root()).unwrap().find(|&n| parsed.document.node_type(n).unwrap() == NodeType::Element).unwrap();
            let container = parsed.document.children(html).unwrap().find(|&n| parsed.document.name(n).unwrap() == Some("body")).unwrap();
            if case != "rows" {
                let text = parsed.document.first_child(container).unwrap().unwrap();
                let value = parsed.document.value(text).unwrap().unwrap();
                assert_eq!(value.len(), if case == "fostered_rows" { count } else { count + usize::from(case == "decoded_fragmented_text") });
            }
            drop(parsed);
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(parse(source.clone(), ParseOptions::default(), None).unwrap());
            }
            results.push(json!({"case":case,"count":count,"rounds":rounds,"source_bytes":source.len(),"first_parse_ms":first_ms,"parse_and_drop_ms":start.elapsed().as_secs_f64()*1000.0/rounds as f64,"nodes":memory.live_nodes,"node_reserved_bytes":memory.node_reserved_bytes,"owned_text_capacity_bytes":memory.owned_text_bytes}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"Static parsing/storage only, no network, JS, layout, UI or Chrome comparison","results":results})).unwrap());
}
