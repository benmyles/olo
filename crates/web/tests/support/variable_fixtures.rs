//! Literal native variable ports. Canonical references are decoded without
//! the engine parser; provided custom-property winners are not a cascade.
use olo_web::css::{
    self, Kind, Syntax,
    variables::{Binding, Computed, Context, Error, Limits, Resolver, Value as Tokens},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc};
struct Host {
    entries: BTreeMap<String, (u64, Tokens)>,
}
impl Context for Host {
    fn lookup(&self, name: &str) -> Option<Binding<'_>> {
        self.entries.get(name).map(|(id, value)| Binding { id: *id, value, animation_tainted: false })
    }
}
fn snapshot(source: &str, consumer: Option<&str>) -> Result<(Host, Option<Tokens>), String> {
    let syntax = Syntax::parse(Arc::from(source), css::Limits::default(), None).map_err(|e| e.to_string())?;
    let declarations = syntax.declarations(None).map_err(|e| e.to_string())?;
    let mut winners: BTreeMap<String, (Tokens, bool)> = BTreeMap::new();
    let mut selected: Option<(Tokens, bool)> = None;
    for d in declarations {
        let Some(name) = d.name() else {
            continue;
        };
        let custom = name.starts_with("--") && name.len() > 2;
        if !custom && !consumer.is_some_and(|p| name.eq_ignore_ascii_case(p)) {
            continue;
        }
        let value = match Tokens::compile(d.value(), Limits::default(), None) {
            Ok(v) => v,
            Err(Error::Invalid) => continue,
            Err(e) => return Err(e.to_string()),
        };
        if custom {
            if winners.get(name).is_some_and(|(_, important)| *important && !d.important()) {
                continue;
            }
            winners.insert(name.to_owned(), (value, d.important()));
        } else if selected.as_ref().is_none_or(|(_, important)| !*important || d.important()) {
            selected = Some((value, d.important()));
        }
    }
    let entries = winners.into_iter().enumerate().map(|(id, (name, (value, _)))| (name, (id as u64, value))).collect();
    Ok((Host { entries }, selected.map(|(v, _)| v)))
}
fn canonical(value: &Computed, expected: &str) -> bool {
    let Some(components) = value.components() else {
        return false;
    };
    let mut values = components.filter(|c| c.kind() != Kind::Whitespace);
    let Some(c) = values.next() else {
        return expected.is_empty();
    };
    if values.next().is_some() {
        return false;
    }
    if c.kind() == Kind::Ident {
        return c.text() == Some(expected);
    }
    let bytes = expected.as_bytes();
    let mut end = 0;
    if bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')) {
        end += 1;
    }
    while bytes.get(end).is_some_and(|b| b.is_ascii_digit() || *b == b'.') {
        end += 1;
    }
    let Ok(number) = expected[..end].parse::<f64>() else {
        return false;
    };
    let Some(actual) = c.number() else {
        return false;
    };
    actual.value == number && if expected[end..].is_empty() { matches!(c.kind(), Kind::Number | Kind::Integer) } else { c.unit() == Some(&expected[end..]) }
}
fn error_outcome(error: &str) -> &'static str {
    if error.contains("Unsupported(") || error.ends_with("RequiresCascade") { "unsupported" } else { "error" }
}
fn finish(mut row: Value, result: Result<bool, String>) -> Value {
    match result {
        Ok(true) => row["outcome"] = json!("passed"),
        Ok(false) => row["outcome"] = json!("mismatch"),
        Err(e) => {
            row["outcome"] = json!(error_outcome(&e));
            row["reason"] = json!(e);
        }
    }
    row
}
pub fn evaluate(dir: &Path) -> Value {
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (file, hash) in manifest["files"].as_object().unwrap() {
        assert_eq!(hash.as_str().unwrap(), format!("{:x}", Sha256::digest(std::fs::read(dir.join(file)).unwrap())), "variable fixture changed: {file}");
    }
    let data: Value = serde_json::from_slice(&std::fs::read(dir.join("derived.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let result = std::panic::catch_unwind(|| -> Result<bool, String> {
            if case["mode"] == "parse" {
                let syntax = Syntax::parse(Arc::from(case["input"].as_str().unwrap()), css::Limits::default(), None).map_err(|e| e.to_string())?;
                return match Tokens::compile(syntax.components(), Limits::default(), None) {
                    Ok(_) => Ok(case["valid"] == true),
                    Err(Error::Invalid) => Ok(case["valid"] == false),
                    Err(e) => Err(e.to_string()),
                };
            }
            if case["requires"].as_array().unwrap().iter().any(|s| s == "registered_properties") {
                return Err("Unsupported(registered-property host)".into());
            }
            let source = format!("{};{}", case["base"].as_str().unwrap(), case["declarations"].as_str().unwrap());
            let (host, consumer) = snapshot(&source, case["property"].as_str())?;
            let consumer = consumer.ok_or("missing provided consumer declaration")?;
            let mut resolver = Resolver::new(&host, Limits::default(), None);
            let actual = resolver.resolve(&consumer, true).map_err(|e| e.to_string())?;
            if case["invalid"] == true { Ok(actual.is_invalid()) } else { Ok(canonical(&actual, case["expected"].as_str().unwrap())) }
        });
        results.push(match result {
            Ok(result) => finish(case.clone(), result),
            Err(_) => {
                let mut row = case.clone();
                row["outcome"] = json!("panic");
                row
            }
        });
    }
    for group in data["cycles"].as_array().unwrap() {
        let result = std::panic::catch_unwind(|| -> Result<Vec<Value>, String> {
            let source = group["declarations"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect::<Vec<_>>().join(";");
            let (host, _) = snapshot(&source, None)?;
            let mut resolver = Resolver::new(&host, Limits::default(), None);
            let mut rows = Vec::new();
            for query in group["queries"].as_array().unwrap() {
                let result = resolver
                    .property(query["property"].as_str().unwrap())
                    .map_err(|e| e.to_string())
                    .map(|actual| if query["invalid"] == true { actual.is_invalid() } else { actual.components().is_some_and(|mut c| c.next().is_some()) });
                let row = json!({"file":group["file"],"line":group["line"],"description":group["description"],"property":query["property"],"expected_invalid":query["invalid"],"mode":"cycle"});
                rows.push(finish(row, result));
            }
            Ok(rows)
        });
        match result {
            Ok(Ok(rows)) => results.extend(rows),
            Ok(Err(e)) => {
                for query in group["queries"].as_array().unwrap() {
                    results.push(finish(json!({"file":group["file"],"line":group["line"],"property":query["property"],"mode":"cycle"}), Err(e.clone())));
                }
            }
            Err(_) => {
                for query in group["queries"].as_array().unwrap() {
                    results.push(json!({"file":group["file"],"line":group["line"],"property":query["property"],"mode":"cycle","outcome":"panic"}));
                }
            }
        }
    }
    for item in data["unported"].as_array().unwrap() {
        let mut row = item.clone();
        row["outcome"] = json!("unsupported");
        results.push(row);
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for row in &results {
        *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
    }
    json!({"scope":data["scope"],"counts":counts,"total":results.len(),"results":results})
}
