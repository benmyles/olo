use olo_web::{
    dom::Document,
    font::{Builder, FontSet, GenericPolicy, Scope},
    layout::{self, BoxKind},
    page, paint,
    text::shaping,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, atomic::AtomicBool};
#[path = "support/paint_host.rs"]
mod paint_host;

fn scope() -> Scope {
    let mut builder = Builder::new(Default::default());
    builder.load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
fn bounds(doc: &Document, layout: &layout::Layout, name: &str) -> layout::Rect {
    let id = layout.cache().tree().iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { .. }) && doc.attribute(b.node, "id").unwrap() == Some(name)).unwrap().0;
    layout.bounds(id).unwrap()
}
#[test]
fn box_container_lengths_match_independent_chrome_at_five_native_scales() {
    let source: Arc<str> = Arc::from(include_str!("container-fixtures/input.html"));
    let reference: serde_json::Value = serde_json::from_str(include_str!("container-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(source.as_bytes())), reference["source_sha256"]);
    let scope = scope();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut checked = 0;
    for row in reference["rows"].as_array().unwrap() {
        let scale = row["scale"].as_f64().unwrap();
        let host = paint_host::Scaled::new(scope.clone(), scale);
        let viewport = layout::Viewport { width: row["viewport"]["smallWidth"].as_f64().unwrap(), height: row["viewport"]["smallHeight"].as_f64().unwrap(), scale };
        let page = page::load(source.clone(), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        assert_eq!(page.stylesheet_issues, 0);
        for probe in row["probes"].as_array().unwrap() {
            let id = probe["id"].as_str().unwrap();
            let got = bounds(&page.document, page.scene.layout(), id).width;
            let expected = probe["rect"]["width"].as_f64().unwrap();
            // Chrome exposes some fractional geometry through float32. This
            // tolerance is far below one layout/device subpixel at every DPR.
            assert!((got - expected).abs() < 0.0001, "{id} at {scale}: {got} != {expected}");
            checked += 1;
        }
    }
    assert_eq!(checked, 240);
}
#[test]
fn changed_containment_and_viewport_recompute_descendant_box_lengths() {
    let scope = scope();
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut viewport = layout::Viewport { width: 800., height: 600., scale: 1. };
    let mut page = page::load(
        Arc::from("<!doctype html><style>body{margin:0}#host{container-type:inline-size;width:400px}#child{width:50cqi;height:10cqb}</style><div id=host><div id=child></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    let geometry = |page: &page::Page, name| bounds(&page.document, page.scene.layout(), name);
    assert_eq!(geometry(&page, "child").width, 200.);
    assert_eq!(geometry(&page, "child").height, 60.);
    assert_eq!(geometry(&page, "host").height, 60.);
    let node = page.scene.layout().cache().tree().iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { .. }) && page.document.attribute(b.node, "id").unwrap() == Some("host")).unwrap().1.node;
    page.document.set_attribute(node, "style", "container-type:size").unwrap();
    page.restyle(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(geometry(&page, "host").height, 0.);
    assert_eq!(geometry(&page, "child").height, 0.);
    page.document.set_attribute(node, "style", "all:initial;display:block;width:400px").unwrap();
    viewport.width = 1000.;
    viewport.height = 400.;
    page.restyle(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(geometry(&page, "child").width, 500.);
    assert_eq!(geometry(&page, "child").height, 40.);
    page.document.set_attribute(node, "style", "container-type:inline-size;width:50%").unwrap();
    page.restyle(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(geometry(&page, "child").width, 250.);
    // Browser's resize path reuses prepared styles/text when no eager viewport
    // unit was read. Query bases must follow the new layout without restyling.
    let cache = page.scene.layout().cache().clone();
    let mut job = layout::Job::new(&mut shaper, &scope, cache, layout::Viewport { width: 1200., height: 500., scale: 1. }, Default::default(), None).unwrap();
    while !job.step(1).unwrap().finished {}
    let resized = job.finish().unwrap();
    assert_eq!(bounds(&page.document, &resized, "host").width, 600.);
    assert_eq!(bounds(&page.document, &resized, "child").width, 300.);
    assert_eq!(bounds(&page.document, &resized, "child").height, 50.);
    assert!(page.restyle(&host, &mut shaper, &mut painter, viewport, Some(&AtomicBool::new(true)), |_, _| {}).is_err());
}
#[test]
fn early_contexts_and_inherited_pending_lengths_render_with_recorded_fallbacks() {
    let scope = scope();
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    // A container unit on a property computed before layout knows its container
    // cannot be computed: the declaration is dropped, recorded, and the page renders.
    let page = page::load(
        Arc::from("<!doctype html><style>div{font-size:10cqi}</style><div>text</div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 800., height: 600., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    assert!(page.fallbacks.counts().iter().any(|(what, n)| *what == "declaration dropped at computed-value time" && *n >= 1), "{}", page.fallbacks);
    // An inherited container-relative box length keeps its expression and
    // resolves against the inheritor's own container at layout.
    let page = page::load(
        Arc::from("<!doctype html><style>#parent{width:10cqi}#child{width:inherit}</style><div id=parent><div id=child></div></div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        layout::Viewport { width: 800., height: 600., scale: 1. },
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(bounds(&page.document, page.scene.layout(), "parent").width, 80.);
    assert_eq!(bounds(&page.document, page.scene.layout(), "child").width, 80.);
}
