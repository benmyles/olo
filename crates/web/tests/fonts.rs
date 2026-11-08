//! `@font-face` descriptors and web font loading.
use olo_web::css::{
    Syntax,
    cascade::{Origin, SheetBuilder},
    fonts::Source,
    media::Media,
};
use std::sync::Arc;
#[test]
fn font_face_rules_become_descriptors_with_sources_and_ranges() {
    let css = r#"@font-face{font-family:sohne-var;font-style:normal;font-weight:200 700;font-display:swap;src:url(https://example.test/sohne-var.woff2) format("woff2"),url(/sohne.woff) format("woff");unicode-range:U+0000-00FF,U+0131,U+2000-206F}
@font-face{font-family:"Source Code Pro";font-style:italic;src:local("Source Code Pro"),url(/code.ttf)}
@media (min-width: 5000px){@font-face{font-family:Never;src:url(/never.woff2)}}
@font-face{font-family:NoSource;font-weight:bold}"#;
    let syntax = Syntax::parse(Arc::from(css), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default()).with_media(Media::screen(1000., 700., 1., false));
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let faces = builder.font_faces();
    assert_eq!(faces.len(), 2, "{faces:?}");
    let sohne = &faces[0];
    assert_eq!(sohne.descriptor.family.as_ref(), "sohne-var");
    assert_eq!((sohne.descriptor.weight.min, sohne.descriptor.weight.max), (200., 700.));
    assert_eq!(sohne.sources.len(), 2);
    assert!(matches!(&sohne.sources[0], Source::Url { url, format: Some(f) } if url.as_ref() == "https://example.test/sohne-var.woff2" && f.as_ref() == "woff2"));
    assert!(sohne.sources[0].decodable());
    assert_eq!(sohne.descriptor.ranges.as_deref(), Some(&[(0, 0xFF), (0x131, 0x131), (0x2000, 0x206F)][..]));
    let code = &faces[1];
    assert_eq!(code.descriptor.family.as_ref(), "Source Code Pro");
    assert!(code.descriptor.slopes.italic.is_some());
    assert!(matches!(&code.sources[0], Source::Local(_)));
    assert!(!code.sources[0].decodable());
    assert!(code.sources[1].decodable());
}
#[test]
fn woff2_files_decode_to_parsable_fonts() {
    // Set OLO_WOFF2_FILES to a colon-separated list of local WOFF2 files to check.
    let Ok(list) = std::env::var("OLO_WOFF2_FILES") else { return };
    for path in list.split(':').filter(|p| !p.is_empty()) {
        let data = std::fs::read(path).unwrap();
        assert!(olo_web::font::woff::is_woff(&data), "{path}");
        let sfnt = olo_web::font::woff::decode(&data, None).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        let face = ttf_parser::Face::parse(&sfnt, 0).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        assert!(face.number_of_glyphs() > 0, "{path}");
        assert!(face.glyph_index('A').is_some(), "{path}");
        let bbox = face.glyph_bounding_box(face.glyph_index('A').unwrap());
        assert!(bbox.is_some_and(|b| b.width() > 0 && b.height() > 0), "{path}: {bbox:?}");
        let advance = face.glyph_hor_advance(face.glyph_index('A').unwrap());
        assert!(advance.is_some_and(|a| a > 0), "{path}: {advance:?}");
        eprintln!("{path}: {} glyphs, upem {}, A bbox {bbox:?} advance {advance:?}, axes {}", face.number_of_glyphs(), face.units_per_em(), face.variation_axes().len());
    }
}
#[test]
fn a_family_split_by_unicode_range_answers_each_character_from_its_face() {
    use olo_web::{
        css::{
            properties::{self as p, Declaration},
            style::{Axes, FontRequest, FontSlant},
        },
        font::{Builder, Descriptor, FontSet, GenericPolicy, Slopes, Span},
    };
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf");
    let mut b = Builder::new(Default::default());
    b.load_file(&file, |_| {}, None).unwrap();
    let base = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    // As Google Fonts serves a family: the same face for Cyrillic, then Latin.
    let data: Arc<[u8]> = Arc::from(std::fs::read(&file).unwrap());
    let face = |ranges: &[(u32, u32)]| {
        (
            data.clone(),
            Descriptor {
                family: Arc::from("Split"),
                weight: Span::point(400.),
                width: Span::point(100.),
                slopes: Slopes { oblique: Some(Span::point(0.)), italic: None },
                ranges: Some(ranges.into()),
            },
        )
    };
    let scope = base.with_faces(vec![face(&[(0x400, 0x4FF)]), face(&[(0, 0xFF)])], None).unwrap();
    let syntax = Syntax::parse(Arc::from("Split"), Default::default(), None).unwrap();
    let Declaration::Known { values, .. } = Declaration::parse("font-family", syntax.components(), false, Default::default(), None).unwrap() else { panic!() };
    let p::Value::Families(families) = &values[0].value else { panic!() };
    let request = FontRequest { size: 20., weight: 400., width: 100., slant: FontSlant::Normal, families: families.clone(), axes: Axes::default() };
    // The first available font (a space's) is the Latin face, not the first listed.
    let selected = scope.first_available(&request, None).unwrap();
    assert_eq!(scope.catalog().face(selected.face).unwrap().ranges.as_deref(), Some(&[(0, 0xFF)][..]));
}
