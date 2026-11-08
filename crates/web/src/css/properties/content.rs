//! `content` (CSS Generated Content 3 §2): strings, `attr()`, counters
//! and quotes for `::before` and `::after` (images are not supported and
//! fail the declaration); and the lists `counter-reset`,
//! `counter-increment`, `counter-set` and `quotes` hold, as parts too
//! (CSS Lists 3 §4, Generated Content 3 §3).
use super::*;
use std::sync::Arc;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentPart {
    Text(Arc<str>),
    /// `attr(name)`: the element's attribute, or nothing.
    Attr(Arc<str>),
    /// `counter(name, style)`: the innermost counter of the name.
    Counter(Arc<str>, Keyword),
    /// `counters(name, separator, style)`: every counter of the name, the
    /// outermost first.
    Counters(Arc<str>, Arc<str>, Keyword),
    /// `open-quote`, `close-quote`, `no-open-quote`, `no-close-quote`:
    /// whether it opens, and whether it shows.
    Quote(bool, bool),
    /// In `counter-reset`/`-increment`/`-set`: a counter and its value.
    Change(Arc<str>, i64),
    /// In `quotes`: one level's opening and closing marks.
    Pair(Arc<str>, Arc<str>),
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
                        ContentPart::Text(t) | ContentPart::Attr(t) | ContentPart::Counter(t, _) | ContentPart::Change(t, _) => t.len(),
                        ContentPart::Counters(a, b, _) | ContentPart::Pair(a, b) => a.len() + b.len(),
                        ContentPart::Quote(..) => 0,
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
            Kind::Function if c.text().unwrap().eq_ignore_ascii_case("counter") || c.text().unwrap().eq_ignore_ascii_case("counters") => {
                let plural = c.text().unwrap().eq_ignore_ascii_case("counters");
                let args: Vec<Component<'_>> = c.children().filter(|c| !matches!(c.kind(), Kind::Whitespace | Kind::Comma)).collect();
                let name = args.first().filter(|n| n.kind() == Kind::Ident).and_then(|n| n.text()).ok_or(Error::Invalid)?;
                let (separator, rest) = if plural {
                    let sep = args.get(1).filter(|s| s.kind() == Kind::String).and_then(|s| s.text()).ok_or(Error::Invalid)?;
                    (Some(sep), &args[2..])
                } else {
                    (None, &args[1..])
                };
                let style = match rest {
                    [] => Keyword::Decimal,
                    [k] if k.kind() == Kind::Ident => list_style(k.text().unwrap()).ok_or(Error::Unsupported("counter style"))?,
                    _ => return Err(Error::Invalid),
                };
                out.push(match separator {
                    Some(sep) => ContentPart::Counters(Arc::from(name), Arc::from(sep), style),
                    None => ContentPart::Counter(Arc::from(name), style),
                });
            }
            Kind::Function => return Err(Error::Unsupported("content function")),
            Kind::Ident => {
                let name = c.text().unwrap().to_ascii_lowercase();
                out.push(match name.as_str() {
                    "open-quote" => ContentPart::Quote(true, true),
                    "close-quote" => ContentPart::Quote(false, true),
                    "no-open-quote" => ContentPart::Quote(true, false),
                    "no-close-quote" => ContentPart::Quote(false, false),
                    _ => return Err(Error::Unsupported("content keyword")),
                });
            }
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
/// A list style a counter may be shown in, by its name.
fn list_style(name: &str) -> Option<Keyword> {
    let name = name.to_ascii_lowercase();
    [
        Keyword::Decimal,
        Keyword::DecimalLeadingZero,
        Keyword::LowerAlpha,
        Keyword::UpperAlpha,
        Keyword::LowerLatin,
        Keyword::UpperLatin,
        Keyword::LowerRoman,
        Keyword::UpperRoman,
        Keyword::Disc,
        Keyword::Circle,
        Keyword::Square,
        Keyword::None,
    ]
    .into_iter()
    .find(|k| k.as_str() == name)
}
/// `counter-reset`, `counter-increment`, `counter-set`: `none | [<ident>
/// <integer>?]+`, a counter without a number taking `default`.
pub(super) fn parse_counters(parts: &[Component<'_>], default: i64) -> Result<Value> {
    if let [c] = parts
        && c.kind() == Kind::Ident
        && c.text().unwrap().eq_ignore_ascii_case("none")
    {
        return Ok(Value::Keyword(Keyword::None));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let c = parts[i];
        if c.kind() != Kind::Ident || ["none", "initial", "inherit", "unset", "default"].iter().any(|w| c.text().unwrap().eq_ignore_ascii_case(w)) {
            return Err(Error::Invalid);
        }
        let mut value = default;
        if let Some(n) = parts.get(i + 1).filter(|n| matches!(n.kind(), Kind::Integer)) {
            value = n.number().ok_or(Error::Invalid)?.value as i64;
            i += 1;
        }
        out.push(ContentPart::Change(Arc::from(c.text().unwrap()), value));
        i += 1;
        if out.len() > 32 {
            return Err(Error::Limit);
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Value::Content(Content(out.into())))
}
/// `quotes`: `auto | none | [<string> <string>]+`.
pub(super) fn parse_quotes(parts: &[Component<'_>]) -> Result<Value> {
    if let [c] = parts
        && c.kind() == Kind::Ident
    {
        let name = c.text().unwrap();
        if name.eq_ignore_ascii_case("auto") {
            return Ok(Value::Keyword(Keyword::Auto));
        }
        if name.eq_ignore_ascii_case("none") {
            return Ok(Value::Keyword(Keyword::None));
        }
        return Err(Error::Invalid);
    }
    if parts.is_empty() || parts.len() % 2 != 0 || parts.len() > 16 || parts.iter().any(|c| c.kind() != Kind::String) {
        return Err(Error::Invalid);
    }
    let out: Vec<ContentPart> = parts.chunks(2).map(|p| ContentPart::Pair(Arc::from(p[0].text().unwrap()), Arc::from(p[1].text().unwrap()))).collect();
    Ok(Value::Content(Content(out.into())))
}
