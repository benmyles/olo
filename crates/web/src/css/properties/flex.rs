//! The flex shorthand's ordered number group and optional unordered basis.
use super::*;
use std::sync::OnceLock;
fn number(value: f64) -> Value {
    Value::Numeric(Numeric::scalar(value, math::Dimension::Number, Range::Nonnegative))
}
fn zero_basis() -> Value {
    // Keep an omitted basis as 0%, distinct from an explicitly authored 0px
    // when the containing size is indefinite. The bounded literal is compiled
    // once; declarations share its immutable expression rather than reparse it.
    static ZERO: OnceLock<Numeric> = OnceLock::new();
    Value::Numeric(
        ZERO.get_or_init(|| {
            let syntax = crate::css::Syntax::parse(Arc::from("0%"), Default::default(), None).expect("literal percentage syntax");
            Numeric::compile(syntax.components().next().unwrap(), NumericKind::LengthPercentage, Range::Nonnegative, Default::default(), None).expect("literal zero percentage")
        })
        .clone(),
    )
}
fn factor(c: Component<'_>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Value> {
    if !matches!(c.kind(), Kind::Integer | Kind::Number | Kind::Function) {
        return Err(Error::Invalid);
    }
    Numeric::compile(c, NumericKind::Number, Range::Nonnegative, limits, cancel).map(Value::Numeric).map_err(|e| match e {
        // An unsupported numeric function can still be a recognized basis
        // function. Let that grammar decide; never discard resource errors.
        Error::Unsupported(_) => Error::Invalid,
        e => e,
    })
}
fn basis(c: Component<'_>, limits: Limits, cancel: Option<&AtomicBool>) -> Result<Value> {
    super::parse::component(super::registry::Grammar::Size(super::registry::SizeKind::Basis), c, limits, cancel)
}
fn assignments(grow: Value, shrink: Value, basis: Value) -> Assignments {
    smallvec::smallvec![
        Assignment { property: Property::FlexGrow, value: grow },
        Assignment { property: Property::FlexShrink, value: shrink },
        Assignment { property: Property::FlexBasis, value: basis }
    ]
}
pub(super) fn parse(parts: &[Component<'_>], limits: Limits, cancel: Option<&AtomicBool>) -> Result<Assignments> {
    check(cancel)?;
    if parts.is_empty() || parts.len() > 3 {
        return Err(Error::Invalid);
    }
    if parts.len() == 1 && parts[0].kind() == Kind::Ident && parts[0].text().unwrap().eq_ignore_ascii_case("none") {
        return Ok(assignments(number(0.), number(0.), Value::Keyword(Keyword::Auto)));
    }
    match factor(parts[0], limits, cancel) {
        Ok(grow) => {
            if parts.len() == 1 {
                return Ok(assignments(grow, number(1.), zero_basis()));
            }
            match factor(parts[1], limits, cancel) {
                Ok(shrink) => Ok(assignments(grow, shrink, if parts.len() == 3 { basis(parts[2], limits, cancel)? } else { zero_basis() })),
                Err(Error::Invalid) if parts.len() == 2 => Ok(assignments(grow, number(1.), basis(parts[1], limits, cancel)?)),
                Err(e) => Err(e),
            }
        }
        Err(Error::Invalid) => {
            let basis = basis(parts[0], limits, cancel)?;
            let grow = if parts.len() > 1 { factor(parts[1], limits, cancel)? } else { number(1.) };
            let shrink = if parts.len() == 3 { factor(parts[2], limits, cancel)? } else { number(1.) };
            Ok(assignments(grow, shrink, basis))
        }
        Err(e) => Err(e),
    }
}
