use super::*;
pub(super) struct Numeric<'a, H> {
    pub host: &'a H,
    pub font: &'a Font,
    pub root_font: Option<&'a Font>,
    pub line_height: f64,
    pub root_line_height: Option<f64>,
    pub root_axes: Option<Axes>,
    pub percentage: Option<f64>,
}
impl<H: Host> math::Context for Numeric<'_, H> {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        use math::RelativeUnit::*;
        let value = match unit {
            Percentage => self.percentage.ok_or(math::Error::ContextUnavailable)?,
            Em => self.font.request.size,
            Ex => self.font.metrics.ex,
            Cap => self.font.metrics.cap,
            Ch => self.font.metrics.ch,
            Ic => self.font.metrics.ic,
            Lh => self.line_height,
            Rem => self.root_font.ok_or(math::Error::ContextUnavailable)?.request.size,
            Rex => self.root_font.ok_or(math::Error::ContextUnavailable)?.metrics.ex,
            Rcap => self.root_font.ok_or(math::Error::ContextUnavailable)?.metrics.cap,
            Rch => self.root_font.ok_or(math::Error::ContextUnavailable)?.metrics.ch,
            Ric => self.root_font.ok_or(math::Error::ContextUnavailable)?.metrics.ic,
            Rlh => self.root_line_height.ok_or(math::Error::ContextUnavailable)?,
            _ => return self.host.factor(unit, self.root_axes),
        };
        Ok(value)
    }
    fn tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
        self.host.tree_count(kind)
    }
}
pub(super) struct Color<'a, H> {
    pub numeric: Numeric<'a, H>,
}
impl<H: Host> color::Context for Color<'_, H> {
    fn scheme(&self) -> color::Scheme {
        self.numeric.host.scheme()
    }
    fn system_color(&self, color: color::SystemColor, scheme: color::Scheme) -> color::Result<color::Color> {
        self.numeric.host.system_color(color, scheme)
    }
    fn math_factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        math::Context::factor(&self.numeric, unit)
    }
    fn math_tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
        self.numeric.host.tree_count(kind)
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn numeric<'a, H: Host>(
    host: &'a H,
    property: Property,
    defaults: &'a Defaults,
    parent: Option<&'a Style>,
    root: Option<&'a Style>,
    font: &'a Font,
    line_height: f64,
    axes: Axes,
) -> Numeric<'a, H> {
    let affecting = matches!(property, Property::FontSize | Property::FontWeight | Property::FontWidth | Property::FontStyle);
    let parent_font = parent.map_or(defaults.initial_font.as_ref(), |p| p.font.as_ref());
    Numeric {
        host,
        font: if affecting { parent_font } else { font },
        root_font: root.map(|r| r.font.as_ref()).or_else(|| parent.is_none().then_some(if affecting { defaults.initial_font.as_ref() } else { font })),
        line_height: if affecting || property == Property::LineHeight { parent.map_or(defaults.initial_font.metrics.normal_line_height, |p| p.line_height) } else { line_height },
        root_line_height: root
            .map(|r| r.line_height)
            .or_else(|| parent.is_none().then_some(if affecting || property == Property::LineHeight { defaults.initial_font.metrics.normal_line_height } else { line_height })),
        root_axes: root.map(|r| r.axes).or_else(|| parent.is_none().then_some(axes)),
        percentage: match property {
            Property::FontSize => Some(parent_font.request.size / 100.),
            Property::LineHeight => Some(font.request.size / 100.),
            Property::FontWidth => Some(1.),
            _ => None,
        },
    }
}
