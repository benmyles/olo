use olo_web::{
    dom::{Document, Namespace, NodeId},
    html::{
        self,
        controls::{DisabledStates, Error, Limits},
    },
};
use std::sync::{Arc, atomic::AtomicBool};
#[path = "support/paint_host.rs"]
mod paint_host;

fn nodes(doc: &Document) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(doc.children(node).unwrap());
    }
    out
}
fn id(doc: &Document, value: &str) -> NodeId {
    nodes(doc).into_iter().find(|node| doc.name(*node).unwrap().is_some() && doc.attribute(*node, "id").unwrap() == Some(value)).unwrap()
}
fn document() -> Document {
    html::parse(Arc::from(include_str!("control-fixtures/input.html")), Default::default(), None).unwrap().document
}
#[test]
fn css_control_states_match_independent_chrome_across_tree_and_attribute_changes() {
    let reference: serde_json::Value = serde_json::from_str(include_str!("control-fixtures/chrome.json")).unwrap();
    let mut doc = document();
    let mut checked = 0;
    for phase in reference["rows"].as_array().unwrap() {
        match phase["id"].as_str().unwrap() {
            "initial" => {}
            "move-first-legend" => {
                doc.append_child(id(&doc, "disabled-fieldset"), id(&doc, "first-legend")).unwrap();
            }
            "enable-fieldset" => {
                doc.remove_attribute(id(&doc, "disabled-fieldset"), "disabled").unwrap();
            }
            "disable-option" => {
                doc.set_attribute(id(&doc, "inside-disabled-select"), "disabled", "").unwrap();
            }
            unknown => panic!("unimplemented independent operation {unknown}"),
        }
        let states = DisabledStates::new(&doc, Default::default(), None).unwrap();
        assert_eq!(states.payload_bytes(), states.control_count() * 8);
        for element in phase["elements"].as_array().unwrap() {
            let name = element["id"].as_str().unwrap();
            let node = id(&doc, name);
            assert_eq!(doc.name(node).unwrap().unwrap(), element["tag"].as_str().unwrap());
            let state = states.disabled(&doc, node).unwrap();
            assert_eq!(state == Some(true), element["disabled"].as_bool().unwrap(), "{name} in {}", phase["id"]);
            assert_eq!(state == Some(false), element["enabled"].as_bool().unwrap(), "{name} in {}", phase["id"]);
            checked += 1;
        }
    }
    assert_eq!(checked, 176);
}
#[test]
fn control_state_bounds_cancellation_and_generation_validation() {
    let mut doc = document();
    let node = id(&doc, "text");
    let states = DisabledStates::new(&doc, Default::default(), None).unwrap();
    assert!(states.control_count() < 100);
    let other = document();
    assert_eq!(states.disabled(&other, other.root()), Err(Error::Stale));
    assert!(matches!(states.disabled(&doc, other.root()), Err(Error::Dom(_))));
    let cancelled = AtomicBool::new(true);
    assert!(matches!(DisabledStates::new(&doc, Default::default(), Some(&cancelled)), Err(Error::Cancelled)));
    for limits in [
        Limits { nodes: 0, ..Default::default() },
        Limits { controls: 0, ..Default::default() },
        Limits { depth: 0, ..Default::default() },
        Limits { depth: 2, ..Default::default() },
        Limits { work: 0, ..Default::default() },
    ] {
        assert!(matches!(DisabledStates::new(&doc, limits, None), Err(Error::Limit)));
    }
    doc.set_attribute(node, "disabled", "").unwrap();
    assert_eq!(states.disabled(&doc, node), Err(Error::Stale));
    assert_eq!(DisabledStates::new(&doc, Default::default(), None).unwrap().disabled(&doc, node), Ok(Some(true)));
    let orphan = doc.create_element("input", Namespace::Html).unwrap();
    assert_eq!(DisabledStates::new(&doc, Default::default(), None).unwrap().disabled(&doc, orphan), Err(Error::Disconnected));
}

#[test]
fn page_css_uses_control_states_and_rebuilds_after_dom_changes() {
    use olo_web::{
        font::{Builder, FontSet, GenericPolicy},
        layout, page, paint,
        text::shaping,
    };
    let mut fonts = Builder::new(Default::default());
    fonts.load_file(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
    let scope = FontSet::new(fonts.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap();
    let host = paint_host::Scaled::new(scope.clone(), 1.);
    let mut shaper = shaping::Engine::default();
    let mut painter = paint::Engine::default();
    let viewport = layout::Viewport { width: 400., height: 200., scale: 1. };
    let mut page = page::load(Arc::from("<!doctype html><style>fieldset,legend,span{display:block}input{display:none}span{color:blue}input:disabled+span{color:red}input:enabled+span{color:green}</style><fieldset id=f disabled><legend><input><span>Exempt</span></legend><input><span>Disabled</span></fieldset>"), &host, &scope, &mut shaper, &mut painter, viewport, None, |_,_|{}).unwrap();
    let color = |page: &page::Page, name| {
        page.scene
            .commands()
            .iter()
            .enumerate()
            .find_map(|(i, c)| {
                if page.scene.text_content(i as u32).as_deref() == Some(name)
                    && let paint::Primitive::Text { color, .. } = c.primitive
                {
                    return Some(color);
                }
                None
            })
            .unwrap()
    };
    assert_eq!(color(&page, "Exempt"), [0, 128, 0, 255]);
    assert_eq!(color(&page, "Disabled"), [255, 0, 0, 255]);
    page.document.remove_attribute(id(&page.document, "f"), "disabled").unwrap();
    page.restyle(&host, &scope, &mut shaper, &mut painter, viewport, None, |_, _| {}).unwrap();
    assert_eq!(color(&page, "Disabled"), [0, 128, 0, 255]);
}
