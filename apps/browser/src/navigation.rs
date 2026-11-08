//! Initial light-document fragment lookup and keyboard-link geometry.
use crate::links::{self, Link};
use olo_web::{
    dom::{Document, Namespace, NodeId, NodeType},
    layout, page,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use url::Url;
#[derive(Clone, Debug)]
pub struct FocusTarget {
    pub link: Arc<Link>,
    pub rect: layout::Rect,
}
fn check(cancel: &AtomicBool) -> page::Result<()> {
    if cancel.load(Ordering::Relaxed) { Err("Cancelled".into()) } else { Ok(()) }
}
fn next(doc: &Document, node: NodeId) -> page::Result<Option<NodeId>> {
    if let Some(child) = doc.first_child(node)? {
        return Ok(Some(child));
    }
    let mut node = node;
    loop {
        if let Some(sibling) = doc.next_sibling(node)? {
            return Ok(Some(sibling));
        }
        if let Some(parent) = doc.parent(node)? {
            node = parent;
        } else {
            return Ok(None);
        }
    }
}
pub fn fragment(doc: &Document, value: Option<&str>, cancel: &AtomicBool) -> page::Result<Option<NodeId>> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if let Some(node) = potential(doc, value, cancel)? {
        return Ok(Some(node));
    }
    let decoded = percent_encoding::percent_decode_str(value).decode_utf8_lossy();
    if decoded.as_ref() == value { Ok(None) } else { potential(doc, &decoded, cancel) }
}
fn potential(doc: &Document, value: &str, cancel: &AtomicBool) -> page::Result<Option<NodeId>> {
    let mut named = None;
    let mut node = Some(doc.root());
    while let Some(at) = node {
        check(cancel)?;
        if doc.node_type(at)? == NodeType::Element {
            if doc.attribute(at, "id")? == Some(value) {
                return Ok(Some(at));
            }
            if named.is_none() && doc.namespace(at)? == Some(Namespace::Html) && doc.name(at)? == Some("a") && doc.attribute(at, "name")? == Some(value) {
                named = Some(at);
            }
        }
        node = next(doc, at)?;
    }
    Ok(named)
}
pub fn target_rect(page: &page::Page, target: NodeId, cancel: &AtomicBool) -> page::Result<Option<layout::Rect>> {
    let scene = &page.scene;
    for (id, node) in scene.layout().cache().tree().iter() {
        check(cancel)?;
        if node.node == target {
            return Ok(scene.layout().bounds(id));
        }
    }
    for (i, _) in scene.commands().iter().enumerate() {
        check(cancel)?;
        let mut node = scene.node(i as u32);
        while let Some(at) = node {
            if at == target {
                if let Some(rect) = scene.visible_bounds(i as u32) {
                    return Ok(Some(rect));
                }
                break;
            }
            node = page.document.parent(at)?;
        }
    }
    Ok(None)
}
fn tabindex(value: Option<&str>) -> i32 {
    let Some(value) = value else {
        return 0;
    };
    let value = value.trim_start_matches(['\t', '\n', '\r', '\x0c', ' ']);
    let end = value.char_indices().skip(usize::from(value.starts_with(['+', '-']))).find(|(_, c)| !c.is_ascii_digit()).map_or(value.len(), |(i, _)| i);
    value[..end].parse().unwrap_or(0)
}
#[derive(Default, Debug)]
pub struct FocusTargets {
    entries: Box<[FocusTarget]>,
    positions: HashMap<NodeId, usize>,
}
impl std::ops::Deref for FocusTargets {
    type Target = [FocusTarget];
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl FocusTargets {
    pub fn position(&self, node: NodeId) -> Option<usize> {
        self.positions.get(&node).copied()
    }
}
pub fn focus_targets(page: &page::Page, base: &Url, origin: &Url, cache: &mut olo_web::dom::NodeMap<Option<Arc<Link>>>, cancel: &AtomicBool) -> page::Result<Arc<FocusTargets>> {
    let mut targets: HashMap<NodeId, FocusTarget> = HashMap::new();
    for (i, _) in page.scene.commands().iter().enumerate() {
        check(cancel)?;
        let Some(node) = page.scene.node(i as u32) else {
            continue;
        };
        let Some(rect) = page.scene.visible_bounds(i as u32) else {
            continue;
        };
        let Some(link) = links::resolve(&page.document, node, base, origin, cache)? else {
            continue;
        };
        targets
            .entry(link.node)
            .and_modify(|t| {
                let r = t.rect;
                let x = r.x.min(rect.x);
                let y = r.y.min(rect.y);
                t.rect = layout::Rect { x, y, width: (r.x + r.width).max(rect.x + rect.width) - x, height: (r.y + r.height).max(rect.y + rect.height) - y };
            })
            .or_insert(FocusTarget { link, rect });
    }
    let mut ordered = Vec::new();
    let mut pending = vec![(page.document.root(), false)];
    while let Some((at, inherited_inert)) = pending.pop() {
        check(cancel)?;
        let inert = inherited_inert || page.document.node_type(at)? == NodeType::Element && page.document.attribute(at, "inert")?.is_some();
        if let Some(target) = targets.remove(&at) {
            let index = tabindex(page.document.attribute(at, "tabindex")?);
            if index >= 0 && !inert {
                ordered.push((index, target));
            }
        }
        if let Some(sibling) = page.document.next_sibling(at)? {
            pending.push((sibling, inherited_inert));
        }
        if let Some(child) = page.document.first_child(at)? {
            pending.push((child, inert));
        }
    }
    ordered.sort_by_key(|(index, _)| (*index == 0, *index));
    let entries: Box<[_]> = ordered.into_iter().map(|(_, target)| target).collect();
    let positions = entries.iter().enumerate().map(|(i, t)| (t.link.node, i)).collect();
    Ok(Arc::new(FocusTargets { entries, positions }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_web::{
        font::{Builder, FontSet, GenericPolicy},
        html, paint,
        text::shaping,
    };
    use std::path::Path;
    fn attr(doc: &Document, node: NodeId, name: &str) -> String {
        doc.attribute(node, name).unwrap().unwrap().to_owned()
    }
    fn scope() -> olo_web::font::Scope {
        let mut builder = Builder::new(Default::default());
        builder.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/web/tests/inline-fixtures/plain.ttf"), |_| {}, None).unwrap();
        FontSet::new(builder.finish(None).unwrap(), GenericPolicy::new(&["Olo Inline plain"]).unwrap(), Default::default()).unwrap().scope("en").unwrap()
    }
    #[test]
    fn fragments_follow_raw_then_decoded_id_and_name_precedence() {
        let doc=html::parse(Arc::from("<!doctype html><a id=named name='raw%20id'>name</a><p id='raw id'>decoded</p><a name=same id=name>name</a><p id=same>id</p><p id='Café'>unicode</p><p id=''>empty</p><p id=dup>first</p><p id=dup>second</p>"),Default::default(),None).unwrap().document;
        let cancel = AtomicBool::new(false);
        let raw = fragment(&doc, Some("raw%20id"), &cancel).unwrap().unwrap();
        assert_eq!(attr(&doc, raw, "id"), "named");
        assert_eq!(attr(&doc, fragment(&doc, Some("%72aw%20id"), &cancel).unwrap().unwrap(), "id"), "raw id");
        assert_eq!(attr(&doc, fragment(&doc, Some("same"), &cancel).unwrap().unwrap(), "id"), "same");
        assert_eq!(attr(&doc, fragment(&doc, Some("Caf%C3%A9"), &cancel).unwrap().unwrap(), "id"), "Café");
        assert!(fragment(&doc, Some(""), &cancel).unwrap().is_none());
        let duplicate = fragment(&doc, Some("dup"), &cancel).unwrap().unwrap();
        assert_eq!(doc.value(doc.first_child(duplicate).unwrap().unwrap()).unwrap(), Some("first"));
        assert!(fragment(&doc, Some("missing"), &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn keyboard_targets_include_offscreen_links_with_tabindex_and_inert_filtering() {
        let scope = scope();
        let key = crate::worker::Key { source: "about:engine".into(), cwd: None, revision: 0, width: 200, height: 100, scale: 1., palette: Arc::new(olo_theme::Palette::light()) };
        let host = crate::worker::PageHost::new(&scope, &key);
        let mut shaper = shaping::Engine::default();
        let mut painter = paint::Engine::default();
        let page=page::load(Arc::from("<!doctype html><style>body{margin:0}p{height:200px}</style><a href=zero>Z</a><a href=two tabindex='2junk'>B</a><a href=one tabindex=1>A</a><a href=skip tabindex=-1>S</a><div inert><a href=inert>I</a></div><a hidden href=hidden>H</a><p></p><a href=offscreen>O</a>"),&host,&scope,&mut shaper,&mut painter,layout::Viewport{width:200.,height:100.,scale:1.},None,|_,_|{}).unwrap();
        let origin = Url::parse("https://example.test/").unwrap();
        let targets = focus_targets(&page, &origin, &origin, &mut Default::default(), &AtomicBool::new(false)).unwrap();
        let urls: Vec<_> = targets.iter().map(|t| t.link.url.as_ref()).collect();
        assert_eq!(urls, ["https://example.test/one", "https://example.test/two", "https://example.test/zero", "https://example.test/offscreen"]);
        assert!(targets.last().unwrap().rect.y > 100.);
        assert_eq!(targets.position(targets[2].link.node), Some(2));
    }
    #[test]
    #[ignore = "Release component measurement, run explicitly with --ignored --nocapture"]
    fn navigation_index_cost() {
        let scope = scope();
        let cancel = AtomicBool::new(false);
        let origin = Url::parse("https://example.test/").unwrap();
        for count in [100, 1000] {
            let mut source = String::from("<!doctype html><style>body{margin:0}</style>");
            for i in 0..count {
                source.push_str(&format!("<div><span><a href='/page/{i}'>Link {i}</a></span></div>"));
            }
            let key = crate::worker::Key { source: "about:engine".into(), cwd: None, revision: 0, width: 800, height: 600, scale: 1., palette: Arc::new(olo_theme::Palette::light()) };
            let host = crate::worker::PageHost::new(&scope, &key);
            let page =
                page::load(source.into(), &host, &scope, &mut shaping::Engine::default(), &mut paint::Engine::default(), layout::Viewport { width: 800., height: 600., scale: 1. }, None, |_, _| {})
                    .unwrap();
            let mut samples = Vec::new();
            for _ in 0..31 {
                let mut cache = olo_web::dom::NodeMap::default();
                let start = std::time::Instant::now();
                let targets = focus_targets(&page, &origin, &origin, &mut cache, &cancel).unwrap();
                samples.push(start.elapsed().as_secs_f64() * 1e6);
                assert_eq!(targets.len(), count);
            }
            samples.remove(0);
            samples.sort_by(f64::total_cmp);
            println!(
                "{}",
                serde_json::json!({"scope":"cold link ancestry + focus index over retained page; excludes parse/layout, tiles, GPU and presentation", "links":count,"samples_us":samples,"median_us":(samples[14]+samples[15])/2.})
            );
        }
    }
}
