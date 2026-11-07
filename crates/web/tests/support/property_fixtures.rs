use olo_web::css::{
    self, Syntax, color, math,
    properties::{self as p, Declaration, Error, Value},
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc};
type Check = Result<(), (&'static str, String)>;
fn unsupported(s: impl Into<String>) -> (&'static str, String) {
    ("unsupported", s.into())
}
fn mismatch(s: impl Into<String>) -> (&'static str, String) {
    ("mismatch", s.into())
}
fn parse(name: &str, input: &str) -> p::Result<Declaration> {
    let syntax = Syntax::parse(Arc::from(input), css::Limits::default(), None).map_err(|_| Error::Limit)?;
    Declaration::parse(name, syntax.components(), false, p::Limits::default(), None)
}
fn compile_error(e: Error) -> (&'static str, String) {
    match e {
        Error::Unsupported(s) => unsupported(s),
        Error::UnknownProperty => unsupported("property not implemented"),
        e => ("error", format!("{e:?}")),
    }
}
struct ColorHost;
impl color::Context for ColorHost {
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
}
fn compare(value: &Value, expected: &Json, property: &str) -> Check {
    if let Some(options) = expected.as_array() {
        let mut supported = false;
        for option in options {
            match compare(value, option, property) {
                Ok(()) => return Ok(()),
                Err(("mismatch", _)) => supported = true,
                _ => {}
            }
        }
        return Err(if supported { mismatch(format!("no allowed reference matched: {expected}")) } else { unsupported("reference alternatives require unavailable host") });
    }
    let expected = expected.as_str().ok_or_else(|| unsupported("non-string scalar reference"))?;
    match value {
        Value::Tracks(_) | Value::GridLine(_) => Err(unsupported("grid value reference")),
        Value::Radius(_) | Value::Images(_) | Value::Shadows(_) | Value::Sizes(_) | Value::Positions(_) | Value::Repeats(_) | Value::Transforms(_) | Value::Features(_) => {
            Err(unsupported("decoration value reference"))
        }
        Value::Numeric(n) => {
            let (reference, suffix) = scalar(expected).ok_or_else(|| unsupported("noncanonical numeric reference or relative host"))?;
            if suffix == "%" {
                return Err(unsupported("preserved percentage computation host"));
            }
            if !matches!(suffix, "" | "px") {
                return Err(unsupported("noncanonical numeric unit reference"));
            }
            let dimension = n.expression().ty().kind();
            if suffix == "px" && dimension != Some(math::Dimension::Length) || suffix.is_empty() && !matches!(dimension, Some(math::Dimension::Number | math::Dimension::Percentage)) {
                return Err(mismatch(format!("numeric reference dimension: {expected}")));
            }
            let actual = n.resolve(&math::NoContext, &mut math::Scratch::default(), math::Limits::default(), None).map_err(|e| match e {
                Error::Numeric(math::Error::ContextUnavailable) => unsupported("font/percent/viewport/tree numeric host"),
                e => ("error", format!("{e:?}")),
            })?;
            if (actual - reference).abs() <= 1e-6f64.max(reference.abs() * 1e-6) { Ok(()) } else { Err(mismatch(format!("numeric {actual}, expected {expected}"))) }
        }
        Value::Keyword(k) => {
            if matches!(property, "font-size" | "font-weight" | "line-height") {
                return Err(unsupported("font keyword computation host"));
            }
            if k.as_str().eq_ignore_ascii_case(expected) { Ok(()) } else { Err(mismatch(format!("keyword {}, expected {expected}", k.as_str()))) }
        }
        Value::Color(c) => {
            if c.dependencies() != color::Dependencies::default() {
                return Err(unsupported("contextual color host"));
            }
            let reference = color_reference(expected).ok_or_else(|| unsupported("noncanonical/compound color reference"))?;
            let c = c.resolve(&ColorHost, color::Limits::default(), None).map_err(|e| ("error", format!("{e:?}")))?.convert_absolute(color::Space::Srgb).computed();
            let v = c.components();
            let actual = [v[0], v[1], v[2], c.alpha()];
            if actual.into_iter().zip(reference).all(|(a, b)| (a - b).abs() < 1e-5) { Ok(()) } else { Err(mismatch(format!("color {actual:?}, expected {reference:?}"))) }
        }
        Value::LocalUrl(_) | Value::Inset(_) => Err(unsupported("clip-source CSSOM reference driver")),
        Value::Families(_) => Err(unsupported("font computed host/CSSOM reference driver")),
        Value::Display(_) => Err(unsupported("display computed/CSSOM reference driver")),
        Value::FitContent(_) => Err(unsupported("intrinsic size computation host")),
        Value::Pending(_) => Err(unsupported("substitution element host")),
        Value::Wide(_) => Err(unsupported("cascade/inheritance host")),
    }
}
// Independent canonical references: Rust's decimal parser and fixed CSS px
// suffix. No engine parser/serializer is used to interpret expected values.
fn scalar(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s.char_indices().find(|(_, c)| !c.is_ascii_digit() && !matches!(c, '+' | '-' | '.' | 'e' | 'E')).map_or(s.len(), |(i, _)| i);
    Some((s[..end].parse().ok()?, &s[end..]))
}
fn color_reference(s: &str) -> Option<[f64; 4]> {
    match s {
        "red" => return Some([1., 0., 0., 1.]),
        "lime" => return Some([0., 1., 0., 1.]),
        "blue" => return Some([0., 0., 1., 1.]),
        "black" => return Some([0., 0., 0., 1.]),
        "white" => return Some([1., 1., 1., 1.]),
        "transparent" => return Some([0., 0., 0., 0.]),
        _ => {}
    }
    let s = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let components: Option<Vec<f64>> = s.split(',').map(|s| s.trim().parse().ok()).collect();
    let c = components?;
    if !(3..=4).contains(&c.len()) {
        return None;
    }
    Some([c[0] / 255., c[1] / 255., c[2] / 255., c.get(3).copied().unwrap_or(1.)])
}
fn case(c: &Json) -> Check {
    let property = c["property"].as_str().unwrap();
    let input = c["input"].as_str().unwrap();
    let parsed = parse(property, input);
    if c["mode"] == "parse" {
        return match (parsed, c["valid"].as_bool().unwrap()) {
            (Ok(_), true) | (Err(Error::Invalid), false) => Ok(()),
            (Ok(_), false) => Err(mismatch("accepted invalid declaration")),
            (Err(Error::Invalid), true) => Err(mismatch("rejected valid declaration")),
            (Err(e), _) => Err(compile_error(e)),
        };
    }
    if c.get("requires").is_some() {
        return Err(unsupported("extra JS setup/host arguments"));
    }
    let Declaration::Known { values, .. } = parsed.map_err(compile_error)? else {
        return Err(unsupported("custom property host"));
    };
    if c["mode"] == "computed" {
        if matches!(property, "overflow" | "overflow-x" | "overflow-y" | "overflow-block" | "overflow-inline" | "width" | "height" | "top" | "right" | "bottom" | "left" | "inset" | "position") {
            return Err(unsupported("coupled/used-value element and layout host"));
        }
        if values.len() != 1 {
            return Err(unsupported("compound computed reference"));
        }
        compare(&values[0].value, &c["expected"], property)
    } else {
        let expected = c["expected"].as_object().ok_or_else(|| unsupported("shorthand reference driver"))?;
        if values.len() != expected.len() {
            return Err(mismatch("shorthand member count"));
        }
        for (property, expected) in expected {
            let value = values.iter().find(|v| v.property.as_str() == property).ok_or_else(|| mismatch(format!("missing longhand {property}")))?;
            compare(&value.value, expected, property)?;
        }
        Ok(())
    }
}
pub fn evaluate(dir: &Path) -> Json {
    let manifest: Json = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (name, hash) in manifest["files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), hash.as_str().unwrap(), "{name}");
    }
    let data: Json = serde_json::from_slice(&std::fs::read(dir.join("derived.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    let mut counts = BTreeMap::<String, usize>::new();
    for case in data["cases"].as_array().unwrap() {
        let result = std::panic::catch_unwind(|| self::case(case));
        let (outcome, reason) = match result {
            Ok(Ok(())) => ("passed", String::new()),
            Ok(Err((kind, reason))) => (kind, reason),
            Err(_) => ("panic", "native evaluator panicked".into()),
        };
        *counts.entry(outcome.into()).or_default() += 1;
        let mut r = case.clone();
        r["outcome"] = json!(outcome);
        if !reason.is_empty() {
            r["reason"] = json!(reason);
        }
        results.push(r);
    }
    for case in data["unported"].as_array().unwrap() {
        let mut r = case.clone();
        r["outcome"] = json!("unsupported");
        results.push(r);
        *counts.entry("unsupported".into()).or_default() += 1;
    }
    json!({"scope":data["scope"],"total":results.len(),"counts":counts,"results":results})
}
