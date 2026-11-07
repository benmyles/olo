use super::*;
use registry::{Expansion, Grammar, SizeKind};
/// Bound visits/bytes before scanning substitution syntax. Nested token ranges
/// are flat and each top-level span includes all descendants.
pub(super) fn preflight(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<()> {
    let (mut nodes, mut bytes) = (0usize, 0usize);
    for c in values {
        check(cancelled)?;
        nodes = nodes.checked_add((c.after() - c.at) as usize).ok_or(Error::Limit)?;
        bytes = bytes.checked_add((c.span().end - c.span().start) as usize).ok_or(Error::Limit)?;
        if nodes > limits.nodes || bytes > limits.bytes || nodes.saturating_add(bytes) > limits.work {
            return Err(Error::Limit);
        }
    }
    Ok(())
}
pub(super) fn known(name: Name, values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    check(cancelled)?;
    preflight(values.clone(), limits, cancelled)?;
    if name == Name::Longhand(Property::FontFamily) {
        let mut parts = values.clone().filter(|c| c.kind() != Kind::Whitespace);
        if let Some(c) = parts.next()
            && parts.next().is_none()
            && c.kind() == Kind::Ident
            && let Some(wide) = wide(c.text().unwrap())
        {
            return Ok(name.wide(wide));
        }
        return font::Families::parse(values, cancelled).map(|f| smallvec::smallvec![Assignment { property: Property::FontFamily, value: Value::Families(f) }]);
    }
    let mut parts = SmallVec::<[Component<'_>; 4]>::new();
    for c in values.clone().filter(|c| c.kind() != Kind::Whitespace) {
        if parts.len() == 4 {
            return Err(Error::Invalid);
        }
        parts.push(c);
    }
    if parts.is_empty() {
        return Err(Error::Invalid);
    }
    if parts.len() == 1 && parts[0].kind() == Kind::Ident {
        let s = parts[0].text().unwrap();
        for (n, w) in [
            ("initial", variables::Wide::Initial),
            ("inherit", variables::Wide::Inherit),
            ("unset", variables::Wide::Unset),
            ("revert", variables::Wide::Revert),
            ("revert-layer", variables::Wide::RevertLayer),
            ("revert-rule", variables::Wide::RevertRule),
        ] {
            if s.eq_ignore_ascii_case(n) {
                return Ok(name.wide(w));
            }
        }
    }
    match name {
        Name::Longhand(property) => Ok(smallvec::smallvec![Assignment { property, value: longhand(property, values, &parts, limits, cancelled)? }]),
        Name::Shorthand(s) => {
            let properties = s.properties();
            let mut parsed = SmallVec::<[Value; 4]>::new();
            match s.expansion() {
                Expansion::WideOnly => Err(Error::Invalid),
                Expansion::WhiteSpace => white_space(&parts),
                Expansion::Flex => super::flex::parse(&parts, limits, cancelled),
                Expansion::Border => border(&parts, properties, limits, cancelled),
                Expansion::Quad | Expansion::Repeat => {
                    if parts.len() > properties.len() {
                        return Err(Error::Invalid);
                    }
                    // All members in these shorthands share one grammar.
                    for &c in &parts {
                        parsed.push(component(properties[0].grammar(), c, limits, cancelled)?);
                    }
                    let indices: &[usize] = match (properties.len(), parsed.len()) {
                        (4, 1) => &[0, 0, 0, 0],
                        (4, 2) => &[0, 1, 0, 1],
                        (4, 3) => &[0, 1, 2, 1],
                        (4, 4) => &[0, 1, 2, 3],
                        (2, 1) => &[0, 0],
                        (2, 2) => &[0, 1],
                        _ => return Err(Error::Invalid),
                    };
                    Ok(properties.iter().zip(indices).map(|(&property, &i)| Assignment { property, value: parsed[i].clone() }).collect())
                }
                Expansion::Unordered => {
                    if parts.len() > 2 {
                        return Err(Error::Invalid);
                    }
                    let (mut first, mut second) = (None, None);
                    for &c in &parts {
                        match component(properties[0].grammar(), c, limits, cancelled) {
                            Ok(v) if first.is_none() => first = Some(v),
                            Ok(_) => return Err(Error::Invalid),
                            Err(Error::Invalid) => {
                                let v = component(properties[1].grammar(), c, limits, cancelled)?;
                                if second.replace(v).is_some() {
                                    return Err(Error::Invalid);
                                }
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    let first = first.unwrap_or(Value::Keyword(Keyword::Row));
                    let second = second.unwrap_or(Value::Keyword(Keyword::Nowrap));
                    Ok(smallvec::smallvec![Assignment { property: properties[0], value: first }, Assignment { property: properties[1], value: second }])
                }
            }
        }
    }
}
// The registered border width/style/color longhands. Border images are not
// implemented; this must not be advertised as complete border-image reset support.
fn border(parts: &[Component<'_>], properties: &[Property], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    if parts.is_empty() || parts.len() > 3 {
        return Err(Error::Invalid);
    }
    let mut found: [Option<Value>; 3] = [None, None, None];
    for &c in parts {
        let mut matched = false;
        let mut unsupported = None;
        for (index, p) in [Property::BorderTopWidth, Property::BorderTopStyle, Property::BorderTopColor].into_iter().enumerate() {
            match component(p.grammar(), c, limits, cancelled) {
                Ok(value) => {
                    if found[index].replace(value).is_some() {
                        return Err(Error::Invalid);
                    }
                    matched = true;
                    break;
                }
                Err(Error::Invalid) => {}
                Err(error @ Error::Unsupported(_)) => {
                    unsupported.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
        if !matched {
            return Err(unsupported.unwrap_or(Error::Invalid));
        }
    }
    let [width, style, color] = found;
    let values = [width.unwrap_or(Value::Keyword(Keyword::Medium)), style.unwrap_or(Value::Keyword(Keyword::None)), color.unwrap_or(Value::Color(color::Value::CurrentColor))];
    Ok(properties.iter().enumerate().map(|(index, &property)| Assignment { property, value: values[index / 4].clone() }).collect())
}
// CSS Text 4's interoperable two-longhand shorthand. white-space-trim and
// discard are not implemented; their syntax must fail, never silently reset.
fn white_space(parts: &[Component<'_>]) -> Result<Assignments> {
    use Keyword::{BreakSpaces, Collapse, Nowrap, Preserve, PreserveBreaks, PreserveSpaces, Wrap};
    let legacy = if parts.len() == 1 && parts[0].kind() == Kind::Ident {
        let name = parts[0].text().unwrap();
        [("normal", (Collapse, Wrap)), ("pre", (Preserve, Nowrap)), ("pre-wrap", (Preserve, Wrap)), ("pre-line", (PreserveBreaks, Wrap))]
            .into_iter()
            .find(|(s, _)| name.eq_ignore_ascii_case(s))
            .map(|(_, pair)| pair)
    } else {
        None
    };
    let (collapse, wrap) = if let Some(pair) = legacy {
        pair
    } else {
        if parts.len() > 2 {
            return Err(Error::Invalid);
        }
        let (mut collapse, mut wrap) = (None, None);
        for &part in parts {
            if let Some(k) = keyword(part, &[Collapse, Preserve, PreserveBreaks, PreserveSpaces, BreakSpaces]) {
                if collapse.replace(k).is_some() {
                    return Err(Error::Invalid);
                }
            } else if let Some(k) = keyword(part, &[Wrap, Nowrap]) {
                if wrap.replace(k).is_some() {
                    return Err(Error::Invalid);
                }
            } else {
                return Err(Error::Invalid);
            }
        }
        (collapse.unwrap_or(Collapse), wrap.unwrap_or(Wrap))
    };
    Ok(smallvec::smallvec![Assignment { property: Property::WhiteSpaceCollapse, value: Value::Keyword(collapse) }, Assignment { property: Property::TextWrapMode, value: Value::Keyword(wrap) }])
}
fn longhand(property: Property, values: Components<'_>, parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let grammar = property.grammar();
    match grammar {
        Grammar::FontStyle => font::style(parts, limits, cancelled),
        Grammar::Keywords(_) if property == Property::TextAlign && parts.len() == 1 && parts[0].kind() == Kind::Ident && parts[0].text().unwrap().eq_ignore_ascii_case("-webkit-match-parent") => {
            Ok(Value::Keyword(Keyword::MatchParent))
        }
        Grammar::Display => {
            if parts.len() > 3 {
                return Err(Error::Invalid);
            }
            Ok(Value::Display(Display::parse(parts)?))
        }
        Grammar::Color(keywords) => {
            if parts.len() == 1
                && let Some(k) = keyword(parts[0], keywords)
            {
                return Ok(Value::Keyword(k));
            }
            color::Value::parse(values, limits.color(), cancelled).map(Value::Color).map_err(color_error)
        }
        _ if parts.len() == 1 => component(grammar, parts[0], limits, cancelled),
        _ => Err(Error::Invalid),
    }
}
pub(super) fn component(grammar: Grammar, c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    check(cancelled)?;
    match grammar {
        Grammar::FontWidth => font::width(c, limits, cancelled),
        Grammar::Overflow => {
            if c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("overlay") {
                return Ok(Value::Keyword(Keyword::Auto));
            }
            keyword(c, &[Keyword::Visible, Keyword::Hidden, Keyword::Clip, Keyword::Scroll, Keyword::Auto]).map(Value::Keyword).ok_or(Error::Invalid)
        }
        Grammar::Keywords(allowed) => keyword(c, allowed).map(Value::Keyword).ok_or(Error::Invalid),
        Grammar::Numeric(kind, range, allowed) => {
            if let Some(k) = keyword(c, allowed) {
                return Ok(Value::Keyword(k));
            }
            unsupported_function(c)?;
            Numeric::compile(c, kind, range, limits, cancelled).map(Value::Numeric)
        }
        Grammar::Color(allowed) => {
            if let Some(k) = keyword(c, allowed) {
                return Ok(Value::Keyword(k));
            }
            // between() preserves this component and excludes its neighbours.
            let values = c.children().between(c.at, c.after());
            color::Value::parse(values, limits.color(), cancelled).map(Value::Color).map_err(color_error)
        }
        Grammar::Size(kind) => {
            let allowed = &[Keyword::MinContent, Keyword::MaxContent, Keyword::Stretch, Keyword::FitContent];
            if let Some(k) = keyword(c, allowed) {
                return Ok(Value::Keyword(k));
            }
            let k = keyword(c, &[Keyword::Auto, Keyword::None, Keyword::Content]);
            if let Some(k) = k {
                if matches!((kind, k), (SizeKind::Preferred | SizeKind::Minimum, Keyword::Auto) | (SizeKind::Maximum, Keyword::None) | (SizeKind::Basis, Keyword::Auto | Keyword::Content)) {
                    return Ok(Value::Keyword(k));
                }
                return Err(Error::Invalid);
            }
            if c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("contain") {
                return Err(Error::Unsupported("contain sizing"));
            }
            if c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("fit-content") {
                let mut parts = c.children().filter(|c| c.kind() != Kind::Whitespace);
                let part = parts.next().ok_or(Error::Invalid)?;
                if parts.next().is_some() {
                    return Err(Error::Invalid);
                }
                return Numeric::compile(part, NumericKind::LengthPercentage, Range::Nonnegative, limits, cancelled).map(Value::FitContent);
            }
            unsupported_function(c)?;
            Numeric::compile(c, NumericKind::LengthPercentage, Range::Nonnegative, limits, cancelled).map(Value::Numeric)
        }
        Grammar::Display | Grammar::FontFamily | Grammar::FontStyle => Err(Error::Invalid),
    }
}
fn wide(s: &str) -> Option<variables::Wide> {
    [
        ("initial", variables::Wide::Initial),
        ("inherit", variables::Wide::Inherit),
        ("unset", variables::Wide::Unset),
        ("revert", variables::Wide::Revert),
        ("revert-layer", variables::Wide::RevertLayer),
        ("revert-rule", variables::Wide::RevertRule),
    ]
    .into_iter()
    .find(|(name, _)| s.eq_ignore_ascii_case(name))
    .map(|(_, wide)| wide)
}
fn keyword(c: Component<'_>, allowed: &[Keyword]) -> Option<Keyword> {
    (c.kind() == Kind::Ident).then(|| allowed.iter().copied().find(|k| c.text().unwrap().eq_ignore_ascii_case(k.as_str()))).flatten()
}
fn unsupported_function(c: Component<'_>) -> Result<()> {
    if c.kind() == Kind::Function && ["anchor", "anchor-size", "calc-size"].iter().any(|s| c.text().unwrap().eq_ignore_ascii_case(s)) {
        Err(Error::Unsupported("anchor or intrinsic sizing function"))
    } else {
        Ok(())
    }
}
fn color_error(e: color::Error) -> Error {
    match e {
        color::Error::Invalid => Error::Invalid,
        color::Error::Unsupported(s) => Error::Unsupported(s),
        color::Error::RequiresSubstitution => Error::Unsupported("color substitution"),
        color::Error::Limit => Error::Limit,
        color::Error::DepthLimit => Error::DepthLimit,
        color::Error::Cancelled => Error::Cancelled,
        color::Error::ContextUnavailable => Error::Unsupported("color parse context"),
    }
}
