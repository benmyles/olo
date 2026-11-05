#[path = "support/selector_fixtures.rs"]
mod fixtures;
use olo_web::{
    css::{
        self, Syntax,
        selectors::{self, AnB, CompileLimits, CompileOptions, Content, DomTree, ElementName, Error, MatchLimits, MatchOptions, PseudoElement, Selectors, Specificity, State, StateSource, Tree},
    },
    dom::{Document, Namespace, NodeId},
    html,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn compiled(s: &str) -> Selectors {
    let css = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
    Selectors::compile(css.components(), Default::default(), Default::default(), None).unwrap()
}
fn syntax(s: &str) -> Result<Selectors, Error> {
    let css = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
    Selectors::compile(css.components(), Default::default(), Default::default(), None)
}
fn ids(document: &Document, selectors: &Selectors, options: MatchOptions<'_, NodeId>) -> Vec<String> {
    let mut result = Vec::new();
    selectors
        .query(&DomTree::new(document), document.root(), false, options, Default::default(), None, |n, _| {
            result.push(document.attribute_ns(n, None, "id")?.unwrap_or("-").to_owned());
            Ok(())
        })
        .unwrap();
    result
}
#[test]
fn independent_selector_queries() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/selector-fixtures"));
    if let Ok(path) = std::env::var("OLO_SELECTOR_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert_eq!(report["total"], 463);
    let failures: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| matches!(r["outcome"].as_str(), Some("mismatch" | "error" | "panic"))).collect();
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(report["counts"]["passed"], 432);
    assert_eq!(report["counts"]["unsupported"], 29);
    assert_eq!(report["counts"]["updated_specification"], 2);
}
#[test]
fn independent_an_plus_b_microsyntax() {
    let cases: serde_json::Value = serde_json::from_str(include_str!("selector-fixtures/An+B.json")).unwrap();
    let (pairs, remainder) = cases.as_array().unwrap().as_chunks::<2>();
    assert!(remainder.is_empty());
    for pair in pairs {
        let css = Syntax::parse(Arc::from(pair[0].as_str().unwrap()), css::Limits::default(), None).unwrap();
        let actual = AnB::parse(css.components()).ok();
        let expected = pair[1].as_array().map(|v| AnB { a: v[0].as_i64().unwrap(), b: v[1].as_i64().unwrap() });
        assert_eq!(actual, expected, "{}", pair[0]);
    }
    for a in -12..=12 {
        for b in -20..=20 {
            let anb = AnB { a, b };
            for index in 0..=80 {
                let expected = index > 0 && (0..=120).any(|n| a * n + b == index);
                assert_eq!(anb.matches(index as u64), expected, "{anb:?}, {index}");
            }
        }
    }
    assert!(AnB { a: i64::MIN, b: i64::MAX }.matches(i64::MAX as u64));
    assert!(!AnB { a: 0, b: i64::MIN }.matches(u64::MAX));
}
#[test]
fn specificity_forgiving_lists_and_namespace_subject_rules() {
    let cases = [
        ("#a.a[type]:hover > p::before", Specificity { ids: 1, classes: 3, types: 2 }),
        (":is(.a,#b)", Specificity { ids: 1, classes: 0, types: 0 }),
        (":where(#a) p", Specificity { ids: 0, classes: 0, types: 1 }),
        (":not(.x,div#id)", Specificity { ids: 1, classes: 0, types: 1 }),
        (":nth-child(2n of .x,#a)", Specificity { ids: 1, classes: 1, types: 0 }),
        (":has(> a.x,+ #id)", Specificity { ids: 1, classes: 0, types: 0 }),
        (":is(#id::before,.x)", Specificity { ids: 0, classes: 1, types: 0 }),
        (":is()", Specificity::default()),
    ];
    for (s, expected) in cases {
        assert_eq!(compiled(s).specificities().collect::<Vec<_>>(), vec![expected], "{s}");
    }
    for s in [
        ":not()",
        ":has()",
        ":has(:has(p))",
        ":has(:is(p):has(a))",
        ":not(::before)",
        ":has(::before)",
        "#123",
        ":nth-child(2n of)",
        ":nth-of-type(2n of p)",
        "div,",
        ":is(p), .",
        "p::before.x",
        "p :hover :",
        "p::unknown",
    ] {
        assert!(matches!(syntax(s), Err(Error::Invalid)), "{s}");
    }
    let s = Syntax::parse(Arc::from(":is(.x)"), css::Limits::default(), None).unwrap();
    assert_eq!(Selectors::compile(s.components(), Default::default(), CompileLimits { tests: 0, ..Default::default() }, None).unwrap_err(), Error::CompileLimit);
    assert!(syntax(":is(.x,::part(a))").is_ok()); // pseudo-elements are invalid within the forgiving list
    assert!(matches!(syntax(":is(.x,&)"), Err(Error::Unsupported(_))));
    let mut doc = Document::new(Arc::from(""), Default::default()).unwrap();
    let root = doc.create_element("svg", Namespace::Svg).unwrap();
    doc.append_child(doc.root(), root).unwrap();
    doc.set_attribute(root, "class", "x").unwrap();
    let compile = |s: &str| {
        let syntax = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
        Selectors::compile(syntax.components(), CompileOptions { default_namespace: Some("http://www.w3.org/1999/xhtml"), ..Default::default() }, Default::default(), None).unwrap()
    };
    let tree = DomTree::new(&doc);
    for (s, expected) in [(".x", false), ("*|*:is(.x)", true), ("*|*:is(*.x)", false), ("*|*:not(.x)", false), ("*|*:not(*.x)", true)] {
        assert_eq!(compile(s).matches(&tree, root, Default::default(), Default::default(), None).unwrap().is_some(), expected, "{s}");
    }
}
#[test]
fn dom_adapter_queries_mutation_scope_templates_and_stale_handles() {
    let mut doc = html::parse(
        Arc::from(
            "<!doctype html><body><section id='root' class='a'><div class='a' id='inner'><span id='x' class='b'></span></div></section><p id='tail'></p><template id='t'><i id='inert'></i></template>",
        ),
        Default::default(),
        None,
    )
    .unwrap()
    .document;
    assert_eq!(ids(&doc, &compiled(".a > .a .b, section:has(> div span)"), Default::default()), ["root", "x"]);
    assert_eq!(ids(&doc, &compiled("template > i, #inert"), Default::default()), Vec::<String>::new());
    let root = doc.children(doc.root()).unwrap().find(|&n| doc.name(n).unwrap() == Some("html")).unwrap();
    let tree = DomTree::new(&doc);
    let mut nodes = Vec::new();
    compiled("#root")
        .query(&tree, doc.root(), false, Default::default(), Default::default(), None, |n, _| {
            nodes.push(n);
            Ok(())
        })
        .unwrap();
    let section = nodes[0];
    let mut scoped = Vec::new();
    compiled(":scope > .a")
        .query(&tree, section, false, MatchOptions { scope: Some(section), ..Default::default() }, Default::default(), None, |n, _| {
            scoped.push(doc.attribute(n, "id")?.unwrap().to_owned());
            Ok(())
        })
        .unwrap();
    assert_eq!(scoped, ["inner"]);
    let selector = compiled(".updated");
    assert!(selector.matches(&tree, section, Default::default(), Default::default(), None).unwrap().is_none());
    doc.set_attribute(section, "class", "updated").unwrap();
    assert!(selector.matches(&DomTree::new(&doc), section, Default::default(), Default::default(), None).unwrap().is_some());
    let other = Document::new(Arc::from(""), Default::default()).unwrap();
    assert_eq!(compiled("*").matches(&DomTree::new(&doc), other.root(), Default::default(), Default::default(), None), Err(Error::Dom(olo_web::dom::Error::WrongDocument)));
    doc.destroy_subtree(section).unwrap();
    assert_eq!(selector.matches(&DomTree::new(&doc), section, Default::default(), Default::default(), None), Err(Error::Dom(olo_web::dom::Error::StaleNode)));
    assert_eq!(
        compiled(":scope").matches(&DomTree::new(&doc), root, MatchOptions { scope: Some(section), ..Default::default() }, Default::default(), None),
        Err(Error::Dom(olo_web::dom::Error::StaleNode))
    );
    let fragment = doc.create_fragment().unwrap();
    let child = doc.create_element("p", Namespace::Html).unwrap();
    doc.append_child(fragment, child).unwrap();
    let mut found = Vec::new();
    compiled(":scope > p")
        .query(&DomTree::new(&doc), fragment, false, MatchOptions { scope: Some(fragment), ..Default::default() }, Default::default(), None, |n, _| {
            found.push(n);
            Ok(())
        })
        .unwrap();
    assert_eq!(found, [child]);
}
#[test]
fn html_case_rules_quirks_attributes_and_utf8_boundaries() {
    let doc = html::parse(
        Arc::from("<!doctype html><div id='AbC' class='FoO' TYPE='TeXT' data-x='é-abc' data-y='AbAbAbC' title='hello🙂 WORLD'><svg id='s' viewBox='x' xlink:href='é'></svg>"),
        Default::default(),
        None,
    )
    .unwrap()
    .document;
    for s in ["DIV[TYPE=TEXT]", "[data-x|='é']", "[data-y*='ABABC' i]", "[title$='world' i]", "[title*='🙂 w' i]", "#s[viewBox]", "[xlink|href='é']"] {
        let result = if s.starts_with("[xlink") {
            let src = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
            Selectors::compile(src.components(), CompileOptions { namespaces: &[("xlink", "http://www.w3.org/1999/xlink")], ..Default::default() }, Default::default(), None).unwrap()
        } else {
            compiled(s)
        };
        assert_eq!(ids(&doc, &result, Default::default()).len(), 1, "{s}");
    }
    for s in ["#abc", ".foo", "[data-x|=a]", "[data-x^=a]", "[data-x$='é-a']", "[data-y*='ABABC' s]", "svg[viewbox]"] {
        assert!(ids(&doc, &compiled(s), Default::default()).is_empty(), "{s}");
    }
    for s in ["#abc", ".foo"] {
        assert_eq!(ids(&doc, &compiled(s), MatchOptions { quirks: true, ..Default::default() }), ["AbC"]);
    }
    assert!(ids(&doc, &compiled("[id=abc]"), MatchOptions { quirks: true, ..Default::default() }).is_empty());
}
struct States(AtomicBool);
impl StateSource for States {
    fn state(&self, _: NodeId, pseudo: Option<PseudoElement>, state: State) -> selectors::Result<bool> {
        Ok(match state {
            State::AnyLink => true,
            State::Visited => true,
            State::Checked => self.0.load(Ordering::Relaxed),
            State::Hover => pseudo == Some(PseudoElement::Before),
            _ => false,
        })
    }
}
#[test]
fn live_host_states_pseudo_targets_and_history_privacy() {
    let mut doc = Document::new(Arc::from(""), Default::default()).unwrap();
    let node = doc.create_element("input", Namespace::Html).unwrap();
    doc.append_child(doc.root(), node).unwrap();
    let states = States(AtomicBool::new(false));
    let tree = DomTree::with_states(&doc, &states);
    let selector = compiled(":checked");
    assert!(selector.matches(&tree, node, Default::default(), Default::default(), None).unwrap().is_none());
    states.0.store(true, Ordering::Relaxed);
    assert!(selector.matches(&tree, node, Default::default(), Default::default(), None).unwrap().is_some());
    assert_eq!(selector.matches(&DomTree::new(&doc), node, Default::default(), Default::default(), None), Err(Error::StateUnavailable));
    assert!(compiled(":visited").matches(&tree, node, Default::default(), Default::default(), None).unwrap().is_none());
    assert!(compiled(":link").matches(&tree, node, Default::default(), Default::default(), None).unwrap().is_some());
    let pseudo = MatchOptions { pseudo: Some(PseudoElement::Before), ..Default::default() };
    assert!(compiled("input::before:hover").matches(&tree, node, pseudo, Default::default(), None).unwrap().is_some());
    assert!(compiled("input:hover::before").matches(&tree, node, pseudo, Default::default(), None).unwrap().is_none());
    assert!(compiled("input").matches(&tree, node, pseudo, Default::default(), None).unwrap().is_none());
    assert!(compiled("input::before").matches(&tree, node, Default::default(), Default::default(), None).unwrap().is_none());
}
struct Chain(usize);
impl Tree for Chain {
    type Node = usize;
    fn content(&self, n: usize) -> selectors::Result<Content<'_>> {
        if n > self.0 { Err(Error::InvalidNode) } else { Ok(Content::Element(ElementName { namespace: "http://www.w3.org/1999/xhtml", local_name: "x" })) }
    }
    fn parent(&self, n: usize) -> selectors::Result<Option<usize>> {
        Ok(n.checked_sub(1))
    }
    fn first_child(&self, n: usize) -> selectors::Result<Option<usize>> {
        Ok((n < self.0).then_some(n + 1))
    }
    fn previous_sibling(&self, _: usize) -> selectors::Result<Option<usize>> {
        Ok(None)
    }
    fn next_sibling(&self, _: usize) -> selectors::Result<Option<usize>> {
        Ok(None)
    }
    fn attributes(&self, _: usize) -> selectors::Result<impl Iterator<Item = selectors::Attribute<'_>>> {
        Ok(std::iter::empty())
    }
}
#[test]
fn deep_chains_limits_cancellation_and_terminal_sink_errors() {
    let selector = compiled(&("x > ".repeat(10000) + "x"));
    assert!(selector.matches(&Chain(10000), 10000, Default::default(), Default::default(), None).unwrap().is_some());
    let input = format!("{}x{}", ":not(".repeat(5000), ")".repeat(5000));
    let source = Syntax::parse(Arc::from(input), css::Limits { depth: 6000, ..Default::default() }, None).unwrap();
    assert_eq!(Selectors::compile(source.components(), Default::default(), CompileLimits { depth: 6000, ..Default::default() }, None).unwrap_err(), Error::DepthLimit);
    let cancelled = AtomicBool::new(true);
    assert_eq!(Selectors::compile(source.components(), Default::default(), Default::default(), Some(&cancelled)).unwrap_err(), Error::Cancelled);
    assert_eq!(compiled("*").matches(&Chain(10), 1, Default::default(), Default::default(), Some(&cancelled)), Err(Error::Cancelled));
    assert_eq!(compiled("x x x").matches(&Chain(10), 10, Default::default(), MatchLimits { work: 2, ..Default::default() }, None), Err(Error::MatchLimit));
    let mut offered = 0;
    assert!(matches!(
        compiled("x").query(&Chain(10), 0, true, Default::default(), MatchLimits { results: 2, ..Default::default() }, None, |_, _| {
            offered += 1;
            Ok(())
        }),
        Err(Error::MatchLimit)
    ));
    assert_eq!(offered, 2);
    let cancelled = AtomicBool::new(false);
    let mut offered = 0;
    assert!(matches!(
        compiled("x").query(&Chain(100), 0, true, Default::default(), Default::default(), Some(&cancelled), |_, _| {
            offered += 1;
            cancelled.store(true, Ordering::Relaxed);
            Ok(())
        }),
        Err(Error::Cancelled)
    ));
    assert_eq!(offered, 1);
    let mut offered = 0;
    assert!(matches!(
        compiled("x").query(&Chain(100), 0, true, Default::default(), Default::default(), None, |_, _| {
            offered += 1;
            Err(Error::Cancelled)
        }),
        Err(Error::Cancelled)
    ));
    assert_eq!(offered, 1);
    assert!(!format!("{:?}", compiled("#private-selector-name[data-value='secret-value']")).contains("secret"));
}

#[test]
fn sibling_indices_are_filtered_typed_bounded_and_not_reused_after_mutation() {
    let mut doc = html::parse(
        Arc::from("<!doctype html><ul><li id='a' class='pick'></li>text<li id='b'></li><p id='p'></p><li id='c' class='pick'></li><p id='q'></p><li id='d' class='pick'></li></ul>"),
        Default::default(),
        None,
    )
    .unwrap()
    .document;
    let cases = [
        ("li:nth-child(2n)", vec!["b", "c", "d"]),
        ("li:nth-child(2n of .pick)", vec!["c"]),
        ("li:nth-last-child(2n of .pick)", vec!["c"]),
        ("li:nth-of-type(2n)", vec!["b", "d"]),
        ("p:nth-last-of-type(2)", vec!["p"]),
        ("li:nth-child(2n of :nth-child(2n))", vec!["c"]),
        ("li:nth-child(2n of :nth-child(2n of .pick))", vec![]),
    ];
    for (s, expected) in cases {
        assert_eq!(ids(&doc, &compiled(s), Default::default()), expected, "{s}");
    }
    let selector = compiled("li:nth-child(2n+1)");
    let mut offered = 0;
    let result = selector.query(&DomTree::new(&doc), doc.root(), false, Default::default(), MatchLimits { index_entries: 2, ..Default::default() }, None, |_, _| {
        offered += 1;
        Ok(())
    });
    if cfg!(feature = "baseline-selector-nth") {
        assert!(result.is_ok());
    } else {
        assert!(matches!(result, Err(Error::MatchLimit)));
        assert_eq!(offered, 0);
    }
    let before = ids(&doc, &selector, Default::default());
    assert_eq!(before, ["a"]);
    let tree = DomTree::new(&doc);
    let mut nodes = Vec::new();
    compiled("#a")
        .query(&tree, doc.root(), false, Default::default(), Default::default(), None, |n, _| {
            nodes.push(n);
            Ok(())
        })
        .unwrap();
    doc.destroy_subtree(nodes[0]).unwrap();
    assert_eq!(ids(&doc, &selector, Default::default()), ["b", "c", "d"]);
    for s in ["n - 9223372036854775808", "n- 9223372036854775808", "n-9223372036854775808"] {
        let src = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
        assert_eq!(AnB::parse(src.components()), Ok(AnB { a: 1, b: i64::MIN }));
    }
}
#[test]
fn malformed_selectors_make_bounded_progress_and_drop_without_panics() {
    let pieces = [
        "a",
        "[",
        "]",
        "(",
        ")",
        "{",
        "}",
        ":is(",
        ":where(",
        ":not(",
        ":has(",
        ":nth-child(",
        ":lang(",
        ":dir(",
        "\\",
        ",",
        "#",
        ".",
        "*",
        "|",
        "~",
        ">",
        "+",
        "=",
        "n-",
        "12px",
        "\"",
        "'",
        "é",
        "🙂",
        "\0",
        " ",
        "\n",
        "/*",
        "*/",
    ];
    let mut seed = 0x624b_a5f0_92dd_3e17u64;
    for _ in 0..10000 {
        let mut source = String::new();
        for _ in 0..40 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            source.push_str(pieces[seed as usize % pieces.len()]);
        }
        let src = Syntax::parse(Arc::from(source), css::Limits::default(), None).unwrap();
        if let Ok(compiled) = Selectors::compile(src.components(), Default::default(), Default::default(), None) {
            let _ = compiled.query(&Chain(10), 0, true, Default::default(), MatchLimits { work: 10000, ..Default::default() }, None, |_, _| Ok(()));
        }
    }
}

#[test]
fn namespace_lookup_and_nested_index_reservations_obey_work_and_memory_limits() {
    let source = Syntax::parse(Arc::from("ns|p"), css::Limits::default(), None).unwrap();
    let bindings = vec![("other", "uri"); 1000];
    assert_eq!(
        Selectors::compile(source.components(), CompileOptions { namespaces: &bindings, ..Default::default() }, CompileLimits { work: 100, ..Default::default() }, None).unwrap_err(),
        Error::CompileLimit
    );
    let doc = html::parse(Arc::from("<!doctype html><ul><li></li><li></li><p></p><li></li><p></p><li></li></ul>"), Default::default(), None).unwrap().document;
    let selector = compiled("li:nth-child(2n of :nth-child(2n))");
    for limit in [11, 12] {
        let result = selector.query(&DomTree::new(&doc), doc.root(), false, Default::default(), MatchLimits { index_entries: limit, ..Default::default() }, None, |_, _| Ok(()));
        if cfg!(feature = "baseline-selector-nth") || limit == 12 {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(matches!(result, Err(Error::MatchLimit)));
        }
    }
}

#[test]
fn featureless_fragment_scope_requires_explicit_eligible_selectors() {
    let mut doc = Document::new(Arc::from(""), Default::default()).unwrap();
    let root = doc.create_fragment().unwrap();
    let p = doc.create_element("p", Namespace::Html).unwrap();
    doc.append_child(root, p).unwrap();
    for (s, expected) in [
        (":scope > p", true),
        (":is(:scope) > p", true),
        (":where(.x,:scope) > p", true),
        (":not(:not(:scope)) > p", true),
        (":not(.x) > p", false),
        (":not(:scope > .x) > p", false),
        (":scope.x > p", false),
        ("* > p", false),
        (":scope:has(>p) > p", true),
        (":has(>p) >p", false),
        (":not(.x:scope) > p", false),
    ] {
        let source = Syntax::parse(Arc::from(s), css::Limits::default(), None).unwrap();
        let selector = Selectors::compile(source.components(), CompileOptions { default_namespace: Some("http://www.w3.org/1999/xhtml"), ..Default::default() }, Default::default(), None).unwrap();
        let mut found = Vec::new();
        selector
            .query(&DomTree::new(&doc), root, false, MatchOptions { scope: Some(root), ..Default::default() }, Default::default(), None, |node, _| {
                found.push(node);
                Ok(())
            })
            .unwrap();
        assert_eq!(!found.is_empty(), expected, "{s}");
    }
}

#[test]
fn insensitive_substring_matching_has_linear_adversarial_work() {
    let mut doc = Document::new(Arc::from(""), Default::default()).unwrap();
    let node = doc.create_element("p", Namespace::Html).unwrap();
    doc.append_child(doc.root(), node).unwrap();
    doc.set_attribute(node, "data-x", format!("{}c", "a".repeat(100000))).unwrap();
    let selector = compiled(&format!("[data-x*=\"{}b\" i]", "a".repeat(1000)));
    let mut offered = 0;
    let progress = selector
        .query(&DomTree::new(&doc), doc.root(), false, Default::default(), MatchLimits { work: 400000, ..Default::default() }, None, |_, _| {
            offered += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(offered, 0);
    assert!(progress.work < 400000);
    assert_eq!(progress.index_entries, 0);
}

#[test]
fn oversized_unknown_pseudo_names_are_charged_before_case_conversion() {
    let source = Syntax::parse(Arc::from(format!(":{}", "A".repeat(100000))), css::Limits::default(), None).unwrap();
    assert_eq!(Selectors::compile(source.components(), Default::default(), CompileLimits { work: 1000, ..Default::default() }, None).unwrap_err(), Error::CompileLimit);
    assert_eq!(Selectors::compile(source.components(), Default::default(), Default::default(), None).unwrap_err(), Error::Invalid);
}
