//! Literal fontTools fixtures and independent OpenType/CSS matching facts.
use olo_web::{
    css::{
        Syntax,
        properties::{self as p, Declaration},
        style::{self, Axes, FontRequest, FontSlant, TextOrientation, WritingMode},
    },
    font::{self, Builder, Catalog, Error, FontSet, GenericPolicy, Limits, MatchOptions, Scope, Slope},
};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
fn path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/font-native-fixtures/fonts").join(name)
}
fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(path(name)).unwrap()
}
fn families(text: &str) -> p::Families {
    let syntax = Syntax::parse(Arc::from(text), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let p::Value::Families(f) = &values[0].value else { panic!() };
    f.clone()
}
fn request(text: &str) -> FontRequest {
    FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families: families(text), axes: Axes::default() }
}
fn catalog(names: &[&str], limits: Limits) -> Arc<Catalog> {
    let mut b = Builder::new(limits);
    for name in names {
        b.add_owned(bytes(name), None).unwrap();
    }
    b.finish(None).unwrap()
}
fn scope(c: Arc<Catalog>, default: &[&str]) -> Scope {
    FontSet::new(c, GenericPolicy::new(default).unwrap(), MatchOptions::default()).unwrap().scope("").unwrap()
}
fn selected(c: &Catalog, r: &FontRequest) -> font::Selection {
    c.select_family("Olo Match", r, MatchOptions::default(), None).unwrap().unwrap()
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-5, "actual {actual}, expected {expected}");
}
#[test]
fn width_style_weight_priority_and_directional_search() {
    let c = catalog(&["regular.ttf", "light.ttf", "medium.ttf", "bold.ttf", "narrow.ttf", "narrow-italic.ttf", "expanded.ttf", "italic.ttf", "oblique.ttf"], Limits::default());
    let mut r = request("'Olo Match'");
    for (weight, expected) in [(100., 300.), (350., 300.), (400., 400.), (450., 500.), (500., 500.), (550., 700.), (900., 700.)] {
        r.weight = weight;
        near(selected(&c, &r).weight, expected);
    }
    r.weight = 700.;
    r.slant = FontSlant::Italic;
    r.width = 75.;
    let s = selected(&c, &r);
    near(s.width, 75.);
    near(s.weight, 900.);
    assert_eq!(s.slope, Slope::Italic(1.));
    r.slant = FontSlant::Normal;
    for (width, expected) in [(80., 75.), (99., 75.), (100., 100.), (101., 125.), (130., 125.)] {
        r.width = width;
        near(selected(&c, &r).width, expected);
    }
    r.width = 100.;
    r.weight = 700.;
    r.slant = FontSlant::Italic;
    let s = selected(&c, &r);
    near(s.weight, 400.);
    assert_eq!(s.slope, Slope::Italic(1.));
    assert!(s.synthesis.bold);
    r.slant = FontSlant::Oblique(14.);
    assert_eq!(selected(&c, &r).slope, Slope::Oblique(20.));
    r.slant = FontSlant::Normal;
    near(selected(&c, &r).weight, 700.);
}
#[test]
fn synthesis_policy_and_explicit_unsupported_new_slopes() {
    let c = catalog(&["regular.ttf"], Limits::default());
    let mut r = request("'Olo Match'");
    r.weight = 700.;
    r.slant = FontSlant::Italic;
    let s = selected(&c, &r);
    assert!(s.synthesis.bold);
    assert_eq!(s.synthesis.shear_degrees, Some(14.));
    let s = c.select_family("Olo Match", &r, MatchOptions { synthesize_style: false, synthesize_weight: false, ..Default::default() }, None).unwrap().unwrap();
    assert_eq!(s.synthesis, font::Synthesis::default());
    for slant in [FontSlant::Left, FontSlant::Right] {
        r.slant = slant;
        assert!(matches!(c.select_family("Olo Match", &r, Default::default(), None), Err(Error::Unsupported(_))));
    }
}
#[test]
fn localized_family_aliases_full_fold_without_normalization_or_fullname_matching() {
    let c = catalog(&["unicode.ttf", "composed.ttf", "decomposed.ttf"], Limits::default());
    let r = request("serif");
    let find = |name| c.select_family(name, &r, Default::default(), None).unwrap().map(|s| s.face);
    let id = find("Straße").unwrap();
    for name in ["STRASSE", "strasse", "書体", "Σ", "σ", "ς"] {
        assert_eq!(find(name), Some(id), "{name}");
    }
    assert_ne!(find("Å"), find("A\u{30a}"));
    assert!(find("Å").is_some());
    assert!(find("A\u{30a}").is_some());
    for name in ["Straße Regular", "Olo-unicode", "Regular", "I", "ı"] {
        assert!(find(name).is_none(), "{name}");
    }
}
#[test]
fn collections_share_one_source_scoped_ids_and_surviving_leases() {
    let data: Arc<[u8]> = bytes("collection.ttc").into();
    let pointer = data.as_ptr();
    let mut b = Builder::new(Default::default());
    assert_eq!(b.add_shared(data.clone(), None).unwrap(), 2);
    let c = b.finish(None).unwrap();
    assert_eq!(c.source_count(), 1);
    assert_eq!(c.faces().count(), 2);
    let a = c.select_family("Olo Match", &request("serif"), Default::default(), None).unwrap().unwrap();
    let z = c.select_family("Olo Fallback", &request("serif"), Default::default(), None).unwrap().unwrap();
    assert_eq!(a.data().as_ptr(), pointer);
    assert_eq!(z.data().as_ptr(), pointer);
    assert_eq!(a.collection_index(), 0);
    assert_eq!(z.collection_index(), 1);
    let other = catalog(&["regular.ttf"], Default::default());
    assert!(matches!(other.face(a.face), Err(Error::Stale)));
    drop(c);
    drop(data);
    assert_eq!(a.glyph_id('0').unwrap(), Some(4));
    assert!(z.glyph_id('水').unwrap().is_some());
}
#[test]
fn first_available_is_independent_of_reference_glyph_fallback_and_units() {
    let c = catalog(&["metrics-first.ttf", "metrics-fallback.ttf"], Default::default());
    let scope = scope(c.clone(), &["Olo Metrics"]);
    let r = request("'Olo Metrics','Olo Fallback'");
    let m = scope.metrics(&r, None).unwrap();
    assert_eq!(c.face(m.first_available).unwrap().families[0].as_ref(), "Olo Metrics");
    assert_ne!(m.first_available, m.zero_face.unwrap());
    assert_eq!(m.zero_face, m.ideograph_face);
    near(m.style.ex, 10.);
    near(m.style.cap, 14.);
    near(m.style.ch, 13.);
    near(m.style.ic, 22.);
    near(m.style.normal_line_height, 22.);
    assert!(!m.fallbacks.ch_estimate);
    assert!(!m.fallbacks.ic_em);
    assert!(scope.first_available(&r, None).unwrap().glyph_id(' ').unwrap().is_none());
}
#[test]
fn writing_modes_choose_reference_advances_and_synthesize_missing_vertical_metrics() {
    let s = scope(catalog(&["metrics-first.ttf", "metrics-fallback.ttf"], Default::default()), &["Olo Metrics"]);
    let mut r = request("'Olo Metrics','Olo Fallback'");
    for mode in [WritingMode::VerticalRl, WritingMode::VerticalLr] {
        r.axes.writing_mode = mode;
        for (orientation, ch, ic) in [(TextOrientation::Mixed, 13., 24.), (TextOrientation::Upright, 23., 24.), (TextOrientation::Sideways, 13., 22.)] {
            r.axes.text_orientation = orientation;
            let m = s.metrics(&r, None).unwrap();
            near(m.style.ch, ch);
            near(m.style.ic, ic);
        }
    }
    r.axes.writing_mode = WritingMode::SidewaysRl;
    r.axes.text_orientation = TextOrientation::Upright;
    let m = s.metrics(&r, None).unwrap();
    near(m.style.ch, 13.);
    near(m.style.ic, 22.);
    let s = scope(catalog(&["regular.ttf"], Default::default()), &["Olo Match"]);
    r.axes.writing_mode = WritingMode::VerticalRl;
    let m = s.metrics(&r, None).unwrap();
    near(m.style.ch, 20.);
    assert!(m.fallbacks.synthesized_vertical_advance);
    assert!(m.fallbacks.ic_em);
}
#[test]
fn bounded_outline_heuristics_and_css_estimates_have_provenance() {
    let r = request("'Olo Missing'");
    let s = scope(catalog(&["metrics-missing.ttf"], Default::default()), &["Olo Missing"]);
    let m = s.metrics(&r, None).unwrap();
    near(m.style.ex, 10.);
    near(m.style.cap, 14.);
    assert!(m.fallbacks.ex_from_outline && m.fallbacks.cap_from_outline && m.fallbacks.ch_estimate && m.fallbacks.ic_em);
    let s = scope(catalog(&["metrics-missing.ttf"], Limits { outline_bytes: 0, ..Default::default() }), &["Olo Missing"]);
    let m = s.metrics(&r, None).unwrap();
    near(m.style.ex, 10.);
    near(m.style.cap, 16.);
    assert!(m.fallbacks.ex_half_em && m.fallbacks.cap_ascent);
    assert!(!m.fallbacks.ex_from_outline);
}
#[test]
fn unavailable_complete_generics_default_before_next_family_but_incomplete_generics_skip() {
    let c = catalog(&["metrics-first.ttf", "metrics-fallback.ttf"], Default::default());
    let mut p = GenericPolicy::new(&["Olo Metrics"]).unwrap();
    p.set(p::GenericFamily::Serif, "", None, &["Unavailable"]).unwrap();
    p.set(p::GenericFamily::UiSerif, "", None, &["Unavailable"]).unwrap();
    let set = FontSet::new(c.clone(), p, Default::default()).unwrap();
    let s = set.scope("").unwrap();
    let a = s.first_available(&request("serif,'Olo Fallback'"), None).unwrap();
    let b = s.first_available(&request("ui-serif,'Olo Fallback'"), None).unwrap();
    assert_eq!(c.face(a.face).unwrap().families[0].as_ref(), "Olo Metrics");
    assert_eq!(c.face(b.face).unwrap().families[0].as_ref(), "Olo Fallback");
    assert!(s.cmap_selection(&request("serif,'Olo Fallback'"), '0', None).unwrap().is_some());
}
#[test]
fn language_range_policy_and_context_identity_are_explicit() {
    let c = catalog(&["metrics-first.ttf", "metrics-fallback.ttf"], Default::default());
    let mut p = GenericPolicy::new(&["Olo Metrics"]).unwrap();
    p.set(p::GenericFamily::Serif, "ja", None, &["Olo Fallback"]).unwrap();
    p.set(p::GenericFamily::Serif, "ja-JP", Some(('水', '水')), &["Olo Metrics"]).unwrap();
    let set = FontSet::new(c.clone(), p, Default::default()).unwrap();
    let a = set.scope("en-US").unwrap();
    let b = set.scope("JA-jp").unwrap();
    assert_eq!(b.language(), "ja-jp");
    assert_eq!(a.revision(), b.revision());
    assert_ne!(a.context(), b.context());
    assert_eq!(a.clone().context(), a.context());
    assert_ne!(set.scope("en-US").unwrap().context(), a.context());
    let r = request("serif");
    assert_ne!(a.first_available(&r, None).unwrap().face, b.first_available(&r, None).unwrap().face);
    assert_eq!(a.metrics(&r, None).unwrap().style.ex, 10.);
    assert_eq!(b.metrics(&r, None).unwrap().style.ex, 5.);
    assert!(set.scope("en--US").is_err());
}
#[test]
fn native_metric_cache_bounds_keys_clone_and_cancel() {
    let s = scope(catalog(&["regular.ttf"], Limits { cache_entries: 2, ..Default::default() }), &["Olo Match"]);
    let mut r = request("'Olo Match'");
    s.metrics(&r, None).unwrap();
    s.metrics(&r, None).unwrap();
    assert_eq!(s.cache_stats().hits, 1);
    assert_eq!(s.cache_stats().misses, 1);
    for size in [21., 22., 23.] {
        r.size = size;
        s.metrics(&r, None).unwrap();
    }
    assert_eq!(s.cache_stats().entries, 2);
    let c = s.clone();
    assert_eq!(c.cache_stats().entries, 0);
    assert_eq!(s.context(), c.context());
    let cancel = AtomicBool::new(true);
    assert!(matches!(s.metrics(&r, Some(&cancel)), Err(Error::Cancelled)));
    assert_eq!(s.cache_stats().hits, 1);
    r.size = f64::NAN;
    assert!(matches!(s.metrics(&r, None), Err(Error::Invalid)));
    assert_eq!(s.cache_stats().entries, 2);
    let s = scope(catalog(&["regular.ttf"], Limits { cache_bytes: 1, ..Default::default() }), &["Olo Match"]);
    assert!(s.metrics(&request("serif"), None).is_ok());
    assert_eq!(s.cache_stats().entries, 0);
}
#[test]
fn malformed_limits_cancellation_and_atomic_loads() {
    for data in [vec![], vec![0; 12], b"wOFFxxxxxxxx".to_vec(), b"wOF2xxxxxxxx".to_vec()] {
        let mut b = Builder::new(Default::default());
        assert!(b.add_owned(data, None).is_err());
        assert_eq!(b.finish(None).unwrap().faces().count(), 0);
    }
    let original = bytes("regular.ttf");
    for end in (0..original.len()).step_by(7) {
        let mut b = Builder::new(Default::default());
        assert!(b.add_owned(original[..end].to_vec(), None).is_err(), "prefix {end}");
    }
    for limits in [
        Limits { file_bytes: 1, ..Default::default() },
        Limits { retained_bytes: 1, ..Default::default() },
        Limits { faces: 0, ..Default::default() },
        Limits { tables: 0, ..Default::default() },
        Limits { names: 0, ..Default::default() },
        Limits { name_bytes: 1, ..Default::default() },
        Limits { work: 0, ..Default::default() },
    ] {
        let mut b = Builder::new(limits);
        assert!(matches!(b.add_owned(original.clone(), None), Err(Error::Limit)));
        assert_eq!(b.finish(None).unwrap().faces().count(), 0);
    }
    let mut b = Builder::new(Default::default());
    b.add_owned(original.clone(), None).unwrap();
    assert!(b.add_owned(original[..100].to_vec(), None).is_err());
    assert_eq!(b.finish(None).unwrap().faces().count(), 1);
    let cancel = AtomicBool::new(true);
    let mut b = Builder::new(Default::default());
    assert!(matches!(b.add_owned(original, Some(&cancel)), Err(Error::Cancelled)));
    cancel.store(false, Ordering::Relaxed);
    let mut progress = 0;
    assert!(matches!(
        b.load_file(
            &path("regular.ttf"),
            |p| {
                progress = p.bytes;
                cancel.store(true, Ordering::Relaxed)
            },
            Some(&cancel)
        ),
        Err(Error::Cancelled)
    ));
    assert!(progress > 0);
    assert_eq!(b.finish(None).unwrap().faces().count(), 0);
    let mut b = Builder::new(Default::default());
    let mut final_progress = None;
    assert_eq!(b.load_file(&path("collection.ttc"), |p| final_progress = Some(p), None).unwrap(), 2);
    assert_eq!(final_progress.unwrap().faces, 2);
}
fn reference_metrics(s: &Scope, r: &FontRequest, case: &serde_json::Value) {
    let m = s.metrics(r, None).unwrap();
    let unit = r.size / case["upem"].as_f64().unwrap();
    let expected = |name: &str| case["units"][name].as_f64().unwrap() * unit;
    for (actual, name) in [(m.style.ch, "ch"), (m.style.ex, "ex"), (m.style.cap, "cap"), (m.ascent, "ascent"), (m.descent, "descent"), (m.line_gap, "line_gap")] {
        near(actual, expected(name));
    }
}
#[test]
fn independent_raw_hvar_mvar_and_real_noto_variable_metrics() {
    let refs: serde_json::Value = serde_json::from_str(include_str!("font-native-fixtures/raw-references.json")).unwrap();
    let c = catalog(&["optical.ttf"], Default::default());
    let s = scope(c.clone(), &["Olo Optical"]);
    let mut r = request("'Olo Optical'");
    for case in refs["optical"].as_array().unwrap() {
        r.size = case["css_size"].as_f64().unwrap();
        let selected = c.select_family("Olo Optical", &r, Default::default(), None).unwrap().unwrap();
        near(f64::from(selected.coordinates[0].value), case["coordinates"]["opsz"].as_f64().unwrap());
        reference_metrics(&s, &r, case);
    }
    r.size = 20.;
    let selected = c.select_family("Olo Optical", &r, MatchOptions { optical_sizing: false, ..Default::default() }, None).unwrap().unwrap();
    assert!(selected.coordinates.is_empty());
    let noto = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf");
    let data = std::fs::read(noto).unwrap();
    let hash = format!("{:x}", Sha256::digest(&data));
    let mut b = Builder::new(Default::default());
    b.add_owned(data, None).unwrap();
    let c = b.finish(None).unwrap();
    let s = scope(c.clone(), &["Noto Sans"]);
    let mut r = request("'Noto Sans'");
    for case in refs["noto"].as_array().unwrap() {
        assert_eq!(case["sha256"].as_str().unwrap(), hash);
        r.weight = case["coordinates"]["wght"].as_f64().unwrap();
        let selected = c.select_family("Noto Sans", &r, Default::default(), None).unwrap().unwrap();
        near(f64::from(selected.coordinates[0].value), r.weight.clamp(c.face(selected.face).unwrap().weight.min, c.face(selected.face).unwrap().weight.max));
        reference_metrics(&s, &r, case);
    }
}

#[test]
fn variable_registered_coordinates_clamp_and_reverse_opentype_slant_sign() {
    let c = catalog(&["axes.ttf"], Default::default());
    let mut r = request("'Olo Axes'");
    r.size = 20.;
    r.weight = 999.5;
    r.width = 137.5;
    r.slant = FontSlant::Oblique(18.);
    let select = |r: &FontRequest| c.select_family("Olo Axes", r, Default::default(), None).unwrap().unwrap();
    let s = select(&r);
    let coordinate = |s: &font::Selection, tag| s.coordinates.iter().find(|a| a.tag == tag).unwrap().value;
    near(s.weight, 999.5);
    near(s.width, 137.5);
    near(f64::from(coordinate(&s, *b"wght")), 999.5);
    near(f64::from(coordinate(&s, *b"wdth")), 137.5);
    near(f64::from(coordinate(&s, *b"slnt")), -18.);
    near(f64::from(coordinate(&s, *b"ital")), 0.);
    r.slant = FontSlant::Italic;
    let s = select(&r);
    assert_eq!(s.slope, Slope::Italic(1.));
    near(f64::from(coordinate(&s, *b"ital")), 1.);
    r.width = 500.;
    r.slant = FontSlant::Oblique(-50.);
    let s = select(&r);
    near(s.width, 200.);
    assert_eq!(s.slope, Slope::Oblique(-20.));
    near(f64::from(coordinate(&s, *b"slnt")), 20.);
    for weight in [1., 1000.] {
        r.weight = weight;
        near(select(&r).weight, weight);
    }
}

#[test]
fn composite_height_inference_skips_recursion_and_gvar_phantoms_keep_precision() {
    let s = scope(catalog(&["metrics-composite.ttf"], Default::default()), &["Olo Composite"]);
    let m = s.metrics(&request("'Olo Composite'"), None).unwrap();
    near(m.style.ex, 10.);
    assert!(m.fallbacks.ex_half_em);
    assert!(!m.fallbacks.ex_from_outline);
    near(m.style.cap, 14.);
    assert!(m.fallbacks.cap_from_outline);
    let s = scope(catalog(&["optical-phantom.ttf"], Default::default()), &["Olo Optical"]);
    let refs: serde_json::Value = serde_json::from_str(include_str!("font-native-fixtures/raw-references.json")).unwrap();
    let mut r = request("'Olo Optical'");
    for case in refs["optical"].as_array().unwrap() {
        r.size = case["css_size"].as_f64().unwrap();
        reference_metrics(&s, &r, case);
    }
    let s = scope(catalog(&["optical-phantom.ttf"], Limits { outline_bytes: 0, ..Default::default() }), &["Olo Optical"]);
    let m = s.metrics(&r, None).unwrap();
    assert!(m.fallbacks.ch_estimate);
}

#[test]
fn selected_chrome_loaded_font_metrics_bound_the_known_integer_rounding_difference() {
    // Chrome exposes integer-font-unit metric rounding and 1/64 px layout
    // quantization. Raw native variation metrics deliberately retain deltas.
    let refs: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-05-native-font-chrome.json")).unwrap();
    let mut b = Builder::new(Default::default());
    b.add_owned(bytes("optical.ttf"), None).unwrap();
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fonts/NotoSans[wght].ttf"), |_| {}, None).unwrap();
    let c = b.finish(None).unwrap();
    let s = scope(c.clone(), &["Olo Optical"]);
    let mut checked = 0;
    for case in refs["cases"].as_array().unwrap() {
        if case["variation"] != "auto" {
            continue;
        }
        let optical = case["family"] == "Optical";
        let mut r = request(if optical { "'Olo Optical'" } else { "'Noto Sans'" });
        r.size = case["size"].as_f64().unwrap();
        if !optical {
            r.weight = 650.5;
        }
        let m = s.metrics(&r, None).unwrap();
        let tolerance = r.size / 1000. + 1. / 64.;
        for (unit, actual) in [("ch", m.style.ch), ("ex", m.style.ex), ("cap", m.style.cap)] {
            let expected = case["units"][unit].as_f64().unwrap();
            assert!((actual - expected).abs() <= tolerance, "{case}: {unit}: {actual} vs {expected}");
            checked += 1;
        }
    }
    assert_eq!(checked, 24);
}

#[path = "support/native_font_host.rs"]
mod host;
#[test]
fn style_jobs_rebind_language_metrics_and_reject_incoherent_root_or_publication() {
    use olo_web::{
        css::{
            cascade::{Origin, SheetBuilder},
            color,
            style::{Defaults, Host},
        },
        html,
    };
    use std::cell::Cell;
    let c = catalog(&["metrics-first.ttf", "metrics-fallback.ttf"], Default::default());
    let mut p = GenericPolicy::new(&["Olo Metrics"]).unwrap();
    p.set(p::GenericFamily::Serif, "ja", None, &["Olo Fallback"]).unwrap();
    let set = FontSet::new(c, p, Default::default()).unwrap();
    let en = host::NativeHost(set.scope("en").unwrap());
    let ja = host::NativeHost(set.scope("ja").unwrap());
    let document = html::parse(Arc::from("<main id=parent><div id=child><i id=grandchild></i></div><div id=sibling></div></main>"), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from("main{font-size:20px;font-family:serif}div,i{width:1ex}"), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default());
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = builder.finish(None).unwrap();
    let defaults = Defaults::new(&en, Default::default(), None).unwrap();
    let mut job = style::Job::new(&document, &rules, defaults.clone(), Default::default(), Default::default(), None).unwrap();
    while !job.step(2, |node| if matches!(document.attribute(node, "id").unwrap(), Some("child" | "grandchild")) { &ja } else { &en }).unwrap().finished {}
    let tree = job.finish().unwrap();
    let find = |name| tree.iter().find(|(n, _)| document.attribute(*n, "id").unwrap() == Some(name)).unwrap().1;
    let (parent, child, grandchild, sibling) = (find("parent"), find("child"), find("grandchild"), find("sibling"));
    near(parent.font().metrics.ex, 10.);
    near(child.font().metrics.ex, 5.);
    near(child.value(p::Property::Width).scalar().unwrap(), 5.);
    near(sibling.value(p::Property::Width).scalar().unwrap(), 10.);
    assert!(!Arc::ptr_eq(parent.font(), child.font()));
    assert!(Arc::ptr_eq(child.font(), grandchild.font()));
    assert_eq!(child.font().context(), ja.0.context());
    let mut wrong = style::Job::new(&document, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    assert!(matches!(wrong.step(128, |_| &ja), Err(style::Error::Stale)));
    struct Changing<'a> {
        host: &'a host::NativeHost,
        context: Cell<u64>,
    }
    impl Host for Changing<'_> {
        fn revision(&self) -> u64 {
            self.host.revision()
        }
        fn scheme(&self) -> color::Scheme {
            self.host.scheme()
        }
        fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
            self.host.system_color(c, s)
        }
        fn font(&self, r: &FontRequest) -> style::Result<style::FontMetrics> {
            let m = self.host.font(r);
            self.context.set(self.context.get() + 1);
            m
        }
        fn font_context(&self) -> u64 {
            self.context.get()
        }
        fn font_size(&self, k: p::Keyword, n: f64) -> style::Result<f64> {
            self.host.font_size(k, n)
        }
        fn device_scale(&self) -> f64 {
            1.
        }
    }
    let changing = Changing { host: &en, context: Cell::new(1) };
    assert!(matches!(Defaults::new(&changing, Default::default(), None), Err(style::Error::Stale)));
}
