use olo_web::css::{
    self, Syntax,
    cascade::{self, Builder, Defaulted, Origin, Priority},
    properties::{Declaration, Property as P, Value},
    selectors::Specificity,
};
use std::sync::{Arc, atomic::AtomicBool};
fn declaration(s: &str) -> Declaration {
    let syntax = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
    Declaration::compile(&syntax.one_declaration(None).unwrap(), Default::default(), None).unwrap()
}
fn spelling(v: Defaulted) -> String {
    match v {
        Defaulted::Initial => "initial".into(),
        Defaulted::Inherit => "inherit".into(),
        Defaulted::Specified(Value::Keyword(k)) => k.as_str().into(),
        Defaulted::Specified(Value::Numeric(n)) => n.resolve(&olo_web::css::math::NoContext, &mut Default::default(), Default::default(), None).unwrap().to_string(),
        v => format!("{v:?}"),
    }
}
fn resolve(rows: &[(Declaration, Priority)], property: P) -> String {
    let mut builder = Builder::new(Default::default(), None);
    for (d, p) in rows {
        builder.add(d, *p).unwrap();
    }
    let cascade = builder.finish(false, None).unwrap();
    spelling(cascade.resolver().property(property, true).unwrap())
}
#[test]
fn origin_importance_inline_layers_specificity_proximity_and_order() {
    let origins = [Origin::UserAgent, Origin::User, Origin::PresentationalHint, Origin::Author, Origin::Animation, Origin::Transition];
    for (i, &a) in origins.iter().enumerate() {
        for (j, &b) in origins.iter().enumerate() {
            let rows = [(declaration("width:1px"), Priority { origin: a, rule: 1, ..Default::default() }), (declaration("width:2px"), Priority { origin: b, rule: 2, ..Default::default() })];
            assert_eq!(resolve(&rows, P::Width), if i > j { "1" } else { "2" });
        }
    }
    let cases = [
        ("width:1px !important", Priority { origin: Origin::Author, ..Default::default() }, "width:2px", Priority { origin: Origin::Animation, ..Default::default() }, "1"),
        ("width:1px !important", Priority { origin: Origin::User, ..Default::default() }, "width:2px !important", Priority { origin: Origin::Author, ..Default::default() }, "1"),
        ("width:1px !important", Priority { origin: Origin::UserAgent, ..Default::default() }, "width:2px !important", Priority { origin: Origin::User, ..Default::default() }, "1"),
        ("width:1px", Priority { layer: 0, specificity: Specificity { ids: 10, ..Default::default() }, ..Default::default() }, "width:2px", Priority::default(), "2"),
        ("width:1px !important", Priority { layer: 0, ..Default::default() }, "width:2px !important", Priority { inline: true, ..Default::default() }, "2"),
        ("width:1px !important", Priority { layer: 0, ..Default::default() }, "width:2px !important", Priority { layer: 1, ..Default::default() }, "1"),
        ("width:1px", Priority { proximity: 1, ..Default::default() }, "width:2px", Priority { proximity: 2, order: 50, ..Default::default() }, "1"),
        ("width:1px", Priority { encapsulation: 0, ..Default::default() }, "width:2px", Priority { encapsulation: 1, ..Default::default() }, "1"),
        ("width:1px !important", Priority { encapsulation: 0, ..Default::default() }, "width:2px !important", Priority { encapsulation: 1, ..Default::default() }, "2"),
    ];
    for (a, mut pa, b, mut pb, want) in cases {
        pa.rule = 1;
        pb.rule = 2;
        assert_eq!(resolve(&[(declaration(a), pa), (declaration(b), pb)], P::Width), want);
    }
}
#[test]
fn rollback_uses_origin_normal_layer_and_rule_boundaries() {
    for (keyword, want) in [("revert", "10"), ("revert-layer", "20"), ("revert-rule", "30")] {
        let rows = [
            (declaration("width:10px"), Priority { origin: Origin::User, rule: 1, ..Default::default() }),
            (declaration("width:20px"), Priority { layer: 0, rule: 2, ..Default::default() }),
            (declaration("width:30px"), Priority { layer: 1, rule: 3, ..Default::default() }),
            (declaration(&format!("width:{keyword}")), Priority { layer: 1, rule: 4, order: 2, ..Default::default() }),
        ];
        assert_eq!(resolve(&rows, P::Width), want);
    }
    // An important revert-layer excludes intervening normal/important layers.
    let rows = [
        (declaration("width:10px"), Priority { layer: 0, rule: 1, ..Default::default() }),
        (declaration("width:revert-layer !important"), Priority { layer: 1, rule: 2, ..Default::default() }),
        (declaration("width:30px !important"), Priority { layer: 2, rule: 3, ..Default::default() }),
    ];
    assert_eq!(resolve(&rows, P::Width), "10");
    let rows =
        [(declaration("width:10px !important"), Priority { rule: 1, ..Default::default() }), (declaration("width:revert-layer !important"), Priority { inline: true, rule: 2, ..Default::default() })];
    assert_eq!(resolve(&rows, P::Width), "10");
    let rows = [(declaration("width:10px"), Priority { origin: Origin::PresentationalHint, rule: 1, ..Default::default() }), (declaration("width:revert"), Priority { rule: 2, ..Default::default() })];
    assert_eq!(resolve(&rows, P::Width), "initial");
}
#[test]
fn defaulting_invalid_substitution_and_rollback_cycles() {
    let rows = [
        (declaration("margin-left:50px"), Priority { layer: 0, rule: 1, ..Default::default() }),
        (declaration("margin-left:revert-rule !important"), Priority { layer: 1, rule: 2, ..Default::default() }),
        (declaration("margin-left:revert-layer"), Priority { layer: 2, rule: 3, ..Default::default() }),
    ];
    assert_eq!(resolve(&rows, P::MarginLeft), "initial");
    let rows = [(declaration("width:10px"), Priority { rule: 1, ..Default::default() }), (declaration("width:var(--missing)"), Priority { rule: 2, order: 1, ..Default::default() })];
    assert_eq!(resolve(&rows, P::Width), "initial");
    let rows = [(declaration("width:var(--missing, revert-rule)"), Priority { rule: 2, order: 1, ..Default::default() }), (declaration("width:10px"), Priority { rule: 1, ..Default::default() })];
    assert_eq!(resolve(&rows, P::Width), "10");
    let declarations = [declaration("font-size:unset"), declaration("width:inherit")];
    let mut b = Builder::new(Default::default(), None);
    for d in &declarations {
        b.add(d, Default::default()).unwrap();
    }
    let c = b.finish(true, None).unwrap();
    assert_eq!(spelling(c.resolver().property(P::FontSize, true).unwrap()), "inherit");
    assert_eq!(spelling(c.resolver().property(P::Width, true).unwrap()), "inherit");
}
#[test]
fn dynamic_custom_rollback_keeps_dependency_guards_and_shared_typed_results() {
    let rows = [
        (declaration("--x:10px"), Priority { layer: 0, rule: 1, ..Default::default() }),
        (declaration("--empty: "), Priority { rule: 2, ..Default::default() }),
        (declaration("--x:var(--empty) revert-layer"), Priority { layer: 1, rule: 3, ..Default::default() }),
        (declaration("margin:var(--x)"), Priority { rule: 4, ..Default::default() }),
    ];
    let mut b = Builder::new(Default::default(), None);
    for (d, p) in &rows {
        b.add(d, *p).unwrap();
    }
    let c = b.finish(false, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(r.custom("--x").unwrap().value().unwrap().source_spelling().trim(), "10px");
    assert_eq!(spelling(r.property(P::MarginLeft, true).unwrap()), "10");
    assert_eq!(spelling(r.property(P::MarginRight, true).unwrap()), "10");
    assert_eq!(r.pending_cached(), 1);
    let rows = [
        (declaration("--x:var(--x,10px)"), Priority { layer: 0, rule: 1, ..Default::default() }),
        (declaration("--x:var(--missing,revert-layer)"), Priority { layer: 1, rule: 2, ..Default::default() }),
    ];
    let mut b = Builder::new(Default::default(), None);
    for (d, p) in &rows {
        b.add(d, *p).unwrap();
    }
    let c = b.finish(false, None).unwrap();
    assert!(c.resolver().custom("--x").unwrap().is_invalid());
}
#[test]
fn cancellation_candidate_sort_work_and_resolution_limits_are_explicit() {
    let d = declaration("width:1px");
    let cancelled = AtomicBool::new(true);
    let mut b = Builder::new(Default::default(), Some(&cancelled));
    assert!(matches!(b.add(&d, Default::default()), Err(cascade::Error::Cancelled)));
    let mut b = Builder::new(cascade::Limits { candidates: 0, ..Default::default() }, None);
    assert!(matches!(b.add(&d, Default::default()), Err(cascade::Error::Limit)));
    let mut b = Builder::new(cascade::Limits { work: 0, ..Default::default() }, None);
    b.add(&d, Default::default()).unwrap();
    b.add(&d, Priority { order: 1, ..Default::default() }).unwrap();
    assert!(matches!(b.finish(false, None), Err(cascade::Error::Limit)));
}
fn sheet(css: &str) -> cascade::RuleSet {
    let syntax = Syntax::parse(Arc::from(css), Default::default(), None).unwrap();
    let mut b = cascade::SheetBuilder::new(Default::default());
    b.add_sheet(&syntax, Origin::Author, None).unwrap();
    b.finish(None).unwrap()
}
fn target(document: &olo_web::dom::Document, selector: &str) -> olo_web::dom::NodeId {
    let syntax = Syntax::parse(Arc::from(selector), Default::default(), None).unwrap();
    let selectors = olo_web::css::selectors::Selectors::compile(syntax.components(), Default::default(), Default::default(), None).unwrap();
    let mut found = None;
    selectors
        .query(&olo_web::css::selectors::DomTree::new(document), document.root(), false, Default::default(), Default::default(), None, |n, _| {
            found = Some(n);
            Ok(())
        })
        .unwrap();
    found.unwrap()
}
#[test]
fn actual_dom_matching_nested_layers_multiple_sheets_and_inline_style() {
    let parsed = olo_web::html::parse(Arc::from("<!doctype html><div id=target class='one Two'></div>"), Default::default(), None).unwrap();
    let node = target(&parsed.document, "#target");
    let tree = olo_web::css::selectors::DomTree::new(&parsed.document);
    let first = Syntax::parse(Arc::from("@layer reset, theme; @layer theme { #target {width:20px} }"), Default::default(), None).unwrap();
    let second = Syntax::parse(
        Arc::from("@layer reset { @layer nested { #target {width:99px} } #target {width:5px} } #missing, .one {height:10px} .Two {height:20px} * {margin-top:1px}"),
        Default::default(),
        None,
    )
    .unwrap();
    let mut b = cascade::SheetBuilder::new(Default::default());
    b.add_sheet(&first, Origin::Author, None).unwrap();
    b.add_sheet(&second, Origin::Author, None).unwrap();
    let rules = b.finish(None).unwrap();
    assert!(rules.issues().is_empty(), "{:?}", rules.issues());
    let mut scratch = cascade::MatchScratch::default();
    let c = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::Width, true).unwrap()), "20");
    assert_eq!(spelling(r.property(P::Height, true).unwrap()), "20");
    assert_eq!(spelling(r.property(P::MarginTop, true).unwrap()), "1");
    let inline = [declaration("width:7px"), declaration("height:revert-layer")];
    let c = rules.match_element(&tree, node, Default::default(), Some(&inline), None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::Width, true).unwrap()), "7");
    assert_eq!(spelling(r.property(P::Height, true).unwrap()), "20");
}
#[test]
fn invalid_layer_names_are_atomic_and_namespaces_are_sheet_local() {
    let parsed = olo_web::html::parse(Arc::from("<!doctype html><svg><rect id=target></rect></svg><div id=html></div>"), Default::default(), None).unwrap();
    let node = target(&parsed.document, "#target");
    let tree = olo_web::css::selectors::DomTree::new(&parsed.document);
    let rules = sheet("@layer a, initial; @layer b { rect {width:20px} } @layer a { rect {width:10px} }");
    let mut scratch = Default::default();
    let c = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
    assert_eq!(spelling(c.resolver().property(P::Width, true).unwrap()), "10");
    let rules = sheet("@namespace svg url('http://www.w3.org/2000/svg'); @namespace 'http://www.w3.org/1999/xhtml'; svg|rect {width:30px} rect {width:40px} *|rect {height:5px}");
    assert!(rules.issues().is_empty(), "{:?}", rules.issues());
    let c = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::Width, true).unwrap()), "30");
    assert_eq!(spelling(r.property(P::Height, true).unwrap()), "5");
}
#[test]
fn all_resets_known_properties_preserves_direction_and_custom_and_chained_rules_descend() {
    let rows = [
        (declaration("width:10px"), Priority { rule: 1, ..Default::default() }),
        (declaration("direction:rtl"), Priority { rule: 2, ..Default::default() }),
        (declaration("--x:3px"), Priority { rule: 3, ..Default::default() }),
        (declaration("all:initial"), Priority { rule: 4, order: 10, ..Default::default() }),
    ];
    let mut b = Builder::new(Default::default(), None);
    for (d, p) in &rows {
        b.add(d, *p).unwrap();
    }
    let c = b.finish(false, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::Width, true).unwrap()), "initial");
    assert_eq!(spelling(r.property(P::Direction, true).unwrap()), "rtl");
    assert_eq!(r.custom("--x").unwrap().value().unwrap().source_spelling(), "3px");
    let mut rows = vec![(declaration("z-index:2"), Priority { rule: 1, ..Default::default() })];
    for i in 2..=50 {
        rows.push((declaration("z-index:revert-rule"), Priority { rule: i, order: i, ..Default::default() }));
    }
    assert_eq!(resolve(&rows, P::ZIndex), "2");
    assert_eq!(olo_web::css::properties::Shorthand::All.properties().len(), P::ALL.len() - 1);
}
#[test]
fn taint_policy_isolated_in_pending_cache_and_retained_budget_is_bounded() {
    let declarations = [declaration("--x:1px"), declaration("margin:var(--x,2px)")];
    let mut b = Builder::new(Default::default(), None);
    b.add_tainted(&declarations[0], Priority { rule: 1, ..Default::default() }, true).unwrap();
    b.add(&declarations[1], Priority { rule: 2, ..Default::default() }).unwrap();
    let c = b.finish(false, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::MarginLeft, true).unwrap()), "1");
    assert_eq!(spelling(r.property(P::MarginRight, false).unwrap()), "2");
    assert_eq!(r.pending_cached(), 2);
    let mut b = Builder::new(cascade::Limits { typed_retained_bytes: 0, ..Default::default() }, None);
    b.add(&declarations[1], Default::default()).unwrap();
    let c = b.finish(false, None).unwrap();
    let mut r = c.resolver();
    assert!(matches!(r.property(P::MarginLeft, true), Err(cascade::Error::Limit)));
    assert_eq!(r.pending_cached(), 0);
}
#[path = "support/cascade_fixtures.rs"]
mod fixtures;
#[test]
fn independent_pinned_cascade_ports() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cascade-fixtures"));
    if let Ok(path) = std::env::var("OLO_CASCADE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 136);
    assert_eq!(report["counts"]["passed"], 128);
    assert_eq!(report["counts"]["unsupported"], 8);
}
#[test]
fn a_rejected_selector_leaves_the_next_rules_matching() {
    // GitHub's reset: an attribute test with an unknown pseudo-element is
    // rejected; the classes interned after it must still match.
    let parsed = olo_web::html::parse(Arc::from("<!doctype html><div id=target class=sr-only></div>"), Default::default(), None).unwrap();
    let node = target(&parsed.document, "#target");
    let tree = olo_web::css::selectors::DomTree::new(&parsed.document);
    let rules = sheet("[type=number]::-webkit-inner-spin-button{height:auto} .sr-only{width:1px} div .sr-only, .sr-only{height:2px}");
    let mut scratch = cascade::MatchScratch::default();
    let c = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
    let mut r = c.resolver();
    assert_eq!(spelling(r.property(P::Width, true).unwrap()), "1");
    assert_eq!(spelling(r.property(P::Height, true).unwrap()), "2");
}
#[test]
fn a_sheet_compiled_again_keeps_its_blocks_and_answers_the_same() {
    // The same parsed sheet compiled again (another medium, other sheets
    // before it) takes its compiled declaration blocks as they are: the
    // rules, their order and the dropped declarations come out the same.
    let parsed = olo_web::html::parse(Arc::from("<!doctype html><div id=target></div>"), Default::default(), None).unwrap();
    let node = target(&parsed.document, "#target");
    let tree = olo_web::css::selectors::DomTree::new(&parsed.document);
    let sheet = Syntax::parse(Arc::from("#target{width:10px;bogus:1;height:2px} @media (min-width:800px){#target{width:20px}}"), Default::default(), None).unwrap();
    let before = Syntax::parse(Arc::from("div{width:5px;margin-top:3px}"), Default::default(), None).unwrap();
    let compile = |width: f64, first: bool| {
        let mut b = cascade::SheetBuilder::new(Default::default()).with_media(olo_web::css::media::Media::screen(width, 600., 1., false));
        if first {
            b.add_sheet(&before, Origin::Author, None).unwrap();
        }
        b.add_sheet(&sheet, Origin::Author, None).unwrap();
        b.finish(None).unwrap()
    };
    let mut scratch = cascade::MatchScratch::default();
    for (width, first, expected) in [(1000., false, "20"), (600., true, "10"), (1000., true, "20"), (600., false, "10")] {
        let rules = compile(width, first);
        assert_eq!(rules.issues().len(), 1, "the dropped declaration, every time: {:?}", rules.issues());
        let c = rules.match_element(&tree, node, Default::default(), None, None, Default::default(), Default::default(), &mut scratch, None, None).unwrap();
        let mut r = c.resolver();
        assert_eq!(spelling(r.property(P::Width, true).unwrap()), expected, "at {width}");
        assert_eq!(spelling(r.property(P::Height, true).unwrap()), "2");
        assert_eq!(spelling(r.property(P::MarginTop, true).unwrap()), if first { "3" } else { "initial" });
    }
}
