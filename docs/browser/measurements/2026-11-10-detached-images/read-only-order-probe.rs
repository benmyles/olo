//! Headless DOM ownership/startup stress. Use an owned XDG_CACHE_HOME for cold
//! snapshot runs; OLO_V8_FLAGS and OLO_NO_SNAPSHOT select V8 controls.
use olo_js::{Config, Realm};
use std::{sync::Arc, time::Instant};

fn main() {
    if std::env::args().any(|a| a == "snapshot-race") {
        snapshot_race();
        return;
    }
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

/// A snapshot creator initializes the default isolate group's read-only heap
/// first, and remains alive while an ordinary isolate runs. Synchronizing the
/// constructors makes the cold-start ordering reproducible without sleeps.
fn snapshot_race() {
    if let Ok(flags) = std::env::var("OLO_V8_FLAGS") {
        v8::V8::set_flags_from_string(&flags);
    }
    v8::V8::initialize_platform(v8::new_default_platform(0, false).make_shared());
    v8::V8::initialize();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut isolate = v8::Isolate::snapshot_creator(None, None);
        {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, Default::default());
            scope.set_default_context(context);
        }
        ready_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        assert!(isolate.create_blob(v8::FunctionCodeHandling::Keep).is_some());
    });
    ready_rx.recv().unwrap();
    {
        let mut isolate = v8::Isolate::new(Default::default());
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        done_tx.send(()).unwrap();
        thread.join().unwrap();
        let source = v8::String::new(scope, "const o={}; for(let i=0;i<50000;i++)o['dynamic-key-'+i]=i; Object.keys(o).length").unwrap();
        let value = v8::Script::compile(scope, source, None).unwrap().run(scope).unwrap();
        assert_eq!(value.integer_value(scope), Some(50000));
    }
    println!("snapshot race passed");
}
