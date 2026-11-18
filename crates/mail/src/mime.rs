//! A mail message's sender, subject and words, from its bytes: headers
//! unfolded (RFC 5322), encoded words decoded (RFC 2047), multipart
//! walked, quoted-printable and base64 undone (RFC 2045), charsets by
//! encoding_rs, and an HTML part made words (its tags, styles, scripts and
//! hidden preheader characters left out, its blocks lines). Enough to look
//! for a one-time code in; attachments and forwarded messages are passed
//! over.

use base64::Engine as _;

/// What a message says, as words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    /// Who it's from: the name it gives ("Acme"), else its address.
    pub from: String,
    /// The address it's from, lower case.
    pub address: String,
    pub subject: String,
    /// Its words: its plain text part, else (none, or one with no digit
    /// in it: "view this in a browser") its HTML part's.
    pub text: String,
    /// Its links (those of its HTML part, where a button's address is
    /// and nowhere else; else the addresses written out in its plain
    /// text): what each is shown as, and where it goes. Only `https`.
    pub links: Vec<Link>,
}

/// A link in a message: the words it's shown as ("Sign in"), and where
/// it goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub url: String,
}

/// The most links kept of a message.
const MOST_LINKS: usize = 200;

/// The deepest multipart nesting followed.
const DEEPEST: usize = 8;

/// The message in `raw` (as much of it as there is: a message cut short
/// gives what it has).
pub fn parse(raw: &[u8]) -> Message {
    let (head, body) = split(raw);
    let headers = Headers::parse(head);
    let (name, address) = mailbox(&decode_words(headers.get("from").unwrap_or_default()));
    let subject = squeeze(&decode_words(headers.get("subject").unwrap_or_default()));
    let mut parts = Parts::default();
    walk(&headers, body, &mut parts, 0);
    let plain = parts.plain.unwrap_or_default();
    let mut links = parts.html.as_deref().map(html_links).unwrap_or_default();
    if links.is_empty() {
        links = plain_links(&plain);
    }
    let text = if plain.chars().any(|c| c.is_ascii_digit()) || parts.html.is_none() { plain } else { html_text(&parts.html.unwrap_or_default()) };
    Message { from: if name.is_empty() { address.clone() } else { name }, address, subject, text: text.trim().to_owned(), links }
}

/// An HTML part's links: each `<a href>` that goes to an `https`
/// address, with the words it's shown as (an image's alt text among
/// them: a button drawn as a picture).
pub fn html_links(html: &str) -> Vec<Link> {
    // (Lower case of the same length: ASCII's only.)
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<a") {
        let start = at + i;
        if !lower.as_bytes().get(start + 2).is_some_and(u8::is_ascii_whitespace) {
            at = start + 2;
            continue;
        }
        let close = tag_end(html.as_bytes(), start);
        let tag = &html[start + 1..close.saturating_sub(1).max(start + 1)];
        let end = lower[close..].find("</a").map_or(html.len(), |e| close + e);
        if let Some(href) = attribute(tag, "href") {
            let url = entities(href.trim());
            if url.starts_with("https://") && !url.contains(char::is_whitespace) {
                out.push(Link { text: html_text(&html[close..end]).split_whitespace().collect::<Vec<_>>().join(" "), url });
                if out.len() == MOST_LINKS {
                    break;
                }
            }
        }
        at = end.max(start + 2);
    }
    out
}

/// The `https` addresses written out in a plain text part, each with
/// the rest of its line as what it's shown as.
fn plain_links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for line in text.lines() {
        for word in line.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"')) {
            if word.starts_with("https://") && word.len() > 12 && out.len() < MOST_LINKS {
                let url = word.trim_end_matches(['.', ',', ';']).to_owned();
                let shown: Vec<&str> = line.split_whitespace().filter(|w| !w.contains("://")).collect();
                out.push(Link { text: shown.join(" "), url });
            }
        }
    }
    out
}

/// The text parts found, the first of each kind.
#[derive(Default)]
struct Parts {
    plain: Option<String>,
    html: Option<String>,
}

/// A message's (or a part's) headers and its body, at the first empty
/// line.
fn split(raw: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < raw.len() {
        let end = raw[i..].iter().position(|&b| b == b'\n').map_or(raw.len(), |p| i + p);
        let line = &raw[i..end];
        if line.is_empty() || line == b"\r" {
            return (&raw[..i], raw.get(end + 1..).unwrap_or_default());
        }
        i = end + 1;
    }
    (raw, &[])
}

/// Headers, unfolded, their names lower case.
struct Headers(Vec<(String, String)>);

impl Headers {
    fn parse(head: &[u8]) -> Headers {
        let text = String::from_utf8_lossy(head);
        let mut out: Vec<(String, String)> = Vec::new();
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.starts_with([' ', '\t']) {
                if let Some((_, v)) = out.last_mut() {
                    v.push_str(line);
                }
            } else if let Some((name, value)) = line.split_once(':') {
                out.push((name.trim().to_ascii_lowercase(), value.trim_start().to_owned()));
            }
        }
        Headers(out)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// A header's value and its parameters (`text/html; charset="utf-8"`):
/// the value lower case, each parameter's name lower case.
fn value_and_params(header: &str) -> (String, Vec<(String, String)>) {
    let mut params = Vec::new();
    let mut chars = header.char_indices().peekable();
    // The value, up to the first `;` not in quotes.
    let mut end = header.len();
    let mut quoted = false;
    for (i, c) in chars.by_ref() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let value = header[..end].trim().to_ascii_lowercase();
    let mut rest = header.get(end + 1..).unwrap_or_default();
    while !rest.trim().is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().trim_start_matches(';').trim().to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let (val, next) = if let Some(q) = after.strip_prefix('"') {
            let mut v = String::new();
            let mut escaped = false;
            let mut stop = q.len();
            for (i, c) in q.char_indices() {
                if escaped {
                    v.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    stop = i + 1;
                    break;
                } else {
                    v.push(c);
                }
            }
            let next = q.get(stop..).unwrap_or_default();
            (v, next.find(';').map_or("", |s| &next[s + 1..]))
        } else {
            let stop = after.find(';').unwrap_or(after.len());
            (after[..stop].trim().to_owned(), after.get(stop + 1..).unwrap_or_default())
        };
        if !name.is_empty() {
            params.push((name, val));
        }
        rest = next;
    }
    (value, params)
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(n, _)| n == name || n.strip_suffix('*') == Some(name) || n.strip_suffix("*0") == Some(name)).map(|(_, v)| v.as_str())
}

/// Walk a part: a text one kept (the first plain, the first HTML), a
/// multipart one's parts walked in turn.
fn walk(headers: &Headers, body: &[u8], parts: &mut Parts, depth: usize) {
    let (kind, params) = value_and_params(headers.get("content-type").unwrap_or("text/plain"));
    let (disposition, _) = value_and_params(headers.get("content-disposition").unwrap_or_default());
    if disposition == "attachment" {
        return;
    }
    if let Some(sub) = kind.strip_prefix("multipart/") {
        let _ = sub;
        let Some(boundary) = param(&params, "boundary") else { return };
        if depth >= DEEPEST {
            return;
        }
        for part in multipart(body, boundary) {
            let (head, body) = split(part);
            walk(&Headers::parse(head), body, parts, depth + 1);
        }
        return;
    }
    let is_plain = kind == "text/plain" || kind.is_empty();
    let is_html = kind == "text/html";
    if !(is_plain && parts.plain.is_none() || is_html && parts.html.is_none()) {
        return;
    }
    let (encoding, _) = value_and_params(headers.get("content-transfer-encoding").unwrap_or("7bit"));
    let bytes = match encoding.as_str() {
        "base64" => base64(body),
        "quoted-printable" => quoted_printable(body),
        _ => body.to_vec(),
    };
    let text = decode(&bytes, param(&params, "charset").unwrap_or("utf-8"));
    if is_plain {
        parts.plain = Some(text);
    } else {
        parts.html = Some(text);
    }
}

/// The parts of a multipart body, between its boundaries.
fn multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let open = format!("--{boundary}");
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < body.len() {
        let end = body[i..].iter().position(|&b| b == b'\n').map_or(body.len(), |p| i + p);
        let line = &body[i..end];
        let trimmed = line.strip_suffix(b"\r").unwrap_or(line);
        let trimmed = trimmed.trim_ascii_end();
        if trimmed.starts_with(open.as_bytes()) {
            let after = &trimmed[open.len()..];
            if after.is_empty() || after == b"--" {
                if let Some(s) = start {
                    // (The line break before a boundary is the boundary's.)
                    let mut e = i.saturating_sub(1).max(s);
                    if e > s && body[e - 1] == b'\r' {
                        e -= 1;
                    }
                    out.push(&body[s..e]);
                }
                if after == b"--" {
                    return out;
                }
                start = Some((end + 1).min(body.len()));
            }
        }
        i = end + 1;
    }
    // (Cut short: what there is of the last part.)
    if let Some(s) = start
        && s < body.len()
    {
        out.push(&body[s..]);
    }
    out
}

/// Base64's bytes, whatever lines and padding it has.
fn base64(body: &[u8]) -> Vec<u8> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const LENIENT: GeneralPurpose =
        GeneralPurpose::new(&base64::alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true));
    let clean: Vec<u8> = body.iter().copied().filter(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/').collect();
    // (A length no base64 has: its last odd character left out.)
    let usable = clean.len() - if clean.len() % 4 == 1 { 1 } else { 0 };
    LENIENT.decode(&clean[..usable]).unwrap_or_default()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Quoted-printable's bytes: `=XX` a byte, `=` at a line's end no break.
fn quoted_printable(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        if b == b'=' {
            if let (Some(h), Some(l)) = (body.get(i + 1).copied().and_then(hex), body.get(i + 2).copied().and_then(hex)) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
            // A soft line break (whatever spaces were left before it).
            let mut j = i + 1;
            while j < body.len() && matches!(body[j], b' ' | b'\t') {
                j += 1;
            }
            if body.get(j) == Some(&b'\r') && body.get(j + 1) == Some(&b'\n') {
                i = j + 2;
                continue;
            }
            if body.get(j) == Some(&b'\n') {
                i = j + 1;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

/// Bytes in `charset` as text (unknown ones as UTF-8).
fn decode(bytes: &[u8], charset: &str) -> String {
    let label = charset.trim().trim_matches('"');
    // (RFC 2231's language after a `*`: "utf-8*en".)
    let label = label.split('*').next().unwrap_or(label);
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// A header's encoded words (`=?utf-8?B?…?=`, `=?iso-8859-1?Q?…?=`)
/// decoded; spaces between two of them dropped.
pub fn decode_words(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let (before, from) = rest.split_at(start);
        let decoded = encoded_word(from);
        match decoded {
            Some((text, used)) => {
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &from[used..];
                last_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &from[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One encoded word at the start of `s`: its text and how many bytes it
/// took.
fn encoded_word(s: &str) -> Option<(String, usize)> {
    let inner = s.strip_prefix("=?")?;
    let q1 = inner.find('?')?;
    let charset = &inner[..q1];
    let after = &inner[q1 + 1..];
    let q2 = after.find('?')?;
    let enc = &after[..q2];
    let data_from = &after[q2 + 1..];
    let end = data_from.find("?=")?;
    let data = &data_from[..end];
    if data.contains(' ') {
        return None;
    }
    let bytes = match enc {
        "B" | "b" => base64(data.as_bytes()),
        "Q" | "q" => {
            let under: Vec<u8> = data.bytes().map(|b| if b == b'_' { b' ' } else { b }).collect();
            quoted_printable(&under)
        }
        _ => return None,
    };
    let used = 2 + q1 + 1 + q2 + 1 + end + 2;
    Some((decode(&bytes, charset), used))
}

/// A From header's name and address ("Acme <no-reply@acme.example>",
/// "\"Acme, Inc.\" <…>", "no-reply@acme.example (Acme)", a bare address).
fn mailbox(from: &str) -> (String, String) {
    let from = from.trim();
    if let (Some(lt), Some(gt)) = (from.rfind('<'), from.rfind('>'))
        && lt < gt
    {
        let address = from[lt + 1..gt].trim().to_ascii_lowercase();
        let name = squeeze(from[..lt].trim().trim_matches('"').replace("\\\"", "\"").trim());
        return (name, address);
    }
    if let (Some(open), Some(close)) = (from.find('('), from.rfind(')'))
        && open < close
    {
        return (squeeze(from[open + 1..close].trim()), from[..open].trim().to_ascii_lowercase());
    }
    (String::new(), from.to_ascii_lowercase())
}

/// Runs of white space made one space.
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The tags whose content is never words.
const UNSEEN: &[&str] = &["style", "script", "head", "title", "noscript", "template", "svg"];

/// Tags that begin or end a line of their own.
const BLOCKS: &[&str] = &[
    "br",
    "p",
    "div",
    "tr",
    "li",
    "ul",
    "ol",
    "table",
    "tbody",
    "thead",
    "tfoot",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "blockquote",
    "pre",
    "section",
    "article",
    "header",
    "footer",
    "center",
    "td",
    "th",
    "dt",
    "dd",
    "address",
    "form",
    "fieldset",
    "nav",
    "main",
];

/// An HTML part's words: its tags left out, a block's a line of its own,
/// entities as their characters, no zero-width or soft-hyphen characters
/// (a preheader's padding), spaces squeezed and empty lines too.
pub fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 3);
    let b = html.as_bytes();
    let mut i = 0;
    let mut text_from = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        out.push_str(&entities(&html[text_from..i]));
        // A comment, or a doctype or a CDATA.
        if html[i..].starts_with("<!--") {
            i = html[i + 4..].find("-->").map_or(b.len(), |e| i + 4 + e + 3);
            text_from = i;
            continue;
        }
        let close = tag_end(b, i);
        let tag = &html[i + 1..close.saturating_sub(1).max(i + 1)];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        i = close;
        text_from = i;
        if !closing && UNSEEN.contains(&name.as_str()) && !tag.ends_with('/') {
            // Its content, up to its end tag, isn't words.
            let end = format!("</{name}");
            let lower_from = i;
            let found = find_ignoring_case(&html[lower_from..], &end);
            i = match found {
                Some(at) => tag_end(b, lower_from + at),
                None => b.len(),
            };
            text_from = i;
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            out.push('\n');
        } else if name == "img" {
            // (An image's words, as a code is sometimes only shown so: its
            // alt text.)
            if let Some(alt) = attribute(tag, "alt") {
                out.push_str(&entities(&alt));
            }
        }
    }
    if text_from < b.len() {
        out.push_str(&entities(&html[text_from..]));
    }
    tidy(&out)
}

/// Where the tag that opens at `at` ends (just past its `>`), quotes in
/// it passed over.
fn tag_end(b: &[u8], at: usize) -> usize {
    let mut quote: Option<u8> = None;
    let mut i = at + 1;
    while i < b.len() {
        match (quote, b[i]) {
            (Some(q), c) if c == q => quote = None,
            (None, b'"' | b'\'') => quote = Some(b[i]),
            (None, b'>') => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

fn find_ignoring_case(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.len();
    hay.as_bytes().windows(n).position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// An attribute's value in a tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let at = find_ignoring_case(tag, &format!("{name}="))?;
    let rest = &tag[at + name.len() + 1..];
    let value = match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?,
        _ => rest.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next()?,
    };
    Some(value.to_owned())
}

/// Text with its entities as their characters.
fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').filter(|&p| p <= 10);
        let decoded = semi.and_then(|p| entity(&after[..p]).map(|c| (c, p)));
        match decoded {
            Some((c, p)) => {
                out.push_str(c.as_str());
                rest = &after[p + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let n = match num.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(n).map(String::from);
    }
    let c = match name {
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "rsquo" | "lsquo" => '’',
        "rdquo" | "ldquo" => '”',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "middot" | "bull" => '·',
        "zwnj" | "zwj" | "shy" => '\u{200b}',
        _ => return None,
    };
    Some(c.to_string())
}

/// Spaces squeezed in each line, characters that show nothing left out,
/// and no empty lines.
fn tidy(s: &str) -> String {
    let unseen = |c: char| matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{180e}');
    s.split('\n').map(|line| line.chars().filter(|c| !unseen(*c)).collect::<String>()).map(|line| squeeze(&line)).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_encoded_words() {
        assert_eq!(decode_words("=?UTF-8?B?WW91ciBjb2RlOiA0ODI5MTM=?="), "Your code: 482913");
        assert_eq!(decode_words("=?iso-8859-1?Q?Best=E4tigungscode_ist?= 482913"), "Bestätigungscode ist 482913");
        // (Spaces between two encoded words are dropped; others kept.)
        assert_eq!(decode_words("=?utf-8?q?Your_?= =?utf-8?q?code?= is here"), "Your code is here");
        assert_eq!(decode_words("Plain =? not one"), "Plain =? not one");
        assert_eq!(mailbox("\"Acme, Inc.\" <No-Reply@Acme.example>"), ("Acme, Inc.".into(), "no-reply@acme.example".into()));
        assert_eq!(mailbox("no-reply@acme.example (Acme)"), ("Acme".into(), "no-reply@acme.example".into()));
        assert_eq!(mailbox("no-reply@acme.example"), ("".into(), "no-reply@acme.example".into()));
        let (v, p) = value_and_params("multipart/alternative;\r\n\tboundary=\"b1;x\"; charset=UTF-8");
        assert_eq!(v, "multipart/alternative");
        assert_eq!(param(&p, "boundary"), Some("b1;x"));
        assert_eq!(param(&p, "charset"), Some("UTF-8"));
    }

    #[test]
    fn transfer_encodings() {
        assert_eq!(quoted_printable(b"Your code =\r\nis 48=3D2913 caf=C3=A9"), "Your code is 48=2913 café".as_bytes());
        assert_eq!(base64(b"WW91ciBj\r\nb2RlIGlz\r\nIDQ4MjkxMw"), b"Your code is 482913");
        assert_eq!(decode(b"Best\xe4tigung", "ISO-8859-1"), "Bestätigung");
        assert_eq!(decode(b"Best\xc3\xa4tigung", "utf8"), "Bestätigung");
    }

    /// A message as a service sends one: a plain part that only says to
    /// look at the HTML one, and the HTML one (quoted-printable, a style
    /// sheet, a hidden preheader, the code in a cell of its own).
    #[test]
    fn multipart_alternative_with_html() {
        let raw = concat!(
            "From: =?utf-8?q?Acme?= <no-reply@acme.example>\r\n",
            "Subject: Your sign-in\r\n code\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/alternative; boundary=\"XX\"\r\n",
            "\r\n",
            "This is a multi-part message.\r\n",
            "--XX\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "Open this message in a browser that shows HTML.\r\n",
            "--XX\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "Content-Transfer-Encoding: quoted-printable\r\n",
            "\r\n",
            "<html><head><style>td { color: #333; } .x{width:600px}</style><title>Ac=\r\n",
            "me</title></head><body><div style=3D\"display:none\">Sign in&nbsp;&zwnj;&nbsp;&zwnj;</div>",
            "<table><tr><td>Your verification code is:</td></tr><tr><td><b>482913</b></td></tr>",
            "<tr><td>It expires in 10&nbsp;minutes.</td></tr></table>",
            "<!-- 999999 --><script>var x = 123456;</script></body></html>\r\n",
            "--XX--\r\n",
        );
        let m = parse(raw.as_bytes());
        assert_eq!(m.from, "Acme");
        assert_eq!(m.address, "no-reply@acme.example");
        assert_eq!(m.subject, "Your sign-in code");
        assert_eq!(m.text, "Sign in\nYour verification code is:\n482913\nIt expires in 10 minutes.");
    }

    /// Nested multiparts (mixed, then related, then alternative), base64
    /// text, an attachment passed over, a part cut short.
    #[test]
    fn nested_parts() {
        let raw = concat!(
            "From: GitHub <noreply@github.com>\n",
            "Subject: [GitHub] Please verify your device\n",
            "Content-Type: multipart/mixed; boundary=outer\n",
            "\n",
            "--outer\n",
            "Content-Type: multipart/alternative; boundary=inner\n",
            "\n",
            "--inner\n",
            "Content-Type: text/plain; charset=\"us-ascii\"\n",
            "Content-Transfer-Encoding: base64\n",
            "\n",
            "VmVyaWZpY2F0aW9uIGNvZGU6IDQ4MjkxMw==\n",
            "--inner--\n",
            "--outer\n",
            "Content-Type: text/plain\n",
            "Content-Disposition: attachment; filename=a.txt\n",
            "\n",
            "123456\n",
            "--outer\n",
            "Content-Type: text/html\n",
            "\n",
            "<p>cut short",
        );
        let m = parse(raw.as_bytes());
        assert_eq!(m.from, "GitHub");
        assert_eq!(m.text, "Verification code: 482913");
    }

    /// A button's address is only in the HTML part: kept, with what the
    /// button says, whichever part gives the words.
    #[test]
    fn a_messages_links() {
        let raw = b"From: Acme <login@mail.acme.example>\r\nSubject: Your secure link to Acme is here\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSign in to Acme\r\nClick the button below to finish signing in. This link expires in 10 minutes.\r\nSign in\r\n--b\r\nContent-Type: text/html\r\n\r\n<html><body><p>Click the button below to finish signing in.</p><A class=\"btn\"\r\n HREF=\"https://acme.example/magic-link?t=abc&amp;u=7#x\"><span>Sign&nbsp;in</span></A> <a href='http://acme.example/plain'>old</a> <a href=\"https://acme.example/help\"><img alt=\"Help\" src=\"x.png\"></a><abbr title=\"x\">not a link</abbr></body></html>\r\n--b--\r\n";
        let m = parse(raw);
        assert!(m.text.starts_with("Sign in to Acme"));
        assert_eq!(m.links, [Link { text: "Sign in".into(), url: "https://acme.example/magic-link?t=abc&u=7#x".into() }, Link { text: "Help".into(), url: "https://acme.example/help".into() }]);
        // Plain text alone: the addresses written out.
        let m = parse(b"From: a@b.example\r\nSubject: Sign in\r\n\r\nUse this link to sign in: https://acme.example/l/abc123def.\r\nNothing else.\r\n");
        assert_eq!(m.links, [Link { text: "Use this link to sign in:".into(), url: "https://acme.example/l/abc123def".into() }]);
        assert!(parse(b"Subject: hi\r\n\r\nno links\r\n").links.is_empty());
    }

    #[test]
    fn html_words() {
        assert_eq!(html_text("<p>Use <span>4829</span>13</p><p>a&amp;b &#x41;&#66;</p>"), "Use 482913\na&b AB");
        assert_eq!(html_text("<img src=\"x.png\" alt='Code 482913'> <a href=\"x>y\">link</a>"), "Code 482913 link");
        assert_eq!(html_text("<STYLE>p{}</STYLE>Hi<SCRIPT>1</SCRIPT>"), "Hi");
    }
}
