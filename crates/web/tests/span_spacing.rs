//! Independent Chrome observations of spacing within a styled paragraph.
use olo_web::{
    css::media::Media,
    dom::{Document, NodeId},
    font::{Builder, FontSet, GenericPolicy},
    layout, page,
    text::shaping,
};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn find(doc: &Document, id: &str) -> NodeId {
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
fn span_spacing_matches_chrome_after_restyles_and_text_reuse() {
    compare(
        include_str!("span-spacing-fixtures/base.html"),
        include_str!("span-spacing-fixtures/cases.json"),
        include_str!("span-spacing-fixtures/chrome.json"),
        include_bytes!("inline-fixtures/plain.ttf"),
    );
}
#[test]
fn text_items_preserve_ligatures_and_spacing_survives_line_reshaping() {
    compare(
        include_str!("span-spacing-fixtures/ligature.html"),
        include_str!("span-spacing-fixtures/ligature-cases.json"),
        include_str!("span-spacing-fixtures/ligature-chrome.json"),
        include_bytes!("inline-fixtures/ligature.ttf"),
    );
}
fn compare(html: &str, cases: &str, oracle: &str, font: &[u8]) {
    let oracle: serde_json::Value = serde_json::from_str(oracle).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(cases)), oracle["cases_sha256"]);
    assert_eq!(format!("{:x}", Sha256::digest(html)), oracle["html_sha256"]);
    assert_eq!(format!("{:x}", Sha256::digest(font)), oracle["font_sha256"]);
    let cases: serde_json::Value = serde_json::from_str(cases).unwrap();
    let mut builder = Builder::new(Default::default());
    builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut failures = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let viewport = layout::Viewport { width: 1000., height: 578., scale };
        let prepared = page::prepare(
            Arc::from(html),
            &scope,
            Media::screen(1000., 578., scale, false),
            true,
            None,
            |_, _| {},
            |kind, _, _| Ok(matches!(kind, page::ResourceKind::Font).then(|| page::Resource::Font(Arc::from(font)))),
        )
        .unwrap();
        let host = paint_host::Scaled::new(prepared.scope().clone(), scale);
        let measurer = prepared.measurer();
        let mut doc = prepared.into_document();
        let mut shaper = shaping::Engine::default();
        let mut previous = None;
        for case in cases.as_array().unwrap() {
            let expected = oracle["rows"].as_array().unwrap().iter().find(|r| r["id"] == case["id"] && r["scale"].as_f64() == Some(scale)).unwrap();
            for id in ["p", "a", "n", "b", "q", "c", "m", "d"] {
                let node = find(&doc, id);
                doc.set_attribute(node, "style", case["styles"][id].as_str().unwrap_or("")).unwrap();
            }
            let measured = measurer.measure(&doc, &host, &mut shaper, viewport, previous, None).unwrap();
            let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
            for id in ["p", "a", "n", "b", "q", "c", "m", "d"] {
                let node = find(&doc, id);
                let rect = measured.border_box(node).unwrap();
                assert_eq!(Some(rect), fresh.border_box(node), "cached/fresh {} at {scale}, {id}", case["id"]);
                for (i, got) in [rect.x, rect.y, rect.width, rect.height].into_iter().enumerate() {
                    let want = expected["rects"][id][i].as_f64().unwrap();
                    if (got - want).abs() > 0.02 {
                        failures.push(format!("{} at {scale}: {id}[{i}] {got} != {want}", case["id"]));
                    }
                }
            }
            previous = Some(measured);
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}
