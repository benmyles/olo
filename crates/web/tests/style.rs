use olo_web::css::{
    Syntax,
    cascade::{Builder, Priority},
    color, math,
    properties::{self as p, Declaration, Keyword as K, Property as P},
    style::{self, Defaults, FontMetrics, FontRequest, Host, Options, Style, Value},
};
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
struct Snapshot {
    revision: Cell<u64>,
    scheme: color::Scheme,
    scale: f64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self { revision: Cell::new(1), scheme: color::Scheme::Light, scale: 1. }
    }
}
impl Host for Snapshot {
    fn revision(&self) -> u64 {
        self.revision.get()
    }
    fn scheme(&self) -> color::Scheme {
        self.scheme
    }
    fn system_color(&self, color: color::SystemColor, scheme: color::Scheme) -> color::Result<color::Color> {
        let white = match color {
            color::SystemColor::CanvasText => scheme == color::Scheme::Dark,
            color::SystemColor::Canvas => scheme == color::Scheme::Light,
            _ => return Err(color::Error::ContextUnavailable),
        };
        Ok(color::Color::new(color::Space::Srgb, [if white { 1. } else { 0. }; 3], 1., 0))
    }
    fn font(&self, request: FontRequest) -> style::Result<FontMetrics> {
        Ok(FontMetrics { id: 7, ex: request.size * 0.5, cap: request.size * 0.75, ch: request.size * 0.5, ic: request.size, normal_line_height: request.size * 1.25 })
    }
    fn font_size(&self, keyword: K, parent: f64) -> style::Result<f64> {
        Ok(match keyword {
            K::Medium => 16.,
            K::Small => 13.,
            K::Large => 18.,
            K::Larger => parent * 1.25,
            K::Smaller => parent / 1.25,
            _ => return Err(style::Error::Unsupported("test font-size policy")),
        })
    }
    fn device_scale(&self) -> f64 {
        self.scale
    }
    fn factor(&self, unit: math::RelativeUnit, _: Option<style::Axes>) -> math::Result<f64> {
        match unit {
            math::RelativeUnit::Vw => Ok(12.),
            math::RelativeUnit::Vh => Ok(8.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}
fn compute(defaults: &Arc<Defaults>, host: &Snapshot, source: &str, parent: Option<&Style>, root: Option<&Style>, options: Options) -> Style {
    let syntax = Syntax::parse(Arc::from(source), Default::default(), None).unwrap();
    let declarations: Vec<_> = syntax.declarations(None).unwrap().iter().map(|d| Declaration::compile(d, Default::default(), None).unwrap()).collect();
    let mut b = Builder::new(Default::default(), None);
    for (i, d) in declarations.iter().enumerate() {
        b.add(d, Priority { declaration: i as u32, rule: i as u64 + 1, ..Default::default() }).unwrap();
    }
    let cascade = b.finish(parent.is_some(), parent.map(|p| p as &dyn olo_web::css::cascade::Inherited)).unwrap();
    Style::compute(defaults.clone(), &cascade, parent, root, host, options, Default::default(), None).unwrap()
}
fn scalar(style: &Style, p: P) -> f64 {
    style.value(p).scalar().unwrap_or_else(|| panic!("{:?}", style.value(p)))
}
fn keyword(style: &Style, p: P) -> K {
    let Value::Keyword(k) = style.value(p) else { panic!("{:?}", style.value(p)) };
    *k
}
fn rgba(style: &Style, p: P) -> [u8; 4] {
    style.used_color(p).unwrap().srgb8_clipped()
}
#[test]
fn defaults_are_computed_once_and_sparse_overrides_own_no_parent() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let root = compute(&defaults, &host, "", None, None, Options::default());
    assert_eq!(scalar(&root, P::FontSize), 16.);
    assert_eq!(scalar(&root, P::FontWeight), 400.);
    assert_eq!(root.line_height(), 20.);
    assert_eq!(scalar(&root, P::BorderTopWidth), 3.);
    assert_eq!(root.used_border_width(P::BorderTopWidth).unwrap(), 0.);
    assert_eq!(scalar(&root, P::WordSpacing), 0.);
    assert_eq!(rgba(&root, P::Color), [0, 0, 0, 255]);
    if !cfg!(feature = "baseline-style-dense") {
        assert_eq!(root.override_payload_bytes(), 0);
    }
    let child = compute(&defaults, &host, "", Some(&root), None, Options::default());
    if !cfg!(feature = "baseline-style-dense") {
        assert!(root.overrides_share(&child));
    }
    assert!(Arc::ptr_eq(root.font(), child.font()));
    drop(root);
    assert_eq!(scalar(&child, P::FontSize), 16.);
    assert_eq!(rgba(&child, P::Color), [0, 0, 0, 255]);
}
#[test]
fn font_units_freeze_before_inheritance_but_percentages_wait_for_layout() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parent = compute(&defaults, &host, "font-size:20px; width:calc(50% - 2em); height:2em; margin-top:2ex", None, None, Options { root: true, ..Default::default() });
    let child = compute(&defaults, &host, "font-size:40px; width:inherit; height:inherit; margin-top:inherit", Some(&parent), Some(&parent), Options::default());
    assert_eq!(scalar(&child, P::Height), 40.);
    assert_eq!(scalar(&child, P::MarginTop), 20.);
    struct Layout;
    impl math::Context for Layout {
        fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
            assert_eq!(unit, math::RelativeUnit::Percentage);
            Ok(4.)
        }
    }
    let Value::Numeric(n) = child.value(P::Width) else { panic!() };
    assert!(!n.expression().dependencies().contains(math::RelativeUnit::Em));
    assert!(n.expression().dependencies().contains(math::RelativeUnit::Percentage));
    assert_eq!(n.resolve(&Layout, &mut Default::default(), Default::default(), None).unwrap(), 160.);
    let local = compute(&defaults, &host, "font-size:40px;width:calc(50% - 2em)", Some(&parent), Some(&parent), Options::default());
    let Value::Numeric(n) = local.value(P::Width) else { panic!() };
    assert_eq!(n.resolve(&Layout, &mut Default::default(), Default::default(), None).unwrap(), 120.);
}
#[test]
fn font_affecting_values_use_parent_metrics_and_root_units_use_initial_root_size() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let root = compute(&defaults, &host, "font-size:2rem; line-height:2; margin-top:1rem", None, None, Options { root: true, ..Default::default() });
    assert_eq!(scalar(&root, P::FontSize), 32.);
    assert_eq!(scalar(&root, P::MarginTop), 32.);
    let child = compute(&defaults, &host, "font-size:4ex; font-weight:calc(2em / 1px); height:2rem; margin-top:2rlh", Some(&root), Some(&root), Options::default());
    assert_eq!(scalar(&child, P::FontSize), 64.);
    assert_eq!(scalar(&child, P::FontWeight), 64.);
    assert_eq!(scalar(&child, P::Height), 64.);
    assert_eq!(scalar(&child, P::MarginTop), 128.);
}
#[test]
fn unitless_line_height_inherits_as_a_multiplier_while_percentages_become_lengths() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    for (line, want) in [("1.5", 48.), ("150%", 24.), ("1.5em", 24.), ("normal", 40.)] {
        let parent = compute(&defaults, &host, &format!("line-height:{line}"), None, None, Options { root: true, ..Default::default() });
        let child = compute(&defaults, &host, "font-size:32px; margin-top:2lh", Some(&parent), Some(&parent), Options::default());
        assert_eq!(child.line_height(), want, "{line}");
        assert_eq!(scalar(&child, P::MarginTop), want * 2.);
    }
    let parent = compute(&defaults, &host, "line-height:2", None, None, Options { root: true, ..Default::default() });
    let child = compute(&defaults, &host, "line-height:2lh;font-size:32px", Some(&parent), Some(&parent), Options::default());
    assert_eq!(child.line_height(), 64.);
}

#[test]
fn text_spacing_normal_is_computed_zero_and_percentages_follow_the_child_font() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parent = compute(&defaults, &host, "font-size:20px;word-spacing:10%;letter-spacing:2em", None, None, Options::default());
    let child = compute(&defaults, &host, "font-size:40px", Some(&parent), None, Options::default());
    assert_eq!(child.used_text_spacing(P::WordSpacing, 40., Default::default(), None).unwrap(), 4.);
    assert_eq!(child.used_text_spacing(P::LetterSpacing, 40., Default::default(), None).unwrap(), 40.);
    let normal = compute(&defaults, &host, "letter-spacing:normal;word-spacing:normal", None, None, Options::default());
    assert_eq!(scalar(&normal, P::LetterSpacing), 0.);
    assert_eq!(scalar(&normal, P::WordSpacing), 0.);
}
#[test]
fn relative_weight_boundaries_follow_the_fonts_table() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    for (weight, bolder, lighter) in [
        (1., 400., 1.),
        (99., 400., 99.),
        (100., 400., 100.),
        (349., 400., 100.),
        (350., 700., 100.),
        (549., 700., 100.),
        (550., 900., 400.),
        (749., 900., 400.),
        (750., 900., 700.),
        (899., 900., 700.),
        (900., 900., 700.),
        (1000., 1000., 700.),
    ] {
        let parent = compute(&defaults, &host, &format!("font-weight:{weight}"), None, None, Options::default());
        for (source, want) in [("font-weight:bolder", bolder), ("font-weight:lighter", lighter)] {
            assert_eq!(scalar(&compute(&defaults, &host, source, Some(&parent), None, Options::default()), P::FontWeight), want);
        }
    }
}
#[test]
fn currentcolor_survives_inheritance_and_light_dark_selects_before_inheriting() {
    for scheme in [color::Scheme::Light, color::Scheme::Dark] {
        let host = Snapshot { scheme, ..Default::default() };
        let defaults = Defaults::new(&host, Default::default(), None).unwrap();
        let parent = compute(&defaults, &host, "color:red;background-color:light-dark(currentcolor,green);border-top-color:currentcolor", None, None, Options::default());
        let child = compute(&defaults, &host, "color:blue;background-color:inherit;border-top-color:inherit", Some(&parent), None, Options::default());
        assert_eq!(rgba(&child, P::BorderTopColor), [0, 0, 255, 255]);
        assert_eq!(rgba(&child, P::BackgroundColor), if scheme == color::Scheme::Light { [0, 0, 255, 255] } else { [0, 128, 0, 255] });
        let current = compute(&defaults, &host, "color:currentcolor", Some(&parent), None, Options::default());
        assert_eq!(rgba(&current, P::Color), [255, 0, 0, 255]);
    }
}
#[test]
fn coupled_overflow_border_and_display_rules_are_computed() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    for (x, y, want_x, want_y) in [("visible", "scroll", K::Auto, K::Scroll), ("clip", "auto", K::Clip, K::Auto), ("overlay", "clip", K::Auto, K::Clip), ("clip", "visible", K::Clip, K::Visible)] {
        let style = compute(&defaults, &host, &format!("overflow-x:{x};overflow-y:{y}"), None, None, Options::default());
        assert_eq!(keyword(&style, P::OverflowX), want_x);
        assert_eq!(keyword(&style, P::OverflowY), want_y);
    }
    let style = compute(&defaults, &host, "border-top-width:5px;border-top-style:hidden;border-right-style:solid", None, None, Options::default());
    assert_eq!(scalar(&style, P::BorderTopWidth), 5.);
    assert_eq!(style.used_border_width(P::BorderTopWidth).unwrap(), 0.);
    assert_eq!(scalar(&style, P::BorderRightWidth), 3.);
    for source in ["display:inline-block;float:left", "display:inline-block;position:absolute", "display:contents"] {
        let style = compute(&defaults, &host, source, None, None, Options { root: true, ..Default::default() });
        assert!(matches!(style.value(P::Display), Value::Display(style::Display::Box { outer: style::Outer::Block, inner: style::Inner::Flow, .. })));
    }
}
#[test]
fn logical_inheritance_uses_parent_axes_and_custom_tokens_do_not_rebind() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parent = compute(&defaults, &host, "--x:10px;--y:var(--x);margin-left:11px;margin-right:22px", None, None, Options::default());
    let child = compute(&defaults, &host, "--x:20px;direction:rtl;margin-inline-start:inherit;width:var(--y)", Some(&parent), None, Options::default());
    assert_eq!(scalar(&child, P::MarginRight), 11.);
    assert_eq!(scalar(&child, P::Width), 10.);
    assert_eq!(child.custom("--x").unwrap().value().unwrap().source_spelling(), "20px");
    assert_eq!(parent.custom("--x").unwrap().value().unwrap().source_spelling(), "10px");
    let child = compute(&defaults, &host, "direction:rtl;all:inherit", Some(&parent), None, Options::default());
    assert_eq!(scalar(&child, P::MarginRight), 22.);
}
#[test]
fn style_generation_cancellation_and_resource_failures_do_not_publish() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let cascade = Builder::new(Default::default(), None).finish(false, None).unwrap();
    for limits in [style::Limits { work: 0, ..Default::default() }, style::Limits { properties: 0, ..Default::default() }, style::Limits { retained_bytes: 0, ..Default::default() }] {
        assert!(matches!(Style::compute(defaults.clone(), &cascade, None, None, &host, Options::default(), limits, None), Err(style::Error::Limit)));
    }
    let cancelled = AtomicBool::new(true);
    assert!(matches!(Style::compute(defaults.clone(), &cascade, None, None, &host, Options::default(), Default::default(), Some(&cancelled)), Err(style::Error::Cancelled)));
    cancelled.store(false, Ordering::Relaxed);
    host.revision.set(2);
    assert!(matches!(Style::compute(defaults, &cascade, None, None, &host, Options::default(), Default::default(), None), Err(style::Error::Stale)));
    assert!(matches!(Defaults::new(&host, Default::default(), Some(&AtomicBool::new(true))), Err(style::Error::Cancelled)));
}

#[test]
fn borders_preserve_inheritance_and_snap_at_all_standard_scales() {
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let host = Snapshot { scale, ..Default::default() };
        let defaults = Defaults::new(&host, Default::default(), None).unwrap();
        let parent = compute(&defaults, &host, "border-top-width:5px;border-top-style:none", None, None, Options::default());
        let child = compute(&defaults, &host, "border-top-width:inherit;border-top-style:solid", Some(&parent), None, Options::default());
        assert_eq!(parent.used_border_width(P::BorderTopWidth).unwrap(), 0.);
        assert_eq!(scalar(&child, P::BorderTopWidth), (5. * scale).floor() / scale);
        for (width, pixels) in [("0px", 0.), ("0.1px", 1.), ("1.9px", (1.9 * scale).floor())] {
            let style = compute(&defaults, &host, &format!("border-top-width:{width};border-top-style:solid"), None, None, Options::default());
            assert_eq!(style.used_border_width(P::BorderTopWidth).unwrap(), pixels / scale);
        }
    }
}
#[test]
fn independent_owned_chrome_probe_values_match_native_computation() {
    let report: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-05-style-chrome.json")).unwrap();
    let mut checked = 0;
    for case in report["rows"].as_array().unwrap() {
        let input = &case["input"];
        let host = Snapshot { scheme: if input["scheme"] == "dark" { color::Scheme::Dark } else { color::Scheme::Light }, ..Default::default() };
        let defaults = Defaults::new(&host, Default::default(), None).unwrap();
        let parent = compute(&defaults, &host, input["parent"].as_str().unwrap(), None, None, Options::default());
        let child = compute(&defaults, &host, input["child"].as_str().unwrap(), Some(&parent), None, Options::default());
        for (property, expected) in case["values"].as_object().unwrap() {
            let expected = expected.as_str().unwrap();
            if property == "overflow" {
                let x = keyword(&child, P::OverflowX).as_str();
                let y = keyword(&child, P::OverflowY).as_str();
                assert_eq!(if x == y { x.to_owned() } else { format!("{x} {y}") }, expected);
            } else {
                let p::Name::Longhand(property) = p::Name::parse(property).unwrap() else { panic!() };
                if property == P::LineHeight {
                    assert_eq!(child.line_height(), expected.strip_suffix("px").unwrap().parse::<f64>().unwrap());
                } else if property == P::BorderTopWidth {
                    assert_eq!(child.used_border_width(property).unwrap(), expected.strip_suffix("px").unwrap().parse::<f64>().unwrap());
                } else if expected.starts_with("rgb(") {
                    let channels: Vec<u8> = expected.strip_prefix("rgb(").unwrap().strip_suffix(')').unwrap().split(',').map(|s| s.trim().parse().unwrap()).collect();
                    assert_eq!(rgba(&child, property), [channels[0], channels[1], channels[2], 255]);
                } else {
                    match child.value(property) {
                        Value::Numeric(_) => assert_eq!(scalar(&child, property), expected.strip_suffix("px").unwrap_or(expected).parse::<f64>().unwrap()),
                        Value::Keyword(k) => assert_eq!(k.as_str(), expected),
                        Value::Display(style::Display::Box { outer: style::Outer::Block, inner: style::Inner::Flow, list_item: false }) => assert_eq!(expected, "block"),
                        v => panic!("unported probe {property:?} {v:?}"),
                    }
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 18);
}

fn rules(source: &str) -> olo_web::css::cascade::RuleSet {
    let syntax = Syntax::parse(Arc::from(source), Default::default(), None).unwrap();
    let mut b = olo_web::css::cascade::SheetBuilder::new(Default::default());
    b.add_sheet(&syntax, olo_web::css::cascade::Origin::Author, None).unwrap();
    b.finish(None).unwrap()
}
#[test]
fn document_job_streams_parent_first_styles_and_skips_contents_for_box_context() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parsed = olo_web::html::parse(
        Arc::from("<!doctype html><html style='font-size:20px'><body><section id=p><div class=contents><span id=target class=target></span></div></section></body>"),
        Default::default(),
        None,
    )
    .unwrap();
    let sheet = rules("#p {display:flex;font-size:2rem;--x:2em;color:red} .contents{display:contents} .target{font-size:30px;inline-size:var(--x)}");
    let mut job = style::Job::new(&parsed.document, &sheet, defaults, style::JobOptions { generation: 7, ..Default::default() }, Default::default(), None).unwrap();
    while !job.progress().finished {
        let before = job.progress();
        let after = job.step(1, |_| &host).unwrap();
        assert!(after.visited - before.visited <= 1);
        assert!(after.styled - before.styled <= 1);
        assert_eq!(job.completed().unwrap().count(), after.styled);
    }
    let tree = job.finish().unwrap();
    assert_eq!(tree.generation(), 7);
    assert_eq!(tree.report().stylesheet_issues, 0);
    let (target, computed) = tree.iter().find(|(n, _)| parsed.document.attribute(*n, "id").unwrap() == Some("target")).unwrap();
    assert_eq!(scalar(computed, P::Width), 60.);
    assert_eq!(rgba(computed, P::Color), [255, 0, 0, 255]);
    assert!(matches!(computed.value(P::Display), Value::Display(style::Display::Box { outer: style::Outer::Block, .. })));
    assert!(tree.get(&parsed.document, target, 7).unwrap().is_some());
    assert!(matches!(tree.get(&parsed.document, target, 8), Err(style::Error::Stale)));
    let memory = style::Memory::for_styles(tree.iter().map(|(_, s)| s));
    assert_eq!(memory.styles, 6);
    assert!(memory.accounted_reserved_bytes > memory.styles_bytes);
    let mut document = parsed.document;
    document.set_attribute(target, "class", "changed").unwrap();
    assert!(matches!(tree.get(&document, target, 7), Err(style::Error::Stale)));
}
#[test]
fn job_failures_poison_completion_and_inline_issues_remain_visible() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parsed = olo_web::html::parse(Arc::from("<div style='width:nope;unknown-prop:7;width:5px'></div>"), Default::default(), None).unwrap();
    let sheet = rules("");
    let mut job = style::Job::new(&parsed.document, &sheet, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    while !job.step(8, |_| &host).unwrap().finished {}
    let tree = job.finish().unwrap();
    assert_eq!(tree.report().inline_invalid, 1);
    assert_eq!(tree.report().inline_unsupported, 1);
    assert_eq!(tree.iter().filter(|(n, _)| parsed.document.name(*n).unwrap() == Some("div")).map(|(_, s)| scalar(s, P::Width)).next(), Some(5.));
    for limits in [
        style::JobLimits { nodes: 0, ..Default::default() },
        style::JobLimits { work: 0, ..Default::default() },
        style::JobLimits { retained_bytes: 0, ..Default::default() },
        style::JobLimits { depth: 0, ..Default::default() },
    ] {
        let mut job = style::Job::new(&parsed.document, &sheet, defaults.clone(), Default::default(), limits, None).unwrap();
        assert!(matches!(job.step(100, |_| &host), Err(style::Error::Limit)));
        assert!(matches!(job.step(100, |_| &host), Err(style::Error::Failed)));
        assert!(matches!(job.finish(), Err(style::Error::Failed)));
    }
    let job = style::Job::new(&parsed.document, &sheet, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    assert!(matches!(job.finish(), Err(style::Error::Incomplete)));
    let cancelled = AtomicBool::new(false);
    let mut job = style::Job::new(&parsed.document, &sheet, defaults, Default::default(), Default::default(), Some(&cancelled)).unwrap();
    job.step(1, |_| &host).unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(job.step(1, |_| &host), Err(style::Error::Cancelled)));
    assert!(matches!(job.finish(), Err(style::Error::Failed)));
}
#[test]
fn explicit_selector_state_snapshots_and_environment_revisions_guard_jobs() {
    use olo_web::{css::selectors, dom::NodeId};
    struct States(bool);
    impl selectors::StateSource for States {
        fn state(&self, _: NodeId, _: Option<selectors::PseudoElement>, _: selectors::State) -> selectors::Result<bool> {
            Ok(self.0)
        }
    }
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parsed = olo_web::html::parse(Arc::from("<div class=target></div>"), Default::default(), None).unwrap();
    let sheet = rules(".target{width:10px}.target:hover{width:20px}");
    for (hover, want) in [(false, 10.), (true, 20.)] {
        let states = States(hover);
        let mut job = style::Job::new(&parsed.document, &sheet, defaults.clone(), style::JobOptions { states: Some(&states), ..Default::default() }, Default::default(), None).unwrap();
        while !job.step(10, |_| &host).unwrap().finished {}
        let tree = job.finish().unwrap();
        let (_, target) = tree.iter().find(|(n, _)| parsed.document.name(*n).unwrap() == Some("div")).unwrap();
        assert_eq!(scalar(target, P::Width), want);
    }
    let mut job = style::Job::new(&parsed.document, &sheet, defaults, Default::default(), Default::default(), None).unwrap();
    job.step(2, |_| &host).unwrap();
    host.revision.set(2);
    assert!(matches!(job.step(10, |_| &host), Err(style::Error::Stale)));
}

#[path = "support/style_fixtures.rs"]
mod fixtures;
#[test]
fn independent_pinned_computed_style_ports() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/style-fixtures"));
    if let Ok(path) = std::env::var("OLO_STYLE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 341);
    assert_eq!(report["counts"]["passed"], 243);
    assert_eq!(report["counts"]["unsupported"], 98);
}

#[test]
fn adjacent_value_sharing_keeps_custom_bindings_separate() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parsed = olo_web::html::parse(Arc::from("<div class=row style='--x:1px'></div><div class=row style='--x:2px'></div>"), Default::default(), None).unwrap();
    let sheet = rules(".row{width:40px;color:red}");
    let mut job = style::Job::new(&parsed.document, &sheet, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(64, |_| &host).unwrap().finished {}
    let tree = job.finish().unwrap();
    let rows: Vec<_> = tree.iter().filter(|(n, _)| parsed.document.name(*n).unwrap() == Some("div")).map(|(_, s)| s).collect();
    assert_eq!(rows.len(), 2);
    if !cfg!(feature = "baseline-style-dense") {
        assert!(rows[0].overrides_share(rows[1]));
    }
    assert_eq!(rows[0].custom("--x").unwrap().value().unwrap().source_spelling(), "1px");
    assert_eq!(rows[1].custom("--x").unwrap().value().unwrap().source_spelling(), "2px");
}

#[test]
fn owned_chrome_alignment_observations_use_computed_parent_direction() {
    let report: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-05-style-align-chrome.json")).unwrap();
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    for case in report["rows"].as_array().unwrap() {
        let parent = case["parent"].as_str().map(|s| compute(&defaults, &host, s, None, None, Options::default()));
        let style = compute(&defaults, &host, case["input"].as_str().unwrap(), parent.as_ref(), None, Options { root: parent.is_none(), ..Default::default() });
        assert_eq!(keyword(&style, P::TextAlign).as_str(), case["textAlign"].as_str().unwrap());
        // The reference build does not expose the unprefixed value. This
        // additional assertion checks the standard alias, not Chrome support.
        let standard = compute(
            &defaults,
            &host,
            &case["input"].as_str().unwrap().replace("-webkit-match-parent", "match-parent"),
            parent.as_ref(),
            None,
            Options { root: parent.is_none(), ..Default::default() },
        );
        assert_eq!(keyword(&style, P::TextAlign), keyword(&standard, P::TextAlign));
    }
    assert_eq!(report["rows"].as_array().unwrap().len(), 4);
}

#[test]
fn orthogonal_box_context_skips_a_display_contents_inheritance_parent() {
    let host = Snapshot::default();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let parsed = olo_web::html::parse(Arc::from("<div style='display:contents;writing-mode:vertical-rl'><span id=target></span></div>"), Default::default(), None).unwrap();
    let sheet = rules("");
    let mut job = style::Job::new(&parsed.document, &sheet, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(32, |_| &host).unwrap().finished {}
    let tree = job.finish().unwrap();
    let (_, target) = tree.iter().find(|(n, _)| parsed.document.attribute(*n, "id").unwrap() == Some("target")).unwrap();
    assert_eq!(target.axes().writing_mode, style::WritingMode::VerticalRl);
    assert!(matches!(target.value(P::Display), Value::Display(style::Display::Box { outer: style::Outer::Inline, inner: style::Inner::FlowRoot, .. })));
}
