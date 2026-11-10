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
#[test]
fn live_media_changes_reselect_rules_without_reloading_sheets_or_unchanged_fonts() {
    let mut b = Builder::new(Default::default());
    b.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(b.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let viewport = layout::Viewport { width: 1000., height: 700., scale: 1. };
    let media = olo_web::css::media::Media::screen(1000., 700., 1., false);
    let source = "<!doctype html><style>@font-face{font-family:unused;src:url(same.ttf)}#t{width:100px}@media(max-width:900px){#t{width:200px}}@media(prefers-color-scheme:dark){#t{width:300px}}</style><div id=t></div>";
    let prepared = page::prepare(Arc::from(source), &scope, media, false, None, |_, _| {}, |_, _, _| Ok(None)).unwrap();
    let mut measurer = prepared.measurer();
    let doc = prepared.into_document();
    let mut shaper = shaping::Engine::default();
    let mut previous = None;
    for (width, dark, expected) in [(1000., false, 100.), (800., false, 200.), (800., true, 300.), (1000., false, 100.)] {
        let viewport = layout::Viewport { width, ..viewport };
        let mut requests = 0;
        measurer
            .set_environment(viewport, dark, None, &mut |_, _, _| {
                requests += 1;
                Ok(None)
            })
            .unwrap();
        assert_eq!(requests, 0, "the sheets and unchanged font sources are retained");
        let host = paint_host::Scaled::new(measurer.scope().clone(), 1.);
        let measured = measurer.measure(&doc, &host, &mut shaper, viewport, previous, None).unwrap();
        assert_eq!(width_of(&measured, &doc, "t"), expected, "width {width}, dark {dark}, {}", measured.path());
        previous = Some(measured);
    }
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
#[test]
fn text_baselines_and_content_bounds_match_native_chrome_after_scale_changes() {
    use sha2::{Digest, Sha256};
    let source = include_str!("baseline-fixtures/baselines.html");
    let oracle: serde_json::Value = serde_json::from_str(include_str!("baseline-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(source)), oracle["source_sha256"]);
    let (_, base_host, _, mut viewport) = measurer_for("");
    viewport.width = 1000.;
    viewport.height = 650.;
    let prepared = page::prepare(
        Arc::from(source),
        &base_host.0.0,
        olo_web::css::media::Media::screen(viewport.width, viewport.height, viewport.scale, false),
        true,
        None,
        |_, _| {},
        |kind, _, url| {
            Ok((kind == page::ResourceKind::Font && url.starts_with("data:font/ttf;base64,")).then(|| page::Resource::Font(Arc::from(include_bytes!("inline-fixtures/plain.ttf").as_slice()))))
        },
    )
    .unwrap();
    let host = paint_host::Scaled::new(prepared.scope().clone(), 1.);
    let measurer = prepared.measurer();
    let doc = prepared.into_document();
    let mut shaper = shaping::Engine::default();
    let mut previous = None;
    let rows = oracle["scales"].as_array().unwrap();
    // Reuse a live document in both directions to catch device-metric caches
    // accidentally retained from the preceding scale.
    for row in rows.iter().chain(rows.iter().rev()) {
        viewport.scale = row["scale"].as_f64().unwrap();
        let host = paint_host::Scaled::new(host.0.0.clone(), viewport.scale);
        let measured = measurer.measure(&doc, &host, &mut shaper, viewport, previous, None).unwrap();
        let layout = measured.layout();
        for case in row["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let (b, text) =
                layout.cache().tree().iter().find_map(|(b, item)| (doc.attribute(item.node, "id").ok().flatten() == Some(id)).then(|| layout.text_layout(b).map(|text| (b, text))).flatten()).unwrap();
            assert_eq!(text.lines().len(), 1, "{id}");
            let baseline = layout.bounds(b).unwrap().y + text.baseline(0).unwrap();
            let expected = case["baseline"].as_f64().unwrap();
            let near = |got: f64, want: f64| assert!((got - want).abs() < 0.0002, "scale {} {id}: {got} != {want}", viewport.scale);
            near(baseline, expected);
            let top = case["span"][1].as_f64().unwrap();
            let height = case["span"][3].as_f64().unwrap();
            for fragment in text.fragments() {
                let (ascent, descent) = text.content_area(fragment);
                near(baseline - ascent, top);
                near(ascent + descent, height);
            }
        }
        previous = Some(measured);
    }
}
#[test]
fn grid_track_distribution_matches_native_chrome() {
    use sha2::{Digest, Sha256};
    let oracle: serde_json::Value = serde_json::from_str(include_str!("grid-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("grid-fixtures/tracks.html"))), oracle["source_sha256"]);
    let (measurer, host, doc, mut viewport) = measurer_for(include_str!("grid-fixtures/tracks.html"));
    let mut errors = Vec::new();
    for row in oracle["rows"].as_array().unwrap() {
        viewport.scale = row["scale"].as_f64().unwrap();
        let host = paint_host::Scaled::new(host.0.0.clone(), viewport.scale);
        let measured = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
        for case in row["cases"].as_array().unwrap() {
            for rect in case["rects"].as_array().unwrap() {
                let id = rect["id"].as_str().unwrap();
                let b = measured.border_box(find(&doc, id)).unwrap();
                for (k, got) in [b.x, b.y, b.width, b.height].into_iter().enumerate() {
                    let expected = rect["rect"][k].as_f64().unwrap();
                    if (got - expected).abs() > 0.0001 {
                        errors.push(format!("{} #{id} coordinate {k}: {got} != {expected}", viewport.scale));
                    }
                }
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
#[test]
fn grid_track_distribution_updates_after_size_and_gap_changes() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("grid-fixtures/chrome.json")).unwrap();
    let (measurer, host, mut doc, mut viewport) = measurer_for(include_str!("grid-fixtures/tracks.html"));
    let grid = find(&doc, "columns-thirds-100");
    for row in oracle["rows"].as_array().unwrap() {
        viewport.scale = row["scale"].as_f64().unwrap();
        let host = paint_host::Scaled::new(host.0.0.clone(), viewport.scale);
        let mut shaper = shaping::Engine::default();
        let mut previous = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
        for (width, gap) in [(960., 16.), (101.3, 1.2), (100., 0.)] {
            doc.set_attribute(grid, "style", &format!("display:grid;width:{width}px;height:12px;grid-template-rows:12px;grid-template-columns:repeat(3,1fr);gap:{gap}px")).unwrap();
            let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(previous), None).unwrap();
            let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
            let reference = row["cases"].as_array().unwrap().iter().find(|c| c["id"] == format!("columns-thirds-{width}")).unwrap();
            for i in 0..3 {
                let node = find(&doc, &format!("columns-thirds-100-{i}"));
                let b = next.border_box(node).unwrap();
                assert_eq!(Some(b), fresh.border_box(node));
                for (k, got) in [b.x, b.y, b.width, b.height].into_iter().enumerate() {
                    assert!((got - reference["rects"][i + 1]["rect"][k].as_f64().unwrap()).abs() < 0.0001);
                }
            }
            previous = next;
        }
    }
}
#[test]
fn outside_markers_reserve_room_and_align_to_the_first_content_baseline() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("marker-fixtures/chrome.json")).unwrap();
    let (measurer, host, doc, mut viewport) = measurer_for(include_str!("marker-fixtures/markers.html"));
    for row in oracle["rows"].as_array().unwrap() {
        viewport.scale = row["scale"].as_f64().unwrap();
        let host = paint_host::Scaled::new(host.0.0.clone(), viewport.scale);
        let measured = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
        for case in row["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let n = find(&doc, id);
            let rect = measured.border_box(n).unwrap();
            for (key, got) in [("x", rect.x), ("y", rect.y), ("width", rect.width), ("height", rect.height)] {
                let expected = case["rect"][key].as_f64().unwrap();
                assert!((got - expected).abs() <= 1. / 64., "{id} {key} at {}x: {got} != Chrome {expected}", viewport.scale);
            }
        }
        assert_eq!(measured.inner_text(&doc, find(&doc, "empty")).as_deref(), Some(""));
    }
}
#[test]
fn marker_styles_survive_reuse_and_change_with_their_originating_item() {
    use olo_web::css::selectors::PseudoElement::Marker;
    let (mut measurer, host, mut doc, viewport) = measurer_for(
        "<!doctype html><style>li::marker{color:rgb(220,0,0);font-size:40px}li.changed::marker{color:rgb(0,0,220);font-size:20px}</style>\
         <ul><li id=a>A B</li><li id=b>C D</li></ul><div id=other>E</div>",
    );
    let mut shaper = shaping::Engine::default();
    let mut last = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let a = find(&doc, "a");
    assert_eq!(last.computed_pseudo(a, Some(Marker), "color").as_deref(), Some("rgb(220, 0, 0)"));
    for (id, name, value) in [("other", "style", "color:green"), ("a", "class", "changed"), ("a", "class", ""), ("a", "style", "display:block"), ("a", "style", "")] {
        doc.set_attribute(find(&doc, id), name, value).unwrap();
        let m = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).unwrap();
        let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
        for id in ["a", "b", "other"] {
            let n = find(&doc, id);
            assert_eq!(m.border_box(n), fresh.border_box(n), "{id} after {name}={value}");
            assert_eq!(m.computed_pseudo(n, Some(Marker), "color"), fresh.computed_pseudo(n, Some(Marker), "color"));
            assert_eq!(m.computed_pseudo(n, Some(Marker), "font-size"), fresh.computed_pseudo(n, Some(Marker), "font-size"));
        }
        assert_as_fresh(&m, &mut measurer, &host, &doc, viewport);
        last = m;
    }
}
fn marker_number(m: &page::Measured, node: olo_web::dom::NodeId) -> String {
    let tree = m.layout().cache().tree();
    let mut text = String::new();
    for (_, b) in tree.iter() {
        if let layout::BoxKind::InlineContent { source } = b.kind {
            let source = tree.text_source(source).unwrap();
            for span in source.spans.iter().filter(|s| s.node == node) {
                text.push_str(&source.source[span.range.start as usize..span.range.end as usize]);
            }
        }
    }
    // Numbering only: the existing marker spacing/glyph differences are
    // tracked separately. Chrome's AX marker uses a space, ours a NBSP.
    text.replace('\u{fffc}', "").trim_end().to_owned()
}
#[test]
fn list_numbering_matches_chrome() {
    use sha2::{Digest, Sha256};
    let oracle: serde_json::Value = serde_json::from_str(include_str!("list-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("list-fixtures/lists.html"))), oracle["source_sha256"]);
    let (measurer, host, doc, viewport) = measurer_for(include_str!("list-fixtures/lists.html"));
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    for row in oracle["rows"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let expected = row["markers"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect::<String>();
        assert_eq!(marker_number(&m, find(&doc, id)), expected.trim_end(), "#{id}");
    }
}
#[test]
fn css_counter_sequencing_matches_chrome() {
    use sha2::{Digest, Sha256};
    let oracle: serde_json::Value = serde_json::from_str(include_str!("counter-fixtures/chrome.json")).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("counter-fixtures/counters.html"))), oracle["source_sha256"]);
    let (measurer, host, doc, viewport) = measurer_for(include_str!("counter-fixtures/counters.html"));
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let mut errors = Vec::new();
    for row in oracle["rows"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let got = marker_number(&m, find(&doc, id));
        let expected = row["generated"].as_str().unwrap().trim_end();
        if got != expected {
            errors.push(format!("#{id}: {got:?} != {expected:?}"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
#[test]
fn css_counters_follow_pseudo_and_element_mutations() {
    let (measurer, host, mut doc, viewport) = measurer_for(
        "<!doctype html><style>body{counter-reset:n 0}#a::before{content:counter(n);counter-increment:n 2}#b::before{content:counter(n)}#a.off::before{display:none}#a.contents::before{display:contents}</style><div id=a>A</div><div id=b>B</div>",
    );
    let mut shaper = shaping::Engine::default();
    let mut last = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    for (attribute, value, expected) in [
        ("style", "display:none", ["", "0"]),
        ("style", "display:contents", ["2", "2"]),
        ("class", "off", ["", "0"]),
        ("class", "contents", ["0", "0"]),
        ("class", "", ["2", "2"]),
        ("style", "counter-reset:n 10", ["12", "0"]),
        ("style", "counter-set:n 10", ["12", "12"]),
        ("style", "counter-increment:n 3", ["5", "5"]),
    ] {
        doc.set_attribute(find(&doc, "a"), attribute, value).unwrap();
        let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).unwrap();
        let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
        for (id, expected) in ["a", "b"].into_iter().zip(expected) {
            let n = find(&doc, id);
            assert_eq!(marker_number(&next, n), expected, "#{id} after {attribute}={value}");
            assert_eq!(marker_number(&next, n), marker_number(&fresh, n));
            assert_eq!(next.border_box(n), fresh.border_box(n));
        }
        last = next;
    }
}
#[test]
fn css_counter_scope_storage_is_bounded() {
    let source = format!("<!doctype html><body id=body>{}", (0..1024).map(|i| format!("<div style='counter-reset:n{i}'>A</div>")).collect::<String>());
    let (measurer, host, mut doc, viewport) = measurer_for(&source);
    let mut shaper = shaping::Engine::default();
    let last = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let body = find(&doc, "body");
    let extra = doc.create_element("div", olo_web::dom::Namespace::Html).unwrap();
    doc.set_attribute(extra, "style", "counter-reset:overflow").unwrap();
    doc.append_child(body, extra).unwrap();
    let error = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).err().expect("a counter beyond the active scope limit fails explicitly");
    assert!(matches!(error.downcast_ref::<layout::Error>(), Some(layout::Error::Limit)), "{error}");
}
#[test]
fn list_numbering_follows_mutations_and_reuse() {
    let (measurer, host, mut doc, viewport) = measurer_for("<!doctype html><ol id=o><li id=a>A</li><li id=b>B</li><li id=c>C</li></ol>");
    let mut shaper = shaping::Engine::default();
    let mut last = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    for (id, attr, value, expected) in [
        ("o", "reversed", "", ["3.", "2.", "1."]),
        ("o", "start", " +10x", ["10.", "9.", "8."]),
        ("b", "value", "-2", ["10.", "-2.", "-3."]),
        ("b", "value", "2147483648", ["10.", "9.", "8."]),
        ("o", "start", "invalid", ["3.", "2.", "1."]),
        ("b", "style", "list-style-type:none", ["3.", "", "1."]),
        ("b", "style", "display:none", ["2.", "", "1."]),
        ("b", "style", "", ["3.", "2.", "1."]),
    ] {
        doc.set_attribute(find(&doc, id), attr, value).unwrap();
        let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).unwrap();
        let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
        for (id, expected) in ["a", "b", "c"].into_iter().zip(expected) {
            let n = find(&doc, id);
            assert_eq!(marker_number(&next, n), expected, "#{id} after {attr}={value}");
            assert_eq!(marker_number(&next, n), marker_number(&fresh, n));
            assert_eq!(next.border_box(n), fresh.border_box(n));
        }
        last = next;
    }
    doc.remove_attribute(find(&doc, "o"), "reversed").unwrap();
    let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(last), None).unwrap();
    assert_eq!(marker_number(&next, find(&doc, "a")), "1.");
    assert_eq!(marker_number(&next, find(&doc, "c")), "3.");
    // Over the patch budget: the incremental style job must invalidate
    // ordinals too, even though these attributes change no CSS value.
    for i in 0..257 {
        doc.set_attribute(find(&doc, "a"), &format!("data-change-{i}"), "x").unwrap();
    }
    doc.set_attribute(find(&doc, "o"), "reversed", "").unwrap();
    let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(next), None).unwrap();
    assert_eq!(marker_number(&next, find(&doc, "a")), "3.");
    let added = doc.create_element("li", olo_web::dom::Namespace::Html).unwrap();
    doc.append_child(find(&doc, "o"), added).unwrap();
    let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(next), None).unwrap();
    assert_eq!(marker_number(&next, find(&doc, "a")), "4.");
    assert_eq!(marker_number(&next, added), "1.");
    doc.detach(added).unwrap();
    let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(next), None).unwrap();
    assert_eq!(marker_number(&next, find(&doc, "a")), "3.");
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
fn a_pseudo_element_answers_its_own_computed_style() {
    // getComputedStyle(el, '::before') as Chrome answers it: the pseudo's
    // own values and box (Olo gave the element's); one nothing styles
    // inherits from its element and is otherwise initial.
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0;color:rgb(10,20,30)} #a::before{content:\"x\" attr(id) counter(n) / \"alt\";display:block;width:50%;padding:3px;color:red} #a::after{content:'y\"'}</style>\
         <div id=a style='width:200px'>hi</div><div id=b style='color:green'>no</div>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    use olo_web::css::selectors::PseudoElement::{After, Before};
    let get = |id: &str, p, name: &str| m.computed_pseudo(find(&doc, id), p, name);
    assert_eq!(get("a", Some(Before), "content").as_deref(), Some("\"x\" attr(id) counter(n)"));
    assert_eq!(get("a", Some(Before), "width").as_deref(), Some("100px"));
    assert_eq!(get("a", Some(Before), "padding-top").as_deref(), Some("3px"));
    assert_eq!(get("a", Some(Before), "color").as_deref(), Some("rgb(255, 0, 0)"));
    assert_eq!(get("a", Some(After), "content").as_deref(), Some("\"y\\\"\""));
    assert_eq!(get("a", None, "width").as_deref(), Some("200px"));
    assert_eq!(get("a", None, "content").as_deref(), Some("normal"));
    assert_eq!(get("b", Some(Before), "color").as_deref(), Some("rgb(0, 128, 0)"), "inherited from its element");
    assert_eq!(get("b", Some(Before), "width"), None, "initial: the caller's to answer");
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
/// A flex item's text that may break anywhere (`word-break: break-word`,
/// `overflow-wrap: anywhere`) shrinks it to the room left, as in Chrome
/// (CSS Text 3 §5.5: those breaks count at min-content; `overflow-wrap:
/// break-word`'s don't). Stack Overflow's titles held their column wide.
/// And a line clamp ends its box below its Nth line.
#[test]
fn breaking_anywhere_shrinks_min_content_and_a_line_clamp_ends_the_box() {
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0;font:16px sans-serif} .f{display:flex;width:400px} .s{width:100px;flex-shrink:0}</style>\
         <div class=f><div class=s>x</div><div id=a style='flex:1 auto;word-break:break-word'>sys.dm_exec_query_stats/sys.dm_exec_sql_text</div></div>\
         <div class=f><div class=s>x</div><div id=b style='flex:1 auto;overflow-wrap:anywhere'>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</div></div>\
         <div class=f><div class=s>x</div><div id=c style='flex:1 auto;overflow-wrap:break-word'>bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb</div></div>\
         <p id=d style='width:300px;margin:0;line-height:20px;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden'>I have been using these DMVs for years to look at performance metrics, but since moving our servers numbers do not match.</p>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let rect = |id: &str| m.border_box(find(&doc, id)).unwrap();
    assert_eq!(rect("a").width, 300.);
    assert_eq!(rect("b").width, 300.);
    assert!(rect("c").width > 300., "break-word's breaks don't count at min-content");
    assert_eq!(rect("d").height, 40., "two lines of twenty");
}
/// A column flex container's and a grid's items with a min-, max- or
/// fit-content width are sized by their content, not stretched (Pixelscan's
/// `width: fit-content` button spanned its whole column).
#[test]
fn content_sized_widths_are_not_stretched_across() {
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0;font:16px sans-serif} .c{display:flex;flex-direction:column;width:600px} .g{display:grid;width:600px}</style>\
         <div class=c><div id=a style='width:max-content'>max content</div><div id=b style='width:fit-content;padding:0 40px'>fit content</div><div id=c>auto</div></div>\
         <div class=g><div id=d style='width:max-content'>max content</div><div id=e>auto</div></div>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    let w = |id: &str| m.border_box(find(&doc, id)).unwrap().width;
    assert!(w("a") < 300., "{}", w("a"));
    assert!(w("b") < 350. && w("b") > 80., "{}", w("b"));
    assert_eq!(w("c"), 600.);
    assert_eq!(w("d"), w("a"));
    assert_eq!(w("e"), 600.);
}
/// An SVG with a viewBox, `width: auto; height: 100%` of a parent whose
/// height is a length, contributes its ratio's width to a shrink-to-fit
/// parent, as in Chrome (Pixelscan's icons measured 150px, Chrome 24).
#[test]
fn a_ratio_svg_with_a_percentage_height_contributes_its_ratio_width() {
    let (mut measurer, host, doc, viewport) = measurer_for(
        "<!doctype html><style>body{margin:0}</style>\
         <span id=a style='display:inline-flex;height:24px'><svg viewBox='0 0 256 256' style='display:block;width:auto;height:100%'></svg></span>\
         <div id=b style='display:inline-flex;height:40px'><svg viewBox='0 0 200 100' style='display:block;width:auto;height:100%'></svg></div>",
    );
    let m = measurer.measure(&doc, &host, &mut shaping::Engine::default(), viewport, None, None).unwrap();
    assert_eq!(m.border_box(find(&doc, "a")).unwrap().width, 24.);
    assert_eq!(m.border_box(find(&doc, "b")).unwrap().width, 80.);
}

#[test]
fn in_place_svg_style_patches_refresh_compiled_paint() {
    let (measurer, host, mut doc, viewport) =
        measurer_for("<!doctype html><style>.selected path{fill:blue}</style><div id=owner><svg width=40 height=20><path d='M0 0H40V20H0Z' fill='red'/></svg></div>");
    let mut shaper = shaping::Engine::default();
    let mut previous = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let owner = previous.layout().cache().tree().styles().iter().find(|(n, _)| doc.attribute(*n, "id").unwrap() == Some("owner")).unwrap().0;
    let geometry = previous.layout().geometry_identity();
    for (class, color) in [("selected", [0, 0, 255, 255]), ("", [255, 0, 0, 255]), ("selected", [0, 0, 255, 255])] {
        doc.set_attribute(owner, "class", class).unwrap();
        let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(previous), None).unwrap();
        assert!(next.path().starts_with("patched in place"), "{}", next.path());
        assert_eq!(next.layout().cache().tree().vector(0).unwrap().shapes[0].color, color);
        assert_eq!(next.layout().geometry_identity(), geometry);
        previous = next;
    }
}

#[test]
fn shared_styles_do_not_hide_a_svg_reference_id_change() {
    let (measurer, host, mut doc, viewport) =
        measurer_for("<!doctype html><svg width=40 height=20><defs><linearGradient id=g><stop stop-color='red'/></linearGradient></defs><path d='M0 0H40V20H0Z' fill='url(#g)'/></svg>");
    let mut shaper = shaping::Engine::default();
    let previous = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    assert_eq!(previous.layout().cache().tree().vector(0).unwrap().shapes[0].color, [255, 0, 0, 255]);
    let shared = previous.layout().cache().tree().styles().clone();
    let gradient = shared.iter().find(|(n, _)| doc.attribute(*n, "id").unwrap() == Some("g")).unwrap().0;
    doc.set_attribute(gradient, "id", "gone").unwrap();
    let next = measurer.measure(&doc, &host, &mut shaper, viewport, Some(previous), None).unwrap();
    assert!(next.path().starts_with("patched a copy"), "{}", next.path());
    let fresh = measurer.measure(&doc, &host, &mut shaper, viewport, None, None).unwrap();
    let colors = |m: &page::Measured| m.layout().cache().tree().vector(0).unwrap().shapes.iter().map(|s| s.color).collect::<Vec<_>>();
    assert_eq!(colors(&next), colors(&fresh));
}
