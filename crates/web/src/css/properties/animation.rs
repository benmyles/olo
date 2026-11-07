//! CSS Animations 1 values: the `animation-*` longhands, each a comma list
//! (one entry per animation), and the `animation` shorthand split into them.
use super::*;
use crate::css::{Component, Kind};

/// Which longhand a list is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Duration,
    Delay,
    Count,
    Direction,
    Fill,
    PlayState,
    Easing,
}
/// An easing function (CSS Easing 1): linear, a cubic Bézier, or steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    Bezier([f64; 4]),
    /// Steps and whether they jump at the start (`jump-start`, `start`).
    Steps(u32, bool),
}
impl Easing {
    pub const EASE: Self = Self::Bezier([0.25, 0.1, 0.25, 1.]);
    /// The output progress for an input progress in [0, 1].
    pub fn at(self, t: f64) -> f64 {
        let t = t.clamp(0., 1.);
        match self {
            Self::Linear => t,
            Self::Steps(n, start) => {
                let n = f64::from(n.max(1));
                let step = (t * n).floor() + if start { 1. } else { 0. };
                (step / n).min(1.)
            }
            Self::Bezier([x1, y1, x2, y2]) => {
                // Solve x(s) = t for s by Newton's method then bisection.
                let bez = |a: f64, b: f64, s: f64| 3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s;
                let mut s = t;
                for _ in 0..8 {
                    let x = bez(x1, x2, s) - t;
                    let d = 3. * (1. - s) * (1. - s) * x1 + 6. * (1. - s) * s * (x2 - x1) + 3. * s * s * (1. - x2);
                    if x.abs() < 1e-7 {
                        return bez(y1, y2, s);
                    }
                    if d.abs() < 1e-6 {
                        break;
                    }
                    s = (s - x / d).clamp(0., 1.);
                }
                let (mut lo, mut hi) = (0., 1.);
                for _ in 0..30 {
                    s = (lo + hi) / 2.;
                    if bez(x1, x2, s) < t {
                        lo = s;
                    } else {
                        hi = s;
                    }
                }
                bez(y1, y2, s)
            }
        }
    }
}
/// One entry of a list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// `none` as a name.
    None,
    Name(Arc<str>),
    /// Seconds.
    Time(f64),
    Count(f64),
    Infinite,
    Keyword(Keyword),
    Easing(Easing),
}
/// A longhand's list, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Animations(pub Arc<[Item]>);
impl Animations {
    pub fn items(&self) -> &[Item] {
        &self.0
    }
    /// The entry for the `i`th animation, the list repeated as needed.
    pub fn at(&self, i: usize) -> Option<&Item> {
        if self.0.is_empty() { None } else { self.0.get(i % self.0.len()) }
    }
}
/// The comma-separated groups of a value's components.
fn groups<'a>(parts: &[Component<'a>]) -> Vec<Vec<Component<'a>>> {
    let mut out = vec![Vec::new()];
    for c in parts {
        if c.kind() == Kind::Comma {
            out.push(Vec::new());
        } else if c.kind() != Kind::Whitespace {
            out.last_mut().unwrap().push(*c);
        }
    }
    out
}
fn num(c: &Component<'_>) -> Option<f64> {
    c.number().map(|n| n.value)
}
fn plain_number(c: &Component<'_>) -> Option<f64> {
    if matches!(c.kind(), Kind::Number | Kind::Integer) { num(c) } else { None }
}
fn time(c: &Component<'_>) -> Option<f64> {
    if !matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension) {
        return (plain_number(c) == Some(0.)).then_some(0.);
    }
    let v = num(c)?;
    let unit = c.unit()?;
    if unit.eq_ignore_ascii_case("s") {
        Some(v)
    } else if unit.eq_ignore_ascii_case("ms") {
        Some(v / 1000.)
    } else {
        None
    }
}
fn easing(c: &Component<'_>) -> Option<Easing> {
    match c.kind() {
        Kind::Ident => {
            let s = c.text()?.to_ascii_lowercase();
            Some(match s.as_str() {
                "linear" => Easing::Linear,
                "ease" => Easing::EASE,
                "ease-in" => Easing::Bezier([0.42, 0., 1., 1.]),
                "ease-out" => Easing::Bezier([0., 0., 0.58, 1.]),
                "ease-in-out" => Easing::Bezier([0.42, 0., 0.58, 1.]),
                "step-start" => Easing::Steps(1, true),
                "step-end" => Easing::Steps(1, false),
                _ => return None,
            })
        }
        Kind::Function => {
            let name = c.text()?.to_ascii_lowercase();
            let args: Vec<Component<'_>> = c.children().filter(|a| a.kind() != Kind::Whitespace && a.kind() != Kind::Comma).collect();
            match name.as_str() {
                "cubic-bezier" if args.len() == 4 => {
                    let v: Vec<f64> = args.iter().filter_map(plain_number).collect();
                    (v.len() == 4 && (0. ..=1.).contains(&v[0]) && (0. ..=1.).contains(&v[2])).then(|| Easing::Bezier([v[0], v[1], v[2], v[3]]))
                }
                "steps" if !args.is_empty() => {
                    let n = plain_number(&args[0]).filter(|n| *n >= 1. && n.fract() == 0.)? as u32;
                    let start = args.get(1).and_then(|a| a.text()).is_some_and(|p| p.eq_ignore_ascii_case("start") || p.eq_ignore_ascii_case("jump-start"));
                    Some(Easing::Steps(n, start))
                }
                // linear() with stops: its ends (a straight line).
                "linear" => Some(Easing::Linear),
                _ => None,
            }
        }
        _ => None,
    }
}
fn keyword_of(c: &Component<'_>, words: &[Keyword]) -> Option<Keyword> {
    let s = c.text().filter(|_| c.kind() == Kind::Ident)?;
    words.iter().copied().find(|k| k.as_str().eq_ignore_ascii_case(s))
}
const DIRECTIONS: &[Keyword] = &[Keyword::Normal, Keyword::Reverse, Keyword::Alternate, Keyword::AlternateReverse];
const FILLS: &[Keyword] = &[Keyword::None, Keyword::Forwards, Keyword::Backwards, Keyword::Both];
const STATES: &[Keyword] = &[Keyword::Running, Keyword::Paused];
fn item(field: Field, c: &Component<'_>) -> Option<Item> {
    match field {
        Field::Name => match c.kind() {
            Kind::Ident if c.text()?.eq_ignore_ascii_case("none") => Some(Item::None),
            Kind::Ident | Kind::String => Some(Item::Name(Arc::from(c.text()?))),
            _ => None,
        },
        Field::Duration | Field::Delay => time(c).filter(|t| field == Field::Delay || *t >= 0.).map(Item::Time),
        Field::Count => match c.kind() {
            Kind::Ident if c.text()?.eq_ignore_ascii_case("infinite") => Some(Item::Infinite),
            Kind::Number | Kind::Integer => plain_number(c).filter(|n| *n >= 0.).map(Item::Count),
            _ => None,
        },
        Field::Direction => keyword_of(c, DIRECTIONS).map(Item::Keyword),
        Field::Fill => keyword_of(c, FILLS).map(Item::Keyword),
        Field::PlayState => keyword_of(c, STATES).map(Item::Keyword),
        Field::Easing => easing(c).map(Item::Easing),
    }
}
/// One longhand's list.
pub(super) fn parse_list(field: Field, parts: &[Component<'_>]) -> Result<Value> {
    let mut out = Vec::new();
    for group in groups(parts) {
        let [c] = group.as_slice() else { return Err(Error::Invalid) };
        out.push(item(field, c).ok_or(Error::Invalid)?);
    }
    Ok(Value::Animation(Animations(out.into())))
}
/// The longhands in the shorthand's order of their lists.
pub const LONGHANDS: [(Property, Field); 8] = [
    (Property::AnimationName, Field::Name),
    (Property::AnimationDuration, Field::Duration),
    (Property::AnimationTimingFunction, Field::Easing),
    (Property::AnimationDelay, Field::Delay),
    (Property::AnimationIterationCount, Field::Count),
    (Property::AnimationDirection, Field::Direction),
    (Property::AnimationFillMode, Field::Fill),
    (Property::AnimationPlayState, Field::PlayState),
];
/// `animation`: each comma group's parts in any order, the first time its
/// duration and the second its delay, a keyword its easing, count,
/// direction, fill or play state when it can be, and the rest its name.
pub(super) fn parse_shorthand(parts: &[Component<'_>]) -> Result<Assignments> {
    let mut lists: [Vec<Item>; 8] = Default::default();
    for group in groups(parts) {
        let mut slots: [Option<Item>; 8] = Default::default();
        for c in &group {
            let pick = |slots: &[Option<Item>; 8], i: usize| slots[i].is_none();
            let found = if let Some(t) = time(c) {
                if pick(&slots, 1) {
                    Some((1, Item::Time(t.max(0.))))
                } else if pick(&slots, 3) {
                    Some((3, Item::Time(t)))
                } else {
                    None
                }
            } else if let Some(e) = easing(c).filter(|_| pick(&slots, 2)) {
                Some((2, Item::Easing(e)))
            } else if let Some(i) = item(Field::Count, c).filter(|_| pick(&slots, 4)) {
                Some((4, i))
            } else if let Some(i) = item(Field::Direction, c).filter(|_| pick(&slots, 5)) {
                Some((5, i))
            } else if let Some(i) = item(Field::Fill, c).filter(|_| pick(&slots, 6) && !(c.text().is_some_and(|t| t.eq_ignore_ascii_case("none")) && pick(&slots, 0) && !pick(&slots, 6))) {
                Some((6, i))
            } else if let Some(i) = item(Field::PlayState, c).filter(|_| pick(&slots, 7)) {
                Some((7, i))
            } else {
                item(Field::Name, c).filter(|_| pick(&slots, 0)).map(|i| (0, i))
            };
            let (i, value) = found.ok_or(Error::Invalid)?;
            slots[i] = Some(value);
        }
        // `none` taken as the fill when the name was still open: it names none.
        if slots[0].is_none() && slots[6] == Some(Item::Keyword(Keyword::None)) {
            slots[0] = Some(Item::None);
            slots[6] = None;
        }
        let defaults =
            [Item::None, Item::Time(0.), Item::Easing(Easing::EASE), Item::Time(0.), Item::Count(1.), Item::Keyword(Keyword::Normal), Item::Keyword(Keyword::None), Item::Keyword(Keyword::Running)];
        for (i, (slot, default)) in slots.into_iter().zip(defaults).enumerate() {
            lists[i].push(slot.unwrap_or(default));
        }
    }
    Ok(LONGHANDS.iter().zip(lists).map(|((property, _), list)| Assignment { property: *property, value: Value::Animation(Animations(list.into())) }).collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Syntax, properties::Declaration};
    fn longhands(text: &str) -> Vec<(Property, Vec<Item>)> {
        let syntax = Syntax::parse(Arc::from(text), Default::default(), None).unwrap();
        let d = syntax.one_declaration(None).unwrap();
        let Declaration::Known { values, .. } = Declaration::compile(&d, Default::default(), None).unwrap() else { panic!() };
        values.iter().map(|a| (a.property, if let Value::Animation(l) = &a.value { l.items().to_vec() } else { panic!() })).collect()
    }
    #[test]
    fn the_shorthand_splits_by_kind_and_order() {
        let v = longhands("animation: spin 2s linear infinite, fade .5s ease-out 1s forwards");
        let get = |p| v.iter().find(|(q, _)| *q == p).unwrap().1.clone();
        assert_eq!(get(Property::AnimationName), [Item::Name("spin".into()), Item::Name("fade".into())]);
        assert_eq!(get(Property::AnimationDuration), [Item::Time(2.), Item::Time(0.5)]);
        assert_eq!(get(Property::AnimationDelay), [Item::Time(0.), Item::Time(1.)]);
        assert_eq!(get(Property::AnimationIterationCount), [Item::Infinite, Item::Count(1.)]);
        assert_eq!(get(Property::AnimationFillMode), [Item::Keyword(Keyword::None), Item::Keyword(Keyword::Forwards)]);
        assert_eq!(get(Property::AnimationTimingFunction), [Item::Easing(Easing::Linear), Item::Easing(Easing::Bezier([0., 0., 0.58, 1.]))]);
        assert_eq!(longhands("animation: none")[0].1, [Item::None]);
        assert_eq!(longhands("-webkit-animation-duration: 300ms")[0], (Property::AnimationDuration, vec![Item::Time(0.3)]));
    }
    #[test]
    fn easings_follow_their_curves() {
        assert!((Easing::EASE.at(0.5) - 0.8024).abs() < 1e-3);
        assert_eq!(Easing::Steps(4, false).at(0.3), 0.25);
        assert_eq!(Easing::Steps(4, true).at(0.3), 0.5);
        assert_eq!(Easing::Linear.at(2.), 1.);
    }
}
