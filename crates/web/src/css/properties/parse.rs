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
    // Track lists and the grid shorthands take any number of components.
    if let Name::Longhand(property @ (Property::GridTemplateColumns | Property::GridTemplateRows | Property::GridAutoColumns | Property::GridAutoRows)) = name {
        let mut parts = values.clone().filter(|c| c.kind() != Kind::Whitespace);
        if let Some(c) = parts.next()
            && parts.next().is_none()
            && c.kind() == Kind::Ident
            && let Some(wide) = wide(c.text().unwrap())
        {
            return Ok(name.wide(wide));
        }
        return grid::parse_tracks(values, limits, cancelled).map(|value| smallvec::smallvec![Assignment { property, value }]);
    }
    // Decoration values (image lists, shadows, radii) take any number of components.
    if let Name::Longhand(property) = name
        && matches!(
            property.grammar(),
            Grammar::Images | Grammar::Sizes | Grammar::Positions | Grammar::Repeats | Grammar::Shadows | Grammar::Radius | Grammar::Transforms | Grammar::Features | Grammar::Ratio | Grammar::Content
        )
    {
        let parts: Vec<Component<'_>> = values.clone().filter(|c| c.kind() != Kind::Whitespace).collect();
        if let [c] = parts.as_slice()
            && c.kind() == Kind::Ident
            && let Some(wide) = wide(c.text().unwrap())
        {
            return Ok(name.wide(wide));
        }
        if parts.is_empty() {
            return Err(Error::Invalid);
        }
        let value = match property.grammar() {
            Grammar::Images => decoration::parse_images(&parts, limits, cancelled)?,
            Grammar::Sizes => decoration::parse_sizes(&parts, limits, cancelled)?,
            Grammar::Positions => decoration::parse_positions(&parts, limits, cancelled)?,
            Grammar::Repeats => decoration::parse_repeats(&parts)?,
            Grammar::Shadows => decoration::parse_shadows(&parts, limits, cancelled)?,
            Grammar::Transforms => transform::parse_transforms(&parts, limits, cancelled)?,
            Grammar::Features => font::parse_features(&parts)?,
            Grammar::Ratio => ratio(&parts)?,
            Grammar::Content => content::parse_content(&parts)?,
            _ => decoration::parse_radius(&parts, limits, cancelled)?,
        };
        return Ok(smallvec::smallvec![Assignment { property, value }]);
    }
    if let Name::Shorthand(shorthand) = name
        && matches!(shorthand.expansion(), Expansion::Radius | Expansion::Background)
    {
        let parts: Vec<Component<'_>> = values.clone().filter(|c| c.kind() != Kind::Whitespace).collect();
        if let [c] = parts.as_slice()
            && c.kind() == Kind::Ident
            && let Some(wide) = wide(c.text().unwrap())
        {
            return Ok(name.wide(wide));
        }
        if parts.is_empty() {
            return Err(Error::Invalid);
        }
        return match shorthand.expansion() {
            Expansion::Radius => decoration::parse_radius_shorthand(&parts, shorthand.properties(), limits, cancelled),
            _ => decoration::parse_background(&parts, shorthand.properties(), limits, cancelled),
        };
    }
    if let Name::Shorthand(shorthand) = name
        && matches!(shorthand.expansion(), Expansion::Grid)
    {
        let mut parts = values.clone().filter(|c| c.kind() != Kind::Whitespace);
        if let Some(c) = parts.next()
            && parts.next().is_none()
            && c.kind() == Kind::Ident
            && let Some(wide) = wide(c.text().unwrap())
        {
            return Ok(name.wide(wide));
        }
        return grid::parse_shorthand(shorthand, values, limits, cancelled);
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
                Expansion::TextWrap => text_wrap(&parts),
                Expansion::TextDecoration => text_decoration(&parts, limits, cancelled),
                Expansion::ListStyle => list_style(&parts, limits, cancelled),
                Expansion::Flex => super::flex::parse(&parts, limits, cancelled),
                Expansion::Grid | Expansion::Radius | Expansion::Background => unreachable!("these shorthands parse before part collection"),
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
    // The longhands come in width, style, colour groups of one side each
    // (`border-top`), two (`border-block`) or four (`border`).
    let sides = properties.len() / 3;
    Ok(properties.iter().enumerate().map(|(index, &property)| Assignment { property, value: values[index / sides].clone() }).collect())
}
/// `aspect-ratio: auto || <ratio>` (CSS Sizing 4 §5): the ratio as one
/// number, width over height; `auto` alone is the keyword. A replaced
/// element's natural ratio wins over the number either way here.
fn ratio(parts: &[Component<'_>]) -> Result<Value> {
    let mut auto = false;
    let mut numbers = Vec::new();
    let mut slash = false;
    for c in parts {
        if c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("auto") {
            if auto {
                return Err(Error::Invalid);
            }
            auto = true;
        } else if c.kind() == Kind::Delim && c.character() == Some('/') {
            if numbers.len() != 1 || slash {
                return Err(Error::Invalid);
            }
            slash = true;
        } else if matches!(c.kind(), Kind::Number | Kind::Integer) {
            let v = c.number().map(|n| n.value).ok_or(Error::Invalid)?;
            if !(v.is_finite() && v >= 0.) || numbers.len() >= 2 || (numbers.len() == 1 && !slash) {
                return Err(Error::Invalid);
            }
            numbers.push(v);
        } else {
            return Err(Error::Invalid);
        }
    }
    if slash && numbers.len() != 2 {
        return Err(Error::Invalid);
    }
    match numbers.as_slice() {
        [] if auto => Ok(Value::Keyword(Keyword::Auto)),
        [w] if *w > 0. => Ok(Value::Numeric(Numeric::scalar(*w, math::Dimension::Number, Range::Nonnegative))),
        [w, h] if *w > 0. && *h > 0. => Ok(Value::Numeric(Numeric::scalar(w / h, math::Dimension::Number, Range::Nonnegative))),
        // A degenerate ratio behaves as auto.
        [_] | [_, _] => Ok(Value::Keyword(Keyword::Auto)),
        _ => Err(Error::Invalid),
    }
}
/// `list-style: <type> || <position> || <image>` (CSS Lists 3 §3.4); one
/// `none` is the image and the type, two are both.
fn list_style(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    if parts.is_empty() || parts.len() > 3 {
        return Err(Error::Invalid);
    }
    let (mut kind, mut position, mut image, mut nones) = (None, None, None, 0);
    for c in parts {
        if c.kind() == Kind::Ident {
            let name = c.text().unwrap();
            if name.eq_ignore_ascii_case("none") {
                nones += 1;
                continue;
            }
            if let Some((_, k)) = [("inside", Keyword::Inside), ("outside", Keyword::Outside)].into_iter().find(|(s, _)| name.eq_ignore_ascii_case(s)) {
                if position.replace(k).is_some() {
                    return Err(Error::Invalid);
                }
                continue;
            }
            if let Ok(Value::Keyword(k)) = component(Property::ListStyleType.grammar(), *c, limits, cancelled) {
                if kind.replace(k).is_some() {
                    return Err(Error::Invalid);
                }
                continue;
            }
            return Err(Error::Invalid);
        }
        match decoration::parse_images(&[*c], limits, cancelled) {
            Ok(v) if image.is_none() => image = Some(v),
            _ => return Err(Error::Invalid),
        }
    }
    if nones > 2 || (nones == 2 && (kind.is_some() || image.is_some())) {
        return Err(Error::Invalid);
    }
    let kind = kind.unwrap_or(if nones > 0 { Keyword::None } else { Keyword::Disc });
    let image = image.unwrap_or(Value::Keyword(Keyword::None));
    Ok(smallvec::smallvec![
        Assignment { property: Property::ListStyleType, value: Value::Keyword(kind) },
        Assignment { property: Property::ListStylePosition, value: Value::Keyword(position.unwrap_or(Keyword::Outside)) },
        Assignment { property: Property::ListStyleImage, value: image },
    ])
}
/// `text-decoration: <line>+ || <style> || <color>` (CSS Text Decoration 3
/// §2.4); a thickness is not supported.
fn text_decoration(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    if parts.is_empty() || parts.len() > 4 {
        return Err(Error::Invalid);
    }
    let (mut line, mut style, mut color) = (None, None, None);
    for c in parts {
        if c.kind() == Kind::Ident {
            let name = c.text().unwrap();
            if let Some((_, k)) = [("none", Keyword::None), ("underline", Keyword::Underline), ("overline", Keyword::Overline), ("line-through", Keyword::LineThrough)]
                .into_iter()
                .find(|(s, _)| name.eq_ignore_ascii_case(s))
            {
                // Several lines combine; the first kept decides the paint.
                if line.is_none() || line == Some(Keyword::None) {
                    line = Some(k);
                }
                continue;
            }
            if let Some((_, k)) = [("solid", Keyword::Solid), ("double", Keyword::Double), ("dotted", Keyword::Dotted), ("dashed", Keyword::Dashed), ("wavy", Keyword::Wavy)]
                .into_iter()
                .find(|(s, _)| name.eq_ignore_ascii_case(s))
            {
                if style.replace(k).is_some() {
                    return Err(Error::Invalid);
                }
                continue;
            }
        }
        if color.is_some() {
            return Err(Error::Invalid);
        }
        match component(Grammar::Color(&[]), *c, limits, cancelled) {
            Ok(v) => color = Some(v),
            Err(Error::Invalid) => return Err(Error::Invalid),
            Err(e) => return Err(e),
        }
    }
    Ok(smallvec::smallvec![
        Assignment { property: Property::TextDecorationLine, value: Value::Keyword(line.unwrap_or(Keyword::None)) },
        Assignment { property: Property::TextDecorationStyle, value: Value::Keyword(style.unwrap_or(Keyword::Solid)) },
        Assignment { property: Property::TextDecorationColor, value: color.unwrap_or(Value::Color(color::Value::CurrentColor)) },
    ])
}
/// `text-wrap: <'text-wrap-mode'> || <'text-wrap-style'>` (CSS Text 4 §5.1).
fn text_wrap(parts: &[Component<'_>]) -> Result<Assignments> {
    if parts.is_empty() || parts.len() > 2 {
        return Err(Error::Invalid);
    }
    let (mut mode, mut style) = (None, None);
    for c in parts {
        if c.kind() != Kind::Ident {
            return Err(Error::Invalid);
        }
        let name = c.text().unwrap();
        let k = [("wrap", Keyword::Wrap), ("nowrap", Keyword::Nowrap), ("auto", Keyword::Auto), ("balance", Keyword::Balance), ("stable", Keyword::Stable), ("pretty", Keyword::Pretty)]
            .into_iter()
            .find(|(s, _)| name.eq_ignore_ascii_case(s))
            .map(|(_, k)| k)
            .ok_or(Error::Invalid)?;
        let slot = if matches!(k, Keyword::Wrap | Keyword::Nowrap) { &mut mode } else { &mut style };
        if slot.replace(k).is_some() {
            return Err(Error::Invalid);
        }
    }
    Ok(smallvec::smallvec![
        Assignment { property: Property::TextWrapMode, value: Value::Keyword(mode.unwrap_or(Keyword::Wrap)) },
        Assignment { property: Property::TextWrapStyle, value: Value::Keyword(style.unwrap_or(Keyword::Auto)) },
    ])
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
    if matches!(property, Property::Fill | Property::Stroke)
        && parts.iter().any(|c| {
            matches!(c.kind(), Kind::Url | Kind::UnclosedUrl)
                || c.kind() == Kind::Function && c.text().is_some_and(|s| s.eq_ignore_ascii_case("url"))
                || c.kind() == Kind::Ident && c.text().is_some_and(|s| s.eq_ignore_ascii_case("context-fill") || s.eq_ignore_ascii_case("context-stroke"))
        })
    {
        return Err(Error::Unsupported("SVG paint server/context paint"));
    }
    match grammar {
        Grammar::ClipPath => super::clip::parse(parts, limits, cancelled),
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
        Grammar::GridLine => grid::parse_line(parts),
        Grammar::GridAutoFlow => {
            let (mut flow, mut dense) = (None, false);
            if parts.len() > 2 {
                return Err(Error::Invalid);
            }
            for &c in parts {
                if let Some(k) = keyword(c, &[Keyword::Row, Keyword::Column]) {
                    if flow.replace(k).is_some() {
                        return Err(Error::Invalid);
                    }
                } else if keyword(c, &[Keyword::Dense]).is_some() && !dense {
                    dense = true;
                } else {
                    return Err(Error::Invalid);
                }
            }
            Ok(Value::Keyword(match (flow, dense) {
                (Some(Keyword::Column), false) => Keyword::Column,
                (Some(Keyword::Column), true) => Keyword::ColumnDense,
                (_, true) => Keyword::RowDense,
                _ => Keyword::Row,
            }))
        }
        Grammar::Tracks
        | Grammar::Images
        | Grammar::Sizes
        | Grammar::Positions
        | Grammar::Repeats
        | Grammar::Shadows
        | Grammar::Radius
        | Grammar::Transforms
        | Grammar::Features
        | Grammar::Ratio
        | Grammar::Content => {
            unreachable!("parsed before part collection")
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
        Grammar::ClipPath => super::clip::parse(&[c], limits, cancelled),
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
        Grammar::Display | Grammar::FontFamily | Grammar::FontStyle | Grammar::Tracks | Grammar::GridLine | Grammar::GridAutoFlow => Err(Error::Invalid),
        Grammar::Images | Grammar::Sizes | Grammar::Positions | Grammar::Repeats | Grammar::Shadows | Grammar::Radius | Grammar::Transforms | Grammar::Features | Grammar::Ratio | Grammar::Content => {
            Err(Error::Invalid)
        }
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
pub(super) fn color_error(e: color::Error) -> Error {
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
