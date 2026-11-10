//! A natural-width line must stay on one line at fractional scales. Pin the
//! one-device-subpixel fit boundary independently with Chrome observations.
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
fn node(doc: &Document, id: &str) -> NodeId {
    let mut q = vec![doc.root()];
    while let Some(n) = q.pop() {
        if doc.attribute(n, "id").ok().flatten() == Some(id) {
            return n;
        }
        q.extend(doc.children(n).unwrap())
    }
    panic!("missing {id}")
}
#[test]
fn natural_width_and_subpixel_shortages_match_chrome_after_reuse() {
    let base = include_str!("fractional-fit-fixtures/base.html");
    let cases: serde_json::Value = serde_json::from_str(include_str!("fractional-fit-fixtures/cases.json")).unwrap();
    let oracle: serde_json::Value = serde_json::from_str(include_str!("fractional-fit-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(base)), oracle["html_sha256"]);
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("fractional-fit-fixtures/cases.json"))), oracle["cases_sha256"]);
    let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf");
    assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(&font).unwrap())), oracle["font_sha256"]);
    let mut b = Builder::new(Default::default());
    b.load_file(&font, |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut errors = Vec::new();
    let mut checked = 0;
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let viewport = layout::Viewport { width: 1000., height: 600., scale };
        let mut engine = shaping::Engine::default();
        for case in cases.as_array().unwrap() {
            let mode = case["mode"].as_str().unwrap();
            let size = case["size"].as_f64().unwrap();
            let html = base.replace("LEFT", case["left"].as_str().unwrap()).replace("RIGHT", case["right"].as_str().unwrap());
            let p = page::prepare(Arc::from(html), &scope, Media::screen(1000., 600., scale, false), false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
            let host = paint_host::Scaled::new(p.scope().clone(), scale);
            let m = p.measurer();
            let mut doc = p.into_document();
            let a = node(&doc, "a");
            let parent = node(&doc, "parent");
            doc.set_attribute(parent, "style", if mode == "flex" { "display:flex" } else { "display:block" }).unwrap();
            let initial = format!("font-size:{size}px;display:{};width:{}", if mode == "inline-block" { "inline-block" } else { "block" }, if mode == "max-content" { "max-content" } else { "auto" });
            let mut previous = None;
            let mut natural = 0.;
            for state in ["natural", "short", "short2", "short4", "wide", "restored"] {
                let delta = match state {
                    "short" => Some(-1.),
                    "short2" => Some(-2.),
                    "short4" => Some(-4.),
                    "wide" => Some(1.),
                    _ => None,
                };
                let style = delta.map_or_else(|| initial.clone(), |d| format!("{initial};width:{}px", natural + d / (64. * scale)));
                doc.set_attribute(a, "style", &style).unwrap();
                let got = m.measure(&doc, &host, &mut engine, viewport, previous, None).unwrap();
                let fresh = m.measure(&doc, &host, &mut engine, viewport, None, None).unwrap();
                let rect = got.border_box(a).unwrap();
                let fresh = fresh.border_box(a).unwrap();
                if state == "natural" {
                    natural = rect.width;
                }
                let expected = oracle["rows"].as_array().unwrap().iter().find(|r| r["id"] == case["id"] && r["state"] == state && r["scale"].as_f64() == Some(scale)).unwrap();
                // Existing line-height storage retains a floating CSS value rather than
                // Chrome's floored 1/64 device grid: at most two lines' rounding here.
                let tolerance = [0.02, 2. / (64. * scale) + 1e-5];
                for (k, (v, f)) in [rect.width, rect.height].into_iter().zip([fresh.width, fresh.height]).enumerate() {
                    let e = expected["rect"][k].as_f64().unwrap();
                    if (v - e).abs() > tolerance[k] || v != f {
                        errors.push(format!("{} {scale} {state}[{k}] reused={v} fresh={f} Chrome={e}", case["id"]));
                    }
                }
                checked += 1;
                previous = Some(got);
            }
        }
    }
    assert_eq!(checked, 1350);
    assert!(errors.is_empty(), "{} differences:\n{}", errors.len(), errors.iter().take(65).cloned().collect::<Vec<_>>().join("\n"));
}
