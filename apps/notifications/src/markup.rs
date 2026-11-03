//! The body markup the notification spec allows: `<b>`, `<i>`, `<u>`,
//! `<a href="…">`, `<img alt="…">` (as its text) and `<br>`, with XML
//! entities; anything else is shown as text, as the spec asks of a
//! server that doesn't know a tag.

/// A run of text and how it looks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Where a link goes.
    pub link: Option<String>,
}

/// The spans of `s`, in order, with neighbours that look the same merged.
pub fn parse(s: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let mut link: Option<String> = None;
    let push = |out: &mut Vec<Span>, text: &str, bold: u32, italic: u32, underline: u32, link: &Option<String>| {
        if text.is_empty() {
            return;
        }
        let style = Span { text: String::new(), bold: bold > 0, italic: italic > 0, underline: underline > 0, link: link.clone() };
        match out.last_mut() {
            Some(last) if Span { text: String::new(), ..last.clone() } == style => last.text.push_str(text),
            _ => out.push(Span { text: text.to_owned(), ..style }),
        }
    };
    let mut rest = s;
    while let Some(i) = rest.find(['<', '&']) {
        push(&mut out, &rest[..i], bold, italic, underline, &link);
        rest = &rest[i..];
        if rest.starts_with('<') {
            let Some(end) = rest.find('>') else {
                push(&mut out, rest, bold, italic, underline, &link);
                rest = "";
                break;
            };
            let tag = rest[1..end].trim();
            let closing = tag.starts_with('/');
            let name = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_ascii_lowercase();
            let mut known = true;
            match (name.as_str(), closing) {
                ("b", false) => bold += 1,
                ("b", true) => bold = bold.saturating_sub(1),
                ("i", false) => italic += 1,
                ("i", true) => italic = italic.saturating_sub(1),
                ("u", false) => underline += 1,
                ("u", true) => underline = underline.saturating_sub(1),
                ("a", false) => link = attribute(tag, "href").map(|h| unescape(&h)),
                ("a", true) => link = None,
                ("br", _) => push(&mut out, "\n", bold, italic, underline, &link),
                ("img", false) => {
                    if let Some(alt) = attribute(tag, "alt") {
                        push(&mut out, &unescape(&alt), bold, italic, underline, &link);
                    }
                }
                _ => known = false,
            }
            if known {
                rest = &rest[end + 1..];
            } else {
                // Not markup we know: the text as it was.
                push(&mut out, "<", bold, italic, underline, &link);
                rest = &rest[1..];
            }
        } else {
            let (ch, len) = entity(rest);
            match ch {
                Some(c) => push(&mut out, c.encode_utf8(&mut [0; 4]), bold, italic, underline, &link),
                None => push(&mut out, "&", bold, italic, underline, &link),
            }
            rest = &rest[len..];
        }
    }
    push(&mut out, rest, bold, italic, underline, &link);
    // Trim the whole: leading space of the first span, trailing of the last.
    if let Some(first) = out.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = out.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    out.retain(|s| !s.text.is_empty());
    out
}

/// The text alone.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// `name="value"` (or single quotes) in a tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[at + name.len()..].trim_start();
        if before_ok && let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let start = tag.len() - v.len();
            let quote = v.chars().next()?;
            return if quote == '"' || quote == '\'' {
                let body = &tag[start + 1..];
                Some(body[..body.find(quote)?].to_owned())
            } else {
                Some(tag[start..].split_whitespace().next()?.trim_end_matches('/').to_owned())
            };
        }
        from = at + name.len();
    }
    None
}

/// The character an entity at the start of `s` stands for, and its
/// length (1 for a bare `&`).
fn entity(s: &str) -> (Option<char>, usize) {
    let Some(end) = s.find(';').filter(|&e| e <= 10) else { return (None, 1) };
    let e = &s[1..end];
    let ch = match e {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")).and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32),
    };
    match ch {
        Some(c) => (Some(c), end + 1),
        None => (None, 1),
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let (ch, len) = entity(&rest[i..]);
        out.push(ch.unwrap_or('&'));
        rest = &rest[i + len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_of(s: &str) -> String {
        plain(&parse(s))
    }

    #[test]
    fn text_and_entities() {
        assert_eq!(plain_of("<b>Ben</b> said: 1 &lt; 2 &amp;&amp; <i>yes</i>"), "Ben said: 1 < 2 && yes");
        assert_eq!(plain_of("a<br/>b<br>c"), "a\nb\nc");
        assert_eq!(plain_of("<a href=\"x\">link</a> &#x263A; &#65; R&D"), "link ☺ A R&D");
        assert_eq!(plain_of("<img src=\"x.png\" alt=\"[cat]\"/> unclosed <b"), "[cat] unclosed <b");
        // Unknown tags stay as text, as the spec asks.
        assert_eq!(plain_of("x <3 y and <span>z</span>"), "x <3 y and <span>z</span>");
    }

    #[test]
    fn styles_and_links() {
        let s = parse("Hi <b>there <i>you</i></b> see <a href='https://x.org/?a=1&amp;b=2'>this</a>.");
        let styled: Vec<(&str, bool, bool, Option<&str>)> = s.iter().map(|s| (s.text.as_str(), s.bold, s.italic, s.link.as_deref())).collect();
        assert_eq!(
            styled,
            [
                ("Hi ", false, false, None),
                ("there ", true, false, None),
                ("you", true, true, None),
                (" see ", false, false, None),
                ("this", false, false, Some("https://x.org/?a=1&b=2")),
                (".", false, false, None),
            ]
        );
        assert!(parse("<u>under</u>")[0].underline);
    }
}
