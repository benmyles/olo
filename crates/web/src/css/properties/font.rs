//! Font family identity and the font-specific longhand grammars. Face loading,
//! matching, synthesis and shaping are separate worker stages.
use super::*;

macro_rules! generics {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum GenericFamily { $($variant),* }
        impl GenericFamily {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $name),* } }
            fn keyword(s: &str) -> Option<Self> {
                $(if !$name.contains('(') && s.eq_ignore_ascii_case($name) { return Some(Self::$variant); })*
                None
            }
        }
    };
}
generics! {
    Serif => "serif", SansSerif => "sans-serif", Monospace => "monospace",
    Cursive => "cursive", Fantasy => "fantasy", SystemUi => "system-ui",
    UiSerif => "ui-serif", UiSansSerif => "ui-sans-serif",
    UiMonospace => "ui-monospace", UiRounded => "ui-rounded", Math => "math",
    FangSong => "generic(fangsong)", Kai => "generic(kai)",
    KhmerMul => "generic(khmer-mul)", Nastaliq => "generic(nastaliq)",
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Named(Arc<str>),
    Generic(GenericFamily),
}
/// Immutable prioritized families, with decoded names independent of source
/// Syntax. Quoted generic names remain Named, never generic keywords.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Families(FamilyStorage);
#[cfg(feature = "baseline-font-family-fat")]
type FamilyStorage = Arc<[Family]>;
#[cfg(not(feature = "baseline-font-family-fat"))]
type FamilyStorage = triomphe::ThinArc<(), Family>;
impl Families {
    pub fn as_slice(&self) -> &[Family] {
        #[cfg(feature = "baseline-font-family-fat")]
        {
            &self.0
        }
        #[cfg(not(feature = "baseline-font-family-fat"))]
        {
            &self.0.slice
        }
    }
    pub(crate) fn identity(&self) -> usize {
        #[cfg(feature = "baseline-font-family-fat")]
        {
            Arc::as_ptr(&self.0) as *const Family as usize
        }
        #[cfg(not(feature = "baseline-font-family-fat"))]
        {
            self.0.heap_ptr() as usize
        }
    }
    /// Referenced allocations including Arc controls; clones share them.
    pub fn reserved_bytes(&self) -> usize {
        2 * size_of::<usize>()
            + size_of_val(self.as_slice())
            + self
                .as_slice()
                .iter()
                .map(|f| match f {
                    Family::Named(name) => name.len() + 2 * size_of::<usize>(),
                    Family::Generic(_) => 0,
                })
                .sum::<usize>()
    }
    pub(crate) fn parse<'a>(values: impl Iterator<Item = Component<'a>>, cancelled: Option<&AtomicBool>) -> Result<Self> {
        let mut input = values.filter(|c| c.kind() != Kind::Whitespace).peekable();
        let mut result = Vec::new();
        loop {
            check(cancelled)?;
            let first = input.next().ok_or(Error::Invalid)?;
            let family = match first.kind() {
                Kind::String | Kind::UnclosedString => Family::Named(Arc::from(first.text().unwrap())),
                Kind::Ident => {
                    let s = first.text().unwrap();
                    if let Some(generic) = GenericFamily::keyword(s) {
                        Family::Generic(generic)
                    } else {
                        let mut name = String::new();
                        let mut next = Some(first);
                        while let Some(c) = next {
                            check(cancelled)?;
                            let s = c.text().unwrap();
                            // A generic keyword names a generic only alone
                            // (the first ident, above); within a name it is
                            // a word of it ("BBC Reith Serif"), as in Chrome.
                            if reserved(s) {
                                return Err(Error::Invalid);
                            }
                            if !name.is_empty() {
                                name.push(' ');
                            }
                            name.push_str(s);
                            next = if input.peek().is_some_and(|c| c.kind() == Kind::Ident) { input.next() } else { None };
                        }
                        Family::Named(Arc::from(name))
                    }
                }
                Kind::Function if first.text().unwrap().eq_ignore_ascii_case("generic") => {
                    let mut args = first.children().filter(|c| c.kind() != Kind::Whitespace);
                    let arg = args.next().ok_or(Error::Invalid)?;
                    if arg.kind() != Kind::Ident || args.next().is_some() {
                        return Err(Error::Invalid);
                    }
                    let s = arg.text().unwrap();
                    let generic = if s.eq_ignore_ascii_case("fangsong") {
                        GenericFamily::FangSong
                    } else if s.eq_ignore_ascii_case("kai") {
                        GenericFamily::Kai
                    } else if s.eq_ignore_ascii_case("khmer-mul") {
                        GenericFamily::KhmerMul
                    } else if s.eq_ignore_ascii_case("nastaliq") {
                        GenericFamily::Nastaliq
                    } else {
                        return Err(Error::Invalid);
                    };
                    Family::Generic(generic)
                }
                _ => return Err(Error::Invalid),
            };
            result.push(family);
            match input.next() {
                None => break,
                Some(c) if c.kind() == Kind::Comma => {}
                _ => return Err(Error::Invalid),
            }
        }
        Ok(Self::from_families(result))
    }
    fn from_families(result: Vec<Family>) -> Self {
        #[cfg(feature = "baseline-font-family-fat")]
        {
            Self(Arc::from(result))
        }
        #[cfg(not(feature = "baseline-font-family-fat"))]
        {
            Self(triomphe::ThinArc::from_header_and_iter((), result.into_iter()))
        }
    }
    /// The UI font (a system font keyword's).
    pub(crate) fn system_ui() -> Self {
        Self::from_families(vec![Family::Generic(GenericFamily::SystemUi), Family::Generic(GenericFamily::SansSerif)])
    }
}
fn reserved(s: &str) -> bool {
    ["default", "initial", "inherit", "unset", "revert", "revert-layer", "revert-rule"].iter().any(|word| s.eq_ignore_ascii_case(word))
}
pub(super) fn style(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let first = parts.first().ok_or(Error::Invalid)?;
    if first.kind() != Kind::Ident {
        return Err(Error::Invalid);
    }
    let text = first.text().unwrap();
    let keyword = [Keyword::Normal, Keyword::Italic, Keyword::Left, Keyword::Right, Keyword::Oblique].into_iter().find(|k| text.eq_ignore_ascii_case(k.as_str())).ok_or(Error::Invalid)?;
    match parts.len() {
        1 => Ok(Value::Keyword(keyword)),
        2 if keyword == Keyword::Oblique => Numeric::compile(parts[1], NumericKind::Angle, Range::FontAngle, limits, cancelled).map(Value::Numeric),
        _ => Err(Error::Invalid),
    }
}
pub(super) fn width(c: Component<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    if c.kind() == Kind::Ident {
        let name = c.text().unwrap();
        let percent = [
            ("ultra-condensed", 50.),
            ("extra-condensed", 62.5),
            ("condensed", 75.),
            ("semi-condensed", 87.5),
            ("normal", 100.),
            ("semi-expanded", 112.5),
            ("expanded", 125.),
            ("extra-expanded", 150.),
            ("ultra-expanded", 200.),
        ]
        .into_iter()
        .find(|(s, _)| name.eq_ignore_ascii_case(s))
        .map(|(_, n)| n)
        .ok_or(Error::Invalid)?;
        return Ok(Value::Numeric(Numeric::scalar(percent, math::Dimension::Percentage, Range::Nonnegative)));
    }
    Numeric::compile(c, NumericKind::Percentage, Range::Nonnegative, limits, cancelled).map(Value::Numeric)
}

/// `font-feature-settings`: OpenType feature tags and values, passed to
/// shaping as given (CSS Fonts 4 §7.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features(pub Arc<[Feature]>);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
}
impl Features {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0.len() * size_of::<Feature>()
    }
}
/// `normal | [ <string> [ <integer> | on | off ]? ]#`.
pub(super) fn parse_features(parts: &[Component<'_>]) -> Result<Value> {
    if let [c] = parts
        && c.kind() == Kind::Ident
        && c.text().unwrap().eq_ignore_ascii_case("normal")
    {
        return Ok(Value::Keyword(Keyword::Normal));
    }
    let mut out = Vec::new();
    for group in parts.split(|c| c.kind() == Kind::Comma) {
        let (tag, value) = match group {
            [tag] => (*tag, None),
            [tag, value] => (*tag, Some(*value)),
            _ => return Err(Error::Invalid),
        };
        if tag.kind() != Kind::String {
            return Err(Error::Invalid);
        }
        let text = tag.text().unwrap();
        if text.len() != 4 || !text.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(Error::Invalid);
        }
        let value = match value {
            None => 1,
            Some(c) if c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("on") => 1,
            Some(c) if c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("off") => 0,
            Some(c) if c.kind() == Kind::Integer => {
                let v = c.number().map(|n| n.value).unwrap_or(-1.);
                if !(0. ..=u32::MAX as f64).contains(&v) {
                    return Err(Error::Invalid);
                }
                v as u32
            }
            _ => return Err(Error::Invalid),
        };
        out.push(Feature { tag: text.as_bytes().try_into().unwrap(), value });
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Features(Features(out.into())))
}
/// The `font` shorthand (CSS Fonts 4 §3.1): `[ <style> || <variant> ||
/// <weight> || <width> ]? <size> [ / <line-height> ]? <family>`, every
/// longhand it covers assigned (the ones left out to their initial
/// values); or a system font keyword, drawn as the UI font.
pub(super) fn parse_shorthand(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Assignments> {
    let parts: Vec<Component<'_>> = values.filter(|c| c.kind() != Kind::Whitespace).collect();
    if parts.is_empty() {
        return Err(Error::Invalid);
    }
    let normal = || Value::Keyword(Keyword::Normal);
    // font-width's normal is 100% (see `width`).
    let normal_width = || Value::Numeric(Numeric::scalar(100., math::Dimension::Percentage, Range::Nonnegative));
    let mut style = None;
    let mut weight = None;
    let mut font_width = None;
    let mut normals = 0usize;
    let mut size = None;
    let mut line_height = None;
    let mut family = None;
    // A system font: the UI font at its size.
    if let [c] = parts.as_slice()
        && c.kind() == Kind::Ident
        && let Some(name) = c.text()
        && ["caption", "icon", "menu", "message-box", "small-caption", "status-bar"].iter().any(|k| name.eq_ignore_ascii_case(k))
    {
        let mut out = Assignments::new();
        out.push(Assignment { property: Property::FontStyle, value: normal() });
        out.push(Assignment { property: Property::FontWeight, value: normal() });
        out.push(Assignment { property: Property::FontWidth, value: normal_width() });
        out.push(Assignment {
            property: Property::FontSize,
            value: Value::Numeric(Numeric::scalar(if name.eq_ignore_ascii_case("small-caption") { 11. } else { 13.333_333 }, math::Dimension::Length, Range::Nonnegative)),
        });
        out.push(Assignment { property: Property::LineHeight, value: normal() });
        out.push(Assignment { property: Property::FontFamily, value: Value::Families(Families::system_ui()) });
        out.push(Assignment { property: Property::FontFeatureSettings, value: normal() });
        return Ok(out);
    }
    let mut i = 0usize;
    // Up to four leading style, variant, weight and width values, in any
    // order; `normal` stands for any of them.
    while i < parts.len() && i < 4 {
        check(cancelled)?;
        let c = parts[i];
        if c.kind() == Kind::Ident {
            let text = c.text().unwrap_or("");
            if text.eq_ignore_ascii_case("normal") {
                normals += 1;
                i += 1;
                continue;
            }
            if text.eq_ignore_ascii_case("small-caps") {
                // font-variant: accepted, not drawn.
                i += 1;
                continue;
            }
            if style.is_none()
                && let Ok(v) = style_of(&parts[i..], limits, cancelled)
            {
                style = Some(v.0);
                i += v.1;
                continue;
            }
            if weight.is_none()
                && ["bold", "bolder", "lighter"].iter().any(|k| text.eq_ignore_ascii_case(k))
                && let Ok(v) = super::parse::component(Property::FontWeight.grammar(), c, limits, cancelled)
            {
                weight = Some(v);
                i += 1;
                continue;
            }
            if font_width.is_none()
                && let Ok(v) = width(c, limits, cancelled)
            {
                font_width = Some(v);
                i += 1;
                continue;
            }
            break;
        }
        if matches!(c.kind(), Kind::Number | Kind::Integer)
            && weight.is_none()
            && let Ok(v) = super::parse::component(Property::FontWeight.grammar(), c, limits, cancelled)
        {
            weight = Some(v);
            i += 1;
            continue;
        }
        break;
    }
    if normals + usize::from(style.is_some()) + usize::from(weight.is_some()) + usize::from(font_width.is_some()) > 4 {
        return Err(Error::Invalid);
    }
    // The size, then an optional / line-height, then the family list.
    let c = *parts.get(i).ok_or(Error::Invalid)?;
    size = Some(super::parse::component(Property::FontSize.grammar(), c, limits, cancelled)?);
    i += 1;
    if parts.get(i).is_some_and(|c| c.kind() == Kind::Delim && c.character() == Some('/')) {
        let c = *parts.get(i + 1).ok_or(Error::Invalid)?;
        line_height = Some(super::parse::component(Property::LineHeight.grammar(), c, limits, cancelled)?);
        i += 2;
    }
    if i >= parts.len() {
        return Err(Error::Invalid);
    }
    family = Some(Families::parse(parts[i..].iter().copied(), cancelled)?);
    let mut out = Assignments::new();
    out.push(Assignment { property: Property::FontStyle, value: style.unwrap_or_else(normal) });
    out.push(Assignment { property: Property::FontWeight, value: weight.unwrap_or_else(normal) });
    out.push(Assignment { property: Property::FontWidth, value: font_width.unwrap_or_else(normal_width) });
    out.push(Assignment { property: Property::FontSize, value: size.ok_or(Error::Invalid)? });
    out.push(Assignment { property: Property::LineHeight, value: line_height.unwrap_or_else(normal) });
    out.push(Assignment { property: Property::FontFamily, value: Value::Families(family.ok_or(Error::Invalid)?) });
    out.push(Assignment { property: Property::FontFeatureSettings, value: normal() });
    Ok(out)
}
/// A font-style at the start of `parts`: the value and how many parts it took.
fn style_of(parts: &[Component<'_>], limits: Limits, cancelled: Option<&AtomicBool>) -> Result<(Value, usize)> {
    let first = parts.first().ok_or(Error::Invalid)?;
    let text = first.text().ok_or(Error::Invalid)?;
    if text.eq_ignore_ascii_case("oblique")
        && parts.get(1).is_some_and(|c| matches!(c.kind(), Kind::Dimension | Kind::IntegerDimension))
        && let Ok(v) = style(&parts[..2], limits, cancelled)
    {
        return Ok((v, 2));
    }
    if ["italic", "oblique"].iter().any(|k| text.eq_ignore_ascii_case(k)) {
        return style(&parts[..1], limits, cancelled).map(|v| (v, 1));
    }
    Err(Error::Invalid)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Syntax;
    fn families(text: &str) -> Result<Vec<String>> {
        let syntax = Syntax::parse(Arc::from(text), Default::default(), None).unwrap();
        Families::parse(syntax.components(), None).map(|f| f.as_slice().iter().map(|f| format!("{f:?}")).collect())
    }
    #[test]
    fn a_generic_word_inside_an_unquoted_name_is_part_of_it() {
        // The BBC's families: a generic keyword as a later word of a name.
        assert_eq!(families("BBC Reith Serif, BBCReithSerif-fallback, serif").unwrap(), ["Named(\"BBC Reith Serif\")", "Named(\"BBCReithSerif-fallback\")", "Generic(Serif)"]);
        assert_eq!(families("Noto Sans Mono, monospace").unwrap(), ["Named(\"Noto Sans Mono\")", "Generic(Monospace)"]);
        // Alone, or first, it is the generic: a word after it is invalid.
        assert!(families("serif Pro").is_err());
        assert!(families("Foo inherit").is_err());
    }
}
