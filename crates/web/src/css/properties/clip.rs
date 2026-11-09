//! Local SVG clip-source identity. URL resolution and clipping are paint work.
use super::*;

/// CSS-decoded local URL, including its leading '#'. A thin shared allocation
/// keeps adding a reference from increasing every ordinary style value's size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalUrl(triomphe::ThinArc<(), u8>);
impl LocalUrl {
    pub fn as_str(&self) -> &str {
        // Constructed only from valid UTF-8 strings; bytes are immutable.
        std::str::from_utf8(&self.0.slice).expect("local URL UTF-8 invariant")
    }
    pub(crate) fn identity(&self) -> usize {
        self.0.heap_ptr() as usize
    }
    pub fn reserved_bytes(&self) -> usize {
        2 * size_of::<usize>() + self.0.slice.len()
    }
}

pub(super) fn parse(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let (c, reference) = match parts {
        [c] => (*c, GeometryBox::Border),
        [a, b] if GeometryBox::parse(*a).is_some() => (*b, GeometryBox::parse(*a).unwrap()),
        [a, b] if GeometryBox::parse(*b).is_some() => (*a, GeometryBox::parse(*b).unwrap()),
        _ => return Err(Error::Invalid),
    };
    if c.kind() == Kind::Function && c.text().is_some_and(|s| s.eq_ignore_ascii_case("inset")) {
        return Inset::parse(c, reference, limits, cancelled).map(Value::Inset);
    }
    if c.kind() == Kind::Function && c.text().is_some_and(|s| ["circle", "ellipse", "polygon", "path", "shape", "rect", "xywh"].iter().any(|n| s.eq_ignore_ascii_case(n))) {
        return Err(Error::Unsupported("clip-path shape or geometry box"));
    }
    if parts.len() != 1 {
        return Err(Error::Invalid);
    }
    if c.kind() == Kind::Ident && c.text().is_some_and(|s| s.eq_ignore_ascii_case("none")) {
        return Ok(Value::Keyword(Keyword::None));
    }
    let text = match c.kind() {
        Kind::Url | Kind::UnclosedUrl => c.text().ok_or(Error::Invalid)?,
        Kind::Function if c.text().is_some_and(|s| s.eq_ignore_ascii_case("url")) => {
            let mut children = c.children().filter(|c| c.kind() != Kind::Whitespace);
            let text = children.next().ok_or(Error::Invalid)?;
            if !matches!(text.kind(), Kind::String | Kind::UnclosedString) || children.next().is_some() {
                return Err(Error::Invalid);
            }
            text.text().ok_or(Error::Invalid)?
        }
        Kind::Ident if c.text().is_some_and(|s| ["margin-box", "border-box", "padding-box", "content-box", "fill-box", "stroke-box", "view-box"].iter().any(|n| s.eq_ignore_ascii_case(n))) => {
            return Err(Error::Unsupported("clip-path shape or geometry box"));
        }
        _ => return Err(Error::Invalid),
    };
    if !text.starts_with('#') {
        return Err(Error::Unsupported("non-local clip source"));
    }
    Ok(Value::LocalUrl(LocalUrl(triomphe::ThinArc::from_header_and_iter((), text.bytes()))))
}

/// `clip` (CSS 2.1 §11.1.2; CSS Masking 1 §10.1): `auto` or rect(top,
/// right, bottom, left), each a length or `auto`, the commas optional (as
/// every browser has them). What visually hides a site's "skip to
/// content" links: `clip: rect(0 0 0 0)`.
pub(super) fn rect(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let [c] = parts else { return Err(Error::Invalid) };
    if c.kind() == Kind::Ident && c.text().is_some_and(|s| s.eq_ignore_ascii_case("auto")) {
        return Ok(Value::Keyword(Keyword::Auto));
    }
    if c.kind() != Kind::Function || !c.text().is_some_and(|s| s.eq_ignore_ascii_case("rect")) {
        return Err(Error::Invalid);
    }
    let parts: SmallVec<[Component<'_>; 8]> = c.children().filter(|c| c.kind() != Kind::Whitespace).collect();
    let commas = parts.iter().filter(|c| c.kind() == Kind::Comma).count();
    let values: SmallVec<[Component<'_>; 4]> = parts.iter().copied().filter(|c| c.kind() != Kind::Comma).collect();
    // All four separated by commas or none: alternating, when there are.
    if values.len() != 4 || !(commas == 0 || commas == 3 && parts.iter().enumerate().all(|(i, c)| (c.kind() == Kind::Comma) == (i % 2 == 1))) {
        return Err(Error::Invalid);
    }
    let mut rect = RECT;
    let mut numerics = SmallVec::<[Numeric; 4]>::new();
    for (i, c) in values.into_iter().enumerate() {
        check(cancelled)?;
        if c.kind() == Kind::Ident && c.text().is_some_and(|s| s.eq_ignore_ascii_case("auto")) {
            rect |= 1 << i;
            numerics.push(Numeric::scalar(0., math::Dimension::Length, Range::All));
        } else {
            numerics.push(Numeric::compile(c, NumericKind::Length, Range::All, limits, cancelled)?);
        }
    }
    let mut indices = [u8::MAX; 12];
    for (i, index) in indices.iter_mut().take(4).enumerate() {
        *index = i as u8;
    }
    Ok(Value::Inset(Inset(triomphe::ThinArc::from_header_and_iter(InsetHeader { reference: GeometryBox::Border, indices, rect }, numerics.into_iter()))))
}

/// Reference box selection stays separate from the shape. Layout supplies its
/// actual dimensions; CSS percentages must not be frozen to a viewport here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryBox {
    Border,
    Padding,
    Content,
    Margin,
    Fill,
    Stroke,
    View,
}
impl GeometryBox {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Border => "border-box",
            Self::Padding => "padding-box",
            Self::Content => "content-box",
            Self::Margin => "margin-box",
            Self::Fill => "fill-box",
            Self::Stroke => "stroke-box",
            Self::View => "view-box",
        }
    }
    fn parse(c: Component<'_>) -> Option<Self> {
        if c.kind() != Kind::Ident {
            return None;
        }
        [Self::Border, Self::Padding, Self::Content, Self::Margin, Self::Fill, Self::Stroke, Self::View].into_iter().find(|b| c.text().is_some_and(|s| s.eq_ignore_ascii_case(b.as_str())))
    }
}

/// Shared, deduplicated inset numerics. Byte indices expand TRBL offsets and
/// (only with round) horizontal/vertical corner radii in TL/TR/BR/BL order.
/// One pointer in ordinary style values; repeated quad values occupy one slot.
#[derive(Clone, Debug)]
pub struct Inset(triomphe::ThinArc<InsetHeader, Numeric>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InsetHeader {
    reference: GeometryBox,
    indices: [u8; 12],
    /// `clip: rect()` (CSS 2.1 §11.1.2) rather than `inset()`: RECT, with
    /// a bit for each of top, right, bottom, left that is `auto`.
    rect: u8,
}
const RECT: u8 = 0x10;
impl Inset {
    /// `clip`'s rect(): the offsets of its top, right, bottom and left
    /// edges from the border box's top left (not insets), with which are
    /// `auto` (that edge of the border box).
    pub fn clip_rect(&self) -> Option<[bool; 4]> {
        let rect = self.0.header.header.rect;
        (rect & RECT != 0).then(|| std::array::from_fn(|i| rect & (1 << i) != 0))
    }
    pub fn reference_box(&self) -> GeometryBox {
        self.0.header.header.reference
    }
    pub fn offsets(&self) -> [&Numeric; 4] {
        std::array::from_fn(|i| &self.0.slice[self.0.header.header.indices[i] as usize])
    }
    pub fn radii(&self) -> Option<[&Numeric; 8]> {
        (self.0.header.header.indices[4] != u8::MAX).then(|| std::array::from_fn(|i| &self.0.slice[self.0.header.header.indices[4 + i] as usize]))
    }
    pub(crate) fn numerics(&self) -> &[Numeric] {
        &self.0.slice
    }
    pub(crate) fn identity(&self) -> usize {
        self.0.heap_ptr() as usize
    }
    pub(crate) fn own_bytes(&self) -> usize {
        size_of::<usize>() + size_of::<triomphe::HeaderSlice<triomphe::HeaderWithLength<InsetHeader>, [Numeric; 0]>>() + size_of_val(&self.0.slice)
    }
    pub fn reserved_bytes(&self) -> usize {
        // Quad expansion shares numeric programs; count each program once.
        self.own_bytes()
            + self
                .numerics()
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    let math::Value::Expression(program) = n.expression() else {
                        return None;
                    };
                    let seen = self.numerics()[..i].iter().any(|p| matches!(p.expression(), math::Value::Expression(previous) if Arc::ptr_eq(previous, program)));
                    (!seen).then(|| n.expression().memory().accounted_reserved_bytes)
                })
                .sum::<usize>()
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.identity() == other.identity()
            || self.0.header.header == other.0.header.header && self.numerics().len() == other.numerics().len() && self.numerics().iter().zip(other.numerics()).all(|(a, b)| a.same(b))
    }
    pub(crate) fn dependencies(&self) -> math::Dependencies {
        self.numerics().iter().fold(Default::default(), |d, n| d.union(n.expression().dependencies()))
    }
    pub(crate) fn computed(&self, context: &impl math::Context, limits: math::Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut values = SmallVec::<[Numeric; 12]>::new();
        for n in self.numerics() {
            check(cancelled)?;
            values.push(n.computed(context, true, limits, cancelled)?);
        }
        if values.iter().zip(self.numerics()).all(|(a, b)| a.same(b)) {
            return Ok(self.clone());
        }
        Ok(Self(triomphe::ThinArc::from_header_and_iter(self.0.header.header, values.into_iter())))
    }
    fn parse(c: Component<'_>, reference: GeometryBox, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut parts = c.children().filter(|c| c.kind() != Kind::Whitespace).peekable();
        let mut values = SmallVec::<[Numeric; 12]>::new();
        let offsets = quad(&mut parts, Range::All, limits, cancelled)?;
        values.extend(offsets);
        if let Some(round) = parts.next() {
            if round.kind() != Kind::Ident || !round.text().is_some_and(|s| s.eq_ignore_ascii_case("round")) {
                return Err(Error::Invalid);
            }
            let horizontal = quad(&mut parts, Range::Nonnegative, limits, cancelled)?;
            let vertical = if let Some(slash) = parts.next() {
                if slash.kind() != Kind::Delim || slash.character() != Some('/') {
                    return Err(Error::Invalid);
                }
                quad(&mut parts, Range::Nonnegative, limits, cancelled)?
            } else {
                horizontal.clone()
            };
            values.extend(horizontal);
            values.extend(vertical);
        }
        if parts.next().is_some() {
            return Err(Error::Invalid);
        }
        let mut unique = SmallVec::<[Numeric; 12]>::new();
        let mut indices = [u8::MAX; 12];
        for (i, n) in values.into_iter().enumerate() {
            let index = unique.iter().position(|v| v.same(&n)).unwrap_or_else(|| {
                let index = unique.len();
                unique.push(n);
                index
            });
            indices[i] = index as u8;
        }
        Ok(Self(triomphe::ThinArc::from_header_and_iter(InsetHeader { reference, indices, rect: 0 }, unique.into_iter())))
    }
}
fn quad<'a>(parts: &mut std::iter::Peekable<impl Iterator<Item = Component<'a>>>, range: Range, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<[Numeric; 4]> {
    let mut values = SmallVec::<[Numeric; 4]>::new();
    while let Some(&c) = parts.peek() {
        if c.kind() == Kind::Delim && c.character() == Some('/') || c.kind() == Kind::Ident && c.text().is_some_and(|s| s.eq_ignore_ascii_case("round")) {
            break;
        }
        if values.len() == 4 {
            return Err(Error::Invalid);
        }
        check(cancelled)?;
        values.push(Numeric::compile(c, NumericKind::LengthPercentage, range, limits, cancelled)?);
        parts.next();
    }
    let indices = match values.len() {
        1 => [0, 0, 0, 0],
        2 => [0, 1, 0, 1],
        3 => [0, 1, 2, 1],
        4 => [0, 1, 2, 3],
        _ => return Err(Error::Invalid),
    };
    Ok(indices.map(|i| values[i].clone()))
}
