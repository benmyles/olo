//! Document-scoped link resolution shared by native and agent input.
use olo_web::{
    dom::{Document, Namespace, NodeId},
    page,
};
use std::{collections::HashMap, sync::Arc};
use url::Url;
#[derive(Clone, Debug)]
pub struct Link {
    pub node: NodeId,
    pub url: String,
    pub blocked: Option<&'static str>,
}
pub fn resolve(doc: &Document, node: NodeId, base: &Url, origin: &Url, cache: &mut HashMap<NodeId, Option<Arc<Link>>>) -> page::Result<Option<Arc<Link>>> {
    let mut path = Vec::new();
    let mut at = Some(node);
    let found = loop {
        let Some(node) = at else {
            break None;
        };
        if let Some(link) = cache.get(&node) {
            break link.clone();
        }
        path.push(node);
        if doc.namespace(node)? == Some(Namespace::Html)
            && matches!(doc.name(node)?, Some("a" | "area"))
            && let Some(href) = doc.attribute(node, "href")?
        {
            let Ok(mut url) = base.join(href) else {
                break None;
            };
            let blocked = if !url.username().is_empty() || url.password().is_some() {
                Some("Embedded URL credentials are not supported")
            } else if !matches!(url.scheme(), "http" | "https" | "file") {
                Some("This link's URL scheme is not supported yet")
            } else if url.scheme() == "file" && origin.scheme() != "file" {
                Some("Web pages cannot navigate to local files")
            } else if doc.attribute(node, "download")?.is_some() {
                Some("Downloads are not supported yet")
            } else if doc.attribute(node, "target")?.is_some_and(|t| !t.is_empty() && !["_self", "_top", "_parent"].iter().any(|v| t.eq_ignore_ascii_case(v))) {
                Some("Links opening another window are not supported yet")
            } else {
                None
            };
            // Structured snapshots never expose embedded credential strings.
            if !url.username().is_empty() || url.password().is_some() {
                let _ = url.set_username("");
                let _ = url.set_password(None);
            }
            break Some(Arc::new(Link { node, url: url.to_string(), blocked }));
        }
        at = doc.parent(node)?;
    };
    for node in path {
        cache.insert(node, found.clone());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_web::{dom::NodeType, html};
    fn find(doc: &Document, id: &str) -> NodeId {
        let mut nodes = vec![doc.root()];
        while let Some(node) = nodes.pop() {
            if doc.node_type(node).unwrap() == NodeType::Element && doc.attribute(node, "id").unwrap() == Some(id) {
                return node;
            }
            nodes.extend(doc.children(node).unwrap());
        }
        panic!("Missing {id}")
    }
    #[test]
    fn native_link_resolution_descendants_schemes_and_capabilities() {
        let doc=html::parse(Arc::from("<!doctype html><a href='../next?q=1#part'><span id=child>follow</span></a><a id=http href='http://example.test/'>downgrade navigation</a><a id=file href='file:///etc/passwd'>file</a><a id=js href='javascript:alert(1)'>script</a><a id=credentials href='https://name:secret@example.test/'>secret</a><a id=download download href=x>download</a><a id=window target=_blank href=x>window</a><div id=none>plain</div>"),Default::default(),None).unwrap().document;
        let origin = Url::parse("https://example.test/dir/page").unwrap();
        let base = origin.join("/assets/deep/").unwrap();
        let mut cache = HashMap::new();
        let child = find(&doc, "child");
        let link = resolve(&doc, child, &base, &origin, &mut cache).unwrap().unwrap();
        assert_eq!(link.url, "https://example.test/assets/next?q=1#part");
        assert!(link.blocked.is_none());
        assert!(Arc::ptr_eq(&link, &resolve(&doc, child, &base, &origin, &mut cache).unwrap().unwrap()));
        assert!(resolve(&doc, find(&doc, "http"), &base, &origin, &mut cache).unwrap().unwrap().blocked.is_none());
        for id in ["file", "js", "credentials", "download", "window"] {
            let link = resolve(&doc, find(&doc, id), &base, &origin, &mut cache).unwrap().unwrap();
            assert!(link.blocked.is_some(), "{id}");
            assert!(!link.url.contains("secret"));
        }
        assert!(resolve(&doc, find(&doc, "none"), &base, &origin, &mut cache).unwrap().is_none());
    }
}
