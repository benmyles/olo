use std::sync::Arc;

use olo_web::dom::{Document, Error, Limits, Namespace, NodeId, NodeType};

fn document() -> Document {
    Document::new(Arc::from(""), Limits::default()).unwrap()
}
fn element(doc: &mut Document, name: &str) -> NodeId {
    doc.create_element(name, Namespace::Html).unwrap()
}
fn children(doc: &Document, id: NodeId) -> Vec<NodeId> {
    doc.children(id).unwrap().collect()
}

#[test]
fn scoped_generation_handles_reject_old_navigation_and_reused_slots() {
    let mut doc = document();
    let mut other = document();
    let old = element(&mut doc, "div");
    let foreign = element(&mut other, "div");
    assert_eq!(doc.node_type(foreign), Err(Error::WrongDocument));
    assert_eq!(doc.append_child(doc.root(), foreign), Err(Error::WrongDocument));
    assert_eq!(doc.destroy_subtree(old).unwrap(), 1);
    let new = element(&mut doc, "span");
    assert_eq!(old.slot(), new.slot());
    assert_ne!(old.generation(), new.generation());
    assert_eq!(doc.node_type(old), Err(Error::StaleNode));
    assert_eq!(doc.node_type(new), Ok(NodeType::Element));
    assert_ne!(old.document(), foreign.document());
}

#[test]
fn moves_fragments_and_rejected_mutations_preserve_order_and_ownership() {
    let mut doc = document();
    let root = element(&mut doc, "main");
    doc.append_child(doc.root(), root).unwrap();
    let a = element(&mut doc, "a");
    let b = element(&mut doc, "b");
    let c = element(&mut doc, "c");
    for n in [a, b, c] {
        doc.append_child(root, n).unwrap();
    }
    doc.insert_before(root, c, Some(b)).unwrap();
    assert_eq!(children(&doc, root), [a, c, b]);
    assert_eq!(doc.first_child(root), Ok(Some(a)));
    assert_eq!(doc.last_child(root), Ok(Some(b)));
    assert_eq!(doc.next_sibling(a), Ok(Some(c)));
    assert_eq!(doc.previous_sibling(b), Ok(Some(c)));
    doc.insert_before(root, b, Some(a)).unwrap();
    assert_eq!(children(&doc, root), [b, a, c]);
    doc.append_child(root, b).unwrap();
    assert_eq!(children(&doc, root), [a, c, b]);
    let revision = doc.revision();
    assert!(!doc.insert_before(root, c, Some(c)).unwrap());
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.append_child(a, root), Err(Error::Cycle));
    assert_eq!(children(&doc, root), [a, c, b]);

    let fragment = doc.create_fragment().unwrap();
    let x = element(&mut doc, "x");
    let y = element(&mut doc, "y");
    doc.append_child(fragment, x).unwrap();
    doc.append_child(fragment, y).unwrap();
    doc.insert_before(root, fragment, Some(c)).unwrap();
    assert_eq!(children(&doc, root), [a, x, y, c, b]);
    assert!(children(&doc, fragment).is_empty());
    assert_eq!(doc.parent(x), Ok(Some(root)));
    let invalid = doc.create_fragment().unwrap();
    let first = element(&mut doc, "first");
    let second = element(&mut doc, "second");
    doc.append_child(invalid, first).unwrap();
    doc.append_child(invalid, second).unwrap();
    assert_eq!(doc.append_child(doc.root(), invalid), Err(Error::Hierarchy));
    assert_eq!(children(&doc, invalid), [first, second]);
    assert_eq!(children(&doc, doc.root()), [root]);
    let text = doc.create_text("text").unwrap();
    assert_eq!(doc.append_child(doc.root(), text), Err(Error::Hierarchy));
    assert_eq!(doc.append_child(text, a), Err(Error::Hierarchy));
    assert_eq!(doc.insert_before(root, a, Some(first)), Err(Error::Hierarchy));
}

#[test]
fn source_ranges_and_owned_strings_are_retained_and_reclaimed_separately() {
    let source: Arc<str> = Arc::from("<p>Καλημέρα🙂</p>");
    let mut doc = Document::new(source.clone(), Limits::default()).unwrap();
    let start = source.find('>').unwrap() + 1;
    let end = source.find("</").unwrap();
    let text = doc.create_text(&source[start..end]).unwrap();
    assert_eq!(doc.value(text), Ok(Some("Καλημέρα🙂")));
    assert_eq!(doc.memory().owned_text_bytes, 0);
    assert_eq!(doc.value(text).unwrap().unwrap().as_ptr(), source[start..].as_ptr());
    let revision = doc.revision();
    assert!(!doc.set_value(text, "Καλημέρα🙂").unwrap());
    assert_eq!(doc.revision(), revision);
    doc.set_value(text, "changed").unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 7);
    doc.set_value(text, "other").unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 5);
    doc.set_value(text, &source[start..end]).unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 0);
    doc.destroy_subtree(text).unwrap();
    assert_eq!(doc.memory().live_nodes, 1);
    assert!(doc.memory().node_size <= 48, "node size {}", doc.memory().node_size);
}

#[test]
#[cfg(not(feature = "baseline-storage"))]
fn decoded_owned_text_moves_into_the_document_without_a_second_copy() {
    let mut doc = document();
    let text = "decoded & normalized text".to_owned();
    assert_eq!(text.len(), text.capacity());
    let original = text.as_ptr();
    let node = doc.create_text(text).unwrap();
    assert_eq!(doc.value(node).unwrap().unwrap().as_ptr(), original);
    let element = element(&mut doc, "p");
    let attribute = "decoded attribute".to_owned();
    let original = attribute.as_ptr();
    doc.set_attribute(element, "title", attribute).unwrap();
    assert_eq!(doc.attribute(element, "title").unwrap().unwrap().as_ptr(), original);
}

#[test]
fn attributes_preserve_insertion_order_and_namespaces_and_skip_noops() {
    let mut doc = document();
    let html = element(&mut doc, "DIV");
    let svg = doc.create_element("foreignObject", Namespace::Svg).unwrap();
    assert_eq!(doc.name(html), Ok(Some("div")));
    assert_eq!(doc.name(svg), Ok(Some("foreignObject")));
    doc.set_attribute(html, "CLASS", "hello").unwrap();
    doc.set_attribute(html, "id", "greeting").unwrap();
    let revision = doc.revision();
    assert!(!doc.set_attribute(html, "class", "hello").unwrap());
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.attributes(html).unwrap().collect::<Vec<_>>(), [("class", "hello"), ("id", "greeting")]);
    assert_eq!(doc.attribute(html, "Class"), Ok(Some("hello")));
    doc.set_attribute(html, "class", "goodbye").unwrap();
    assert_eq!(doc.attributes(html).unwrap().collect::<Vec<_>>(), [("class", "goodbye"), ("id", "greeting")]);
    doc.remove_attribute(html, "CLASS").unwrap();
    doc.set_attribute(html, "class", "again").unwrap();
    assert_eq!(doc.attributes(html).unwrap().collect::<Vec<_>>(), [("id", "greeting"), ("class", "again")]);
    doc.set_attribute(svg, "viewBox", "0 0 10 10").unwrap();
    assert_eq!(doc.attribute(svg, "viewbox"), Ok(None));
    assert_eq!(doc.attribute(svg, "viewBox"), Ok(Some("0 0 10 10")));
    doc.destroy_subtree(html).unwrap();
    doc.destroy_subtree(svg).unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 0);
    assert_eq!(doc.memory().live_attributes, 0);
}

#[test]
fn limits_preflight_without_losing_the_previous_value_or_children() {
    let mut doc = Document::new(Arc::from(""), Limits { nodes: 4, attributes: 1, owned_text_bytes: 4, name_bytes: 20, depth: 3, ..Limits::default() }).unwrap();
    let a = element(&mut doc, "a");
    let b = element(&mut doc, "b");
    doc.append_child(doc.root(), a).unwrap();
    doc.append_child(a, b).unwrap();
    let text = doc.create_text("1234").unwrap();
    assert_eq!(doc.append_child(b, text), Err(Error::DepthLimit));
    assert_eq!(doc.parent(text), Ok(None));
    assert_eq!(doc.create_text(""), Err(Error::NodeLimit));
    assert!(doc.set_value(text, "abcd").unwrap(), "replacement credits the released bytes");
    assert_eq!(doc.set_value(text, "12345"), Err(Error::TextLimit));
    assert_eq!(doc.value(text), Ok(Some("abcd")));
    assert_eq!(doc.set_attribute(a, "id", "x"), Err(Error::TextLimit));
    assert_eq!(doc.attribute(a, "id"), Ok(None));
    doc.set_value(text, "").unwrap();
    doc.set_attribute(a, "id", "1234").unwrap();
    assert_eq!(doc.set_attribute(a, "class", ""), Err(Error::AttributeLimit));
    assert_eq!(doc.attribute(a, "id"), Ok(Some("1234")));
    doc.remove_attribute(a, "id").unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 0);
}

#[test]
fn destroying_deep_and_wide_subtrees_is_iterative_and_reuses_storage() {
    let mut doc = Document::new(Arc::from(""), Limits { depth: 4096, ..Limits::default() }).unwrap();
    let root = element(&mut doc, "root");
    let mut parent = root;
    for _ in 0..2048 {
        let child = element(&mut doc, "node");
        doc.append_child(parent, child).unwrap();
        parent = child;
    }
    let leaf = doc.create_text("a live leaf").unwrap();
    doc.append_child(parent, leaf).unwrap();
    assert_eq!(doc.destroy_subtree(root).unwrap(), 2050);
    assert_eq!(doc.node_type(leaf), Err(Error::StaleNode));
    let reserved = doc.memory().node_reserved_bytes;
    for _ in 0..5 {
        let root = element(&mut doc, "root");
        for _ in 0..2048 {
            let child = element(&mut doc, "node");
            doc.append_child(root, child).unwrap();
            doc.set_attribute(child, "id", "one").unwrap();
        }
        assert_eq!(doc.destroy_subtree(root).unwrap(), 2049);
        assert_eq!(doc.memory().owned_text_bytes, 0);
        assert_eq!(doc.memory().live_attributes, 0);
    }
    assert_eq!(doc.memory().node_reserved_bytes, reserved);
}

#[test]
fn random_reparenting_matches_an_independent_parent_list_model() {
    let mut doc = document();
    let root = element(&mut doc, "root");
    let mut ids = vec![root];
    for _ in 1..64 {
        ids.push(element(&mut doc, "node"));
    }
    let mut parents = vec![None; ids.len()];
    let mut model = vec![Vec::<usize>::new(); ids.len()];
    let mut seed = 0xa17b_092d_45ec_3113u64;
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let child = (seed as usize >> 8) % ids.len();
        let parent = (seed as usize >> 20) % ids.len();
        let before = model[parent].get((seed as usize >> 32) % (model[parent].len() + 1)).copied();
        let mut ancestor = Some(parent);
        let mut cycle = false;
        while let Some(at) = ancestor {
            if at == child {
                cycle = true;
                break;
            }
            ancestor = parents[at];
        }
        let result = doc.insert_before(ids[parent], ids[child], before.map(|at| ids[at]));
        if cycle {
            assert_eq!(result, Err(Error::Cycle));
        } else {
            result.unwrap();
            if before != Some(child) {
                if let Some(old) = parents[child] {
                    model[old].retain(|&id| id != child);
                }
                let at = before.map(|id| model[parent].iter().position(|&i| i == id).unwrap()).unwrap_or(model[parent].len());
                model[parent].insert(at, child);
                parents[child] = Some(parent);
            }
        }
        for at in [parent, child] {
            assert_eq!(children(&doc, ids[at]), model[at].iter().map(|&i| ids[i]).collect::<Vec<_>>());
            assert_eq!(doc.parent(ids[at]), Ok(parents[at].map(|i| ids[i])));
        }
    }
}

#[test]
fn document_type_is_unique_before_the_element_and_never_an_element_child() {
    let mut doc = document();
    let html = element(&mut doc, "html");
    let doctype = doc.create_doctype("html", "PUBLIC", "SYSTEM").unwrap();
    doc.append_child(doc.root(), html).unwrap();
    assert_eq!(doc.append_child(doc.root(), doctype), Err(Error::Hierarchy));
    doc.insert_before(doc.root(), doctype, Some(html)).unwrap();
    assert_eq!(children(&doc, doc.root()), [doctype, html]);
    assert_eq!(doc.node_type(doctype), Ok(NodeType::Doctype));
    let d = doc.doctype(doctype).unwrap().unwrap();
    assert_eq!((d.name, d.public_id, d.system_id), ("html", "PUBLIC", "SYSTEM"));
    let second = doc.create_doctype("html", "", "").unwrap();
    assert_eq!(doc.insert_before(doc.root(), second, Some(doctype)), Err(Error::Hierarchy));
    assert_eq!(doc.append_child(html, second), Err(Error::Hierarchy));
    assert_eq!(doc.insert_before(doc.root(), html, Some(doctype)), Err(Error::Hierarchy));
    assert_eq!(children(&doc, doc.root()), [doctype, html]);
    doc.destroy_subtree(doctype).unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 0);
    assert_eq!(doc.parent(second), Ok(None));
    doc.insert_before(doc.root(), second, Some(html)).unwrap();
}

#[test]
fn failed_doctype_allocation_reclaims_its_partially_stored_identifiers() {
    let mut doc = Document::new(Arc::from(""), Limits { owned_text_bytes: 5, ..Limits::default() }).unwrap();
    assert_eq!(doc.create_doctype("html", "1234", "5678"), Err(Error::TextLimit));
    assert_eq!(doc.memory().owned_text_bytes, 0);
    assert_eq!(doc.memory().live_nodes, 1);
    let d = doc.create_doctype("html", "1234", "").unwrap();
    doc.destroy_subtree(d).unwrap();
    assert_eq!(doc.memory().owned_text_bytes, 0);
}

#[test]
fn debug_output_does_not_dump_page_source_or_character_data() {
    let mut doc = Document::new(Arc::from("private-source-token"), Limits::default()).unwrap();
    doc.create_text("private-field-value").unwrap();
    let debug = format!("{doc:?}");
    assert!(!debug.contains("private-source-token") && !debug.contains("private-field-value"));
}
