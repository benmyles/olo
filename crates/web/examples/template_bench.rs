//! Static template parsing, owned content roots and EOF unwinding only.
use olo_web::html::{ParseOptions, parse};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn measure(case: &str, count: usize, markup: String, rounds: usize) -> serde_json::Value {
    let source: Arc<str> = Arc::from(format!("<!doctype html>{markup}"));
    let first = Instant::now();
    let parsed = parse(source.clone(), ParseOptions::default(), None).unwrap();
    let first_ms = first.elapsed().as_secs_f64() * 1000.0;
    let memory = parsed.document.memory();
    let mut pending = vec![parsed.document.root()];
    let mut templates = 0;
    while let Some(node) = pending.pop() {
        pending.extend(parsed.document.children(node).unwrap());
        if let Some(content) = parsed.document.template_contents(node).unwrap() {
            assert!(parsed.document.first_child(node).unwrap().is_none());
            assert_eq!(parsed.document.parent(content).unwrap(), None);
            templates += 1;
            pending.push(content);
        }
    }
    assert_eq!(templates, count);
    drop(parsed);
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(parse(source.clone(), ParseOptions::default(), None).unwrap());
    }
    json!({"case":case,"count":count,"rounds":rounds,"source_bytes":source.len(),"first_parse_ms":first_ms,"parse_and_drop_ms":start.elapsed().as_secs_f64()*1000.0/rounds as f64,"nodes":memory.live_nodes,"attributes":memory.live_attributes,"node_size":memory.node_size,"node_reserved_bytes":memory.node_reserved_bytes,"attribute_reserved_bytes":memory.attribute_reserved_bytes,"owned_text_capacity_bytes":memory.owned_text_bytes})
}
fn main() {
    let rounds: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(10);
    assert!(rounds > 0);
    let mut results = Vec::new();
    for count in [10_000, 20_000, 40_000] {
        for (case, unit) in [
            ("mixed_contents", "<template><p>Text</p><svg><use xlink:href='#icon'/></svg></template>"),
            ("implicit_table_contents", "<template><tr><td>one<td>two</tr></template>"),
            ("fostered_table_contents", "<template><table>before<tr><td>cell</td></tr>after</table></template>"),
        ] {
            results.push(measure(case, count, unit.repeat(count), rounds));
        }
    }
    for count in [32, 64, 128, 256] {
        results.push(measure("nested_eof", count, format!("{}text", "<template>".repeat(count)), rounds));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"scope":"Static template construction/storage only; no scripts, shadow DOM, content patching, layout, network, UI or Chrome comparison","results":results})
        )
        .unwrap()
    );
}
