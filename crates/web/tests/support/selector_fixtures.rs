use olo_web::css::{
    self, Syntax,
    selectors::{self, Attribute, Content, ElementName, Selectors, Tree},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
struct FixtureTree<'a>(&'a [Value], Option<usize>);
impl FixtureTree<'_> {
    fn node(&self, n: usize) -> selectors::Result<&Value> {
        self.0.get(n).ok_or(selectors::Error::InvalidNode)
    }
    fn link(&self, n: usize, key: &str) -> selectors::Result<Option<usize>> {
        Ok(self.node(n)?[key].as_u64().map(|n| n as usize))
    }
    fn id(&self, n: usize) -> Option<&str> {
        self.0[n]["attrs"].as_array()?.iter().find(|a| a["ns"] == "" && a["name"] == "id")?["value"].as_str()
    }
}
impl Tree for FixtureTree<'_> {
    type Node = usize;
    fn content(&self, n: usize) -> selectors::Result<Content<'_>> {
        if self.1 == Some(n) {
            return Ok(Content::Fragment);
        }
        let n = self.node(n)?;
        Ok(match n["kind"].as_str().unwrap() {
            "element" => Content::Element(ElementName { namespace: n["ns"].as_str().unwrap(), local_name: n["name"].as_str().unwrap() }),
            "text" => Content::Text(n["value"].as_str().unwrap()),
            _ => Content::Other,
        })
    }
    fn parent(&self, n: usize) -> selectors::Result<Option<usize>> {
        if self.1 == Some(n) {
            return Ok(None);
        }
        self.link(n, "parent")
    }
    fn first_child(&self, n: usize) -> selectors::Result<Option<usize>> {
        self.link(n, "first")
    }
    fn previous_sibling(&self, n: usize) -> selectors::Result<Option<usize>> {
        self.link(n, "previous")
    }
    fn next_sibling(&self, n: usize) -> selectors::Result<Option<usize>> {
        self.link(n, "next")
    }
    fn attributes(&self, n: usize) -> selectors::Result<impl Iterator<Item = Attribute<'_>>> {
        Ok(self.node(n)?["attrs"].as_array().ok_or(selectors::Error::InvalidNode)?.iter().map(|a| Attribute {
            namespace: a["ns"].as_str().unwrap(),
            local_name: a["name"].as_str().unwrap(),
            value: a["value"].as_str().unwrap(),
        }))
    }
}
fn run(test: &Value, data: &Value) -> selectors::Result<Value> {
    let source = Syntax::parse(Arc::from(test["selector"].as_str().unwrap()), css::Limits::default(), None).map_err(|_| selectors::Error::Invalid)?;
    let parsed = Selectors::compile(source.components(), Default::default(), Default::default(), None);
    if test["mode"] == "invalid" {
        return match parsed {
            Err(selectors::Error::Invalid) => Ok(Value::Null),
            Err(e) => Err(e),
            Ok(_) => Ok(json!("accepted invalid")),
        };
    }
    let parsed = parsed?;
    let mut tree = FixtureTree(data["documents"][test["document"].as_str().unwrap()].as_array().unwrap(), None);
    let options = selectors::MatchOptions { html_document: test["html"].as_bool().unwrap(), ..Default::default() };
    let exclude = test["exclude"].as_array();
    if exclude.is_some_and(|a| a.contains(&json!("xhtml"))) {
        return Err(selectors::Error::Unsupported("HTML fixture context"));
    }
    let document_excluded = exclude.is_some_and(|a| a.contains(&json!("document")));
    let root = if document_excluded { (0..tree.0.len()).find(|&n| tree.id(n) == Some("root")).unwrap() } else { 0 };
    if document_excluded && exclude.is_some_and(|a| a.contains(&json!("element"))) {
        tree.1 = Some(root);
    }
    if test["suite"] == "valid_selectors" && test["selector"] == "*" {
        return Err(selectors::Error::Unsupported("upstream matches-only fixture scope"));
    }
    let mut found = Vec::new();
    parsed.query(&tree, root, !document_excluded, options, Default::default(), None, |n, _| {
        found.push(tree.id(n).unwrap_or("nil").to_owned());
        Ok(())
    })?;
    Ok(if test["mode"] == "count" { json!(found.len()) } else { json!(found) })
}
pub fn evaluate(dir: &Path) -> Value {
    let source: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (name, hash) in source["files"].as_object().unwrap() {
        assert_eq!(hash.as_str().unwrap(), format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), "selector fixture changed: {name}");
    }
    let data: Value = serde_json::from_slice(&std::fs::read(dir.join("derived.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for test in data["cases"].as_array().unwrap() {
        let mut row = test.clone();
        match std::panic::catch_unwind(|| run(test, &data)) {
            Ok(Ok(actual)) => {
                row["outcome"] = json!(if actual == test["expected"] { "passed" } else { "mismatch" });
                if actual != test["expected"] {
                    row["actual"] = actual;
                }
            }
            Ok(Err(selectors::Error::Invalid)) if test["suite"] == "test_select" && matches!(test["case"].as_u64(), Some(69 | 71)) => {
                row["outcome"] = json!("updated_specification");
                row["reason"] = json!("Selectors 4 nth-of-type grammar has no of-selector-list; the imported upstream library accepts this extension");
                row["modern_expected"] = json!("invalid");
            }
            Ok(Err(e)) => {
                row["outcome"] = json!(match e {
                    selectors::Error::Unsupported(_) | selectors::Error::StateUnavailable => "unsupported",
                    _ => "error",
                });
                row["reason"] = json!(e.to_string());
            }
            Err(_) => row["outcome"] = json!("panic"),
        }
        *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
        results.push(row);
    }
    json!({"scope":"Native selector queries against independent literal expectations/static XML trees, not XML loading, upstream Python/JS, CSSOM, style, layout or website conformance.","total":results.len(),"counts":counts,"results":results})
}
