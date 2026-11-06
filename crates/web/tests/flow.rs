use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        style::{self, Defaults},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
    html,
    layout::{self, BoxKind},
    text::shaping::Engine,
};
use std::{path::Path, sync::Arc};
#[path = "support/native_font_host.rs"]
mod native_host;
fn scope() -> Scope {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
const BASE: &str = include_str!("flow-fixtures/base.css");
struct Scaled<'a>(&'a native_host::NativeHost, f64);
impl style::Host for Scaled<'_> {
    fn revision(&self) -> u64 {
        self.0.revision() ^ self.1.to_bits().rotate_left(7)
    }
    fn scheme(&self) -> olo_web::css::color::Scheme {
        self.0.scheme()
    }
    fn system_color(&self, c: olo_web::css::color::SystemColor, s: olo_web::css::color::Scheme) -> olo_web::css::color::Result<olo_web::css::color::Color> {
        self.0.system_color(c, s)
    }
    fn font(&self, r: &style::FontRequest) -> style::Result<style::FontMetrics> {
        self.0.font(r)
    }
    fn font_context(&self) -> u64 {
        self.0.font_context()
    }
    fn font_size(&self, k: style::Keyword, p: f64) -> style::Result<f64> {
        self.0.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        self.1
    }
}
fn tree(input: &str, css: &str, scope: &Scope, budget: usize) -> (olo_web::dom::Document, Arc<layout::Tree>) {
    tree_scaled(input, css, scope, budget, 1.)
}
fn tree_scaled(input: &str, css: &str, scope: &Scope, budget: usize, scale: f64) -> (olo_web::dom::Document, Arc<layout::Tree>) {
    let document = html::parse(Arc::from(format!("<!doctype html><html id=root><head></head><body id=body>{input}</body></html>")), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from(format!("{BASE}{css}")), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default());
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = builder.finish(None).unwrap();
    let native = native_host::NativeHost(scope.clone());
    let host = Scaled(&native, scale);
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&document, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(budget, |_| &host).unwrap().finished {}
    let styles = Arc::new(job.finish().unwrap());
    assert_eq!(styles.report().stylesheet_issues, 0);
    assert_eq!(styles.report().inline_invalid, 0);
    assert_eq!(styles.report().inline_unsupported, 0);
    let mut j = layout::BuildJob::new(&document, styles, Default::default(), None).unwrap();
    while !j.step(budget).unwrap().finished {}
    let tree = j.finish().unwrap();
    (document, tree)
}
fn cache(e: &mut Engine, s: &Scope, t: Arc<layout::Tree>) -> Arc<layout::Cache> {
    let mut j = layout::PrepareJob::new(e, s, t, Default::default(), None).unwrap();
    while !j.step(1).unwrap().finished {}
    j.finish().unwrap()
}
fn fit(e: &mut Engine, s: &Scope, c: Arc<layout::Cache>, width: f64, budget: usize) -> layout::Layout {
    let mut j = layout::Job::new(e, s, c, layout::Viewport { width, height: 800., scale: 1. }, Default::default(), None).unwrap();
    while !j.step(budget).unwrap().finished {}
    j.finish().unwrap()
}
fn id(doc: &olo_web::dom::Document, l: &layout::Layout, name: &str) -> layout::BoxId {
    l.cache().tree().iter().find(|(_, b)| matches!(b.kind, BoxKind::Block { .. }) && doc.attribute(b.node, "id").unwrap() == Some(name)).unwrap().0
}
fn rect(doc: &olo_web::dom::Document, l: &layout::Layout, name: &str) -> layout::Rect {
    l.bounds(id(doc, l, name)).unwrap()
}
#[test]
fn block_width_padding_border_auto_margins_and_percentages() {
    let s = scope();
    let mut e = Engine::default();
    let (d, t) = tree(
        "<div id=a></div><div id=b></div>",
        "#a{width:100px;height:20px;padding:10px;border-width:2px;border-style:solid;margin-left:auto;margin-right:auto}#b{width:50%;height:20px;padding:5%;margin-left:5%;margin-right:5%}",
        &s,
        1,
    );
    let c = cache(&mut e, &s, t);
    let l = fit(&mut e, &s, c, 400., 1);
    assert_eq!(rect(&d, &l, "a"), layout::Rect { x: 138., y: 0., width: 124., height: 44. });
    assert_eq!(rect(&d, &l, "b"), layout::Rect { x: 20., y: 44., width: 240., height: 60. });
}
#[test]
fn adjoining_positive_negative_and_empty_block_margins() {
    let s = scope();
    let mut e = Engine::default();
    for (input, css, expected) in [
        (
            "<div id=a></div><div id=b></div>",
            "#a{height:20px;margin-top:10px;margin-bottom:30px}#b{height:20px;margin-top:20px;margin-bottom:5px}",
            vec![("a", 10., 20.), ("b", 60., 20.), ("body", 10., 70.), ("root", 0., 85.)],
        ),
        ("<div id=a></div><div id=b></div>", "#a{height:20px;margin-bottom:30px}#b{height:20px;margin-top:-10px}", vec![("a", 0., 20.), ("b", 40., 20.)]),
        (
            "<div id=a></div><div id=empty></div><div id=b></div>",
            "#a{height:20px;margin-bottom:10px}#empty{margin-top:20px;margin-bottom:30px}#b{height:20px;margin-top:40px}",
            vec![("empty", 40., 0.), ("b", 60., 20.)],
        ),
        ("<div id=empty></div><div id=b></div>", "#empty{margin-top:20px;margin-bottom:30px}#b{height:20px;margin-top:40px}", vec![("empty", 40., 0.), ("b", 40., 20.), ("body", 40., 20.)]),
        ("<div id=a></div>", "body{display:flow-root}#a{height:20px;margin-top:10px;margin-bottom:5px}", vec![("a", 10., 20.), ("body", 0., 35.)]),
    ] {
        let (d, t) = tree(input, css, &s, 1);
        let c = cache(&mut e, &s, t);
        let l = fit(&mut e, &s, c, 400., 1);
        for (name, y, height) in expected {
            let r = rect(&d, &l, name);
            assert_eq!((r.y, r.height), (y, height), "{name} {css}")
        }
    }
}
#[test]
fn inline_sources_survive_wrap_block_splits_and_resize() {
    let s = scope();
    let mut e = Engine::default();
    let (d, t) = tree("<div id=p>A <span id=s>B</span> C</div>", "", &s, 1);
    let c = cache(&mut e, &s, t);
    let l = fit(&mut e, &s, c.clone(), 50., 1);
    assert_eq!(rect(&d, &l, "p").height, 40.);
    let text = l.cache().tree().iter().find(|(_, b)| matches!(b.kind, BoxKind::InlineContent { .. })).unwrap().0;
    let text_layout = l.text_layout(text).unwrap();
    assert_eq!(text_layout.lines().iter().map(|l| l.advance).collect::<Vec<_>>(), vec![50., 20.]);
    let source = c.tree().text_source(0).unwrap();
    assert_eq!(source.spans.len(), 3);
    assert_eq!(d.attribute(source.span_at(2).unwrap().style_node, "id").unwrap(), Some("s"));
    assert!(Arc::ptr_eq(text_layout.text(), c.text(0).unwrap()));
    let wide = fit(&mut e, &s, c, 100., 64);
    assert_eq!(rect(&d, &wide, "p").height, 20.);
    let (d, t) = tree("<div id=p>A<span>B<div id=b>C</div>D</span>E</div>", "", &s, 2);
    let c = cache(&mut e, &s, t);
    let l = fit(&mut e, &s, c, 50., 2);
    assert_eq!(rect(&d, &l, "p").height, 60.);
    assert_eq!(rect(&d, &l, "b").y, 20.);
    assert_eq!(l.cache().tree().iter().filter(|(_, b)| matches!(b.kind, BoxKind::InlineContent { .. })).count(), 3);
}

#[test]
fn independent_chrome_normal_flow_rectangles() {
    use sha2::{Digest, Sha256};
    let metadata: serde_json::Value = serde_json::from_str(include_str!("flow-fixtures/SOURCE.json")).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/flow-fixtures");
    for (name, hash) in metadata["inputs"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), hash.as_str().unwrap());
    }
    for (_, report) in metadata["reports"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(dir.join(report["file"].as_str().unwrap())).unwrap())), report["sha256"].as_str().unwrap());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(metadata["font"]["path"].as_str().unwrap())).unwrap())), metadata["font"]["sha256"].as_str().unwrap());
    let scope = scope();
    let mut e = Engine::default();
    let mut failures = Vec::new();
    for data in [include_str!("flow-fixtures/chrome-1.json"), include_str!("flow-fixtures/chrome-1.5.json"), include_str!("flow-fixtures/chrome-2.json")] {
        let report: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(BASE.trim(), report["base"].as_str().unwrap().trim());
        let scale = report["devicePixelRatio"].as_f64().unwrap();
        for c in report["rows"].as_array().unwrap() {
            for budget in [1, 128] {
                let (d, t) = tree_scaled(c["body"].as_str().unwrap(), c["css"].as_str().unwrap(), &scope, budget, scale);
                let cache = cache(&mut e, &scope, t);
                let viewport = layout::Viewport { width: c["layoutViewport"]["width"].as_f64().unwrap(), height: c["layoutViewport"]["height"].as_f64().unwrap(), scale };
                let mut j = layout::Job::new(&mut e, &scope, cache, viewport, Default::default(), None).unwrap();
                while !j.step(budget).unwrap().finished {}
                let l = j.finish().unwrap();
                for (name, expected) in c["boxes"].as_object().unwrap() {
                    let actual = rect(&d, &l, name);
                    for (key, value) in [("x", actual.x), ("y", actual.y), ("width", actual.width), ("height", actual.height)] {
                        // Blink exports unzoomed physical rect coordinates through float32;
                        // native retained geometry keeps exact binary layout-grid arithmetic.
                        let cssom = f64::from((value * scale) as f32 * (1f32 / scale as f32));
                        if (cssom - expected[key].as_f64().unwrap()).abs() > 1e-9 {
                            failures.push(format!("DPR{scale} {} {name}.{key}: {cssom} != {}", c["name"], expected[key]));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn stale_contexts_cancellation_quotas_and_poisoning() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let s = scope();
    let mut e = Engine::default();
    let (mut d, t) = tree("<div id=p>A B C</div>", "", &s, 1);
    let styles = t.styles().clone();
    let cancelled = AtomicBool::new(true);
    assert!(matches!(layout::BuildJob::new(&d, styles.clone(), Default::default(), Some(&cancelled)), Err(layout::Error::Cancelled)));
    cancelled.store(false, Ordering::Relaxed);
    let mut j = layout::BuildJob::new(&d, styles.clone(), layout::BuildLimits { nodes: 0, ..Default::default() }, None).unwrap();
    assert!(matches!(j.step(1), Err(layout::Error::Limit)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    let mut j = layout::BuildJob::new(&d, styles.clone(), Default::default(), Some(&cancelled)).unwrap();
    assert!(!j.step(0).unwrap().finished);
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(j.step(1), Err(layout::Error::Cancelled)));
    assert!(matches!(j.step(1), Err(layout::Error::Failed)));
    drop(j);
    for limits in [
        layout::BuildLimits { boxes: 1, ..Default::default() },
        layout::BuildLimits { text_bytes: 0, ..Default::default() },
        layout::BuildLimits { paragraph_bytes: 0, ..Default::default() },
        layout::BuildLimits { spans: 0, ..Default::default() },
        layout::BuildLimits { depth: 1, ..Default::default() },
    ] {
        let mut j = layout::BuildJob::new(&d, styles.clone(), limits, None).unwrap();
        assert!(matches!(j.step(1024), Err(layout::Error::Limit)));
    }
    let c = cache(&mut e, &s, t.clone());
    assert!(matches!(layout::Job::new(&mut e, &s, c.clone(), layout::Viewport { width: 400., height: 800., scale: 2. }, Default::default(), None), Err(layout::Error::Stale)));
    let mut j = layout::Job::new(&mut e, &s, c.clone(), layout::Viewport { width: 400., height: 800., scale: 1. }, layout::Limits { work: 0, ..Default::default() }, None).unwrap();
    assert!(matches!(j.step(1), Err(layout::Error::Limit)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    cancelled.store(false, Ordering::Relaxed);
    let mut j = layout::Job::new(&mut e, &s, c.clone(), layout::Viewport { width: 400., height: 800., scale: 1. }, Default::default(), Some(&cancelled)).unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(j.step(1), Err(layout::Error::Cancelled)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    for scale in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(matches!(layout::Job::new(&mut e, &s, c.clone(), layout::Viewport { width: 400., height: 800., scale }, Default::default(), None), Err(layout::Error::Invalid)));
    }
    let node = t.boxes()[1].node;
    d.set_attribute(node, "data-changed", "yes").unwrap();
    assert!(matches!(t.validate(&d), Err(layout::Error::Stale)));
    assert!(matches!(layout::BuildJob::new(&d, styles, Default::default(), None), Err(layout::Error::Style(style::Error::Stale))));
    fn send_sync<T: Send + Sync>() {}
    send_sync::<layout::Tree>();
    send_sync::<layout::Cache>();
    send_sync::<layout::Layout>();
}

#[test]
fn unsupported_layout_never_silently_flattens_inline_typography() {
    let s = scope();
    let mut e = Engine::default();
    for css in ["span{font-size:30px}", "span{direction:rtl}", "span{white-space:pre}", "span{line-height:30px}"] {
        let (_, t) = tree("<div>A <span>B</span> C</div>", css, &s, 1);
        let mut j = layout::PrepareJob::new(&mut e, &s, t, Default::default(), None).unwrap();
        assert!(matches!(j.step(1), Err(layout::Error::Unsupported(_))));
        assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    }
}

#[test]
fn identical_paragraphs_share_glyphs_with_distinct_dom_source_mappings() {
    let s = scope();
    let mut e = Engine::default();
    let (d, t) = tree("<div id=a>A B C</div><div id=b><span id=s>A B C</span></div>", "span{color:red}", &s, 1);
    let mut j = layout::PrepareJob::new(&mut e, &s, t.clone(), Default::default(), None).unwrap();
    while !j.step(1).unwrap().finished {}
    assert_eq!(j.progress().reused_texts, 1);
    let shared = j.finish().unwrap();
    assert!(Arc::ptr_eq(shared.text(0).unwrap(), shared.text(1).unwrap()));
    let raw = shared.tree().text_source(1).unwrap();
    assert_eq!(d.attribute(raw.span_at(0).unwrap().style_node, "id").unwrap(), Some("s"));
    let mut j = layout::PrepareJob::new(&mut e, &s, t, layout::Limits { cached_texts: 0, ..Default::default() }, None).unwrap();
    while !j.step(1).unwrap().finished {}
    assert_eq!(j.progress().reused_texts, 0);
    let separate = j.finish().unwrap();
    assert!(!Arc::ptr_eq(separate.text(0).unwrap(), separate.text(1).unwrap()));
    assert_eq!(shared.glyph_geometry_payload_bytes() * 2, separate.glyph_geometry_payload_bytes());
    let (_, t) = tree("<div>A B C</div><div>A <span>B</span> C</div>", "", &s, 1);
    let c = cache(&mut e, &s, t);
    assert!(!Arc::ptr_eq(c.text(0).unwrap(), c.text(1).unwrap()));
    assert_eq!(c.text(1).unwrap().prepared().spans().len(), 3);
}
