//! The measurements scripts read (`page::Measurer`): each measured against
//! the one before, as the page's scripts change the document.
use olo_web::{
    font::{Builder, FontSet, GenericPolicy},
    layout, page,
    text::shaping,
};
use std::{path::Path, sync::Arc};
#[path = "support/paint_host.rs"]
mod paint_host;
fn width_of(m: &page::Measured, doc: &olo_web::dom::Document, id: &str) -> f64 {
    let l = m.layout();
    let (b, _) = l.cache().tree().iter().find(|(_, b)| matches!(b.kind, layout::BoxKind::Block { .. }) && doc.attribute(b.node, "id").unwrap() == Some(id)).unwrap();
    l.bounds(b).unwrap().width
}
/// An inline style that changes an element's font is patched in place in
/// the previous measurement's styles; its texts are shaped again in the
/// new font, not taken from the previous preparation (which then held the
/// new styles beside texts in the old font: "flow layout Text(Stale)", and
/// a full restyle, at every one of Wikipedia's scroll-driven measurements).
#[test]
fn a_font_patched_in_place_shapes_its_texts_again() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let viewport = layout::Viewport { width: 400., height: 300., scale: 1. };
    let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false);
    let source = "<!doctype html><style>html,body{margin:0}body{font-family:'Olo Inline plain'}#t{float:left}</style><div id=t>Some words <b id=s>here</b></div><p>after</p>";
    let prepared = page::prepare(Arc::from(source), &scope, media, false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
    let measurer = prepared.measurer();
    let mut shaper = shaping::Engine::default();
    let mut doc = prepared.into_document();
    let mut last = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let before = width_of(&last, &doc, "t");
    let node = |doc: &olo_web::dom::Document, id: &str| {
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.node_type(n).unwrap() == olo_web::dom::NodeType::Element && doc.attribute(n, "id").unwrap() == Some(id) {
                return n;
            }
            stack.extend(doc.children(n).unwrap());
        }
        panic!("no #{id}")
    };
    // The container's font, then a span's alone, then the container's
    // back as it was: each a patch in place.
    for (id, style) in [("t", "font-size:32px"), ("s", "font-size:8px"), ("t", "")] {
        let n = node(&doc, id);
        doc.set_attribute(n, "style", style).unwrap();
        let m = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).unwrap();
        assert!(m.path().starts_with("patched in place"), "{id} {style}: {}", m.path());
        let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
        assert_eq!(width_of(&m, &doc, "t"), width_of(&fresh, &doc, "t"), "{id} {style}");
        last = m;
    }
    assert!(width_of(&last, &doc, "t") < before, "the span's text is smaller than before");
}
/// A `::before`/`::after` exists where a rule giving it `content` matches,
/// and only there: rules keyed on the element's name with an ancestor in
/// an `:is()` (as CSS nesting writes them) are ruled out by the
/// ancestors alone, in the document and in a shadow tree.
#[test]
fn pseudo_elements_are_made_where_their_content_rules_match() {
    use olo_web::css::selectors::PseudoElement;
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let viewport = layout::Viewport { width: 400., height: 300., scale: 1. };
    let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false);
    let source = "<!doctype html><style>:is(.crumbs li)::after{content:'>'} li::before{color:red} .x li::before{content:'*'}</style>\
        <ul class=crumbs><li id=a>a</li></ul><ul><li id=b>b</li></ul><div class=x><ul><li id=c>c</li></ul></div>\
        <div id=h><template shadowrootmode=open><style>:is(.c span)::before{content:'*'}</style><p class=c><span id=s>s</span></p><p><span id=t>t</span></p></template></div>";
    let prepared = page::prepare(Arc::from(source), &scope, media, false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
    let measurer = prepared.measurer();
    let mut shaper = shaping::Engine::default();
    let doc = prepared.into_document();
    let m = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let styles = m.layout().cache().tree().styles();
    let find = |id: &str| {
        let mut stack = vec![doc.root()];
        while let Some(n) = stack.pop() {
            if doc.node_type(n).unwrap() == olo_web::dom::NodeType::Element && doc.attribute(n, "id").unwrap() == Some(id) {
                return n;
            }
            stack.extend(doc.children(n).unwrap());
            stack.extend(doc.shadow_root(n));
        }
        panic!("no #{id}")
    };
    let has = |id: &str, pseudo| styles.pseudo(find(id), pseudo).is_some();
    assert!(has("a", PseudoElement::After) && !has("b", PseudoElement::After) && !has("c", PseudoElement::After));
    assert!(!has("a", PseudoElement::Before) && !has("b", PseudoElement::Before) && has("c", PseudoElement::Before), "a rule without content makes none");
    assert!(has("s", PseudoElement::Before) && !has("t", PseudoElement::Before), "in a shadow tree");
}
