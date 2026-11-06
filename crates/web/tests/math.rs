#[path = "support/math_fixtures.rs"]
mod fixtures;
use olo_web::css::{
    self, Syntax,
    math::{self, Context, Dependencies, Dimension as D, Error, Limits, NumericType, Options, RelativeUnit as U, Scratch, Value},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn parse(s: &str, options: Options) -> Result<Value, Error> {
    let syntax = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
    Value::parse(syntax.components(), options, Limits::default(), None)
}
fn raw(s: &str) -> f64 {
    parse(s, Options::default()).unwrap().resolve_raw(&math::NoContext, Limits::default(), None).unwrap().value
}
struct Host {
    em: f64,
    width: f64,
}
#[test]
fn container_lengths_can_wait_for_layout_while_font_context_is_frozen() {
    let specified = parse("calc(2em + 10cqw + 20%)", Options { percentage_hint: Some(D::Length), ..Default::default() }).unwrap();
    let computed = specified.computed_retaining(&Host { em: 16., width: 100. }, Dependencies::CONTAINERS.union(Dependencies::PERCENTAGE), Default::default(), None).unwrap();
    assert!(!computed.dependencies().contains(U::Em));
    assert!(computed.dependencies().contains(U::Cqw));
    assert!(computed.dependencies().contains(U::Percentage));
    assert_eq!(computed.resolve_raw(&Host { em: 999., width: 400. }, Default::default(), None).unwrap().value, 152.);
    assert!(computed.computed(&Host { em: 999., width: 400. }, false, Default::default(), None).unwrap().constant().is_some());
    for limits in [Limits { work: 0, ..Default::default() }, Limits { instructions: 0, ..Default::default() }, Limits { stack: 0, ..Default::default() }] {
        assert_eq!(specified.computed_retaining(&Host { em: 16., width: 100. }, Dependencies::CONTAINERS, limits, None).unwrap_err(), Error::Limit);
    }
    assert_eq!(specified.computed_retaining(&math::NoContext, Dependencies::CONTAINERS, Default::default(), Some(&AtomicBool::new(true))).unwrap_err(), Error::Cancelled);
}
#[test]
fn computed_calculations_freeze_units_tree_counts_and_preserve_layout_percentages() {
    let options = Options { percentage_hint: Some(D::Length), ..Default::default() };
    let parent = Host { em: 16., width: 100. };
    let child = Host { em: 40., width: 400. };
    for source in ["calc(50% - 2em)", "clamp(10px, 2em + 10%, 100px)", "calc(1em / 1px * 20%)", "calc(sign(2em - 20px) * 50% + 1px)", "hypot(1em, 10%)", "calc(0% + 1em)"] {
        let specified = parse(source, options).unwrap();
        let computed = specified.computed(&parent, true, Default::default(), None).unwrap();
        assert!(computed.dependencies().contains(U::Percentage), "{source}");
        assert!(!computed.dependencies().contains(U::Em), "{source}");
        let expected = specified.resolve_raw(&Host { em: 16., width: 400. }, Default::default(), None).unwrap().value;
        assert_eq!(computed.resolve_raw(&child, Default::default(), None).unwrap().value, expected, "{source}");
        let frozen = specified.computed(&parent, false, Default::default(), None).unwrap();
        assert!(frozen.constant().is_some());
        assert_eq!(frozen.resolve_raw(&math::NoContext, Default::default(), None).unwrap().value, specified.resolve_raw(&parent, Default::default(), None).unwrap().value);
    }
    for source in ["calc(2em + 3px)", "calc(2em / 1px)", "calc(1 + 2 * 3)", "calc(pow(2em / 1px, 2))"] {
        let v = parse(source, options).unwrap();
        let result = v.computed(&parent, true, Default::default(), None).unwrap();
        assert!(result.constant().is_some(), "{source}");
        assert!(result.dependencies().is_empty());
        assert_eq!(result.resolve_raw(&math::NoContext, Default::default(), None).unwrap(), v.resolve_raw(&parent, Default::default(), None).unwrap());
    }
    struct Tree;
    impl Context for Tree {
        fn tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
            Ok(if kind == math::TreeCount::SiblingIndex { 2 } else { 7 })
        }
    }
    let v = parse("calc(sibling-index() / sibling-count())", Options::default()).unwrap();
    let c = v.computed(&Tree, true, Default::default(), None).unwrap();
    assert_eq!(c.constant().unwrap().value, 2. / 7.);
}
#[test]
fn computed_numeric_failures_are_bounded_and_do_not_change_specified_programs() {
    let v = parse("calc(50% - 2em)", Options { percentage_hint: Some(D::Length), ..Default::default() }).unwrap();
    let instructions = v.memory().instructions;
    assert_eq!(v.computed(&math::NoContext, true, Default::default(), None).unwrap_err(), Error::ContextUnavailable);
    for limits in [Limits { work: 0, ..Default::default() }, Limits { instructions: 0, ..Default::default() }, Limits { stack: 0, ..Default::default() }] {
        assert_eq!(v.computed(&Host { em: 16., width: 100. }, true, limits, None).unwrap_err(), Error::Limit);
    }
    assert_eq!(v.computed(&Host { em: 16., width: 100. }, true, Default::default(), Some(&AtomicBool::new(true))).unwrap_err(), Error::Cancelled);
    assert_eq!(v.memory().instructions, instructions);
    assert!(v.dependencies().contains(U::Em));
    assert_eq!(v.resolve_raw(&Host { em: 20., width: 100. }, Default::default(), None).unwrap().value, 10.);
    let nan = parse("calc(0em / 0px)", Options::default()).unwrap().computed(&Host { em: 16., width: 100. }, true, Default::default(), None).unwrap();
    assert!(nan.constant().unwrap().value.is_nan());
}
impl Context for Host {
    fn factor(&self, unit: U) -> math::Result<f64> {
        match unit {
            U::Em => Ok(self.em),
            U::Rem => Ok(16.),
            U::Percentage | U::Cqw => Ok(self.width / 100.),
            _ => Err(Error::ContextUnavailable),
        }
    }
}
#[test]
fn independent_pinned_numeric_ports() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/numeric-fixtures");
    let report = fixtures::evaluate(&dir);
    if let Ok(path) = std::env::var("OLO_MATH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("error" | "mismatch" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 1586);
    assert_eq!(report["counts"]["passed"], 1572);
    assert_eq!(report["counts"]["unsupported"], 14);
}
#[test]
fn type_algebra_percentage_policy_and_absolute_unit_conversion() {
    assert_eq!(size_of::<NumericType>(), 8);
    assert_eq!(size_of::<Value>(), 16);
    let zero = parse("0em", Options::default()).unwrap();
    assert_eq!(zero.resolve_raw(&Host { em: 16., width: 100. }, Limits::default(), None).unwrap().value, 0.);
    if !cfg!(feature = "baseline-math-folding") {
        assert!(zero.dependencies().is_empty());
        assert!(zero.resolve_raw(&math::NoContext, Limits::default(), None).is_ok());
    }
    assert_eq!(raw("calc(1in / 1px)"), 96.);
    assert!((raw("calc(2.54cm / 1in)") - 1.).abs() < 1e-12);
    assert_eq!(raw("calc(1000ms / 1s)"), 1.);
    assert_eq!(raw("calc(1turn / 90deg)"), 4.);
    assert_eq!(raw("calc(1kHz / 1Hz)"), 1000.);
    assert_eq!(raw("calc(96dpi / 1dppx)"), 1.);
    assert_eq!(raw("calc(2px * 3deg / 1deg)"), 6.);
    assert_eq!(parse("calc(2px * 3deg / 1deg)", Options::default()).unwrap().ty().kind(), Some(D::Length));
    for bad in [
        "calc(0 + 1px)",
        "calc(0 * 1px + 1s)",
        "calc(1px * 1px)",
        "calc(1 / 1px)",
        "calc(0.5 + 50%)",
        "sin(50%)",
        "pow(2px,2)",
        "sqrt(1px * 1px)",
        "calc(hypot(1px * 1px) / 1px)",
        "calc(atan2(1px * 1px, 2px * 2px) / 1deg)",
        "progress(1px * 1px, 2px * 2px, 3px * 3px)",
        "calc(1px+ 2px)",
        "calc(1px +2px)",
        "calc(1px/**/+/**/2px)",
        "calc(-(1 + 2))",
        "pi",
        "(1 + 2)",
    ] {
        assert_eq!(parse(bad, Options::default()).unwrap_err(), Error::Invalid, "{bad}");
    }
    let v = parse("calc(10% / 20%)", Options::default()).unwrap();
    assert_eq!(v.ty().kind(), Some(D::Number));
    assert_eq!(v.ty().percentage_hint(), Some(D::Percentage));
    assert_eq!(v.resolve_raw(&math::NoContext, Limits::default(), None).unwrap().value, 0.5);
    assert_eq!(parse("calc(10% / 20%)", Options { allow_percentage: false, ..Default::default() }).unwrap_err(), Error::Invalid);
}
#[test]
fn progress_and_tree_counting_use_explicit_live_context() {
    assert_eq!(raw("progress(11,1,6)"), 1.);
    assert_eq!(raw("progress(no-clamp 11,1,6)"), 2.);
    assert_eq!(raw("progress(1,0,0)"), 0.);
    assert_eq!(raw("progress(no-clamp 1,0,0)"), f64::INFINITY);
    assert_eq!(raw("progress(no-clamp -1,0,0)"), f64::NEG_INFINITY);
    assert_eq!(raw("progress(no-clamp 0,0,0)"), 0.);
    assert!(raw("progress(NaN,0,0)").is_nan());
    struct Tree(std::cell::Cell<u32>);
    impl Context for Tree {
        fn tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
            Ok(self.0.get() + u32::from(kind == math::TreeCount::SiblingCount) * 2)
        }
    }
    let tree = Tree(std::cell::Cell::new(1));
    let v = parse("calc(sibling-index() / sibling-count())", Options::default()).unwrap();
    assert!(v.dependencies().tree_count(math::TreeCount::SiblingIndex));
    assert!(v.dependencies().tree_count(math::TreeCount::SiblingCount));
    assert_eq!(v.resolve_raw(&math::NoContext, Limits::default(), None), Err(Error::ContextUnavailable));
    assert_eq!(v.resolve_raw(&tree, Limits::default(), None).unwrap().value, 1. / 3.);
    tree.0.set(4);
    assert_eq!(v.resolve_raw(&tree, Limits::default(), None).unwrap().value, 4. / 6.);
    assert_eq!(parse("sibling-index(1)", Options::default()).unwrap_err(), Error::Invalid);
    let options = Options { percentage_hint: Some(D::Length), ..Default::default() };
    let v = parse("progress(50%,0px,100px)", options).unwrap();
    assert_eq!(v.resolve_raw(&Host { em: 16., width: 100. }, Limits::default(), None).unwrap().value, 0.5);
    assert_eq!(v.resolve_raw(&Host { em: 16., width: -100. }, Limits::default(), None).unwrap().value, 0.);
}
#[test]
fn relative_units_are_deferred_and_dependency_bits_follow_the_expression() {
    let options = Options { percentage_hint: Some(D::Length), ..Default::default() };
    let source: Arc<str> = Arc::from("clamp(10px, 1em + 10%, 500px)");
    let syntax = Syntax::parse(source.clone(), css::Limits::default(), None).unwrap();
    let value = Value::parse(syntax.components(), options, Limits::default(), None).unwrap();
    drop(syntax);
    assert_eq!(Arc::strong_count(&source), 1);
    drop(source);
    let dependencies = value.dependencies();
    assert!(dependencies.contains(U::Em));
    assert!(dependencies.contains(U::Percentage));
    assert!(!dependencies.contains(U::Rem));
    assert_eq!(value.resolve_raw(&math::NoContext, Limits::default(), None), Err(Error::ContextUnavailable));
    let mut scratch = Scratch::default();
    assert_eq!(value.resolve_with(&Host { em: 16., width: 100. }, &mut scratch, Limits::default(), None).unwrap().value, 26.);
    assert_eq!(value.resolve_with(&Host { em: 20., width: 200. }, &mut scratch, Limits::default(), None).unwrap().value, 40.);
    let sign = parse("sign(10%)", options).unwrap();
    assert_eq!(sign.resolve_raw(&Host { em: 16., width: -100. }, Limits::default(), None).unwrap().value, -1.);
    let unavailable = Host { em: f64::INFINITY, width: 100. };
    assert_eq!(value.resolve_raw(&unavailable, Limits::default(), None), Err(Error::ContextUnavailable));
    let negative_zero = Host { em: -0., width: 100. };
    assert_eq!(value.resolve_raw(&negative_zero, Limits::default(), None), Err(Error::ContextUnavailable));
    let percentage_zero = parse("sign(10%)", options).unwrap().resolve_raw(&Host { em: 16., width: -0. }, Limits::default(), None).unwrap();
    assert_eq!(percentage_zero.value, 0.);
    assert!(percentage_zero.value.is_sign_negative());
    assert!(Dependencies::default().is_empty());
}
#[test]
fn ieee_values_stay_inside_calculations_until_materialized() {
    assert_eq!(raw("calc(1 / (-5 * 0))"), f64::NEG_INFINITY);
    assert_eq!(raw("calc(1 / calc(-5 * 0))"), f64::NEG_INFINITY);
    assert!(raw("calc(0 / 0)").is_nan());
    assert!(raw("pow(1, infinity)").is_nan());
    assert!(raw("pow(NaN, 0)").is_nan());
    assert!(raw("hypot(infinity, NaN)").is_nan());
    assert!(raw("log(2, 1)").is_nan());
    assert!(raw("log(2, 0)").is_nan());
    assert_eq!(raw("calc(1 / log(1, 0.5))"), f64::INFINITY);
    for expression in ["calc(-5 * 0)", "min(0, -0)", "mod(1, -1)", "round(nearest,-1,infinity)"] {
        let v = parse(expression, Options::default()).unwrap().resolve_raw(&math::NoContext, Limits::default(), None).unwrap();
        assert_eq!(v.value, 0.);
        assert!(v.value.is_sign_negative(), "{expression}");
        assert!(!v.materialize(-1e9, 1e9).value.is_sign_negative());
    }
    let nan = math::Quantity { value: f64::NAN, ty: NumericType::dimension(D::Number) };
    assert_eq!(nan.materialize(0., 1.).value, 0.);
    assert_eq!(raw("round(-15,10)"), -10.);
    assert_eq!(raw("round(15,10)"), 20.);
    assert_eq!(raw("mod(-18,5)"), 2.);
    assert_eq!(raw("rem(-18,5)"), -3.);
    assert_eq!(raw("clamp(100,20,50)"), 100.);
    assert_eq!(raw("clamp(none,20,none)"), 20.);
}
#[test]
fn limits_cancellation_depth_flat_storage_and_private_debug() {
    let syntax = Syntax::parse(Arc::from("calc(1em + 2px)"), css::Limits::default(), None).unwrap();
    let cancelled = AtomicBool::new(true);
    assert_eq!(Value::parse(syntax.components(), Options::default(), Limits::default(), Some(&cancelled)).unwrap_err(), Error::Cancelled);
    assert_eq!(Value::parse(syntax.components(), Options::default(), Limits { work: 0, ..Default::default() }, None).unwrap_err(), Error::Limit);
    assert_eq!(Value::parse(syntax.components(), Options::default(), Limits { instructions: 0, ..Default::default() }, None).unwrap_err(), Error::Limit);
    let value = Value::parse(syntax.components(), Options::default(), Limits::default(), None).unwrap();
    assert_eq!(value.resolve_raw(&Host { em: 16., width: 100. }, Limits::default(), Some(&cancelled)), Err(Error::Cancelled));
    cancelled.store(false, Ordering::Relaxed);
    assert_eq!(value.resolve_raw(&Host { em: 16., width: 100. }, Limits { work: 0, ..Default::default() }, Some(&cancelled)), Err(Error::Limit));
    assert_eq!(value.resolve_raw(&Host { em: 16., width: 100. }, Limits { stack: 0, ..Default::default() }, None), Err(Error::Limit));
    assert!(!format!("{value:?}").contains("1em"));
    let source = format!("{}1{}", "calc(".repeat(10000), ")".repeat(10000));
    let syntax = Syntax::parse(Arc::from(source), css::Limits { depth: 20000, ..Default::default() }, None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Options::default(), Limits { depth: usize::MAX, work: usize::MAX, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    let too_many = format!("calc({} / {})", vec!["1px"; 128].join(" * "), vec!["1px"; 128].join(" * "));
    assert_eq!(parse(&too_many, Options::default()).unwrap_err(), Error::TypeLimit);
    let chain = format!("calc(1em{})", " + 1px".repeat(5000));
    let value = parse(&chain, Options::default()).unwrap();
    assert_eq!(value.resolve_raw(&Host { em: 16., width: 100. }, Limits::default(), None).unwrap().value, 5016.);
    assert_eq!(value.memory().instruction_size, 16);
}
#[test]
fn deterministic_hostile_numeric_grammar_fuzz() {
    let alphabet = b"calcminmaxround+-*/%0123456789 .eEinftyNaN(pxdeg,s)";
    let mut seed = 0x4f067a21u64;
    for n in 0..10000 {
        let mut s = String::new();
        for _ in 0..n % 121 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            s.push(alphabet[seed as usize % alphabet.len()] as char);
        }
        if let Ok(value) = parse(&s, Options::default()) {
            let _ = value.resolve_raw(&Host { em: 16., width: 100. }, Limits::default(), None);
        }
    }
}
