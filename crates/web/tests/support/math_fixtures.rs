//! Literal WPT numeric ports. Restricted references never use engine math.
use olo_web::css::{
    self, Syntax,
    math::{self, Context, Dimension, RelativeUnit, Value as Math},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
struct Host {
    em: f64,
    rem: f64,
    width: f64,
    percentage: f64,
    line_height: f64,
    sibling_index: u32,
    sibling_count: u32,
}
impl Context for Host {
    fn factor(&self, unit: RelativeUnit) -> math::Result<f64> {
        use RelativeUnit::*;
        Ok(match unit {
            Percentage => self.percentage / 100.,
            Em => self.em,
            Rem => self.rem,
            // Explicit representative native context, not font measurement.
            Ex | Ch => self.em / 2.,
            Cap => self.em * 0.7,
            Ic => self.em,
            Lh => self.line_height,
            Rex | Rch => self.rem / 2.,
            Rcap => self.rem * 0.7,
            Ric => self.rem,
            Rlh => self.rem * 1.2,
            Vw | Svw | Lvw | Dvw | Vi | Svi | Lvi | Dvi => 12.,
            Vh | Svh | Lvh | Dvh | Vb | Svb | Lvb | Dvb => 8.,
            Vmin | Svmin | Lvmin | Dvmin => 8.,
            Vmax | Svmax | Lvmax | Dvmax => 12.,
            Cqw | Cqi | Cqmin => self.width / 100.,
            Cqh | Cqb | Cqmax => 1.,
        })
    }
    fn tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
        Ok(match kind {
            math::TreeCount::SiblingIndex => self.sibling_index,
            math::TreeCount::SiblingCount => self.sibling_count,
        })
    }
}
fn reference(input: &str, host: &Host, hint: Option<Dimension>) -> Option<(f64, Dimension)> {
    if let Some(value) = numeric(input, host) {
        return Some(value);
    }
    let body = input.strip_prefix("calc(")?.strip_suffix(')')?;
    if let Some(value) = numeric(body, host) {
        return Some(value);
    }
    // Restricted canonical linear reference only: no engine parser, type
    // algebra, nested functions or general expression evaluation.
    let (left, right) = body.split_once(" + ")?;
    let (mut a, mut at) = numeric(left, host)?;
    let (mut b, mut bt) = numeric(right, host)?;
    if let Some(hint) = hint {
        if at == Dimension::Percentage {
            at = hint;
            a *= host.percentage / 100.;
        }
        if bt == Dimension::Percentage {
            bt = hint;
            b *= host.percentage / 100.;
        }
    }
    if at != bt {
        return None;
    }
    Some((a + b, at))
}
fn numeric(input: &str, host: &Host) -> Option<(f64, Dimension)> {
    let input = input.trim();
    if let Some(body) = input.strip_prefix("calc(").and_then(|s| s.strip_suffix(')')) {
        return match body.to_ascii_lowercase().as_str() {
            "infinity" => Some((f64::INFINITY, Dimension::Number)),
            "-infinity" => Some((f64::NEG_INFINITY, Dimension::Number)),
            "nan" => Some((f64::NAN, Dimension::Number)),
            _ => None,
        };
    }
    let bytes = input.as_bytes();
    let mut split = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    while bytes.get(split).is_some_and(|b| b.is_ascii_digit() || *b == b'.') {
        split += 1;
    }
    if bytes.get(split).is_some_and(|b| matches!(b, b'e' | b'E')) {
        let mut end = split + 1;
        if bytes.get(end).is_some_and(|b| matches!(b, b'+' | b'-')) {
            end += 1;
        }
        if bytes.get(end).is_some_and(u8::is_ascii_digit) {
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            split = end;
        }
    }
    let (number, unit) = input.split_at(split);
    let value = number.parse::<f64>().ok()?;
    use Dimension::*;
    let (factor, dimension) = match unit {
        "" => (1., Number),
        "%" => (1., Percentage),
        "px" => (1., Length),
        "cm" => (96. / 2.54, Length),
        "mm" => (96. / 25.4, Length),
        "Q" | "q" => (96. / 101.6, Length),
        "in" => (96., Length),
        "pt" => (96. / 72., Length),
        "pc" => (16., Length),
        "deg" => (1., Angle),
        "grad" => (0.9, Angle),
        "turn" => (360., Angle),
        "rad" => (180. / std::f64::consts::PI, Angle),
        "s" => (1., Time),
        "ms" => (0.001, Time),
        "hz" | "Hz" => (1., Frequency),
        "khz" | "kHz" => (1000., Frequency),
        "dppx" | "x" => (1., Resolution),
        "dpi" => (1. / 96., Resolution),
        "dpcm" => (2.54 / 96., Resolution),
        "fr" => (1., Flex),
        "em" => (host.em, Length),
        "rem" => (host.rem, Length),
        "ex" | "ch" => (host.em / 2., Length),
        "cap" => (host.em * 0.7, Length),
        "ic" => (host.em, Length),
        "lh" => (host.line_height, Length),
        "rex" | "rch" => (host.rem / 2., Length),
        "rcap" => (host.rem * 0.7, Length),
        "ric" => (host.rem, Length),
        "rlh" => (host.rem * 1.2, Length),
        "vw" | "vi" | "svw" | "lvi" | "dvw" => (12., Length),
        "vh" | "vb" | "svh" | "lvb" | "dvh" => (8., Length),
        "vmin" => (8., Length),
        "vmax" => (12., Length),
        "cqw" | "cqi" => (host.width / 100., Length),
        _ => return None,
    };
    Some((value * factor, dimension))
}
fn options(case: &Value) -> Result<(math::Options, Dimension, bool), String> {
    let ty = case["options"]["type"].as_str().unwrap_or("length");
    let prop = case["options"]["prop"].as_str();
    let (kind, percentage, integer) = match prop {
        Some("width" | "height" | "margin-left" | "margin-right" | "flex-basis") => (Dimension::Length, true, false),
        Some("line-height") => (Dimension::Length, true, false),
        Some("opacity" | "scale") => (Dimension::Number, true, false),
        Some("z-index" | "order" | "column-count") => (Dimension::Number, false, true),
        Some("tab-size" | "flex-grow" | "flex-shrink" | "font-weight") => (Dimension::Number, false, false),
        Some("rotate") => (Dimension::Angle, false, false),
        Some("transition-delay" | "animation-duration") => (Dimension::Time, false, false),
        Some("letter-spacing" | "outline-offset") => (Dimension::Length, false, false),
        Some(_) => return Err("Unsupported: property driver".into()),
        None => match ty {
            "number" => (Dimension::Number, true, false),
            "integer" => (Dimension::Number, false, true),
            "length" => (Dimension::Length, true, false),
            "angle" => (Dimension::Angle, false, false),
            "time" => (Dimension::Time, false, false),
            "resolution" => (Dimension::Resolution, false, false),
            "flex" => (Dimension::Flex, false, false),
            _ => return Err("Unsupported: numeric type driver".into()),
        },
    };
    Ok((math::Options { allow_percentage: percentage, percentage_hint: if percentage && kind != Dimension::Number { Some(kind) } else { None } }, kind, integer))
}
fn run(case: &Value) -> Result<(bool, Value), String> {
    let input = case["input"].as_str().unwrap();
    if ["rotate(", "scale(", "rotate3d(", "rgb(", "color("].iter().any(|s| input.starts_with(s)) {
        return Err("Unsupported: nonnumeric function/property wrapper".into());
    }
    if matches!(case["mode"].as_str(), Some("specified" | "computed_serialization")) {
        return Err("Unsupported: specified/computed serialization assertion".into());
    }
    let (options, expected_kind, integer) = options(case)?;
    let source = Syntax::parse(Arc::from(input), css::Limits::default(), None).map_err(|e| e.to_string())?;
    let value = Math::parse(source.components(), options, Default::default(), None);
    if case["mode"] == "invalid" {
        return match value {
            Err(math::Error::Invalid) => Ok((true, json!("invalid numeric grammar/type"))),
            Err(e) => Err(e.to_string()),
            Ok(v) => Ok((
                v.ty().kind() != Some(expected_kind)
                    && !(case["options"]["prop"] == "tab-size" && v.ty().kind() == Some(Dimension::Length))
                    && !(case["options"]["prop"] == "line-height" && v.ty().kind() == Some(Dimension::Number))
                    && !(expected_kind == Dimension::Number && options.allow_percentage && v.ty().kind() == Some(Dimension::Percentage)),
                json!("numeric type acceptance"),
            )),
        };
    }
    let value = value.map_err(|e| e.to_string())?;
    let host = Host {
        em: case["context"]["em"].as_f64().unwrap_or(16.),
        rem: case["context"]["rem"].as_f64().unwrap_or(16.),
        width: case["context"]["width"].as_f64().unwrap_or(100.),
        percentage: if case["options"]["prop"] == "line-height" { case["context"]["em"].as_f64().unwrap_or(16.) } else { case["context"]["width"].as_f64().unwrap_or(100.) },
        line_height: case["context"]["line_height"].as_f64().unwrap_or(case["context"]["em"].as_f64().unwrap_or(16.) * 1.2),
        sibling_index: case["context"]["sibling_index"].as_u64().unwrap_or(1) as u32,
        sibling_count: case["context"]["sibling_count"].as_u64().unwrap_or(1) as u32,
    };
    let expected = case["expected"].as_str().unwrap();
    let Some((mut reference, reference_kind)) = reference(expected, &host, options.percentage_hint) else {
        return Err("Unsupported: nonliteral canonical reference".into());
    };
    let actual = value.resolve_raw(&host, Default::default(), None).map_err(|e| e.to_string())?;
    let mut a = actual.value;
    let mut kind = actual.ty.kind().unwrap();
    if kind == Dimension::Percentage && options.percentage_hint.is_some() {
        return Err("unexpected unresolved percentage".into());
    }
    if reference_kind == Dimension::Percentage && options.percentage_hint == Some(Dimension::Length) {
        reference *= host.percentage / 100.;
    }
    let reference_kind = if reference_kind == Dimension::Percentage { options.percentage_hint.unwrap_or(reference_kind) } else { reference_kind };
    if expected_kind == Dimension::Number && kind == Dimension::Percentage {
        a /= 100.;
        kind = Dimension::Number;
    }
    if reference_kind != kind {
        return Ok((false, json!({"value":a,"kind":format!("{kind:?}"),"expected_kind":format!("{reference_kind:?}")})));
    }
    if a.is_nan() && reference.is_finite() && case["mode"] != "raw" {
        a = 0.;
    }
    if integer && a.is_finite() {
        a = (a + 0.5).floor();
    }
    let tolerance = case["options"]["approx"].as_f64().unwrap_or(1e-6_f64.max(reference.abs() * 1e-7));
    let good = if reference.is_nan() {
        a.is_nan()
    } else if reference.is_infinite() {
        a == reference
    } else {
        (a - reference).abs() <= tolerance
    };
    Ok((good, json!({"value":format!("{a:?}"),"kind":format!("{kind:?}"),"expected":format!("{reference:?}")})))
}
pub fn evaluate(dir: &Path) -> Value {
    let source: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (name, hash) in source["files"].as_object().unwrap() {
        assert_eq!(hash.as_str().unwrap(), format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), "numeric fixture changed: {name}");
    }
    let corpus: Value = serde_json::from_slice(&std::fs::read(dir.join("derived.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for case in corpus["cases"].as_array().unwrap() {
        let mut row = case.clone();
        match std::panic::catch_unwind(|| run(case)) {
            Ok(Ok((true, _))) => row["outcome"] = json!("passed"),
            Ok(Ok((false, actual))) => {
                row["outcome"] = json!("mismatch");
                row["actual"] = actual;
            }
            Ok(Err(e)) => {
                row["outcome"] = json!(if e.contains("Unsupported") || e.ends_with("RequiresSubstitution") || e.ends_with("ContextUnavailable") { "unsupported" } else { "error" });
                row["reason"] = json!(e);
            }
            Err(_) => row["outcome"] = json!("panic"),
        }
        *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
        results.push(row);
    }
    for case in corpus["unported"].as_array().unwrap() {
        let mut row = case.clone();
        row["outcome"] = json!("unsupported");
        *counts.entry("unsupported".into()).or_default() += 1;
        results.push(row);
    }
    json!({"scope":"Native numeric grammar/type/raw-coordinate ports with explicit representative host units. No upstream JS/CSSOM/page/style/layout execution or complete property conformance.","total":results.len(),"counts":counts,"results":results})
}
