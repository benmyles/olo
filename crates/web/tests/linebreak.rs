use olo_web::text::linebreak::{self as lb, Kind, Opportunity};
use std::sync::Arc;
fn breaks(text: &str, budget: usize) -> lb::Breaks {
    let mut job = lb::Job::new(Arc::from(text), Default::default(), None).unwrap();
    while !job.step(budget).unwrap().finished {}
    assert_eq!(job.progress().input_bytes, text.len());
    assert_eq!(job.step(0).unwrap(), job.progress());
    job.finish().unwrap()
}
#[test]
fn unicode_18_all_default_line_break_boundaries() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/linebreak-fixtures");
    let manifest: serde_json::Value = serde_json::from_str(include_str!("linebreak-fixtures/SOURCE.json")).unwrap();
    for (path, info) in manifest["files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), info["sha256"].as_str().unwrap(), "{path}");
    }
    for (path, hash) in manifest["local_files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}");
    }
    let mut count = 0;
    let mut failures = Vec::new();
    for (line_index, line) in include_str!("linebreak-fixtures/ucd/LineBreakTest.txt").lines().enumerate() {
        let (input, comment) = line.split_once('#').unwrap_or((line, ""));
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut expected = Vec::new();
        for token in input.split_whitespace() {
            match token {
                "×" => (),
                "÷" => expected.push(text.len() as u32),
                _ => text.push(char::from_u32(u32::from_str_radix(token, 16).unwrap()).unwrap()),
            }
        }
        let rules: Vec<_> = comment.split('÷').skip(1).map(|s| s.trim_start().strip_prefix('[').unwrap().split(']').next().unwrap()).collect();
        assert_eq!(rules.len(), expected.len());
        let expected: Vec<_> = expected
            .into_iter()
            .zip(rules)
            .map(|(offset, rule)| Opportunity { offset, kind: if rule.starts_with("3.") || rule.starts_with("4.") || rule.starts_with("5.") { Kind::Required } else { Kind::Allowed } })
            .collect();
        for budget in [1, 4096] {
            let actual: Vec<_> = breaks(&text, budget).iter().collect();
            if actual != expected {
                if failures.len() < 20 {
                    failures.push(format!("line {} budget {budget}: {line}\nactual {actual:?}, expected {expected:?}", line_index + 1));
                }
                break;
            }
        }
        count += 1;
    }
    assert_eq!(count, 19346);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn streaming_frontier_limits_cancellation_and_failure_poisoning() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let text: Arc<str> = Arc::from("first word second word third\r\nlast");
    let mut job = lb::Job::new(text.clone(), Default::default(), None).unwrap();
    assert_eq!(job.step(0).unwrap(), lb::Progress::default());
    assert!(matches!(job.finish(), Err(lb::Error::Incomplete)));
    let mut job = lb::Job::new(text.clone(), Default::default(), None).unwrap();
    let mut published = Vec::new();
    loop {
        let progress = job.step(1).unwrap();
        let frontier: Vec<_> = job.completed().unwrap().collect();
        assert!(frontier.starts_with(&published));
        assert!(frontier.iter().all(|o| o.offset as usize <= progress.input_bytes));
        published = frontier;
        if progress.finished {
            break;
        }
    }
    let p = job.finish().unwrap();
    assert_eq!(published, p.iter().collect::<Vec<_>>());
    assert!(Arc::ptr_eq(p.source(), &text));
    for limit in [0, 1, 3] {
        let mut job = lb::Job::new(text.clone(), lb::Limits { opportunities: limit, ..Default::default() }, None).unwrap();
        assert_eq!(job.step(1000), Err(lb::Error::Limit));
        assert_eq!(job.step(1), Err(lb::Error::Failed));
        assert!(matches!(job.completed(), Err(lb::Error::Failed)));
        assert!(matches!(job.finish(), Err(lb::Error::Failed)));
    }
    assert!(matches!(lb::Job::new(text.clone(), lb::Limits { text_bytes: 2, ..Default::default() }, None), Err(lb::Error::Limit)));
    let cancel = AtomicBool::new(true);
    assert!(matches!(lb::Job::new(text.clone(), Default::default(), Some(&cancel)), Err(lb::Error::Cancelled)));
    cancel.store(false, Ordering::Relaxed);
    let mut job = lb::Job::new(text, Default::default(), Some(&cancel)).unwrap();
    job.step(2).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(job.step(1), Err(lb::Error::Cancelled));
    cancel.store(false, Ordering::Relaxed);
    assert!(matches!(job.finish(), Err(lb::Error::Failed)));
    let p = breaks("ภาษาไทย", 1);
    assert_eq!(p.complex_context_scalars(), 7);
    assert!(p.iter().all(|o| o.kind == Kind::Required));
    assert_eq!(breaks("Latin 字", 1).complex_context_scalars(), 0);
    fn share<T: Send + Sync>() {}
    share::<lb::Breaks>();
}

#[test]
fn prepared_source_breaks_share_storage_and_reverse_iteration() {
    use olo_web::text::prepare::{self, Span, WhiteSpace};
    let source: Arc<str> = Arc::from("alpha  \n \t beta gamma");
    let spans = Arc::from([Span { range: 0..source.len() as u32, whitespace: WhiteSpace::default() }]);
    let mut job = prepare::Job::new(source, spans, Default::default(), None).unwrap();
    while !job.step(8).unwrap().finished {}
    let p = job.finish().unwrap();
    assert_eq!(&**p.text(), "alpha beta gamma");
    let source = p.text().clone();
    let mut job = lb::Job::new(source.clone(), Default::default(), None).unwrap();
    while !job.step(8).unwrap().finished {}
    let b = job.finish().unwrap();
    assert!(Arc::ptr_eq(b.source(), &source));
    let items: Vec<_> = b.iter().collect();
    assert_eq!(items.iter().map(|b| b.offset).collect::<Vec<_>>(), vec![6, 11, 16]);
    let mut iter = b.iter();
    assert_eq!(iter.len(), 3);
    assert_eq!(iter.next_back(), Some(items[2]));
    assert_eq!(iter.next(), Some(items[0]));
    assert_eq!(iter.next_back(), Some(items[1]));
    assert_eq!(iter.len(), 0);
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    for o in items.iter().take(2) {
        let mapping = p.mapping_at_output(o.offset).unwrap();
        assert_eq!(mapping.kind, prepare::MappingKind::Copy);
    }
    assert_eq!(breaks("longword", 32).payload_bytes(), 0);
    assert_eq!(breaks("", 32).payload_bytes(), 0);
}

#[test]
fn ascii_run_boundaries_preserve_quotation_numeric_and_combining_contexts() {
    for count in 1..100 {
        for (prefix, suffix) in
            [("", ""), ("1.23 ", ".5"), ("“", "”字"), ("字“", "”字"), ("( ", ")abc"), ("\u{200b}", "\u{a0}x"), ("ש-", " 9"), ("a\u{200d}", "👍🏽"), ("a\u{308}", "\u{308}字"), ("ภาษา", "ฯ")]
        {
            let text = format!("{prefix}{}{suffix}", "AbCdEfghijKlmnoPqrsTuvWxyz01234567890123456789".repeat(count));
            let expected: Vec<_> = breaks(&text, 1).iter().collect();
            for budget in [2, 3, 4, 7, 31, 32, 33, 128, 4096] {
                assert_eq!(breaks(&text, budget).iter().collect::<Vec<_>>(), expected, "{count} {prefix:?} {suffix:?} budget {budget}");
            }
        }
    }
}
#[test]
fn required_markers_utf8_offsets_and_constant_combining_state() {
    let p = breaks("a\r\nb\u{85}中\u{2028}c", 1);
    assert_eq!(
        p.iter().collect::<Vec<_>>(),
        vec![
            Opportunity { offset: 3, kind: Kind::Required },
            Opportunity { offset: 6, kind: Kind::Required },
            Opportunity { offset: 12, kind: Kind::Required },
            Opportunity { offset: 13, kind: Kind::Required }
        ]
    );
    let text = format!("a{}b c", "\u{308}".repeat(100000));
    let p = breaks(&text, 128);
    assert_eq!(p.iter().map(|o| o.offset as usize).collect::<Vec<_>>(), vec![text.len() - 1, text.len()]);
    assert_eq!(p.payload_bytes(), 4);
    assert_eq!(breaks("", 1).iter().collect::<Vec<_>>(), vec![Opportunity { offset: 0, kind: Kind::Required }]);
}
