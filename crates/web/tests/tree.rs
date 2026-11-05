#[path = "support/tree_fixtures.rs"]
mod fixtures;

#[test]
fn wpt_static_tree_construction() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt-tree");
    let report = fixtures::evaluate(&dir);
    assert_eq!(report["total"], 3904, "review corpus updates and their complete denominator");
    eprintln!("WPT static trees: {}; total {}; unsupported cases are NOT passes; diagnostics unchecked", report["counts"], report["total"]);
    if let Ok(path) = std::env::var("OLO_TREE_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let bad: Vec<_> = report["results"].as_array().unwrap().iter().filter(|r| !matches!(r["outcome"].as_str().unwrap(), "passed" | "unsupported")).take(12).collect();
    assert!(bad.is_empty(), "{}", serde_json::to_string_pretty(&bad).unwrap());
    let baseline: serde_json::Value = serde_json::from_str(include_str!("tree-coverage.json")).unwrap();
    for result in report["results"].as_array().unwrap() {
        let key = format!("{}:{}", result["file"].as_str().unwrap(), result["scripting"].as_bool().unwrap());
        if baseline["cases"][key].as_array().is_some_and(|cases| cases.contains(&result["case"])) {
            assert_eq!(result["outcome"], "passed", "previously matched case regressed: {result}");
        }
    }
    if std::env::var_os("OLO_TREE_REQUIRE_COMPLETE").is_some() {
        assert_eq!(report["counts"]["passed"], report["total"], "complete static tree coverage still incomplete; diagnostics checked separately");
    }
}

#[test]
fn parsing_obeys_cancellation_stack_and_source_limits() {
    use olo_web::html::{Error, ParseError, ParseOptions, parse};
    use std::sync::{Arc, atomic::AtomicBool};
    let cancelled = AtomicBool::new(true);
    assert!(matches!(parse(Arc::from("<p>hello"), ParseOptions::default(), Some(&cancelled)), Err(ParseError::Tokenizer(Error::Cancelled))));
    assert!(matches!(parse(Arc::from("<div><div><div>"), ParseOptions { stack_limit: 3, ..ParseOptions::default() }, None), Err(ParseError::StackLimit)));
    assert!(matches!(
        parse(Arc::from("<p>hello"), ParseOptions { tokenizer_limits: olo_web::html::Limits { input_bytes: 3, ..Default::default() }, ..Default::default() }, None),
        Err(ParseError::Tokenizer(Error::InputLimit))
    ));
}

#[test]
fn document_mode_preserves_current_doctype_rules() {
    use olo_web::html::{ParseOptions, QuirksMode, parse};
    use std::sync::Arc;
    for (input, mode) in [
        ("<p>no doctype", QuirksMode::Quirks),
        ("<!doctype html>", QuirksMode::NoQuirks),
        ("<!doctype html SYSTEM 'about:legacy-compat'>", QuirksMode::NoQuirks),
        ("<!doctype html PUBLIC '-//W3C//DTD HTML 4.01 Transitional//EN'>", QuirksMode::Quirks),
        ("<!doctype html PUBLIC '-//W3C//DTD HTML 4.01 Transitional//EN' ''>", QuirksMode::Quirks),
        ("<!doctype html PUBLIC '-//W3C//DTD HTML 4.01 Transitional//EN' 'http://www.w3.org/TR/html4/loose.dtd'>", QuirksMode::LimitedQuirks),
        ("<!doctype html PUBLIC '-//W3C//DTD XHTML 1.0 Transitional//EN'>", QuirksMode::LimitedQuirks),
    ] {
        assert_eq!(parse(Arc::from(input), ParseOptions::default(), None).unwrap().quirks, mode, "{input}");
    }
}

#[test]
fn formatting_limits_include_markers_as_well_as_formatting_elements() {
    use olo_web::html::{ParseError, ParseOptions, parse};
    use std::sync::Arc;
    assert!(matches!(parse(Arc::from("<object><object>"), ParseOptions { formatting_limit: 1, ..Default::default() }, None), Err(ParseError::FormattingLimit)));
}

#[test]
fn malformed_markup_does_not_panic_or_reprocess_forever() {
    use olo_web::html::{ParseError, ParseOptions, parse};
    use std::sync::Arc;
    let tags = [
        "div",
        "p",
        "a",
        "b",
        "i",
        "html",
        "head",
        "body",
        "pre",
        "li",
        "dl",
        "dd",
        "form",
        "button",
        "option",
        "select",
        "optgroup",
        "script",
        "style",
        "title",
        "textarea",
        "frameset",
        "frame",
        "template",
        "svg",
        "math",
        "foreignObject",
        "title",
        "mi",
        "mo",
        "annotation-xml",
        "mglyph",
        "circle",
        "br",
        "table",
        "tbody",
        "thead",
        "tfoot",
        "tr",
        "td",
        "th",
        "caption",
        "colgroup",
        "col",
    ];
    let mut seed = 0x1309_aca1_2077_9bdfu64;
    for _ in 0..10_000 {
        let mut input = String::new();
        for _ in 0..40 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let tag = tags[(seed as usize >> 8) % tags.len()];
            match seed % 5 {
                0 => input.push_str(&format!("<{tag}>")),
                1 => input.push_str(&format!("</{tag}>")),
                2 => input.push_str("\0 &amp; 🙂"),
                3 => input.push_str(" \n\t"),
                _ => input.push_str("<<x/>"),
            }
        }
        if let Err(e) = parse(Arc::from(input.as_str()), ParseOptions::default(), None) {
            assert!(matches!(e, ParseError::Unsupported(_)), "{e}: {input:?}");
        }
        let (name, namespace) = [
            ("div", olo_web::dom::Namespace::Html),
            ("table", olo_web::dom::Namespace::Html),
            ("template", olo_web::dom::Namespace::Html),
            ("path", olo_web::dom::Namespace::Svg),
            ("foreignObject", olo_web::dom::Namespace::Svg),
            ("mtext", olo_web::dom::Namespace::MathMl),
        ][seed as usize % 6];
        if let Err(e) = olo_web::html::parse_fragment(Arc::from(input.as_str()), &olo_web::html::FragmentContext::new(name, namespace), ParseOptions::default(), None) {
            assert!(matches!(e, ParseError::Unsupported(_)), "{e}: {name} {input:?}");
        }
    }
}

#[test]
fn table_text_uses_source_ranges_and_enforces_its_temporary_buffer_quota() {
    use olo_web::{
        dom::{self, NodeType},
        html::{ParseError, ParseOptions, parse},
    };
    use std::sync::Arc;
    let source: Arc<str> = Arc::from(format!("<!doctype html><table>{}</table>", "<".repeat(20_000)));
    let doc = parse(source, ParseOptions { document_limits: dom::Limits { owned_text_bytes: 0, ..Default::default() }, ..Default::default() }, None).unwrap().document;
    assert_eq!(doc.memory().owned_text_bytes, 0);
    let html = doc.children(doc.root()).unwrap().find(|&n| doc.node_type(n).unwrap() == NodeType::Element).unwrap();
    let body = doc.children(html).unwrap().find(|&n| doc.name(n).unwrap() == Some("body")).unwrap();
    let text = doc.first_child(body).unwrap().unwrap();
    assert_eq!(doc.value(text).unwrap().unwrap().len(), 20_000);
    let source: Arc<str> = Arc::from(format!("<!doctype html><table>&amp;{}</table>", "<".repeat(100)));
    assert!(matches!(
        parse(source, ParseOptions { document_limits: dom::Limits { owned_text_bytes: 32, ..Default::default() }, ..Default::default() }, None),
        Err(ParseError::Document(dom::Error::TextLimit))
    ));
}

#[test]
fn foreign_attributes_keep_expanded_identity_through_cloning_and_qualified_mutation() {
    use olo_web::{
        dom::{AttributeNamespace as Ns, Namespace, NodeType},
        html::{ParseOptions, parse},
    };
    use std::sync::Arc;
    let mut document = parse(
        Arc::from("<!doctype html><svg viewBox='0 0 1 1' xlink:href='#icon' xml:lang='en' xmlns='urn:ignored' xmlns:xlink='http://www.w3.org/1999/xlink' xlink:unknown='plain'/>"),
        ParseOptions::default(),
        None,
    )
    .unwrap()
    .document;
    let html = document.children(document.root()).unwrap().find(|&n| document.node_type(n).unwrap() == NodeType::Element).unwrap();
    let body = document.children(html).unwrap().find(|&n| document.name(n).unwrap() == Some("body")).unwrap();
    let svg = document.first_child(body).unwrap().unwrap();
    assert_eq!(document.namespace(svg).unwrap(), Some(Namespace::Svg));
    assert_eq!(document.attribute_ns(svg, Some(Ns::Xlink), "href").unwrap(), Some("#icon"));
    assert_eq!(document.attribute_ns(svg, None, "xlink:href").unwrap(), None);
    assert_eq!(document.attribute_ns(svg, Some(Ns::Xml), "lang").unwrap(), Some("en"));
    assert_eq!(document.attribute_ns(svg, Some(Ns::Xmlns), "xmlns").unwrap(), Some("urn:ignored"));
    assert_eq!(document.attribute_ns(svg, Some(Ns::Xmlns), "xlink").unwrap(), Some(Ns::Xlink.uri()));
    assert_eq!(document.attribute_ns(svg, None, "xlink:unknown").unwrap(), Some("plain"));
    let href = document.attributes_ns(svg).unwrap().find(|a| a.namespace == Some(Ns::Xlink)).unwrap();
    assert_eq!((href.qualified_name, href.prefix, href.local_name), ("xlink:href", Some("xlink"), "href"));
    let cloned = document.clone_subtree(svg).unwrap();
    assert_eq!(document.attributes_ns(svg).unwrap().collect::<Vec<_>>(), document.attributes_ns(cloned).unwrap().collect::<Vec<_>>());
    let revision = document.revision();
    assert!(!document.set_attribute(cloned, "xlink:href", "#icon").unwrap());
    assert_eq!(document.revision(), revision);
    document.set_attribute(cloned, "xlink:href", "#changed").unwrap();
    assert_eq!(document.attribute_ns(cloned, Some(Ns::Xlink), "href").unwrap(), Some("#changed"));
    assert_eq!(document.attribute(svg, "xlink:href").unwrap(), Some("#icon"));
    assert!(document.remove_attribute(cloned, "xlink:href").unwrap());
    assert_eq!(document.attribute_ns(cloned, Some(Ns::Xlink), "href").unwrap(), None);
    document.destroy_subtree(cloned).unwrap();
    document.destroy_subtree(svg).unwrap();
    assert_eq!(document.memory().live_attributes, 0);
}

#[test]
fn unclosed_templates_unwind_beyond_the_old_reprocessing_limit() {
    use olo_web::html::{ParseOptions, parse};
    use std::sync::Arc;
    let input: Arc<str> = Arc::from(format!("<!doctype html>{}text", "<template>".repeat(128)));
    let doc = parse(input, ParseOptions::default(), None).unwrap().document;
    let mut node = doc.children(doc.root()).unwrap().find(|&n| doc.node_type(n).unwrap() == olo_web::dom::NodeType::Element && doc.name(n).unwrap() == Some("html")).unwrap();
    node = doc.first_child(node).unwrap().unwrap(); // head
    for _ in 0..128 {
        node = doc.first_child(node).unwrap().unwrap();
        assert_eq!(doc.name(node).unwrap(), Some("template"));
        assert!(doc.first_child(node).unwrap().is_none());
        node = doc.template_contents(node).unwrap().unwrap();
    }
    assert_eq!(doc.value(doc.first_child(node).unwrap().unwrap()).unwrap(), Some("text"));
}

#[test]
fn fragment_context_inherits_form_ancestry_quirks_and_namespaced_integration_attributes() {
    use olo_web::{
        dom::{Document, Limits, Namespace, NodeType},
        html::{FragmentContext, ParseOptions, QuirksMode, parse_fragment},
    };
    use std::sync::Arc;
    let mut source = Document::new(Arc::from(""), Limits::default()).unwrap();
    let form = source.create_element("form", Namespace::Html).unwrap();
    let context = source.create_element("div", Namespace::Html).unwrap();
    source.append_child(form, context).unwrap();
    let revision = source.revision();
    let metadata = FragmentContext::from_element(&source, context, QuirksMode::Quirks, None).unwrap();
    let parsed = parse_fragment(Arc::from("<form id=ignored><input></form><form id=allowed></form><p>x<table>"), &metadata, ParseOptions::default(), None).unwrap();
    assert_eq!(parsed.quirks, QuirksMode::Quirks);
    let children: Vec<_> = parsed.document.children(parsed.fragment).unwrap().collect();
    assert_eq!(children.iter().map(|&n| parsed.document.name(n).unwrap()).collect::<Vec<_>>(), vec![Some("input"), Some("form"), Some("p")]);
    assert_eq!(parsed.document.attribute(children[1], "id").unwrap(), Some("allowed"));
    assert_eq!(source.revision(), revision);
    assert_eq!(source.first_child(context).unwrap(), None);
    let p = children[2];
    assert!(parsed.document.children(p).unwrap().any(|n| parsed.document.name(n).unwrap() == Some("table")));
    let context = source.create_element("annotation-xml", Namespace::MathMl).unwrap();
    source.set_attribute(context, "encoding", "TeXt/HtMl").unwrap();
    let metadata = FragmentContext::from_element(&source, context, QuirksMode::NoQuirks, None).unwrap();
    let parsed = parse_fragment(Arc::from("<foo></foo><svg><![CDATA[a<b]]></svg>"), &metadata, ParseOptions::default(), None).unwrap();
    let children: Vec<_> = parsed.document.children(parsed.fragment).unwrap().collect();
    assert_eq!(parsed.document.namespace(children[0]).unwrap(), Some(Namespace::Html));
    assert_eq!(parsed.document.namespace(children[1]).unwrap(), Some(Namespace::Svg));
    let text = parsed.document.first_child(children[1]).unwrap().unwrap();
    assert_eq!(parsed.document.value(text).unwrap(), Some("a<b"));
    assert_eq!(parsed.document.node_type(parsed.fragment).unwrap(), NodeType::Fragment);
    assert_eq!(parsed.document.children(parsed.document.root()).unwrap().count(), 0);
}

#[test]
fn fragment_raw_contexts_keep_literal_end_tags_and_context_select_ignores_nested_select_and_input() {
    use olo_web::{
        dom::Namespace,
        html::{FragmentContext, ParseOptions, parse_fragment},
    };
    use std::sync::Arc;
    for (context, text, expected) in [("textarea", "</textarea><b>&amp;", "</textarea><b>&"), ("script", "</script><b>&amp;", "</script><b>&amp;"), ("style", "</style><b>&amp;", "</style><b>&amp;")] {
        let parsed = parse_fragment(Arc::from(text), &FragmentContext::new(context, Namespace::Html), ParseOptions::default(), None).unwrap();
        let child = parsed.document.first_child(parsed.fragment).unwrap().unwrap();
        assert_eq!(parsed.document.value(child).unwrap(), Some(expected));
        assert!(parsed.document.next_sibling(child).unwrap().is_none());
    }
    let parsed = parse_fragment(Arc::from("<select><input><option>value</option>"), &FragmentContext::new("select", Namespace::Html), ParseOptions::default(), None).unwrap();
    let child = parsed.document.first_child(parsed.fragment).unwrap().unwrap();
    assert_eq!(parsed.document.name(child).unwrap(), Some("option"));
    assert!(parsed.document.next_sibling(child).unwrap().is_none());
}

#[test]
fn fragment_context_and_parsing_obey_cancellation_and_limits_without_mutating_the_source() {
    use olo_web::{
        dom::{self, Document, Limits, Namespace},
        html::{self, FragmentContext, ParseError, ParseOptions, QuirksMode, parse_fragment},
    };
    use std::sync::{Arc, atomic::AtomicBool};
    let mut source = Document::new(Arc::from(""), Limits::default()).unwrap();
    let element = source.create_element("annotation-xml", Namespace::MathMl).unwrap();
    source.set_attribute(element, "encoding", "text/html").unwrap();
    source.set_attribute(element, "data-secret", "private-context-value").unwrap();
    let revision = source.revision();
    let cancelled = AtomicBool::new(true);
    assert!(matches!(FragmentContext::from_element(&source, element, QuirksMode::NoQuirks, Some(&cancelled)), Err(dom::Error::Cancelled)));
    let context = FragmentContext::from_element(&source, element, QuirksMode::NoQuirks, None).unwrap();
    assert!(!format!("{context:?}").contains("private-context-value"));
    assert!(matches!(parse_fragment(Arc::from("hello"), &context, ParseOptions::default(), Some(&cancelled)), Err(ParseError::Tokenizer(html::Error::Cancelled))));
    assert!(matches!(
        parse_fragment(Arc::from("hello"), &context, ParseOptions { tokenizer_limits: html::Limits { input_bytes: 3, ..Default::default() }, ..Default::default() }, None),
        Err(ParseError::Tokenizer(html::Error::InputLimit))
    ));
    assert!(matches!(
        parse_fragment(Arc::from("hello"), &context, ParseOptions { document_limits: Limits { attributes: 0, ..Default::default() }, ..Default::default() }, None),
        Err(ParseError::Document(dom::Error::AttributeLimit))
    ));
    assert!(matches!(parse_fragment(Arc::from("hello"), &context, ParseOptions { stack_limit: 0, ..Default::default() }, None), Err(ParseError::StackLimit)));
    assert_eq!(source.revision(), revision);
    assert_eq!(source.first_child(element).unwrap(), None);
}

#[test]
fn inert_fragment_parsing_keeps_declarative_shadow_markup_as_ordinary_template_content() {
    use olo_web::{
        dom::Namespace,
        html::{FragmentContext, ParseOptions, parse_fragment},
    };
    use std::sync::Arc;
    let parsed = parse_fragment(Arc::from("<template shadowrootmode=open><p>content</p></template>"), &FragmentContext::new("div", Namespace::Html), ParseOptions::default(), None).unwrap();
    let template = parsed.document.first_child(parsed.fragment).unwrap().unwrap();
    assert_eq!(parsed.document.attribute(template, "shadowrootmode").unwrap(), Some("open"));
    assert!(parsed.document.first_child(template).unwrap().is_none());
    let content = parsed.document.template_contents(template).unwrap().unwrap();
    assert_eq!(parsed.document.name(parsed.document.first_child(content).unwrap().unwrap()).unwrap(), Some("p"));
}
