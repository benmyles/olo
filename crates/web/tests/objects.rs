use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        properties::{Declaration, Families, Value},
        style::{self, Axes, Defaults, FontRequest, FontSlant},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
    html,
    text::{
        layout::{self, Object, Options},
        linebreak,
        prepare::{self, Control, ControlKind as K, Span, WhiteSpace},
        shaping::{self, BaseDirection, Engine},
    },
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
#[path = "support/paint_host.rs"]
mod paint_host;
fn scope() -> Scope {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
fn request() -> FontRequest {
    let syntax = Syntax::parse(Arc::from("'Olo Inline plain'"), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let Value::Families(f) = &values[0].value else { panic!() };
    let families: Families = f.clone();
    FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families, axes: Axes::default() }
}
fn text(e: &mut Engine, s: &Scope, source: &str, rtl: bool, wrap: bool, budget: usize) -> Arc<layout::Text> {
    text_requested(e, s, source, rtl, wrap, budget, request())
}
fn text_requested(e: &mut Engine, s: &Scope, source: &str, rtl: bool, wrap: bool, budget: usize, request: FontRequest) -> Arc<layout::Text> {
    let controls: Box<[_]> = source
        .char_indices()
        .filter_map(|(i, c)| match c {
            '\u{fffc}' => Some(Control::new(i as u32, K::Object).unwrap()),
            '\n' => Some(Control::new(i as u32, K::Break).unwrap()),
            _ => None,
        })
        .collect();
    let input: Arc<str> = Arc::from(source);
    let spans: Arc<[Span]> = if source.is_empty() { Arc::from([]) } else { Arc::from([Span { range: 0..source.len() as u32, whitespace: WhiteSpace { wrap, ..Default::default() } }]) };
    let mut p = prepare::Job::new(input, spans, Default::default(), None).unwrap().with_controls(controls).unwrap();
    while !p.step(budget).unwrap().finished {}
    let p = Arc::new(p.finish().unwrap());
    let positions: Box<[_]> = p.text().char_indices().filter_map(|(i, c)| (c == '\u{fffc}').then_some(i as u32)).collect();
    let mut sh = shaping::ParagraphJob::for_layout(e, s, request, p.text().clone(), if rtl { BaseDirection::Rtl } else { BaseDirection::Ltr }, Default::default(), None)
        .unwrap()
        .with_inline_objects(positions)
        .unwrap();
    while !sh.step(budget).unwrap().finished {}
    let sh = Arc::new(sh.finish().unwrap());
    assert_eq!(sh.report.missing_clusters, 0);
    let mut br = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
    while !br.step(budget).unwrap().finished {}
    let mut j = layout::TextJob::new(e, p, sh, Arc::new(br.finish().unwrap()), Default::default(), None).unwrap();
    while !j.step(budget).unwrap().finished {}
    j.finish().unwrap()
}
fn fit(e: &mut Engine, t: Arc<layout::Text>, objects: Vec<Object>, width: f64, rtl: bool, budget: usize) -> layout::Layout {
    let mut j = layout::Job::new(e, t, Options { width, rtl, ..Default::default() }, Default::default(), None).unwrap().with_objects(objects).unwrap();
    while !j.step(budget).unwrap().finished {}
    j.finish().unwrap()
}
#[test]
fn atomic_items_fit_without_glyphs_and_expand_only_their_own_line() {
    let s = scope();
    let mut e = Engine::default();
    for budget in [1, 128] {
        let t = text(&mut e, &s, "A\u{fffc}B\nC", false, true, budget);
        assert_eq!(t.object_count(), 1);
        let object_run = t.paragraph().runs.iter().find(|r| r.range == (1..4)).unwrap();
        assert!(object_run.glyphs.is_empty());
        let o = Object { at: 1, advance: 13.125, ascent: 30., descent: 0., placeholder: false, align: Default::default() };
        let l = fit(&mut e, t.clone(), vec![o], 100., false, budget);
        assert_eq!(l.lines().len(), 2);
        assert_eq!(l.lines()[0].advance, 53.125);
        assert_eq!(l.baseline(0), Some(30.));
        assert_eq!(l.baseline(1), Some(50.));
        assert_eq!(l.height(), 54.);
        assert_eq!(l.object_fragments()[0].x, 20.);
        assert_eq!(l.object_fragments()[0].line, 0);
        let narrow = fit(&mut e, t.clone(), vec![o], 40., false, budget);
        assert_eq!(narrow.lines().len(), 3);
        assert_eq!(narrow.object_fragments()[0].line, 0);
        assert_eq!(narrow.lines()[0].advance, 33.125);
        assert_eq!(narrow.baseline(2), Some(70.));
        assert_eq!(narrow.height(), 74.);
    }
}
#[test]
fn objects_keep_bidi_order_nowrap_and_fractional_advances() {
    let s = scope();
    let mut e = Engine::default();
    for rtl in [false, true] {
        for wrap in [false, true] {
            let t = text(&mut e, &s, "\u{fffc}\u{fffc}", rtl, wrap, 1);
            let os = vec![
                Object { at: 0, advance: 13.125, ascent: 5., descent: 0., placeholder: false, align: Default::default() },
                Object { at: 3, advance: 7.75, ascent: 8., descent: 0., placeholder: false, align: Default::default() },
            ];
            let l = fit(&mut e, t, os, 15., rtl, 1);
            assert_eq!(l.lines().len(), if wrap { 2 } else { 1 });
            assert_eq!(l.height(), if wrap { 40. } else { 20. });
            if !wrap {
                let objects = l.object_fragments();
                assert_eq!(objects[0].at, if rtl { 3 } else { 0 });
                assert_eq!(l.lines()[0].advance, 20.875);
            }
        }
    }
}
#[test]
fn object_metadata_and_metrics_are_mandatory_bounded_and_cancellable() {
    let s = scope();
    let mut e = Engine::default();
    let t = text(&mut e, &s, "\u{fffc}", false, true, 1);
    let mut missing = layout::Job::new(&mut e, t.clone(), Default::default(), Default::default(), None).unwrap();
    assert!(matches!(missing.step(1), Err(layout::Error::Invalid)));
    assert!(matches!(missing.step(1), Err(layout::Error::Failed)));
    drop(missing);
    for input in [
        vec![],
        vec![Object { at: 1, advance: 1., ascent: 1., descent: 0., placeholder: false, align: Default::default() }],
        vec![Object { at: 0, advance: f64::NAN, ascent: 1., descent: 0., placeholder: false, align: Default::default() }],
    ] {
        assert!(layout::Job::new(&mut e, t.clone(), Default::default(), Default::default(), None).unwrap().with_objects(input).is_err());
    }
    let object = Object { at: 0, advance: 1., ascent: 1., descent: 0., placeholder: false, align: Default::default() };
    assert!(matches!(
        layout::Job::new(&mut e, t.clone(), Default::default(), layout::Limits { fragments: 0, ..Default::default() }, None).unwrap().with_objects(vec![object]),
        Err(layout::Error::Limit)
    ));
    let cancel = AtomicBool::new(false);
    let job = layout::Job::new(&mut e, t, Default::default(), Default::default(), Some(&cancel)).unwrap();
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(matches!(job.with_objects(vec![object]), Err(layout::Error::Cancelled)));
}

fn parent_style(c: &serde_json::Value, scope: &Scope, scale: f64, budget: usize) -> (style::StyleTree, olo_web::dom::NodeId) {
    let body = c["body"].as_str().unwrap();
    let css = c["css"].as_str().unwrap();
    let document = html::parse(Arc::from(format!("<!doctype html><html id=root><head></head><body id=body>{body}</body></html>")), Default::default(), None).unwrap().document;
    let base = include_str!("flow-fixtures/base.css");
    let syntax = Syntax::parse(Arc::from(format!("{base}{css}")), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default());
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = builder.finish(None).unwrap();
    let host = paint_host::Scaled::new(scope.clone(), scale);
    let defaults = Defaults::new(&host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&document, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(budget, |_| &host).unwrap().finished {}
    let styles = job.finish().unwrap();
    assert_eq!(styles.report().stylesheet_issues, 0);
    assert_eq!(styles.report().inline_invalid, 0);
    assert_eq!(styles.report().inline_unsupported, 0);
    let node = styles.iter().find(|(n, _)| document.attribute(*n, "id").unwrap() == Some("p")).unwrap().0;
    (styles, node)
}

#[test]
fn independent_chrome_atomic_object_positions_and_line_metrics() {
    let s = scope();
    let mut e = Engine::default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/object-fixtures");
    let mut failures = Vec::new();
    for scale in [1., 1.25, 1.5, 1.75, 2.] {
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(format!("chrome-{scale}.json"))).unwrap()).unwrap();
        assert_eq!(report["devicePixelRatio"].as_f64().unwrap(), scale);
        for c in report["rows"].as_array().unwrap() {
            for budget in [1, 128] {
                let source = c["source"].as_str().unwrap();
                let rtl = c["rtl"].as_bool().unwrap();
                let (styles, node) = parent_style(c, &s, scale, budget);
                let style = styles.iter().find(|(n, _)| *n == node).unwrap().1;
                let t = text_requested(&mut e, &s, source, rtl, c["wrap"].as_bool().unwrap(), budget, style.font().request.clone());
                let objects = c["objects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| Object {
                        at: o["at"].as_u64().unwrap() as u32,
                        advance: o["width"].as_f64().unwrap() + o["ml"].as_f64().unwrap() + o["mr"].as_f64().unwrap(),
                        ascent: o["height"].as_f64().unwrap() + o["mt"].as_f64().unwrap() + o["mb"].as_f64().unwrap(),
                        descent: 0.,
                        placeholder: false,
                        align: Default::default(),
                    })
                    .collect();
                let mut j = layout::Job::for_style_with_spacing_at_scale(
                    &mut e,
                    t,
                    style,
                    &s,
                    (c["layoutViewport"]["width"].as_f64().unwrap() * scale).round() / scale,
                    scale,
                    Default::default(),
                    None,
                    Vec::new(),
                )
                .unwrap()
                .with_objects(objects)
                .unwrap();
                while !j.step(budget).unwrap().finished {}
                let l = j.finish().unwrap();
                let convert = |v: f64| f64::from((v * scale) as f32 * (1f32 / scale as f32));
                let mut check = |field: String, value: f64, expected: f64, range: bool| {
                    let value = convert(value);
                    let tolerance = if range { 1. / (64. * scale) + 1e-5 } else { 1e-5 };
                    if (value - expected).abs() > tolerance {
                        failures.push(format!("{scale}:{} {field} {value} != {expected}", c["name"].as_str().unwrap()));
                    }
                };
                check("height".into(), l.height(), c["boxes"]["p"]["height"].as_f64().unwrap(), false);
                assert_eq!(l.object_fragments().len(), c["objects"].as_array().unwrap().len());
                for (i, o) in c["objects"].as_array().unwrap().iter().enumerate() {
                    let placed = l.object_fragments().iter().find(|p| p.at == o["at"].as_u64().unwrap() as u32).unwrap();
                    let expected = &c["boxes"][format!("o{i}")];
                    check(format!("o{i}.x"), placed.x + o["ml"].as_f64().unwrap(), expected["x"].as_f64().unwrap(), false);
                    check(format!("o{i}.y"), l.baseline(placed.line as usize).unwrap() - placed.ascent + o["mt"].as_f64().unwrap(), expected["y"].as_f64().unwrap(), false);
                }
                let mut letters = std::collections::BTreeMap::new();
                for (i, line) in l.lines().iter().enumerate() {
                    for f in &l.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
                        let r = l.run(f).unwrap();
                        let unit = 20. / f64::from(r.font.units_per_em());
                        let mut pen = 0i64;
                        for g in &r.glyphs[f.glyphs.start as usize..f.glyphs.end as usize] {
                            let ch = r.source[g.cluster as usize..].chars().next().unwrap();
                            if ch.is_ascii_uppercase() {
                                letters.insert(ch, (f.x + (pen + i64::from(g.x_offset)) as f64 * unit, l.baseline(i).unwrap() - 16.));
                            }
                            pen += i64::from(g.x_advance);
                        }
                    }
                }
                assert_eq!(letters.len(), c["letters"].as_array().unwrap().len());
                for expected in c["letters"].as_array().unwrap() {
                    let letter = expected["letter"].as_str().unwrap().chars().next().unwrap();
                    let (x, y) = letters[&letter];
                    check(format!("{letter}.x"), x, expected["x"].as_f64().unwrap(), true);
                    check(format!("{letter}.y"), y, expected["y"].as_f64().unwrap(), false);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn independent_object_capture_provenance_is_complete() {
    use sha2::{Digest, Sha256};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/object-fixtures");
    let metadata: serde_json::Value = serde_json::from_str(include_str!("object-fixtures/SOURCE.json")).unwrap();
    let hash = |path: &Path| format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()));
    for (file, expected) in metadata["inputs"].as_object().unwrap() {
        assert_eq!(hash(&root.join(file)), expected.as_str().unwrap());
    }
    for key in ["font", "base"] {
        assert_eq!(hash(&root.join(metadata[key]["path"].as_str().unwrap())), metadata[key]["sha256"].as_str().unwrap());
    }
    let inputs: serde_json::Value = serde_json::from_str(include_str!("object-fixtures/inputs.json")).unwrap();
    assert_eq!(inputs.as_array().unwrap().len(), 76);
    assert_eq!(metadata["reports"].as_object().unwrap().len(), 5);
    for (scale, report) in metadata["reports"].as_object().unwrap() {
        let path = root.join(report["file"].as_str().unwrap());
        assert_eq!(hash(&path), report["sha256"].as_str().unwrap());
        let capture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(capture["devicePixelRatio"].as_f64().unwrap(), scale.parse::<f64>().unwrap());
        assert_eq!(capture["userAgent"], report["userAgent"]);
        assert_eq!(capture["base"].as_str().unwrap(), include_str!("flow-fixtures/base.css"));
        let rows = capture["rows"].as_array().unwrap();
        assert_eq!(rows.len(), inputs.as_array().unwrap().len());
        for (input, row) in inputs.as_array().unwrap().iter().zip(rows) {
            for (key, value) in input.as_object().unwrap() {
                assert_eq!(&row[key], value, "{scale}:{}:{key}", input["name"]);
            }
        }
    }
}

#[test]
fn literal_object_character_remains_font_text_and_semantic_objects_isolate_context() {
    let s = scope();
    let mut e = Engine::default();
    let source: Arc<str> = Arc::from("A\u{fffc}B");
    let mut ordinary = shaping::ParagraphJob::for_layout(&mut e, &s, request(), source.clone(), BaseDirection::Ltr, Default::default(), None).unwrap();
    while !ordinary.step(1).unwrap().finished {}
    let ordinary = ordinary.finish().unwrap();
    assert!(ordinary.runs.iter().any(|r| r.glyphs.iter().any(|g| g.cluster == 1)));
    let t = text(&mut e, &s, &source, false, true, 1);
    let runs = &t.paragraph().runs;
    assert_eq!(runs.iter().map(|r| r.range.clone()).collect::<Vec<_>>(), [0..1, 1..4, 4..5]);
    assert!(runs[1].glyphs.is_empty());
    for positions in [vec![0], vec![2], vec![1, 1], vec![1, 0]] {
        let result = shaping::ParagraphJob::for_layout(&mut e, &s, request(), source.clone(), BaseDirection::Ltr, Default::default(), None).unwrap().with_inline_objects(positions.into_boxed_slice());
        assert!(matches!(result, Err(shaping::Error::Invalid)));
    }
}
