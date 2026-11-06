#[path = "support/variable_fixtures.rs"]
mod fixtures;
use olo_web::css::{
    self, Kind, Syntax,
    variables::{Binding, Computed, Context, Error, Limits, Resolver, Value, Wide},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
fn parse(input: &str) -> Result<Value, Error> {
    let syntax = Syntax::parse(Arc::from(input), css::Limits::default(), None).unwrap();
    Value::compile(syntax.components(), Limits::default(), None)
}
struct Host {
    entries: BTreeMap<String, (u64, Value, bool)>,
    inherited: BTreeMap<String, Computed>,
}
impl Host {
    fn new(entries: &[(&str, &str)]) -> Self {
        Self { entries: entries.iter().enumerate().map(|(i, (name, source))| (name.to_string(), (i as u64, parse(source).unwrap(), false))).collect(), inherited: BTreeMap::new() }
    }
}
impl Context for Host {
    fn lookup(&self, name: &str) -> Option<Binding<'_>> {
        self.entries.get(name).map(|(id, value, taint)| Binding { id: *id, value, animation_tainted: *taint })
    }
    fn inherited(&self, name: &str) -> Option<Computed> {
        self.inherited.get(name).cloned()
    }
}
fn signature(values: css::Components<'_>) -> Vec<String> {
    let mut result = Vec::new();
    for c in values {
        if c.kind() == Kind::Whitespace {
            continue;
        }
        result.push(format!("{:?}:{:?}:{:?}:{:?}", c.kind(), c.text(), c.number().map(|n| (n.value, n.integer, n.representation)), c.unit()));
        if c.is_container() {
            result.extend(signature(c.children()));
            result.push("close".into());
        }
    }
    result
}
fn equal(value: &Computed, expected: &str) {
    assert!(!value.is_invalid(), "expected {expected}");
    let reference = Syntax::parse(Arc::from(expected), css::Limits::default(), None).unwrap();
    assert_eq!(signature(value.components().unwrap()), signature(reference.components()));
}
#[test]
fn variables_preserve_token_boundaries_and_typed_color_math_consumers() {
    let host = Host::new(&[("--n", "10"), ("--px", "px"), ("--dash", "-"), ("--light", "50%"), ("--offset", "10%")]);
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    for (source, expected) in [("var(--n)px", "10/**/px"), ("var(--dash)-a", "-/**/-a"), ("#var(--px)", "#var(--px)"), ("var(--n)%", "10/**/%"), ("var(--dash)var(--dash)>", "-/**/->")] {
        equal(&resolver.resolve(&parse(source).unwrap(), true).unwrap(), expected);
    }
    let value = resolver.resolve(&parse("rgb(calc(var(--light) + var(--offset)) 0 0)").unwrap(), true).unwrap();
    let color = css::color::Value::parse(value.components().unwrap(), Default::default(), None).unwrap();
    struct Palette;
    impl css::color::Context for Palette {
        fn scheme(&self) -> css::color::Scheme {
            css::color::Scheme::Light
        }
    }
    assert_eq!(color.resolve(&Palette, Default::default(), None).unwrap().srgb8_clipped(), [153, 0, 0, 255]);
    let value = resolver.resolve(&parse("calc(var(--n) * 1px)").unwrap(), true).unwrap();
    assert_eq!(
        css::math::Value::parse(value.components().unwrap(), Default::default(), Default::default(), None).unwrap().resolve_raw(&css::math::NoContext, Default::default(), None).unwrap().value,
        10.
    );
}
#[test]
fn declaration_grammar_dynamic_names_wrappers_and_lazy_fallbacks() {
    let host = Host::new(&[("--x", "red"), ("--name", "--x"), ("--empty", ""), ("--other", "--name")]);
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    for (source, expected) in [
        ("var(--x,env(bad))", "red"),
        ("var(var(--name))", "red"),
        ("var(var(var(--other)))", "red"),
        ("var({--x})", "red"),
        ("var({var(--name)})", "red"),
        ("var(--unset,)", ""),
        ("var(--unset, {blue, green})", "blue,green"),
        ("var({--x,--name},blue)", "blue"),
        ("var(var(--empty),blue)", "blue"),
        ("var(foo(),blue)", "blue"),
        ("var(--x ())", ""),
    ] {
        let actual = resolver.resolve(&parse(source).unwrap(), true).unwrap();
        if source == "var(--x ())" {
            assert!(actual.is_invalid());
        } else {
            equal(&actual, expected);
        }
    }
    assert!(resolver.resolve(&parse("var(--empty,blue)").unwrap(), true).unwrap().components().unwrap().next().is_none());
    assert!(matches!(resolver.resolve(&parse("var(--unset,env(foo))").unwrap(), true), Err(Error::Unsupported(_))));
    for source in ["var()", "var({})", "var(,red)", "var(--x {--y})", "var({--x} --y)", "var(--x,!)", "var(--x,;)", "var(--x,{red} blue)", "]", "f(])", "a!b", "a;b"] {
        assert_eq!(parse(source).unwrap_err(), Error::Invalid, "{source}");
    }
    assert!(parse("f(!;) {} []").is_ok());
}
#[test]
fn cycles_mark_all_participants_and_keep_unrelated_fallbacks_valid() {
    let host = Host::new(&[("--a", "var(--b,red) var(--c,red)"), ("--b", "var(--a,red)"), ("--c", "var(--b,red)"), ("--x", "var(--a,green)"), ("--self", "var(var(--self),red)")]);
    for first in ["--a", "--b", "--c", "--x"] {
        let mut resolver = Resolver::new(&host, Limits::default(), None);
        let _ = resolver.property(first).unwrap();
        for name in ["--a", "--b", "--c", "--self"] {
            assert!(resolver.property(name).unwrap().is_invalid(), "{first}:{name}");
        }
        equal(&resolver.property("--x").unwrap(), "green");
    }
    let host = Host::new(&[("--a", "var(--valid,var(--b))"), ("--b", "var(--valid,var(--a))"), ("--valid", "red")]);
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    equal(&resolver.property("--a").unwrap(), "red");
    equal(&resolver.property("--b").unwrap(), "red");
}
#[test]
fn css_wide_keywords_apply_after_substitution_and_inheritance_is_already_computed() {
    let parent = Host::new(&[("--x", "red")]);
    let mut parent_resolver = Resolver::new(&parent, Limits::default(), None);
    let mut host = Host::new(&[("--x", "var(--empty) inherit"), ("--empty", ""), ("--initial", "var(--empty) initial"), ("--reset", "revert-layer")]);
    host.inherited.insert("--x".into(), parent_resolver.property("--x").unwrap());
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    equal(&resolver.property("--x").unwrap(), "red");
    assert!(resolver.property("--initial").unwrap().is_invalid());
    assert_eq!(resolver.property("--reset").unwrap_err(), Error::RequiresCascade);
    assert_eq!(parse("revert-layer").unwrap().wide(), Some(Wide::RevertLayer));
}
#[test]
fn values_share_compact_batch_storage_release_sources_and_hide_data_in_debug() {
    assert_eq!(size_of::<Value>(), 16);
    let source: Arc<str> = Arc::from("--a: rgb(1 2 3); --b: var(--a); --c: 10px");
    let syntax = Syntax::parse(source.clone(), Default::default(), None).unwrap();
    let declarations = syntax.declarations(None).unwrap();
    let values = Value::compile_batch(declarations.iter().map(|d| d.value()), Limits::default(), None).unwrap();
    drop(declarations);
    drop(syntax);
    assert_eq!(Arc::strong_count(&source), 1);
    drop(source);
    assert_eq!(values.len(), 3);
    assert!(values[1].needs_substitution());
    assert_eq!(values[0].arena_reserved_bytes(), values[1].arena_reserved_bytes());
    assert!(!format!("{:?}", values[1]).contains("--a"));
    assert_eq!(values[2].components().next().unwrap().unit(), Some("px"));
    let a = Syntax::parse(Arc::from("red"), Default::default(), None).unwrap();
    let b = Syntax::parse(Arc::from("blue"), Default::default(), None).unwrap();
    let values = Value::compile_batch([a.components(), b.components()], Limits::default(), None).unwrap();
    assert_eq!(values[0].source_spelling(), "red");
    assert_eq!(values[1].source_spelling(), "blue");
}
#[test]
fn taint_policy_cache_revisions_cancellation_and_resource_failures_are_explicit() {
    let mut host = Host::new(&[("--x", "red"), ("--alias", "var(--x)")]);
    host.entries.get_mut("--x").unwrap().2 = true;
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    assert!(resolver.property("--alias").unwrap().animation_tainted());
    equal(&resolver.resolve(&parse("var(--alias,blue)").unwrap(), false).unwrap(), "blue");
    let cancelled = AtomicBool::new(true);
    let mut resolver = Resolver::new(&host, Limits::default(), Some(&cancelled));
    assert_eq!(resolver.property("--x").unwrap_err(), Error::Cancelled);
    cancelled.store(false, Ordering::Relaxed);
    equal(&resolver.property("--x").unwrap(), "red");
    let mut resolver = Resolver::new(&host, Limits { work: 0, ..Default::default() }, None);
    assert_eq!(resolver.property("--x").unwrap_err(), Error::Limit);
    let mut resolver = Resolver::new(&host, Limits { retained_bytes: 0, ..Default::default() }, None);
    assert_eq!(resolver.property("--x").unwrap_err(), Error::Limit);
    assert_eq!(resolver.stats().cached_properties, 0);
    let mut host = Host::new(&[("--name", "--target"), ("--target", "red"), ("--by-name", "var(var(--name))"), ("--bad", "initial"), ("--fallback", "var(--bad, green)")]);
    host.entries.get_mut("--name").unwrap().2 = true;
    host.entries.get_mut("--bad").unwrap().2 = true;
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    assert!(resolver.property("--by-name").unwrap().animation_tainted());
    assert!(resolver.property("--fallback").unwrap().animation_tainted());
    let mut resolver = Resolver::new(&host, Limits { properties: 1, ..Default::default() }, None);
    assert_eq!(resolver.property("--by-name").unwrap_err(), Error::Limit);
    assert_eq!(resolver.stats().cached_properties, 0);
}

#[test]
fn assembled_spelling_round_trips_boundaries_escapes_and_eof_recovery() {
    let fragments = ["a", "--", "-", "#", "@", "1", "-1", "1%", "1px", "url(foo)", "a()", "(", "*", "%", ".", "+", "/", "<", "!", ">", "#abc", "#123", "é", "\\61", "\\-", "'a\\a b'", "f\\(oo()"];
    for a in fragments {
        for b in fragments {
            let text = format!("f({a}/**/{b})");
            let original = Syntax::parse(Arc::from(text.as_str()), Default::default(), None).unwrap();
            if let Ok(value) = Value::compile(original.components(), Limits::default(), None) {
                let reparsed = Syntax::parse(Arc::from(value.source_spelling()), Default::default(), None).unwrap();
                assert_eq!(signature(value.components()), signature(reparsed.components()), "{text}: {}", value.source_spelling());
            }
        }
    }
    for source in ["f(</**/!/**/--)", "f(1/**/px)", "f(-/**/-/**/>)", "f\\(oo(var(--x,red))", "'a\\a b", "url(a\\20 b", "f([a"] {
        let value = parse(source).unwrap();
        let reparsed = Syntax::parse(Arc::from(value.source_spelling()), Default::default(), None).unwrap();
        assert_eq!(signature(value.components()), signature(reparsed.components()), "{source}");
    }
}

#[test]
fn cache_observes_revisions_and_rejects_mid_operation_changes() {
    use std::cell::Cell;
    struct Versions {
        values: [Value; 2],
        version: Cell<u64>,
        change_during_lookup: Cell<bool>,
    }
    impl Context for Versions {
        fn lookup(&self, _: &str) -> Option<Binding<'_>> {
            let at = self.version.get() as usize % 2;
            if self.change_during_lookup.get() {
                self.version.set(self.version.get() + 1);
            }
            Some(Binding { id: 0, value: &self.values[at], animation_tainted: false })
        }
        fn revision(&self) -> u64 {
            self.version.get()
        }
    }
    let host = Versions { values: [parse("red").unwrap(), parse("blue").unwrap()], version: Cell::new(0), change_during_lookup: Cell::new(false) };
    let mut resolver = Resolver::new(&host, Limits::default(), None);
    equal(&resolver.property("--x").unwrap(), "red");
    equal(&resolver.property("--x").unwrap(), "red");
    assert_eq!(resolver.stats().cache_hits, 1);
    host.version.set(1);
    equal(&resolver.property("--x").unwrap(), "blue");
    assert_eq!(resolver.stats().cache_hits, 0);
    host.change_during_lookup.set(true);
    assert_eq!(resolver.property("--x").unwrap_err(), Error::ContextChanged);
    assert_eq!(resolver.stats().cached_properties, 0);
}

#[test]
fn expansion_depth_cancellation_and_hostile_values_are_bounded() {
    let nested = format!("{}a{}", "f(".repeat(10000), ")".repeat(10000));
    let syntax = Syntax::parse(Arc::from(nested), css::Limits { depth: 20000, ..Default::default() }, None).unwrap();
    assert_eq!(Value::compile(syntax.components(), Limits { depth: usize::MAX, work: usize::MAX, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    let mut strings = vec!["x".to_owned()];
    for i in 1..24 {
        strings.push(format!("var(--x{}) var(--x{})", i - 1, i - 1));
    }
    let entries: Vec<_> = strings.iter().enumerate().map(|(i, s)| (format!("--x{i}"), s.as_str())).collect();
    let refs: Vec<_> = entries.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let host = Host::new(&refs);
    let mut resolver = Resolver::new(&host, Limits { bytes: 1024, retained_bytes: 1024 * 1024, ..Default::default() }, None);
    assert!(resolver.property("--x23").unwrap().is_invalid());
    equal(&resolver.resolve(&parse("var(--x23, red)").unwrap(), true).unwrap(), "red");
    assert!(resolver.stats().cached_properties <= 24);
    let prefix = "a ".repeat(20);
    let a = format!("{prefix}var(--b,red)");
    let host = Host::new(&[("--a", &a), ("--b", "var(--a,green)"), ("--x", "var(--a,blue)")]);
    let mut resolver = Resolver::new(&host, Limits { bytes: 16, ..Default::default() }, None);
    assert!(resolver.property("--a").unwrap().is_invalid());
    assert!(resolver.property("--b").unwrap().is_invalid());
    equal(&resolver.property("--x").unwrap(), "blue");
    let alphabet = b"varenv--()[]{},!;0123456789 abc\\\"'";
    let mut seed = 0x815f309ba1u64;
    let empty = Host::new(&[]);
    for n in 0..10000 {
        let mut source = String::new();
        for _ in 0..n % 90 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            source.push(alphabet[seed as usize % alphabet.len()] as char);
        }
        if let Ok(value) = parse(&source) {
            let mut resolver = Resolver::new(&empty, Limits::default(), None);
            let _ = resolver.resolve(&value, true);
        }
    }
}

#[test]
fn independent_pinned_variable_grammar_cycles_and_substituted_names() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/variable-fixtures");
    let report = fixtures::evaluate(&dir);
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["total"], 151);
    assert_eq!(report["counts"]["passed"], 119);
    assert_eq!(report["counts"]["unsupported"], 32);
}

#[test]
fn generated_static_reference_graphs_match_independent_cycle_reachability() {
    // With literal fallbacks on every direct edge, exactly the cycle members
    // are invalid. Compare against transitive closure, not resolver state.
    let mut seed = 0x4719ab62cu64;
    for trial in 0..1000 {
        let size = 8;
        let mut edges = vec![vec![false; size]; size];
        for row in &mut edges {
            for edge in row {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *edge = seed % 10 < trial % 4;
            }
        }
        let mut reachable = edges.clone();
        for via in 0..size {
            for a in 0..size {
                for b in 0..size {
                    reachable[a][b] |= reachable[a][via] && reachable[via][b];
                }
            }
        }
        let declarations: Vec<_> = edges
            .iter()
            .enumerate()
            .map(|(a, row)| {
                let references: Vec<_> = row.iter().enumerate().filter(|(_, edge)| **edge).map(|(b, _)| format!("var(--g{b}, ok)")).collect();
                (format!("--g{a}"), if references.is_empty() { "ok".into() } else { references.join(" ") })
            })
            .collect();
        let refs: Vec<_> = declarations.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        let host = Host::new(&refs);
        let mut resolver = Resolver::new(&host, Limits::default(), None);
        let mut order: Vec<_> = (0..size).collect();
        for i in (1..size).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            order.swap(i, seed as usize % (i + 1));
        }
        for at in order {
            assert_eq!(resolver.property(&format!("--g{at}")).unwrap().is_invalid(), reachable[at][at], "trial {trial}, property {at}, edges {edges:?}");
        }
    }
}

#[test]
fn work_budget_counts_cached_whitespace_name_keyword_and_spelling_scans() {
    // Comments split whitespace tokens in the input without adding value data.
    let whitespace = " /**/ ".repeat(300);
    let host = Host::new(&[("--w", &whitespace)]);
    let mut resolver = Resolver::new(&host, Limits { work: 200, ..Default::default() }, None);
    assert_eq!(resolver.property("--w").unwrap_err(), Error::Limit);
    let mut resolver = Resolver::new(&host, Limits { work: 1000, ..Default::default() }, None);
    assert!(!resolver.property("--w").unwrap().is_invalid());
    let consumer = parse("var(var(--w),red) var(var(--w),red) var(var(--w),red) var(var(--w),red)").unwrap();
    assert_eq!(resolver.resolve(&consumer, true).unwrap_err(), Error::Limit);
    let data = "a ".repeat(300);
    let host = Host::new(&[("--large", &data)]);
    let value = parse("var(--large, red)").unwrap();
    let mut resolver = Resolver::new(&host, Limits { bytes: 16, work: 200, ..Default::default() }, None);
    assert_eq!(resolver.resolve(&value, true).unwrap_err(), Error::Limit);
    let mut resolver = Resolver::new(&host, Limits { bytes: 16, work: 2000, ..Default::default() }, None);
    equal(&resolver.resolve(&value, true).unwrap(), "red");
}
