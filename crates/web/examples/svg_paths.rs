//! Inspect real retained HTML paths using our DOM and geometry worker. This
//! measures geometry compilation only; it neither lays out nor renders a page.
use olo_web::{dom::Namespace, html, svg};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{hint::black_box, sync::Arc, time::Instant};
fn main() {
    let filename = std::env::args().nth(1).expect("HTML file");
    let source = std::fs::read_to_string(filename).unwrap();
    let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    let doc = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
    let mut stack = vec![doc.root()];
    let mut paths = Vec::new();
    while let Some(node) = stack.pop() {
        if doc.namespace(node).unwrap() == Some(Namespace::Svg)
            && doc.name(node).unwrap() == Some("path")
            && let Some(d) = doc.attribute(node, "d").unwrap()
        {
            paths.push((node, d));
        }
        let mut child = doc.first_child(node).unwrap();
        while let Some(c) = child {
            stack.push(c);
            child = doc.next_sibling(c).unwrap();
        }
    }
    let mut rows = Vec::new();
    for (node, d) in &paths {
        let mut job = svg::PathJob::new(d, Default::default(), None).unwrap();
        let work = loop {
            let p = job.step(1).unwrap();
            if p.finished {
                break p.segments;
            }
        };
        let result = job.finish().unwrap();
        rows.push(json!({"slot":node.slot(),"source_bytes":d.len(),"work":work,"payload_bytes":result.payload_bytes(),"malformed":result.malformed(),"drawable":result.geometry().is_some()}));
    }
    let started = Instant::now();
    let rounds = 100;
    for _ in 0..rounds {
        for (_, d) in &paths {
            let mut job = svg::PathJob::new(d, Default::default(), None).unwrap();
            while !job.step(128).unwrap().finished {}
            black_box(job.finish().unwrap());
        }
    }
    println!(
        "{}",
        json!({"scope":"Retained HTML geometry-only diagnostic. Excludes CSS, layout, raster, network, JS; no website acceptance. Arc tolerance 0.001 user units is not scale-independent fidelity.","source_sha256":hash,"paths":rows,"rounds":rounds,"all_paths_us":started.elapsed().as_secs_f64()*1e6/rounds as f64})
    );
}
