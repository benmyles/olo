use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    UserAgent,
    User,
    PresentationalHint,
    Author,
    Animation,
    Transition,
}
/// Global source/declaration order and rule identity are supplied by the
/// stylesheet set/host. Layer is the nested-layer postorder rank; MAX is the
/// implicit outer layer. Encapsulation/proximity are host metadata only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Priority {
    pub origin: Origin,
    pub important: bool,
    pub encapsulation: u16,
    pub inline: bool,
    pub layer: u32,
    pub specificity: selectors::Specificity,
    pub proximity: u32,
    pub order: u64,
    pub declaration: u32,
    pub rule: u64,
}
impl Default for Priority {
    fn default() -> Self {
        Self { origin: Origin::Author, important: false, encapsulation: 0, inline: false, layer: u32::MAX, specificity: Default::default(), proximity: u32::MAX, order: 0, declaration: 0, rule: 0 }
    }
}
impl Priority {
    pub(super) fn validate(self) -> Result<()> {
        if self.important && matches!(self.origin, Origin::Animation | Origin::Transition | Origin::PresentationalHint) || self.inline && self.origin != Origin::Author {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn rank(self) -> u8 {
        match (self.origin, self.important) {
            (Origin::Transition, _) => 8,
            (Origin::UserAgent, true) => 7,
            (Origin::User, true) => 6,
            (Origin::Author, true) => 5,
            (Origin::Animation, _) => 4,
            (Origin::Author, _) => 3,
            (Origin::PresentationalHint, _) => 2,
            (Origin::User, _) => 1,
            (Origin::UserAgent, _) => 0,
        }
    }
    pub(super) fn eligible_after(self, from: Self, wide: variables::Wide) -> Result<bool> {
        use variables::Wide as W;
        if self.encapsulation != 0 || from.encapsulation != 0 {
            return Err(Error::Unsupported("encapsulation rollback host"));
        }
        Ok(match wide {
            W::Revert => match from.origin {
                Origin::UserAgent => false,
                Origin::User => self.origin == Origin::UserAgent,
                Origin::Author | Origin::PresentationalHint | Origin::Animation => self.origin <= Origin::User,
                Origin::Transition => return Err(Error::Unsupported("transition rollback")),
            },
            W::RevertRule if from.origin != Origin::Animation => self < from && self.rule != from.rule,
            W::RevertLayer | W::RevertRule => {
                if self.origin < from.origin {
                    true
                } else if self.origin != from.origin {
                    false
                } else if from.inline {
                    !self.inline
                } else {
                    !self.inline && self.layer < from.layer
                }
            }
            _ => return Err(Error::Invalid),
        })
    }
}
impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank()
            .cmp(&other.rank())
            .then_with(|| if self.important { self.encapsulation.cmp(&other.encapsulation) } else { other.encapsulation.cmp(&self.encapsulation) })
            .then(self.inline.cmp(&other.inline))
            .then_with(|| if self.important { other.layer.cmp(&self.layer) } else { self.layer.cmp(&other.layer) })
            .then(self.specificity.cmp(&other.specificity))
            .then(other.proximity.cmp(&self.proximity))
            .then(self.order.cmp(&other.order))
            .then(self.declaration.cmp(&other.declaration))
            .then(self.rule.cmp(&other.rule))
    }
}
impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
