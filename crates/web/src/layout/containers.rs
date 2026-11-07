//! Query-container length bases for the implemented horizontal normal flow.
//! Vertical writing modes are rejected by box construction before this path.
use super::*;
use crate::css::{
    math,
    properties::{Keyword, Property},
    style::{Style, Value},
};

/// Nearest eligible ancestor on each physical axis. In horizontal flow these
/// are also the inline/block axes. Carried down the active layout stack rather
/// than walking ancestors per length or retaining a second document index.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct QueryBasis {
    width: f64,
    height: f64,
}
impl QueryBasis {
    pub fn viewport(viewport: Viewport) -> Self {
        Self { width: viewport.width, height: viewport.height }
    }
    pub fn descendants(self, style: &Style, width: f64, height: f64) -> Result<Self> {
        Ok(match style.value(Property::ContainerType) {
            Value::Keyword(Keyword::Normal) => self,
            Value::Keyword(Keyword::InlineSize) => Self { width, ..self },
            Value::Keyword(Keyword::Size) => Self { width, height },
            _ => return Err(Error::Invalid),
        })
    }
    pub fn factor(self, unit: math::RelativeUnit) -> math::Result<f64> {
        use math::RelativeUnit::*;
        Ok(match unit {
            Cqw | Cqi => self.width,
            Cqh | Cqb => self.height,
            Cqmin => self.width.min(self.height),
            Cqmax => self.width.max(self.height),
            _ => return Err(math::Error::UnitContextUnavailable(unit)),
        } / 100.)
    }
}
