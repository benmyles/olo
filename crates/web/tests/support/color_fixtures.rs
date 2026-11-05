//! Canonical reference strings are decoded by the deliberately restricted
//! adapter below, never by the engine parser/converter under test.
use olo_web::css::{
    self, Syntax,
    color::{Color, Context, Scheme, Space, Value as ColorValue},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
struct Palette {
    scheme: Scheme,
    em: f64,
    container_width: Option<f64>,
}
impl Palette {
    fn new(scheme: Scheme) -> Self {
        Self { scheme, em: 16., container_width: None }
    }
    fn from_case(scheme: Scheme, case: &Value) -> Self {
        Self { scheme, em: case["context"]["em"].as_f64().unwrap_or(16.), container_width: case["context"]["container_width"].as_f64() }
    }
}
impl Context for Palette {
    fn math_factor(&self, unit: css::math::RelativeUnit) -> css::math::Result<f64> {
        match unit {
            css::math::RelativeUnit::Em => Ok(self.em),
            css::math::RelativeUnit::Cqw => self.container_width.map(|w| w / 100.).ok_or(css::math::Error::ContextUnavailable),
            _ => Err(css::math::Error::ContextUnavailable),
        }
    }
    fn current_color(&self) -> css::color::Result<Color> {
        Ok(Color::new(Space::Srgb, [1., 0., 0.], 1., 0))
    }
    fn scheme(&self) -> Scheme {
        self.scheme
    }
}
#[derive(Clone, Debug)]
struct Reference {
    space: Space,
    coordinates: [f64; 3],
    alpha: f64,
    missing: u8,
}
fn reference(input: &str) -> Reference {
    let (function, body) = input.split_once('(').expect("canonical numeric color");
    let body = body.strip_suffix(')').unwrap();
    let mut words: Vec<_> = body.split([' ', ',', '/']).filter(|s| !s.is_empty()).collect();
    let space = match function {
        "rgb" | "rgba" => Space::Srgb,
        "hsl" | "hsla" => Space::Hsl,
        "hwb" => Space::Hwb,
        "lab" => Space::Lab,
        "lch" => Space::Lch,
        "oklab" => Space::Oklab,
        "oklch" => Space::Oklch,
        "color" => match words.remove(0) {
            "srgb" => Space::Srgb,
            "srgb-linear" => Space::SrgbLinear,
            "display-p3" => Space::DisplayP3,
            "display-p3-linear" => Space::DisplayP3Linear,
            "a98-rgb" => Space::A98Rgb,
            "rec2020" => Space::Rec2020,
            "prophoto-rgb" => Space::ProphotoRgb,
            "xyz" | "xyz-d65" => Space::XyzD65,
            "xyz-d50" => Space::XyzD50,
            s => panic!("unsupported canonical space {s}"),
        },
        s => panic!("unsupported canonical function {s}"),
    };
    assert!(matches!(words.len(), 3 | 4));
    let mut missing = 0;
    let values: Vec<_> = words
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if *s == "none" {
                missing |= 1 << i;
                0.
            } else {
                s.trim_end_matches('%').parse::<f64>().unwrap()
            }
        })
        .collect();
    let mut coordinates = [values[0], values[1], values[2]];
    if matches!(function, "rgb" | "rgba") {
        coordinates = coordinates.map(|c| c / 255.);
    }
    Reference { space, coordinates, alpha: values.get(3).copied().unwrap_or(1.), missing }
}
fn modern(mut r: Reference) -> Reference {
    if matches!(r.space, Space::Lab | Space::Lch | Space::Oklab | Space::Oklch) {
        r.coordinates[0] = r.coordinates[0].clamp(0., if matches!(r.space, Space::Lab | Space::Lch) { 100. } else { 1. });
    }
    if matches!(r.space, Space::Lch | Space::Oklch) {
        r.coordinates[1] = r.coordinates[1].max(0.);
        r.coordinates[2] = r.coordinates[2].rem_euclid(360.);
    }
    r
}
fn same_reference(a: &Reference, b: &Reference) -> bool {
    a.space == b.space && a.coordinates == b.coordinates && a.alpha == b.alpha && a.missing == b.missing
}
fn actual(color: Color) -> Value {
    json!({"space":format!("{:?}",color.space()),"coordinates":color.components(),"alpha":color.alpha(),"missing":color.missing()})
}
fn compare(color: Color, expected: &Reference) -> bool {
    let color = if expected.space == Space::Srgb && matches!(color.space(), Space::Hsl | Space::Hwb) { color.convert_absolute(Space::Srgb) } else { color };
    color.space() == expected.space
        && color.missing() == expected.missing
        && color.components().iter().zip(expected.coordinates).all(|(&a, b)| (a - b).abs() <= 1e-6 + 1e-7 * b.abs())
        && // Hex references round alpha to six decimal places (half a unit in
        // the last place), plus a small allowance for f32 storage.
        (color.alpha() - expected.alpha).abs() <= 6e-7
}
fn run(input: &str, expected: &Value) -> Result<(bool, bool, Value), String> {
    let syntax = Syntax::parse(Arc::from(input), css::Limits::default(), None).map_err(|e| e.to_string())?;
    let value = match ColorValue::parse(syntax.components(), Default::default(), None) {
        Ok(v) => v,
        Err(css::color::Error::Invalid) => return Ok((expected.is_null(), false, json!("invalid"))),
        Err(e) => return Err(e.to_string()),
    };
    if expected.is_null() {
        return Ok((false, false, json!("accepted invalid")));
    }
    if expected.as_str().is_some_and(|s| s.eq_ignore_ascii_case("currentcolor")) {
        return Ok((matches!(value, ColorValue::CurrentColor), false, json!("currentcolor")));
    }
    let mut passed = true;
    let mut updated = false;
    let mut output = Vec::new();
    for (i, scheme) in [Scheme::Light, Scheme::Dark].into_iter().enumerate() {
        let reference_value = if expected.is_array() { &expected[i] } else { expected };
        let Some(text) = reference_value.as_str() else {
            return Ok((false, false, json!("expected invalid branch")));
        };
        let old = reference(text);
        let current = modern(old.clone());
        updated |= !same_reference(&old, &current);
        let color = value.resolve(&Palette::new(scheme), Default::default(), None).map_err(|e| e.to_string())?;
        passed &= compare(color, &current);
        output.push(actual(color));
    }
    Ok((passed, updated, json!(output)))
}
pub fn evaluate(dir: &Path) -> Value {
    let pinned: Value = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (name, hash) in pinned["files"].as_object().unwrap() {
        assert_eq!(hash.as_str().unwrap(), format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), "color fixture changed: {name}");
    }
    let hex4: Value = serde_json::from_slice(&std::fs::read(dir.join("color_hexadecimal_4.json")).unwrap()).unwrap();
    let hex4 = hex4.as_array().unwrap().as_chunks::<2>().0.iter().map(|p| (p[0].as_str().unwrap().to_owned(), p[1].clone())).collect::<std::collections::HashMap<_, _>>();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut results = Vec::new();
    for name in pinned["files"].as_object().unwrap().keys().filter(|s| s.starts_with("color_") && s.ends_with(".json")) {
        let data: Value = serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap();
        let (pairs, remainder) = data.as_array().unwrap().as_chunks::<2>();
        assert!(remainder.is_empty());
        for (i, pair) in pairs.iter().enumerate() {
            let input = pair[0].as_str().unwrap();
            let newer = if name == "color_hexadecimal_3.json" && pair[1].is_null() { hex4.get(input).filter(|v| !v.is_null()) } else { None };
            let expected = newer.unwrap_or(&pair[1]);
            let mut row = json!({"file":name,"case":i+1,"input":input});
            match std::panic::catch_unwind(|| run(input, expected)) {
                Ok(Ok((true, changed, _))) => {
                    row["outcome"] = json!(if changed || newer.is_some() { "updated_specification" } else { "passed" });
                    if changed || newer.is_some() {
                        row["reason"] = json!(if newer.is_some() { "CSS Color 4 alpha hex syntax" } else { "CSS Color 4 parse-time lightness/chroma clamping and hue normalization" });
                    }
                }
                Ok(Ok((false, _, actual))) => {
                    row["outcome"] = json!("mismatch");
                    row["actual"] = actual;
                    row["expected"] = expected.clone();
                }
                Ok(Err(e)) => {
                    row["outcome"] = json!(error_outcome(&e));
                    row["reason"] = json!(e);
                }
                Err(_) => row["outcome"] = json!("panic"),
            }
            *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
            results.push(row);
        }
    }
    json!({"scope":"Typed color grammar and explicit absolute coordinate conversion against independent canonical numeric references. Reviewed current-spec adjustments; no CSSOM serialization, cascade, interpolation, gamut mapping, UI, rendering or website claim.","counts":counts,"total":results.len(),"results":results})
}

fn error_outcome(e: &str) -> &'static str {
    if e.contains("Unsupported(") || e.ends_with("RequiresSubstitution") || e.ends_with("ContextUnavailable") { "unsupported" } else { "error" }
}

pub fn evaluate_extra(dir: &Path) -> Value {
    let wpt: Value = serde_json::from_slice(&std::fs::read(dir.join("derived-wpt.json")).unwrap()).unwrap();
    let conversions: Value = serde_json::from_slice(&std::fs::read(dir.join("derived-conversions.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for test in wpt["cases"].as_array().unwrap() {
        let mut row = test.clone();
        let result = std::panic::catch_unwind(|| {
            let input = test["input"].as_str().unwrap();
            let syntax = Syntax::parse(Arc::from(input), css::Limits::default(), None).map_err(|e| e.to_string())?;
            let value = ColorValue::parse(syntax.components(), Default::default(), None);
            if test["mode"] == "invalid" {
                return match value {
                    Err(css::color::Error::Invalid) => Ok((true, json!("invalid"))),
                    Err(e) => Err(e.to_string()),
                    Ok(_) => Ok((false, json!("accepted invalid"))),
                };
            }
            let value = value.map_err(|e| e.to_string())?;
            if test["mode"] == "valid" {
                return Ok((true, json!("validity only")));
            }
            let color = value.resolve(&Palette::from_case(Scheme::Light, test), Default::default(), None).map_err(|e| e.to_string())?;
            let expected = test["expected"].as_str().unwrap();
            if expected.contains("calc(") {
                return Err("Unsupported(\"nonfinite computed serialization reference\")".to_owned());
            }
            let reference = reference(expected);
            // These old WPT computed RGB strings round channels to bytes.
            // Test the exact byte endpoint rather than pretend this port
            // executes CSSOM's full serialization algorithm.
            let good = if expected.starts_with("rgb(") || expected.starts_with("rgba(") {
                let bytes = color.srgb8_clipped();
                let expected_rgb = reference.coordinates.map(|x| (x * 255.).round() as u8);
                bytes[..3] == expected_rgb && (color.alpha() - reference.alpha).abs() <= 6e-7
            } else {
                compare(color, &reference)
            };
            Ok((good, actual(color)))
        });
        match result {
            Ok(Ok((true, _))) => row["outcome"] = json!("passed"),
            Ok(Ok((false, value))) => {
                row["outcome"] = json!("mismatch");
                row["actual"] = value;
            }
            Ok(Err(e)) => {
                row["outcome"] = json!(if e.contains("Unsupported(") || e.ends_with("RequiresSubstitution") || e.ends_with("ContextUnavailable") { "unsupported" } else { "error" });
                row["reason"] = json!(e);
            }
            Err(_) => row["outcome"] = json!("panic"),
        }
        row["suite"] = json!("wpt literal port");
        *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
        results.push(row);
    }
    for test in wpt["unported"].as_array().unwrap() {
        let mut row = test.clone();
        row["outcome"] = json!("unsupported");
        row["suite"] = json!("wpt unported");
        *counts.entry("unsupported".to_owned()).or_default() += 1;
        results.push(row);
    }
    for test in conversions["cases"].as_array().unwrap() {
        let mut row = test.clone();
        let result = std::panic::catch_unwind(|| {
            let target = match test["target"].as_str().unwrap() {
                "srgb" => Space::Srgb,
                "srgb-linear" => Space::SrgbLinear,
                "xyz" | "xyz-d65" => Space::XyzD65,
                "xyz-d50" => Space::XyzD50,
                "p3" | "display-p3" => Space::DisplayP3,
                "p3-linear" | "display-p3-linear" => Space::DisplayP3Linear,
                "a98rgb" | "a98-rgb" => Space::A98Rgb,
                "prophoto" | "prophoto-rgb" => Space::ProphotoRgb,
                "rec2020" => Space::Rec2020,
                "lab" => Space::Lab,
                "lch" => Space::Lch,
                "oklab" => Space::Oklab,
                "oklch" => Space::Oklch,
                "hsl" => Space::Hsl,
                "hwb" => Space::Hwb,
                s => panic!("unsupported standard target {s}"),
            };
            let syntax = Syntax::parse(Arc::from(test["input"].as_str().unwrap()), css::Limits::default(), None).map_err(|e| e.to_string())?;
            let value = ColorValue::parse(syntax.components(), Default::default(), None).map_err(|e| e.to_string())?;
            // Keep the raw specified hue model for conversion; resolution
            // intentionally clips legacy computed RGB and is tested apart.
            let color = match value {
                ColorValue::Absolute(c) => c,
                _ => return Err("contextual conversion source".to_owned()),
            };
            let actual_color = color.convert_absolute(target);
            let values = actual_color.components();
            let epsilon = test["epsilon"].as_f64().unwrap();
            let mut good = true;
            for i in 0..3 {
                let expected = &test["expected"][i];
                if expected.is_object() {
                    continue;
                }
                if expected.is_null() {
                    good &= actual_color.is_missing(i);
                } else {
                    good &= !actual_color.is_missing(i) && (values[i] - expected.as_f64().unwrap()).abs() <= epsilon;
                }
            }
            Ok((good, actual(actual_color)))
        });
        match result {
            Ok(Ok((true, _))) => row["outcome"] = json!("passed"),
            Ok(Ok((false, value))) => {
                row["outcome"] = json!("mismatch");
                row["actual"] = value;
            }
            Ok(Err(e)) => {
                row["outcome"] = json!(if e.contains("Unsupported(") || e.ends_with("RequiresSubstitution") || e.ends_with("ContextUnavailable") { "unsupported" } else { "error" });
                row["reason"] = json!(e);
            }
            Err(_) => row["outcome"] = json!("panic"),
        }
        row["suite"] = json!("independent absolute conversions");
        *counts.entry(row["outcome"].as_str().unwrap().to_owned()).or_default() += 1;
        results.push(row);
    }
    json!({"scope":"Native literal WPT validity/numeric/byte-endpoint ports and independently authored absolute conversion vectors; no upstream JS, CSSOM, full WPT pages or rendering executed.","counts":counts,"total":results.len(),"results":results})
}
