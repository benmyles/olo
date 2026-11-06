use olo_web::{
    css::{
        self, Syntax,
        cascade::{self, Origin, SheetBuilder},
        math, properties as p,
        style::{self, Defaults, Style, Value},
    },
    dom::{Document, NodeId, NodeType},
    html,
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc};
type Failure = (&'static str, String);
type Result<T> = std::result::Result<T, Failure>;
fn unsupported(s: impl Into<String>) -> Failure {
    ("unsupported", s.into())
}
fn error(s: impl std::fmt::Debug) -> Failure {
    ("error", format!("{s:?}"))
}
fn mismatch(s: impl std::fmt::Debug) -> Failure {
    ("mismatch", format!("{s:?}"))
}

#[path = "style_host.rs"]
mod host;
pub use host::FixtureHost;
fn named(document: &Document, name: &str) -> Result<NodeId> {
    let mut pending = vec![document.root()];
    let mut found = None;
    while let Some(node) = pending.pop() {
        if document.node_type(node).map_err(error)? == NodeType::Element && document.attribute(node, "id").map_err(error)? == Some(name) {
            if found.is_some() {
                return Err(error("duplicate fixture target"));
            }
            found = Some(node);
        }
        let mut child = document.first_child(node).map_err(error)?;
        while let Some(node) = child {
            pending.push(node);
            child = document.next_sibling(node).map_err(error)?;
        }
    }
    found.ok_or_else(|| error("missing fixture target"))
}
fn display_matches(actual: css::properties::Display, expected: &str) -> bool {
    use css::properties::{Display as D, Inner as I, Internal as T, Outer as O};
    match actual {
        D::None => expected == "none",
        D::Contents => expected == "contents",
        D::Internal(internal) => {
            expected
                == match internal {
                    T::TableRowGroup => "table-row-group",
                    T::TableHeaderGroup => "table-header-group",
                    T::TableFooterGroup => "table-footer-group",
                    T::TableRow => "table-row",
                    T::TableCell => "table-cell",
                    T::TableColumnGroup => "table-column-group",
                    T::TableColumn => "table-column",
                    T::TableCaption => "table-caption",
                    T::RubyBase => "ruby-base",
                    T::RubyText => "ruby-text",
                    T::RubyBaseContainer => "ruby-base-container",
                    T::RubyTextContainer => "ruby-text-container",
                }
        }
        D::Box { outer, inner, list_item } => {
            let actual_outer = match outer {
                O::Block => "block",
                O::Inline => "inline",
                O::RunIn => "run-in",
            };
            let actual_inner = match inner {
                I::Flow => "flow",
                I::FlowRoot => "flow-root",
                I::Table => "table",
                I::Flex => "flex",
                I::Grid => "grid",
                I::Ruby => "ruby",
            };
            let legacy = match expected {
                "inline-block" => Some(("inline", "flow-root")),
                "inline-table" => Some(("inline", "table")),
                "inline-flex" => Some(("inline", "flex")),
                "inline-grid" => Some(("inline", "grid")),
                _ => None,
            };
            if let Some((o, i)) = legacy {
                return actual_outer == o && actual_inner == i && !list_item;
            }
            let words: Vec<_> = expected.split_ascii_whitespace().collect();
            if words.iter().any(|w| !matches!(*w, "block" | "inline" | "run-in" | "flow" | "flow-root" | "table" | "flex" | "grid" | "ruby" | "list-item")) {
                return false;
            }
            let o = words.iter().copied().find(|w| matches!(*w, "block" | "inline" | "run-in")).unwrap_or(if words.contains(&"ruby") { "inline" } else { "block" });
            let i = words.iter().copied().find(|w| matches!(*w, "flow" | "flow-root" | "table" | "flex" | "grid" | "ruby")).unwrap_or("flow");
            actual_outer == o && actual_inner == i && list_item == words.contains(&"list-item")
        }
    }
}
fn close(actual: f64, expected: f64) -> Result<()> {
    if (actual - expected).abs() <= 1e-6 { Ok(()) } else { Err(mismatch(format!("numeric {actual}, expected {expected}"))) }
}
fn numeric(style: &Style, property: p::Property, expected: &str) -> Result<()> {
    if property == p::Property::LineHeight && expected != "normal" {
        return close(style.line_height(), expected.strip_suffix("px").unwrap_or(expected).parse().map_err(error)?);
    }
    if expected == "normal" && property == p::Property::LetterSpacing {
        return close(style.value(property).scalar().ok_or_else(|| mismatch("normal letter spacing must compute to zero"))?, 0.);
    }
    let Value::Numeric(n) = style.value(property) else {
        return if let Value::Keyword(k) = style.value(property) {
            if k.as_str() == expected { Ok(()) } else { Err(mismatch(format!("keyword {k:?}, expected {expected}"))) }
        } else {
            Err(unsupported("non-numeric reference"))
        };
    };
    let linear = if let Some(percent) = expected.strip_suffix('%') {
        Some((percent.parse::<f64>().map_err(error)?, 0.))
    } else if expected.starts_with("calc(") {
        let compact: String = expected.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let body = compact.strip_prefix("calc(").unwrap().strip_suffix(')').ok_or_else(|| unsupported("compound reference"))?;
        let (percent, length) = body.split_once('%').ok_or_else(|| unsupported("compound reference"))?;
        Some((percent.parse::<f64>().map_err(error)?, length.strip_suffix("px").ok_or_else(|| unsupported("compound reference"))?.parse::<f64>().map_err(error)?))
    } else {
        None
    };
    if let Some((percent, length)) = linear {
        if !n.expression().dependencies().contains(math::RelativeUnit::Percentage) {
            return Err(mismatch("computed percentage was resolved too early"));
        }
        struct Basis(f64);
        impl math::Context for Basis {
            fn factor(&self, u: math::RelativeUnit) -> math::Result<f64> {
                if u == math::RelativeUnit::Percentage { Ok(self.0) } else { Err(math::Error::ContextUnavailable) }
            }
        }
        for basis in [1., 2.75] {
            close(n.resolve(&Basis(basis), &mut Default::default(), Default::default(), None).map_err(error)?, percent * basis + length)?;
        }
        Ok(())
    } else {
        close(n.expression().constant().ok_or_else(|| mismatch("non-constant computed length/number"))?.value, expected.strip_suffix("px").unwrap_or(expected).parse().map_err(error)?)
    }
}
fn compare(style: &Style, property: &str, expected: &str) -> Result<()> {
    if property == "display" {
        let Value::Display(display) = style.value(p::Property::Display) else { return Err(error("display value type")) };
        return if display_matches(*display, expected) { Ok(()) } else { Err(mismatch(format!("display {display:?}, expected {expected}"))) };
    }
    if property == "overflow" {
        let words: Vec<_> = expected.split_ascii_whitespace().collect();
        for (i, p) in [p::Property::OverflowX, p::Property::OverflowY].into_iter().enumerate() {
            numeric(style, p, words[i.min(words.len() - 1)])?;
        }
        return Ok(());
    }
    if property == "border-width" {
        let words: Vec<_> = expected.split_ascii_whitespace().collect();
        let indices = match words.len() {
            1 => [0, 0, 0, 0],
            2 => [0, 1, 0, 1],
            3 => [0, 1, 2, 1],
            4 => [0, 1, 2, 3],
            _ => return Err(unsupported("border shorthand reference")),
        };
        for (p, i) in [p::Property::BorderTopWidth, p::Property::BorderRightWidth, p::Property::BorderBottomWidth, p::Property::BorderLeftWidth].into_iter().zip(indices) {
            close(style.used_border_width(p).map_err(error)?, words[i].strip_suffix("px").ok_or_else(|| unsupported("border length reference"))?.parse().map_err(error)?)?;
        }
        return Ok(());
    }
    let p::Name::Longhand(property) = p::Name::parse(property).ok_or_else(|| unsupported("property registry"))? else { return Err(unsupported("shorthand reference")) };
    if matches!(property, p::Property::BorderTopWidth | p::Property::BorderRightWidth | p::Property::BorderBottomWidth | p::Property::BorderLeftWidth) {
        return close(style.used_border_width(property).map_err(error)?, expected.strip_suffix("px").unwrap_or(expected).parse().map_err(error)?);
    }
    numeric(style, property, expected)
}
fn evaluate_case(dir: &Path, case: &Json) -> Result<()> {
    let source = std::fs::read_to_string(dir.join("wpt").join(case["file"].as_str().unwrap())).map_err(error)?;
    let mut parsed = html::parse(Arc::from(source), Default::default(), None).map_err(error)?;
    let target = named(&parsed.document, "target")?;
    // A CSSOM value can end in an implicitly closed function at EOF. A
    // trailing semicolon would become a token inside that function.
    let extra = case["extra_inline"].as_str().map(|s| format!(";{s}")).unwrap_or_default();
    parsed.document.set_attribute(target, "style", format!("{}:{}{}", case["property"].as_str().unwrap(), case["specified"].as_str().unwrap(), extra)).map_err(error)?;
    if let Some(inline) = case["parent_inline"].as_str() {
        let parent = named(&parsed.document, "container")?;
        parsed.document.set_attribute(parent, "style", inline).map_err(error)?;
    }
    let mut builder = SheetBuilder::new(Default::default());
    for sheet in case["styles"].as_array().unwrap() {
        let syntax = Syntax::parse(Arc::from(sheet.as_str().unwrap()), Default::default(), None).map_err(error)?;
        builder.add_sheet(&syntax, Origin::Author, None).map_err(error)?;
    }
    let rules = builder.finish(None).map_err(error)?;
    for issue in rules.issues() {
        if !matches!(issue, cascade::Issue::Invalid | cascade::Issue::Property(p::Error::Invalid)) {
            return Err(unsupported(format!("stylesheet context: {issue:?}")));
        }
    }
    let host = FixtureHost;
    let defaults = Defaults::new(&host, Default::default(), None).map_err(error)?;
    let mut job =
        style::Job::new(&parsed.document, &rules, defaults, style::JobOptions { quirks: parsed.quirks == html::QuirksMode::Quirks, ..Default::default() }, Default::default(), None).map_err(error)?;
    loop {
        match job.step(32, |_| &host) {
            Ok(progress) if progress.finished => break,
            Ok(_) => {}
            Err(style::Error::ContextUnavailable | style::Error::Unsupported(_)) => return Err(unsupported("computed context")),
            Err(e) => return Err(error(e)),
        }
    }
    let tree = job.finish().map_err(error)?;
    if tree.report().inline_unsupported != 0 {
        return Err(unsupported("inline feature"));
    }
    if tree.report().inline_invalid != 0 {
        return Err(mismatch("known-valid inline declaration rejected"));
    }
    let style = tree.get(&parsed.document, target, 0).map_err(error)?.ok_or_else(|| error("missing computed target"))?;
    let expected = &case["expected"];
    if let Some(values) = expected.as_array() {
        for v in values {
            if compare(style, case["property"].as_str().unwrap(), v.as_str().unwrap()).is_ok() {
                return Ok(());
            }
        }
        Err(mismatch("no allowed computed reference matched"))
    } else {
        compare(style, case["property"].as_str().unwrap(), expected.as_str().unwrap())
    }
}
pub fn evaluate(dir: &Path) -> Json {
    let manifest: Json = serde_json::from_slice(&std::fs::read(dir.join("SOURCE.json")).unwrap()).unwrap();
    for (name, hash) in manifest["files"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(dir.join(name)).unwrap())), hash.as_str().unwrap(), "{name}");
    }
    let data: Json = serde_json::from_slice(&std::fs::read(dir.join("derived.json")).unwrap()).unwrap();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut results = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let (outcome, reason) = match std::panic::catch_unwind(|| evaluate_case(dir, case)) {
            Ok(Ok(())) => ("passed", String::new()),
            Ok(Err(f)) => f,
            Err(_) => ("panic", "computed-style port panicked".into()),
        };
        let mut row = case.clone();
        row["outcome"] = json!(outcome);
        if !reason.is_empty() {
            row["reason"] = json!(reason);
        }
        results.push(row);
        *counts.entry(outcome.into()).or_default() += 1;
    }
    for case in data["unported"].as_array().unwrap() {
        let mut row = case.clone();
        row["outcome"] = json!("unsupported");
        results.push(row);
        *counts.entry("unsupported".into()).or_default() += 1;
    }
    json!({"scope":data["scope"],"total":results.len(),"counts":counts,"results":results})
}
