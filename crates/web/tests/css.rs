#[path = "support/css_fixtures.rs"]
mod fixtures;
use olo_web::css::{self, Kind, Limits, StatementKind, Syntax};
use std::sync::{Arc, atomic::AtomicBool};
#[test]
fn independent_generic_css_fixtures() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/css-parsing-tests"));
    if let Ok(path) = std::env::var("OLO_CSS_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert_eq!(report["total"], 177);
    assert_eq!(report["counts"]["passed"], 165, "{report}");
    assert_eq!(report["counts"]["updated_specification"], 12, "{report}");
    assert!(report["counts"].get("unsupported").is_none(), "{report}");
    assert!(report["counts"].get("mismatch").is_none(), "{report}");
    assert!(report["counts"].get("error").is_none());
    assert!(report["counts"].get("panic").is_none());
}
#[test]
fn current_unicode_ranges_delimiters_and_declaration_rules() {
    let syntax = Syntax::parse(Arc::from("u+1234 ~= ||"), Limits::default(), None).unwrap();
    assert_eq!(syntax.components().map(|c| c.kind()).collect::<Vec<_>>(), vec![Kind::Ident, Kind::Integer, Kind::Whitespace, Kind::Delim, Kind::Delim, Kind::Whitespace, Kind::Delim, Kind::Delim]);
    let syntax = Syntax::parse(Arc::from("--a:  bar !IMPORTANT ; foo:  baz ; @foo x;"), Limits::default(), None).unwrap();
    let decls = syntax.declarations(None).unwrap();
    assert_eq!(decls.len(), 2);
    assert_eq!(decls[0].kind(), StatementKind::Declaration);
    assert!(decls[0].important());
    assert_eq!(decls[0].value().next().unwrap().text(), Some("bar"));
    assert_eq!(decls[0].value().count(), 1);
    let syntax = Syntax::parse(Arc::from("--a: a {b} c; foo: a {b}; bar: {b} !important; a:hover{color:red}"), Limits::default(), None).unwrap();
    let statements = syntax.block_contents(None).unwrap();
    assert_eq!(statements.iter().map(|s| s.kind()).collect::<Vec<_>>(), vec![StatementKind::Declaration, StatementKind::QualifiedRule, StatementKind::Declaration, StatementKind::QualifiedRule]);
}
#[test]
fn syntax_is_bounded_cancellable_and_source_backed() {
    let cancelled = AtomicBool::new(true);
    assert!(matches!(Syntax::parse(Arc::from("a{}"), Limits::default(), Some(&cancelled)), Err(css::Error::Cancelled)));
    assert!(matches!(Syntax::parse(Arc::from("a{}"), Limits { input_bytes: 2, ..Default::default() }, None), Err(css::Error::InputLimit)));
    assert!(matches!(Syntax::parse(Arc::from("a b"), Limits { tokens: 1, ..Default::default() }, None), Err(css::Error::TokenLimit)));
    assert!(matches!(Syntax::parse(Arc::from("((x))"), Limits { depth: 1, ..Default::default() }, None), Err(css::Error::DepthLimit)));
    assert!(matches!(Syntax::parse(Arc::from("\"\\61\""), Limits { decoded_bytes: 0, ..Default::default() }, None), Err(css::Error::DecodedLimit)));
    assert!(matches!(Syntax::parse(Arc::from("/*"), Limits { diagnostics: 0, ..Default::default() }, None), Err(css::Error::DiagnosticLimit)));
    let source: Arc<str> = Arc::from("a{content:\"private-css-value\"; width:12px}");
    let syntax = Syntax::parse(source.clone(), Limits { decoded_bytes: 0, ..Default::default() }, None).unwrap();
    assert_eq!(syntax.memory().decoded_bytes, 0);
    assert_eq!(syntax.memory().node_size, 24);
    let empty = Syntax::parse(Arc::from("\"\\\n\""), Limits { decoded_bytes: 0, ..Default::default() }, None).unwrap();
    assert_eq!(empty.components().next().unwrap().text(), Some(""));
    assert_eq!(empty.memory().decoded_slot_reserved_bytes, 0);
    assert!(!format!("{syntax:?}").contains("private-css-value"));
    let ident = syntax.components().next().unwrap();
    let text = ident.text().unwrap();
    assert!(text.as_ptr() as usize >= source.as_ptr() as usize);
    assert!((text.as_ptr() as usize) + text.len() <= source.as_ptr() as usize + source.len());
}
#[test]
fn deep_components_and_long_unicode_strings_drop_without_recursion_or_split_utf8() {
    let source: Arc<str> = Arc::from(format!("{}x{}", "(".repeat(12000), ")".repeat(12000)));
    let syntax = Syntax::parse(source, Limits { depth: 12001, ..Default::default() }, None).unwrap();
    assert_eq!(syntax.memory().nodes, 12001);
    let mut value = syntax.components().next().unwrap();
    for _ in 0..12000 {
        assert!(value.is_closed());
        value = value.children().next().unwrap();
    }
    assert_eq!(value.text(), Some("x"));
    drop(syntax);
    let text = "🙂".repeat(50000);
    let syntax = Syntax::parse(Arc::from(format!("\"{text}\"")), Limits::default(), None).unwrap();
    assert_eq!(syntax.components().next().unwrap().text(), Some(text.as_str()));
    assert_eq!(syntax.memory().decoded_bytes, 0);
}
#[test]
fn malformed_css_makes_bounded_progress() {
    let parts = ["a", ":", ";", "{", "}", "[", "]", "(", ")", "'", "\"", "\\", "\n", "\r\n", "\0", "🙂", "u+??", "url(", "/*", "*/", "!important", "1e-999px"];
    let mut seed = 0x230a_df60_72e9_4b18u64;
    for _ in 0..10000 {
        let mut input = String::new();
        for _ in 0..40 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            input.push_str(parts[seed as usize % parts.len()]);
        }
        let syntax = Syntax::parse(Arc::from(input), Limits::default(), None).unwrap();
        syntax.stylesheet(None).unwrap();
        syntax.block_contents(None).unwrap();
        syntax.declarations(None).unwrap();
    }
}

#[test]
fn pinned_wpt_syntax_assertions_without_claiming_cssom_or_page_execution() {
    use sha2::{Digest, Sha256};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/css-wpt");
    let pinned: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (file, sha) in pinned["files"].as_object().unwrap() {
        assert_eq!(sha.as_str().unwrap(), format!("{:x}", Sha256::digest(std::fs::read(dir.join(file)).unwrap())));
    }
    let data = std::fs::read_to_string(dir.join("non-ascii-codepoints.html")).unwrap();
    let ranges = data.split("testValidRanges([\n").nth(1).unwrap().split("]);\n").next().unwrap();
    let mut valid = Vec::new();
    let mut invalid = vec![0x80];
    // Port the data/intended lexer assertions, not WPT's JS/CSSOM harness.
    // The pinned source has a missing comma between two range entries;
    // line-wise data extraction keeps the source unmodified and explicit.
    for line in ranges.lines() {
        let line = line.trim().trim_end_matches(',');
        let numbers: Vec<u32> = line.trim_matches(['[', ']']).split(',').map(|n| u32::from_str_radix(n.trim().trim_start_matches("0x"), 16).unwrap()).collect();
        let start = numbers[0];
        let end = *numbers.last().unwrap();
        valid.extend([start, (start + end) / 2, end]);
        invalid.extend([start - 1, end + 1]);
    }
    for cp in valid {
        if let Some(c) = char::from_u32(cp) {
            let source: Arc<str> = Arc::from(format!("f{c}oo"));
            let syntax = Syntax::parse(source, Limits::default(), None).unwrap();
            assert_eq!(syntax.components().count(), 1, "U+{cp:04x}");
            assert_eq!(syntax.components().next().unwrap().kind(), Kind::Ident);
        }
    }
    // Omit neighbors that are themselves adjacent valid ranges, as WPT does.
    for cp in invalid {
        if cp > 0x1ffff
            || ranges.lines().any(|line| {
                let ns: Vec<_> = line.trim().trim_end_matches(',').trim_matches(['[', ']']).split(',').map(|n| u32::from_str_radix(n.trim().trim_start_matches("0x"), 16).unwrap()).collect();
                cp >= ns[0] && cp <= *ns.last().unwrap()
            })
        {
            continue;
        }
        if let Some(c) = char::from_u32(cp) {
            let syntax = Syntax::parse(Arc::from(format!("f{c}oo")), Limits::default(), None).unwrap();
            assert!(syntax.components().count() > 1, "U+{cp:04x}");
        }
    }
    let data = std::fs::read_to_string(dir.join("declarations-trim-whitespace.html")).unwrap();
    let css = data.split("<style id=\"style-el\">").nth(1).unwrap().split("</style>").next().unwrap();
    let canonical = data.split("let canonical = \"").nth(1).unwrap().split('"').next().unwrap();
    let syntax = Syntax::parse(Arc::from(css), Limits::default(), None).unwrap();
    let rules = syntax.stylesheet(None).unwrap();
    let declarations = rules[0].block_contents(None).unwrap();
    assert_eq!(declarations.len(), 9);
    for (i, d) in declarations.iter().enumerate() {
        assert_eq!(d.kind(), StatementKind::Declaration);
        assert_eq!(d.value().count(), 1);
        assert_eq!(d.value().next().unwrap().text(), Some(canonical));
        assert_eq!(d.important(), (4..=7).contains(&i));
    }
}
