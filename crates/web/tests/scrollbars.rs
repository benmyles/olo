//! Native classic viewport geometry against captured Wayland Chrome.
use olo_web::{
    css::media::Media,
    dom::{Document, NodeId},
    layout, page, paint,
    text::shaping,
};
use std::sync::Arc;
#[path = "support/classic_host.rs"]
mod classic_host;
use classic_host::{Classic, scope};
fn find(doc: &Document, selector: &str) -> NodeId {
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.name(n).ok().flatten() == Some(selector) || doc.attribute(n, "id").ok().flatten() == Some(selector) {
            return n;
        }
        stack.extend(doc.children(n).unwrap());
    }
    panic!("no {selector}")
}
const CASES: &[(&str, &str, &str, i32, i32)] = &[
    ("short", "", "", 200, 100),
    ("tall", "", "", 2000, 100),
    ("root-scroll", "overflow:scroll", "", 200, 100),
    ("root-y-scroll", "overflow-y:scroll", "", 200, 100),
    ("root-hidden", "overflow:hidden", "", 2000, 100),
    ("body-hidden", "", "overflow:hidden", 2000, 100),
    ("body-scroll", "", "overflow:scroll", 200, 100),
    ("root-clip", "overflow:clip", "", 2000, 100),
    ("root-x-hidden", "overflow-x:hidden", "", 2000, 100),
    ("stable-short", "scrollbar-gutter:stable", "", 200, 100),
    ("stable-both", "scrollbar-gutter:stable both-edges", "", 200, 100),
    ("body-gutter", "", "scrollbar-gutter:stable both-edges", 200, 100),
    ("root-thin", "scrollbar-width:thin", "", 2000, 100),
    ("root-none", "scrollbar-width:none", "", 2000, 100),
    ("body-thin", "", "scrollbar-width:thin", 2000, 100),
    ("root-rtl", "direction:rtl", "", 2000, 100),
    ("body-rtl", "", "direction:rtl", 2000, 100),
    ("wide", "", "", 200, 120),
    ("wide-tall", "", "", 2000, 120),
    ("root-none-wide", "scrollbar-width:none", "", 2000, 120),
];
#[test]
fn classic_root_scrollports_match_chrome_at_five_scales() {
    let rows: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-10-viewport-overflow/chrome.json")).unwrap();
    let source = include_str!("../../../docs/browser/measurements/2026-11-10-viewport-overflow/probe.html");
    let scope = scope();
    let mut differences = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let short = rows.as_array().unwrap().iter().find(|r| r["scale"].as_f64() == Some(scale) && r["case"] == "short").unwrap();
        let view = layout::Viewport { width: short["data"]["visual"][0].as_f64().unwrap(), height: short["data"]["visual"][1].as_f64().unwrap(), scale };
        let prepared = page::prepare(Arc::from(source), &scope, Media::screen(view.width, view.height, scale, false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
        let host = Classic::new(prepared.scope().clone(), view);
        let measurer = prepared.measurer();
        let mut doc = prepared.into_document();
        let mut shaper = shaping::Engine::default();
        let mut previous = None;
        for &(name, html, body, height, width) in CASES {
            let expected = &rows.as_array().unwrap().iter().find(|r| r["scale"].as_f64() == Some(scale) && r["case"] == name).unwrap()["data"];
            for (selector, css) in [("html", html.to_owned()), ("body", body.to_owned()), ("content", format!("height:{height}px;width:{width}%"))] {
                let n = find(&doc, selector);
                doc.set_attribute(n, "style", &css).unwrap();
            }
            let m = measurer.measure(&doc, &host, &mut shaper, view, previous, None).unwrap();
            let v = m.layout().viewport_geometry();
            let mut compare = |field: &str, actual: &[f64], expected: &serde_json::Value| {
                let expected: Vec<f64> = expected.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                if actual.iter().zip(&expected).any(|(a, b)| (a - b).abs() > 0.03) {
                    differences.push(format!("{name} {scale} {field}: {actual:?} != {expected:?}"));
                }
            };
            compare("visual", &v.client, &expected["visual"]);
            compare("scroll", &[m.document_size().0.round(), m.document_size().1.round()], &expected["html"]["scroll"]);
            for selector in ["html", "body", "percent", "vw", "fixed"] {
                let r = m.border_box(find(&doc, selector)).unwrap();
                compare(selector, &[r.x, r.y, r.width, r.height], &expected[selector]["rect"]);
            }
            let fresh = measurer.measure(&doc, &host, &mut shaper, view, None, None).unwrap();
            assert_eq!(m.layout().viewport_geometry(), fresh.layout().viewport_geometry(), "{name} {scale}: retained vs fresh");
            assert_eq!(m.document_size(), fresh.document_size(), "{name} {scale}: retained vs fresh extent");
            previous = Some(m);
        }
    }
    assert!(differences.is_empty(), "{} discrepancies:\n{}", differences.len(), differences.join("\n"));
}

#[test]
fn classic_rendering_and_measurement_share_gutters_across_restyles_and_resize() {
    let scope = scope();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let mut view = layout::Viewport { width: 1000., height: 700., scale };
        let p = page::prepare(
            Arc::from(include_str!("../../../docs/browser/measurements/2026-11-10-viewport-overflow/probe.html")),
            &scope,
            Media::screen(view.width, view.height, scale, false),
            false,
            None,
            |_, _| {},
            |_, _, _| Ok(None),
        )
        .unwrap();
        let mut host = Classic::new(p.scope().clone(), view);
        let mut measurer = p.measurer();
        let mut shaper = shaping::Engine::default();
        let mut painter = paint::Engine::default();
        let mut page = page::finish(p, &host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
        let mut previous = None;
        for &(name, html, body, height, width) in CASES.iter().chain(CASES.iter().rev()) {
            // The second half also resizes the live page, while returning
            // through every scrollbar policy and removing automatic bars.
            if name == "root-none-wide" {
                view.width = 800.;
                host.view = view;
            }
            for (selector, css) in [("html", html.to_owned()), ("body", body.to_owned()), ("content", format!("height:{height}px;width:{width}%"))] {
                let n = find(&page.document, selector);
                page.document.set_attribute(n, "style", &css).unwrap();
            }
            measurer.set_environment(view, false, None, &mut |_, _, _| Ok(None)).unwrap();
            let m = measurer.measure(&page.document, &host, &mut shaper, view, previous, None).unwrap();
            page.restyle(&host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
            let rendered = page.scene.layout();
            assert_eq!(rendered.viewport_geometry(), m.layout().viewport_geometry(), "{name} {scale}");
            assert_eq!(rendered.document_size(), m.layout().document_size(), "extent {name} {scale}");
            for n in ["html", "body", "percent", "vw", "fixed"] {
                let node = find(&page.document, n);
                let a = rendered.cache().tree().boxes_of(node).next().and_then(|id| rendered.bounds(id)).unwrap();
                assert_eq!(a, m.border_box(node).unwrap(), "{name} {scale} {n}");
            }
            previous = Some(m);
        }
    }
}

#[test]
fn viewport_direction_follows_body_independently_of_overflow_owner() {
    let rows: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-10-classic-viewport/rtl.json")).unwrap();
    let scope = scope();
    let view = layout::Viewport { width: 1000., height: 613., scale: 1. };
    let p = page::prepare(
        Arc::from(include_str!("../../../docs/browser/measurements/2026-11-10-viewport-overflow/probe.html")),
        &scope,
        Media::screen(1000., 613., 1., false),
        false,
        None,
        |_, _| {},
        |_, _, _| Ok(None),
    )
    .unwrap();
    let host = Classic::new(p.scope().clone(), view);
    let measure = p.measurer();
    let mut doc = p.into_document();
    let mut shaper = shaping::Engine::default();
    let mut previous = None;
    let content = find(&doc, "content");
    doc.set_attribute(content, "style", "height:2000px").unwrap();
    for row in rows.as_array().unwrap() {
        for selector in ["html", "body"] {
            let n = find(&doc, selector);
            doc.set_attribute(n, "style", row[selector].as_str().unwrap()).unwrap();
        }
        let m = measure.measure(&doc, &host, &mut shaper, view, previous, None).unwrap();
        let r = m.border_box(find(&doc, "vw")).unwrap();
        assert_eq!(r.x, row["data"]["vw"][0].as_f64().unwrap());
        assert_eq!(r.width, row["data"]["vw"][1].as_f64().unwrap());
        assert_eq!(m.document_size().0, row["data"]["scroll"].as_f64().unwrap());
        assert_eq!(m.layout().cache().tree().viewport_rtl(), row["body"] == "direction:rtl");
        previous = Some(m);
    }
}
#[test]
fn automatic_gutter_passes_share_work_limits_and_cancel() {
    use olo_web::css::style::{Host, ScrollbarWidths};
    use std::sync::atomic::AtomicBool;
    let scope = scope();
    let view = layout::Viewport { width: 400., height: 300., scale: 1. };
    let p = page::prepare(
        Arc::from("<!doctype html><style>html,body{margin:0}div{width:100vw;height:1000px}</style><div></div>"),
        &scope,
        Media::screen(400., 300., 1., false),
        false,
        None,
        |_, _| {},
        |_, _, _| Ok(None),
    )
    .unwrap();
    let host = Classic::new(p.scope().clone(), view);
    let measure = p.measurer();
    let doc = p.into_document();
    let mut shaper = shaping::Engine::default();
    let m = measure.measure(&doc, &host, &mut shaper, view, None, None).unwrap();
    let cache = m.layout().cache();
    let mut reports = Vec::new();
    let laid = layout::layout_viewport(&mut shaper, &scope, cache.clone(), view, host.scrollbar_widths(), None, Default::default(), None, |p| reports.push(p.work)).unwrap();
    assert_eq!(laid.layout.viewport_geometry().bars, [true, true]);
    assert!(reports.windows(2).all(|w| w[0] <= w[1]));
    let limits = layout::Limits { work: laid.progress.work - 1, ..Default::default() };
    assert!(matches!(layout::layout_viewport(&mut shaper, &scope, cache.clone(), view, host.scrollbar_widths(), None, limits, None, |_| {}), Err(layout::Error::Limit)));
    assert!(matches!(
        layout::layout_viewport(&mut shaper, &scope, cache.clone(), view, host.scrollbar_widths(), None, Default::default(), Some(&AtomicBool::new(true)), |_| {}),
        Err(layout::Error::Cancelled)
    ));
    assert!(matches!(
        layout::layout_viewport(&mut shaper, &scope, cache.clone(), view, ScrollbarWidths { auto: f64::NAN, thin: 10. }, None, Default::default(), None, |_| {}),
        Err(layout::Error::Invalid)
    ));
}

#[test]
fn platform_metrics_changes_relayout_without_document_changes() {
    use olo_web::css::style::ScrollbarWidths;
    let scope = scope();
    let view = layout::Viewport { width: 400., height: 300., scale: 1.25 };
    let p = page::prepare(
        Arc::from("<!doctype html><style>html,body{margin:0}div{height:1000px}</style><div></div>"),
        &scope,
        Media::screen(400., 300., 1.25, false),
        false,
        None,
        |_, _| {},
        |_, _, _| Ok(None),
    )
    .unwrap();
    let mut host = Classic::new(p.scope().clone(), view);
    host.widths = ScrollbarWidths::default();
    let measurer = p.measurer();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut page = page::finish(p, &host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
    let mut previous = None;
    for width in [0., 15., 10., 0., 15.] {
        host.widths = ScrollbarWidths { auto: width, thin: width };
        let m = measurer.measure(&page.document, &host, &mut shaper, view, previous, None).unwrap();
        page.restyle(&host, &mut shaper, &mut painter, view, None, |_, _| {}).unwrap();
        let expected = 400. - (width * view.scale).round() / view.scale;
        assert_eq!(m.layout().viewport_geometry().client[0], expected, "measured {width}");
        assert_eq!(page.scene.layout().viewport_geometry().client[0], expected, "rendered {width}");
        previous = Some(m);
    }
}
