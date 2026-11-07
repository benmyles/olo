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
fn integrated_flex_pages_match_independent_chrome_at_five_native_scales() {
    use sha2::{Digest, Sha256};
    let cases: serde_json::Value = serde_json::from_str(include_str!("flex-flow-fixtures/cases.json")).unwrap();
    let reference: serde_json::Value = serde_json::from_str(include_str!("flex-flow-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("flex-flow-fixtures/cases.json"))), reference["cases_sha256"]);
    let s = scope();
    let mut e = Engine::default();
    let mut failures = Vec::new();
    let mut results = Vec::new();
    for row in reference["rows"].as_array().unwrap() {
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let html = format!("<div id=probe>{}</div>", case["html"].as_str().unwrap());
        let css = format!("#probe{{display:flex;width:301px;flex-direction:row;flex-wrap:nowrap}}{}", case["css"].as_str().unwrap());
        let (d, t) = tree_scaled(&html, &css, &s, 1, scale);
        let c = cache(&mut e, &s, t);
        let mut j = layout::Job::new(&mut e, &s, c, layout::Viewport { width: 800., height: 600., scale }, Default::default(), None).unwrap();
        while !j.step(1).unwrap().finished {}
        let l = j.finish().unwrap();
        let origin = rect(&d, &l, "probe");
        let mut got = serde_json::Map::new();
        let mut passed = true;
        for name in case["measure"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()) {
            let r = rect(&d, &l, name);
            let expected = &row["rects"][name];
            for (key, value) in [("x", r.x - origin.x), ("y", r.y - origin.y), ("width", r.width), ("height", r.height)] {
                passed &= (value - expected[key].as_f64().unwrap()).abs() < 0.0001;
            }
            got.insert(name.into(), serde_json::json!({"x":r.x-origin.x,"y":r.y-origin.y,"width":r.width,"height":r.height}));
        }
        let result = serde_json::json!({"id":row["id"],"scale":scale,"got":got,"expected":row["rects"],"passed":passed});
        if !passed {
            failures.push(result.clone());
        }
        results.push(result);
        // A fresh width job consumes the published intrinsic cache; slices do
        // not alter geometry or force a new preparation/font generation.
        let mut j = layout::Job::new(&mut e, &s, l.cache().clone(), layout::Viewport { width: 800., height: 600., scale }, Default::default(), None).unwrap();
        while !j.step(17).unwrap().finished {}
        let again = j.finish().unwrap();
        for name in case["measure"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()) {
            assert_eq!(rect(&d, &l, name), rect(&d, &again, name));
        }
    }
    if let Ok(path) = std::env::var("OLO_FLEX_FLOW_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    }
    assert_eq!(results.len(), 260);
    assert!(failures.is_empty(), "{}", serde_json::to_string_pretty(&failures).unwrap());
}
#[test]
fn definite_flex_reference_runs_through_production_css_and_page_layout() {
    let cases: serde_json::Value = serde_json::from_str(include_str!("flex-fixtures/cases.json")).unwrap();
    let reference: serde_json::Value = serde_json::from_str(include_str!("flex-fixtures/chrome.json")).unwrap();
    let s = scope();
    let mut e = Engine::default();
    let mut failures = Vec::new();
    for row in reference["rows"].as_array().unwrap() {
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let scale = row["scale"].as_f64().unwrap();
        let mut html = String::from("<div id=probe>");
        let mut css = format!("#probe{{display:flex;align-items:flex-start;width:{}px;height:10px;column-gap:{}px}}", case["available"], case["gap"]);
        for (index, i) in case["items"].as_array().unwrap().iter().enumerate() {
            html.push_str(&format!("<div id=a{index}></div>"));
            let max = i["max"].as_f64().map_or("none".into(), |v| format!("{v}px"));
            css.push_str(&format!("#a{index}{{height:10px;flex-basis:{}px;flex-grow:{};flex-shrink:{};min-width:{}px;max-width:{max};border-style:solid", i["base"], i["grow"], i["shrink"], i["min"]));
            for (side, ix) in [("left", 0), ("right", 1)] {
                for property in ["padding", "margin", "border"] {
                    css.push_str(&format!(";{property}-{side}{}:{}px", if property == "border" { "-width" } else { "" }, i[property][ix]));
                }
            }
            css.push('}');
        }
        html.push_str("</div>");
        let (d, t) = tree_scaled(&html, &css, &s, 1, scale);
        let c = cache(&mut e, &s, t);
        let mut j = layout::Job::new(&mut e, &s, c, layout::Viewport { width: 800., height: 600., scale }, Default::default(), None).unwrap();
        while !j.step(1).unwrap().finished {}
        let l = j.finish().unwrap();
        let origin = rect(&d, &l, "probe");
        for (index, expected) in row["items"].as_array().unwrap().iter().enumerate() {
            let r = rect(&d, &l, &format!("a{index}"));
            if (r.x - origin.x - expected["x"].as_f64().unwrap()).abs() >= 0.0001 || (r.width - expected["border_box"].as_f64().unwrap()).abs() >= 0.0001 {
                failures.push(serde_json::json!({"id":row["id"],"scale":scale,"index":index,"x":r.x-origin.x,"width":r.width,"expected":expected}));
            }
        }
    }
    assert!(failures.is_empty(), "{}", serde_json::to_string_pretty(&failures).unwrap());
}
#[test]
fn flex_reflow_keeps_intrinsics_and_rejects_unimplemented_contexts() {
    let s = scope();
    let mut e = Engine::default();
    let (_, t) = tree("<div id=probe><div id=a>AB CD</div><div id=b>DE</div></div>", "#probe{display:flex}#a,#b{flex:1;min-width:0}", &s, 1);
    let prepared = cache(&mut e, &s, t);
    // Explicit bases/minima need no intrinsic measurements in this first job.
    let first = fit(&mut e, &s, prepared.clone(), 400., 1);
    assert!(Arc::ptr_eq(first.cache(), &prepared));
    let (_, t) = tree("<div id=probe><div id=a>AB CD</div><div id=b>DE</div></div>", "#probe{display:flex}#a,#b{flex-grow:1}", &s, 1);
    let prepared = cache(&mut e, &s, t);
    let first = fit(&mut e, &s, prepared.clone(), 400., 1);
    assert!(!Arc::ptr_eq(first.cache(), &prepared));
    assert!(Arc::ptr_eq(first.cache().text(0).unwrap(), prepared.text(0).unwrap()));
    let mut run = |cache| {
        let mut j = layout::Job::new(&mut e, &s, cache, layout::Viewport { width: 100., height: 800., scale: 1. }, Default::default(), None).unwrap();
        while !j.step(1).unwrap().finished {}
        let work = j.progress().work;
        (j.finish().unwrap(), work)
    };
    let (fresh, fresh_work) = run(prepared);
    let (reused, reused_work) = run(first.cache().clone());
    assert!(reused_work < fresh_work);
    for (id, _) in fresh.cache().tree().iter() {
        assert_eq!(fresh.bounds(id), reused.bounds(id));
    }
    for (input, css, reason) in [
        ("<div id=probe><div>AB</div></div>", "#probe{display:flex;flex-wrap:wrap}", "wrapping flex formatting"),
        ("<div id=probe><div>AB</div></div>", "#probe{display:flex;direction:rtl}", "non-LTR-row flex formatting"),
        ("<div id=probe><div>AB</div></div>", "#probe{display:flex;align-items:baseline}", "flex baseline alignment"),
        ("<div id=probe><div style='order:1'>AB</div></div>", "#probe{display:flex}", "ordered flex items"),
        ("<div id=probe><div><div>AB</div></div></div>", "#probe{display:flex}", "nested flex item intrinsic contributions"),
        ("<div id=probe><div>AB</div></div>", "#probe{display:flex}#probe>div{box-sizing:border-box;flex:1;min-width:0;padding:5px}", "negative border-box flex inner basis"),
    ] {
        let (_, t) = tree(input, css, &s, 1);
        let c = cache(&mut e, &s, t);
        let mut j = layout::Job::new(&mut e, &s, c, layout::Viewport { width: 400., height: 800., scale: 1. }, Default::default(), None).unwrap();
        loop {
            match j.step(1) {
                Ok(p) => assert!(!p.finished, "unexpected support: {reason}"),
                Err(error) => {
                    assert!(matches!(error,layout::Error::Unsupported(r) if r==reason), "{error:?}");
                    break;
                }
            }
        }
        assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    }
}

#[test]
fn flex_intrinsic_work_obeys_global_quota_and_cancellation() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let s = scope();
    let mut e = Engine::default();
    let (_, t) = tree(&format!("<div id=probe><div>{}</div></div>", "AB CD ".repeat(1000)), "#probe{display:flex}", &s, 1);
    let c = cache(&mut e, &s, t);
    let viewport = layout::Viewport { width: 400., height: 800., scale: 1. };
    let mut j = layout::Job::new(&mut e, &s, c.clone(), viewport, layout::Limits { work: 80, ..Default::default() }, None).unwrap();
    loop {
        match j.step(1) {
            Ok(p) => assert!(!p.finished),
            Err(error) => {
                assert!(matches!(error, layout::Error::Limit));
                break;
            }
        }
    }
    assert!(matches!(j.step(1), Err(layout::Error::Failed)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    let cancel = AtomicBool::new(false);
    let mut j = layout::Job::new(&mut e, &s, c, viewport, Default::default(), Some(&cancel)).unwrap();
    while j.progress().work < 80 {
        assert!(!j.step(1).unwrap().finished);
    }
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(j.step(1), Err(layout::Error::Cancelled)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
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
fn semantic_br_survives_collapsing_and_wbr_wraps_without_a_glyph() {
    let s = scope();
    let mut e = Engine::default();
    for (body, css, width, expected, height) in [
        ("A<br>B", "", 100., vec![('A', 0., 0.), ('B', 0., 20.)], 40.),
        ("A <br> B", "", 100., vec![('A', 0., 0.), ('B', 0., 20.)], 40.),
        ("<br>", "", 100., Vec::new(), 20.),
        ("<br><br>", "", 100., Vec::new(), 40.),
        ("A<br>", "", 100., vec![('A', 0., 0.)], 20.),
        ("A<br>B", "#p{white-space:nowrap}", 100., vec![('A', 0., 0.), ('B', 0., 20.)], 40.),
        ("A<wbr>B", "", 30., vec![('A', 0., 0.), ('B', 0., 20.)], 40.),
        ("A<wbr>B", "", 100., vec![('A', 0., 0.), ('B', 20., 0.)], 20.),
        ("A<wbr>B", "#p{white-space:nowrap}", 30., vec![('A', 0., 0.), ('B', 0., 20.)], 40.),
        ("A<br hidden>B", "br[hidden]{display:none}", 100., vec![('A', 0., 0.), ('B', 20., 0.)], 20.),
    ] {
        let (d, t) = tree(&format!("<div id=p>{body}</div>"), css, &s, 1);
        let c = cache(&mut e, &s, t);
        let l = fit(&mut e, &s, c, width, 1);
        assert_eq!(letters(&l), expected, "{body} {css}");
        assert_eq!(rect(&d, &l, "p").height, height, "{body} {css}");
    }
}

fn letters(l: &layout::Layout) -> Vec<(char, f64, f64)> {
    let mut result = Vec::new();
    for (id, _) in l.cache().tree().iter() {
        let Some(text) = l.text_layout(id) else { continue };
        let bounds = l.bounds(id).unwrap();
        for (li, line) in text.lines().iter().enumerate() {
            for fi in line.fragments.clone() {
                let f = &text.fragments()[fi as usize];
                let run = text.run(f).unwrap();
                let unit = text.text().paragraph().font_size / f64::from(run.font.units_per_em());
                let mut pen = 0i64;
                for g in &run.glyphs[f.glyphs.start as usize..f.glyphs.end as usize] {
                    let c = run.source[g.cluster as usize..].chars().next().unwrap();
                    if c.is_ascii_alphabetic() {
                        result.push((c, bounds.x + f.x + pen as f64 * unit, bounds.y + li as f64 * 20.));
                    }
                    pen += i64::from(g.x_advance);
                }
            }
        }
    }
    result
}
#[test]
fn inline_edges_fit_wrap_and_preserve_actual_box_ends() {
    let s = scope();
    let mut e = Engine::default();
    let cases = [
        ("<div id=p>A<span style='margin-left:3px;padding-left:7px;margin-right:5px;padding-right:11px'>B</span>C</div>", 200., vec![('A', 0., 0.), ('B', 30., 0.), ('C', 66., 0.)], 20.),
        ("<div id=p>A <span style='margin-left:10px;margin-right:5px'>B C</span> D</div>", 70., vec![('A', 0., 0.), ('B', 40., 0.), ('C', 0., 20.), ('D', 35., 20.)], 40.),
        ("<div id=p>A<span style='margin-left:10px;margin-right:5px'>B<div>C</div>D</span>E</div>", 200., vec![('A', 0., 0.), ('B', 30., 0.), ('C', 0., 20.), ('D', 0., 40.), ('E', 25., 40.)], 60.),
        ("<div id=p>A<span style='padding-left:10px;padding-right:5px'></span>B</div>", 200., vec![('A', 0., 0.), ('B', 35., 0.)], 20.),
        ("<div id=p>A<span style='margin:auto;margin-top:200px;margin-bottom:100px'>B</span>C</div>", 200., vec![('A', 0., 0.), ('B', 20., 0.), ('C', 40., 0.)], 20.),
        ("<div id=p><span style='margin-left:10px;padding-right:5px'></span></div>", 200., Vec::new(), 20.),
    ];
    for (body, width, expected, height) in cases {
        let (d, t) = tree(body, "", &s, 1);
        let c = cache(&mut e, &s, t);
        let l = fit(&mut e, &s, c, width, 1);
        assert_eq!(letters(&l), expected, "{body}");
        assert_eq!(rect(&d, &l, "p").height, height, "{body}");
    }
}

#[test]
fn used_zero_percentage_edges_restore_cross_span_shaping_without_mutating_cache() {
    let mut builder = Builder::new(Default::default());
    builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/ligature.ttf"), |_| {}, None).unwrap();
    let s = FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline ligature"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut e = Engine::default();
    let (_, t) = tree("<div><span id=s>f</span>i</div>", "html{font-family:'Olo Inline ligature'}#s{padding-right:5%}", &s, 1);
    let c = cache(&mut e, &s, t);
    for (width, glyphs, advance) in [(0., 1, 18.), (100., 2, 45.), (0., 1, 18.)] {
        let l = fit(&mut e, &s, c.clone(), width, 1);
        let text = l.cache().tree().iter().find_map(|(id, _)| l.text_layout(id)).unwrap();
        assert_eq!(text.text().paragraph().runs.iter().map(|r| r.glyphs.len()).sum::<usize>(), glyphs);
        assert_eq!(text.lines()[0].advance, advance);
        assert_eq!(c.text(0).unwrap().paragraph().shaping_breaks(), &[1]);
        assert!(Arc::ptr_eq(text.text().prepared(), c.text(0).unwrap().prepared()));
    }
}

#[test]
fn completed_inline_edges_do_not_create_a_line_for_trailing_collapsed_space() {
    let s = scope();
    let mut e = Engine::default();
    let (d, t) = tree("<div id=p>A<span id=s></span>\n </div>", "#p{white-space:pre-line}#s{margin-right:5px}", &s, 1);
    let c = cache(&mut e, &s, t);
    let l = fit(&mut e, &s, c, 200., 1);
    assert_eq!(rect(&d, &l, "p").height, 20.);
}

#[test]
fn inline_fragment_storage_is_bounded_even_across_block_interruptions() {
    let s = scope();
    let (d, tree) = tree("<div><span>A<div>B</div>C<div>D</div>E</span></div>", "", &s, 1);
    for limit in [0, 1, 2] {
        let mut job = layout::BuildJob::new(&d, tree.styles().clone(), layout::BuildLimits { inline_boxes: limit, ..Default::default() }, None).unwrap();
        loop {
            match job.step(1) {
                Ok(p) => assert!(!p.finished),
                Err(error) => {
                    assert!(matches!(error, layout::Error::Limit));
                    break;
                }
            }
        }
        assert!(matches!(job.step(1), Err(layout::Error::Failed)));
        assert!(matches!(job.finish(), Err(layout::Error::Failed)));
    }
}

#[test]
fn independent_chrome_break_controls() {
    use sha2::{Digest, Sha256};
    let s = scope();
    let mut e = Engine::default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/break-control-fixtures");
    let metadata: serde_json::Value = serde_json::from_str(include_str!("break-control-fixtures/SOURCE.json")).unwrap();
    for (name, hash) in metadata["inputs"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(name)).unwrap())), hash.as_str().unwrap());
    }
    for key in ["base", "font"] {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(metadata[key]["path"].as_str().unwrap())).unwrap())), metadata[key]["sha256"].as_str().unwrap());
    }
    for (_, report) in metadata["reports"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(report["file"].as_str().unwrap())).unwrap())), report["sha256"].as_str().unwrap());
    }
    let known: serde_json::Value = serde_json::from_str(include_str!("break-control-fixtures/known-nonpasses.json")).unwrap();
    let mut nonpasses = std::collections::BTreeMap::new();
    let mut failures = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(format!("chrome-{scale}.json"))).unwrap()).unwrap();
        assert_eq!(report["devicePixelRatio"].as_f64().unwrap(), scale);
        assert_eq!(report["base"].as_str().unwrap().trim(), BASE.trim());
        for c in report["rows"].as_array().unwrap() {
            for budget in [1, 128] {
                let (d, t) = tree_scaled(c["body"].as_str().unwrap(), c["css"].as_str().unwrap(), &s, budget, scale);
                let cache = cache(&mut e, &s, t);
                let mut j = layout::Job::new(&mut e, &s, cache, layout::Viewport { width: c["layoutViewport"]["width"].as_f64().unwrap(), height: 800., scale }, Default::default(), None).unwrap();
                while !j.step(budget).unwrap().finished {}
                let l = j.finish().unwrap();
                let actual = letters(&l);
                let mut coordinates = Vec::new();
                let expected = c["letters"].as_array().unwrap();
                if actual.len() != expected.len() {
                    failures.push(format!("DPR{scale} {}: {} letters != {}", c["name"], actual.len(), expected.len()));
                    continue;
                }
                for ((letter, x, y), expected) in actual.iter().zip(expected) {
                    assert_eq!(letter.to_string(), expected["letter"]);
                    for (key, value) in [("x", *x), ("y", *y)] {
                        let cssom = f64::from((value * scale) as f32 * (1f32 / scale as f32));
                        // DOM Range rectangles enclose caret positions on Blink's
                        // layout grid. They can start one layout unit before
                        // an exact glyph origin, even with this integer font.
                        let allowance = if key == "x" { 1. / (64. * scale) + 1e-5 } else { 1e-5 };
                        if (cssom - expected[key].as_f64().unwrap()).abs() > allowance {
                            coordinates.push(serde_json::json!({"field":format!("{letter}.{key}"),"native":cssom,"chrome":expected[key]}));
                        }
                    }
                }
                let height = rect(&d, &l, "p").height;
                if (height - c["boxes"]["p"]["height"].as_f64().unwrap()).abs() > 1e-5 {
                    coordinates.push(serde_json::json!({"field":"height","native":height,"chrome":c["boxes"]["p"]["height"]}));
                }
                if !coordinates.is_empty() {
                    let key = format!("{scale}:{}", c["name"].as_str().unwrap());
                    if let Some(previous) = nonpasses.insert(key, coordinates.clone()) {
                        assert_eq!(previous, coordinates);
                    }
                }
            }
        }
    }
    assert_eq!(nonpasses.keys().collect::<Vec<_>>(), known["expected_nonpasses"].as_object().unwrap().keys().collect::<Vec<_>>(), "Component non-pass cases changed");
    for (case, coordinates) in nonpasses {
        let baseline = known["expected_nonpasses"][&case].as_array().unwrap();
        assert_eq!(coordinates.len(), baseline.len(), "{case}: mismatch count changed");
        for (actual, expected) in coordinates.iter().zip(baseline) {
            assert_eq!(actual["field"], expected["field"], "{case}");
            for field in ["native", "chrome"] {
                assert!((actual[field].as_f64().unwrap() - expected[field].as_f64().unwrap()).abs() < 1e-9, "{case}: pinned {field} coordinate changed: {actual} != {expected}");
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn independent_chrome_inline_spacing() {
    use sha2::{Digest, Sha256};
    let s = scope();
    let mut e = Engine::default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-box-fixtures");
    let source: serde_json::Value = serde_json::from_str(include_str!("inline-box-fixtures/SOURCE.json")).unwrap();
    for (name, hash) in source["inputs"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(name)).unwrap())), hash.as_str().unwrap());
    }
    for key in ["base", "font"] {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(source[key]["path"].as_str().unwrap())).unwrap())), source[key]["sha256"].as_str().unwrap());
    }
    for (_, report) in source["reports"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(report["file"].as_str().unwrap())).unwrap())), report["sha256"].as_str().unwrap());
    }
    let mut failures = Vec::new();
    let known: serde_json::Value = serde_json::from_str(include_str!("inline-box-fixtures/known-nonpasses.json")).unwrap();
    let mut nonpasses = std::collections::BTreeMap::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(format!("chrome-{scale}.json"))).unwrap()).unwrap();
        assert_eq!(report["devicePixelRatio"].as_f64().unwrap(), scale);
        assert_eq!(report["base"].as_str().unwrap().trim(), BASE.trim());
        for c in report["rows"].as_array().unwrap() {
            for budget in [1, 128] {
                let (d, t) = tree_scaled(c["body"].as_str().unwrap(), c["css"].as_str().unwrap(), &s, budget, scale);
                let cache = cache(&mut e, &s, t);
                let mut j = layout::Job::new(&mut e, &s, cache, layout::Viewport { width: c["layoutViewport"]["width"].as_f64().unwrap(), height: 800., scale }, Default::default(), None).unwrap();
                while !j.step(budget).unwrap().finished {}
                let l = j.finish().unwrap();
                let actual = letters(&l);
                let mut coordinates = Vec::new();
                let expected = c["letters"].as_array().unwrap();
                if actual.len() != expected.len() {
                    failures.push(format!("DPR{scale} {}: {} letters != {}", c["name"], actual.len(), expected.len()));
                    continue;
                }
                for ((letter, x, y), expected) in actual.iter().zip(expected) {
                    assert_eq!(letter.to_string(), expected["letter"]);
                    for (key, value) in [("x", *x), ("y", *y)] {
                        let cssom = f64::from((value * scale) as f32 * (1f32 / scale as f32));
                        // DOM Range rectangles enclose caret positions on Blink's
                        // layout grid. They can start one layout unit before
                        // an exact glyph origin, even with this integer font.
                        let allowance = if key == "x" { 1. / (64. * scale) + 1e-5 } else { 1e-5 };
                        if (cssom - expected[key].as_f64().unwrap()).abs() > allowance {
                            if key == "x" {
                                coordinates.push(serde_json::json!({"letter":letter.to_string(), "native":cssom,"chrome":expected[key]}));
                            } else {
                                failures.push(format!("DPR{scale} {} {letter}.{key}: {cssom} != {}", c["name"], expected[key]));
                            }
                        }
                    }
                }
                if !coordinates.is_empty() {
                    let key = format!("{scale}:{}", c["name"].as_str().unwrap());
                    if let Some(previous) = nonpasses.insert(key, coordinates.clone()) {
                        assert_eq!(previous, coordinates);
                    }
                }
                let height = rect(&d, &l, "p").height;
                if (height - c["boxes"]["p"]["height"].as_f64().unwrap()).abs() > 1e-5 {
                    failures.push(format!("DPR{scale} {} height {height} != {}", c["name"], c["boxes"]["p"]["height"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(nonpasses.keys().collect::<Vec<_>>(), known["expected_nonpasses"].as_object().unwrap().keys().collect::<Vec<_>>(), "Component non-pass cases changed");
    for (case, coordinates) in nonpasses {
        let baseline = known["expected_nonpasses"][&case].as_array().unwrap();
        assert_eq!(coordinates.len(), baseline.len(), "{case}: mismatch count changed");
        for (actual, expected) in coordinates.iter().zip(baseline) {
            assert_eq!(actual["letter"], expected["letter"], "{case}");
            for field in ["native", "chrome"] {
                assert!((actual[field].as_f64().unwrap() - expected[field].as_f64().unwrap()).abs() < 1e-9, "{case}: pinned {field} coordinate changed: {actual} != {expected}");
            }
        }
    }
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

#[test]
fn semantic_opportunities_cannot_reuse_literal_unicode_nowrap_cache_entries() {
    let s = scope();
    let mut e = Engine::default();
    let (d, t) = tree("<div id=semantic>A<wbr>B</div><div id=literal>A\u{200b}B</div>", "div{white-space:nowrap;width:30px}", &s, 1);
    assert_eq!(t.text_source(0).unwrap().source, t.text_source(1).unwrap().source);
    assert_eq!(t.controls(0).len(), 1);
    assert!(t.controls(1).is_empty());
    let c = cache(&mut e, &s, t);
    assert!(!Arc::ptr_eq(c.text(0).unwrap().prepared(), c.text(1).unwrap().prepared()));
    let l = fit(&mut e, &s, c, 100., 1);
    assert_eq!(rect(&d, &l, "semantic").height, 40.);
    assert_eq!(rect(&d, &l, "literal").height, 20.);
}
