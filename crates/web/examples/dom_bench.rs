//! Synthetic storage benchmark, not HTML tree construction or rendering.
use olo_web::dom::{Document, Limits, Namespace};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let owned = args.iter().any(|s| s == "--owned");
    let rows: usize = args.iter().find(|s| s.as_str() != "--owned").map(|s| s.parse().unwrap()).unwrap_or(50_000);
    let block = "story A synthetic paragraph with multilingual text: Καλημέρα 日本語 🙂.\n";
    let source: Arc<str> = Arc::from(block.repeat(rows));
    let begin = Instant::now();
    let mut doc = Document::new(source.clone(), Limits::default()).unwrap();
    let root = doc.create_element("main", Namespace::Html).unwrap();
    doc.append_child(doc.root(), root).unwrap();
    let mut paragraphs = Vec::with_capacity(rows);
    let mut first_text = None;
    for row in 0..rows {
        let p = doc.create_element("p", Namespace::Html).unwrap();
        doc.set_attribute(p, "class", &source[..5]).unwrap();
        doc.append_child(root, p).unwrap();
        let text = &source[row * block.len()..(row + 1) * block.len()];
        let text = if owned {
            let decoded = text.to_owned();
            doc.create_text(decoded).unwrap()
        } else {
            doc.create_text(text).unwrap()
        };
        doc.append_child(p, text).unwrap();
        paragraphs.push(p);
        first_text.get_or_insert(text);
    }
    let build_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let first = first_text.unwrap();
    let unchanged = doc.value(first).unwrap().unwrap().to_owned();
    let revision = doc.revision();
    let begin = Instant::now();
    for _ in 0..100_000 {
        black_box(doc.set_value(first, black_box(unchanged.as_str())).unwrap());
    }
    let unchanged_ns = begin.elapsed().as_secs_f64() * 1e9 / 100_000.0;
    assert_eq!(doc.revision(), revision);
    let begin = Instant::now();
    for p in paragraphs.iter().take(1000) {
        doc.append_child(root, *p).unwrap();
    }
    let reorder_ms = begin.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(doc.children(root).unwrap().count(), rows);
    let memory = doc.memory();
    let begin = Instant::now();
    let removed = doc.destroy_subtree(root).unwrap();
    let destroy_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let after = doc.memory();
    println!("{}",serde_json::to_string_pretty(&json!({"rows":rows,"owned_input":owned,"source_bytes":memory.source_bytes,"node_size":memory.node_size,"live_nodes":memory.live_nodes,"node_reserved_bytes":memory.node_reserved_bytes,"live_attributes":memory.live_attributes,"attribute_reserved_bytes":memory.attribute_reserved_bytes,"owned_text_bytes":memory.owned_text_bytes,"interned_name_bytes":memory.interned_name_bytes,"build_ms":build_ms,"unchanged_value_ns":unchanged_ns,"reorder_1000_ms":reorder_ms,"destroy_ms":destroy_ms,"destroyed_nodes":removed,"remaining_nodes":after.live_nodes,"remaining_attributes":after.live_attributes,"remaining_owned_text_bytes":after.owned_text_bytes,"scope":"Synthetic DOM storage only; no HTML tree construction, CSS, layout, JS, frames or Chrome comparison. Reserved pool/name/hash metadata and input/handle-vector allocations contribute to process RSS."})).unwrap());
}
