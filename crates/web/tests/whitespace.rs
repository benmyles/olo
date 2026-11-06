use olo_web::{
    css::{self, properties as p},
    text::prepare::{self as prep, Collapse as C, MappingKind as M, Span, WhiteSpace},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
fn uniform(source: &str, mode: C, budget: usize) -> prep::Prepared {
    let source: Arc<str> = Arc::from(source);
    let spans: Arc<[Span]> = if source.is_empty() { Arc::from([]) } else { Arc::from([Span { range: 0..source.len() as u32, whitespace: WhiteSpace { collapse: mode, wrap: true } }]) };
    let expected_visits = source.len() * 2;
    let mut job = prep::Job::new(source, spans, Default::default(), None).unwrap();
    while !job.step(budget).unwrap().finished {}
    assert_eq!(job.progress().scanned_bytes, expected_visits);
    job.finish().unwrap()
}
fn invariants(p: &prep::Prepared) {
    let (mut input, mut output) = (0, 0);
    for m in p.mappings() {
        assert_eq!(m.input.start, input);
        assert_eq!(m.output.start, output);
        assert!(m.input.start < m.input.end);
        assert!(p.spans()[m.span as usize].range.start <= m.input.start && p.spans()[m.span as usize].range.end >= m.input.end);
        for b in [m.input.start, m.input.end] {
            assert!(p.source().is_char_boundary(b as usize));
        }
        for b in [m.output.start, m.output.end] {
            assert!(p.text().is_char_boundary(b as usize));
        }
        if m.kind == M::Removed {
            assert!(m.output.is_empty());
        } else {
            assert_eq!(m.input.len(), m.output.len());
        }
        if m.kind == M::Copy {
            assert_eq!(p.source()[m.input.start as usize..m.input.end as usize], p.text()[m.output.start as usize..m.output.end as usize]);
        }
        for b in m.input.clone() {
            assert_eq!(p.mapping_at_source(b), Some(m.clone()));
        }
        for b in m.output.clone() {
            assert_eq!(p.mapping_at_output(b), Some(m.clone()));
        }
        input = m.input.end;
        output = m.output.end;
    }
    assert_eq!(input as usize, p.source().len());
    assert_eq!(output as usize, p.text().len());
    assert!(p.mapping_at_source(input).is_none());
    assert!(p.mapping_at_output(output).is_none());
}
#[test]
fn phase_one_modes_dom_carriage_returns_and_non_css_spaces() {
    for (mode, expected) in
        [(C::Collapse, " a b c "), (C::PreserveBreaks, " a\nb\n\nc "), (C::Preserve, " \ta \t\n \tb\n\nc \t "), (C::BreakSpaces, " \ta \t\n \tb\n\nc \t "), (C::PreserveSpaces, "  a     b  c   ")]
    {
        let text = " \ta \t\n \tb\n\nc \t\r";
        for budget in [1, 2, 7, 128] {
            let p = uniform(text, mode, budget);
            assert_eq!(&**p.text(), expected, "{mode:?}");
            invariants(&p);
        }
    }
    for mode in [C::Collapse, C::PreserveBreaks, C::Preserve, C::BreakSpaces, C::PreserveSpaces] {
        let p = uniform("a\u{a0}\u{2003}\u{3000}\u{b}\u{c}\u{85}\u{2028}\u{2029}b", mode, 1);
        assert!(Arc::ptr_eq(p.source(), p.text()));
        invariants(&p);
        assert_eq!(
            &**uniform("\r\n", mode, 1).text(),
            match mode {
                C::Collapse => " ",
                C::PreserveBreaks => "\n",
                C::Preserve | C::BreakSpaces => " \n",
                C::PreserveSpaces => "  ",
            }
        );
    }
    // Phase II owns leading/trailing trimming and preserved tab positioning.
    assert_eq!(&**uniform("  a  ", C::Collapse, 1).text(), " a ");
    assert_eq!(&**uniform("\n  \n", C::PreserveBreaks, 1).text(), "\n\n");
}
#[test]
fn collapsing_ignores_all_bidi_controls_but_preserves_them() {
    for control in ['\u{61c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'] {
        let text = format!("a \t{control} \n \t{control}  b");
        for (mode, expected) in [(C::Collapse, format!("a{control} {control}b")), (C::PreserveBreaks, format!("a{control}\n{control}b"))] {
            let p = uniform(&text, mode, 1);
            assert_eq!(&**p.text(), expected);
            invariants(&p);
        }
        assert_eq!(&**uniform(&format!("a {control}  b"), C::Collapse, 2).text(), format!("a {control}b"));
        assert_eq!(&**uniform(&format!("a\n{control}\nb"), C::Collapse, 1).text(), format!("a {control}b"));
    }
}
#[test]
fn source_spans_are_transparent_to_collapsing_and_retain_removed_nodes() {
    let chunks = [("a ", C::Collapse), ("", C::Preserve), (" \t", C::PreserveBreaks), ("b\n", C::PreserveBreaks), ("  ", C::Collapse), (" c ", C::Preserve), ("  d", C::Collapse)];
    let mut source = String::new();
    let mut spans = Vec::new();
    for (text, mode) in chunks {
        let start = source.len() as u32;
        source.push_str(text);
        if !text.is_empty() {
            spans.push(Span { range: start..source.len() as u32, whitespace: WhiteSpace { collapse: mode, wrap: true } });
        }
    }
    let mut job = prep::Job::new(Arc::from(source), Arc::from(spans), Default::default(), None).unwrap();
    while !job.step(1).unwrap().finished {}
    let p = job.finish().unwrap();
    assert_eq!(&**p.text(), "a b\n c  d");
    invariants(&p);
    assert!(p.mappings().any(|m| m.span == 1 && m.kind == M::Removed));
    assert!(p.mappings().any(|m| m.span == 3 && m.kind == M::Removed));
    // Breaking a uniform style into arbitrary text nodes changes only span IDs.
    let text = "é  a\u{2067} \n \t中 \u{2069}  b\n";
    let expected = uniform(text, C::Collapse, 9);
    let spans: Vec<_> = text.char_indices().map(|(i, c)| Span { range: i as u32..(i + c.len_utf8()) as u32, whitespace: WhiteSpace::default() }).collect();
    let mut job = prep::Job::new(Arc::from(text), Arc::from(spans), Default::default(), None).unwrap();
    while !job.step(3).unwrap().finished {}
    let p = job.finish().unwrap();
    assert_eq!(p.text(), expected.text());
    invariants(&p);
}
#[test]
fn invalid_partitions_limits_cancellation_and_poisoned_outputs() {
    let source: Arc<str> = Arc::from("é  \na\t");
    let valid: Arc<[Span]> = Arc::from([Span { range: 0..source.len() as u32, whitespace: WhiteSpace::default() }]);
    for range in [1..source.len() as u32, 0..1, 0..0, 0..99] {
        let spans = Arc::from([Span { range, whitespace: WhiteSpace::default() }]);
        assert!(matches!(prep::Job::new(source.clone(), spans, Default::default(), None), Err(prep::Error::Invalid)));
    }
    for limits in [prep::Limits { text_bytes: 1, ..Default::default() }, prep::Limits { spans: 0, ..Default::default() }] {
        assert!(matches!(prep::Job::new(source.clone(), valid.clone(), limits, None), Err(prep::Error::Limit)));
    }
    for limits in [prep::Limits { mappings: 0, ..Default::default() }, prep::Limits { trim_ranges: 0, ..Default::default() }] {
        let mut job = prep::Job::new(source.clone(), valid.clone(), limits, None).unwrap();
        assert_eq!(job.step(100), Err(prep::Error::Limit));
        assert_eq!(job.step(100), Err(prep::Error::Failed));
        assert!(matches!(job.finish(), Err(prep::Error::Failed)));
    }
    let mut job = prep::Job::new(source.clone(), valid.clone(), Default::default(), None).unwrap();
    assert_eq!(job.step(0).unwrap(), prep::Progress::default());
    assert!(!job.step(1).unwrap().finished);
    assert!(matches!(job.finish(), Err(prep::Error::Incomplete)));
    let cancel = AtomicBool::new(false);
    let mut job = prep::Job::new(source, valid, Default::default(), Some(&cancel)).unwrap();
    job.step(2).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(job.step(1), Err(prep::Error::Cancelled));
    cancel.store(false, Ordering::Relaxed);
    assert_eq!(job.step(1), Err(prep::Error::Failed));
    assert!(uniform("", C::Collapse, 1).text().is_empty());
    fn share<T: Send + Sync>() {}
    share::<prep::Prepared>();
}
#[test]
fn modern_white_space_shorthand_expands_both_longhands_atomically() {
    for (input, collapse, wrap) in [
        ("normal", p::Keyword::Collapse, true),
        ("pre", p::Keyword::Preserve, false),
        ("pre-wrap", p::Keyword::Preserve, true),
        ("pre-line", p::Keyword::PreserveBreaks, true),
        ("nowrap", p::Keyword::Collapse, false),
        ("break-spaces", p::Keyword::BreakSpaces, true),
        ("nowrap preserve-breaks", p::Keyword::PreserveBreaks, false),
        ("preserve-spaces wrap", p::Keyword::PreserveSpaces, true),
    ] {
        let syntax = css::Syntax::parse(Arc::from(input), Default::default(), None).unwrap();
        let p::Declaration::Known { values, .. } = p::Declaration::parse("white-space", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].property, p::Property::WhiteSpaceCollapse);
        assert!(matches!(values[0].value,p::Value::Keyword(k) if k==collapse));
        assert_eq!(values[1].property, p::Property::TextWrapMode);
        assert!(matches!(values[1].value,p::Value::Keyword(k) if k==if wrap {p::Keyword::Wrap}else{p::Keyword::Nowrap}));
    }
    for input in ["", "normal wrap", "pre nowrap", "collapse preserve", "wrap nowrap", "break-spaces break-spaces", "discard-before", "collapse none", "discard"] {
        let syntax = css::Syntax::parse(Arc::from(input), Default::default(), None).unwrap();
        assert!(p::Declaration::parse("white-space", syntax.components(), false, Default::default(), None).is_err(), "{input}");
    }
}

#[test]
fn independent_regex_phase_oracle_and_literal_wpt_acceptance_ports() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/whitespace-fixtures");
    let manifest: serde_json::Value = serde_json::from_str(include_str!("whitespace-fixtures/SOURCE.json")).unwrap();
    for (path, info) in manifest["files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), info["sha256"].as_str().unwrap());
    }
    for (path, hash) in manifest["local_files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap());
    }
    let data: serde_json::Value = serde_json::from_str(include_str!("whitespace-fixtures/derived.json")).unwrap();
    assert_eq!(data["parsing"].as_array().unwrap().len(), 52);
    for case in data["parsing"].as_array().unwrap() {
        let syntax = css::Syntax::parse(Arc::from(case["input"].as_str().unwrap()), Default::default(), None).unwrap();
        let result = p::Declaration::parse(case["property"].as_str().unwrap(), syntax.components(), false, Default::default(), None);
        assert_eq!(result.is_ok(), case["valid"].as_bool().unwrap(), "{case}");
    }
    assert_eq!(data["phase_one"].as_array().unwrap().len(), 3925);
    for case in data["phase_one"].as_array().unwrap() {
        let mode = match case["mode"].as_str().unwrap() {
            "collapse" => C::Collapse,
            "preserve" => C::Preserve,
            "preserve-breaks" => C::PreserveBreaks,
            "preserve-spaces" => C::PreserveSpaces,
            "break-spaces" => C::BreakSpaces,
            _ => panic!(),
        };
        let text = case["input"].as_str().unwrap();
        for budget in [1, 29] {
            let p = uniform(text, mode, budget);
            assert_eq!(&**p.text(), case["expected"].as_str().unwrap(), "{case}");
            invariants(&p);
        }
        let spans: Vec<_> = text.char_indices().map(|(i, c)| Span { range: i as u32..(i + c.len_utf8()) as u32, whitespace: WhiteSpace { collapse: mode, wrap: false } }).collect();
        let mut job = prep::Job::new(Arc::from(text), Arc::from(spans), Default::default(), None).unwrap();
        while !job.step(7).unwrap().finished {}
        let p = job.finish().unwrap();
        assert_eq!(&**p.text(), case["expected"].as_str().unwrap(), "split {case}");
        invariants(&p);
    }
}

#[test]
fn bulk_scan_boundaries_preserve_all_utf8_and_edit_positions() {
    for count in 0..70 {
        for special in [" ", "\t", "\r", "\n", "é", "中", "\u{2067}", "\u{2069}", "\u{7f}", "\u{1f600}"] {
            let text = format!("{}{} \n \t{}{}", "a".repeat(count), special, "b".repeat(150), special);
            let scalar = uniform(&text, C::Collapse, 1);
            for budget in [31, 32, 33, 63, 64, 65, 65536] {
                let p = uniform(&text, C::Collapse, budget);
                assert_eq!(p.text(), scalar.text());
                assert_eq!(p.mappings().collect::<Vec<_>>(), scalar.mappings().collect::<Vec<_>>());
                invariants(&p);
            }
        }
    }
}
