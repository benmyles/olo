//! `transform` (CSS Transforms 1): a list of 2D functions. The 3D functions
//! that reduce to 2D (`translate3d`, `translateZ`, `scale3d`, `rotateZ`) are
//! accepted as their 2D equivalents; the rest are not supported.
use super::*;
use crate::css::math;
use std::sync::Arc;
#[derive(Clone, Debug)]
pub enum Function {
    /// Lengths or percentages (of the box's own size), resolved at paint.
    Translate(Numeric, Numeric),
    Scale(f64, f64),
    /// An angle, in degrees once computed.
    Rotate(Numeric),
    Skew(Numeric, Numeric),
    /// `matrix(a, b, c, d, e, f)`.
    Matrix([f64; 6]),
}
#[derive(Clone, Debug)]
pub struct Transforms(pub Arc<[Function]>);
fn bytes(n: &Numeric) -> usize {
    n.expression().memory().accounted_reserved_bytes
}
impl Transforms {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Function>()
            + self
                .0
                .iter()
                .map(|f| match f {
                    Function::Translate(x, y) | Function::Skew(x, y) => bytes(x) + bytes(y),
                    Function::Rotate(a) => bytes(a),
                    _ => 0,
                })
                .sum::<usize>()
    }
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn computed(&self, numeric: &impl math::Context, math: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Transforms> {
        let mut out = Vec::with_capacity(self.0.len());
        for f in self.0.iter() {
            check(cancelled)?;
            out.push(match f {
                Function::Translate(x, y) => Function::Translate(x.computed(numeric, true, math, cancelled)?, y.computed(numeric, true, math, cancelled)?),
                Function::Scale(x, y) => Function::Scale(*x, *y),
                Function::Rotate(a) => Function::Rotate(a.computed(numeric, false, math, cancelled)?),
                Function::Skew(x, y) => Function::Skew(x.computed(numeric, false, math, cancelled)?, y.computed(numeric, false, math, cancelled)?),
                Function::Matrix(m) => Function::Matrix(*m),
            });
        }
        Ok(Transforms(out.into()))
    }
    /// The list as one affine matrix `[a, b, c, d, e, f]` (x' = a·x + c·y + e,
    /// y' = b·x + d·y + f), percentages of the box's width and height.
    pub fn matrix(&self, width: f64, height: f64) -> [f64; 6] {
        struct Basis(f64);
        impl math::Context for Basis {
            fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
                if unit == math::RelativeUnit::Percentage { Ok(self.0 / 100.) } else { Err(math::Error::ContextUnavailable) }
            }
        }
        let resolve = |n: &Numeric, basis: f64| n.resolve(&Basis(basis), &mut math::Scratch::default(), Default::default(), None).unwrap_or(0.);
        let mut m = IDENTITY;
        for f in self.0.iter() {
            let local = match f {
                Function::Translate(x, y) => [1., 0., 0., 1., resolve(x, width), resolve(y, height)],
                Function::Scale(x, y) => [*x, 0., 0., *y, 0., 0.],
                Function::Rotate(a) => {
                    let (s, c) = resolve(a, 0.).to_radians().sin_cos();
                    [c, s, -s, c, 0., 0.]
                }
                Function::Skew(x, y) => [1., resolve(y, 0.).to_radians().tan(), resolve(x, 0.).to_radians().tan(), 1., 0., 0.],
                Function::Matrix(m) => *m,
            };
            m = multiply(m, local);
        }
        m
    }
}
pub const IDENTITY: [f64; 6] = [1., 0., 0., 1., 0., 0.];
/// `a` then `b` in the usual column-vector sense: the result applies `b` to a
/// point first, then `a` (so a list `f1 f2` is `multiply(f1, f2)`).
pub fn multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1], a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3], a[0] * b[4] + a[2] * b[5] + a[4], a[1] * b[4] + a[3] * b[5] + a[5]]
}
pub fn apply(m: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}
pub fn is_identity(m: [f64; 6]) -> bool {
    m == IDENTITY
}
fn number(c: Component<'_>) -> Result<f64> {
    if matches!(c.kind(), Kind::Number | Kind::Integer) { c.number().map(|n| n.value).filter(|v| v.is_finite()).ok_or(Error::Invalid) } else { Err(Error::Invalid) }
}
fn angle(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Numeric> {
    if matches!(c.kind(), Kind::Number | Kind::Integer) && c.number().is_some_and(|n| n.value == 0.) {
        return Ok(Numeric::scalar(0., math::Dimension::Angle, Range::All));
    }
    Numeric::compile(c, NumericKind::Angle, Range::All, limits, cancelled)
}
fn length(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Numeric> {
    Numeric::compile(c, NumericKind::LengthPercentage, Range::All, limits, cancelled)
}
fn zero() -> Numeric {
    Numeric::scalar(0., math::Dimension::Length, Range::All)
}
/// `transform: none | <transform-function>+`.
pub(super) fn parse_transforms(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    if let [c] = parts
        && c.kind() == Kind::Ident
        && c.text().unwrap().eq_ignore_ascii_case("none")
    {
        return Ok(Value::Keyword(Keyword::None));
    }
    let mut out = Vec::new();
    for c in parts {
        check(cancelled)?;
        if c.kind() != Kind::Function {
            return Err(Error::Invalid);
        }
        let name = c.text().unwrap().to_ascii_lowercase();
        let args: Vec<Component<'_>> = c.children().filter(|c| c.kind() != Kind::Whitespace).collect();
        let args: Vec<&[Component<'_>]> = args.split(|c| c.kind() == Kind::Comma).collect();
        let arg = |i: usize| -> Result<Component<'_>> {
            match args.get(i) {
                Some([c]) => Ok(*c),
                _ => Err(Error::Invalid),
            }
        };
        let n = args.len();
        out.push(match name.as_str() {
            "translate" if (1..=2).contains(&n) => Function::Translate(length(arg(0)?, limits, cancelled)?, if n == 2 { length(arg(1)?, limits, cancelled)? } else { zero() }),
            "translatex" if n == 1 => Function::Translate(length(arg(0)?, limits, cancelled)?, zero()),
            "translatey" if n == 1 => Function::Translate(zero(), length(arg(0)?, limits, cancelled)?),
            "translate3d" if n == 3 => {
                length(arg(2)?, limits, cancelled)?;
                Function::Translate(length(arg(0)?, limits, cancelled)?, length(arg(1)?, limits, cancelled)?)
            }
            "translatez" if n == 1 => {
                length(arg(0)?, limits, cancelled)?;
                Function::Translate(zero(), zero())
            }
            "scale" if (1..=2).contains(&n) => {
                let x = number(arg(0)?)?;
                Function::Scale(x, if n == 2 { number(arg(1)?)? } else { x })
            }
            "scalex" if n == 1 => Function::Scale(number(arg(0)?)?, 1.),
            "scaley" if n == 1 => Function::Scale(1., number(arg(0)?)?),
            "scale3d" if n == 3 => {
                number(arg(2)?)?;
                Function::Scale(number(arg(0)?)?, number(arg(1)?)?)
            }
            "scalez" if n == 1 => {
                number(arg(0)?)?;
                Function::Scale(1., 1.)
            }
            "rotate" | "rotatez" if n == 1 => Function::Rotate(angle(arg(0)?, limits, cancelled)?),
            "skew" if (1..=2).contains(&n) => {
                Function::Skew(angle(arg(0)?, limits, cancelled)?, if n == 2 { angle(arg(1)?, limits, cancelled)? } else { Numeric::scalar(0., math::Dimension::Angle, Range::All) })
            }
            "skewx" if n == 1 => Function::Skew(angle(arg(0)?, limits, cancelled)?, Numeric::scalar(0., math::Dimension::Angle, Range::All)),
            "skewy" if n == 1 => Function::Skew(Numeric::scalar(0., math::Dimension::Angle, Range::All), angle(arg(0)?, limits, cancelled)?),
            "matrix" if n == 6 => {
                let mut m = [0.; 6];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = number(arg(i)?)?;
                }
                Function::Matrix(m)
            }
            "rotatex" | "rotatey" | "rotate3d" | "matrix3d" | "perspective" => return Err(Error::Unsupported("3D transform")),
            _ => return Err(Error::Invalid),
        });
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Transforms(Transforms(out.into())))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matrices_compose_in_list_order() {
        let t = multiply([1., 0., 0., 1., 10., 20.], [2., 0., 0., 2., 0., 0.]);
        assert_eq!(apply(t, 1., 1.), (12., 22.));
        let r = multiply([2., 0., 0., 2., 0., 0.], [1., 0., 0., 1., 10., 20.]);
        assert_eq!(apply(r, 1., 1.), (22., 42.));
    }
}
