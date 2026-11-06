use olo_web::{
    css::{
        Syntax,
        properties::{self as p, Declaration},
        style::{Axes, FontRequest, FontSlant},
    },
    font::{self, Builder, Catalog, FontSet, GenericPolicy, MatchOptions, Scope},
    text::shaping::{self as s, Analysis, BaseDirection, Direction, Engine, Feature, RunOptions, Script},
};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn families(text: &str) -> p::Families {
    let syntax = Syntax::parse(Arc::from(text), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let p::Value::Families(f) = &values[0].value else { panic!() };
    f.clone()
}
fn request(family: &str) -> FontRequest {
    FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families: families(&format!("'{family}'")), axes: Axes::default() }
}
fn references() -> serde_json::Value {
    serde_json::from_str(include_str!("shaping-fixtures/harfbuzz.json")).unwrap()
}
fn catalog() -> Arc<Catalog> {
    let mut paths = std::collections::HashSet::new();
    let mut b = Builder::new(Default::default());
    for case in references()["cases"].as_array().unwrap() {
        let path = case["font"].as_str().unwrap();
        if paths.insert(path.to_owned()) {
            b.load_file(&repo().join(path), |_| {}, None).unwrap();
        }
    }
    b.finish(None).unwrap()
}
fn scope(c: Arc<Catalog>, family: &str, language: &str) -> Scope {
    FontSet::new(c, GenericPolicy::new(&[family]).unwrap(), MatchOptions::default()).unwrap().scope(language).unwrap()
}
fn direction(text: &str) -> Direction {
    match text {
        "ltr" => Direction::LeftToRight,
        "rtl" => Direction::RightToLeft,
        "ttb" => Direction::TopToBottom,
        _ => panic!(),
    }
}
fn feature_list(case: &serde_json::Value) -> Vec<Feature> {
    case["features"].as_object().unwrap().iter().map(|(tag, value)| Feature::new(harfrust::Tag::new(tag.as_bytes().try_into().unwrap()), u32::from(value.as_bool().unwrap()), ..)).collect()
}
fn case_request(case: &serde_json::Value) -> FontRequest {
    let mut r = request(case["family"].as_str().unwrap());
    if let Some(w) = case["variations"]["wght"].as_f64() {
        r.weight = w;
    }
    if let Some(w) = case["variations"]["wdth"].as_f64() {
        r.width = w;
    }
    if let Some(size) = case["variations"]["opsz"].as_f64() {
        r.size = size;
    }
    r
}
fn assert_glyphs(run: &s::Run, expected: &serde_json::Value, name: &str) {
    let rows = expected.as_array().unwrap();
    assert_eq!(run.glyphs.len(), rows.len(), "{name}: glyph count");
    for (i, (g, e)) in run.glyphs.iter().zip(rows).enumerate() {
        for (key, actual) in [
            ("id", i64::from(g.id)),
            ("cluster", i64::from(g.cluster)),
            ("x_advance", i64::from(g.x_advance)),
            ("y_advance", i64::from(g.y_advance)),
            ("x_offset", i64::from(g.x_offset)),
            ("y_offset", i64::from(g.y_offset)),
        ] {
            assert_eq!(actual, e[key].as_i64().unwrap(), "{name}: glyph {i}, {key}");
        }
        assert_eq!(g.unsafe_to_break, e["unsafe_to_break"].as_bool().unwrap(), "{name}: break flag {i}");
    }
}
#[test]
fn native_harfbuzz_glyphs_positions_flags_variations_features_and_utf8_clusters() {
    let c = catalog();
    let mut engine = Engine::default();
    let refs = references();
    for case in refs["cases"].as_array().unwrap() {
        let path = repo().join(case["font"].as_str().unwrap());
        let hash = format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()));
        assert_eq!(case["sha256"].as_str().unwrap(), hash);
        let r = case_request(case);
        let selected = c.select_family(case["family"].as_str().unwrap(), &r, Default::default(), None).unwrap().unwrap();
        let features = feature_list(case);
        let text: Arc<str> = Arc::from(case["text"].as_str().unwrap());
        let start = case["start"].as_u64().unwrap() as usize;
        let end = case["end"].as_u64().unwrap() as usize;
        let run = engine
            .shape_run(
                &selected,
                text.clone(),
                start..end,
                RunOptions {
                    direction: direction(case["direction"].as_str().unwrap()),
                    script: Script::from_iso15924_tag(harfrust::Tag::new(case["script"].as_str().unwrap().as_bytes().try_into().unwrap())).unwrap(),
                    language: Some(case["language"].as_str().unwrap()),
                    features: &features,
                    beginning_of_text: start == 0,
                    end_of_text: end == text.len(),
                    context: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(run.font.units_per_em(), case["upem"].as_u64().unwrap() as u16);
        assert_glyphs(&run, &case["glyphs"], case["name"].as_str().unwrap());
    }
    assert_eq!(refs["cases"].as_array().unwrap().len(), 24);
}
#[test]
fn unicode_18_bidi_character_conformance_through_l2() {
    let source = include_str!("shaping-fixtures/ucd/BidiCharacterTest.txt");
    let (mut total, mut failed) = (0, 0);
    let mut messages = Vec::new();
    for (line_number, line) in source.lines().enumerate() {
        let record = line.split('#').next().unwrap().trim();
        if record.is_empty() {
            continue;
        }
        let fields: Vec<_> = record.split(';').map(str::trim).collect();
        let chars: Vec<char> = fields[0].split_whitespace().map(|v| char::from_u32(u32::from_str_radix(v, 16).unwrap()).unwrap()).collect();
        let text: String = chars.iter().collect();
        let offsets: Vec<_> = text.char_indices().map(|(i, _)| i).collect();
        let base = match fields[1] {
            "0" => BaseDirection::Ltr,
            "1" => BaseDirection::Rtl,
            "2" => BaseDirection::Auto,
            _ => panic!(),
        };
        let expected_base: u8 = fields[2].parse().unwrap();
        let expected_levels: Vec<Option<u8>> = fields[3].split_whitespace().map(|v| if v == "x" { None } else { Some(v.parse().unwrap()) }).collect();
        let expected_order: Vec<usize> = fields[4].split_whitespace().map(|v| v.parse().unwrap()).collect();
        let analysis = Analysis::new(Arc::from(text.as_str()), base, 1024 * 1024, None).unwrap();
        let base_ok = analysis.paragraphs().next().unwrap().1 == expected_base;
        let visual_runs = analysis.visual_runs(0..text.len()).unwrap();
        // The corpus expects line levels after L1; paragraph levels precede it.
        let levels_ok = offsets.iter().zip(&expected_levels).all(|(&at, &value)| value.is_none_or(|value| visual_runs.iter().any(|r| r.range.contains(&(at as u32)) && r.level == value)));
        let mut actual = Vec::new();
        for run in visual_runs {
            let mut indices: Vec<usize> =
                offsets.iter().enumerate().filter(|&(i, &at)| at >= run.range.start as usize && at < run.range.end as usize && expected_levels[i].is_some()).map(|(i, _)| i).collect();
            if run.level % 2 == 1 {
                indices.reverse();
            }
            actual.extend(indices);
        }
        total += 1;
        if !base_ok || !levels_ok || actual != expected_order {
            failed += 1;
            if messages.len() < 8 {
                messages.push(format!("line {}: base {base_ok}, levels {levels_ok}, order {actual:?} vs {expected_order:?}", line_number + 1));
            }
        }
    }
    eprintln!("Unicode 18 bidi character cases: {total}, failed {failed}");
    assert!(total > 90000);
    assert_eq!(failed, 0, "{}", messages.join("\n"));
}
#[test]
fn unicode_18_extended_grapheme_conformance() {
    let (mut total, mut failed) = (0, 0);
    let mut messages = Vec::new();
    for (line_number, line) in include_str!("shaping-fixtures/ucd/GraphemeBreakTest.txt").lines().enumerate() {
        let record = line.split('#').next().unwrap().trim();
        if record.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut expected = Vec::new();
        for token in record.split_whitespace() {
            match token {
                "÷" => expected.push(text.len()),
                "×" => {}
                value => text.push(char::from_u32(u32::from_str_radix(value, 16).unwrap()).unwrap()),
            }
        }
        let mut actual: Vec<_> = s::graphemes(&text).map(|r| r.start).collect();
        actual.push(text.len());
        total += 1;
        if actual != expected {
            failed += 1;
            if messages.len() < 8 {
                messages.push(format!("line {}: {actual:?} vs {expected:?}", line_number + 1));
            }
        }
    }
    eprintln!("Unicode 18 grapheme cases: {total}, failed {failed}");
    assert!(total > 700);
    assert_eq!(failed, 0, "{}", messages.join("\n"));
}

fn paragraph(engine: &mut Engine, scope: &Scope, r: FontRequest, text: &str, step: usize) -> s::Paragraph {
    let mut job = s::ParagraphJob::new(engine, scope, r, Arc::from(text), BaseDirection::Auto, Default::default(), None).unwrap();
    let mut previous = 0;
    loop {
        let p = job.step(step).unwrap();
        assert!(p.clusters >= previous);
        previous = p.clusters;
        if p.finished {
            break;
        }
    }
    job.finish().unwrap()
}
#[test]
fn uniform_script_paragraphs_match_independent_font_runs() {
    let c = catalog();
    let mut engine = Engine::default();
    let refs = references();
    let mut checked = 0;
    for case in refs["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        if case["start"] != 0 || case["end"].as_u64().unwrap() as usize != text.len() || case["direction"] == "ttb" || !case["features"].as_object().unwrap().is_empty() {
            continue;
        }
        let sc = scope(c.clone(), case["family"].as_str().unwrap(), case["language"].as_str().unwrap());
        let p = paragraph(&mut engine, &sc, case_request(case), text, 2);
        assert_eq!(p.runs.len(), 1, "{}", case["name"]);
        assert_glyphs(&p.runs[0], &case["glyphs"], case["name"].as_str().unwrap());
        assert_eq!(p.report, s::ParagraphReport::default());
        checked += 1;
    }
    assert!(checked >= 15);
}
type RunSignature = (std::ops::Range<u32>, font::FaceId, Script, Direction, Vec<s::Glyph>);
fn signature(p: &s::Paragraph) -> Vec<RunSignature> {
    p.runs.iter().map(|r| (r.range.clone(), r.font.selection().face, r.script, r.direction, r.glyphs.to_vec())).collect()
}
#[test]
fn mixed_scripts_fallback_and_cache_controls_preserve_complete_runs() {
    let c = catalog();
    let sc = scope(c.clone(), "Noto Sans", "und");
    let text = "office שלום السَّلَامُ हिन्दी ภาษาไทย end";
    let mut warm = Engine::default();
    let a = paragraph(&mut warm, &sc, request("Noto Sans"), text, 1);
    let mut cold = Engine::new(s::Limits { cached_faces: 0, cached_instances: 0, cached_plans: 0, ..Default::default() });
    let b = paragraph(&mut cold, &sc, request("Noto Sans"), text, 7);
    assert_eq!(signature(&a), signature(&b));
    assert_eq!(a.report, s::ParagraphReport::default());
    assert!(a.runs.len() > 4);
    assert_eq!(a.runs.iter().map(|r| r.range.len()).sum::<usize>(), text.len());
    let families: std::collections::HashSet<_> = a.runs.iter().flat_map(|r| c.face(r.font.selection().face).unwrap().families.iter().map(|n| n.to_string())).collect();
    for name in ["Noto Sans", "Noto Sans Hebrew", "Noto Sans Arabic", "Noto Sans Devanagari", "Noto Sans Thai"] {
        assert!(families.contains(name), "{families:?}");
    }
    assert_eq!(cold.cache_stats().faces, 0);
    assert_eq!(cold.cache_stats().plans, 0);
    for r in &a.runs {
        assert!(r.glyphs.iter().all(|g| g.cluster >= r.range.start && g.cluster < r.range.end));
        let cs: Vec<_> = r.glyphs.iter().map(|g| g.cluster).collect();
        assert!(cs.windows(2).all(|c| if r.direction == Direction::RightToLeft { c[0] >= c[1] } else { c[0] <= c[1] }));
    }
}
#[test]
fn whole_grapheme_and_variation_fallback_have_explicit_provenance() {
    let c = catalog();
    let sc = scope(c.clone(), "Noto Sans", "und");
    let mut engine = Engine::default();
    let p = paragraph(&mut engine, &sc, request("Noto Sans"), "क्ष", 1);
    assert_eq!(p.runs.len(), 1);
    assert_eq!(c.face(p.runs[0].font.selection().face).unwrap().families[0].as_ref(), "Noto Sans Devanagari");
    assert_eq!(p.report.missing_clusters, 0);
    assert_eq!(s::graphemes("क्ष").count(), 1);
    let p = paragraph(&mut engine, &sc, request("Noto Sans"), "a\u{fe0f}", 1);
    assert_eq!(p.report.variation_fallbacks, 1);
    assert_eq!(p.report.missing_clusters, 0);
    let p = paragraph(&mut engine, &sc, request("Noto Sans"), "\u{10ffff}", 1);
    assert_eq!(p.report.missing_clusters, 1);
    assert_eq!(p.report.zero_glyphs, 1);
}
#[test]
fn bidi_line_order_is_byte_mapped_and_resets_whitespace_after_wrapping() {
    let a = Analysis::new(Arc::from("abc אבג  xyz"), BaseDirection::Ltr, 1024, None).unwrap();
    let first = a.visual_runs(0.."abc אבג  ".len()).unwrap();
    assert_eq!(first.last().unwrap().level, 0);
    assert_eq!(first.last().unwrap().range.end, "abc אבג  ".len() as u32);
    let runs = a.visual_runs(0..a.source().len()).unwrap();
    assert!(runs.iter().any(|r| r.level % 2 == 1));
    assert!(a.visual_runs(0..5).is_err());
    let p = Analysis::new(Arc::from("abc\nשלום"), BaseDirection::Auto, 1024, None).unwrap();
    assert_eq!(p.paragraphs().count(), 2);
    assert!(p.visual_runs(0..p.source().len()).is_err());
    assert!(p.visual_runs(0..4).is_ok());
}
#[test]
fn shaper_caches_are_bounded_and_font_leases_survive_clear_and_catalog_drop() {
    let c = catalog();
    let mut r = request("Noto Sans");
    let regular = c.select_family("Noto Sans", &r, Default::default(), None).unwrap().unwrap();
    r.weight = 650.5;
    let variable = c.select_family("Noto Sans", &r, Default::default(), None).unwrap().unwrap();
    let mut engine = Engine::new(s::Limits { cached_faces: 1, cached_instances: 2, cached_plans: 2, ..Default::default() });
    let text: Arc<str> = Arc::from("office AV");
    let a = engine.shape_run(&regular, text.clone(), 0..text.len(), Default::default(), None).unwrap();
    let b = engine.shape_run(&variable, text.clone(), 0..text.len(), Default::default(), None).unwrap();
    assert_eq!(a.font.selection().data().as_ptr(), regular.data().as_ptr());
    assert_eq!(b.font.selection().data().as_ptr(), regular.data().as_ptr());
    assert_eq!(engine.cache_stats().faces, 1);
    assert_eq!(engine.cache_stats().source_bytes, regular.data().len());
    assert_eq!(engine.cache_stats().instances, 2);
    assert_eq!(engine.cache_stats().plans, 2);
    let arabic = c.select_family("Noto Sans Arabic", &r, Default::default(), None).unwrap().unwrap();
    engine.shape_run(&arabic, Arc::from("سلام"), 0.."سلام".len(), RunOptions { direction: Direction::RightToLeft, script: Script::ARABIC, ..Default::default() }, None).unwrap();
    assert_eq!(engine.cache_stats().faces, 1);
    assert_eq!(engine.cache_stats().instances, 2);
    engine.clear();
    assert_eq!(engine.cache_stats().source_bytes, 0);
    drop(c);
    drop(regular);
    drop(variable);
    let again = engine.shape_run(a.font.selection(), text.clone(), 0..text.len(), Default::default(), None).unwrap();
    assert_eq!(a.glyphs, again.glyphs);
}
#[test]
fn run_bounds_cancellation_and_output_limits_recover_without_partial_results() {
    use std::sync::atomic::AtomicBool;
    let c = catalog();
    let selected = c.select_family("Noto Sans", &request("Noto Sans"), Default::default(), None).unwrap().unwrap();
    let mut e = Engine::new(s::Limits { run_bytes: 8, run_glyphs: 2, features: 0, ..Default::default() });
    assert!(matches!(e.shape_run(&selected, Arc::from("é"), 0..1, Default::default(), None), Err(s::Error::Invalid)));
    assert!(matches!(e.shape_run(&selected, Arc::from("a"), 0..1, RunOptions { direction: Direction::Invalid, ..Default::default() }, None), Err(s::Error::Invalid)));
    assert!(matches!(e.shape_run(&selected, Arc::from("123456789"), 0..9, Default::default(), None), Err(s::Error::Limit)));
    assert!(matches!(e.shape_run(&selected, Arc::from("abc"), 0..3, Default::default(), None), Err(s::Error::Limit)));
    let good = e.shape_run(&selected, Arc::from("a"), 0..1, Default::default(), None).unwrap();
    assert_eq!(good.glyphs.len(), 1);
    assert!(matches!(e.shape_run(&selected, Arc::from("a"), 0..1, Default::default(), Some(&AtomicBool::new(true))), Err(s::Error::Cancelled)));
    assert!(matches!(e.shape_run(&selected, Arc::from("a"), 0..1, RunOptions { context: Some((1, 1)), ..Default::default() }, None), Err(s::Error::Invalid)));
}
#[test]
fn paragraph_jobs_report_progress_poison_on_failure_and_reject_unprepared_contexts() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let c = catalog();
    let sc = scope(c, "Noto Sans", "und");
    let mut engine = Engine::default();
    let cancel = AtomicBool::new(false);
    let mut job = s::ParagraphJob::new(&mut engine, &sc, request("Noto Sans"), Arc::from("abc שלום hello"), BaseDirection::Auto, Default::default(), Some(&cancel)).unwrap();
    assert!(!job.step(5).unwrap().finished);
    assert!(!job.completed_runs().unwrap().is_empty());
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(job.step(1), Err(s::Error::Cancelled)));
    assert!(matches!(job.completed_runs(), Err(s::Error::Failed)));
    assert!(matches!(job.finish(), Err(s::Error::Failed)));
    cancel.store(false, Ordering::Relaxed);
    let job = s::ParagraphJob::new(&mut engine, &sc, request("Noto Sans"), Arc::from("abc"), BaseDirection::Auto, Default::default(), None).unwrap();
    assert!(matches!(job.finish(), Err(s::Error::Incomplete)));
    for limits in [s::ParagraphLimits { text_bytes: 0, ..Default::default() }, s::ParagraphLimits { clusters: 0, ..Default::default() }, s::ParagraphLimits { cluster_bytes: 0, ..Default::default() }]
    {
        assert!(matches!(s::ParagraphJob::new(&mut engine, &sc, request("Noto Sans"), Arc::from("a"), BaseDirection::Auto, limits, None), Err(s::Error::Limit)));
    }
    let mut job = s::ParagraphJob::new(&mut engine, &sc, request("Noto Sans"), Arc::from("abc"), BaseDirection::Auto, s::ParagraphLimits { runs: 0, ..Default::default() }, None).unwrap();
    assert!(matches!(job.step(100), Err(s::Error::Limit)));
    assert!(matches!(job.finish(), Err(s::Error::Failed)));
    for text in ["a\nb", "a\tb", "a\u{2028}b"] {
        assert!(matches!(s::ParagraphJob::new(&mut engine, &sc, request("Noto Sans"), Arc::from(text), BaseDirection::Auto, Default::default(), None), Err(s::Error::Unsupported(_))));
    }
    let mut r = request("Noto Sans");
    r.axes.writing_mode = olo_web::css::style::WritingMode::VerticalRl;
    assert!(matches!(s::ParagraphJob::new(&mut engine, &sc, r, Arc::from("abc"), BaseDirection::Ltr, Default::default(), None), Err(s::Error::Unsupported(_))));
}
#[path = "support/native_font_host.rs"]
mod native_host;
#[test]
fn style_font_contexts_bind_to_the_same_native_scope() {
    use olo_web::{
        css::{
            cascade::{Origin, SheetBuilder},
            style::{self, Defaults},
        },
        html,
    };
    let c = catalog();
    let sc = scope(c, "Noto Sans", "en");

    let host = native_host::NativeHost(sc.clone());
    let doc = html::parse(Arc::from("<p id=target>office</p>"), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from("p{font-family:'Noto Sans';font-size:20px}"), Default::default(), None).unwrap();
    let mut b = SheetBuilder::new(Default::default());
    b.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = b.finish(None).unwrap();
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&doc, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(16, |_| &host).unwrap().finished {}
    let tree = job.finish().unwrap();
    let style = tree.iter().find(|(n, _)| doc.attribute(*n, "id").unwrap() == Some("target")).unwrap().1;
    let mut engine = Engine::default();
    let mut job = s::ParagraphJob::for_style(&mut engine, &sc, style, Arc::from("office"), BaseDirection::Ltr, Default::default(), None).unwrap();
    assert!(job.step(100).unwrap().finished);
    assert!(job.finish().unwrap().advance().0 > 0.);
    let other = scope(catalog(), "Noto Sans", "en");
    assert!(matches!(s::ParagraphJob::for_style(&mut engine, &other, style, Arc::from("office"), BaseDirection::Ltr, Default::default(), None), Err(s::Error::Stale)));
}

#[test]
fn empty_text_uses_no_shaping_cache_and_ascii_controls_match_the_general_algorithm() {
    let sc = scope(catalog(), "Noto Sans", "und");
    let mut engine = Engine::default();
    let p = paragraph(&mut engine, &sc, request("Noto Sans"), "", 1);
    assert!(p.runs.is_empty());
    assert_eq!(p.advance(), (0., 0.));
    assert_eq!(engine.cache_stats().faces, 0);
    assert_eq!(engine.cache_stats().plans, 0);
    let a = Analysis::new(Arc::from("abc 123 ()!"), BaseDirection::Auto, 1024, None).unwrap();
    assert!(a.visual_runs(0..a.source().len()).unwrap().iter().all(|r| r.level == 0));
    let rtl = Analysis::new(Arc::from("abc 123 ()!"), BaseDirection::Rtl, 1024, None).unwrap();
    assert!(rtl.visual_runs(0..rtl.source().len()).unwrap().iter().any(|r| r.level != 0));
}

#[test]
fn finished_glyph_data_can_cross_the_worker_ui_boundary() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<s::Paragraph>();
    send_sync::<s::Run>();
    send::<Engine>();
}
