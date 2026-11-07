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

pub(super) fn parse(c: Component<'_>) -> Result<Value> {
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
        Kind::Function if c.text().is_some_and(|s| ["inset", "circle", "ellipse", "polygon", "path", "shape", "rect", "xywh"].iter().any(|n| s.eq_ignore_ascii_case(n))) => {
            return Err(Error::Unsupported("clip-path shape or geometry box"));
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
