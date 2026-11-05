#[path = "support/color_fixtures.rs"]
mod fixtures;
use olo_web::css::{
    self, Syntax,
    color::{Color, Context, Conversion, Error, Limits, Scheme, Space, SystemColor, Value},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn parse(input: &str) -> Result<Value, Error> {
    let syntax = Syntax::parse(Arc::from(input), css::Limits::default(), None).unwrap();
    Value::parse(syntax.components(), Limits::default(), None)
}
fn absolute(input: &str) -> Color {
    match parse(input).unwrap() {
        Value::Absolute(c) => c,
        _ => panic!("not absolute: {input}"),
    }
}
struct Host(Scheme);
impl Context for Host {
    fn scheme(&self) -> Scheme {
        self.0
    }
    fn current_color(&self) -> Result<Color, Error> {
        Ok(absolute("rebeccapurple"))
    }
    fn system_color(&self, color: SystemColor, scheme: Scheme) -> Result<Color, Error> {
        assert_eq!(color, SystemColor::Canvas);
        Ok(absolute(if scheme == Scheme::Light { "white" } else { "#161616" }))
    }
}
#[test]
fn independent_pinned_grammar_and_conversion_vectors() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/color-fixtures");
    for (report, total, passed, updated, unsupported) in [(fixtures::evaluate(&dir), 8061, 5284, 2760, 17), (fixtures::evaluate_extra(&dir), 4642, 4637, 0, 5)] {
        assert_eq!(report["total"], total);
        let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(report["counts"]["passed"], passed);
        assert_eq!(report["counts"]["updated_specification"].as_u64().unwrap_or(0), updated);
        assert_eq!(report["counts"]["unsupported"], unsupported);
    }
}
#[test]
fn retained_precision_storage_and_all_byte_endpoints() {
    assert_eq!(size_of::<Color>(), if cfg!(feature = "baseline-color-f64") { 40 } else { 20 });
    assert_eq!(size_of::<Value>(), if cfg!(feature = "baseline-color-f64") { 40 } else { 24 });
    for n in 0..=255 {
        let s = format!("#{n:02x}{n:02x}{n:02x}{n:02x}");
        let c = absolute(&s);
        assert_eq!(c.srgb8_clipped(), [n; 4], "{s}");
        assert_eq!(parse(&s).unwrap().expression_reserved_bytes(), 0);
    }
    assert_eq!(absolute("rgb(70% 30% 10% / 50%)").srgb8_clipped(), [179, 77, 26, 128]);
    assert_eq!(absolute("transparent").srgb8_clipped(), [0; 4]);
    assert_eq!(absolute("\\72 ed").srgb8_clipped(), [255, 0, 0, 255]);
}
#[test]
fn syntax_source_is_not_retained_and_context_can_change() {
    let source: Arc<str> = Arc::from("light-dark(currentColor, light-dark(Canvas, #abc))");
    let syntax = Syntax::parse(source.clone(), css::Limits::default(), None).unwrap();
    let value = Value::parse(syntax.components(), Limits::default(), None).unwrap();
    assert_eq!(value.expression_nodes(), 5);
    assert!(value.expression_reserved_bytes() > 0);
    drop(syntax);
    assert_eq!(Arc::strong_count(&source), 1);
    drop(source);
    let cloned = value.clone();
    let resolve = |value: &Value, scheme| value.resolve(&Host(scheme), Limits::default(), None).unwrap().srgb8_clipped();
    assert_eq!(resolve(&cloned, Scheme::Light), [102, 51, 153, 255]);
    assert_eq!(resolve(&value, Scheme::Dark), [170, 187, 204, 255]);
    assert_eq!(resolve(&parse("Canvas").unwrap(), Scheme::Light), [255; 4]);
    assert_eq!(resolve(&parse("Canvas").unwrap(), Scheme::Dark), [22, 22, 22, 255]);
    // Unselected branches need no unavailable host data, but both must be valid.
    struct Empty;
    impl Context for Empty {
        fn scheme(&self) -> Scheme {
            Scheme::Dark
        }
    }
    assert_eq!(parse("light-dark(Canvas, blue)").unwrap().resolve(&Empty, Limits::default(), None).unwrap().srgb8_clipped(), [0, 0, 255, 255]);
    assert_eq!(parse("currentColor").unwrap().resolve(&Empty, Limits::default(), None), Err(Error::ContextUnavailable));
    assert_eq!(parse("light-dark(unknown, blue)").unwrap_err(), Error::Invalid);
}
#[test]
fn color_spaces_missing_components_clamps_and_computed_phase() {
    let c = absolute("oklch(150% -10% -0.25turn / none)");
    assert_eq!(c.space(), Space::Oklch);
    assert_eq!(c.components(), [1., 0., 270.]);
    assert_eq!(c.missing(), 8);
    assert_eq!(c.alpha(), 0.);
    let c = absolute("color(display-p3 none -10% 2 / 150%)");
    assert_eq!(c.space(), Space::DisplayP3);
    assert_eq!(c.missing(), 1);
    assert!((c.components()[1] + 0.1).abs() < 1e-7);
    assert_eq!(c.components()[2], 2.);
    assert_eq!(c.alpha(), 1.);
    assert_eq!(c.convert_absolute(Space::DisplayP3).missing(), 0);
    assert_eq!(absolute("hsl(none 50% 50%)").computed().space(), Space::Hsl);
    assert_eq!(absolute("hsl(120 100% 50%)").space(), Space::Hsl);
    assert_eq!(absolute("hsl(120 100% 50%)").computed().space(), Space::Srgb);
    assert_eq!(absolute("hwb(0 80% 80%)").computed().srgb8_clipped(), [128, 128, 128, 255]);
    let gray = absolute("#808080");
    for (space, missing) in [(Space::Hsl, 1), (Space::Hwb, 1), (Space::Lch, 4), (Space::Oklch, 4)] {
        assert_eq!(gray.convert_absolute(space).missing(), missing, "{space:?}");
    }
    for bad in [
        "",
        "red blue",
        "rgb(1, 2%, 3)",
        "rgb(none,0,0)",
        "color(unknown 0 0 0)",
        "#12345",
        "light-dark(red)",
        "light-dark(from, blue)",
        "imaginary(from red r g b)",
        "lab(2 3 4 / 2deg)",
        "oklch(1 2 30%)",
    ] {
        assert_eq!(parse(bad).unwrap_err(), Error::Invalid, "{bad}");
    }
    for pending in ["var(--color)", "rgb(var(--r) 0 0)", "env(foo)"] {
        assert_eq!(parse(pending).unwrap_err(), Error::RequiresSubstitution, "{pending}");
    }
    for unsupported in ["rgb(from red r g b)", "color-mix(in oklab, red, blue)", "color(--profile 0 0 0)"] {
        assert!(matches!(parse(unsupported), Err(Error::Unsupported(_))), "{unsupported}");
    }
}
#[test]
fn hostile_values_limits_depth_cancellation_and_flat_drop() {
    let cancelled = AtomicBool::new(true);
    let syntax = Syntax::parse(Arc::from("red"), css::Limits::default(), None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits::default(), Some(&cancelled)).unwrap_err(), Error::Cancelled);
    assert_eq!(Value::parse(syntax.components(), Limits { work: 0, ..Default::default() }, None).unwrap_err(), Error::Limit);
    let syntax = Syntax::parse(Arc::from("light-dark(red, blue)"), css::Limits::default(), None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits { nodes: 2, ..Default::default() }, None).unwrap_err(), Error::Limit);
    let value = Value::parse(syntax.components(), Limits::default(), None).unwrap();
    assert_eq!(value.resolve(&Host(Scheme::Dark), Limits::default(), Some(&cancelled)), Err(Error::Cancelled));
    assert_eq!(value.resolve(&Host(Scheme::Dark), Limits { work: 1, ..Default::default() }, None), Err(Error::Limit));
    cancelled.store(false, Ordering::Relaxed);
    assert!(value.resolve(&Host(Scheme::Light), Limits::default(), Some(&cancelled)).is_ok());
    let deep = format!("{}red{}", "light-dark(blue,".repeat(10000), ")".repeat(10000));
    let syntax = Syntax::parse(Arc::from(deep), css::Limits { depth: 20000, ..Default::default() }, None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits { depth: usize::MAX, work: usize::MAX, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    let huge = Syntax::parse(Arc::from("r".repeat(100000)), css::Limits::default(), None).unwrap();
    assert_eq!(Value::parse(huge.components(), Limits { work: 32, ..Default::default() }, None).unwrap_err(), Error::Limit);
}
const SPACES: [Space; 15] = [
    Space::Srgb,
    Space::SrgbLinear,
    Space::DisplayP3,
    Space::DisplayP3Linear,
    Space::A98Rgb,
    Space::ProphotoRgb,
    Space::Rec2020,
    Space::XyzD50,
    Space::XyzD65,
    Space::Lab,
    Space::Lch,
    Space::Oklab,
    Space::Oklch,
    Space::Hsl,
    Space::Hwb,
];
#[test]
fn conversions_preserve_gamut_and_are_finite_for_extremes() {
    let red = absolute("color(display-p3 1 0 0)").convert_absolute(Space::Srgb);
    assert!(red.components()[0] > 1. && red.components()[1] < 0.);
    for space in SPACES {
        let c = absolute("#369c");
        let roundtrip = c.convert_absolute(space).convert_absolute(Space::Srgb);
        for (actual, expected) in roundtrip.components().into_iter().zip(c.components()) {
            assert!((actual - expected).abs() < 2e-5, "{space:?}: {actual} != {expected}");
        }
        assert_eq!(roundtrip.alpha(), c.alpha());
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0., -0., f32::MAX as f64] {
            let input = Color::new(space, [v, -v, v], v, 0);
            for target in SPACES {
                let output = input.convert_absolute(target);
                assert!(output.components().into_iter().all(f64::is_finite), "{space:?} to {target:?}");
                assert!(output.alpha().is_finite());
            }
        }
    }
}
#[test]
fn deterministic_malformed_value_fuzz() {
    let alphabet = b"rgbahslok()%,/#0123456789+-.eE none\\\0";
    let mut seed = 0xd093a5e712u64;
    for n in 0..10000 {
        let mut input = String::new();
        for _ in 0..n % 111 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push(alphabet[seed as usize % alphabet.len()] as char);
        }
        if let Ok(value) = parse(&input)
            && let Ok(c) = value.resolve(&Host(Scheme::Dark), Limits::default(), None)
        {
            for target in SPACES {
                assert!(c.convert_absolute(target).components().into_iter().all(f64::is_finite));
            }
        }
    }
}
#[test]
fn bounded_batch_matches_individual_conversions_and_resumes_after_cancel() {
    for source in SPACES {
        for target in SPACES {
            for len in [0, 1, 2, 3, 4, 5, 7, 8, 15, 63] {
                let mut colors: Vec<_> = (0..len).map(|i| Color::new(source, [i as f64 / 11. - 2., i as f64 / 19., i as f64 / 23.], 0.7, (i % 16) as u8)).collect();
                let expected: Vec<_> = colors.iter().map(|c| c.convert_absolute(target)).collect();
                let mut worker = Conversion::new(&mut colors, target);
                loop {
                    let progress = worker.step(7, None).unwrap();
                    assert_eq!(progress.total, len);
                    if progress.finished() {
                        break;
                    }
                }
                assert_eq!(colors, expected, "{source:?} to {target:?}, length {len}");
            }
        }
    }
    // Mixed spaces must take the scalar fallback without misinterpreting lanes.
    let mut colors: Vec<_> = SPACES.into_iter().map(|s| Color::new(s, [0.5, 0.2, 0.1], 1., 0)).collect();
    let expected: Vec<_> = colors.iter().map(|c| c.convert_absolute(Space::SrgbLinear)).collect();
    let cancelled = AtomicBool::new(false);
    let mut worker = Conversion::new(&mut colors, Space::SrgbLinear);
    assert_eq!(worker.step(0, None), Err(Error::Limit));
    assert_eq!(worker.step(5, Some(&cancelled)).unwrap().converted, 5);
    cancelled.store(true, Ordering::Relaxed);
    assert_eq!(worker.step(5, Some(&cancelled)), Err(Error::Cancelled));
    assert_eq!(worker.progress().converted, 5);
    cancelled.store(false, Ordering::Relaxed);
    assert!(worker.step(usize::MAX, Some(&cancelled)).unwrap().finished());
    assert_eq!(colors, expected);
    let mut large = vec![absolute("red"); 5000];
    let mut worker = Conversion::new(&mut large, Space::Oklab);
    assert_eq!(worker.step(usize::MAX, None).unwrap().converted, 4096);
}

#[test]
fn numeric_color_channels_fold_after_type_checking_and_keep_missing_information() {
    for (source, expected) in [
        ("rgb(calc(100 + 20) 0 0)", "rgb(120 0 0)"),
        ("rgb(calc(50% / 2), 0%, 0%, calc(1 / 2))", "rgb(25%,0%,0%,0.5)"),
        ("oklch(calc(25% * 2) calc(10% / 2) calc(1turn + 30deg) / none)", "oklch(50% 5% 30 / none)"),
        ("color(display-p3 min(.3,.5) hypot(.3,.4) calc(-1 / 2))", "color(display-p3 .3 .5 -.5)"),
        ("hsl(calc(infinity) 100% 50%)", "hsl(0 100% 50%)"),
        ("rgb(calc(NaN) calc(infinity) calc(-infinity) / calc(0 / 0))", "rgb(0 255 0 / 0)"),
        ("lab(calc(infinity) 0 0)", "lab(100 0 0)"),
    ] {
        let value = parse(source).unwrap();
        let color = value.resolve(&Host(Scheme::Light), Limits::default(), None).unwrap();
        assert_eq!(color, absolute(expected).computed(), "{source}");
        if !cfg!(feature = "baseline-math-folding") {
            assert_eq!(value.expression_reserved_bytes(), 0, "{source}");
        }
    }
    for source in [
        "rgb(calc(0.5 + 50%) 0 0)",
        "rgb(calc(1px * 0) 0 0)",
        "rgb(calc(50%), 0, 0)",
        "hsl(calc(1turn + 10) 20% 30%)",
        "hsl(sign(10%) 20% 30%)",
        "oklch(.5 .1 atan2(10%,20%))",
        "color(srgb .1 .2 1deg)",
        "light-dark(red, rgb(calc(1s + 2px) 0 0))",
    ] {
        assert_eq!(parse(source).unwrap_err(), Error::Invalid, "{source}");
    }
    assert_eq!(parse("rgb(var(--x) 0 0)").unwrap_err(), Error::RequiresSubstitution);
    assert!(matches!(parse("rgb(media-progress(width, 0px, 100px) 0 0)"), Err(Error::Unsupported(_))));
}

#[test]
fn contextual_color_math_uses_live_host_dependencies_and_only_selected_branches() {
    use css::math::{self, RelativeUnit as U, TreeCount};
    struct Live {
        scheme: Scheme,
        em: std::cell::Cell<f64>,
        calls: std::cell::Cell<usize>,
    }
    impl Context for Live {
        fn scheme(&self) -> Scheme {
            self.scheme
        }
        fn math_factor(&self, unit: U) -> math::Result<f64> {
            assert_eq!(unit, U::Em);
            self.calls.set(self.calls.get() + 1);
            Ok(self.em.get())
        }
        fn math_tree_count(&self, kind: TreeCount) -> math::Result<u32> {
            Ok(if kind == TreeCount::SiblingIndex { 1 } else { 4 })
        }
    }
    let host = Live { scheme: Scheme::Dark, em: std::cell::Cell::new(16.), calls: std::cell::Cell::new(0) };
    let source: Arc<str> = Arc::from("light-dark(currentColor, oklab(calc(.5 + sign(1em - 10px) * .1) .2 none / calc(sibling-index() / sibling-count())))");
    let syntax = Syntax::parse(source.clone(), css::Limits::default(), None).unwrap();
    let value = Value::parse(syntax.components(), Limits::default(), None).unwrap();
    let deps = value.dependencies();
    assert!(deps.scheme && deps.current_color && !deps.system_colors);
    assert!(deps.math.contains(U::Em));
    assert!(deps.math.tree_count(TreeCount::SiblingIndex) && deps.math.tree_count(TreeCount::SiblingCount));
    drop(syntax);
    assert_eq!(Arc::strong_count(&source), 1);
    drop(source);
    assert!(!format!("{value:?}").contains("sibling"));
    let cloned = value.clone();
    let color = cloned.resolve(&host, Limits::default(), None).unwrap();
    assert!((color.components()[0] - 0.6).abs() < 1e-7);
    assert_eq!(color.alpha(), 0.25);
    assert_eq!(color.missing(), 4);
    assert_eq!(host.calls.get(), 1);
    host.em.set(4.);
    assert!((value.resolve(&host, Limits::default(), None).unwrap().components()[0] - 0.4).abs() < 1e-7);
    let root = parse("rgb(calc(sign(1em) * 255) 0 0)").unwrap();
    assert!(root.dependencies().math.contains(U::Em));
    assert_eq!(root.resolve(&Host(Scheme::Light), Limits::default(), None), Err(Error::ContextUnavailable));
    host.em.set(f64::NAN);
    assert_eq!(root.resolve(&host, Limits::default(), None), Err(Error::ContextUnavailable));
    let lazy = parse("light-dark(rgb(sign(1em) 0 0), blue)").unwrap();
    assert_eq!(lazy.resolve(&Host(Scheme::Dark), Limits::default(), None).unwrap().srgb8_clipped(), [0, 0, 255, 255]);
    assert!(parse("Canvas").unwrap().dependencies().system_colors);
}

#[test]
fn math_color_quotas_cover_all_channels_and_conditional_nodes() {
    let root = parse("color(srgb sign(1em) sign(1em) sign(1em) / sign(1em))").unwrap();
    struct Font;
    impl Context for Font {
        fn scheme(&self) -> Scheme {
            Scheme::Dark
        }
        fn math_factor(&self, _: css::math::RelativeUnit) -> css::math::Result<f64> {
            Ok(16.)
        }
    }
    assert_eq!(root.resolve(&Font, Limits { work: 7, ..Default::default() }, None), Err(Error::Limit));
    assert_eq!(root.resolve(&Font, Limits { work: 8, ..Default::default() }, None).unwrap().srgb8_clipped(), [255; 4]);
    let conditional = parse("light-dark(red, color(srgb sign(1em) sign(1em) sign(1em) / sign(1em)))").unwrap();
    assert_eq!(conditional.resolve(&Font, Limits { work: 9, ..Default::default() }, None), Err(Error::Limit));
    assert!(conditional.resolve(&Font, Limits { work: 10, ..Default::default() }, None).is_ok());
    let cancelled = AtomicBool::new(true);
    assert_eq!(root.resolve(&Font, Limits::default(), Some(&cancelled)), Err(Error::Cancelled));
    cancelled.store(false, Ordering::Relaxed);
    assert!(root.resolve(&Font, Limits::default(), Some(&cancelled)).is_ok());
    let syntax = Syntax::parse(Arc::from("rgb(sign(1em) sign(1em) sign(1em))"), css::Limits::default(), None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits { nodes: 5, ..Default::default() }, None).unwrap_err(), Error::Limit);
    assert_eq!(Value::parse(syntax.components(), Limits { depth: 0, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    assert_eq!(Value::parse(syntax.components(), Limits::default(), Some(&AtomicBool::new(true))).unwrap_err(), Error::Cancelled);
    let nested = format!("rgb({}1{} 0 0)", "calc(".repeat(10000), ")".repeat(10000));
    let syntax = Syntax::parse(Arc::from(nested), css::Limits { depth: 20000, ..Default::default() }, None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits { depth: usize::MAX, work: usize::MAX, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    let inner = format!("rgb({}1{} 0 0)", "calc(".repeat(80), ")".repeat(80));
    let mixed = format!("{}{}{}", "light-dark(red,".repeat(80), inner, ")".repeat(80));
    let syntax = Syntax::parse(Arc::from(mixed), css::Limits { depth: 1000, ..Default::default() }, None).unwrap();
    assert_eq!(Value::parse(syntax.components(), Limits { depth: usize::MAX, work: usize::MAX, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
}
