//! Isolated immutable CSS state construction and lookup. No forms/UI/JS timing.
#[path = "support/features.rs"]
mod build_features;
use olo_web::{
    dom::{Document, Namespace, NodeId},
    html::{self, controls::DisabledStates},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

// Independent direct ancestor walk for these input-only fixtures, including
// the first direct legend exception. This is a cost control, not a complete
// HTML control/option model or the production implementation.
fn walk(doc: &Document, node: NodeId, cancel: &AtomicBool) -> bool {
    assert!(!cancel.load(Ordering::Relaxed));
    if doc.attribute(node, "disabled").unwrap().is_some() {
        return true;
    }
    let mut child = node;
    while let Some(parent) = doc.parent(child).unwrap() {
        if doc.namespace(parent).unwrap() == Some(Namespace::Html) && doc.name(parent).unwrap() == Some("fieldset") && doc.attribute(parent, "disabled").unwrap().is_some() {
            if doc.name(child).unwrap() != Some("legend") {
                return true;
            }
            let first = doc.children(parent).unwrap().find(|n| doc.namespace(*n).unwrap() == Some(Namespace::Html) && doc.name(*n).unwrap() == Some("legend"));
            if first != Some(child) {
                return true;
            }
        }
        child = parent;
    }
    false
}
fn main() {
    let rounds: usize = std::env::args().nth(1).unwrap_or_else(|| "20".into()).parse().unwrap();
    assert!(rounds > 0);
    let cancel = AtomicBool::new(false);
    let mut rows = Vec::new();
    for n in [100, 1000] {
        for late in [false, true] {
            let mut source = String::from("<!doctype html><fieldset disabled>");
            if late {
                for _ in 0..n {
                    source.push_str("<span></span>");
                }
            }
            source.push_str("<legend>");
            for i in 0..n {
                source.push_str(if i % 7 == 0 { "<input disabled>" } else { "<input>" });
            }
            source.push_str("</legend></fieldset>");
            let hash = format!("{:x}", Sha256::digest(source.as_bytes()));
            let doc = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
            let mut inputs = Vec::new();
            let mut stack = vec![doc.root()];
            while let Some(node) = stack.pop() {
                if doc.name(node).unwrap() == Some("input") {
                    inputs.push(node);
                }
                stack.extend(doc.children(node).unwrap());
            }
            let states = DisabledStates::new(&doc, Default::default(), Some(&cancel)).unwrap();
            let expected = inputs.iter().map(|node| walk(&doc, *node, &cancel) as u8).collect::<Vec<_>>();
            assert!(inputs.iter().zip(&expected).all(|(node, v)| states.disabled(&doc, *node).unwrap() == Some(*v != 0)));
            let at = Instant::now();
            for _ in 0..rounds {
                black_box(DisabledStates::new(&doc, Default::default(), Some(&cancel)).unwrap());
            }
            let build_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let at = Instant::now();
            for _ in 0..rounds {
                for node in &inputs {
                    black_box(states.disabled(&doc, *node).unwrap());
                }
            }
            let indexed_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            let at = Instant::now();
            for _ in 0..rounds {
                for node in &inputs {
                    black_box(walk(&doc, *node, &cancel));
                }
            }
            let walk_us = at.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            rows.push(json!({"elements":n,"late_legend":late,"source_sha256":hash,"result_sha256":format!("{:x}",Sha256::digest(&expected)),"build_us":build_us,"indexed_queries_us":indexed_us,"walk_queries_us":walk_us,"controls":states.control_count(),"payload_bytes":states.payload_bytes()}));
        }
    }
    println!(
        "{}",
        json!({"features":build_features::enabled(),"scope":"Original input-only fixtures: cold state index and warm full-set queries versus direct ancestor walking. Parse excluded. No form interaction, render, GPU or website speed claim.","rounds":rounds,"rows":rows})
    );
}
