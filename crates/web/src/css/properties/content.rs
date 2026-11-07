//! `content` (CSS Generated Content 3 §2): strings and `attr()` for
//! `::before` and `::after`. Counters, quotes and images are not
//! supported and fail the declaration.
use super::*;
use std::sync::Arc;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentPart {
    Text(Arc<str>),
    /// `attr(name)`: the element's attribute, or nothing.
    Attr(Arc<str>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content(pub Arc<[ContentPart]>);
impl Content {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.0
            .iter()
            .map(|p| {
                size_of::<ContentPart>()
                    + match p {
                        ContentPart::Text(t) | ContentPart::Attr(t) => t.len(),
                    }
            })
            .sum()
    }
}
/// `normal | none | [ <string> | attr(<ident>) ]+`.
pub(super) fn parse_content(parts: &[Component<'_>]) -> Result<Value> {
    if let [c] = parts
        && c.kind() == Kind::Ident
    {
        let name = c.text().unwrap();
        if name.eq_ignore_ascii_case("normal") {
            return Ok(Value::Keyword(Keyword::Normal));
        }
        if name.eq_ignore_ascii_case("none") {
            return Ok(Value::Keyword(Keyword::None));
        }
    }
    let mut out = Vec::new();
    for c in parts {
        match c.kind() {
            Kind::String => out.push(ContentPart::Text(Arc::from(c.text().unwrap()))),
            Kind::Function if c.text().unwrap().eq_ignore_ascii_case("attr") => {
                let mut args = c.children().filter(|c| c.kind() != Kind::Whitespace);
                let (Some(name), None) = (args.next(), args.next()) else { return Err(Error::Invalid) };
                if name.kind() != Kind::Ident {
                    return Err(Error::Invalid);
                }
                out.push(ContentPart::Attr(Arc::from(name.text().unwrap())));
            }
            Kind::Function => return Err(Error::Unsupported("content function")),
            Kind::Ident => return Err(Error::Unsupported("content keyword")),
            _ => return Err(Error::Invalid),
        }
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Content(Content(out.into())))
}
