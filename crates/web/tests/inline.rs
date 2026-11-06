use olo_web::{
    css::{
        Syntax,
        properties::{Declaration, Families, Value},
        style::{Axes, FontRequest, FontSlant},
    },
    font::{Builder, FontSet, GenericPolicy, Scope},
    text::{
        layout::{self, Align, Options, Text},
        linebreak,
        prepare::{self, Collapse, Prepared, Span, WhiteSpace},
        shaping::{self, BaseDirection, Engine, Paragraph},
    },
};
use std::{path::Path, sync::Arc};
fn scope() -> Scope {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut b = Builder::new(Default::default());
    for path in [
        "data/fonts/NotoSans[wght].ttf",
        "crates/web/tests/shaping-fixtures/fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf",
        "crates/web/tests/shaping-fixtures/fonts/notosansarabic/NotoSansArabic[wdth,wght].ttf",
    ] {
        b.load_file(&root.join(path), |_| {}, None).unwrap();
    }
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Noto Sans", "Noto Sans Hebrew", "Noto Sans Arabic"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
fn family(name: &str) -> Families {
    let s = Syntax::parse(Arc::from(format!("'{name}'")), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", s.components(), false, Default::default(), None).unwrap() else { panic!() };
    let Value::Families(f) = &values[0].value else { panic!() };
    f.clone()
}
fn prepared(text: &str, whitespace: WhiteSpace) -> Arc<Prepared> {
    let spans: Arc<[Span]> = if text.is_empty() { Arc::from([]) } else { Arc::from([Span { range: 0..text.len() as u32, whitespace }]) };
    let mut job = prepare::Job::new(Arc::from(text), spans, Default::default(), None).unwrap();
    while !job.step(64).unwrap().finished {}
    Arc::new(job.finish().unwrap())
}
fn shaped(engine: &mut Engine, scope: &Scope, p: &Prepared, base: BaseDirection) -> Arc<Paragraph> {
    shaped_family(engine, scope, p, base, "Noto Sans")
}
fn shaped_family(engine: &mut Engine, scope: &Scope, p: &Prepared, base: BaseDirection, name: &str) -> Arc<Paragraph> {
    let request = FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families: family(name), axes: Axes::default() };
    let mut job = shaping::ParagraphJob::for_layout(engine, scope, request, p.text().clone(), base, Default::default(), None).unwrap();
    while !job.step(16).unwrap().finished {}
    Arc::new(job.finish().unwrap())
}
fn fixture_scope(name: &str) -> Scope {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/inline-fixtures/{name}.ttf")), |_| {}, None).unwrap();
    FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&[&format!("Olo Inline {name}")]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
}
fn fixture_text(e: &mut Engine, name: &str, input: &str, w: WhiteSpace) -> Arc<Text> {
    let scope = fixture_scope(name);
    let p = prepared(input, w);
    let s = shaped_family(e, &scope, &p, BaseDirection::Ltr, &format!("Olo Inline {name}"));
    let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
    while !b.step(64).unwrap().finished {}
    let mut job = layout::TextJob::new(e, p, s, Arc::new(b.finish().unwrap()), Default::default(), None).unwrap();
    while !job.step(1).unwrap().finished {}
    job.finish().unwrap()
}
fn text(engine: &mut Engine, scope: &Scope, input: &str, w: WhiteSpace, base: BaseDirection) -> Arc<Text> {
    let p = prepared(input, w);
    let s = shaped(engine, scope, &p, base);
    let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
    while !b.step(64).unwrap().finished {}
    let b = Arc::new(b.finish().unwrap());
    let mut job = layout::TextJob::new(engine, p, s, b, Default::default(), None).unwrap();
    while !job.step(2).unwrap().finished {}
    job.finish().unwrap()
}
fn fit(engine: &mut Engine, text: Arc<Text>, options: Options, budget: usize) -> layout::Layout {
    let mut job = layout::Job::new(engine, text, options, Default::default(), None).unwrap();
    while !job.step(budget).unwrap().finished {}
    job.finish().unwrap()
}
fn visible(l: &layout::Layout) -> Vec<&str> {
    l.lines().iter().map(|line| &l.text().prepared().text()[line.text.start as usize..line.text.end as usize]).collect()
}
#[test]
fn wraps_real_glyphs_trims_edges_and_reuses_width_independent_text() {
    let scope = scope();
    let mut e = Engine::default();
    let t = text(&mut e, &scope, "  abc abc abc  ", WhiteSpace::default(), BaseDirection::Ltr);
    let l = fit(&mut e, t.clone(), Options { width: 40., ..Default::default() }, 1);
    assert_eq!(visible(&l), vec!["abc", "abc", "abc"]);
    assert!(l.lines().iter().all(|l| !l.overflow && l.advance <= 40.));
    assert_eq!(l.height(), 60.);
    assert_eq!(l.baseline(0), Some(16.));
    assert_eq!(l.baseline(2), Some(56.));
    assert_eq!(l.baseline(3), None);
    for line in l.lines() {
        for f in &l.fragments()[line.fragments.start as usize..line.fragments.end as usize] {
            let r = l.run(f).unwrap();
            assert!(Arc::ptr_eq(&r.source, t.prepared().text()));
            assert!(f.glyphs.end as usize <= r.glyphs.len());
        }
    }
    let wide = fit(&mut e, t.clone(), Options { width: 1000., ..Default::default() }, 64);
    assert_eq!(visible(&wide), vec!["abc abc abc"]);
    assert!(Arc::ptr_eq(wide.text(), &t));
    let overflowing = fit(&mut e, t, Options { width: 1., ..Default::default() }, 2);
    assert_eq!(visible(&overflowing), vec!["abc", "abc", "abc"]);
    assert!(overflowing.lines().iter().all(|l| l.overflow));
}

#[test]
fn inline_edge_boundaries_isolate_ligatures_and_later_reshape_context() {
    let scope = fixture_scope("ligature");
    let mut e = Engine::default();
    let t = fixture_text(&mut e, "ligature", "fi", WhiteSpace::default());
    assert_eq!(t.paragraph().runs[0].glyphs.len(), 1);
    let p = t.prepared().clone();
    let mut job = shaping::ParagraphJob::for_layout(&mut e, &scope, t.paragraph().request().clone(), p.text().clone(), BaseDirection::Ltr, Default::default(), None)
        .unwrap()
        .with_shaping_breaks(vec![1].into_boxed_slice())
        .unwrap();
    while !job.step(1).unwrap().finished {}
    let shaped = job.finish().unwrap();
    assert_eq!(shaped.runs.len(), 2);
    assert_eq!(shaped.runs.iter().map(|r| r.glyphs.len()).sum::<usize>(), 2);
    assert_eq!(shaped.context_range(0..1).unwrap(), 0..1);
    assert_eq!(shaped.context_range(1..2).unwrap(), 1..2);
    assert!(shaped.context_range(0..2).is_err());
    for boundaries in [vec![0], vec![2], vec![1, 1], vec![1, 0]] {
        let job = shaping::ParagraphJob::for_layout(&mut e, &scope, t.paragraph().request().clone(), p.text().clone(), BaseDirection::Ltr, Default::default(), None).unwrap();
        assert!(job.with_shaping_breaks(boundaries.into_boxed_slice()).is_err());
    }
    let job = shaping::ParagraphJob::for_layout(&mut e, &scope, t.paragraph().request().clone(), Arc::from("é"), BaseDirection::Ltr, Default::default(), None).unwrap();
    assert!(job.with_shaping_breaks(vec![1].into_boxed_slice()).is_err());
}
#[test]
fn preserved_lf_no_wrap_blank_lines_and_phantom_whitespace() {
    let scope = scope();
    let mut e = Engine::default();
    for (input, w, expected) in [
        ("abc abc\n\nabc\n", WhiteSpace { collapse: Collapse::Preserve, wrap: false }, vec!["abc abc", "", "abc"]),
        (" abc \n abc ", WhiteSpace { collapse: Collapse::PreserveBreaks, wrap: true }, vec!["abc", "abc"]),
        ("abc abc", WhiteSpace { wrap: false, ..Default::default() }, vec!["abc abc"]),
        ("  ", WhiteSpace::default(), vec![]),
        ("", WhiteSpace::default(), vec![]),
        ("\n", WhiteSpace { collapse: Collapse::Preserve, wrap: false }, vec![""]),
    ] {
        let t = text(&mut e, &scope, input, w, BaseDirection::Ltr);
        let l = fit(&mut e, t, Options { width: 40., ..Default::default() }, 2);
        assert_eq!(visible(&l), expected, "{input:?} {w:?}");
    }
    let p = prepared("\n\n", WhiteSpace { collapse: Collapse::Preserve, wrap: false });
    let mut fresh = Engine::default();
    let para = shaped(&mut fresh, &scope, &p, BaseDirection::Ltr);
    assert_eq!(para.report.hard_breaks, 2);
    assert_eq!(para.report.missing_clusters, 0);
    assert!(para.runs.is_empty());
    assert_eq!(fresh.cache_stats().faces, 0);
}
#[test]
fn bidi_visual_slices_and_alignment_follow_each_line() {
    let scope = scope();
    let mut e = Engine::default();
    let t = text(&mut e, &scope, "שלום עולם שלום", WhiteSpace::default(), BaseDirection::Rtl);
    let l = fit(&mut e, t, Options { width: 70., rtl: true, ..Default::default() }, 1);
    assert_eq!(visible(&l), vec!["שלום", "עולם", "שלום"]);
    for line in l.lines() {
        assert_eq!(line.offset, 70. - line.advance);
        let f = &l.fragments()[line.fragments.start as usize..line.fragments.end as usize];
        assert!(f.iter().all(|f| f.level % 2 == 1));
        assert!(f.windows(2).all(|w| w[0].x <= w[1].x));
    }
    let t = text(&mut e, &scope, "abc", WhiteSpace::default(), BaseDirection::Ltr);
    for (align, rtl) in [(Align::Left, false), (Align::Right, false), (Align::Center, false), (Align::Start, true), (Align::End, true)] {
        let l = fit(&mut e, t.clone(), Options { width: 100., align, rtl, ..Default::default() }, 32);
        let line = &l.lines()[0];
        let expected = match align {
            Align::Left | Align::End => 0.,
            Align::Right | Align::Start => 100. - line.advance,
            Align::Center => (100. - line.advance) / 2.,
        };
        assert_eq!(line.offset, expected);
    }
}

#[test]
fn unsafe_ligature_cuts_nonmonotonic_widths_and_deleted_prefixes() {
    let mut e = Engine::default();
    let t = fixture_text(&mut e, "ligature", "A B C", WhiteSpace::default());
    assert!(t.paragraph().runs.iter().any(|r| r.glyphs.iter().any(|g| g.id == 9)));
    for (width, expected, advances) in [(15., vec!["A B", "C"], vec![10., 20.]), (5., vec!["A", "B", "C"], vec![20., 20., 20.]), (50., vec!["A B C"], vec![40.])] {
        let l = fit(&mut e, t.clone(), Options { width, ..Default::default() }, 1);
        assert_eq!(visible(&l), expected);
        assert_eq!(l.lines().iter().map(|l| l.advance).collect::<Vec<_>>(), advances);
    }
    let t = fixture_text(&mut e, "deletion", "D E", WhiteSpace::default());
    for width in [5., 25.] {
        let l = fit(&mut e, t.clone(), Options { width, ..Default::default() }, 1);
        assert_eq!(visible(&l), vec!["D", "E"]);
        assert_eq!(l.lines().iter().map(|l| l.advance).collect::<Vec<_>>(), vec![20., 20.]);
    }
}

#[test]
fn preserved_spaces_hang_conditionally_and_break_spaces_stay_measured() {
    let mut e = Engine::default();
    for (mode, width, expected, advances, content) in [
        (Collapse::Preserve, 25., vec!["A  ", "B"], vec![20., 20.], vec![40., 20.]),
        (Collapse::Preserve, 100., vec!["A  B"], vec![60.], vec![60.]),
        (Collapse::BreakSpaces, 35., vec!["A ", " B"], vec![30., 30.], vec![30., 30.]),
        (Collapse::BreakSpaces, 20., vec!["A ", " ", "B"], vec![30., 10., 20.], vec![30., 10., 20.]),
    ] {
        let t = fixture_text(&mut e, "plain", "A  B", WhiteSpace { collapse: mode, wrap: true });
        let l = fit(&mut e, t, Options { width, ..Default::default() }, 1);
        assert_eq!(visible(&l), expected, "{mode:?} {width}");
        assert_eq!(l.lines().iter().map(|l| l.advance).collect::<Vec<_>>(), advances);
        assert_eq!(l.lines().iter().map(|l| l.content_advance).collect::<Vec<_>>(), content);
    }
    for (width, advance) in [(100., 40.), (25., 20.)] {
        let t = fixture_text(&mut e, "plain", "A  ", WhiteSpace { collapse: Collapse::Preserve, wrap: true });
        let l = fit(&mut e, t, Options { width, ..Default::default() }, 1);
        assert_eq!(l.lines()[0].advance, advance);
        assert_eq!(l.lines()[0].content_advance, 40.);
    }
}

#[test]
fn independent_native_harfbuzz_line_oracle() {
    use sha2::{Digest, Sha256};
    let references: serde_json::Value = serde_json::from_str(include_str!("inline-fixtures/harfbuzz.json")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, hash) in references["fonts"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap());
    }
    let general = scope();
    let mut e = Engine::default();
    for case in references["cases"].as_array().unwrap() {
        let name = case["font"].as_str().unwrap();
        let fixture;
        let scope = if ["plain", "ligature", "deletion"].contains(&name) {
            fixture = fixture_scope(name);
            &fixture
        } else {
            &general
        };
        let rtl = case["rtl"].as_bool().unwrap();
        let p = prepared(case["text"].as_str().unwrap(), WhiteSpace::default());
        let s = shaped_family(&mut e, scope, &p, if rtl { BaseDirection::Rtl } else { BaseDirection::Ltr }, case["family"].as_str().unwrap());
        let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
        while !b.step(2).unwrap().finished {}
        let mut t = layout::TextJob::new(&mut e, p, s, Arc::new(b.finish().unwrap()), Default::default(), None).unwrap();
        while !t.step(1).unwrap().finished {}
        let t = t.finish().unwrap();
        for budget in [1, 128] {
            let l = fit(&mut e, t.clone(), Options { width: case["width"].as_f64().unwrap(), rtl, ..Default::default() }, budget);
            let expected = case["lines"].as_array().unwrap();
            assert_eq!(l.lines().len(), expected.len(), "{case}");
            for (line, reference) in l.lines().iter().zip(expected) {
                assert_eq!(line.text, reference["start"].as_u64().unwrap() as u32..reference["end"].as_u64().unwrap() as u32, "{case}");
                assert!((line.advance - reference["advance"].as_f64().unwrap()).abs() < 1e-9, "{} {reference} {case}", line.advance);
                let actual: Vec<_> = l.fragments()[line.fragments.start as usize..line.fragments.end as usize]
                    .iter()
                    .flat_map(|f| l.run(f).unwrap().glyphs[f.glyphs.start as usize..f.glyphs.end as usize].iter().map(|g| (g.id, g.cluster, g.x_advance, g.y_advance, g.x_offset, g.y_offset)))
                    .collect();
                let expected: Vec<_> = reference["glyphs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|g| {
                        (
                            g["id"].as_u64().unwrap() as u32,
                            g["cluster"].as_u64().unwrap() as u32,
                            g["x_advance"].as_i64().unwrap() as i32,
                            g["y_advance"].as_i64().unwrap() as i32,
                            g["x_offset"].as_i64().unwrap() as i32,
                            g["y_offset"].as_i64().unwrap() as i32,
                        )
                    })
                    .collect();
                assert_eq!(actual, expected, "{case}");
            }
        }
    }
}

#[test]
fn cancellation_quotas_poisoning_and_invalid_options() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut e = Engine::default();
    let t = fixture_text(&mut e, "plain", "A B C", WhiteSpace::default());
    let cancelled = AtomicBool::new(true);
    assert!(matches!(layout::Job::new(&mut e, t.clone(), Options::default(), Default::default(), Some(&cancelled)), Err(layout::Error::Cancelled)));
    cancelled.store(false, Ordering::Relaxed);
    let mut j = layout::Job::new(&mut e, t.clone(), Options { width: 25., ..Default::default() }, Default::default(), Some(&cancelled)).unwrap();
    assert!(j.completed_lines().unwrap().is_empty());
    assert!(!j.step(0).unwrap().finished);
    j.step(1).unwrap();
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(j.step(1), Err(layout::Error::Cancelled)));
    assert!(matches!(j.completed_lines(), Err(layout::Error::Failed)));
    assert!(matches!(j.step(1), Err(layout::Error::Failed)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    for limits in [layout::Limits { lines: 0, ..Default::default() }, layout::Limits { fragments: 0, ..Default::default() }, layout::Limits { work: 0, ..Default::default() }] {
        let mut j = layout::Job::new(&mut e, t.clone(), Options::default(), limits, None).unwrap();
        assert!(matches!(j.step(64), Err(layout::Error::Limit)));
        assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    }
    for width in [f64::NAN, f64::INFINITY, -1.] {
        assert!(matches!(layout::Job::new(&mut e, t.clone(), Options { width, ..Default::default() }, Default::default(), None), Err(layout::Error::Invalid)));
    }
    assert!(matches!(layout::Job::new(&mut e, t.clone(), Options::default(), layout::Limits { points: 0, ..Default::default() }, None), Err(layout::Error::Limit)));
    let j = layout::Job::new(&mut e, t, Options::default(), Default::default(), None).unwrap();
    assert!(matches!(j.finish(), Err(layout::Error::Incomplete)));
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Text>();
    send_sync::<layout::Layout>();
}

#[path = "support/native_font_host.rs"]
mod native_host;
#[test]
fn computed_styles_bind_scope_direction_whitespace_and_line_metrics() {
    use olo_web::{
        css::{
            cascade::{Origin, SheetBuilder},
            style::{self, Defaults},
        },
        html,
    };
    let sc = scope();
    let host = native_host::NativeHost(sc.clone());
    let mut e = Engine::default();
    for (declarations, unsupported) in [
        ("text-align:center;text-align-last:right", false),
        ("direction:rtl", false),
        ("letter-spacing:1px", true),
        ("word-break:break-all", true),
        ("writing-mode:vertical-rl", true),
        ("text-align:justify", true),
    ] {
        let doc = html::parse(Arc::from("<p id=target>abc abc abc</p>"), Default::default(), None).unwrap().document;
        let syntax = Syntax::parse(Arc::from(format!("p{{font-family:'Noto Sans';font-size:20px;line-height:30px;{declarations}}}")), Default::default(), None).unwrap();
        let mut builder = SheetBuilder::new(Default::default());
        builder.add_sheet(&syntax, Origin::Author, None).unwrap();
        let rules = builder.finish(None).unwrap();
        let defaults = Defaults::new(&host, Default::default(), None).unwrap();
        let mut j = style::Job::new(&doc, &rules, defaults, Default::default(), Default::default(), None).unwrap();
        while !j.step(16, |_| &host).unwrap().finished {}
        let tree = j.finish().unwrap();
        let style = tree.iter().find(|(n, _)| doc.attribute(*n, "id").unwrap() == Some("target")).unwrap().1;
        let options = layout::Options::for_style(style, &sc, 50., None);
        if unsupported {
            assert!(matches!(options, Err(layout::Error::Unsupported(_))), "{declarations}");
            continue;
        }
        let p = prepared("abc abc abc", WhiteSpace::for_style(style).unwrap());
        let mut j = shaping::ParagraphJob::for_layout_style(&mut e, &sc, style, p.text().clone(), Default::default(), None).unwrap();
        while !j.step(16).unwrap().finished {}
        let s = Arc::new(j.finish().unwrap());
        let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
        while !b.step(64).unwrap().finished {}
        let mut j = layout::TextJob::new(&mut e, p, s, Arc::new(b.finish().unwrap()), Default::default(), None).unwrap();
        while !j.step(16).unwrap().finished {}
        let t = j.finish().unwrap();
        let mut j = layout::Job::for_style(&mut e, t.clone(), style, &sc, 50., Default::default(), None).unwrap();
        while !j.step(16).unwrap().finished {}
        let l = j.finish().unwrap();
        assert_eq!(l.lines().len(), 3);
        assert_eq!(l.height(), 90.);
        assert_eq!(l.baseline(1).unwrap() - l.baseline(0).unwrap(), 30.);
        if declarations.starts_with("text-align") {
            assert_eq!(l.lines()[0].offset, (50. - l.lines()[0].advance) / 2.);
            assert_eq!(l.lines()[2].offset, 50. - l.lines()[2].advance);
        }
        let other = scope();
        assert!(matches!(layout::Job::for_style(&mut e, t, style, &other, 50., Default::default(), None), Err(layout::Error::Stale)));
    }
}

#[test]
fn text_builder_validates_provenance_and_caps_before_publication() {
    let sc = fixture_scope("plain");
    let mut e = Engine::default();
    let p = prepared("A B C", WhiteSpace::default());
    let s = shaped_family(&mut e, &sc, &p, BaseDirection::Ltr, "Olo Inline plain");
    let mut b = linebreak::Job::new(p.text().clone(), Default::default(), None).unwrap();
    while !b.step(64).unwrap().finished {}
    let b = Arc::new(b.finish().unwrap());
    for limits in [layout::Limits { clusters: 0, ..Default::default() }, layout::Limits { text_bytes: 1, ..Default::default() }] {
        assert!(matches!(layout::TextJob::new(&mut e, p.clone(), s.clone(), b.clone(), limits, None), Err(layout::Error::Limit)));
    }
    let mut j = layout::TextJob::new(&mut e, p.clone(), s.clone(), b.clone(), layout::Limits { points: 0, ..Default::default() }, None).unwrap();
    assert!(matches!(j.step(1), Err(layout::Error::Limit)));
    assert!(matches!(j.finish(), Err(layout::Error::Failed)));
    let different = prepared("A B C", WhiteSpace::default());
    assert!(matches!(layout::TextJob::new(&mut e, different, s.clone(), b.clone(), Default::default(), None), Err(layout::Error::Stale)));
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert!(matches!(layout::TextJob::new(&mut e, p.clone(), s.clone(), b.clone(), Default::default(), Some(&cancelled)), Err(layout::Error::Cancelled)));
    let j = layout::TextJob::new(&mut e, p, s, b, Default::default(), None).unwrap();
    assert!(matches!(j.finish(), Err(layout::Error::Incomplete)));
}
