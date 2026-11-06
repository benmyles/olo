use super::*;
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
        return Ok(value.clone());
    };
    Ok(match value {
        p::Value::Numeric(n) => {
            let computed = n.computed(&numeric, property != Property::LineHeight, limits.math, cancelled).map_err(Error::property)?;
            if border_width(property) {
                scalar(snap_border(computed.expression().constant().ok_or(Error::Invalid)?.value, numeric.host.device_scale())?, math::Dimension::Length, p::Range::Nonnegative)
            } else {
                Value::Numeric(computed)
            }
        }
        p::Value::FitContent(n) => Value::FitContent(n.computed(&numeric, true, limits.math, cancelled).map_err(Error::property)?),
        p::Value::Color(color) => Value::Color(color.computed(&context::Color { numeric }, limits.color, cancelled).map_err(Error::color)?),
        p::Value::Display(display) => Value::Display(display),
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
    let inherited = parent.map_or(&[][..], |p| p.customs.values.as_ref());
    if inherited.len() > limits.custom_properties {
        return Err(Error::Limit);
    }
    let mut local = cascade.custom_names().peekable();
    if local.peek().is_none() {
        return Ok(parent.map_or_else(|| defaults.empty_customs.clone(), |p| p.customs.clone()));
    }
    let mut inherited = inherited.iter().peekable();
    let mut values = Vec::new();
    while inherited.peek().is_some() || local.peek().is_some() {
        check(cancelled)?;
        if values.len() >= limits.custom_properties || values.len() >= limits.work {
            return Err(Error::Limit);
        }
        if inherited.peek().is_some_and(|a| local.peek().is_none_or(|b| a.name.as_ref() < *b)) {
            values.push(inherited.next().unwrap().clone());
        } else {
            let name = local.next().unwrap();
            if inherited.peek().is_some_and(|v| v.name.as_ref() == name) {
                inherited.next();
            }
            values.push(Custom { name: Arc::from(name), value: resolver.custom(name).map_err(Error::cascade)? });
        }
    }
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
    Ok(Arc::new(Dictionary { values: values.into_boxed_slice(), bytes }))
}
#[allow(clippy::too_many_arguments)]
pub(super) fn values(
    defaults: &Defaults,
    cascade: &cascade::Cascade<'_>,
    parent: Option<&Style>,
    root: Option<&Style>,
    host: &impl Host,
    options: Options,
    limits: Limits,
    cancelled: Option<&AtomicBool>,
) -> Result<Computed> {
    check(cancelled)?;
    if Property::ALL.len() > limits.properties {
        return Err(Error::Limit);
    }
    let mut resolver = cascade.resolver();
    let axes = resolver.axes(parent.map(|p| p.axes), options.allow_animation_tainted).map_err(Error::cascade)?;
    let parent_font = parent.map_or(defaults.initial_font.as_ref(), |p| p.font.as_ref());
    let mut dependencies = Dependencies { layout_parent: options.layout_parent.is_some(), ..Default::default() };
    let fs = source(defaults, &mut resolver, Property::FontSize, axes, parent, options, &mut dependencies)?;
    let numeric = context::numeric(host, Property::FontSize, defaults, parent, root, parent_font, parent.map_or(parent_font.metrics.normal_line_height, |p| p.line_height), axes);
    let size = font_size(fs, host, &numeric, parent_font.request.size, limits, cancelled)?;
    let fw = source(defaults, &mut resolver, Property::FontWeight, axes, parent, options, &mut dependencies)?;
    let numeric = context::numeric(host, Property::FontWeight, defaults, parent, root, parent_font, parent.map_or(parent_font.metrics.normal_line_height, |p| p.line_height), axes);
    let weight = font_weight(fw, &numeric, parent_font.request.weight, limits, cancelled)?;
    let request = FontRequest { size, weight, axes };
    let font = if parent.is_some_and(|p| p.font.request == request) {
        parent.unwrap().font.clone()
    } else if defaults.initial_font.request == request {
        defaults.initial_font.clone()
    } else {
        Arc::new(Font::new(host, request)?)
    };
    let lh = source(defaults, &mut resolver, Property::LineHeight, axes, parent, options, &mut dependencies)?;
    let numeric = context::numeric(host, Property::LineHeight, defaults, parent, root, &font, font.metrics.normal_line_height, axes);
    let line = ordinary(lh, Property::LineHeight, numeric, parent, limits, cancelled)?;
    let line_height = match &line {
        Value::Keyword(Keyword::Normal) => font.metrics.normal_line_height,
        Value::Numeric(n) => {
            let q = n.expression().constant().ok_or(Error::Invalid)?;
            let number = if q.ty.kind() == Some(math::Dimension::Number) { q.value * size } else { q.value };
            math::Quantity { value: number, ty: math::NumericType::dimension(math::Dimension::Length) }.materialize(0., f64::MAX).value
        }
        _ => return Err(Error::Invalid),
    };
    let mut declared = [false; Property::ALL.len()];
    if !cfg!(feature = "baseline-style-dense") {
        for property in cascade.declared_properties() {
            declared[axes.physical(property) as usize] = true;
        }
    }
    let use_baseline = !defaults.values.is_empty() && !cfg!(feature = "baseline-style-dense");
    let mut values = Vec::with_capacity(Property::ALL.len());
    let mut work = 0usize;
    for &property in Property::ALL {
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
                let source = source(defaults, &mut resolver, property, axes, parent, options, &mut dependencies)?;
                ordinary(source, property, context::numeric(host, property, defaults, parent, root, &font, line_height, axes), parent, limits, cancelled)?
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
    coupled::apply(&mut values, parent, axes, options)?;
    let customs = customs(defaults, cascade, &mut resolver, parent, limits, cancelled)?;
    dependencies.substitution |= resolver.pending_cached() != 0 || cascade.custom_names().next().is_some();
    let bytes = values.len() * size_of::<Entry>() + values.iter().map(|e| e.value.reserved_bytes()).sum::<usize>() + customs.bytes;
    if bytes > limits.retained_bytes {
        return Err(Error::Limit);
    }
    Ok(Computed { values, customs, font, axes, line_height, dependencies })
}
