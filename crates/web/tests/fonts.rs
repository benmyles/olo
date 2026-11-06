use olo_web::{
    css::{
        Syntax,
        cascade::{Origin, SheetBuilder},
        color, math,
        properties::{self as p, Declaration, Family, GenericFamily, Name, Property as P},
        style::{self, Defaults, FontMetrics, FontRequest, FontSlant, Host, StyleTree, Value},
    },
    dom::Document,
    html,
};
use std::{
    cell::RefCell,
    sync::{Arc, atomic::AtomicBool},
};
#[path = "support/style_fixtures.rs"]
mod fixtures;

fn declaration(name: &str, value: &str) -> p::Result<Declaration> {
    let syntax = Syntax::parse(Arc::from(value), Default::default(), None).unwrap();
    Declaration::parse(name, syntax.components(), false, Default::default(), None)
}
fn families(value: &str) -> p::Families {
    let Declaration::Known { values, .. } = declaration("font-family", value).unwrap() else { panic!() };
    let p::Value::Families(f) = &values[0].value else { panic!() };
    f.clone()
}
#[test]
fn decoded_family_identity_generics_and_unbounded_length_grammar() {
    let f = families(r#"New/**/Century Schoolbook, 'serif', sAnS-sErIf, \31 st, 'a,b', generic(kai), generic\(kai\), ''"#);
    assert_eq!(
        f.as_slice(),
        &[
            Family::Named(Arc::from("New Century Schoolbook")),
            Family::Named(Arc::from("serif")),
            Family::Generic(GenericFamily::SansSerif),
            Family::Named(Arc::from("1st")),
            Family::Named(Arc::from("a,b")),
            Family::Generic(GenericFamily::Kai),
            Family::Named(Arc::from("generic(kai)")),
            Family::Named(Arc::from("")),
        ]
    );
    // Old fixed quad-value scratch must not cap family names or fallback lists.
    let f = families("a b c d e f, x, y, z, w, v, monospace");
    assert_eq!(f.as_slice().len(), 7);
    assert_eq!(f.as_slice()[0], Family::Named(Arc::from("a b c d e f")));
    assert_eq!(families("'family at EOF").as_slice(), &[Family::Named(Arc::from("family at EOF"))]);
    for value in ["", ",serif", "serif,", "serif sans-serif", "foo initial", "inherit,foo", "foo default bar", "'Lucida' Grande", "generic(foo)", "generic(kai,serif)", "generic(serif)", "serif( )"] {
        assert!(matches!(declaration("font-family", value), Err(p::Error::Invalid)), "{value}");
    }
    for word in ["inherit", "initial", "unset", "revert", "revert-layer", "revert-rule"] {
        let Declaration::Known { values, .. } = declaration("font-family", word).unwrap() else { panic!() };
        assert!(matches!(values[0].value, p::Value::Wide(_)));
    }
    assert_eq!(Name::parse("FONT-STRETCH"), Name::parse("font-width"));
}

/// Supplied test face metrics deliberately vary with family, style and width.
/// This verifies request/context wiring, not real font loading or selection.
struct Context {
    requests: RefCell<Vec<FontRequest>>,
}
impl Context {
    fn new() -> Self {
        Self { requests: RefCell::new(Vec::new()) }
    }
}
impl Host for Context {
    fn revision(&self) -> u64 {
        1
    }
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
    fn system_color(&self, c: color::SystemColor, s: color::Scheme) -> color::Result<color::Color> {
        fixtures::FixtureHost.system_color(c, s)
    }
    fn font_size(&self, k: p::Keyword, parent: f64) -> style::Result<f64> {
        fixtures::FixtureHost.font_size(k, parent)
    }
    fn device_scale(&self) -> f64 {
        1.
    }
    fn font_context(&self) -> u64 {
        1
    }
    fn font(&self, request: &FontRequest) -> style::Result<FontMetrics> {
        self.requests.borrow_mut().push(request.clone());
        let mono = request.families.as_slice().first() == Some(&Family::Generic(GenericFamily::Monospace));
        let ratio = if mono { 0.6 } else { 0.4 };
        Ok(FontMetrics { id: if mono { 2 } else { 1 }, ex: request.size * ratio, cap: request.size * 0.75, ch: request.size * ratio, ic: request.size, normal_line_height: request.size * 1.2 })
    }
}
fn styles(source: &str, sheets: &str, host: &impl Host) -> (Document, StyleTree) {
    let document = html::parse(Arc::from(source), Default::default(), None).unwrap().document;
    let syntax = Syntax::parse(Arc::from(sheets), Default::default(), None).unwrap();
    let mut builder = SheetBuilder::new(Default::default());
    builder.add_sheet(&syntax, Origin::Author, None).unwrap();
    let rules = builder.finish(None).unwrap();
    assert!(rules.issues().is_empty(), "{:?}", rules.issues());
    let defaults = Defaults::new(host, Default::default(), None).unwrap();
    let mut job = style::Job::new(&document, &rules, defaults, Default::default(), Default::default(), None).unwrap();
    while !job.step(3, |_| host).unwrap().finished {}
    let tree = job.finish().unwrap();
    (document, tree)
}
fn named<'a>(document: &Document, tree: &'a StyleTree, id: &str) -> &'a style::Style {
    tree.iter().find(|(n, _)| document.attribute(*n, "id").unwrap() == Some(id)).unwrap().1
}
fn scalar(style: &style::Style, p: P) -> f64 {
    style.value(p).scalar().unwrap()
}
#[test]
fn selected_font_prerequisites_and_parent_units_are_frozen_in_order() {
    let host = Context::new();
    let (document, tree) = styles(
        "<div id=p style='font-family:monospace;font-size:20px;font-width:125%;font-style:italic;--family:serif'><div id=c style='font-family:var(--family);font-size:calc(1ex + 10px);font-width:calc(100% + sign(1ex - 10px) * 5%);font-style:oblique calc(sign(1ex - 10px) * 10deg);width:1ex'><div id=g style='font-size:30px'></div></div></div>",
        "",
        &host,
    );
    let parent = named(&document, &tree, "p");
    let child = named(&document, &tree, "c");
    let grandchild = named(&document, &tree, "g");
    assert_eq!(parent.font().request.slant, FontSlant::Italic);
    assert_eq!(parent.font().request.width, 125.);
    assert_eq!(child.font().request.size, 22.); // Parent mono ex = 12px.
    assert_eq!(child.font().request.width, 105.);
    assert_eq!(child.font().request.slant, FontSlant::Oblique(10.));
    assert_eq!(child.font().request.families.as_slice(), &[Family::Generic(GenericFamily::Serif)]);
    assert!((scalar(child, P::Width) - 8.8).abs() < 1e-12); // Subject serif ex.
    assert_eq!(grandchild.font().request.width, 105.); // No child-metric rebinding.
    assert_eq!(grandchild.font().request.slant, FontSlant::Oblique(10.));
    assert!((grandchild.line_height() - 36.).abs() < 1e-12);
    assert!(host.requests.borrow().iter().any(|r| r.size == 22. && r.width == 105. && r.slant == FontSlant::Oblique(10.)));
    assert!(child.dependencies().color.math.contains(math::RelativeUnit::Ex));
}
#[test]
fn angle_ranges_percentage_width_alias_and_inherited_oblique_identity() {
    for value in ["oblique 91deg", "oblique -91deg", "oblique 1turn", "oblique 101grad", "oblique 2rad", "oblique 0", "italic 10deg"] {
        assert!(matches!(declaration("font-style", value), Err(p::Error::Invalid)), "{value}");
    }
    let (document, tree) = styles(
        "<main id=p style='font-style:oblique;font-width:75%'><i id=a style='font-style:oblique 14deg'></i><i id=b style='font-style:oblique calc(-100deg);font-stretch:condensed;font-width:calc(-100%)'></i><i id=c style='font-style:oblique calc(10deg - 10deg)'></i><i id=d style='font-style:oblique 10grad;font-width:calc(50% + 12.5%)'></i></main>",
        "",
        &fixtures::FixtureHost,
    );
    let parent = named(&document, &tree, "p");
    let a = named(&document, &tree, "a");
    let b = named(&document, &tree, "b");
    let c = named(&document, &tree, "c");
    let d = named(&document, &tree, "d");
    assert!(Arc::ptr_eq(parent.font(), a.font())); // Same used slope, distinct computed syntax.
    assert!(matches!(parent.value(P::FontStyle), Value::Keyword(p::Keyword::Oblique)));
    assert!(matches!(a.value(P::FontStyle), Value::Oblique(14.)));
    assert_eq!(b.font().request.slant, FontSlant::Oblique(-90.));
    assert_eq!(b.font().request.width, 0.);
    assert_eq!(c.font().request.slant, FontSlant::Normal);
    assert_eq!(d.font().request.slant, FontSlant::Oblique(9.));
    assert_eq!(d.font().request.width, 62.5);
    assert_eq!(d.value(P::FontWidth).scalar(), Some(62.5));
    let (document, tree) = styles("<i id=x style='font-stretch:200%;font-width:75%;font-stretch:125%'></i>", "", &fixtures::FixtureHost);
    assert_eq!(named(&document, &tree, "x").font().request.width, 125.);
}
#[test]
fn family_storage_shares_across_inheritance_and_bounds_compilation() {
    let host = fixtures::FixtureHost;
    let (document, tree) = styles("<div id=p style='font-family:Named Face, monospace'><i id=a></i><i id=b></i><i id=c style='all:initial'></i></div>", "", &host);
    let parent = named(&document, &tree, "p");
    let a = named(&document, &tree, "a");
    let b = named(&document, &tree, "b");
    let c = named(&document, &tree, "c");
    assert!(Arc::ptr_eq(parent.font(), a.font()));
    assert!(Arc::ptr_eq(a.font(), b.font()));
    assert_eq!(c.font().request.families.as_slice(), &[Family::Generic(GenericFamily::Serif)]);
    let one = style::Memory::for_styles([parent]);
    let many = style::Memory::for_styles([parent, a, b]);
    assert_eq!(one.families_bytes, many.families_bytes);
    assert!(one.families_bytes > 0);
    let syntax = Syntax::parse(Arc::from("one, two, three"), Default::default(), None).unwrap();
    assert!(matches!(Declaration::parse("font-family", syntax.components(), false, p::Limits { nodes: 2, ..Default::default() }, None), Err(p::Error::Limit)));
    assert!(matches!(Declaration::parse("font-family", syntax.components(), false, Default::default(), Some(&AtomicBool::new(true))), Err(p::Error::Cancelled)));
    assert_eq!(tree.report().inline_invalid, 0);
}
#[test]
fn independent_pinned_font_assertions_and_explicit_browser_nonpasses() {
    let report = fixtures::evaluate(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/font-fixtures"));
    assert_eq!(report["total"], 149);
    assert_eq!(report["counts"]["passed"], 137, "{report}");
    assert_eq!(report["counts"]["unsupported"], 12);
    for failure in ["mismatch", "error", "panic"] {
        assert!(report["counts"][failure].is_null(), "{report}");
    }
}
#[test]
fn selected_chrome_font_values_and_reference_feature_gaps_are_explicit() {
    let report: serde_json::Value = serde_json::from_str(include_str!("../../../docs/browser/measurements/2026-11-05-font-chrome.json")).unwrap();
    let mut observations = 0;
    for row in report["rows"].as_array().unwrap() {
        let (document, tree) = styles(&format!("<div id=x style='{}'></div>", row["css"].as_str().unwrap()), "", &fixtures::FixtureHost);
        let style = named(&document, &tree, "x");
        for (property, expected) in row["values"].as_object().unwrap() {
            fixtures::compare(style, property, expected.as_str().unwrap()).unwrap();
            observations += 1;
        }
    }
    assert_eq!(observations, 17);
    assert_eq!(report["reference_feature_gaps"].as_array().unwrap().len(), 6);
    assert_eq!(report["support"]["fontWidth"], false);
    assert_eq!(report["support"]["left"], false);
    assert_eq!(report["support"]["right"], false);
    assert_eq!(report["support"]["generic"], false);
    // These current-spec values are separate native assertions, not claimed
    // Chrome compatibility when this reference does not expose them.
    for value in ["left", "right"] {
        assert!(declaration("font-style", value).is_ok());
    }
    assert_eq!(families("generic(fangsong), generic(kai), generic(khmer-mul), generic(nastaliq)").as_slice().len(), 4);
}
