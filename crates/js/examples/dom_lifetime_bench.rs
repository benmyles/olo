//! Headless DOM ownership/startup stress. Use an owned XDG_CACHE_HOME for cold
//! snapshot runs; OLO_V8_FLAGS and OLO_NO_SNAPSHOT select V8 controls.
use olo_js::{Config, Realm};
use std::{sync::Arc, time::Instant};

fn main() {
    let mut rows = Vec::new();
    for _ in 0..3 {
        let document = olo_web::html::parse(Arc::from("<body>"), Default::default(), None).unwrap().document;
        let started = Instant::now();
        let mut realm = Realm::new(document, Config::default(), None, None).unwrap();
        let setup_ms = started.elapsed().as_secs_f64() * 1000.;
        realm.run(include_str!("../tests/dom-churn.js"), "dom-churn.js").unwrap();
        let phases: serde_json::Value = serde_json::from_str(&realm.eval("benchResult").unwrap()).unwrap();
        let (heap, capacity, images, _) = realm.memory();
        let wrappers = realm.wrapper_counts();
        rows.push(serde_json::json!({"setup_ms":setup_ms,"phases_ms":phases,"heap_bytes":heap,"heap_capacity":capacity,"image_bytes":images,"wrappers":wrappers}));
        assert!(realm.report().errors.is_empty());
    }
    println!("{}", serde_json::json!({"realms":rows}));
}
