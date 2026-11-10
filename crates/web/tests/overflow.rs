//! Chrome observations of viewport overflow propagation, including live restyles.
use olo_web::{
    css::{media::Media, properties::Keyword as K},
    dom::{Document, NodeId},
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn scope() -> olo_web::font::Scope {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
fn find(doc: &Document, selector: &str) -> NodeId {
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if match selector.strip_prefix('#') {
            Some(id) => doc.attribute(n, "id").ok().flatten() == Some(id),
            None => doc.name(n).ok().flatten() == Some(selector),
        } {
            return n;
        }
        stack.extend(doc.children(n).unwrap());
    }
    panic!("no {selector}")
}
#[test]
fn viewport_source_and_one_axis_clips_match_chrome_after_live_restyles() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("overflow-fixtures/chrome.json")).unwrap();
    let scope = scope();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let viewport = layout::Viewport { width: 1000., height: 613., scale };
        let prepared = page::prepare(Arc::from(include_str!("overflow-fixtures/overflow.html")), &scope, Media::screen(1000., 613., scale, false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
        let host = paint_host::Scaled::new(prepared.scope().clone(), scale);
        let measurer = prepared.measurer();
        let mut doc = prepared.into_document();
        let mut shaper = shaping::Engine::default();
        let mut painter = paint::Engine::default();
        let mut previous = None;
        // Returning to the first case exercises display:none -> visible, too.
        for row in oracle.as_array().unwrap().iter().chain(oracle.as_array().unwrap().iter().take(1)) {
            let case = row["case"].as_str().unwrap();
            for (selector, field) in [("html", "html"), ("body", "body"), ("#box", "box")] {
                let n = find(&doc, selector);
                doc.set_attribute(n, "style", row[field].as_str().unwrap()).unwrap();
            }
            let m = measurer.measure(&doc, &host, &mut shaper, viewport, previous, None).unwrap();
            let expected = (row["data"]["size"][0].as_f64().unwrap(), row["data"]["size"][1].as_f64().unwrap());
            // The oracle records integer CSSOM dimensions. Native root
            // extents first snap to device pixels, including the synthetic
            // 613 CSS px viewport at fractional scales.
            let size = m.document_size();
            assert_eq!((size.0.round(), size.1.round()), expected, "{case} {scale}");
            let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
            assert_eq!(m.document_size(), fresh.document_size(), "fresh: {case}");
            assert_eq!(m.layout().cache().tree().viewport_overflow(), fresh.layout().cache().tree().viewport_overflow(), "fresh source: {case}");
            for element in row["data"]["elements"].as_array().unwrap() {
                let n = find(&doc, element["id"].as_str().unwrap());
                for (property, expected) in
                    [("overflow-x", &element["overflow"][0]), ("overflow-y", &element["overflow"][1]), ("scrollbar-width", &element["width"]), ("scrollbar-gutter", &element["gutter"])]
                {
                    // A hidden subtree has no computed snapshot in this engine.
                    if let Some(actual) = m.computed(n, property) {
                        assert_eq!(actual, expected.as_str().unwrap(), "{case} {property}");
                    }
                }
            }
            let mut build = paint::BuildJob::new(&doc, m.layout().clone(), &mut painter, Default::default(), Default::default(), None).unwrap();
            while !build.step(1).unwrap().finished {}
            let scene = build.finish().unwrap();
            for (i, (x, y)) in [(20., 20.), (200., 20.), (20., 200.), (200., 200.)].into_iter().enumerate() {
                let tile = painter.raster(&scene, paint::DeviceRect { x: (x * scale) as i32, y: (y * scale) as i32, width: 1, height: 1 }, None).unwrap();
                let green = row["data"]["hits"][i].as_str() == Some("content");
                assert_eq!(tile.rgba(), if green { &[0, 255, 0, 255] } else { &[255, 255, 255, 255] }, "{case} {scale} at {x},{y}");
            }
            let expected_scrollers = usize::from(matches!(case, "root-auto-body-hidden" | "body-own-scroll" | "nested-clip-auto" | "nested-auto-clip"));
            assert_eq!(scene.scrollers().len(), expected_scrollers, "{case}");
            drop(scene);
            previous = Some(m);
        }
    }
}
#[test]
fn root_scrollbar_properties_do_not_propagate_from_body_or_inherit() {
    let scope = scope();
    let prepared =
        page::prepare(Arc::from("<!doctype html><style>html,body{margin:0}</style><div></div>"), &scope, Media::screen(400., 300., 1., false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
    let measurer = prepared.measurer();
    let mut doc = prepared.into_document();
    let html = find(&doc, "html");
    let body = find(&doc, "body");
    let mut shaper = shaping::Engine::default();
    let mut previous = None;
    for (root_style, body_style, source, axes, width, gutter) in [
        ("", "overflow:scroll;scrollbar-width:thin;scrollbar-gutter:stable both-edges", Some(body), [K::Scroll; 2], K::Auto, K::Auto),
        ("scrollbar-width:none;scrollbar-gutter:both-edges stable", "overflow:hidden", Some(body), [K::Hidden; 2], K::None, K::StableBothEdges),
        ("overflow:clip;scrollbar-width:thin;scrollbar-gutter:stable", "overflow:scroll", Some(html), [K::Hidden; 2], K::Thin, K::Stable),
        ("overflow-x:clip;overflow-y:auto", "overflow:scroll", Some(html), [K::Hidden, K::Auto], K::Auto, K::Auto),
        ("", "display:none;overflow:scroll", Some(html), [K::Auto; 2], K::Auto, K::Auto),
        ("display:none;overflow:scroll", "", None, [K::Auto; 2], K::Auto, K::Auto),
        ("", "", Some(body), [K::Auto; 2], K::Auto, K::Auto),
    ] {
        doc.set_attribute(html, "style", root_style).unwrap();
        doc.set_attribute(body, "style", body_style).unwrap();
        let m = measurer.measure(&doc, &host, &mut shaper, layout::Viewport { width: 400., height: 300., scale: 1. }, previous, None).unwrap();
        assert_eq!(m.layout().cache().tree().viewport_overflow(), layout::ViewportOverflow { source, axes, scrollbar_width: width, scrollbar_gutter: gutter });
        if !root_style.contains("display:none") && !body_style.contains("display:none") {
            assert_eq!(m.computed(find(&doc, "div"), "scrollbar-width").as_deref(), Some("auto"));
            assert_eq!(m.computed(find(&doc, "div"), "scrollbar-gutter").as_deref(), Some("auto"));
        }
        previous = Some(m);
    }
}
#[test]
fn unpainted_overflow_on_an_unclipped_axis_still_reaches_the_page() {
    let scope = scope();
    let viewport = layout::Viewport { width: 1000., height: 613., scale: 1. };
    for (overflow, expected) in [("clip visible", (1000., 1200.)), ("visible clip", (1400., 613.))] {
        let source = format!(
            "<!doctype html><style>html,body{{margin:0}}#box{{width:120px;height:40px;overflow:{overflow}}}#content{{width:1400px;height:1200px}}</style><div id=box><div id=content></div></div>"
        );
        let prepared = page::prepare(Arc::from(source), &scope, Media::screen(1000., 613., 1., false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
        let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
        let measurer = prepared.measurer();
        let doc = prepared.into_document();
        let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
        let mut painter = paint::Engine::default();
        let mut build = paint::BuildJob::new(&doc, m.layout().clone(), &mut painter, Default::default(), Default::default(), None).unwrap();
        while !build.step(1).unwrap().finished {}
        let scene = build.finish().unwrap();
        let r = scene.content_extent();
        assert_eq!(((r.x + r.width).max(viewport.width), (r.y + r.height).max(viewport.height)), expected, "{overflow}");
        assert_eq!(m.document_size(), expected, "{overflow}");
    }
}
