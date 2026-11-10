//! Container-query geometry observed independently in Chrome, then read through
//! the script measurement path with and without previous-layout reuse.
use olo_web::{
    css::media::Media,
    dom::{Document, NodeId},
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn node(doc: &Document, id: &str) -> NodeId {
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.attribute(n, "id").ok().flatten() == Some(id) {
            return n;
        }
        stack.extend(doc.children(n).unwrap());
    }
    panic!("missing {id}")
}
#[test]
fn container_queries_settle_before_script_geometry_is_returned() {
    let html = include_str!("query-measure-fixtures/base.html");
    let cases: serde_json::Value = serde_json::from_str(include_str!("query-measure-fixtures/cases.json")).unwrap();
    let oracle: serde_json::Value = serde_json::from_str(include_str!("query-measure-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(html)), oracle["html_sha256"]);
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("query-measure-fixtures/cases.json"))), oracle["cases_sha256"]);
    let mut builder = Builder::new(Default::default());
    builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut errors = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let viewport = layout::Viewport { width: 1000., height: 578., scale };
        let prepared = page::prepare(Arc::from(html), &scope, Media::screen(1000., 578., scale, false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
        let host = paint_host::Scaled::new(prepared.scope().clone(), scale);
        let measurer = prepared.measurer();
        let mut doc = prepared.into_document();
        let mut engine = shaping::Engine::default();
        let mut previous = None;
        for case in cases.as_array().unwrap() {
            let expected = oracle["rows"].as_array().unwrap().iter().find(|r| r["id"] == case["id"] && r["scale"].as_f64() == Some(scale)).unwrap();
            for id in expected["boxes"].as_object().unwrap().keys() {
                doc.set_attribute(node(&doc, id), "style", case["styles"][id].as_str().unwrap_or("")).unwrap();
            }
            let measured = measurer.measure(&doc, &host, &mut engine, viewport, previous, None).unwrap();
            let fresh = measurer.measure(&doc, &host, &mut engine, viewport, None, None).unwrap();
            for (id, want) in expected["boxes"].as_object().unwrap() {
                let n = node(&doc, id);
                let got = measured.border_box(n).unwrap_or_default();
                let new = fresh.border_box(n).unwrap_or_default();
                for (k, (got, new)) in [got.x, got.y, got.width, got.height].into_iter().zip([new.x, new.y, new.width, new.height]).enumerate() {
                    let expected = want["rect"][k].as_f64().unwrap();
                    if (got - expected).abs() > 0.02 || (new - expected).abs() > 0.02 {
                        errors.push(format!("{} {scale} {id}[{k}]: reused {got}, fresh {new}, Chrome {expected}", case["id"]));
                    }
                }
                let padding = measured.computed(n, "padding-right").unwrap();
                if padding != want["paddingRight"].as_str().unwrap() {
                    errors.push(format!("{} {scale} {id} padding-right {padding} != {}", case["id"], want["paddingRight"]));
                }
            }
            previous = Some(measured);
        }
    }
    assert!(errors.is_empty(), "{} differences:\n{}", errors.len(), errors.join("\n"));
}
#[test]
fn query_paint_layout_matches_chrome_through_nested_cascades_and_mutations() {
    let mut builder = Builder::new(Default::default());
    builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!("query-measure-fixtures/cases.json")).unwrap();
    let oracle: serde_json::Value = serde_json::from_str(include_str!("query-measure-fixtures/chrome.json")).unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = paint_host::Scaled::new(scope.clone(), scale);
        let viewport = layout::Viewport { width: 1000., height: 578., scale };
        let mut page = page::load(Arc::from(include_str!("query-measure-fixtures/base.html")), &host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
        for case in cases.as_array().unwrap() {
            let expected = oracle["rows"].as_array().unwrap().iter().find(|r| r["id"] == case["id"] && r["scale"].as_f64() == Some(scale)).unwrap();
            for id in expected["boxes"].as_object().unwrap().keys() {
                let n = node(&page.document, id);
                page.document.set_attribute(n, "style", case["styles"][id].as_str().unwrap_or("")).unwrap();
            }
            page.restyle(&host, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
            for (id, want) in expected["boxes"].as_object().unwrap() {
                let n = node(&page.document, id);
                let layout = page.scene.layout();
                let bounds = layout.cache().tree().iter().find(|(_, b)| b.node == n).and_then(|(id, _)| layout.bounds(id)).unwrap_or_default();
                for (k, got) in [bounds.x, bounds.y, bounds.width, bounds.height].into_iter().enumerate() {
                    let expected = want["rect"][k].as_f64().unwrap();
                    assert!((got - expected).abs() <= 0.02, "{} {scale} {id}[{k}]: paint {got}, Chrome {expected}", case["id"]);
                }
            }
        }
    }
}
