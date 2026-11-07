//! Media queries (Media Queries Level 4/5) and `@supports` conditions over
//! parsed components. Evaluation is three-valued: a feature the engine does
//! not know is unknown, which `not` keeps unknown and a top-level query
//! treats as false, as the specification's general-enclosed rule says.
use super::{Components, Kind, properties as p};
use std::sync::atomic::{AtomicBool, Ordering};
/// What the page is being shown on: the inputs media features read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Media {
    /// The viewport in CSS pixels.
    pub width: f64,
    pub height: f64,
    /// Device pixels per CSS pixel.
    pub scale: f64,
    pub dark: bool,
    pub print: bool,
    /// Scripts run on this page (`scripting: enabled`).
    pub scripting: bool,
}
impl Media {
    pub fn screen(width: f64, height: f64, scale: f64, dark: bool) -> Self {
        Self { width, height, scale, dark, print: false, scripting: false }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Truth {
    True,
    False,
    Unknown,
}
impl Truth {
    fn of(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}
/// A media query list is true when any of its queries is; an empty list is.
pub fn evaluate(list: Components<'_>, media: &Media, cancelled: Option<&AtomicBool>) -> bool {
    let items: Vec<_> = list.filter(|c| c.kind() != Kind::Whitespace).collect();
    if items.is_empty() {
        return true;
    }
    let mut result = false;
    for query in items.split(|c| c.kind() == Kind::Comma) {
        if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return false;
        }
        // One malformed query in a list is `not all`, never the whole list.
        if query_truth(query, media) == Truth::True {
            result = true;
        }
    }
    result
}
type Item<'a> = super::Component<'a>;
fn ident<'a>(c: &Item<'a>) -> Option<&'a str> {
    (c.kind() == Kind::Ident).then(|| c.text()).flatten()
}
fn is(c: &Item<'_>, word: &str) -> bool {
    ident(c).is_some_and(|i| i.eq_ignore_ascii_case(word))
}
fn query_truth(query: &[Item<'_>], media: &Media) -> Truth {
    let Some(first) = query.first() else { return Truth::False };
    if first.kind() == Kind::Round {
        return condition(query, media, true);
    }
    let mut at = 0;
    let mut negate = false;
    if is(first, "not") || is(first, "only") {
        negate = is(first, "not");
        at = 1;
    }
    let Some(ty) = query.get(at).and_then(ident) else { return Truth::False };
    let type_truth = Truth::of(ty.eq_ignore_ascii_case("all") || ty.eq_ignore_ascii_case("screen") && !media.print || ty.eq_ignore_ascii_case("print") && media.print);
    at += 1;
    let mut truth = type_truth;
    if let Some(and) = query.get(at) {
        if !is(and, "and") {
            return Truth::False;
        }
        let rest = &query[at + 1..];
        if rest.is_empty() {
            return Truth::False;
        }
        truth = truth.and(condition(rest, media, false));
    }
    if negate { truth.not() } else { truth }
}
/// `<media-condition>`: `not` in parens, or parens joined by all `and` or all `or`.
fn condition(items: &[Item<'_>], media: &Media, allow_or: bool) -> Truth {
    let Some(first) = items.first() else { return Truth::False };
    if is(first, "not") {
        if items.len() != 2 {
            return Truth::False;
        }
        return in_parens(&items[1], media).not();
    }
    let mut truth = in_parens(first, media);
    let mut op: Option<&str> = None;
    let mut rest = &items[1..];
    while !rest.is_empty() {
        let Some(word) = ident(&rest[0]) else { return Truth::False };
        let word = if word.eq_ignore_ascii_case("and") {
            "and"
        } else if word.eq_ignore_ascii_case("or") && allow_or {
            "or"
        } else {
            return Truth::False;
        };
        if op.is_some_and(|o| o != word) {
            return Truth::False;
        }
        op = Some(word);
        let Some(next) = rest.get(1) else { return Truth::False };
        let value = in_parens(next, media);
        truth = if word == "and" { truth.and(value) } else { truth.or(value) };
        rest = &rest[2..];
    }
    truth
}
fn in_parens(item: &Item<'_>, media: &Media) -> Truth {
    if item.kind() != Kind::Round {
        // A function such as `general-enclosed` is unknown.
        return Truth::Unknown;
    }
    let inner: Vec<_> = item.children().filter(|c| c.kind() != Kind::Whitespace).collect();
    let Some(first) = inner.first() else { return Truth::Unknown };
    if first.kind() == Kind::Round || is(first, "not") {
        return condition(&inner, media, true);
    }
    // `( feature )`, `( feature : value )` or a range.
    if let Some(name) = ident(first) {
        if inner.len() == 1 {
            return boolean_feature(name, media);
        }
        if inner[1].kind() == Kind::Colon {
            return plain_feature(name, &inner[2..], media);
        }
    }
    range_feature(&inner, media)
}
/// Units in a media query's calc(): the viewport's, and the initial font
/// (16px em, with the usual ratios), as in a style with no element.
struct MediaContext<'a>(&'a Media);
impl super::math::Context for MediaContext<'_> {
    fn factor(&self, unit: super::math::RelativeUnit) -> super::math::Result<f64> {
        use super::math::RelativeUnit as U;
        let (w, h) = (self.0.width, self.0.height);
        Ok(match unit {
            U::Em | U::Rem | U::Ic | U::Ric => 16.,
            U::Ex | U::Rex | U::Ch | U::Rch => 8.,
            U::Cap | U::Rcap => 11.,
            U::Lh | U::Rlh => 19.2,
            U::Vw | U::Svw | U::Lvw | U::Dvw | U::Vi | U::Svi | U::Lvi | U::Dvi => w / 100.,
            U::Vh | U::Svh | U::Lvh | U::Dvh | U::Vb | U::Svb | U::Lvb | U::Dvb => h / 100.,
            U::Vmin | U::Svmin | U::Lvmin | U::Dvmin => w.min(h) / 100.,
            U::Vmax | U::Svmax | U::Lvmax | U::Dvmax => w.max(h) / 100.,
            _ => return Err(super::math::Error::ContextUnavailable),
        })
    }
}
fn length(item: &Item<'_>, media: &Media) -> Option<f64> {
    // calc() and the other math functions: compiled and resolved against
    // the viewport (`(max-width: calc(1120px - 1px))`).
    if item.kind() == Kind::Function {
        let expression = super::math::Value::compile(*item, super::math::Options { allow_percentage: false, percentage_hint: None }, Default::default(), None).ok()?;
        if expression.ty().kind() != Some(super::math::Dimension::Length) {
            return None;
        }
        let q = expression.resolve_with(&MediaContext(media), &mut super::math::Scratch::default(), Default::default(), None).ok()?;
        return q.value.is_finite().then_some(q.value);
    }
    let n = item.number()?;
    match item.kind() {
        Kind::Integer | Kind::Number => (n.value == 0.).then_some(0.),
        Kind::Dimension | Kind::IntegerDimension => {
            let unit = item.unit()?;
            let factor = match unit.to_ascii_lowercase().as_str() {
                "px" => 1.,
                "em" | "rem" => 16.,
                "ex" | "rex" => 8.,
                "ch" | "rch" => 8.,
                "cm" => 96. / 2.54,
                "mm" => 96. / 25.4,
                "q" => 96. / 101.6,
                "in" => 96.,
                "pt" => 4. / 3.,
                "pc" => 16.,
                "vw" | "svw" | "lvw" | "dvw" | "vi" => media.width / 100.,
                "vh" | "svh" | "lvh" | "dvh" | "vb" => media.height / 100.,
                "vmin" => media.width.min(media.height) / 100.,
                "vmax" => media.width.max(media.height) / 100.,
                _ => return None,
            };
            Some(n.value * factor)
        }
        _ => None,
    }
}
fn resolution(item: &Item<'_>) -> Option<f64> {
    let n = item.number()?;
    let unit = item.unit()?;
    match unit.to_ascii_lowercase().as_str() {
        "dppx" | "x" => Some(n.value),
        "dpi" => Some(n.value / 96.),
        "dpcm" => Some(n.value * 2.54 / 96.),
        _ => None,
    }
}
/// `a / b` or a single number.
fn ratio(items: &[Item<'_>]) -> Option<f64> {
    match items {
        [a] => a.number().map(|n| n.value).filter(|v| *v > 0.),
        [a, slash, b] if slash.character() == Some('/') => {
            let (a, b) = (a.number()?.value, b.number()?.value);
            (a >= 0. && b > 0.).then(|| a / b)
        }
        _ => None,
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Feature {
    Width,
    Height,
    AspectRatio,
    Resolution,
    Color,
    Monochrome,
    Other,
}
/// The current value of a numeric feature.
fn numeric(name: &str, media: &Media) -> (Feature, f64) {
    match name.to_ascii_lowercase().as_str() {
        "width" | "device-width" => (Feature::Width, media.width),
        "height" | "device-height" => (Feature::Height, media.height),
        "aspect-ratio" | "device-aspect-ratio" => (Feature::AspectRatio, if media.height > 0. { media.width / media.height } else { 0. }),
        "resolution" => (Feature::Resolution, media.scale),
        "color" | "color-index" => (Feature::Color, 8.),
        "monochrome" => (Feature::Monochrome, 0.),
        _ => (Feature::Other, 0.),
    }
}
fn value(feature: Feature, items: &[Item<'_>], media: &Media) -> Option<f64> {
    match feature {
        Feature::Width | Feature::Height => match items {
            [item] => length(item, media),
            _ => None,
        },
        Feature::AspectRatio => ratio(items),
        Feature::Resolution => match items {
            [item] => resolution(item),
            _ => None,
        },
        Feature::Color | Feature::Monochrome => match items {
            [item] => item.number().filter(|n| n.integer && n.value >= 0.).map(|n| n.value),
            _ => None,
        },
        Feature::Other => None,
    }
}
fn boolean_feature(name: &str, media: &Media) -> Truth {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "color" | "color-index" | "width" | "height" | "aspect-ratio" | "resolution" | "hover" | "any-hover" | "pointer" | "any-pointer" | "grid" | "device-width" | "device-height" => {
            Truth::of(!matches!(lower.as_str(), "grid"))
        }
        "monochrome" | "prefers-reduced-motion" | "prefers-reduced-transparency" | "prefers-contrast" | "forced-colors" | "inverted-colors" => Truth::False,
        "orientation" | "prefers-color-scheme" | "display-mode" | "update" | "overflow-block" | "overflow-inline" | "color-gamut" | "dynamic-range" | "video-dynamic-range" => Truth::True,
        "scripting" => Truth::of(media.scripting),
        _ => Truth::Unknown,
    }
}
fn plain_feature(name: &str, items: &[Item<'_>], media: &Media) -> Truth {
    let lower = name.to_ascii_lowercase();
    let (prefix, base) = if let Some(b) = lower.strip_prefix("min-") {
        (Some("min"), b)
    } else if let Some(b) = lower.strip_prefix("max-") {
        (Some("max"), b)
    } else {
        (None, lower.as_str())
    };
    let (feature, current) = numeric(base, media);
    if feature != Feature::Other {
        let Some(v) = value(feature, items, media) else { return Truth::Unknown };
        return Truth::of(match prefix {
            Some("min") => current >= v,
            Some("max") => current <= v,
            _ => current == v,
        });
    }
    if prefix.is_some() {
        return Truth::Unknown;
    }
    let [item] = items else { return Truth::Unknown };
    let Some(word) = ident(item).map(str::to_ascii_lowercase) else { return Truth::Unknown };
    let word = word.as_str();
    match base {
        "orientation" => Truth::of(match word {
            "landscape" => media.width >= media.height,
            "portrait" => media.width < media.height,
            _ => return Truth::Unknown,
        }),
        "prefers-color-scheme" => Truth::of(match word {
            "dark" => media.dark,
            "light" => !media.dark,
            _ => return Truth::Unknown,
        }),
        "prefers-reduced-motion" | "prefers-reduced-transparency" | "prefers-contrast" => Truth::of(word == "no-preference"),
        "forced-colors" | "inverted-colors" => Truth::of(word == "none"),
        "hover" | "any-hover" => Truth::of(word == "hover"),
        "pointer" | "any-pointer" => Truth::of(word == "fine"),
        "update" => Truth::of(word == "fast"),
        "overflow-block" | "overflow-inline" => Truth::of(word == "scroll"),
        "display-mode" => Truth::of(word == "browser"),
        "color-gamut" => Truth::of(word == "srgb"),
        "dynamic-range" | "video-dynamic-range" => Truth::of(word == "standard"),
        "scripting" => Truth::of(if media.scripting { word == "enabled" } else { word == "none" }),
        "grid" => Truth::of(word == "0"),
        _ => Truth::Unknown,
    }
}
/// `(width >= 600px)`, `(600px < width)`, `(400px <= width <= 800px)`.
fn range_feature(items: &[Item<'_>], media: &Media) -> Truth {
    // Split on comparison operators (`<`, `>`, `=`, `<=`, `>=`).
    let mut parts: Vec<Vec<Item<'_>>> = vec![Vec::new()];
    let mut ops: Vec<String> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let c = &items[i];
        if let Some(ch @ ('<' | '>' | '=')) = c.character() {
            let mut op = ch.to_string();
            if ch != '=' && items.get(i + 1).and_then(|n| n.character()) == Some('=') {
                op.push('=');
                i += 1;
            }
            ops.push(op);
            parts.push(Vec::new());
        } else {
            parts.last_mut().unwrap().push(*c);
        }
        i += 1;
    }
    let compare = |op: &str, a: f64, b: f64| match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        _ => a == b,
    };
    match (parts.as_slice(), ops.as_slice()) {
        ([left, right], [op]) => {
            let (name, value_items, flipped) = match (left.as_slice(), right.as_slice()) {
                ([n], v) if ident(n).is_some() => (ident(n).unwrap(), v, false),
                (v, [n]) if ident(n).is_some() => (ident(n).unwrap(), v, true),
                _ => return Truth::Unknown,
            };
            let (feature, current) = numeric(name, media);
            let Some(v) = value(feature, value_items, media) else { return Truth::Unknown };
            Truth::of(if flipped { compare(op, v, current) } else { compare(op, current, v) })
        }
        ([low, middle, high], [a, b]) => {
            let [n] = middle.as_slice() else { return Truth::Unknown };
            let Some(name) = ident(n) else { return Truth::Unknown };
            let (feature, current) = numeric(name, media);
            let (Some(lo), Some(hi)) = (value(feature, low, media), value(feature, high, media)) else { return Truth::Unknown };
            Truth::of(compare(a, lo, current) && compare(b, current, hi))
        }
        _ => Truth::Unknown,
    }
}
/// `@supports` conditions: declarations the engine's grammar accepts count as
/// supported. `selector()` and font functions are unknown.
pub fn supports(condition: Components<'_>, limits: p::Limits, cancelled: Option<&AtomicBool>) -> bool {
    let items: Vec<_> = condition.filter(|c| c.kind() != Kind::Whitespace).collect();
    supports_condition(&items, limits, cancelled) == Truth::True
}
fn supports_condition(items: &[Item<'_>], limits: p::Limits, cancelled: Option<&AtomicBool>) -> Truth {
    let Some(first) = items.first() else { return Truth::False };
    if is(first, "not") {
        if items.len() != 2 {
            return Truth::False;
        }
        return supports_in_parens(&items[1], limits, cancelled).not();
    }
    let mut truth = supports_in_parens(first, limits, cancelled);
    let mut op: Option<&str> = None;
    let mut rest = &items[1..];
    while !rest.is_empty() {
        let Some(word) = ident(&rest[0]) else { return Truth::False };
        let word = if word.eq_ignore_ascii_case("and") {
            "and"
        } else if word.eq_ignore_ascii_case("or") {
            "or"
        } else {
            return Truth::False;
        };
        if op.is_some_and(|o| o != word) {
            return Truth::False;
        }
        op = Some(word);
        let Some(next) = rest.get(1) else { return Truth::False };
        let value = supports_in_parens(next, limits, cancelled);
        truth = if word == "and" { truth.and(value) } else { truth.or(value) };
        rest = &rest[2..];
    }
    truth
}
fn supports_in_parens(item: &Item<'_>, limits: p::Limits, cancelled: Option<&AtomicBool>) -> Truth {
    if item.kind() != Kind::Round {
        return Truth::Unknown;
    }
    let inner: Vec<_> = item.children().filter(|c| c.kind() != Kind::Whitespace).collect();
    let Some(first) = inner.first() else { return Truth::Unknown };
    if first.kind() == Kind::Round || is(first, "not") {
        return supports_condition(&inner, limits, cancelled);
    }
    // `( property : value )`
    if let Some(name) = ident(first)
        && inner.get(1).is_some_and(|c| c.kind() == Kind::Colon)
    {
        let values = item.children();
        // Skip to after the colon in the unfiltered children.
        let mut after_colon = values;
        for c in after_colon.by_ref() {
            if c.kind() == Kind::Colon {
                break;
            }
        }
        return Truth::of(p::Declaration::parse(name, after_colon, false, limits, cancelled).is_ok());
    }
    Truth::Unknown
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Syntax;
    use std::sync::Arc;
    fn eval(query: &str, media: &Media) -> bool {
        let syntax = Syntax::parse(Arc::from(query), Default::default(), None).unwrap();
        evaluate(syntax.components(), media, None)
    }
    #[test]
    fn widths_types_and_logic() {
        let m = Media::screen(1000., 700., 2., false);
        assert!(eval("", &m));
        assert!(eval("screen", &m));
        assert!(eval("all and (min-width: 600px)", &m));
        assert!(!eval("print", &m));
        assert!(!eval("(min-width: 1001px)", &m));
        assert!(eval("(max-width: 1000px)", &m));
        assert!(eval("(width >= 600px) and (height < 800px)", &m));
        assert!(eval("(600px <= width <= 1000px)", &m));
        assert!(!eval("not all and (min-width: 600px)", &m));
        assert!(eval("only screen and (min-width: 37.5em)", &m));
        assert!(eval("(min-resolution: 2dppx)", &m));
        assert!(eval("(-webkit-min-device-pixel-ratio: 2), (min-resolution: 192dpi)", &m));
        assert!(eval("(orientation: landscape)", &m));
        assert!(!eval("(prefers-color-scheme: dark)", &m));
        assert!(eval("(prefers-color-scheme: dark)", &Media::screen(1000., 700., 1., true)));
        assert!(eval("(hover: hover) and (pointer: fine)", &m));
        assert!(eval("(prefers-reduced-motion: no-preference)", &m));
        assert!(!eval("(prefers-reduced-motion: reduce)", &m));
        // Unknown features are false at the top level and stay unknown under not.
        assert!(!eval("(unknown-feature: 1)", &m));
        assert!(!eval("not (unknown-feature: 1)", &m));
        assert!(eval("(unknown-feature: 1), screen", &m));
        assert!(!eval("screen and (unknown-feature: 1)", &m));
        assert!(eval("(min-width: 600px) or (max-width: 100px)", &m));
        assert!(!eval("screen and (min-width: 600px) or (max-width: 100px)", &m));
    }
    #[test]
    fn supports_declarations() {
        let m = |s: &str| {
            let syntax = Syntax::parse(Arc::from(s), Default::default(), None).unwrap();
            supports(syntax.components(), Default::default(), None)
        };
        assert!(m("(display: flex)"));
        assert!(m("(display: flex) and (color: red)"));
        assert!(!m("(display: nonsense)"));
        assert!(m("not (display: nonsense)"));
        assert!(m("(display: nonsense) or (color: red)"));
        assert!(!m("selector(:has(a))"));
    }
}
