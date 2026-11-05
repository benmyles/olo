//! Pinned independent WPT .dat input and expected trees. Never derive an
//! expected tree from the implementation under test.
use olo_web::{
    dom::{Document, Namespace, NodeId, NodeType},
    html::{ParseError, ParseOptions, parse},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

#[derive(Default, Clone, Debug)]
pub struct Case {
    pub file: String,
    pub number: usize,
    pub input: String,
    pub expected: String,
    pub fragment: Option<String>,
    pub scripting: Option<bool>,
}

pub fn load(dir: &Path) -> Vec<Case> {
    let pinned: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "dat")).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let data = std::fs::read_to_string(&path).unwrap();
        let file = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(pinned["files"][file], format!("{:x}", Sha256::digest(data.as_bytes())), "upstream fixture changed: {file}");
        let mut current = None::<Case>;
        let mut section = "";
        let mut number = 0;
        for line in data.split_inclusive('\n') {
            match line.trim_end_matches('\n') {
                "#data" => {
                    if let Some(mut c) = current.take() {
                        c.expected = c.expected.trim_end_matches('\n').into();
                        out.push(c);
                    }
                    number += 1;
                    current = Some(Case { file: file.into(), number, ..Case::default() });
                    section = "data";
                }
                "#errors" => {
                    if let Some(c) = &mut current
                        && c.input.ends_with('\n')
                    {
                        c.input.pop();
                    }
                    section = "errors";
                }
                "#new-errors" => section = "errors",
                "#document" => section = "document",
                "#document-fragment" => section = "fragment",
                "#script-on" => {
                    current.as_mut().unwrap().scripting = Some(true);
                    section = "other";
                }
                "#script-off" => {
                    current.as_mut().unwrap().scripting = Some(false);
                    section = "other";
                }
                "#description" => {
                    section = "description";
                }
                _ => {
                    if let Some(c) = &mut current {
                        match section {
                            "data" => c.input.push_str(line),
                            "document" => c.expected.push_str(line),
                            "fragment" => c.fragment = Some(line.trim_end_matches('\n').into()),
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Some(mut c) = current {
            c.expected = c.expected.trim_end_matches('\n').into();
            out.push(c);
        }
    }
    out
}

pub fn dump(document: &Document, root: NodeId) -> String {
    let mut out = String::new();
    let mut stack: Vec<_> = document.children(root).unwrap().map(|n| (n, 0)).collect();
    stack.reverse();
    while let Some((node, depth)) = stack.pop() {
        let prefix = format!("| {}", "  ".repeat(depth));
        match document.node_type(node).unwrap() {
            NodeType::Element => {
                let ns = match document.namespace(node).unwrap().unwrap() {
                    Namespace::Html => "",
                    Namespace::Svg => "svg ",
                    Namespace::MathMl => "math ",
                };
                out.push_str(&format!("{prefix}<{ns}{}>\n", document.name(node).unwrap().unwrap()));
                let mut attrs: Vec<_> = document.attributes(node).unwrap().collect();
                attrs.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
                for (name, value) in attrs {
                    out.push_str(&format!("{prefix}  {name}=\"{value}\"\n"));
                }
            }
            NodeType::Text => out.push_str(&format!("{prefix}\"{}\"\n", document.value(node).unwrap().unwrap())),
            NodeType::Comment => out.push_str(&format!("{prefix}<!-- {} -->\n", document.value(node).unwrap().unwrap())),
            NodeType::ProcessingInstruction => out.push_str(&format!("{prefix}<?{} {}?>\n", document.name(node).unwrap().unwrap(), document.value(node).unwrap().unwrap())),
            NodeType::Doctype => {
                let d = document.doctype(node).unwrap().unwrap();
                out.push_str(&format!("{prefix}<!DOCTYPE {}", d.name));
                if !d.public_id.is_empty() || !d.system_id.is_empty() {
                    out.push_str(&format!(" \"{}\" \"{}\"", d.public_id, d.system_id));
                }
                out.push_str(">\n");
            }
            NodeType::Document | NodeType::Fragment => panic!("nested tree root"),
        }
        let mut children: Vec<_> = document.children(node).unwrap().map(|n| (n, depth + 1)).collect();
        children.reverse();
        stack.extend(children);
    }
    out.trim_end_matches('\n').into()
}

pub fn run(c: &Case, scripting: bool) -> Value {
    let mut result = json!({"file":c.file,"case":c.number,"scripting":scripting});
    if c.fragment.is_some() {
        result["outcome"] = json!("unsupported");
        result["reason"] = json!("fragment parsing");
        return result;
    }
    if c.file.starts_with("scripted_") && scripting {
        result["outcome"] = json!("unsupported");
        result["reason"] = json!("JavaScript execution");
        return result;
    }
    match std::panic::catch_unwind(|| parse(Arc::from(c.input.as_str()), ParseOptions { scripting, ..ParseOptions::default() }, None)) {
        Ok(Ok(parsed)) => {
            let actual = dump(&parsed.document, parsed.document.root());
            if actual == c.expected {
                result["outcome"] = json!("passed");
            } else {
                result["outcome"] = json!("mismatch");
                result["input"] = json!(c.input);
                result["expected"] = json!(c.expected);
                result["actual"] = json!(actual);
            }
        }
        Ok(Err(ParseError::Unsupported(feature))) => {
            result["outcome"] = json!("unsupported");
            result["reason"] = json!(feature);
        }
        Ok(Err(e)) => {
            result["outcome"] = json!("error");
            result["reason"] = json!(e.to_string());
            result["input"] = json!(c.input);
        }
        Err(_) => {
            result["outcome"] = json!("panic");
            result["input"] = json!(c.input);
        }
    }
    result
}

pub fn evaluate(dir: &Path) -> Value {
    let cases = load(dir);
    let mut results = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for c in cases {
        for scripting in c.scripting.map(|s| vec![s]).unwrap_or_else(|| vec![false, true]) {
            let result = run(&c, scripting);
            *counts.entry(result["outcome"].as_str().unwrap().into()).or_default() += 1;
            results.push(result);
        }
    }
    json!({"scope":"Static HTML tree comparisons only. No error-code/location conformance, JavaScript execution, or website compatibility claim. Unsupported cases stay in the denominator.","diagnostics_checked":false,"total":results.len(),"counts":counts,"results":results})
}
