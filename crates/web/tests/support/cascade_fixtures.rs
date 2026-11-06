use olo_web::{
    css::{
        self, Syntax,
        cascade::{self, Defaulted, Origin, SheetBuilder},
        color, math,
        properties::{self as p, Declaration, Property, Value},
        selectors::{self, DomTree},
        variables,
    },
    dom::{Document, NodeId, NodeType},
    html,
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};
type Failure = (&'static str, String);
type Result<T> = std::result::Result<T, Failure>;
fn fail(s: impl Into<String>) -> Failure {
    ("unsupported", s.into())
}
fn err(e: impl std::fmt::Debug) -> Failure {
    ("error", format!("{e:?}"))
}
struct Parent(BTreeMap<String, variables::Computed>);
impl cascade::Inherited for Parent {
    fn custom(&self, n: &str) -> Option<variables::Computed> {
        self.0.get(n).cloned()
    }
}
struct Engine<'a> {
    rules: &'a cascade::RuleSet,
    document: &'a Document,
    target: NodeId,
    inline: Option<&'a str>,
    quirks: bool,
    customs: RefCell<HashMap<NodeId, BTreeMap<String, variables::Computed>>>,
    axes: RefCell<HashMap<NodeId, cascade::Axes>>,
}
impl Engine<'_> {
    fn parent(&self, n: NodeId) -> Result<Option<NodeId>> {
        Ok(self.document.parent(n).map_err(err)?.filter(|p| self.document.node_type(*p).ok() == Some(NodeType::Element)))
    }
    fn inline(&self, n: NodeId) -> Result<Vec<Declaration>> {
        let s = if n == self.target { self.inline.or(self.document.attribute(n, "style").map_err(err)?) } else { self.document.attribute(n, "style").map_err(err)? };
        let Some(s) = s else {
            return Ok(Vec::new());
        };
        let syntax = Syntax::parse(Arc::from(s), Default::default(), None).map_err(err)?;
        syntax.declarations(None).map_err(err)?.iter().map(|d| Declaration::compile(d, Default::default(), None).map_err(err)).collect()
    }
    fn snapshot<'a>(&'a self, n: NodeId, parent: &'a Parent, inline: &'a [Declaration]) -> Result<cascade::Cascade<'a>> {
        self.rules
            .match_element(
                &DomTree::new(self.document),
                n,
                selectors::MatchOptions { quirks: self.quirks, ..Default::default() },
                Some(inline),
                Some(parent),
                Default::default(),
                Default::default(),
                &mut Default::default(),
                None,
            )
            .map_err(err)
    }
    fn customs(&self, n: NodeId, depth: usize) -> Result<BTreeMap<String, variables::Computed>> {
        if depth > 128 {
            return Err(fail("native parent depth"));
        }
        if let Some(v) = self.customs.borrow().get(&n) {
            return Ok(v.clone());
        }
        let parent = Parent(if let Some(n) = self.parent(n)? { self.customs(n, depth + 1)? } else { BTreeMap::new() });
        let inline = self.inline(n)?;
        let c = self.snapshot(n, &parent, &inline)?;
        let mut r = c.resolver();
        let mut values = parent.0.clone();
        for name in c.custom_names() {
            values.insert(name.to_owned(), r.custom(name).map_err(err)?);
        }
        self.customs.borrow_mut().insert(n, values.clone());
        Ok(values)
    }
    fn axes(&self, n: NodeId, depth: usize) -> Result<cascade::Axes> {
        if depth > 128 {
            return Err(fail("native axes parent depth"));
        }
        if let Some(axes) = self.axes.borrow().get(&n) {
            return Ok(*axes);
        }
        let parent_node = self.parent(n)?;
        let parent_axes = parent_node.map(|parent| self.axes(parent, depth + 1)).transpose()?;
        let parent = Parent(if let Some(parent) = parent_node { self.customs(parent, depth + 1)? } else { BTreeMap::new() });
        let inline = self.inline(n)?;
        let c = self.snapshot(n, &parent, &inline)?;
        let axes = c.resolver().axes(parent_axes, true).map_err(err)?;
        self.axes.borrow_mut().insert(n, axes);
        Ok(axes)
    }
    fn defaulted(&self, n: NodeId, p: Property, depth: usize) -> Result<cascade::Projected> {
        let parent = Parent(if let Some(parent) = self.parent(n)? { self.customs(parent, depth + 1)? } else { BTreeMap::new() });
        let inline = self.inline(n)?;
        let c = self.snapshot(n, &parent, &inline)?;
        c.resolver().projected(p, self.axes(n, depth)?, true).map_err(err)
    }
    fn color(&self, n: NodeId, p: Property, depth: usize) -> Result<[f64; 4]> {
        if depth > 128 {
            return Err(fail("native color parent depth"));
        }
        let projected = self.defaulted(n, p, depth)?;
        let value = match projected.value {
            Defaulted::Inherit => {
                if let Some(parent) = self.parent(n)? {
                    return self.color(parent, projected.source, depth + 1);
                } else {
                    initial(p)?
                }
            }
            Defaulted::Initial => initial(p)?,
            Defaulted::Specified(v) => v,
        };
        let Value::Color(value) = value else {
            return Err(fail("non-color computed host"));
        };
        let current = if value.dependencies().current_color {
            if p == Property::Color {
                if let Some(parent) = self.parent(n)? { self.color(parent, Property::Color, depth + 1)? } else { [0., 0., 0., 1.] }
            } else {
                self.color(n, Property::Color, depth + 1)?
            }
        } else {
            [0., 0., 0., 1.]
        };
        let c = value
            .resolve(&ColorHost(current), Default::default(), None)
            .map_err(|e| match e {
                color::Error::ContextUnavailable => fail("color context host"),
                e => err(e),
            })?
            .convert_absolute(color::Space::Srgb)
            .computed();
        let rgb = c.components();
        Ok([rgb[0], rgb[1], rgb[2], c.alpha()])
    }
    fn numeric(&self, n: NodeId, p: Property, depth: usize) -> Result<f64> {
        if depth > 128 {
            return Err(fail("native numeric parent depth"));
        }
        let projected = self.defaulted(n, p, depth)?;
        let value = match projected.value {
            Defaulted::Inherit => {
                if let Some(parent) = self.parent(n)? {
                    return self.numeric(parent, projected.source, depth + 1);
                } else {
                    initial(p)?
                }
            }
            Defaulted::Initial => initial(p)?,
            Defaulted::Specified(v) => v,
        };
        let Value::Numeric(value) = value else {
            return Err(fail("font/intrinsic/used numeric host"));
        };
        value.resolve(&NumericHost, &mut Default::default(), Default::default(), None).map_err(|e| match e {
            p::Error::Numeric(math::Error::ContextUnavailable) => fail("font/percentage/viewport/tree host"),
            e => err(e),
        })
    }
}
fn initial(p: Property) -> Result<Value> {
    let s = Syntax::parse(Arc::from(p.initial_spelling()), Default::default(), None).map_err(err)?;
    let Declaration::Known { values, .. } = Declaration::parse(p.as_str(), s.components(), false, Default::default(), None).map_err(err)? else { unreachable!() };
    Ok(values[0].value.clone())
}
struct NumericHost;
impl math::Context for NumericHost {
    fn factor(&self, u: math::RelativeUnit) -> math::Result<f64> {
        if u == math::RelativeUnit::Rem { Ok(16.) } else { Err(math::Error::ContextUnavailable) }
    }
}
struct ColorHost([f64; 4]);
impl color::Context for ColorHost {
    fn scheme(&self) -> color::Scheme {
        color::Scheme::Light
    }
    fn current_color(&self) -> color::Result<color::Color> {
        Ok(color::Color::new(color::Space::Srgb, [self.0[0], self.0[1], self.0[2]], self.0[3], 0))
    }
    fn system_color(&self, c: color::SystemColor, _: color::Scheme) -> color::Result<color::Color> {
        match c {
            color::SystemColor::CanvasText => Ok(color::Color::new(color::Space::Srgb, [0.; 3], 1., 0)),
            color::SystemColor::Canvas => Ok(color::Color::new(color::Space::Srgb, [1.; 3], 1., 0)),
            _ => Err(color::Error::ContextUnavailable),
        }
    }
}
fn color_reference(s: &str) -> Option<[f64; 4]> {
    let s = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let c: Option<Vec<f64>> = s.split(',').map(|s| s.trim().parse().ok()).collect();
    let c = c?;
    if !(3..=4).contains(&c.len()) {
        return None;
    }
    Some([c[0] / 255., c[1] / 255., c[2] / 255., c.get(3).copied().unwrap_or(1.)])
}
fn evaluate_case(dir: &Path, case: &Json) -> Result<()> {
    let source = std::fs::read_to_string(dir.join("wpt").join(case["file"].as_str().unwrap())).map_err(err)?;
    if source.contains("attachShadow") || source.contains("getAnimations") {
        return Err(fail("shadow/animation document host"));
    }
    // Some upstream helpers create their target nodes through JavaScript.
    // A port may supply that explicit inert DOM; no script is evaluated here.
    let parsed = html::parse(Arc::from(case["html"].as_str().unwrap_or(&source)), Default::default(), None).map_err(err)?;
    let mut b = SheetBuilder::new(Default::default());
    for style in case["styles"].as_array().unwrap() {
        let s = Syntax::parse(Arc::from(style.as_str().unwrap()), Default::default(), None).map_err(err)?;
        b.add_sheet(&s, Origin::Author, None).map_err(err)?;
    }
    let rules = b.finish(None).map_err(err)?;
    for issue in rules.issues() {
        match issue {
            cascade::Issue::Invalid | cascade::Issue::Property(p::Error::Invalid) => {}
            issue => return Err(fail(format!("unimplemented stylesheet context: {issue:?}"))),
        }
    }
    let syntax = Syntax::parse(Arc::from(case["target"].as_str().unwrap()), Default::default(), None).map_err(err)?;
    let query = selectors::Selectors::compile(syntax.components(), Default::default(), Default::default(), None).map_err(err)?;
    let tree = DomTree::new(&parsed.document);
    let mut nodes = Vec::new();
    query
        .query(&tree, parsed.document.root(), false, Default::default(), Default::default(), None, |n, _| {
            nodes.push(n);
            Ok(())
        })
        .map_err(err)?;
    if nodes.len() != 1 {
        return Err(err(format!("target cardinality {}", nodes.len())));
    }
    let engine = Engine {
        rules: &rules,
        document: &parsed.document,
        target: nodes[0],
        inline: case["inline"].as_str(),
        quirks: parsed.quirks == html::QuirksMode::Quirks,
        customs: RefCell::new(HashMap::new()),
        axes: RefCell::new(HashMap::new()),
    };
    let expected = case["expected"].as_str().unwrap();
    let property = case["property"].as_str().unwrap();
    if case["mode"] == "custom" {
        let values = engine.customs(nodes[0], 0)?;
        let computed = values.get(property).ok_or_else(|| ("mismatch", "missing custom value".into()))?;
        let Some(values) = computed.components() else {
            return Err(("mismatch", "guaranteed-invalid custom value".into()));
        };
        let mut values = values.filter(|c| c.kind() != css::Kind::Whitespace);
        let c = values.next().ok_or_else(|| ("mismatch", "empty custom value".into()))?;
        if values.next().is_some() || c.kind() != css::Kind::Ident {
            return Err(fail("compound custom reference port"));
        }
        return if c.text() == Some(expected.trim()) { Ok(()) } else { Err(("mismatch", format!("custom token {:?}, expected {expected}", c.text()))) };
    }
    let p::Name::Longhand(property) = p::Name::parse(property).ok_or_else(|| fail("unknown property port"))? else {
        return Err(fail("shorthand result driver"));
    };
    if case["mode"] == "color" {
        let reference = color_reference(expected).ok_or_else(|| fail("noncanonical color reference"))?;
        let actual = engine.color(nodes[0], property, 0)?;
        if actual.into_iter().zip(reference).all(|(a, b)| (a - b).abs() < 1e-5) { Ok(()) } else { Err(("mismatch", format!("color {actual:?}, expected {reference:?}"))) }
    } else {
        let reference: f64 = expected.strip_suffix("px").unwrap_or(expected).parse().map_err(|_| fail("noncanonical numeric reference"))?;
        let actual = engine.numeric(nodes[0], property, 0)?;
        if (actual - reference).abs() < 1e-6 { Ok(()) } else { Err(("mismatch", format!("numeric {actual}, expected {reference}"))) }
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
        let (outcome, reason) = match std::panic::catch_unwind(|| evaluate_case(dir, case)) {
            Ok(Ok(())) => ("passed", String::new()),
            Ok(Err((kind, reason))) => (kind, reason),
            Err(_) => ("panic", "native cascade evaluator panicked".into()),
        };
        let mut r = case.clone();
        r["outcome"] = json!(outcome);
        if !reason.is_empty() {
            r["reason"] = json!(reason);
        }
        results.push(r);
        *counts.entry(outcome.into()).or_default() += 1;
    }
    for row in data["unported"].as_array().unwrap() {
        let mut r = row.clone();
        r["outcome"] = json!("unsupported");
        results.push(r);
        *counts.entry("unsupported".into()).or_default() += 1;
    }
    json!({"scope":data["scope"],"total":results.len(),"counts":counts,"results":results})
}
