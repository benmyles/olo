use super::*;
use crate::css::math::{self, Dimension, NumericType, Quantity};
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Unit {
    Number,
    Percent,
    None,
    Angle,
}
#[derive(Clone, Copy)]
pub(super) struct Numeric {
    pub value: f64,
    pub unit: Unit,
    pub calculation: u32,
}
impl Numeric {
    pub fn literal(value: f64, unit: Unit) -> Self {
        Self { value, unit, calculation: u32::MAX }
    }
    pub fn quantity(self) -> Quantity {
        let dimension = match self.unit {
            Unit::Angle => Dimension::Angle,
            Unit::Percent => Dimension::Percentage,
            _ => Dimension::Number,
        };
        Quantity { value: self.value, ty: NumericType::dimension(dimension) }
    }
}
pub(super) fn angular(space: Space, i: usize) -> bool {
    matches!(space, Space::Hsl | Space::Hwb) && i == 0 || matches!(space, Space::Lch | Space::Oklch) && i == 2
}
/// Four checked numeric channels; dynamic math retains no source syntax.
/// Stored directly in conditional arenas, shared by Arc for a root value.
pub struct Functional {
    pub(super) channels: [math::Value; 4],
    pub(super) units: [Unit; 4],
    pub(super) space: Space,
    pub(super) legacy: bool,
}
struct MathHost<'a, C>(&'a C);
impl<C: Context> math::Context for MathHost<'_, C> {
    fn factor(&self, unit: math::RelativeUnit) -> math::Result<f64> {
        self.0.math_factor(unit)
    }
    fn tree_count(&self, kind: math::TreeCount) -> math::Result<u32> {
        self.0.math_tree_count(kind)
    }
}
impl Functional {
    pub(super) fn dependencies(&self) -> math::Dependencies {
        self.channels.iter().fold(math::Dependencies::default(), |deps, v| deps.union(v.dependencies()))
    }
    pub(super) fn reserved_bytes(&self) -> usize {
        self.channels.iter().map(|v| v.memory().accounted_reserved_bytes).sum()
    }
    pub(super) fn resolve(&self, context: &impl Context, mut limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Color> {
        let mut numbers = [Numeric::literal(0., Unit::Number); 4];
        let mut scratch = math::Scratch::default();
        for (i, channel) in self.channels.iter().enumerate() {
            let cost = channel.memory().instructions;
            if cost > limits.work {
                return Err(Error::Limit);
            }
            let value = channel.resolve_with(&MathHost(context), &mut scratch, math_limits(limits), cancelled).map_err(math_error)?;
            limits.work -= cost;
            numbers[i] = Numeric::literal(value.value, self.units[i]);
        }
        Ok(materialize(&numbers, self.space, self.legacy))
    }
}
pub(super) fn math_limits(limits: Limits) -> math::Limits {
    math::Limits { instructions: limits.nodes, work: limits.work, depth: limits.depth.min(128), stack: limits.nodes.min(4096) }
}
pub(super) fn math_error(error: math::Error) -> Error {
    match error {
        math::Error::Invalid => Error::Invalid,
        math::Error::Unsupported(reason) => Error::Unsupported(reason),
        math::Error::RequiresSubstitution => Error::RequiresSubstitution,
        math::Error::ContextUnavailable => Error::ContextUnavailable,
        math::Error::Limit | math::Error::TypeLimit => Error::Limit,
        math::Error::DepthLimit => Error::DepthLimit,
        math::Error::Cancelled => Error::Cancelled,
    }
}
#[inline]
fn separators(numbers: &[Numeric; 4], space: Space, comma: bool) -> Result<()> {
    if comma && numbers.iter().any(|n| n.unit == Unit::None) {
        return Err(Error::Invalid);
    }
    if comma && space == Space::Srgb && numbers[0..3].iter().any(|n| n.unit != numbers[0].unit) {
        return Err(Error::Invalid);
    }
    if comma && space == Space::Hsl && (numbers[1].unit != Unit::Percent || numbers[2].unit != Unit::Percent) {
        return Err(Error::Invalid);
    }
    if !matches!(numbers[3].unit, Unit::Number | Unit::Percent | Unit::None) {
        return Err(Error::Invalid);
    }
    Ok(())
}
#[inline]
pub(super) fn validate(numbers: &[Numeric; 4], space: Space, comma: bool) -> Result<()> {
    separators(numbers, space, comma)?;
    for (i, n) in numbers.iter().enumerate().take(3) {
        if angular(space, i) {
            if !matches!(n.unit, Unit::Number | Unit::Angle | Unit::None) {
                return Err(Error::Invalid);
            }
        } else if !matches!(n.unit, Unit::Number | Unit::Percent | Unit::None) {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
#[inline]
pub(super) fn materialize(numbers: &[Numeric; 4], space: Space, legacy_allowed: bool) -> Color {
    materialize_impl::<false>(numbers, space, legacy_allowed).unwrap()
}
#[inline]
pub(super) fn checked_materialize(numbers: &[Numeric; 4], space: Space, legacy_allowed: bool, comma: bool) -> Result<Color> {
    separators(numbers, space, comma)?;
    materialize_impl::<true>(numbers, space, legacy_allowed)
}
#[inline]
fn materialize_impl<const CHECK: bool>(numbers: &[Numeric; 4], space: Space, legacy_allowed: bool) -> Result<Color> {
    let mut values = [0.; 3];
    let mut missing = 0;
    for (i, number) in numbers.iter().enumerate() {
        if number.unit == Unit::None {
            missing |= 1 << i;
        }
    }
    for i in 0..3 {
        let n = numbers[i];
        let angular = angular(space, i);
        values[i] = if angular {
            if CHECK && !matches!(n.unit, Unit::Number | Unit::Angle | Unit::None) {
                return Err(Error::Invalid);
            }
            hue(n.value)
        } else {
            if CHECK && !matches!(n.unit, Unit::Number | Unit::Percent | Unit::None) {
                return Err(Error::Invalid);
            }
            let scale = match space {
                Space::Srgb if legacy_allowed => {
                    if n.unit == Unit::Percent {
                        0.01
                    } else {
                        1. / 255.
                    }
                }
                Space::Hsl | Space::Hwb => 1.,
                Space::Lab => {
                    if n.unit == Unit::Percent && i > 0 {
                        1.25
                    } else {
                        1.
                    }
                }
                Space::Lch => {
                    if n.unit == Unit::Percent && i == 1 {
                        1.5
                    } else {
                        1.
                    }
                }
                Space::Oklab | Space::Oklch => {
                    if n.unit == Unit::Percent {
                        if i == 0 { 0.01 } else { 0.004 }
                    } else {
                        1.
                    }
                }
                _ => {
                    if n.unit == Unit::Percent {
                        0.01
                    } else {
                        1.
                    }
                }
            };
            n.value * scale
        };
    }
    // Color(srgb ...) is unbounded; legacy RGB values clamp at parse time.
    if space == Space::Srgb && legacy_allowed {
        values = values.map(|v| v.clamp(0., 1.));
    }
    if space == Space::Hsl {
        values[1] = values[1].max(0.);
    }
    if matches!(space, Space::Lab | Space::Lch | Space::Oklab | Space::Oklch) {
        values[0] = values[0].clamp(0., if matches!(space, Space::Lab | Space::Lch) { 100. } else { 1. });
    }
    if matches!(space, Space::Lch | Space::Oklch) {
        values[1] = values[1].max(0.);
    }
    let alpha = numbers[3].value * if numbers[3].unit == Unit::Percent { 0.01 } else { 1. };
    Ok(Color::new(space, values, alpha, missing))
}
