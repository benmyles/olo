use olo_web::css::{
    self, Syntax, color, math,
    properties::{self as p, Declaration, Display, Error, Inner, Keyword, Name, Outer, Property as P, Value},
    variables,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
fn syntax(s: &str) -> Syntax {
    Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap()
}
fn parse(name: &str, s: &str) -> p::Result<Declaration> {
    Declaration::parse(name, syntax(s).components(), false, p::Limits::default(), None)
}
fn values(name: &str, s: &str) -> p::Assignments {
    let Declaration::Known { values, .. } = parse(name, s).unwrap() else { panic!() };
    values
}
struct NumericHost;
impl math::Context for NumericHost {
    fn factor(&self, u: math::RelativeUnit) -> math::Result<f64> {
        match u {
            math::RelativeUnit::Em => Ok(20.),
            math::RelativeUnit::Percentage => Ok(2.),
            _ => Err(math::Error::ContextUnavailable),
        }
    }
}
fn number(v: &Value) -> f64 {
    let Value::Numeric(n) = v else { panic!("{v:?}") };
    n.resolve(&NumericHost, &mut math::Scratch::default(), math::Limits::default(), None).unwrap()
}
#[test]
fn registry_names_initial_values_and_inheritance_are_typed() {
    assert_eq!(P::ALL.len(), 187);
    assert_eq!(p::Shorthand::ALL.len(), 51);
    for &property in P::ALL {
        assert!(Name::parse(property.as_str()).is_some());
        assert_eq!(Name::parse(&property.as_str().to_ascii_uppercase()), Some(Name::Longhand(property)));
        assert_eq!(values(property.as_str(), property.initial_spelling())[0].property, property);
    }
    for &shorthand in p::Shorthand::ALL {
        assert_eq!(Name::parse(shorthand.as_str()), Some(Name::Shorthand(shorthand)));
    }
    assert!(P::Color.inherited());
    assert!(P::FontSize.inherited());
    assert!(!P::Width.inherited());
    // Legacy names Chrome still reads.
    assert_eq!(Name::parse("word-wrap"), Some(Name::Longhand(P::OverflowWrap)));
    assert_eq!(Name::parse("-webkit-appearance"), Some(Name::Longhand(P::Appearance)));
    assert_eq!(Name::parse("Grid-Row-Gap"), Some(Name::Longhand(P::RowGap)));
    assert_eq!(parse("unknown-property", "red").unwrap_err(), Error::UnknownProperty);
    assert_eq!(parse("--", "red").unwrap_err(), Error::UnknownProperty);
    let s = syntax("MARGIN: 1PX 2px !ImPoRtAnT;");
    let statement = s.one_declaration(None).unwrap();
    let d = Declaration::compile(&statement, p::Limits::default(), None).unwrap();
    assert!(d.important());
    let Declaration::Known { values: v, .. } = d else { panic!() };
    assert_eq!(number(&v[0].value), 1.);
    assert_eq!(number(&v[1].value), 2.);
}
#[test]
fn numeric_dimensions_ranges_zero_percentages_and_integer_rounding() {
    for (prop, s, want) in [
        ("width", "1in", 96.),
        ("width", "50%", 100.),
        ("margin-top", "calc(2em + 10%)", 60.),
        ("padding-top", "calc(-1px)", 0.),
        ("opacity", "150%", 1.),
        ("opacity", "-0.2", 0.),
        ("opacity", "calc(50%)", 0.5),
        ("order", "calc(-1.5)", -1.),
        ("order", "calc(1.5)", 2.),
        ("order", "calc(-1.6)", -2.),
        ("font-weight", "calc(-10)", 1.),
        ("font-weight", "calc(2000)", 1000.),
        ("line-height", "1.2", 1.2),
        ("line-height", "125%", 250.),
        ("vertical-align", "-10%", -20.),
    ] {
        assert_eq!(number(&values(prop, s)[0].value), want, "{prop}: {s}");
    }
    for (prop, s) in [
        ("width", "calc(0)"),
        ("width", "-1px"),
        ("width", "1deg"),
        ("width", "none"),
        ("max-width", "auto"),
        ("padding", "-1%"),
        ("padding", "auto"),
        ("border-top-width", "1%"),
        ("order", "1.0"),
        ("order", "1%"),
        ("font-weight", "0"),
        ("font-weight", "1001"),
        ("font-weight", "1px"),
        ("flex-grow", "-0.1"),
        ("flex-grow", "1%"),
        ("line-height", "-1"),
        ("gap", "-1px"),
    ] {
        assert_eq!(parse(prop, s).unwrap_err(), Error::Invalid, "{prop}: {s}");
    }
    for s in ["0", "+0.0", "-0", "0px", "0%"] {
        assert_eq!(number(&values("padding", s)[0].value), 0.);
    }
    assert_eq!(number(&values("opacity", "calc(NaN)")[0].value), 0.);
    assert_eq!(number(&values("width", "1e999px")[0].value), f64::MAX);
    assert_eq!(number(&values("margin-top", "-1e999px")[0].value), -f64::MAX);
    assert_eq!(number(&values("width", "calc(infinity * 1px)")[0].value), f64::MAX);
    assert_eq!(number(&values("order", "calc(-infinity)")[0].value), -f64::MAX);
    assert!(matches!(parse("width", "anchor-size(width)"), Err(Error::Unsupported(_))));
    let v = values("width", "fit-content(calc(50% - 1px))");
    let Value::FitContent(n) = &v[0].value else { panic!() };
    assert_eq!(n.resolve(&NumericHost, &mut math::Scratch::default(), math::Limits::default(), None).unwrap(), 99.);
    let v = values("width", "1em");
    assert!(v[0].value.dependencies().color.math.contains(math::RelativeUnit::Em));
}
#[test]
fn shorthands_expand_atomically_and_keep_logical_identity() {
    for (s, want) in [("1px", [1., 1., 1., 1.]), ("1px 2px", [1., 2., 1., 2.]), ("1px 2px 3px", [1., 2., 3., 2.]), ("1px 2px 3px 4px", [1., 2., 3., 4.])] {
        let v = values("margin", s);
        assert_eq!(v.iter().map(|a| number(&a.value)).collect::<Vec<_>>(), want);
        assert_eq!(v.iter().map(|a| a.property).collect::<Vec<_>>(), [P::MarginTop, P::MarginRight, P::MarginBottom, P::MarginLeft]);
    }
    let v = values("padding-inline", "1px 2px");
    assert_eq!(v[0].property, P::PaddingInlineStart);
    assert_eq!(v[1].property, P::PaddingInlineEnd);
    assert_eq!(number(&v[1].value), 2.);
    let v = values("flex-flow", "wrap column");
    assert!(matches!(v[0].value, Value::Keyword(Keyword::Column)));
    assert!(matches!(v[1].value, Value::Keyword(Keyword::Wrap)));
    let v = values("flex-flow", "wrap");
    assert!(matches!(v[0].value, Value::Keyword(Keyword::Row)));
    for (prop, s) in [
        ("margin", "1px 2px 3px 4px 5px"),
        ("padding-inline", "1px 2px 3px"),
        ("margin", "1px initial"),
        ("padding", "1px -2px"),
        ("flex-flow", "row column"),
        ("flex-flow", "wrap nowrap"),
        ("overflow", "auto nonsense"),
    ] {
        assert_eq!(parse(prop, s).unwrap_err(), Error::Invalid);
    }
    for &shorthand in p::Shorthand::ALL {
        let v = values(shorthand.as_str(), "revert-layer");
        assert_eq!(v.len(), shorthand.properties().len());
        assert!(v.iter().all(|a| matches!(a.value, Value::Wide(variables::Wide::RevertLayer))));
    }
}
#[test]
fn display_box_types_aliases_and_order_independent_grammar() {
    for (s, outer, inner, list_item) in [
        ("inline-block", Outer::Inline, Inner::FlowRoot, false),
        ("flex", Outer::Block, Inner::Flex, false),
        ("flow", Outer::Block, Inner::Flow, false),
        ("ruby", Outer::Inline, Inner::Ruby, false),
        ("list-item inline flow-root", Outer::Inline, Inner::FlowRoot, true),
        ("flow-root list-item run-in", Outer::RunIn, Inner::FlowRoot, true),
    ] {
        assert!(matches!(&values("display",s)[0].value,Value::Display(Display::Box {outer:o,inner:i,list_item:l}) if (*o,*i,*l)==(outer,inner,list_item)), "{s}");
    }
    for s in ["inline inline", "block run-in", "flex grid", "list-item flex", "none inline", "table-row inline", "flow flow", "list-item list-item", "block flow list-item list-item"] {
        assert_eq!(parse("display", s).unwrap_err(), Error::Invalid, "{s}");
    }
    assert!(matches!(values("display", "table-cell")[0].value, Value::Display(Display::Internal(_))));
}
#[test]
fn typed_colors_and_styles_do_not_retain_source() {
    struct Host;
    impl color::Context for Host {
        fn scheme(&self) -> color::Scheme {
            color::Scheme::Dark
        }
    }
    let v = values("border-color", "red #00ff00 rgb(0 0 255) light-dark(white,black)");
    for (a, want) in v.iter().zip([[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [0, 0, 0, 255]]) {
        let Value::Color(c) = &a.value else { panic!() };
        assert_eq!(c.resolve(&Host, color::Limits::default(), None).unwrap().srgb8_clipped(), want);
    }
    assert!(v[3].value.dependencies().color.scheme);
    assert!(matches!(values("caret-color", "auto")[0].value, Value::Keyword(Keyword::Auto)));
    assert_eq!(parse("color", "auto").unwrap_err(), Error::Invalid);
    let source: Arc<str> = Arc::from("width: calc(1em + 1px)");
    let weak = Arc::downgrade(&source);
    let syntax = Syntax::parse(source, css::Limits::default(), None).unwrap();
    let d = Declaration::compile(&syntax.one_declaration(None).unwrap(), p::Limits::default(), None).unwrap();
    drop(syntax);
    assert!(weak.upgrade().is_none());
    let Declaration::Known { values: v, .. } = d else { panic!() };
    assert_eq!(number(&v[0].value), 21.);
}
struct Variables(BTreeMap<String, variables::Value>);
impl variables::Context for Variables {
    fn lookup(&self, name: &str) -> Option<variables::Binding<'_>> {
        self.0.iter().enumerate().find(|(_, (key, _))| key.as_str() == name).map(|(i, (_, v))| variables::Binding { id: i as u64, value: v, animation_tainted: false })
    }
}
#[test]
fn pending_values_share_shorthand_and_invalid_results_become_unset() {
    let v = values("margin", "var(--m)");
    assert_eq!(v.len(), 4);
    assert!(v[0].value.dependencies().substitution);
    let Value::Pending(pending) = &v[0].value else { panic!() };
    for a in &v[1..] {
        let Value::Pending(other) = &a.value else { panic!() };
        assert!(Arc::ptr_eq(pending, other));
    }
    let mut host = Variables(BTreeMap::new());
    host.0.insert("--m".into(), variables::Value::compile(syntax("1px 2px").components(), variables::Limits::default(), None).unwrap());
    let mut resolver = variables::Resolver::new(&host, variables::Limits::default(), None);
    let out = pending.resolve(&mut resolver, true, p::Limits::default(), None).unwrap();
    assert_eq!(number(&out[1].value), 2.);
    for s in ["var(--missing)", "var(--missing, 1px bogus)", "1px var(--missing)"] {
        let v = values("padding", s);
        let Value::Pending(pending) = &v[0].value else { panic!() };
        let out = pending.resolve(&mut resolver, true, p::Limits::default(), None).unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|a| matches!(a.value, Value::Wide(variables::Wide::Unset))));
    }
    let v = values("width", "var(--missing, inherit)");
    let Value::Pending(pending) = &v[0].value else { panic!() };
    let out = pending.resolve(&mut resolver, true, p::Limits::default(), None).unwrap();
    assert!(matches!(out[0].value, Value::Wide(variables::Wide::Inherit)));
    assert_eq!(parse("margin", "var()").unwrap_err(), Error::Invalid);
    let Declaration::Custom { name, .. } = parse("--CaseSensitive", "  ").unwrap() else { panic!() };
    assert_eq!(&*name, "--CaseSensitive");
}
#[test]
fn cancellation_work_depth_and_input_quotas_are_checked() {
    let cancelled = AtomicBool::new(true);
    assert_eq!(Declaration::parse("width", syntax("1px").components(), false, p::Limits::default(), Some(&cancelled)).unwrap_err(), Error::Cancelled);
    for limits in [p::Limits { nodes: 0, ..Default::default() }, p::Limits { bytes: 1, ..Default::default() }, p::Limits { work: 2, ..Default::default() }] {
        assert_eq!(Declaration::parse("width", syntax("1px").components(), false, limits, None).unwrap_err(), Error::Limit);
    }
    let nested = format!("{}1{}", "calc(".repeat(100), ")".repeat(100));
    assert_eq!(parse("opacity", &nested).unwrap_err(), Error::DepthLimit);
    let source = "x".repeat(10000);
    assert_eq!(parse("width", &source).unwrap_err(), Error::Invalid);
    for seed in 0usize..2000 {
        let alphabet = ["var(", "[", "!", "}", "1px", ",", "'", "calc(", "-", "/**/", "0", "\\"];
        let mut state = seed + 1;
        let mut s = String::new();
        for _ in 0..12 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            s.push_str(alphabet[state % alphabet.len()]);
        }
        let _ = parse("margin", &s);
    }
}

#[path = "support/property_fixtures.rs"]
mod fixtures;
#[test]
fn independent_pinned_property_grammar_expansion_and_numeric_ports() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/property-fixtures"));
    if let Ok(path) = std::env::var("OLO_PROPERTY_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("error" | "mismatch" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 1109);
    assert_eq!(report["counts"]["passed"], 821);
    assert_eq!(report["counts"]["unsupported"], 288);
}

#[test]
fn local_clip_urls_are_decoded_bounded_and_share_thin_storage() {
    for (source, expected) in [("url(#Clip)", "#Clip"), ("URL('#:R1:')", "#:R1:"), (r"url(#\41 )", "#A"), ("url(\"#a%20b\")", "#a%20b"), ("url('#')", "#"), ("url(#open", "#open")] {
        let a = values("clip-path", source);
        let Value::LocalUrl(url) = &a[0].value else { panic!("{source}: {:?}", a[0].value) };
        assert_eq!(url.as_str(), expected);
        let copy = url.clone();
        assert_eq!(url.as_str().as_ptr(), copy.as_str().as_ptr());
        assert_eq!(url.reserved_bytes(), 2 * size_of::<usize>() + expected.len());
        assert_eq!(size_of_val(url), size_of::<usize>());
    }
    assert!(matches!(values("clip-path", "none")[0].value, Value::Keyword(Keyword::None)));
    for source in ["url('#x') junk", "url('#x' '#y')", "url() none", "12px", "url(#x) url(#y)", "banana", "banana(0)"] {
        assert!(matches!(parse("clip-path", source), Err(Error::Invalid)), "{source}");
    }
    for source in ["url(https://example.test/a.svg#clip)", "url('a.svg#clip')", "circle(50%)", "border-box"] {
        assert!(matches!(parse("clip-path", source), Err(Error::Unsupported(_))), "{source}");
    }
    let input = syntax("url(#bounded)");
    assert!(matches!(Declaration::parse("clip-path", input.components(), false, p::Limits { bytes: 2, ..Default::default() }, None), Err(Error::Limit)));
    assert!(matches!(Declaration::parse("clip-path", input.components(), false, Default::default(), Some(&AtomicBool::new(true))), Err(Error::Cancelled)));
}

#[test]
fn inset_shapes_expand_quads_and_reject_invalid_round_syntax() {
    for (source, offsets, radii) in [
        ("inset(3px)", [3., 3., 3., 3.], None),
        ("inset(1px 2px)", [1., 2., 1., 2.], None),
        ("inset(1px 2px 3px)", [1., 2., 3., 2.], None),
        ("inset(-1px 2px 3px 4px round 10% / 5px 6px 7px)", [-1., 2., 3., 4.], Some([20., 20., 20., 20., 5., 6., 7., 6.])),
        ("inset(0 round calc(-2px) 3px)", [0.; 4], Some([0., 3., 0., 3., 0., 3., 0., 3.])),
    ] {
        let a = values("clip-path", source);
        let Value::Inset(s) = &a[0].value else { panic!("{source}") };
        let resolve = |n: &p::Numeric| n.resolve(&NumericHost, &mut Default::default(), Default::default(), None).unwrap();
        assert_eq!(s.offsets().into_iter().map(resolve).collect::<Vec<_>>(), offsets);
        assert_eq!(s.radii().map(|r| r.into_iter().map(resolve).collect::<Vec<_>>()), radii.map(Vec::from));
        assert_eq!(size_of_val(s), size_of::<usize>());
        assert_eq!(s.offsets()[0] as *const _, s.clone().offsets()[0] as *const _);
    }
    for source in [
        "inset()",
        "inset(round 2px)",
        "inset(0 round)",
        "inset(0 / 1px)",
        "inset(0 round / 1px)",
        "inset(0 round 1px /)",
        "inset(0 round 1px / 2px / 3px)",
        "inset(0 round 1px round 2px)",
        "inset(0 round -1px)",
        "inset(0 round -2%)",
        "inset(0 0 0 0 0)",
        "inset(0 round 0 0 0 0 0)",
        "inset(0,0)",
        "inset(auto)",
        "inset(2)",
        "inset(1deg)",
        "inset(0) none",
        "none border-box",
        "url(#a) content-box",
        "inset(0) border-box padding-box",
    ] {
        assert!(matches!(parse("clip-path", source), Err(Error::Invalid)), "{source}: {:?}", parse("clip-path", source));
    }
    for b in [p::GeometryBox::Border, p::GeometryBox::Padding, p::GeometryBox::Content, p::GeometryBox::Margin, p::GeometryBox::Fill, p::GeometryBox::Stroke, p::GeometryBox::View] {
        for text in [format!("{} inset(0)", b.as_str()), format!("inset(0) {}", b.as_str())] {
            let a = values("clip-path", &text);
            let Value::Inset(s) = &a[0].value else { panic!() };
            assert_eq!(s.reference_box(), b);
        }
    }
    let a = values("clip-path", "inset(0 round 6px)");
    let Value::Inset(s) = &a[0].value else { panic!() };
    let radii = s.radii().unwrap();
    assert!(radii.iter().all(|n| std::ptr::eq(*n, radii[0])));
    assert!(s.offsets().iter().all(|n| std::ptr::eq(*n, s.offsets()[0])));
    assert!(s.reserved_bytes() < 3 * size_of::<p::Numeric>() + 32, "two unique scalars, not twelve expanded values");
    let a = values("clip-path", "inset(calc(10% + 2em))");
    assert!(a[0].value.dependencies().color.math.intersects(math::Dependencies::PERCENTAGE));
    assert!(a[0].value.dependencies().color.math.contains(math::RelativeUnit::Em));
    let input = syntax("inset(calc(10% + 2em) round 1px / 2px)");
    for limits in [p::Limits { bytes: 1, ..Default::default() }, p::Limits { nodes: 2, ..Default::default() }, p::Limits { work: 2, ..Default::default() }] {
        assert!(matches!(Declaration::parse("clip-path", input.components(), false, limits, None), Err(Error::Limit)));
    }
    assert!(matches!(Declaration::parse("clip-path", input.components(), false, Default::default(), Some(&AtomicBool::new(true))), Err(Error::Cancelled)));
}
#[test]
fn clip_takes_rect_with_or_without_commas_and_auto_sides() {
    // CSS 2.1 §11.1.2 as browsers take it: what hides "skip to content"
    // links (`clip: rect(0 0 0 0)`).
    for text in ["rect(0 0 0 0)", "rect(0px, 0px, 0px, 0px)", "rect(1px auto 3px 4px)", "auto"] {
        assert_eq!(values("clip", text)[0].property, P::Clip, "{text}");
    }
    for text in ["rect(0 0 0)", "rect(0, 0 0 0)", "rect(0 0 0 0 0)", "rect(10% 0 0 0)", "inset(0)", "circle(1px)"] {
        assert!(parse("clip", text).is_err(), "{text}");
    }
}
