use super::*;

/// A computed color preserves currentcolor for properties other than color.
/// Inheriting that keyword must use the child's color at used-value time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Computed {
    Absolute(Color),
    CurrentColor,
}
impl Computed {
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::CurrentColor, Self::CurrentColor) => true,
            (Self::Absolute(a), Self::Absolute(b)) => a.space == b.space && a.missing == b.missing && a.channels.iter().zip(b.channels).all(|(a, b)| a.to_bits() == b.to_bits()),
            _ => false,
        }
    }
    pub fn used(self, current: Color) -> Color {
        match self {
            Self::Absolute(color) => color,
            Self::CurrentColor => current,
        }
    }
}
impl Value {
    /// Select scheme/system/functional values at computed-value time without
    /// prematurely substituting currentcolor. The color property itself must
    /// replace CurrentColor with its inherited computed color.
    pub fn computed(&self, context: &impl Context, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Computed> {
        check(cancelled)?;
        match self {
            Self::CurrentColor => return Ok(Computed::CurrentColor),
            Self::Expression(expression) => {
                let mut at = expression.root;
                for work in 0..limits.work {
                    check(cancelled)?;
                    match expression.nodes[at as usize] {
                        Node::Current => return Ok(Computed::CurrentColor),
                        Node::Absolute(color) => return Ok(Computed::Absolute(color.computed())),
                        Node::System(system) => return context.system_color(system, context.scheme()).map(|c| Computed::Absolute(c.computed())),
                        Node::Functional(i) => {
                            return expression.functions[i as usize].resolve(context, Limits { work: limits.work - work - 1, ..limits }, cancelled).map(|c| Computed::Absolute(c.computed()));
                        }
                        Node::LightDark { light, dark } => at = if context.scheme() == Scheme::Light { light } else { dark },
                    }
                }
                return Err(Error::Limit);
            }
            _ => {}
        }
        self.resolve(context, limits, cancelled).map(Computed::Absolute)
    }
}
