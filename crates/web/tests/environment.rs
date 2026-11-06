use olo_web::{
    css::{
        self, Syntax, color,
        properties::Property,
        style::{self, FontMetrics, FontRequest, Host, Keyword},
        variables::{self, Binding, Context, Environment, Error, Limits, Resolver, Value},
    },
    font::{Builder, FontSet, GenericPolicy},
    layout, page, paint,
    text::shaping,
};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, atomic::AtomicBool},
};
#[path = "support/paint_host.rs"]
mod paint_host;

fn parse(source: &str) -> variables::Result<Value> {
    let syntax = Syntax::parse(Arc::from(source), Default::default(), None).unwrap();
    Value::compile(syntax.components(), Default::default(), None)
}
struct Variables(Vec<(String, Value)>);
impl Context for Variables {
    fn lookup(&self, name: &str) -> Option<Binding<'_>> {
        self.0.iter().enumerate().find(|(_, (n, _))| n == name).map(|(i, (_, value))| Binding { id: i as u64, value, animation_tainted: false })
    }
}
fn entry(name: &str, indices: &[u32], source: &str) -> (Arc<str>, Box<[u32]>, Value) {
    (Arc::from(name), indices.into(), parse(source).unwrap())
}
#[test]
fn environment_substitution_is_indexed_case_sensitive_lazy_and_bounded() {
    let env =
        Environment::new([entry("test", &[], "11px"), entry("test", &[0, 1], "13px"), entry("empty", &[], ""), entry("1x", &[], "37px"), entry("a b", &[], "41px")], Default::default(), None).unwrap();
    let context = Variables(vec![("--indices".into(), parse("0 1").unwrap()), ("--name".into(), parse("test").unwrap())]);
    let mut resolver = Resolver::new_with_environment(&context, Some(&env), Default::default(), None);
    for (input, expected) in [
        (r"env(\31 x)", "37px"),
        (r"env(a\ b)", "41px"),
        ("env(test, var(--cycle))", "11px"),
        ("ENV(test)", "11px"),
        ("env(Test, 17px)", "17px"),
        ("env(test 0 1)", "13px"),
        ("env(test var(--indices))", "13px"),
        ("env(var(--name))", "11px"),
        ("env(test 0, 19px)", "19px"),
        ("env(test 4294967296, 23px)", "23px"),
        ("env(missing, env(test))", "11px"),
        ("env(missing,)", ""),
        ("env(empty, 29px)", ""),
    ] {
        let actual = resolver.resolve(&parse(input).unwrap(), true).unwrap();
        assert_eq!(actual.value().unwrap().source_spelling().trim(), expected, "{input}");
    }
    let many_indices = format!("env(test {},31px)", "0 ".repeat(100));
    assert_eq!(resolver.resolve(&parse(&many_indices).unwrap(), true).unwrap().value().unwrap().source_spelling().trim(), "31px");
    assert!(resolver.resolve(&parse("env(missing)").unwrap(), true).unwrap().is_invalid());
    assert!(Resolver::new(&context, Default::default(), None).resolve(&parse("env(test)").unwrap(), true).unwrap().is_invalid());
    for input in ["env()", "env(default)", "env(INITIAL, 0px)", "env(test -1)", "env(test .5)", "env(test 1.0)", "env(test 1e0)", "env(test other)"] {
        assert_eq!(parse(input).unwrap_err(), Error::Invalid, "{input}");
    }
    let cancelled = AtomicBool::new(true);
    assert!(matches!(Resolver::new_with_environment(&context, Some(&env), Default::default(), Some(&cancelled)).resolve(&parse("env(test)").unwrap(), true), Err(Error::Cancelled)));
    for limits in [Limits { nodes: 0, ..Default::default() }, Limits { bytes: 0, ..Default::default() }] {
        assert!(Resolver::new_with_environment(&context, Some(&env), limits, None).resolve(&parse("env(test)").unwrap(), true).unwrap().is_invalid());
    }
    assert!(matches!(Resolver::new_with_environment(&context, Some(&env), Limits { work: 0, ..Default::default() }, None).resolve(&parse("env(test)").unwrap(), true), Err(Error::Limit)));
}
#[test]
fn environment_publication_rejects_uncomputed_duplicate_and_excessive_payloads() {
    assert!(matches!(Environment::new([entry("x", &[], "var(--x)")], Default::default(), None), Err(Error::Invalid)));
    assert!(matches!(Environment::new([entry("x", &[], "1px"), entry("x", &[], "2px")], Default::default(), None), Err(Error::Invalid)));
    for name in ["initial", "default", "", "\0"] {
        assert!(matches!(Environment::new([entry(name, &[], "1px")], Default::default(), None), Err(Error::Invalid)), "{name}");
    }
    for limits in [
        Limits { nodes: 0, ..Default::default() },
        Limits { properties: 0, ..Default::default() },
        Limits { work: 0, ..Default::default() },
        Limits { retained_bytes: 0, ..Default::default() },
        Limits { bytes: 0, ..Default::default() },
    ] {
        assert!(matches!(Environment::new([entry("x", &[], "1px")], limits, None), Err(Error::Limit)));
    }
    assert!(matches!(Environment::new([entry("x", &[0; 17], "1px")], Default::default(), None), Err(Error::Limit)));
    assert!(matches!(Environment::new([entry("x", &[], "1px")], Default::default(), Some(&AtomicBool::new(true))), Err(Error::Cancelled)));
    let a = Environment::rectangular().unwrap();
    let b = Environment::rectangular().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    let value = a.get("safe-area-inset-top", &[]).unwrap();
    assert_eq!(value.source_spelling(), "0px");
    assert!(a.get("safe-area-inset-top", &[0]).is_none());
    assert!(a.get("viewport-segment-width", &[0, 0]).is_none());
    assert!(a.reserved_bytes() < 2048);
}

struct EnvironmentHost {
    inner: paint_host::Scaled,
    environment: RefCell<Arc<Environment>>,
    revision: Cell<u64>,
    calls: Cell<usize>,
}
impl Host for EnvironmentHost {
    fn environment(&self) -> style::Result<Option<Arc<Environment>>> {
        self.calls.set(self.calls.get() + 1);
        Ok(Some(self.environment.borrow().clone()))
    }
    fn revision(&self) -> u64 {
        self.inner.revision() ^ self.revision.get()
    }
    fn scheme(&self) -> color::Scheme {
        self.inner.scheme()
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        self.inner.system_color(c, s)
    }
    fn font(&self, r: &FontRequest) -> style::Result<FontMetrics> {
        self.inner.font(r)
    }
    fn font_context(&self) -> u64 {
        self.inner.font_context()
    }
    fn font_size(&self, k: Keyword, p: f64) -> style::Result<f64> {
        self.inner.font_size(k, p)
    }
    fn device_scale(&self) -> f64 {
        self.inner.device_scale()
    }
}
fn padding(page: &page::Page) -> f64 {
    page.scene.layout().cache().tree().styles().iter().find(|(node, _)| page.document.attribute(*node, "id").unwrap() == Some("probe")).unwrap().1.value(Property::PaddingBottom).scalar().unwrap()
}
#[test]
fn real_style_pipeline_matches_chrome_safe_area_fallbacks_and_cycles_at_five_scales() {
    let reference: serde_json::Value = serde_json::from_str(include_str!("env-fixtures/chrome.json")).unwrap();
    let cases: serde_json::Value = serde_json::from_str(include_str!("env-fixtures/cases.json")).unwrap();
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let mut checked = 0;
    for row in reference["rows"].as_array().unwrap() {
        let scale = row["scale"].as_f64().unwrap();
        let host =
            EnvironmentHost { inner: paint_host::Scaled::new(scope.clone(), scale), environment: RefCell::new(Environment::rectangular().unwrap()), revision: Cell::new(1), calls: Cell::new(0) };
        let case = cases.as_array().unwrap().iter().find(|c| c["id"] == row["id"]).unwrap();
        let source: Arc<str> = Arc::from(format!("<!doctype html><div id=probe style=\"padding-bottom:7px;{}\">probe</div>", case["declaration"].as_str().unwrap()));
        let page = page::load(source, &host, &scope, &mut shaper, &mut painter, layout::Viewport { width: 400., height: 200., scale }, None, |_, _| {}).unwrap();
        let expected: f64 = row["padding"].as_str().unwrap().strip_suffix("px").unwrap().parse().unwrap();
        assert_eq!(padding(&page), expected, "{} at {scale}", row["id"]);
        assert_eq!(host.calls.get(), 1, "one environment snapshot per style generation");
        checked += 1;
    }
    assert_eq!(checked, 115);
    // A new generation recomputes inherited customs and consumers. A stale
    // Defaults must be rejected rather than mixing environment generations.
    let host = EnvironmentHost { inner: paint_host::Scaled::new(scope.clone(), 1.), environment: RefCell::new(Environment::rectangular().unwrap()), revision: Cell::new(1), calls: Cell::new(0) };
    let viewport = layout::Viewport { width: 400., height: 200., scale: 1. };
    let old = style::Defaults::new(&host, Default::default(), None).unwrap();
    let mut page = page::load(
        Arc::from("<!doctype html><div id=probe style='--a:env(safe-area-inset-bottom);padding-bottom:var(--a)'>probe</div>"),
        &host,
        &scope,
        &mut shaper,
        &mut painter,
        viewport,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(padding(&page), 0.);
    *host.environment.borrow_mut() = Arc::new(Environment::new([entry("safe-area-inset-bottom", &[], "12px")], Default::default(), None).unwrap());
    host.revision.set(2);
    let cascade = css::cascade::Builder::new(Default::default(), None).finish(false, None).unwrap();
    assert!(matches!(style::Style::compute(old, &cascade, None, None, &host, Default::default(), Default::default(), None), Err(style::Error::Stale)));
    page.restyle(&host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(padding(&page), 12.);
}
