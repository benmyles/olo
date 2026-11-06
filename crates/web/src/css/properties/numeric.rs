use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericKind {
    Length,
    LengthPercentage,
    Number,
    NumberPercentage,
    Integer,
    LineHeight,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Nonnegative,
    Unit,
    FontWeight,
}
impl Range {
    fn bounds(self) -> (f64, f64) {
        match self {
            Self::All => (-f64::MAX, f64::MAX),
            Self::Nonnegative => (0., f64::MAX),
            Self::Unit => (0., 1.),
            Self::FontWeight => (1., 1000.),
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
        let percentage = matches!(kind, NumericKind::LengthPercentage | NumericKind::LineHeight | NumericKind::NumberPercentage);
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
            // Unbounded grammar ranges also accept finite-token overflow.
            // Censor IEEE specials only when computing the property value.
            let invalid = value.is_nan()
                || match range {
                    Range::All | Range::Unit => false,
                    Range::Nonnegative => value < 0.,
                    Range::FontWeight => !(1. ..=1000.).contains(&value),
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
