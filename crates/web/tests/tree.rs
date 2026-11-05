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
        "div", "p", "a", "b", "i", "html", "head", "body", "pre", "li", "dl", "dd", "form", "button", "option", "select", "optgroup", "script", "style", "title", "textarea", "frameset", "frame", "br",
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
    }
}
