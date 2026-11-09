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
fn measurer_for(source: &str) -> (page::Measurer, paint_host::Scaled, olo_web::dom::Document, layout::Viewport) {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let viewport = layout::Viewport { width: 400., height: 300., scale: 1. };
    let media = olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false);
    let prepared = page::prepare(Arc::from(source), &scope, media, false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
    let measurer = prepared.measurer();
    (measurer, host, prepared.into_document(), viewport)
}
fn find(doc: &olo_web::dom::Document, id: &str) -> olo_web::dom::NodeId {
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if doc.node_type(n).unwrap() == olo_web::dom::NodeType::Element && doc.attribute(n, "id").unwrap() == Some(id) {
            return n;
        }
        stack.extend(doc.children(n).unwrap());
        stack.extend(doc.shadow_root(n));
    }
    panic!("no #{id}")
}
/// An element's style in a measurement (its own: the first entry).
fn style_of(m: &page::Measured, node: olo_web::dom::NodeId) -> Option<&olo_web::css::style::Style> {
    m.layout().cache().tree().styles().iter().find(|(n, _)| *n == node).map(|(_, s)| s)
}
/// Each element's style in `m` is the one a measurement from nothing makes.
fn assert_as_fresh(m: &page::Measured, measurer: &mut page::Measurer, host: &paint_host::Scaled, doc: &olo_web::dom::Document, viewport: layout::Viewport) {
    let fresh = measurer.measure(doc, host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let (kept, made): (Vec<_>, Vec<_>) = (m.layout().cache().tree().styles().iter().collect(), fresh.layout().cache().tree().styles().iter().collect());
    assert_eq!(kept.len(), made.len(), "as many styles as from nothing");
    for ((a, x), (b, y)) in kept.iter().zip(&made) {
        assert_eq!(a, b);
        assert!(!x.differs_in_value(y), "<{:?}> {}", doc.name(*a), x.difference(y));
    }
}
/// A component's styles added to its shadow root (MDN's, Reddit's, as
/// they load) change the rules; the styles outside its trees are kept,
/// not made again from nothing, and those it reaches (its shadow tree,
/// itself through `:host`, its children through `::slotted`) follow.
#[test]
fn a_shadow_trees_new_sheet_restyles_only_what_it_reaches() {
    let (mut measurer, host, mut doc, viewport) = measurer_for(
        "<!doctype html><style>p{color:blue}</style><p id=out>out</p>\
         <div id=h><template shadowrootmode=open><style>i{color:red}</style><i id=in>in</i><slot></slot></template><b id=light>light</b></div>",
    );
    let mut shaper = shaping::Engine::default();
    let first = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let identity = |m: &page::Measured, id: &str, doc: &olo_web::dom::Document| style_of(m, find(doc, id)).unwrap().identity();
    let out_before = identity(&first, "out", &doc);
    let in_before = identity(&first, "in", &doc);
    // The component adds a sheet to its shadow root.
    let root = doc.shadow_root(find(&doc, "h")).unwrap();
    let style = doc.create_element("style", olo_web::dom::Namespace::Html).unwrap();
    let text = doc.create_text(":host{color:green} i{color:purple} ::slotted(b){color:orange}").unwrap();
    doc.append_child(style, text).unwrap();
    doc.append_child(root, style).unwrap();
    assert!(measurer.refresh(&doc, None, &mut |_, _, _| Ok(None)).unwrap(), "the rules changed");
    let second = measurer.measure(&doc, &host, &mut shaper, viewport, Some(first), None).unwrap();
    assert_eq!(identity(&second, "out", &doc), out_before, "outside the component: kept");
    assert_ne!(identity(&second, "in", &doc), in_before, "in its shadow tree: styled again");
    assert_as_fresh(&second, &mut measurer, &host, &doc, viewport);
}
/// A child added to a host shows through a slot deep in its shadow tree:
/// it is styled there (the slot is walked, not copied with what it showed).
#[test]
fn a_child_added_to_a_host_is_styled_where_its_slot_is() {
    let (mut measurer, host, mut doc, viewport) =
        measurer_for("<!doctype html><div id=h><template shadowrootmode=open><div><section><slot></slot></section></div></template><b>first</b></div><p>after</p>");
    let mut shaper = shaping::Engine::default();
    let first = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let added = doc.create_element("i", olo_web::dom::Namespace::Html).unwrap();
    doc.append_child(find(&doc, "h"), added).unwrap();
    let second = measurer.measure(&doc, &host, &mut shaper, viewport, Some(first), None).unwrap();
    assert!(style_of(&second, added).is_some(), "the added child is styled");
    assert_as_fresh(&second, &mut measurer, &host, &doc, viewport);
    // And gone again.
    doc.detach(added).unwrap();
    let third = measurer.measure(&doc, &host, &mut shaper, viewport, Some(second), None).unwrap();
    assert_as_fresh(&third, &mut measurer, &host, &doc, viewport);
}
/// A parent becoming a flex container blockifies its children, though
/// `display` is not inherited: they are styled again, not kept.
#[test]
fn a_parent_turned_flex_blockifies_its_kept_children() {
    let (mut measurer, host, mut doc, viewport) = measurer_for("<!doctype html><style>.flex{display:flex}</style><div id=p><img id=i width=10 height=10><span>s</span></div>");
    let mut shaper = shaping::Engine::default();
    let first = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let p = find(&doc, "p");
    doc.set_attribute(p, "class", "flex").unwrap();
    let second = measurer.measure(&doc, &host, &mut shaper, viewport, Some(first), None).unwrap();
    assert_as_fresh(&second, &mut measurer, &host, &doc, viewport);
    // And by its inline style (patched in place).
    doc.set_attribute(p, "class", "").unwrap();
    let third = measurer.measure(&doc, &host, &mut shaper, viewport, Some(second), None).unwrap();
    doc.set_attribute(p, "style", "display:grid").unwrap();
    let fourth = measurer.measure(&doc, &host, &mut shaper, viewport, Some(third), None).unwrap();
    assert_as_fresh(&fourth, &mut measurer, &host, &doc, viewport);
}
/// A component's instances have the same sheets in their shadow roots:
/// compiled once, every instance takes their rules (its shadow tree, its
/// host through `:host`, its children through `::slotted`), as a single
/// instance does.
#[test]
fn instances_of_a_component_share_their_compiled_sheets() {
    let instance = "<x-c><template shadowrootmode=open><style>:host{display:block;color:red} p{color:green} ::slotted(b){color:blue}</style><p>in</p><slot></slot></template><b>light</b></x-c>";
    let (mut measurer, host, doc, viewport) = measurer_for(&format!("<!doctype html>{instance}{instance}{instance}"));
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let mut seen = 0;
    for (node, style) in m.layout().cache().tree().styles().iter() {
        let name = doc.name(node).unwrap().unwrap_or("");
        let colour = format!("{:?}", style.value(olo_web::css::properties::Property::Color));
        let display = format!("{:?}", style.value(olo_web::css::properties::Property::Display));
        match name {
            "x-c" => {
                assert!(colour.contains("[1.0, 0.0, 0.0") && display.contains("Block"), "host: {colour} {display}");
                seen += 1;
            }
            "p" => assert!(colour.contains("[0.0, 0.50"), "shadow content: {colour}"),
            "b" => assert!(colour.contains("[0.0, 0.0, 1.0"), "slotted: {colour}"),
            _ => {}
        }
    }
    assert_eq!(seen, 3);
}
/// `:dir()` in a shadow tree takes its host's direction (the walk up
/// passes from the shadow root to its host, never asking the root for an
/// attribute: that failed Reddit's every restyle).
#[test]
fn dir_in_a_shadow_tree_follows_the_host() {
    let (mut measurer, host, doc, viewport) =
        measurer_for("<!doctype html><html dir=rtl><div id=h><template shadowrootmode=open><style>i:dir(rtl){color:red} i:dir(ltr){color:blue}</style><i id=in>x</i></template></div></html>");
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let colour = format!("{:?}", style_of(&m, find(&doc, "in")).unwrap().value(olo_web::css::properties::Property::Color));
    assert!(colour.contains("[1.0, 0.0, 0.0"), "{colour}");
}
/// A component's sheet in each of its instances' shadow roots counts once
/// toward the page's CSS budget (8 MB): Reddit's copies passed it, and the
/// later instances were left without their rules.
#[test]
fn a_components_sheet_counts_once_in_the_css_budget() {
    let big: String = (0..60_000).map(|i| format!(".r{i}{{color:green}}")).collect();
    assert!(big.len() > 1024 * 1024);
    let instance = format!("<x-c><template shadowrootmode=open><style>{big} p{{color:red}}</style><p>in</p></template></x-c>");
    let (mut measurer, host, doc, viewport) = measurer_for(&format!("<!doctype html>{}", instance.repeat(9)));
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let mut paragraphs = 0;
    for (node, style) in m.layout().cache().tree().styles().iter() {
        if doc.name(node).unwrap() == Some("p") {
            paragraphs += 1;
            assert!(format!("{:?}", style.value(olo_web::css::properties::Property::Color)).contains("[1.0, 0.0, 0.0"), "instance {paragraphs} without its rules");
        }
    }
    assert_eq!(paragraphs, 9);
}
fn rect_of(m: &page::Measured, doc: &olo_web::dom::Document, id: &str) -> layout::Rect {
    let l = m.layout();
    let (b, _) = l.cache().tree().iter().find(|(_, b)| doc.attribute(b.node, "id").ok().flatten() == Some(id) && matches!(b.kind, layout::BoxKind::Block { .. })).unwrap();
    l.bounds(b).unwrap()
}
/// Reddit's right sidebar: a grid item clamped by its max-height (a
/// scroller, sticky), a column flex container whose first item grows
/// (`flex: 1 1 auto`) but never shrinks below its content (its automatic
/// minimum height), the legal links after it.
#[test]
fn a_column_flex_item_keeps_its_content_height_in_a_clamped_grid_item() {
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0}</style><div style='display:grid;grid-template-columns:1fr 100px;height:2000px'><div>main</div>\
         <div id=side style='display:flex;flex-direction:column;position:sticky;top:0;max-height:300px;min-height:300px;overflow:auto'>\
         <div id=contents style='flex:1 1 auto'><div style='height:400px'></div></div><div id=legal style='height:100px'></div></div></div>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let (side, contents, legal) = (rect_of(&m, &doc, "side"), rect_of(&m, &doc, "contents"), rect_of(&m, &doc, "legal"));
    assert_eq!(side.height, 300., "clamped by its max-height");
    assert_eq!(contents.height, 400., "not shrunk below its content");
    assert_eq!(legal.y, contents.y + 400.);
}
/// An element with nothing to measure (an empty inline, a custom element
/// with no content) sits where it is, as Chrome places one: after the
/// box before it, empty (Reddit's feed waits for its empty next-page
/// partial to near the viewport).
#[test]
fn an_empty_element_is_placed_where_it_sits() {
    let (mut measurer, host, doc, viewport) =
        measurer_for("<!doctype html><style>body{margin:8px}</style><div style='height:300px'>top</div><x-p id=empty></x-p><div id=after style='height:10px'></div>");
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let empty = m.placed_box(&doc, find(&doc, "empty")).unwrap();
    assert_eq!((empty.x, empty.y, empty.width, empty.height), (8., 308., 0., 0.));
    assert!(m.border_box(find(&doc, "empty")).is_none(), "nothing of its own to measure");
}
#[test]
fn a_length_margin_resolves_to_itself_not_the_room_left_over() {
    // getComputedStyle's margins as Chrome's: a 300px block in a 400px box
    // has a 0px right margin (Chart.js took the 100px left over for one and
    // drew its charts at half size); `auto` margins and percentages, the
    // used value.
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0}</style><canvas id=c style='display:block;width:300px;height:10px'></canvas>\
         <div id=auto style='width:200px;margin:0 auto'></div><div id=pct style='width:100px;margin-left:10%'></div><div id=em style='width:100px;margin-right:2em;font-size:10px'></div>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let get = |id: &str, p: &str| m.computed(find(&doc, id), p);
    assert_eq!(get("c", "margin-right").as_deref(), Some("0px"));
    assert_eq!(get("auto", "margin-left").as_deref(), Some("100px"));
    assert_eq!(get("auto", "margin-right").as_deref(), Some("100px"));
    assert_eq!(get("pct", "margin-left").as_deref(), Some("40px"));
    assert_eq!(get("em", "margin-right").as_deref(), Some("20px"));
}
#[test]
fn a_picture_a_script_loaded_measures_its_image_after_the_document_changes() {
    // The pictures bound for measuring were the document's at one revision:
    // after any change an image measured as its placeholder (0 tall with a
    // width attribute alone), its own pictures and those scripts loaded
    // alike (IMDb's carousel read its slides 0 tall).
    let (mut measurer, host, mut doc, viewport) =
        measurer_for("<!doctype html><style>body{margin:0}</style><div style='width:400px'><img id=i width=500 style='width:100%;display:block'></div><p id=p>x</p>");
    let img = find(&doc, "i");
    let picture = olo_web::image::Image::from_rgba(500, 281, vec![255; 500 * 281 * 4], page::PAGE_IMAGE_LIMITS, None).unwrap();
    let height = |m: &page::Measured, doc: &olo_web::dom::Document| {
        let l = m.layout();
        let (b, _) = l.cache().tree().iter().find(|(_, b)| doc.attribute(b.node, "id").ok().flatten() == Some("i")).unwrap();
        l.bounds(b).unwrap().height
    };
    measurer.bind_pictures(&doc, &[(img, picture.clone())], None);
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    assert!((height(&m, &doc) - 224.8).abs() < 0.1, "{}", height(&m, &doc));
    let p = find(&doc, "p");
    doc.set_attribute(p, "title", "changed").unwrap();
    measurer.bind_pictures(&doc, &[(img, picture)], None);
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, Some(m), None).unwrap();
    assert!((height(&m, &doc) - 224.8).abs() < 0.1, "{}", height(&m, &doc));
}
