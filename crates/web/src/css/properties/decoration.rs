//! Box decoration values: corner radii, background images (gradients and
//! URLs) with their size, position and repeat, and box shadows. Colours are
//! generic so one type serves specified (`color::Value`) and computed
//! (`color::Computed`) values.
use super::*;
/// One corner's horizontal and vertical radii.
#[derive(Clone, Debug)]
pub struct Radius {
    pub x: Numeric,
    pub y: Numeric,
}
impl Radius {
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.x.same(&other.x) && self.y.same(&other.y)
    }
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.x.expression().memory().accounted_reserved_bytes + self.y.expression().memory().accounted_reserved_bytes
    }
    pub fn computed(&self, context: &impl math::Context, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        Ok(Self { x: self.x.computed(context, true, limits, cancelled)?, y: self.y.computed(context, true, limits, cancelled)? })
    }
}
#[derive(Clone, Debug)]
pub struct Stop<C> {
    pub color: C,
    /// Along the gradient line; a length-percentage, or none for even spacing.
    pub position: Option<Numeric>,
}
/// Where a linear gradient points: an angle, or a side/corner as unit steps.
#[derive(Clone, Debug)]
pub enum Direction {
    Angle(Numeric),
    To { x: i8, y: i8 },
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadialSize {
    ClosestSide,
    FarthestSide,
    ClosestCorner,
    FarthestCorner,
}
#[derive(Clone, Debug)]
pub enum Image<C> {
    Url(Arc<str>),
    Linear { direction: Direction, stops: Box<[Stop<C>]>, repeating: bool },
    Radial { circle: bool, size: RadialSize, explicit: Option<(Numeric, Numeric)>, at: (Numeric, Numeric), stops: Box<[Stop<C>]>, repeating: bool },
}
/// `background-image`'s layers, first on top. `none` is the keyword.
#[derive(Clone, Debug)]
pub struct Images<C>(pub Arc<[Option<Image<C>>]>);
#[derive(Clone, Debug)]
pub struct Shadow<C> {
    /// None takes the box's `color`.
    pub color: Option<C>,
    pub x: Numeric,
    pub y: Numeric,
    pub blur: Numeric,
    pub spread: Numeric,
    pub inset: bool,
}
#[derive(Clone, Debug)]
pub struct Shadows<C>(pub Arc<[Shadow<C>]>);
#[derive(Clone, Debug)]
pub enum Size {
    Auto,
    Cover,
    Contain,
    /// Width and height; None is auto.
    Explicit(Option<Numeric>, Option<Numeric>),
}
#[derive(Clone, Debug)]
pub struct Sizes(pub Arc<[Size]>);
/// Horizontal and vertical offsets, each a length-percentage (keywords are percentages).
#[derive(Clone, Debug)]
pub struct Positions(pub Arc<[(Numeric, Numeric)]>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatAxis {
    Repeat,
    NoRepeat,
    Space,
    Round,
}
#[derive(Clone, Debug)]
pub struct Repeats(pub Arc<[(RepeatAxis, RepeatAxis)]>);
fn bytes(n: &Numeric) -> usize {
    n.expression().memory().accounted_reserved_bytes
}
fn same_option(a: &Option<Numeric>, b: &Option<Numeric>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same(b),
        (None, None) => true,
        _ => false,
    }
}
impl Direction {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Angle(a), Self::Angle(b)) => a.same(b),
            (Self::To { x: ax, y: ay }, Self::To { x: bx, y: by }) => ax == bx && ay == by,
            _ => false,
        }
    }
}
/// Value comparison of computed images, shadows and the rest: by content,
/// so a value computed anew that reads the same counts as unchanged.
impl Images<crate::css::color::Computed> {
    pub(crate) fn same(&self, other: &Self) -> bool {
        let stops = |a: &[Stop<crate::css::color::Computed>], b: &[Stop<crate::css::color::Computed>]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.color.same(&b.color) && same_option(&a.position, &b.position))
        };
        Arc::ptr_eq(&self.0, &other.0)
            || self.0.len() == other.0.len()
                && self.0.iter().zip(other.0.iter()).all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(Image::Url(a)), Some(Image::Url(b))) => a == b,
                    (Some(Image::Linear { direction: da, stops: sa, repeating: ra }), Some(Image::Linear { direction: db, stops: sb, repeating: rb })) => ra == rb && da.same(db) && stops(sa, sb),
                    (
                        Some(Image::Radial { circle: ca, size: za, explicit: ea, at: aa, stops: sa, repeating: ra }),
                        Some(Image::Radial { circle: cb, size: zb, explicit: eb, at: ab, stops: sb, repeating: rb }),
                    ) => {
                        ca == cb
                            && za == zb
                            && ra == rb
                            && match (ea, eb) {
                                (Some((ax, ay)), Some((bx, by))) => ax.same(bx) && ay.same(by),
                                (None, None) => true,
                                _ => false,
                            }
                            && aa.0.same(&ab.0)
                            && aa.1.same(&ab.1)
                            && stops(sa, sb)
                    }
                    _ => false,
                })
    }
}
impl Shadows<crate::css::color::Computed> {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || self.0.len() == other.0.len()
                && self.0.iter().zip(other.0.iter()).all(|(a, b)| {
                    a.inset == b.inset
                        && a.x.same(&b.x)
                        && a.y.same(&b.y)
                        && a.blur.same(&b.blur)
                        && a.spread.same(&b.spread)
                        && match (&a.color, &b.color) {
                            (Some(a), Some(b)) => a.same(b),
                            (None, None) => true,
                            _ => false,
                        }
                })
    }
}
impl Sizes {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || self.0.len() == other.0.len()
                && self.0.iter().zip(other.0.iter()).all(|(a, b)| match (a, b) {
                    (Size::Auto, Size::Auto) | (Size::Cover, Size::Cover) | (Size::Contain, Size::Contain) => true,
                    (Size::Explicit(aw, ah), Size::Explicit(bw, bh)) => same_option(aw, bw) && same_option(ah, bh),
                    _ => false,
                })
    }
}
impl Positions {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.len() == other.0.len() && self.0.iter().zip(other.0.iter()).all(|(a, b)| a.0.same(&b.0) && a.1.same(&b.1))
    }
}
impl Repeats {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.len() == other.0.len() && self.0.iter().zip(other.0.iter()).all(|(a, b)| a == b)
    }
}
impl<C> Images<C> {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Option<Image<C>>>()
            + self
                .0
                .iter()
                .flatten()
                .map(|i| match i {
                    Image::Url(u) => u.len(),
                    Image::Linear { stops, .. } | Image::Radial { stops, .. } => stops.len() * size_of::<Stop<C>>() + stops.iter().filter_map(|s| s.position.as_ref()).map(bytes).sum::<usize>(),
                })
                .sum::<usize>()
    }
}
impl Images<color::Value> {
    pub fn computed(&self, colors: &impl color::Context, numeric: &impl math::Context, math: math::Limits, limits: color::Limits, cancelled: Option<&AtomicBool>) -> Result<Images<color::Computed>> {
        let stop = |s: &Stop<color::Value>| -> Result<Stop<color::Computed>> {
            Ok(Stop {
                color: s.color.computed(colors, limits, cancelled).map_err(super::parse::color_error)?,
                position: s.position.as_ref().map(|p| p.computed(numeric, true, math, cancelled)).transpose()?,
            })
        };
        let mut out = Vec::with_capacity(self.0.len());
        for image in self.0.iter() {
            check(cancelled)?;
            out.push(match image {
                None => None,
                Some(Image::Url(u)) => Some(Image::Url(u.clone())),
                Some(Image::Linear { direction, stops, repeating }) => Some(Image::Linear {
                    direction: match direction {
                        Direction::Angle(a) => Direction::Angle(a.computed(numeric, false, math, cancelled)?),
                        Direction::To { x, y } => Direction::To { x: *x, y: *y },
                    },
                    stops: stops.iter().map(stop).collect::<Result<Vec<_>>>()?.into(),
                    repeating: *repeating,
                }),
                Some(Image::Radial { circle, size, explicit, at, stops, repeating }) => Some(Image::Radial {
                    circle: *circle,
                    size: *size,
                    explicit: explicit.as_ref().map(|(w, h)| Ok::<_, Error>((w.computed(numeric, true, math, cancelled)?, h.computed(numeric, true, math, cancelled)?))).transpose()?,
                    at: (at.0.computed(numeric, true, math, cancelled)?, at.1.computed(numeric, true, math, cancelled)?),
                    stops: stops.iter().map(stop).collect::<Result<Vec<_>>>()?.into(),
                    repeating: *repeating,
                }),
            });
        }
        Ok(Images(out.into()))
    }
}
impl<C> Shadows<C> {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Shadow<C>>() + self.0.iter().map(|s| bytes(&s.x) + bytes(&s.y) + bytes(&s.blur) + bytes(&s.spread)).sum::<usize>()
    }
}
impl Shadows<color::Value> {
    pub fn computed(&self, colors: &impl color::Context, numeric: &impl math::Context, math: math::Limits, limits: color::Limits, cancelled: Option<&AtomicBool>) -> Result<Shadows<color::Computed>> {
        let mut out = Vec::with_capacity(self.0.len());
        for s in self.0.iter() {
            check(cancelled)?;
            out.push(Shadow {
                color: s.color.as_ref().map(|c| c.computed(colors, limits, cancelled).map_err(super::parse::color_error)).transpose()?,
                x: s.x.computed(numeric, false, math, cancelled)?,
                y: s.y.computed(numeric, false, math, cancelled)?,
                blur: s.blur.computed(numeric, false, math, cancelled)?,
                spread: s.spread.computed(numeric, false, math, cancelled)?,
                inset: s.inset,
            });
        }
        Ok(Shadows(out.into()))
    }
}
impl Sizes {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Size>()
    }
    pub fn computed(&self, numeric: &impl math::Context, math: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut out = Vec::with_capacity(self.0.len());
        for s in self.0.iter() {
            out.push(match s {
                Size::Explicit(w, h) => {
                    Size::Explicit(w.as_ref().map(|v| v.computed(numeric, true, math, cancelled)).transpose()?, h.as_ref().map(|v| v.computed(numeric, true, math, cancelled)).transpose()?)
                }
                other => other.clone(),
            });
        }
        Ok(Self(out.into()))
    }
}
impl Positions {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<(Numeric, Numeric)>()
    }
    pub fn computed(&self, numeric: &impl math::Context, math: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut out = Vec::with_capacity(self.0.len());
        for (x, y) in self.0.iter() {
            out.push((x.computed(numeric, true, math, cancelled)?, y.computed(numeric, true, math, cancelled)?));
        }
        Ok(Self(out.into()))
    }
}
impl Repeats {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * 2
    }
}
fn is(c: Component<'_>, word: &str) -> bool {
    c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case(word)
}
fn length(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Numeric> {
    Numeric::compile(c, NumericKind::LengthPercentage, Range::Nonnegative, limits, cancelled)
}
fn length_any(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Numeric> {
    Numeric::compile(c, NumericKind::LengthPercentage, Range::All, limits, cancelled)
}
fn percent(value: f64) -> Numeric {
    Numeric::scalar(value, math::Dimension::Percentage, Range::All)
}
/// `<length-percentage>{1,2}` for one corner.
pub(super) fn parse_radius(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    match parts {
        [one] => {
            let x = length(*one, limits, cancelled)?;
            Ok(Value::Radius(Radius { x: x.clone(), y: x }))
        }
        [x, y] => Ok(Value::Radius(Radius { x: length(*x, limits, cancelled)?, y: length(*y, limits, cancelled)? })),
        _ => Err(Error::Invalid),
    }
}
/// The `border-radius` shorthand: up to four horizontal radii, `/`, up to four vertical.
pub(super) fn parse_radius_shorthand(parts: &[Component<'_>], properties: &[Property], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    let (horizontal, vertical) = match parts.iter().position(|c| c.character() == Some('/')) {
        Some(i) => (&parts[..i], Some(&parts[i + 1..])),
        None => (parts, None),
    };
    let four = |group: &[Component<'_>]| -> Result<[Numeric; 4]> {
        let values: Vec<Numeric> = group.iter().map(|c| length(*c, limits, cancelled)).collect::<Result<_>>()?;
        Ok(match values.as_slice() {
            [a] => [a.clone(), a.clone(), a.clone(), a.clone()],
            [a, b] => [a.clone(), b.clone(), a.clone(), b.clone()],
            [a, b, c] => [a.clone(), b.clone(), c.clone(), b.clone()],
            [a, b, c, d] => [a.clone(), b.clone(), c.clone(), d.clone()],
            _ => return Err(Error::Invalid),
        })
    };
    let h = four(horizontal)?;
    let v = match vertical {
        Some(v) => four(v)?,
        None => h.clone(),
    };
    // top-left, top-right, bottom-right, bottom-left
    Ok(properties.iter().zip(h.into_iter().zip(v)).map(|(&property, (x, y))| Assignment { property, value: Value::Radius(Radius { x, y }) }).collect())
}
/// `<color-stop-list>` after the gradient's leading arguments.
fn stops(groups: &[&[Component<'_>]], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Box<[Stop<color::Value>]>> {
    let mut out = Vec::new();
    for group in groups {
        check(cancelled)?;
        let mut color = None;
        let mut positions = Vec::new();
        for c in group.iter() {
            if matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage | Kind::Number | Kind::Integer)
                || c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("calc")
            {
                positions.push(length_any(*c, limits, cancelled)?);
            } else if color.is_none() {
                let values = c.children().between(c.at, c.after());
                color = Some(color::Value::parse(values, limits.color(), cancelled).map_err(super::parse::color_error)?);
            } else {
                return Err(Error::Invalid);
            }
        }
        match (color, positions.len()) {
            (Some(color), 0) => out.push(Stop { color, position: None }),
            (Some(color), 1) => out.push(Stop { color, position: positions.pop() }),
            (Some(color), 2) => {
                // Two positions: the same colour twice (CSS Images 4).
                let second = positions.pop();
                let first = positions.pop();
                out.push(Stop { color: color.clone(), position: first });
                out.push(Stop { color, position: second });
            }
            // A bare hint between colours: kept as a position-only stop later; skip now.
            (None, 1) => {}
            _ => return Err(Error::Invalid),
        }
        if out.len() > 256 {
            return Err(Error::Limit);
        }
    }
    if out.len() < 2 {
        return Err(Error::Invalid);
    }
    Ok(out.into())
}
fn split_commas<'a, 'b>(parts: &'b [Component<'a>]) -> Vec<&'b [Component<'a>]> {
    parts.split(|c| c.kind() == Kind::Comma).collect()
}
fn gradient(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Option<Image<color::Value>>> {
    let name = c.text().unwrap().to_ascii_lowercase();
    let name = name.strip_prefix("-webkit-").unwrap_or(&name);
    let (repeating, kind) = match name.strip_prefix("repeating-") {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    if !matches!(kind, "linear-gradient" | "radial-gradient") {
        return Ok(None);
    }
    let parts: Vec<_> = c.children().filter(|c| c.kind() != Kind::Whitespace).collect();
    let groups = split_commas(&parts);
    if groups.is_empty() {
        return Err(Error::Invalid);
    }
    if kind == "linear-gradient" {
        let first = groups[0];
        let mut direction = Direction::To { x: 0, y: 1 };
        let mut skip = 0;
        if let [angle] = first
            && matches!(angle.kind(), Kind::Dimension | Kind::IntegerDimension)
            && angle.unit().is_some_and(|u| ["deg", "grad", "rad", "turn"].iter().any(|k| u.eq_ignore_ascii_case(k)))
        {
            direction = Direction::Angle(Numeric::compile(*angle, NumericKind::Angle, Range::All, limits, cancelled)?);
            skip = 1;
        } else if let [zero] = first
            && zero.number().is_some_and(|n| n.value == 0.)
            && matches!(zero.kind(), Kind::Number | Kind::Integer)
        {
            direction = Direction::Angle(Numeric::scalar(0., math::Dimension::Angle, Range::All));
            skip = 1;
        } else if first.first().is_some_and(|c| is(*c, "to")) {
            let (mut x, mut y) = (0i8, 0i8);
            for side in &first[1..] {
                if is(*side, "left") {
                    x = -1;
                } else if is(*side, "right") {
                    x = 1;
                } else if is(*side, "top") {
                    y = -1;
                } else if is(*side, "bottom") {
                    y = 1;
                } else {
                    return Err(Error::Invalid);
                }
            }
            if x == 0 && y == 0 || first.len() > 3 {
                return Err(Error::Invalid);
            }
            direction = Direction::To { x, y };
            skip = 1;
        } else if first.iter().any(|c| matches!(c.kind(), Kind::Ident) && c.text().is_some_and(|t| t.eq_ignore_ascii_case("in"))) {
            // Colour interpolation methods: the default is used.
            skip = 1;
        }
        return Ok(Some(Image::Linear { direction, stops: stops(&groups[skip..], limits, cancelled)?, repeating }));
    }
    // radial-gradient([ <shape> || <size> ]? [ at <position> ]?, stops)
    let first = groups[0];
    let mut circle = false;
    let mut size = RadialSize::FarthestCorner;
    let mut explicit = None;
    let mut at = (percent(50.), percent(50.));
    let mut skip = 0;
    let has_prelude = first.iter().any(|c| {
        c.kind() == Kind::Ident
            && !is(*c, "transparent")
            && !is(*c, "currentcolor")
            && (is(*c, "circle") || is(*c, "ellipse") || is(*c, "at") || is(*c, "closest-side") || is(*c, "farthest-side") || is(*c, "closest-corner") || is(*c, "farthest-corner"))
    }) || !first.is_empty() && first.iter().all(|c| matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage)) && groups.len() > 2;
    if has_prelude {
        skip = 1;
        let mut i = 0;
        let mut lengths = Vec::new();
        while i < first.len() {
            let c = first[i];
            if is(c, "circle") {
                circle = true;
            } else if is(c, "ellipse") {
                circle = false;
            } else if is(c, "closest-side") {
                size = RadialSize::ClosestSide;
            } else if is(c, "farthest-side") {
                size = RadialSize::FarthestSide;
            } else if is(c, "closest-corner") {
                size = RadialSize::ClosestCorner;
            } else if is(c, "farthest-corner") {
                size = RadialSize::FarthestCorner;
            } else if is(c, "at") {
                at = position(&first[i + 1..], limits, cancelled)?;
                break;
            } else if matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage) {
                lengths.push(length(c, limits, cancelled)?);
            } else {
                return Err(Error::Invalid);
            }
            i += 1;
        }
        match lengths.as_slice() {
            [] => {}
            [r] => {
                circle = true;
                explicit = Some((r.clone(), r.clone()));
            }
            [w, h] => explicit = Some((w.clone(), h.clone())),
            _ => return Err(Error::Invalid),
        }
    }
    Ok(Some(Image::Radial { circle, size, explicit, at, stops: stops(&groups[skip..], limits, cancelled)?, repeating }))
}
/// `<bg-position>`: keywords and lengths, as a pair of length-percentages.
fn position(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<(Numeric, Numeric)> {
    let mut x = None;
    let mut y = None;
    let mut values = Vec::new();
    for c in parts {
        if is(*c, "left") {
            x = Some(percent(0.));
        } else if is(*c, "right") {
            x = Some(percent(100.));
        } else if is(*c, "top") {
            y = Some(percent(0.));
        } else if is(*c, "bottom") {
            y = Some(percent(100.));
        } else if is(*c, "center") {
            values.push(percent(50.));
        } else {
            values.push(length_any(*c, limits, cancelled)?);
        }
    }
    if parts.len() > 4 {
        return Err(Error::Invalid);
    }
    let mut values = values.into_iter();
    let x = match x {
        Some(x) => x,
        None => values.next().unwrap_or_else(|| percent(50.)),
    };
    let y = match y {
        Some(y) => y,
        None => values.next().unwrap_or_else(|| percent(50.)),
    };
    Ok((x, y))
}
fn image(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Option<Image<color::Value>>> {
    match c.kind() {
        Kind::Url => Ok(Some(Image::Url(Arc::from(c.text().unwrap())))),
        Kind::Function if c.text().unwrap().eq_ignore_ascii_case("url") || c.text().unwrap().eq_ignore_ascii_case("src") => {
            Ok(c.children().find(|c| c.kind() == Kind::String).and_then(|c| c.text()).map(|t| Image::Url(Arc::from(t))))
        }
        Kind::Function => gradient(c, limits, cancelled),
        _ => Ok(None),
    }
}
/// `background-image`: `none | <image>`, comma-separated layers.
pub(super) fn parse_images(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let mut out = Vec::new();
    for group in split_commas(parts) {
        check(cancelled)?;
        match group {
            [c] if is(*c, "none") => out.push(None),
            [c] => match image(*c, limits, cancelled)? {
                Some(i) => out.push(Some(i)),
                None => return Err(if c.kind() == Kind::Function { Error::Unsupported("image function") } else { Error::Invalid }),
            },
            _ => return Err(Error::Invalid),
        }
        if out.len() > 64 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    if out.len() == 1 && out[0].is_none() {
        return Ok(Value::Keyword(Keyword::None));
    }
    Ok(Value::Images(Images(out.into())))
}
pub(super) fn parse_sizes(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let mut out = Vec::new();
    for group in split_commas(parts) {
        out.push(match group {
            [c] if is(*c, "cover") => Size::Cover,
            [c] if is(*c, "contain") => Size::Contain,
            [c] if is(*c, "auto") => Size::Auto,
            [w] => Size::Explicit(Some(length(*w, limits, cancelled)?), None),
            [w, h] => Size::Explicit(if is(*w, "auto") { None } else { Some(length(*w, limits, cancelled)?) }, if is(*h, "auto") { None } else { Some(length(*h, limits, cancelled)?) }),
            _ => return Err(Error::Invalid),
        });
        if out.len() > 64 {
            return Err(Error::Limit);
        }
    }
    Ok(Value::Sizes(Sizes(out.into())))
}
pub(super) fn parse_positions(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let mut out = Vec::new();
    for group in split_commas(parts) {
        if group.is_empty() {
            return Err(Error::Invalid);
        }
        out.push(position(group, limits, cancelled)?);
        if out.len() > 64 {
            return Err(Error::Limit);
        }
    }
    Ok(Value::Positions(Positions(out.into())))
}
pub(super) fn parse_repeats(parts: &[Component<'_>]) -> Result<Value> {
    let axis = |c: Component<'_>| {
        if is(c, "repeat") {
            Some(RepeatAxis::Repeat)
        } else if is(c, "no-repeat") {
            Some(RepeatAxis::NoRepeat)
        } else if is(c, "space") {
            Some(RepeatAxis::Space)
        } else if is(c, "round") {
            Some(RepeatAxis::Round)
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for group in split_commas(parts) {
        out.push(match group {
            [c] if is(*c, "repeat-x") => (RepeatAxis::Repeat, RepeatAxis::NoRepeat),
            [c] if is(*c, "repeat-y") => (RepeatAxis::NoRepeat, RepeatAxis::Repeat),
            [c] => {
                let a = axis(*c).ok_or(Error::Invalid)?;
                (a, a)
            }
            [x, y] => (axis(*x).ok_or(Error::Invalid)?, axis(*y).ok_or(Error::Invalid)?),
            _ => return Err(Error::Invalid),
        });
        if out.len() > 64 {
            return Err(Error::Limit);
        }
    }
    Ok(Value::Repeats(Repeats(out.into())))
}
/// A `filter` or `backdrop-filter` function (Filter Effects 1 §13):
/// `blur()` (its standard deviation), the colour functions by their
/// amounts (1 or 100% as they are; `hue-rotate` in degrees), and those
/// not drawn yet (`drop-shadow()`, `url()`), kept to be said not to be.
#[derive(Clone, Debug)]
pub enum Filter {
    Blur(Numeric),
    Saturate(f64),
    Brightness(f64),
    Contrast(f64),
    Grayscale(f64),
    HueRotate(f64),
    Invert(f64),
    Opacity(f64),
    Sepia(f64),
    Other,
}
/// The 4x5 matrix (rows R, G, B, A over unpremultiplied R, G, B, A, 1)
/// the colour functions of a filter list make, composed in their order;
/// none when they leave colours as they are (or there are none).
pub fn color_matrix(filters: &[Filter]) -> Option<[f32; 20]> {
    let identity = |m: &mut [f64; 20]| {
        *m = [0.; 20];
        m[0] = 1.;
        m[6] = 1.;
        m[12] = 1.;
        m[18] = 1.;
    };
    let mut total = [0f64; 20];
    identity(&mut total);
    let mut any = false;
    for f in filters {
        let mut m = [0f64; 20];
        identity(&mut m);
        // Filter Effects 1 §13 and §15's matrices.
        let rgb = |m: &mut [f64; 20], k: [f64; 9]| {
            for r in 0..3 {
                for c in 0..3 {
                    m[r * 5 + c] = k[r * 3 + c];
                }
            }
        };
        match *f {
            Filter::Saturate(s) => {
                rgb(&mut m, [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s])
            }
            Filter::Grayscale(g) => {
                let a = 1. - g.clamp(0., 1.);
                rgb(
                    &mut m,
                    [
                        0.2126 + 0.7874 * a,
                        0.7152 - 0.7152 * a,
                        0.0722 - 0.0722 * a,
                        0.2126 - 0.2126 * a,
                        0.7152 + 0.2848 * a,
                        0.0722 - 0.0722 * a,
                        0.2126 - 0.2126 * a,
                        0.7152 - 0.7152 * a,
                        0.0722 + 0.9278 * a,
                    ],
                )
            }
            Filter::Sepia(v) => {
                let a = 1. - v.clamp(0., 1.);
                rgb(&mut m, [0.393 + 0.607 * a, 0.769 - 0.769 * a, 0.189 - 0.189 * a, 0.349 - 0.349 * a, 0.686 + 0.314 * a, 0.168 - 0.168 * a, 0.272 - 0.272 * a, 0.534 - 0.534 * a, 0.131 + 0.869 * a])
            }
            Filter::HueRotate(deg) => {
                let (s, c) = deg.to_radians().sin_cos();
                rgb(
                    &mut m,
                    [
                        0.213 + c * 0.787 - s * 0.213,
                        0.715 - c * 0.715 - s * 0.715,
                        0.072 - c * 0.072 + s * 0.928,
                        0.213 - c * 0.213 + s * 0.143,
                        0.715 + c * 0.285 + s * 0.140,
                        0.072 - c * 0.072 - s * 0.283,
                        0.213 - c * 0.213 - s * 0.787,
                        0.715 - c * 0.715 + s * 0.715,
                        0.072 + c * 0.928 + s * 0.072,
                    ],
                )
            }
            Filter::Brightness(b) => {
                for i in [0, 6, 12] {
                    m[i] = b;
                }
            }
            Filter::Contrast(c) => {
                for (i, o) in [(0, 4), (6, 9), (12, 14)] {
                    m[i] = c;
                    m[o] = 0.5 - 0.5 * c;
                }
            }
            Filter::Invert(v) => {
                let v = v.clamp(0., 1.);
                for (i, o) in [(0, 4), (6, 9), (12, 14)] {
                    m[i] = 1. - 2. * v;
                    m[o] = v;
                }
            }
            Filter::Opacity(o) => m[18] = o.clamp(0., 1.),
            Filter::Blur(_) | Filter::Other => continue,
        }
        any = true;
        // total = m after total: m x total (4x5 as 5x5 with a last row of 0 0 0 0 1).
        let mut next = [0f64; 20];
        for r in 0..4 {
            for c in 0..5 {
                let mut v = if c == 4 { m[r * 5 + 4] } else { 0. };
                for k in 0..4 {
                    v += m[r * 5 + k] * total[k * 5 + c];
                }
                next[r * 5 + c] = v;
            }
        }
        total = next;
    }
    let mut identity_matrix = [0f64; 20];
    identity(&mut identity_matrix);
    (any && total.iter().zip(identity_matrix.iter()).any(|(a, b)| (a - b).abs() > 1e-6)).then(|| total.map(|v| v as f32))
}
#[derive(Clone, Debug)]
pub struct Filters(pub Arc<[Filter]>);
impl Filters {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Filter>() + self.0.iter().map(|f| if let Filter::Blur(n) = f { bytes(n) } else { 0 }).sum::<usize>()
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().zip(other.0.iter()).all(|(a, b)| match (a, b) {
                (Filter::Blur(x), Filter::Blur(y)) => x.same(y),
                (Filter::Saturate(x), Filter::Saturate(y))
                | (Filter::Brightness(x), Filter::Brightness(y))
                | (Filter::Contrast(x), Filter::Contrast(y))
                | (Filter::Grayscale(x), Filter::Grayscale(y))
                | (Filter::HueRotate(x), Filter::HueRotate(y))
                | (Filter::Invert(x), Filter::Invert(y))
                | (Filter::Opacity(x), Filter::Opacity(y))
                | (Filter::Sepia(x), Filter::Sepia(y)) => x == y,
                (Filter::Other, Filter::Other) => true,
                _ => false,
            })
    }
    pub fn computed(&self, numeric: &impl math::Context, math: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut out = Vec::with_capacity(self.0.len());
        for f in self.0.iter() {
            check(cancelled)?;
            out.push(match f {
                Filter::Blur(n) => Filter::Blur(n.computed(numeric, false, math, cancelled)?),
                other => other.clone(),
            });
        }
        Ok(Self(out.into()))
    }
}
/// An amount: a number, or a percentage of 1 (nonnegative); `default`
/// when none is given.
fn amount(args: &[Component<'_>], default: f64) -> Result<f64> {
    match args {
        [] => Ok(default),
        [a] if matches!(a.kind(), Kind::Number | Kind::Integer) => Ok(a.number().ok_or(Error::Invalid)?.value),
        [a] if matches!(a.kind(), Kind::Percentage | Kind::IntegerPercentage) => Ok(a.number().ok_or(Error::Invalid)?.value / 100.),
        _ => Err(Error::Invalid),
    }
    .and_then(|v| if v < 0. { Err(Error::Invalid) } else { Ok(v) })
}
/// `filter` and `backdrop-filter`: `none | <filter-function>+` (Filter
/// Effects 1 §6, 2 §2).
pub(super) fn parse_filters(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    if let [c] = parts
        && is(*c, "none")
    {
        return Ok(Value::Keyword(Keyword::None));
    }
    let mut out = Vec::new();
    for c in parts {
        check(cancelled)?;
        if c.kind() != Kind::Function {
            return Err(Error::Invalid);
        }
        let name = c.text().unwrap_or("").to_ascii_lowercase();
        let args: Vec<Component<'_>> = c.children().filter(|a| a.kind() != Kind::Whitespace).collect();
        out.push(match name.as_str() {
            "blur" => match args.as_slice() {
                [] => Filter::Blur(Numeric::scalar(0., math::Dimension::Length, Range::Nonnegative)),
                [a] => Filter::Blur(Numeric::compile(*a, NumericKind::Length, Range::Nonnegative, limits, cancelled)?),
                _ => return Err(Error::Invalid),
            },
            "saturate" => Filter::Saturate(amount(&args, 1.)?),
            "brightness" => Filter::Brightness(amount(&args, 1.)?),
            "contrast" => Filter::Contrast(amount(&args, 1.)?),
            // Amounts past 100% are 100%.
            "grayscale" => Filter::Grayscale(amount(&args, 1.)?.min(1.)),
            "invert" => Filter::Invert(amount(&args, 1.)?.min(1.)),
            "opacity" => Filter::Opacity(amount(&args, 1.)?.min(1.)),
            "sepia" => Filter::Sepia(amount(&args, 1.)?.min(1.)),
            "hue-rotate" => match args.as_slice() {
                [] => Filter::HueRotate(0.),
                [a] if matches!(a.kind(), Kind::Number | Kind::Integer) && a.number().is_some_and(|n| n.value == 0.) => Filter::HueRotate(0.),
                // An angle (its constant in degrees), as rotate() takes one.
                [a] => Filter::HueRotate(Numeric::compile(*a, NumericKind::Angle, Range::All, limits, cancelled)?.expression().constant().ok_or(Error::Invalid)?.value),
                _ => return Err(Error::Invalid),
            },
            "drop-shadow" | "url" => Filter::Other,
            _ => return Err(Error::Invalid),
        });
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Filters(Filters(out.into())))
}
/// `box-shadow`: `none | <shadow>#`.
pub(super) fn parse_shadows(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    if let [c] = parts
        && is(*c, "none")
    {
        return Ok(Value::Keyword(Keyword::None));
    }
    let mut out = Vec::new();
    for group in split_commas(parts) {
        check(cancelled)?;
        let mut color = None;
        let mut inset = false;
        let mut lengths = Vec::new();
        for c in group {
            if is(*c, "inset") {
                inset = true;
            } else if matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Number | Kind::Integer) || c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("calc") {
                lengths.push(Numeric::compile(*c, NumericKind::Length, Range::All, limits, cancelled)?);
            } else if color.is_none() {
                let values = c.children().between(c.at, c.after());
                color = Some(color::Value::parse(values, limits.color(), cancelled).map_err(super::parse::color_error)?);
            } else {
                return Err(Error::Invalid);
            }
        }
        if lengths.len() < 2 || lengths.len() > 4 {
            return Err(Error::Invalid);
        }
        let zero = || Numeric::scalar(0., math::Dimension::Length, Range::All);
        let mut it = lengths.into_iter();
        let (x, y) = (it.next().unwrap(), it.next().unwrap());
        let blur = it.next().unwrap_or_else(zero);
        let spread = it.next().unwrap_or_else(zero);
        out.push(Shadow { color, x, y, blur, spread, inset });
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Shadows(Shadows(out.into())))
}
/// The `background` shorthand: per layer an image, a position (with `/ size`),
/// a repeat and attachment/box keywords; a colour on the last layer only.
pub(super) fn parse_background(parts: &[Component<'_>], properties: &[Property], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    let groups = split_commas(parts);
    let mut images = Vec::new();
    let mut positions = Vec::new();
    let mut sizes = Vec::new();
    let mut repeats = Vec::new();
    let mut color = None;
    let mut clip = None;
    for (index, group) in groups.iter().enumerate() {
        check(cancelled)?;
        let last = index + 1 == groups.len();
        let mut layer_image = None;
        let mut layer_position = None;
        let mut layer_size = None;
        let mut layer_repeat = None;
        let mut boxes = Vec::new();
        let mut i = 0;
        while i < group.len() {
            let c = group[i];
            if c.character() == Some('/') {
                // Size follows a position.
                let mut j = i + 1;
                let mut size_parts = Vec::new();
                while j < group.len()
                    && (is(group[j], "auto")
                        || is(group[j], "cover")
                        || is(group[j], "contain")
                        || matches!(group[j].kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage | Kind::Number | Kind::Integer))
                    && size_parts.len() < 2
                {
                    size_parts.push(group[j]);
                    j += 1;
                }
                if size_parts.is_empty() || layer_position.is_none() {
                    return Err(Error::Invalid);
                }
                let Value::Sizes(s) = parse_sizes(&size_parts, limits, cancelled)? else { return Err(Error::Invalid) };
                layer_size = Some(s.0[0].clone());
                i = j;
                continue;
            }
            if is(c, "none")
                || c.kind() == Kind::Url
                || c.kind() == Kind::Function
                    && !c.text().unwrap().eq_ignore_ascii_case("calc")
                    && !c.text().unwrap().eq_ignore_ascii_case("var")
                    && image(c, limits, cancelled).map(|i| i.is_some()).unwrap_or(false)
            {
                if layer_image.is_some() {
                    return Err(Error::Invalid);
                }
                layer_image = Some(if is(c, "none") { None } else { image(c, limits, cancelled)? });
                i += 1;
                continue;
            }
            if is(c, "repeat") || is(c, "no-repeat") || is(c, "repeat-x") || is(c, "repeat-y") || is(c, "space") || is(c, "round") {
                let mut j = i + 1;
                let mut rep = vec![c];
                if j < group.len() && (is(group[j], "repeat") || is(group[j], "no-repeat") || is(group[j], "space") || is(group[j], "round")) && !is(c, "repeat-x") && !is(c, "repeat-y") {
                    rep.push(group[j]);
                    j += 1;
                }
                let Value::Repeats(r) = parse_repeats(&rep)? else { return Err(Error::Invalid) };
                layer_repeat = Some(r.0[0]);
                i = j;
                continue;
            }
            if is(c, "scroll") || is(c, "fixed") || is(c, "local") {
                i += 1;
                continue;
            }
            if is(c, "border-box") || is(c, "padding-box") || is(c, "content-box") || is(c, "text") {
                boxes.push(c);
                i += 1;
                continue;
            }
            if is(c, "left")
                || is(c, "right")
                || is(c, "top")
                || is(c, "bottom")
                || is(c, "center")
                || matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage | Kind::Number | Kind::Integer)
            {
                let mut j = i;
                let mut pos = Vec::new();
                while j < group.len()
                    && pos.len() < 4
                    && (is(group[j], "left")
                        || is(group[j], "right")
                        || is(group[j], "top")
                        || is(group[j], "bottom")
                        || is(group[j], "center")
                        || matches!(group[j].kind(), Kind::Dimension | Kind::IntegerDimension | Kind::Percentage | Kind::IntegerPercentage | Kind::Number | Kind::Integer))
                {
                    pos.push(group[j]);
                    j += 1;
                }
                layer_position = Some(position(&pos, limits, cancelled)?);
                i = j;
                continue;
            }
            // Anything else is the colour, allowed on the last layer only.
            if !last || color.is_some() {
                return Err(Error::Invalid);
            }
            let values = c.children().between(c.at, c.after());
            color = Some(color::Value::parse(values, limits.color(), cancelled).map_err(super::parse::color_error)?);
            i += 1;
        }
        if let Some(b) = boxes.last() {
            clip = Some(if is(*b, "text") {
                Keyword::Text
            } else if is(*b, "padding-box") {
                Keyword::PaddingBox
            } else if is(*b, "content-box") {
                Keyword::ContentBox
            } else {
                Keyword::BorderBox
            });
        }
        images.push(layer_image.unwrap_or(None));
        positions.push(layer_position.unwrap_or_else(|| (percent(0.), percent(0.))));
        sizes.push(layer_size.unwrap_or(Size::Auto));
        repeats.push(layer_repeat.unwrap_or((RepeatAxis::Repeat, RepeatAxis::Repeat)));
    }
    let image_value = if images.iter().all(Option::is_none) { Value::Keyword(Keyword::None) } else { Value::Images(Images(images.into())) };
    let mut out = Assignments::new();
    for &property in properties {
        let value = match property {
            Property::BackgroundColor => color.clone().map_or(Value::Wide(variables::Wide::Initial), Value::Color),
            Property::BackgroundImage | Property::MaskImage => image_value.clone(),
            Property::BackgroundPosition | Property::MaskPosition => Value::Positions(Positions(positions.clone().into())),
            Property::BackgroundSize | Property::MaskSize => Value::Sizes(Sizes(sizes.clone().into())),
            Property::BackgroundRepeat | Property::MaskRepeat => Value::Repeats(Repeats(repeats.clone().into())),
            Property::BackgroundClip => clip.map_or(Value::Wide(variables::Wide::Initial), Value::Keyword),
            _ => Value::Wide(variables::Wide::Initial),
        };
        out.push(Assignment { property, value });
    }
    Ok(out)
}
#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::css::{Syntax, properties::Declaration};
    fn filters(text: &str) -> Option<(Property, Vec<Filter>)> {
        let syntax = Syntax::parse(Arc::from(text), Default::default(), None).unwrap();
        let d = syntax.one_declaration(None).unwrap();
        let Declaration::Known { values, .. } = Declaration::compile(&d, Default::default(), None).ok()? else { return None };
        let a = values.first()?;
        match &a.value {
            Value::Filters(f) => Some((a.property, f.0.to_vec())),
            _ => None,
        }
    }
    #[test]
    fn backdrop_filters_keep_blur_and_saturate_in_order() {
        let (p, f) = filters("backdrop-filter: saturate(180%) blur(20px)").unwrap();
        assert_eq!(p, Property::BackdropFilter);
        assert!(matches!(f[0], Filter::Saturate(s) if (s - 1.8).abs() < 1e-9));
        assert!(matches!(&f[1], Filter::Blur(n) if n.expression().constant().is_some_and(|q| q.value == 20.)));
        let (p, f) = filters("-webkit-backdrop-filter: saturate(1.5) brightness(1.1)").unwrap();
        assert_eq!(p, Property::BackdropFilter);
        assert!(matches!(f[0], Filter::Saturate(s) if (s - 1.5).abs() < 1e-9) && matches!(f[1], Filter::Brightness(b) if (b - 1.1).abs() < 1e-9));
        assert!(filters("backdrop-filter: blur(4px) wobble(2)").is_none());
    }
    #[test]
    fn colour_filters_make_one_matrix() {
        let m = |text: &str| color_matrix(&filters(text).unwrap().1);
        let apply = |m: [f32; 20], c: [f32; 4]| -> [f32; 4] { std::array::from_fn(|r| (0..4).map(|k| m[r * 5 + k] * c[k]).sum::<f32>() + m[r * 5 + 4]) };
        assert!(m("filter: grayscale(0) brightness(100%)").is_none(), "identity");
        let gray = apply(m("filter: grayscale(1)").unwrap(), [1., 0., 0., 1.]);
        assert!((gray[0] - 0.2126).abs() < 1e-4 && (gray[1] - 0.2126).abs() < 1e-4 && (gray[2] - 0.2126).abs() < 1e-4);
        let inverted = apply(m("filter: invert(100%)").unwrap(), [0.2, 0.5, 1., 1.]);
        assert!((inverted[0] - 0.8).abs() < 1e-4 && (inverted[1] - 0.5).abs() < 1e-4 && inverted[2].abs() < 1e-4);
        // In order: brightness(2) then contrast(0) is grey whatever the colour.
        let grey = apply(m("filter: brightness(2) contrast(0)").unwrap(), [0.1, 0.9, 0.3, 1.]);
        assert!(grey[..3].iter().all(|v| (v - 0.5).abs() < 1e-4));
        assert!(filters("filter: grayscale(-1)").is_none());
        assert!(matches!(filters("filter: hue-rotate(0.5turn)").unwrap().1[0], Filter::HueRotate(d) if (d - 180.).abs() < 1e-9));
    }
}
