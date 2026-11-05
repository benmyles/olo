//! Independent fixtures. The adapter folds adjacent delim pairs only for
//! the corpus's legacy serialization, without changing engine tokens.
use olo_web::css::{self, Component, Components, Kind, Statement, StatementKind, Syntax, SyntaxError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
fn error(e: SyntaxError) -> Value {
    json!([
        "error",
        match e {
            SyntaxError::Empty => "empty",
            SyntaxError::Invalid => "invalid",
            SyntaxError::ExtraInput => "extra-input",
        }
    ])
}
pub fn components(values: Components<'_>) -> Value {
    let mut values = values.peekable();
    let mut out = Vec::new();
    while let Some(c) = values.next() {
        if c.kind() == Kind::Delim
            && let Some(next) = values.peek()
            && next.kind() == Kind::Delim
            && c.span().end == next.span().start
        {
            let pair = format!("{}{}", c.character().unwrap(), next.character().unwrap());
            if matches!(pair.as_str(), "~=" | "|=" | "^=" | "$=" | "*=" | "||") {
                out.push(json!(pair));
                values.next();
                continue;
            }
        }
        out.push(component(c));
        if matches!(c.kind(), Kind::UnclosedString | Kind::UnclosedUrl) {
            out.push(json!(["error", if c.kind() == Kind::UnclosedString { "eof-in-string" } else { "eof-in-url" }]));
        }
    }
    json!(out)
}
fn component(c: Component<'_>) -> Value {
    let kind = c.kind();
    match kind {
        Kind::Ident | Kind::AtKeyword | Kind::String | Kind::UnclosedString | Kind::Url | Kind::UnclosedUrl => json!([
            match kind {
                Kind::Ident => "ident",
                Kind::AtKeyword => "at-keyword",
                Kind::String | Kind::UnclosedString => "string",
                _ => "url",
            },
            c.text().unwrap()
        ]),
        Kind::Hash | Kind::IdHash => json!(["hash", c.text().unwrap(), if kind == Kind::IdHash { "id" } else { "unrestricted" }]),
        Kind::BadString | Kind::BadUrl => json!(["error", if kind == Kind::BadString { "bad-string" } else { "bad-url" }]),
        Kind::Number | Kind::Integer | Kind::Percentage | Kind::IntegerPercentage | Kind::Dimension | Kind::IntegerDimension => {
            let n = c.number().unwrap();
            let mut v = json!([
                if c.unit().is_some() {
                    "dimension"
                } else if matches!(kind, Kind::Percentage | Kind::IntegerPercentage) {
                    "percentage"
                } else {
                    "number"
                },
                n.representation,
                n.value,
                if n.integer { "integer" } else { "number" }
            ]);
            if let Some(unit) = c.unit() {
                v.as_array_mut().unwrap().push(json!(unit));
            }
            v
        }
        Kind::UnicodeRange => {
            let r = c.unicode_range().unwrap();
            json!(["unicode-range", r.start(), r.end()])
        }
        Kind::Whitespace => json!(" "),
        Kind::Delim => json!(c.character().unwrap().to_string()),
        Kind::Close => json!(["error", c.character().unwrap().to_string()]),
        Kind::Colon => json!(":"),
        Kind::Semicolon => json!(";"),
        Kind::Comma => json!(","),
        Kind::Cdo => json!("<!--"),
        Kind::Cdc => json!("-->"),
        Kind::Function | Kind::Round | Kind::Square | Kind::Curly => {
            let mut out = vec![json!(match kind {
                Kind::Function => "function",
                Kind::Round => "()",
                Kind::Square => "[]",
                _ => "{}",
            })];
            if kind == Kind::Function {
                out.push(json!(c.text().unwrap()));
            }
            out.extend(components(c.children()).as_array().unwrap().iter().cloned());
            json!(out)
        }
    }
}
fn statement(s: &Statement<'_>) -> Value {
    match s.kind() {
        StatementKind::AtRule => json!(["at-rule", s.name().unwrap(), components(s.value()), s.block().map(components)]),
        StatementKind::QualifiedRule => json!(["qualified rule", components(s.value()), components(s.block().unwrap())]),
        StatementKind::Declaration => json!(["declaration", s.name().unwrap(), components(s.value()), s.important()]),
        StatementKind::Invalid(e) => error(e),
    }
}
fn run(file: &str, input: &str) -> std::result::Result<Value, String> {
    // The old corpus tokenizes unicode ranges everywhere. This exercises
    // the current optional descriptor mode; default mode is checked apart.
    let syntax = Syntax::parse_with_unicode_ranges(Arc::from(input), css::Limits::default(), None, true).map_err(|e| e.to_string())?;
    let statements = match file {
        "component_value_list.json" => return Ok(components(syntax.components())),
        "one_component_value.json" => return Ok(syntax.one_component().map(component).unwrap_or_else(error)),
        "one_declaration.json" => {
            return match syntax.one_declaration(None) {
                Ok(s) => Ok(statement(&s)),
                Err(css::ParseError::Syntax(e)) => Ok(error(e)),
                Err(e) => Err(e.to_string()),
            };
        }
        "one_rule.json" => {
            return match syntax.one_rule(None) {
                Ok(s) => Ok(statement(&s)),
                Err(css::ParseError::Syntax(e)) => Ok(error(e)),
                Err(e) => Err(e.to_string()),
            };
        }
        "stylesheet.json" => syntax.stylesheet(None),
        "rule_list.json" => syntax.rule_list(None),
        "declaration_list.json" => syntax.declarations(None),
        "blocks_contents.json" => syntax.block_contents(None),
        _ => unreachable!(),
    }
    .map_err(|e| e.to_string())?;
    Ok(json!(statements.iter().map(statement).collect::<Vec<_>>()))
}
fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b)),
        _ => a == b,
    }
}
// Exact, reviewed legacy cases. Their modern expectations come from
// current CSS Syntax algorithms, not from an engine output snapshot.
fn modern_expected(file: &str, case: usize, expected: &Value) -> Option<(Value, &'static str)> {
    if file == "component_value_list.json" && case == 7 {
        let mut out = Vec::new();
        for value in expected.as_array().unwrap() {
            if value == &json!(["ident", "\u{80}\u{81}"]) {
                out.extend([json!("\u{80}"), json!("\u{81}")]);
            } else {
                out.push(value.clone());
            }
        }
        return Some((json!(out), "2026 non-ASCII identifier ranges exclude U+0080/U+0081; CSS Syntax 4.2"));
    }
    if file == "one_declaration.json" {
        if case == 13 {
            return Some((json!(["declaration", "foo", [], false]), "consume-declaration stops at semicolon; CSS Syntax 5.4.7/5.5.6"));
        }
        if case == 15 {
            return Some((json!(["error", "extra-input"]), "parse-declaration rejects remaining input after semicolon; CSS Syntax 5.4.7"));
        }
        if matches!(case, 12 | 16..=20) {
            let mut current = expected.clone();
            let values = current[2].as_array_mut().unwrap();
            while values.first() == Some(&json!(" ")) {
                values.remove(0);
            }
            while values.last() == Some(&json!(" ")) {
                values.pop();
            }
            return Some((current, "consume-declaration trims leading/trailing whitespace; CSS Syntax 5.5.6; WPT declarations-trim-whitespace.html"));
        }
    }
    if file == "declaration_list.json" && matches!(case, 7..=9) {
        return Some((json!(expected.as_array().unwrap().iter().filter(|v| v[0] != "at-rule").cloned().collect::<Vec<_>>()), "parse-list-of-declarations discards at-rules; CSS Syntax 5.4.5"));
    }
    None
}
pub fn evaluate(dir: &Path) -> Value {
    let pinned: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for (file, sha) in pinned["files"].as_object().unwrap() {
        let bytes = std::fs::read(dir.join(file)).unwrap();
        assert_eq!(sha.as_str().unwrap(), format!("{:x}", Sha256::digest(&bytes)), "pinned CSS fixture changed");
        if !file.ends_with(".json") {
            continue;
        }
        let cases: Value = serde_json::from_slice(&bytes).unwrap();
        let (pairs, remainder) = cases.as_array().unwrap().as_chunks::<2>();
        assert!(remainder.is_empty(), "unpaired CSS fixture input");
        for (i, pair) in pairs.iter().enumerate() {
            let mut r = json!({"file":file,"case":i+1});
            if file == "stylesheet_bytes.json" {
                r["outcome"] = json!("unsupported");
                r["reason"] = json!("byte decoding");
            } else {
                match std::panic::catch_unwind(|| run(file, pair[0].as_str().unwrap())) {
                    Ok(Ok(actual)) => {
                        if equivalent(&actual, &pair[1]) {
                            r["outcome"] = json!("passed");
                        } else {
                            if let Some((modern, reason)) = modern_expected(file, i + 1, &pair[1])
                                && equivalent(&actual, &modern)
                            {
                                r["outcome"] = json!("updated_specification");
                                r["reason"] = json!(reason);
                            } else {
                                r["outcome"] = json!("mismatch");
                            }
                            r["input"] = pair[0].clone();
                            r["legacy_expected"] = pair[1].clone();
                            r["actual"] = actual;
                        }
                    }
                    Ok(Err(e)) => {
                        r["outcome"] = json!("error");
                        r["reason"] = json!(e);
                    }
                    Err(_) => r["outcome"] = json!("panic"),
                }
            }
            *counts.entry(r["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
            results.push(r);
        }
    }
    json!({"scope":"Generic CSS syntax only; legacy delimiter serialization and optional unicode-range descriptor mode. No selectors/property grammar/cascade/layout/site claim.","counts":counts,"total":results.len(),"results":results})
}
