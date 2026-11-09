//! Make realms one after another and time them: what a page's scripting
//! costs to set up (its prelude), for measuring and profiling it.
//!
//!   cargo run --release -p olo-js --example realms -- [COUNT]
use olo_js::{Config, Realm};
use olo_web::html;
use std::{sync::Arc, time::Instant};
fn main() {
    let count: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(50);
    let mut times = Vec::with_capacity(count);
    for _ in 0..count {
        let parsed =
            html::parse(Arc::from("<!DOCTYPE html><html><head><title>t</title></head><body><p>x</p></body></html>"), html::ParseOptions { scripting: true, ..Default::default() }, None).unwrap();
        let started = Instant::now();
        let realm = Realm::new(parsed.document, Config { url: "https://example.test/".into(), ..Default::default() }, None, None).unwrap();
        times.push(started.elapsed().as_secs_f64() * 1000.);
        drop(realm);
    }
    times.sort_by(f64::total_cmp);
    println!("{count} realms: median {:.1} ms, min {:.1} ms, max {:.1} ms", times[count / 2], times[0], times[count - 1]);
}
