//! Native DOM ownership mirrored in traced V8 internal fields. The identity table is
//! weak: a connected node survives through the document, a detached subtree
//! through any script reference into it, and an unreachable cycle can die.
use super::{Shared, State, node_of, wrap};
use olo_web::dom::{NodeId, NodeSet, NodeType};
use std::{ffi::c_void, rc::Rc};

/// Decoded assets charged once per shared image, even when several elements
/// use it. Accounting changes with bindings, never by scanning each frame.
#[derive(Default)]
pub(super) struct Images {
    nodes: olo_web::dom::NodeMap<std::sync::Arc<olo_web::image::Image>>,
    assets: std::collections::HashMap<usize, (usize, usize)>,
    bytes: usize,
}
impl std::ops::Deref for Images {
    type Target = olo_web::dom::NodeMap<std::sync::Arc<olo_web::image::Image>>;
    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}
impl Images {
    pub fn insert(&mut self, node: NodeId, image: std::sync::Arc<olo_web::image::Image>) {
        self.remove(&node);
        let key = std::sync::Arc::as_ptr(&image) as usize;
        let entry = self.assets.entry(key).or_insert((0, image.payload_bytes()));
        if entry.0 == 0 {
            self.bytes += entry.1;
        }
        entry.0 += 1;
        self.nodes.insert(node, image);
    }
    pub fn remove(&mut self, node: &NodeId) -> Option<std::sync::Arc<olo_web::image::Image>> {
        let image = self.nodes.remove(node)?;
        let key = std::sync::Arc::as_ptr(&image) as usize;
        let entry = self.assets.get_mut(&key).unwrap();
        entry.0 -= 1;
        if entry.0 == 0 {
            self.bytes -= entry.1;
            self.assets.remove(&key);
        }
        Some(image)
    }
}

pub(super) fn account_images(isolate: &mut v8::Isolate, state: &Shared) {
    let delta = {
        let mut s = state.borrow_mut();
        reap(&mut s);
        let bytes = s.images.bytes;
        let delta = bytes as i64 - s.image_accounted as i64;
        s.image_accounted = bytes;
        delta
    };
    // This can start GC. Neither State nor the finalizer queue is borrowed.
    if delta != 0 {
        isolate.adjust_amount_of_external_allocated_memory(delta);
    }
}

pub(super) struct Wrapper {
    handle: v8::Weak<v8::Object>,
    // The External in the V8 object points here, until that object is dead.
    _id: Box<NodeId>,
    parent: Option<NodeId>,
    children: NodeSet,
}

pub(super) fn get<'s>(scope: &v8::PinScope<'s, '_>, state: &Shared, node: NodeId) -> Option<v8::Local<'s, v8::Object>> {
    state.borrow().wrappers.get(&node)?.handle.to_local(scope)
}

pub(super) fn parent(state: &State, node: NodeId) -> Option<NodeId> {
    let doc = &state.document;
    doc.parent(node).ok().flatten().or_else(|| doc.shadow_host(node)).or_else(|| doc.template_host(node).ok().flatten())
}

pub(super) fn insert(scope: &mut v8::PinScope, state: &Shared, node: NodeId, object: v8::Local<v8::Object>) -> Option<()> {
    let mut id = Box::new(node);
    let external = v8::External::new(scope, (&mut *id as *mut NodeId).cast::<c_void>());
    object.set_internal_field(0, external.into());
    let collected = Rc::clone(&state.borrow().collected_wrappers);
    let handle = v8::Weak::with_finalizer(scope, object, Box::new(move |_| collected.borrow_mut().push(node)));
    state.borrow_mut().wrappers.insert(node, Wrapper { handle, _id: id, parent: None, children: Default::default() });
    reparent(scope, state, node);
    Some(())
}

pub(super) fn reparent(scope: &mut v8::PinScope, state: &Shared, node: NodeId) {
    let Some(object) = get(scope, state, node) else { return };
    let (old, new) = {
        let s = state.borrow();
        (s.wrappers.get(&node).and_then(|w| w.parent), parent(&s, node))
    };
    if old == new {
        return;
    }
    // Internal fields avoid adding/deleting API-object properties (V8 15's
    // extended-map normalization can abort in its range write barrier).
    // Establish the new edges before removing old ones: allocations here can
    // collect, including when wrapping an as-yet unobserved ancestor.
    if let Some(new) = new {
        let Some(owner) = wrap(scope, state, new) else { return };
        let children = owner.get_internal_field(scope, 2).and_then(|v| v.try_cast::<v8::Set>().ok()).unwrap_or_else(|| {
            let set = v8::Set::new(scope);
            owner.set_internal_field(2, set.into());
            set
        });
        children.add(scope, object.into());
        object.set_internal_field(1, owner.into());
        state.borrow_mut().wrappers.get_mut(&new).unwrap().children.insert(node);
    } else {
        let empty = v8::null(scope);
        object.set_internal_field(1, empty.into());
    }
    if let Some(old) = old {
        if let Some(owner) = get(scope, state, old)
            && let Some(children) = owner.get_internal_field(scope, 2).and_then(|v| v.try_cast::<v8::Set>().ok())
        {
            children.delete(scope, object.into());
        }
        if let Some(w) = state.borrow_mut().wrappers.get_mut(&old) {
            w.children.remove(&node);
        }
    }
    if let Some(w) = state.borrow_mut().wrappers.get_mut(&node) {
        w.parent = new;
    }
}

/// Capture only edges this mutation can change. Ordinary getters, attributes,
/// drawing and animation do no graph work; appending one child never scans its
/// parent's other children. A fragment/replace-all visits the changed list.
pub(super) fn before_mutation(scope: &mut v8::PinScope, name: &str, args: &v8::FunctionCallbackArguments) -> Option<(Shared, Vec<NodeId>)> {
    if !matches!(name, "append" | "insertBefore" | "remove" | "setTextContent" | "setInnerHTML") {
        return None;
    }
    let state = scope.get_slot::<Shared>()?.clone();
    let index = if matches!(name, "append" | "insertBefore") { 1 } else { 0 };
    let node = node_of(scope, args.get(index))?;
    let nodes = {
        let s = state.borrow();
        if index == 1 && s.document.node_type(node).ok() == Some(NodeType::Fragment) {
            s.document.children(node).ok()?.filter(|n| s.wrappers.contains_key(n)).collect()
        } else if matches!(name, "setTextContent" | "setInnerHTML") {
            let target = if name == "setInnerHTML" { s.document.template_contents(node).ok().flatten().unwrap_or(node) } else { node };
            s.wrappers.get(&target).map(|w| w.children.iter().copied().collect()).unwrap_or_default()
        } else {
            vec![node]
        }
    };
    Some((state, nodes))
}

/// V8 callbacks only enqueue ids: they can run during an allocation while
/// State is borrowed. Resource destruction happens on the page worker, at its
/// next presentation/access, with no idle timer or whole-table GC scan.
pub(super) fn reap(s: &mut State) {
    let dead = std::mem::take(&mut *s.collected_wrappers.borrow_mut());
    for node in dead {
        // A new wrapper may have been made since the old one's callback.
        if !s.wrappers.get(&node).is_some_and(|w| w.handle.is_empty()) {
            continue;
        }
        if let Some(w) = s.wrappers.remove(&node)
            && let Some(p) = w.parent.and_then(|n| s.wrappers.get_mut(&n))
        {
            p.children.remove(&node);
        }
        s.images.remove(&node);
        s.image_urls.remove(&node);
        s.images_away.remove(&node);
        s.image_latest.remove(&node);
        s.canvases.remove(&node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Entered, Loaded, Realm};
    use std::sync::Arc;

    fn realm() -> Realm {
        let parsed = olo_web::html::parse(Arc::from("<body>"), Default::default(), None).unwrap();
        Realm::new(parsed.document, Config::default(), None, None).unwrap()
    }
    fn collect(r: &mut Realm) {
        let _entered = Entered::new(&r.isolate);
        r.isolate.clear_kept_objects();
        r.isolate.low_memory_notification();
        account_images(&mut r.isolate, &r.state);
    }
    fn check(r: &mut Realm, source: &str) {
        assert_eq!(r.eval(source).unwrap(), "true", "{source}");
    }

    #[test]
    fn bulk_dom_moves_do_not_normalize_api_object_maps_during_marking() {
        // This release workload aborted V8's range write barrier when
        // detachment deleted a private property on an API-object wrapper.
        for _ in 0..3 {
            let mut r = realm();
            r.run(include_str!("../tests/dom-churn.js"), "dom-churn.js").unwrap();
            collect(&mut r);
            assert!(r.state.borrow().wrappers.len() < 10);
            assert!(r.report().errors.is_empty());
        }
    }

    #[test]
    fn dom_edges_preserve_identity_and_unreachable_cycles_are_collected() {
        let mut r = realm();
        r.run(
            r#"
          globalThis.events = 0;
          (() => {
            const p = document.createElement('div'), c = document.createElement('span');
            c.token = 17; c.addEventListener('ping', () => events++);
            p.append(c); document.body.append(p);
            globalThis.wp = new WeakRef(p); globalThis.wc = new WeakRef(c);
          })();
        "#,
            "ownership.js",
        )
        .unwrap();
        collect(&mut r);
        check(&mut r, "document.body.firstChild === wp.deref() && wp.deref().firstChild === wc.deref() && wc.deref().token === 17");
        r.run("globalThis.child = wc.deref(); child.parentNode.remove(); child.dispatchEvent(new Event('ping'))", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "events === 1 && child.parentNode === wp.deref() && child === wc.deref()");
        r.run("child = null", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "!wp.deref() && !wc.deref()");
    }

    #[test]
    fn fragment_moves_and_replacing_children_remove_old_ownership_edges() {
        let mut r = realm();
        r.run(
            r#"
          globalThis.p = document.createElement('div');
          (() => {
            const f = document.createDocumentFragment(), c = document.createElement('i');
            f.append(c); p.append(f);
            globalThis.wf = new WeakRef(f); globalThis.wc = new WeakRef(c);
          })();
        "#,
            "ownership.js",
        )
        .unwrap();
        collect(&mut r);
        check(&mut r, "!wf.deref() && p.firstChild === wc.deref()");
        r.run("p.innerHTML = '<b>new</b>'", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "!wc.deref()");
        r.run("globalThis.wb = new WeakRef(p.firstChild); p.textContent = ''", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "!wb.deref()");
    }

    #[test]
    fn shadow_and_template_ownership_does_not_change_dom_parents() {
        let mut r = realm();
        r.run(
            r#"
          (() => {
            const h = document.createElement('div'), t = document.createElement('template');
            h.attachShadow({mode:'open'}).append(t); t.innerHTML = '<img>';
            globalThis.image = t.content.firstChild;
            globalThis.wh = new WeakRef(h); globalThis.wt = new WeakRef(t);
            globalThis.wcontent = new WeakRef(t.content);
          })();
        "#,
            "ownership.js",
        )
        .unwrap();
        collect(&mut r);
        check(&mut r, "wh.deref().shadowRoot.firstChild === wt.deref() && wt.deref().content === wcontent.deref() && image.parentNode === wcontent.deref() && wcontent.deref().parentNode === null");
        r.run("image = null", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "!wh.deref() && !wt.deref() && !wcontent.deref()");
    }

    #[test]
    fn an_in_flight_image_is_alive_through_its_load_event_then_reclaimed() {
        let mut r = realm().with_image_lookup(Some(Box::new(|_| None)));
        r.run(
            r#"
          globalThis.loaded = 0;
          (() => {
            const image = new Image();
            image.onload = () => { loaded = image.naturalWidth; };
            image.src = 'https://example.test/image.png';
            globalThis.wi = new WeakRef(image);
          })();
        "#,
            "ownership.js",
        )
        .unwrap();
        r.advance(10., std::time::Duration::from_secs(1));
        let requests = r.take_requests();
        assert_eq!(requests.len(), 1);
        collect(&mut r);
        check(&mut r, "!!wi.deref()");
        let image = olo_web::image::Image::from_rgba(2, 1, vec![255; 8], Default::default(), None).unwrap();
        let weak_pixels = Arc::downgrade(&image);
        r.deliver(requests[0].id, Ok(Loaded::Image(image)));
        check(&mut r, "loaded === 2");
        collect(&mut r);
        check(&mut r, "!wi.deref()");
        assert!(weak_pixels.upgrade().is_none());
        assert_eq!(r.state.borrow().image_accounted, 0);
    }

    #[test]
    fn a_host_started_image_request_also_pins_its_detached_element() {
        let mut r = realm().with_image_lookup(Some(Box::new(|_| None)));
        r.run("globalThis.image = new Image(); image.id = 'poster'; globalThis.wi = new WeakRef(image)", "ownership.js").unwrap();
        let node = {
            let s = r.state.borrow();
            *s.wrappers.keys().find(|&&n| s.document.attribute(n, "id").ok().flatten() == Some("poster")).unwrap()
        };
        r.request_image(node, "https://example.test/poster.png");
        r.run("image = null", "ownership.js").unwrap();
        collect(&mut r);
        check(&mut r, "!!wi.deref()");
        let request = r.take_requests().pop().unwrap();
        r.deliver(request.id, Err("test failure".into()));
        collect(&mut r);
        check(&mut r, "!wi.deref()");
    }

    #[test]
    fn shared_images_are_charged_once_and_released_after_the_last_binding() {
        let r = realm();
        let ids = {
            let mut s = r.state.borrow_mut();
            [s.document.create_element("img", olo_web::dom::Namespace::Html).unwrap(), s.document.create_element("img", olo_web::dom::Namespace::Html).unwrap()]
        };
        let image = olo_web::image::Image::from_rgba(2, 1, vec![255; 8], Default::default(), None).unwrap();
        let mut images = Images::default();
        images.insert(ids[0], image.clone());
        images.insert(ids[1], image);
        assert_eq!(images.bytes, 8);
        images.remove(&ids[0]);
        assert_eq!(images.bytes, 8);
        images.remove(&ids[1]);
        assert_eq!(images.bytes, 0);
    }
}
