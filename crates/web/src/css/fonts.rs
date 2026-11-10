//! `@font-face` rules: the descriptors a page's web fonts are loaded with.
use super::{Kind, Statement, Syntax};
use crate::font::{Descriptor, Slopes, Span};
use std::sync::{Arc, atomic::AtomicBool};
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A URL with its declared `format()`, if any.
    Url {
        url: Arc<str>,
        format: Option<Arc<str>>,
    },
    Local(Arc<str>),
}
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub descriptor: Descriptor,
    pub sources: Vec<Source>,
    /// The stylesheet's URL, which relative sources resolve against.
    pub base: Option<Arc<str>>,
}
impl Source {
    /// Whether the engine can decode this source: WOFF2, WOFF, TrueType and
    /// OpenType, by declared format or by the URL's extension.
    pub fn decodable(&self) -> bool {
        match self {
            Self::Local(_) => false,
            Self::Url { url, format } => match format {
                Some(f) => {
                    let f = f.to_ascii_lowercase();
                    matches!(f.as_str(), "woff2" | "woff" | "truetype" | "opentype" | "woff2-variations" | "woff-variations" | "truetype-variations" | "opentype-variations")
                }
                None => {
                    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
                    path.ends_with(".woff2") || path.ends_with(".woff") || path.ends_with(".ttf") || path.ends_with(".otf") || url.starts_with("data:")
                }
            },
        }
    }
}
fn is(c: super::Component<'_>, word: &str) -> bool {
    c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case(word)
}
/// Parse one `@font-face` rule; None when it has no usable family or src.
pub fn parse(syntax: &Syntax, rule: &Statement<'_>, cancelled: Option<&AtomicBool>) -> Option<FontFace> {
    let declarations = rule.block_contents(cancelled).ok()?;
    let mut family = None;
    let mut sources = Vec::new();
    let mut weight = Span::point(400.);
    let mut width = Span::point(100.);
    let mut slopes = Slopes { oblique: Some(Span::point(0.)), italic: None };
    let mut ranges = None;
    for d in declarations {
        if d.kind() != super::StatementKind::Declaration {
            continue;
        }
        let name = d.name()?.to_ascii_lowercase();
        let parts: Vec<_> = d.value().filter(|c| c.kind() != Kind::Whitespace).collect();
        match name.as_str() {
            "font-family" => {
                let text: Vec<&str> = parts.iter().filter_map(|c| c.text()).collect();
                if parts.len() == 1 && parts[0].kind() == Kind::String {
                    family = Some(Arc::from(parts[0].text()?));
                } else if !parts.is_empty() && parts.iter().all(|c| c.kind() == Kind::Ident) {
                    family = Some(Arc::from(text.join(" ")));
                }
            }
            "src" => {
                for group in parts.split(|c| c.kind() == Kind::Comma) {
                    let Some(first) = group.first() else { continue };
                    if first.kind() == Kind::Function && first.text().unwrap().eq_ignore_ascii_case("local") {
                        let name: Vec<&str> = first.children().filter(|c| c.kind() != Kind::Whitespace).filter_map(|c| c.text()).collect();
                        if !name.is_empty() {
                            sources.push(Source::Local(Arc::from(name.join(" "))));
                        }
                        continue;
                    }
                    let url = match first.kind() {
                        Kind::Url => first.text().map(Arc::from),
                        Kind::Function if first.text().unwrap().eq_ignore_ascii_case("url") || first.text().unwrap().eq_ignore_ascii_case("src") => {
                            first.children().find(|c| c.kind() == Kind::String).and_then(|c| c.text()).map(Arc::from)
                        }
                        _ => None,
                    };
                    let Some(url) = url else { continue };
                    let format = group
                        .iter()
                        .skip(1)
                        .find(|c| c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("format"))
                        .and_then(|f| f.children().find(|c| c.kind() != Kind::Whitespace).and_then(|c| c.text()).map(Arc::from));
                    sources.push(Source::Url { url, format });
                }
            }
            "font-weight" => {
                let mut values = Vec::new();
                for c in &parts {
                    if is(*c, "normal") {
                        values.push(400.);
                    } else if is(*c, "bold") {
                        values.push(700.);
                    } else if let Some(n) = c.number().filter(|n| (1. ..=1000.).contains(&n.value)) {
                        values.push(n.value);
                    }
                }
                match values.as_slice() {
                    [v] => weight = Span::point(*v),
                    [a, b] => weight = Span::new(*a, *b),
                    _ => {}
                }
            }
            "font-stretch" | "font-width" => {
                let mut values = Vec::new();
                for c in &parts {
                    if let Some(n) = c.number().filter(|_| matches!(c.kind(), Kind::Percentage | Kind::IntegerPercentage)) {
                        values.push(n.value);
                    } else if let Some(word) = c.text() {
                        let keywords = [
                            ("ultra-condensed", 50.),
                            ("extra-condensed", 62.5),
                            ("condensed", 75.),
                            ("semi-condensed", 87.5),
                            ("normal", 100.),
                            ("semi-expanded", 112.5),
                            ("expanded", 125.),
                            ("extra-expanded", 150.),
                            ("ultra-expanded", 200.),
                        ];
                        if let Some((_, v)) = keywords.iter().find(|(k, _)| word.eq_ignore_ascii_case(k)) {
                            values.push(*v);
                        }
                    }
                }
                match values.as_slice() {
                    [v] => width = Span::point(*v),
                    [a, b] => width = Span::new(*a, *b),
                    _ => {}
                }
            }
            "font-style" => {
                if let Some(first) = parts.first() {
                    if is(*first, "italic") {
                        slopes = Slopes { oblique: None, italic: Some(Span::point(1.)) };
                    } else if is(*first, "oblique") {
                        let angles: Vec<f64> = parts.iter().skip(1).filter_map(|c| c.number().map(|n| n.value)).collect();
                        slopes = Slopes {
                            oblique: Some(match angles.as_slice() {
                                [a] => Span::point(*a),
                                [a, b] => Span::new(*a, *b),
                                _ => Span::point(14.),
                            }),
                            italic: None,
                        };
                    } else if is(*first, "normal") {
                        slopes = Slopes { oblique: Some(Span::point(0.)), italic: None };
                    }
                }
            }
            "unicode-range" => {
                // Descriptors tokenize ranges; stylesheets do not, so the
                // declaration's text is read again with ranges enabled.
                let (Some(first), Some(last)) = (parts.first(), parts.last()) else { continue };
                let text = &syntax.source()[first.span().start as usize..last.span().end as usize];
                if text.len() > 64 * 1024 {
                    continue;
                }
                let Ok(reparsed) = Syntax::parse_with_unicode_ranges(Arc::from(text), Default::default(), cancelled, true) else { continue };
                let mut out = Vec::new();
                for c in reparsed.components() {
                    if let Some(r) = c.unicode_range() {
                        out.push((*r.start(), *r.end()));
                        if out.len() > 4096 {
                            break;
                        }
                    }
                }
                if !out.is_empty() {
                    ranges = Some(out.into_boxed_slice());
                }
            }
            _ => {}
        }
    }
    let family = family?;
    if sources.is_empty() {
        return None;
    }
    Some(FontFace { descriptor: Descriptor { family, weight, width, slopes, ranges }, sources, base: None })
}
