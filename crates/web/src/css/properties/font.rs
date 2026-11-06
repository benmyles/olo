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
    pub(crate) fn parse(values: Components<'_>, cancelled: Option<&AtomicBool>) -> Result<Self> {
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
                            if reserved(s) || GenericFamily::keyword(s).is_some() {
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
        #[cfg(feature = "baseline-font-family-fat")]
        {
            Ok(Self(Arc::from(result)))
        }
        #[cfg(not(feature = "baseline-font-family-fat"))]
        {
            Ok(Self(triomphe::ThinArc::from_header_and_iter((), result.into_iter())))
        }
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
