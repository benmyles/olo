//! Background CSS burst: `compile_queue_bench [PAGES=64] [RULES=10000] [KEEP=4]`.
//! Pages share preparsed CSS, but each URL needs its own compiled rule set.
//! All but KEEP pages are dropped immediately after requesting compilation.
//! The /proc sampler is benchmark-only; production has no polling.
#[path = "../tests/support/native_font_host.rs"]
mod native_host;
use olo_web::{
    css,
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping::Engine as Shaper,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn workers() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().filter_map(|p| p.ok()).filter(|p| std::fs::read_to_string(p.path().join("comm")).is_ok_and(|s| s.starts_with("olo-style-comp"))).count()
}
fn main() {
    let args: Vec<usize> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let count = args.first().copied().unwrap_or(64);
    let rules = args.get(1).copied().unwrap_or(10000);
    let keep = args.get(2).copied().unwrap_or(4).min(count);
    let setup = Instant::now();
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = native_host::NativeHost(scope.clone());
    let mut shaper = Shaper::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 100., height: 100., scale: 1. };
    let css: String = (0..rules).map(|i| format!(".r{i}{{height:{}px;color:rgb(1,2,3)}}", i % 500)).collect();
    let syntax = Arc::new(css::Syntax::parse(Arc::from(css), Default::default(), None).unwrap());
    let mut pages = Vec::new();
    for i in 0..count {
        let p = page::load_assets(
            Arc::from("<!doctype html><link id=s rel=stylesheet href=initial.css><div></div>"),
            &host,
            &scope,
            &mut shaper,
            &mut painter,
            viewport,
            None,
            |_, _| {},
            |_, _, _| Ok(None),
        )
        .unwrap();
        let mut doc = p.document.clone();
        let mut stack = vec![doc.root()];
        let link = loop {
            let node = stack.pop().unwrap();
            if doc.attribute(node, "id").ok().flatten() == Some("s") {
                break node;
            }
            stack.extend(doc.children(node).unwrap());
        };
        doc.set_attribute(link, "href", format!("job-{i}.css")).unwrap();
        pages.push((p, doc));
    }
    let setup_ms = setup.elapsed().as_secs_f64() * 1000.;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_sample = stop.clone();
    let sampler = std::thread::spawn(move || {
        let mut peak = 0;
        while !stop_sample.load(Ordering::Relaxed) {
            peak = peak.max(workers());
            std::thread::sleep(Duration::from_millis(1));
        }
        peak
    });
    let (tx, rx) = mpsc::channel();
    let mut kept = Vec::new();
    let started = Instant::now();
    let mut resource = |kind, _: Option<&str>, url: &str| -> page::Result<Option<page::Resource>> {
        Ok((kind == page::ResourceKind::Stylesheet).then(|| page::Resource::Stylesheet(syntax.clone(), Some(Arc::from(url)))))
    };
    for (i, (mut p, doc)) in pages.into_iter().enumerate() {
        let tx = tx.clone();
        let wake: page::Wake = Arc::new(move || drop(tx.send(i)));
        assert_eq!(p.refresh(&doc, None, &mut resource, Some(&wake)).unwrap_err().to_string(), page::COMPILING_RULES);
        if i >= count - keep {
            kept.push((i, p, doc, wake));
        }
    }
    let submitted_ms = started.elapsed().as_secs_f64() * 1000.;
    while !kept.is_empty() {
        let id = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        if let Some(index) = kept.iter().position(|(i, ..)| *i == id) {
            let (_, mut p, doc, wake) = kept.swap_remove(index);
            assert!(p.refresh(&doc, None, &mut resource, Some(&wake)).unwrap());
        }
    }
    let ready_ms = started.elapsed().as_secs_f64() * 1000.;
    while workers() > 0 {
        assert!(started.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(1));
    }
    let drained_ms = started.elapsed().as_secs_f64() * 1000.;
    stop.store(true, Ordering::Relaxed);
    let peak = sampler.join().unwrap();
    println!(
        "{{\"pages\":{count},\"rules\":{rules},\"kept\":{keep},\"setup_ms\":{setup_ms},\"submitted_ms\":{submitted_ms},\"ready_ms\":{ready_ms},\"drained_ms\":{drained_ms},\"peak_observed_workers\":{peak}}}"
    );
}
