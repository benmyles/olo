use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
pub(super) struct Computed {
    pub values: Vec<Entry>,
    pub customs: Arc<Dictionary>,
    pub font: Arc<Font>,
    pub axes: Axes,
    pub line_height: f64,
    pub dependencies: Dependencies,
}
enum Source<'a> {
    Inherited(&'a Value),
    Specified(p::Value),
}
#[allow(clippy::too_many_arguments)]
fn source<'a>(
    defaults: &Defaults,
    resolver: &mut cascade::Resolver<'_>,
    property: Property,
    axes: Axes,
    parent: Option<&'a Style>,
    options: Options,
    dependencies: &mut Dependencies,
) -> Result<Source<'a>> {
    let result = resolver.projected(property, axes, options.allow_animation_tainted).map_err(Error::cascade)?;
    let specified = match result.value {
        cascade::Defaulted::Inherit => {
            dependencies.inherited = true;
            if let Some(parent) = parent {
                return Ok(Source::Inherited(parent.value(result.source)));
            }
            defaults.specified[property as usize].clone()
        }
        cascade::Defaulted::Initial => defaults.specified[property as usize].clone(),
        cascade::Defaulted::Specified(value) => value,
    };
    record(&specified, dependencies);
    Ok(Source::Specified(specified))
}
fn record(specified: &p::Value, dependencies: &mut Dependencies) {
    let d = specified.dependencies();
    dependencies.substitution |= d.substitution;
    dependencies.color.current_color |= d.color.current_color;
    dependencies.color.system_colors |= d.color.system_colors;
    dependencies.color.scheme |= d.color.scheme;
    dependencies.color.math = dependencies.color.math.union(d.color.math);
}
fn bounded(value: f64) -> Result<f64> {
    if !value.is_finite() || value.is_sign_negative() { Err(Error::ContextUnavailable) } else { Ok(value) }
}
fn scalar(value: f64, dimension: math::Dimension, range: p::Range) -> Value {
    Value::Numeric(p::Numeric::scalar(value, dimension, range))
}
fn font_size(source: Source<'_>, host: &impl Host, numeric: &impl math::Context, parent: f64, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<f64> {
    let value = match source {
        Source::Inherited(value) => value.scalar().ok_or(Error::Invalid)?,
        Source::Specified(p::Value::Numeric(n)) => n.resolve(numeric, &mut Default::default(), limits.math, cancelled).map_err(Error::property)?,
        Source::Specified(p::Value::Keyword(k)) => host.font_size(k, parent)?,
        _ => return Err(Error::Invalid),
    };
    bounded(value)
}
fn font_weight(source: Source<'_>, numeric: &impl math::Context, parent: f64, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<f64> {
    let value = match source {
        Source::Inherited(value) => value.scalar().ok_or(Error::Invalid)?,
        Source::Specified(p::Value::Numeric(n)) => n.resolve(numeric, &mut Default::default(), limits.math, cancelled).map_err(Error::property)?,
        Source::Specified(p::Value::Keyword(Keyword::Normal)) => 400.,
        Source::Specified(p::Value::Keyword(Keyword::Bold)) => 700.,
        Source::Specified(p::Value::Keyword(Keyword::Bolder)) => {
            if parent < 350. {
                400.
            } else if parent < 550. {
                700.
            } else if parent < 900. {
                900.
            } else {
                parent
            }
        }
        Source::Specified(p::Value::Keyword(Keyword::Lighter)) => {
            if parent < 100. {
                parent
            } else if parent < 550. {
                100.
            } else if parent < 750. {
                400.
            } else {
                700.
            }
        }
        _ => return Err(Error::Invalid),
    };
    if !(1. ..=1000.).contains(&value) {
        return Err(Error::Invalid);
    }
    Ok(value)
}
fn border_width(property: Property) -> bool {
    matches!(property, Property::BorderTopWidth | Property::BorderRightWidth | Property::BorderBottomWidth | Property::BorderLeftWidth)
}
fn snap_border(value: f64, scale: f64) -> Result<f64> {
    if !scale.is_finite() || scale <= 0. {
        return Err(Error::ContextUnavailable);
    }
    if value == 0. {
        return Ok(0.);
    }
    let pixels = value * scale;
    Ok(if !pixels.is_finite() {
        value
    } else if pixels < 1. {
        (1. / scale).min(f64::MAX)
    } else {
        (pixels.floor() / scale).min(f64::MAX)
    })
}
fn ordinary(source: Source<'_>, property: Property, numeric: context::Numeric<'_, impl Host>, parent: Option<&Style>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let Source::Specified(value) = source else {
        let Source::Inherited(value) = source else { unreachable!() };
        // An inherited container-relative length keeps its expression; the
        // inheritor's layout resolves it against its own container.
        return Ok(value.clone());
    };
    Ok(match value {
        p::Value::Numeric(n) if property == Property::FontStyle => {
            let angle = n.resolve(&numeric, &mut Default::default(), limits.math, cancelled).map_err(Error::property)?;
            if angle == 0. { Value::Keyword(Keyword::Normal) } else { Value::Oblique(angle) }
        }
        p::Value::Numeric(n) => {
            let defer_percentage = !matches!(property, Property::LineHeight | Property::FontWidth);
            let computed = if matches!(
                property,
                Property::Width
                    | Property::FlexBasis
                    | Property::Height
                    | Property::MinWidth
                    | Property::MaxWidth
                    | Property::MinHeight
                    | Property::MaxHeight
                    | Property::MarginTop
                    | Property::MarginRight
                    | Property::MarginBottom
                    | Property::MarginLeft
                    | Property::PaddingTop
                    | Property::PaddingRight
                    | Property::PaddingBottom
                    | Property::PaddingLeft
                    | Property::Top
                    | Property::Right
                    | Property::Bottom
                    | Property::Left
            ) && n.expression().dependencies().intersects(math::Dependencies::CONTAINERS)
            {
                n.computed_retaining(&numeric, math::Dependencies::CONTAINERS.union(if defer_percentage { math::Dependencies::PERCENTAGE } else { Default::default() }), limits.math, cancelled)
            } else {
                n.computed(&numeric, defer_percentage, limits.math, cancelled)
            }
            .map_err(Error::property)?;
            if border_width(property) {
                scalar(snap_border(computed.expression().constant().ok_or(Error::Invalid)?.value, numeric.host.device_scale())?, math::Dimension::Length, p::Range::Nonnegative)
            } else {
                Value::Numeric(computed)
            }
        }
        p::Value::FitContent(n) => Value::FitContent(n.computed(&numeric, true, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Color(color) => Value::Color(color.computed(&context::Color { numeric }, limits.color, cancelled).map_err(Error::color)?),
        p::Value::Display(display) => Value::Display(display),
        p::Value::Families(families) => Value::Families(families),
        p::Value::LocalUrl(url) => Value::LocalUrl(url),
        p::Value::Inset(shape) => Value::Inset(shape.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Tracks(tracks) => Value::Tracks(tracks.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::GridLine(line) => Value::GridLine(line),
        p::Value::Radius(r) => Value::Radius(r.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Sizes(s) => Value::Sizes(s.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Positions(p) => Value::Positions(p.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Repeats(r) => Value::Repeats(r),
        p::Value::Features(f) => Value::Features(f),
        p::Value::Content(c) => Value::Content(c),
        p::Value::Animation(a) => Value::Animation(a),
        p::Value::Transforms(t) => Value::Transforms(t.computed(&numeric, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Images(images) => {
            let colors = context::Color { numeric };
            Value::Images(images.computed(&colors, &colors.numeric, limits.math, limits.color, cancelled).map_err(Error::property)?)
        }
        p::Value::Shadows(shadows) => {
            let colors = context::Color { numeric };
            Value::Shadows(shadows.computed(&colors, &colors.numeric, limits.math, limits.color, cancelled).map_err(Error::property)?)
        }
        p::Value::Keyword(k @ (Keyword::Thin | Keyword::Medium | Keyword::Thick))
            if matches!(property, Property::BorderTopWidth | Property::BorderRightWidth | Property::BorderBottomWidth | Property::BorderLeftWidth) =>
        {
            scalar(
                snap_border(
                    match k {
                        Keyword::Thin => 1.,
                        Keyword::Medium => 3.,
                        _ => 5.,
                    },
                    numeric.host.device_scale(),
                )?,
                math::Dimension::Length,
                p::Range::Nonnegative,
            )
        }
        p::Value::Keyword(Keyword::Normal) if matches!(property, Property::WordSpacing | Property::LetterSpacing) => scalar(0., math::Dimension::Length, p::Range::All),
        p::Value::Keyword(Keyword::MatchParent) if property == Property::TextAlign => {
            let keyword = if let Some(parent) = parent {
                let Value::Keyword(align) = parent.value(property) else { return Err(Error::Invalid) };
                match align {
                    Keyword::Start => {
                        if parent.axes.direction == Direction::Ltr {
                            Keyword::Left
                        } else {
                            Keyword::Right
                        }
                    }
                    Keyword::End => {
                        if parent.axes.direction == Direction::Ltr {
                            Keyword::Right
                        } else {
                            Keyword::Left
                        }
                    }
                    k => *k,
                }
            } else {
                Keyword::Start
            };
            Value::Keyword(keyword)
        }
        p::Value::Keyword(k) => Value::Keyword(k),
        _ => return Err(Error::Invalid),
    })
}
fn customs(
    defaults: &Defaults,
    cascade: &cascade::Cascade<'_>,
    resolver: &mut cascade::Resolver<'_>,
    parent: Option<&Style>,
    limits: Limits,
    cancelled: Option<&AtomicBool>,
) -> Result<Arc<Dictionary>> {
    let inherited = parent.map_or_else(|| defaults.empty_customs.clone(), |p| p.customs.clone());
    if inherited.len > limits.custom_properties {
        return Err(Error::Limit);
    }
    let mut local = cascade.custom_names().peekable();
    if local.peek().is_none() {
        return Ok(inherited);
    }
    // What this element declares, over its parent's dictionary.
    let mut values = Vec::new();
    for name in local {
        check(cancelled)?;
        if inherited.len + values.len() >= limits.custom_properties || values.len() >= limits.work {
            return Err(Error::Limit);
        }
        values.push(Custom { name: Arc::from(name), value: resolver.custom(name).map_err(Error::cascade)? });
    }
    values.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    COUNTS[0].fetch_add(1, Ordering::Relaxed);
    COUNTS[1].fetch_add(values.len() as u64, Ordering::Relaxed);
    let mut bytes = values.len() * size_of::<Custom>() + size_of::<Dictionary>() + 2 * size_of::<usize>();
    let mut arenas = std::collections::HashSet::new();
    for value in &values {
        check(cancelled)?;
        bytes = bytes.saturating_add(value.name.len() + 2 * size_of::<usize>());
        if let Some(tokens) = value.value.value()
            && arenas.insert(tokens.arena_identity())
        {
            bytes = bytes.saturating_add(tokens.arena_reserved_bytes());
        }
        if bytes > limits.retained_bytes {
            return Err(Error::Limit);
        }
    }
    let len = inherited.len + values.len();
    let parent = if inherited.values.is_empty() && inherited.parent.is_none() { None } else { Some(inherited) };
    Ok(Arc::new(Dictionary { parent, values: values.into_boxed_slice(), bytes, len }))
}
#[allow(clippy::too_many_arguments)]
/// Nanoseconds in each part of computing, summed when OLO_STYLE_TIMING
/// is set (the style job prints them): resolver and axes, the font
/// properties, the declared properties, custom properties, coupling.
pub(super) static TIMING: AtomicBool = AtomicBool::new(false);
pub(super) static PHASES: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
/// Elements merging custom properties, and the inherited entries they copied.
pub(super) static COUNTS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
fn stamp(on: bool) -> Option<std::time::Instant> {
    on.then(std::time::Instant::now)
}
fn lap(clock: &mut Option<std::time::Instant>, phase: usize) {
    if let Some(c) = clock {
        let now = std::time::Instant::now();
        PHASES[phase].fetch_add((now - *c).as_nanos() as u64, Ordering::Relaxed);
        *c = now;
    }
}
pub(super) fn values(
    defaults: &Defaults,
    cascade: &cascade::Cascade<'_>,
    parent: Option<&Style>,
    root: Option<&Style>,
    fonts: Option<&mut font_pool::FontPool>,
    host: &impl Host,
    options: Options,
    limits: Limits,
    cancelled: Option<&AtomicBool>,
) -> Result<Computed> {
    check(cancelled)?;
    if Property::ALL.len() > limits.properties {
        return Err(Error::Limit);
    }
    let mut clock = stamp(TIMING.load(Ordering::Relaxed));
    let mut resolver = cascade.resolver_with_environment(defaults.environment.as_deref());
    let axes = resolver.axes(parent.map(|p| p.axes), options.allow_animation_tainted).map_err(Error::cascade)?;
    let mut declared = [false; Property::ALL.len()];
    if !cfg!(feature = "baseline-style-dense") {
        for property in cascade.declared_properties() {
            declared[axes.physical(property) as usize] = true;
        }
    }
    lap(&mut clock, 0);
    let use_baseline = !defaults.values.is_empty() && !cfg!(feature = "baseline-style-dense");
    let parent_font = parent.map_or(defaults.initial_font.as_ref(), |p| p.font.as_ref());
    let mut dependencies = Dependencies { layout_parent: options.layout_parent.is_some(), ..Default::default() };
    // Font values that cannot be computed in this context (a container unit
    // before layout) are dropped as invalid, counted, and inherit.
    let fatal = |e: &Error| matches!(e, Error::Cancelled | Error::Limit | Error::Stale | Error::Dom(_));
    let numeric = context::numeric(host, Property::FontSize, defaults, parent, root, parent_font, parent.map_or(parent_font.metrics.normal_line_height, |p| p.line_height), axes);
    let size =
        match source(defaults, &mut resolver, Property::FontSize, axes, parent, options, &mut dependencies).and_then(|fs| font_size(fs, host, &numeric, parent_font.request.size, limits, cancelled)) {
            Ok(size) => size,
            Err(e) if fatal(&e) => return Err(e),
            Err(_) => {
                dependencies.fallbacks = dependencies.fallbacks.saturating_add(1);
                dependencies.inherited = true;
                parent_font.request.size
            }
        };
    let numeric = context::numeric(host, Property::FontWeight, defaults, parent, root, parent_font, parent.map_or(parent_font.metrics.normal_line_height, |p| p.line_height), axes);
    let weight =
        match source(defaults, &mut resolver, Property::FontWeight, axes, parent, options, &mut dependencies).and_then(|fw| font_weight(fw, &numeric, parent_font.request.weight, limits, cancelled)) {
            Ok(weight) => weight,
            Err(e) if fatal(&e) => return Err(e),
            Err(_) => {
                dependencies.fallbacks = dependencies.fallbacks.saturating_add(1);
                dependencies.inherited = true;
                parent_font.request.weight
            }
        };
    let mut font_value = |property| -> Result<Entry> {
        if use_baseline && !declared[property as usize] {
            let value = if let Some(parent) = parent {
                dependencies.inherited = true;
                parent.value(property)
            } else {
                defaults.value(property)
            };
            return Ok(Entry { property, value: value.clone() });
        }
        let value = source(defaults, &mut resolver, property, axes, parent, options, &mut dependencies)?;
        let value = ordinary(
            value,
            property,
            context::numeric(host, property, defaults, parent, root, parent_font, parent.map_or(parent_font.metrics.normal_line_height, |p| p.line_height), axes),
            parent,
            limits,
            cancelled,
        )?;
        Ok(Entry { property, value })
    };
    let font_values = [font_value(Property::FontFamily)?, font_value(Property::FontStyle)?, font_value(Property::FontWidth)?];
    let Value::Families(families) = &font_values[0].value else { return Err(Error::Invalid) };
    let slant = match font_values[1].value {
        Value::Keyword(Keyword::Normal) => FontSlant::Normal,
        Value::Keyword(Keyword::Italic) => FontSlant::Italic,
        Value::Keyword(Keyword::Left) => FontSlant::Left,
        Value::Keyword(Keyword::Right) => FontSlant::Right,
        Value::Keyword(Keyword::Oblique) => FontSlant::Oblique(14.),
        Value::Oblique(angle) => FontSlant::Oblique(angle),
        _ => return Err(Error::Invalid),
    };
    let request = FontRequest { size, weight, width: font_values[2].value.scalar().ok_or(Error::Invalid)?, slant, families: families.clone(), axes };
    let font = if parent.is_some_and(|p| p.font.request == request && p.font.context == host.font_context()) {
        parent.unwrap().font.clone()
    } else if defaults.initial_font.request == request && defaults.initial_font.context == host.font_context() {
        defaults.initial_font.clone()
    } else if let Some(fonts) = fonts {
        fonts.resolve(host, request)?
    } else {
        Arc::new(Font::new(host, request)?)
    };
    let numeric = context::numeric(host, Property::LineHeight, defaults, parent, root, &font, font.metrics.normal_line_height, axes);
    let line =
        match source(defaults, &mut resolver, Property::LineHeight, axes, parent, options, &mut dependencies).and_then(|lh| ordinary(lh, Property::LineHeight, numeric, parent, limits, cancelled)) {
            Ok(line) => line,
            Err(e) if fatal(&e) => return Err(e),
            Err(_) => {
                dependencies.fallbacks = dependencies.fallbacks.saturating_add(1);
                if let Some(parent) = parent {
                    dependencies.inherited = true;
                    parent.value(Property::LineHeight).clone()
                } else {
                    defaults.value(Property::LineHeight).clone()
                }
            }
        };
    let line_height = match &line {
        Value::Keyword(Keyword::Normal) => font.metrics.normal_line_height,
        Value::Numeric(n) => {
            let q = n.expression().constant().ok_or(Error::Invalid)?;
            let number = if q.ty.kind() == Some(math::Dimension::Number) { q.value * size } else { q.value };
            math::Quantity { value: number, ty: math::NumericType::dimension(math::Dimension::Length) }.materialize(0., f64::MAX).value
        }
        _ => return Err(Error::Invalid),
    };
    lap(&mut clock, 1);
    // Sparse computation: a style stores only what differs from the
    // defaults, so an element's values are its parent's inherited entries
    // (computed already) less what it declares, the font properties, and
    // what it declares, computed. Nothing else is visited: an element's
    // cost follows its declarations, not the property count.
    let sparse = use_baseline;
    let mut values = Vec::with_capacity(if sparse { 8 + parent.map_or(0, |p| p.values.len()) + cascade.declared_properties().count() } else { Property::ALL.len() });
    let mut work = 0usize;
    let is_font = |p: Property| matches!(p, Property::FontSize | Property::FontWeight | Property::LineHeight | Property::FontFamily | Property::FontStyle | Property::FontWidth);
    if sparse {
        if let Some(parent) = parent {
            dependencies.inherited = true;
            for e in parent.values.iter() {
                if e.property.inherited() && !declared[e.property as usize] && !is_font(e.property) {
                    values.push(Entry { property: e.property, value: e.value.clone() });
                }
            }
        }
        dependencies.substitution |= defaults.dependencies.substitution;
        dependencies.color.current_color |= defaults.dependencies.color.current_color;
        dependencies.color.system_colors |= defaults.dependencies.color.system_colors;
        dependencies.color.scheme |= defaults.dependencies.color.scheme;
        dependencies.color.math = dependencies.color.math.union(defaults.dependencies.color.math);
    }
    for &property in Property::ALL {
        if sparse && !declared[property as usize] && !is_font(property) {
            continue;
        }
        check(cancelled)?;
        work += 1;
        if work > limits.work {
            return Err(Error::Limit);
        }
        if axes.physical(property) != property {
            continue;
        }
        let value = match property {
            Property::FontSize => scalar(size, math::Dimension::Length, p::Range::Nonnegative),
            Property::FontWeight => scalar(weight, math::Dimension::Number, p::Range::FontWeight),
            Property::LineHeight => line.clone(),
            Property::FontFamily => font_values[0].value.clone(),
            Property::FontStyle => font_values[1].value.clone(),
            Property::FontWidth => font_values[2].value.clone(),
            _ if use_baseline && !declared[property as usize] => {
                if property.inherited()
                    && let Some(parent) = parent
                {
                    dependencies.inherited = true;
                    parent.value(property).clone()
                } else {
                    record(&defaults.specified[property as usize], &mut dependencies);
                    defaults.value(property).clone()
                }
            }
            _ => {
                // A declaration whose value cannot be computed here (an unsupported
                // substitution, a unit without context) is invalid at computed-value
                // time: dropped, leaving the inherited or initial value, and counted.
                let fallback = |dependencies: &mut Dependencies| {
                    dependencies.fallbacks = dependencies.fallbacks.saturating_add(1);
                    if property.inherited()
                        && let Some(parent) = parent
                    {
                        dependencies.inherited = true;
                        parent.value(property).clone()
                    } else {
                        record(&defaults.specified[property as usize], dependencies);
                        defaults.value(property).clone()
                    }
                };
                match source(defaults, &mut resolver, property, axes, parent, options, &mut dependencies) {
                    Ok(source) => match ordinary(source, property, context::numeric(host, property, defaults, parent, root, &font, line_height, axes), parent, limits, cancelled) {
                        Ok(value) => value,
                        Err(e @ (Error::Cancelled | Error::Limit | Error::Stale | Error::Dom(_))) => return Err(e),
                        Err(_) => fallback(&mut dependencies),
                    },
                    Err(e @ (Error::Cancelled | Error::Limit | Error::Stale | Error::Dom(_))) => return Err(e),
                    Err(_) => fallback(&mut dependencies),
                }
            }
        };
        let value = if property == Property::Color {
            let Value::Color(color) = value else { return Err(Error::Invalid) };
            Value::Color(match color {
                color::Computed::CurrentColor => color::Computed::Absolute(if let Some(parent) = parent {
                    parent.used_color(Property::Color)?
                } else {
                    host.system_color(color::SystemColor::CanvasText, host.scheme()).map_err(Error::color)?
                }),
                value => value,
            })
        } else {
            value
        };
        if matches!(value, Value::Color(color::Computed::CurrentColor)) {
            dependencies.color.current_color = true;
        }
        values.push(Entry { property, value });
    }
    if sparse {
        values.sort_unstable_by_key(|e| e.property as u16);
    }
    lap(&mut clock, 2);
    coupled::apply(&mut values, defaults, parent, axes, options)?;
    lap(&mut clock, 3);
    let customs = customs(defaults, cascade, &mut resolver, parent, limits, cancelled)?;
    lap(&mut clock, 4);
    dependencies.substitution |= resolver.pending_cached() != 0 || cascade.custom_names().next().is_some();
    let bytes = values.len() * size_of::<Entry>() + values.iter().map(|e| e.value.reserved_bytes()).sum::<usize>() + customs.bytes;
    if bytes > limits.retained_bytes {
        return Err(Error::Limit);
    }
    Ok(Computed { values, customs, font, axes, line_height, dependencies })
}
