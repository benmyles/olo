//! Synthetic adjacent-character-data benchmark. The baseline reconstructs
//! the full value on every append; the default calls the arena append API.
use olo_web::dom::{Document, Limits};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() {
    let mut samples = Vec::new();
    for count in [10_000, 20_000, 40_000] {
        for naive in [true, false] {
            let mut doc = Document::new(Arc::from(""), Limits::default()).unwrap();
            let node = doc.create_text("").unwrap();
            let start = Instant::now();
            for _ in 0..count {
                if naive {
                    let mut value = doc.value(node).unwrap().unwrap().to_owned();
                    value.push_str(black_box("🙂"));
                    doc.set_value(node, value).unwrap();
                } else {
                    doc.append_value(node, black_box("🙂")).unwrap();
                }
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(doc.value(node).unwrap().unwrap().len(), count * 4);
            samples.push(json!({"appends":count,"baseline_reconstruction":naive,"elapsed_ms":ms,"retained_text_capacity_bytes":doc.memory().owned_text_bytes}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"scope":"Synthetic character data only, no parser/UI/Chrome comparison","samples":samples})).unwrap());
}
