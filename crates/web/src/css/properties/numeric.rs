use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericKind {
    Length,
    LengthPercentage,
    Number,
    NumberPercentage,
    Integer,
    LineHeight,
    Angle,
    Percentage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Nonnegative,
    Unit,
    FontWeight,
    FontAngle,
}
impl Range {
    fn bounds(self) -> (f64, f64) {
        match self {
            Self::All => (-f64::MAX, f64::MAX),
            Self::Nonnegative => (0., f64::MAX),
            Self::Unit => (0., 1.),
            Self::FontWeight => (1., 1000.),
            Self::FontAngle => (-90., 90.),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Numeric {
    expression: math::Value,
    range: Range,
    integer: bool,
    percentage_number: bool,
}
impl Numeric {
    pub(crate) fn scalar(value: f64, dimension: math::Dimension, range: Range) -> Self {
        Self { expression: math::Value::Constant(math::Quantity { value, ty: math::NumericType::dimension(dimension) }), range, integer: false, percentage_number: false }
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.range == other.range && self.integer == other.integer && self.percentage_number == other.percentage_number && self.expression.same(&other.expression)
    }
    /// Freeze this element's computed context while preserving percentages
    /// that require layout. Inheritance copies this result, not the original
    /// specified expression. Range/rounding applies once fully resolved.
    pub fn computed(&self, context: &impl math::Context, defer_percentage: bool, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        self.computed_retaining(context, if defer_percentage { math::Dependencies::PERCENTAGE } else { Default::default() }, limits, cancelled)
    }
    /// Internal delayed box lengths. The owner must supply the original
    /// element's container context when resolving the remaining expression.
    pub fn computed_retaining(&self, context: &impl math::Context, deferred: math::Dependencies, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let expression = self.expression.computed_retaining(context, deferred, limits, cancelled).map_err(Error::Numeric)?;
        let mut computed = Self { expression, range: self.range, integer: self.integer, percentage_number: self.percentage_number };
        if computed.expression.constant().is_some() {
            let value = computed.resolve(&math::NoContext, &mut math::Scratch::default(), limits, cancelled)?;
            let ty = if self.percentage_number { math::NumericType::dimension(math::Dimension::Number) } else { computed.expression.ty() };
            computed.expression = math::Value::Constant(math::Quantity { value, ty });
            computed.percentage_number = false;
        }
        Ok(computed)
    }
    pub fn expression(&self) -> &math::Value {
        &self.expression
    }
    pub fn range(&self) -> Range {
        self.range
    }
    pub fn rounds_integer(&self) -> bool {
        self.integer
    }
    /// Host supplies the correct property's percent/font/viewport basis.
    /// Font-size/line-height, block sizes and margins need different snapshots.
    pub fn resolve(&self, context: &impl math::Context, scratch: &mut math::Scratch, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<f64> {
        let q = self.expression.resolve_with(context, scratch, limits, cancelled).map_err(Error::Numeric)?;
        let mut value = if self.percentage_number { q.value / 100. } else { q.value };
        if self.integer && value.is_finite() {
            value = if value.fract() == -0.5 { value.ceil() } else { value.round() };
        }
        let (min, max) = self.range.bounds();
        Ok(math::Quantity { value, ty: q.ty }.materialize(min, max).value)
    }
    pub(super) fn compile(c: Component<'_>, kind: NumericKind, range: Range, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        use math::Dimension as D;
        let length = matches!(kind, NumericKind::Length | NumericKind::LengthPercentage | NumericKind::LineHeight);
        let percentage = matches!(kind, NumericKind::LengthPercentage | NumericKind::LineHeight | NumericKind::NumberPercentage | NumericKind::Percentage);
        let options = math::Options { allow_percentage: percentage, percentage_hint: (length && percentage).then_some(D::Length) };
        let expression = if matches!(c.kind(), Kind::Integer | Kind::Number) && c.number().unwrap().value == 0. && matches!(kind, NumericKind::Length | NumericKind::LengthPercentage) {
            math::Value::Constant(math::Quantity { value: 0., ty: math::NumericType::dimension(D::Length) })
        } else {
            math::Value::compile(c, options, limits.math(), cancelled).map_err(math_error)?
        };
        let ty = expression.ty().kind();
        let valid = match kind {
            NumericKind::Length | NumericKind::LengthPercentage => ty == Some(D::Length),
            NumericKind::Number | NumericKind::Integer => ty == Some(D::Number),
            NumericKind::NumberPercentage => matches!(ty, Some(D::Number | D::Percentage)),
            NumericKind::LineHeight => matches!(ty, Some(D::Length | D::Number)),
            NumericKind::Angle => ty == Some(D::Angle),
            NumericKind::Percentage => ty == Some(D::Percentage),
        };
        if !valid {
            return Err(Error::Invalid);
        }
        if kind == NumericKind::Integer && c.kind() != Kind::Function && !c.number().is_some_and(|n| n.integer) {
            return Err(Error::Invalid);
        }
        let percentage_number = kind == NumericKind::NumberPercentage && ty == Some(D::Percentage);
        if c.kind() != Kind::Function {
            let mut value = c.number().ok_or(Error::Invalid)?.value;
            if percentage_number {
                value /= 100.;
            }
            if range == Range::FontAngle {
                value = expression.resolve_raw(&math::NoContext, limits.math(), cancelled).map_err(math_error)?.value;
            }
            // Unbounded grammar ranges also accept finite-token overflow.
            // Censor IEEE specials only when computing the property value.
            let invalid = value.is_nan()
                || match range {
                    Range::All | Range::Unit => false,
                    Range::Nonnegative => value < 0.,
                    Range::FontWeight => !(1. ..=1000.).contains(&value),
                    Range::FontAngle => !(-90. ..=90.).contains(&value),
                };
            if invalid {
                return Err(Error::Invalid);
            }
        }
        Ok(Self { expression, range, integer: kind == NumericKind::Integer, percentage_number })
    }
}
fn math_error(e: math::Error) -> Error {
    match e {
        math::Error::Invalid => Error::Invalid,
        math::Error::Unsupported(s) => Error::Unsupported(s),
        math::Error::RequiresSubstitution => Error::Unsupported("numeric substitution"),
        math::Error::Limit | math::Error::TypeLimit => Error::Limit,
        math::Error::DepthLimit => Error::DepthLimit,
        math::Error::Cancelled => Error::Cancelled,
        e => Error::Numeric(e),
    }
}
