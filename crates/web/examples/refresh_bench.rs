//! Measure the background rule handoff, including shared-cache eviction.
//! `refresh_bench [ROUNDS=40] [RULES=1000] [OTHER_PAGES=0]` emits JSON.
//! No GUI or network; the same Page API used by the browser's live worker.
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping::Engine as Shaper,
};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let rounds = args.first().copied().unwrap_or(40);
    let rules = args.get(1).copied().unwrap_or(1000);
    let other_pages = args.get(2).copied().unwrap_or(0);
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let mut page = page::load(Arc::from("<!doctype html><style id=s>body{margin:0}</style><div></div>"), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    let mut stack = vec![page.document.root()];
    let style = loop {
        let node = stack.pop().unwrap();
        if page.document.attribute(node, "id").ok().flatten() == Some("s") {
            break node;
        }
        stack.extend(page.document.children(node).unwrap());
    };
    let mut doc = page.document.clone();
    let text = doc.children(style).unwrap().next().unwrap();
    let css: String = (0..rules).map(|i| format!(".rule{i}{{height:{}px;color:rgb(1,2,3)}}", i % 500)).collect();
    let mut rows = Vec::new();
    for round in 0..rounds + 2 {
        doc.set_value(text, &format!("{css}/* round {round} */")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let wake: page::Wake = Arc::new(move || drop(tx.send(())));
        let start = Instant::now();
        let result = page.refresh(&doc, None, &mut |_, _, _| Ok(None), Some(&wake));
        let request_us = start.elapsed().as_secs_f64() * 1e6;
        assert_eq!(result.unwrap_err().to_string(), page::COMPILING_RULES);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let ready_us = start.elapsed().as_secs_f64() * 1e6;
        for i in 0..other_pages {
            page::load(Arc::from(format!("<!doctype html><style>body{{height:{}px}}/* other {round} {i} */</style>", i + 100)), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {})
                .unwrap();
        }
        let apply = Instant::now();
        let mut retries = 0;
        loop {
            match page.refresh(&doc, None, &mut |_, _, _| Ok(None), Some(&wake)) {
                Ok(changed) => {
                    assert!(changed);
                    break;
                }
                Err(e) => {
                    assert_eq!(e.to_string(), page::COMPILING_RULES);
                    retries += 1;
                    assert!(retries < 10);
                    rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }
            }
        }
        if round >= 2 {
            rows.push(serde_json::json!({"request_us":request_us,"ready_us":ready_us,"apply_us":apply.elapsed().as_secs_f64()*1e6,"retries":retries}));
        }
    }
    println!("{}", serde_json::json!({"rules":rules,"other_pages":other_pages,"samples":rows}));
}
