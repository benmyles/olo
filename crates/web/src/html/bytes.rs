//! Bounded HTML encoding prescan. The first 1024 bytes are inspected as bytes,
//! before character decoding (attribute references are deliberately not expanded).
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252, X_USER_DEFINED};
/// BOM, transport metadata, meta prescan, then the browser fallback.
pub fn encoding(bytes: &[u8], protocol: Option<&str>) -> (&'static Encoding, usize) {
    if let Some(choice) = Encoding::for_bom(bytes) {
        return choice;
    }
    if let Some(choice) = protocol.and_then(|s| Encoding::for_label(s.as_bytes())) {
        return (choice, 0);
    }
    let bytes = &bytes[..bytes.len().min(1024)];
    if bytes.starts_with(b"<\0?\0x\0") {
        return (UTF_16LE, 0);
    }
    if bytes.starts_with(b"\0<\0?\0x") {
        return (UTF_16BE, 0);
    }
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'<' {
            at += 1;
            continue;
        }
        if bytes[at..].starts_with(b"<!--") {
            at += 2;
            while at + 2 < bytes.len() && &bytes[at..at + 3] != b"-->" {
                at += 1;
            }
            at = (at + 3).min(bytes.len());
            continue;
        }
        let tag = bytes.get(at + 1).is_some_and(u8::is_ascii_alphabetic) || bytes.get(at + 1) == Some(&b'/') && bytes.get(at + 2).is_some_and(u8::is_ascii_alphabetic);
        if !tag {
            if bytes.get(at + 1).is_some_and(|b| matches!(b, b'!' | b'/' | b'?')) {
                while at < bytes.len() && bytes[at] != b'>' {
                    at += 1;
                }
            }
            at += 1;
            continue;
        }
        at += 1;
        let start = at;
        while at < bytes.len() && !space(bytes[at]) && !matches!(bytes[at], b'/' | b'>') {
            at += 1;
        }
        let meta = bytes[start..at].eq_ignore_ascii_case(b"meta");
        let mut attrs: Vec<(&[u8], &[u8])> = Vec::new();
        while at < bytes.len() && bytes[at] != b'>' {
            while at < bytes.len() && (space(bytes[at]) || bytes[at] == b'/') {
                at += 1;
            }
            let start = at;
            while at < bytes.len() && !space(bytes[at]) && !matches!(bytes[at], b'=' | b'>' | b'/') {
                at += 1;
            }
            if at == start {
                if at < bytes.len() && bytes[at] != b'>' {
                    at += 1;
                }
                continue;
            }
            let name = &bytes[start..at];
            while at < bytes.len() && space(bytes[at]) {
                at += 1;
            }
            let mut value = &bytes[at..at];
            if bytes.get(at) == Some(&b'=') {
                at += 1;
                while at < bytes.len() && space(bytes[at]) {
                    at += 1;
                }
                let quote = bytes.get(at).copied().filter(|b| matches!(b, b'\'' | b'"'));
                if quote.is_some() {
                    at += 1;
                }
                let start = at;
                while at < bytes.len()
                    && match quote {
                        Some(q) => bytes[at] != q,
                        None => !space(bytes[at]) && bytes[at] != b'>',
                    }
                {
                    at += 1;
                }
                value = &bytes[start..at];
                if quote.is_some() && at < bytes.len() {
                    at += 1;
                }
            }
            if meta && !attrs.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                attrs.push((name, value));
            }
        }
        if bytes.get(at) != Some(&b'>') {
            break;
        }
        at += 1;
        let mut pragma = false;
        let mut need_pragma = None;
        let mut charset = None;
        let mut chosen = false;
        for (name, value) in attrs {
            if name.eq_ignore_ascii_case(b"http-equiv") {
                pragma = value.eq_ignore_ascii_case(b"content-type");
            } else if name.eq_ignore_ascii_case(b"charset") {
                charset = Encoding::for_label(value);
                need_pragma = Some(false);
                chosen = true;
            } else if name.eq_ignore_ascii_case(b"content") && !chosen {
                charset = content_encoding(value);
                if charset.is_some() {
                    need_pragma = Some(true);
                    chosen = true;
                }
            }
        }
        if let Some(mut charset) = charset
            && (need_pragma == Some(false) || need_pragma == Some(true) && pragma)
        {
            if charset == UTF_16LE || charset == UTF_16BE {
                charset = UTF_8;
            }
            if charset == X_USER_DEFINED {
                charset = WINDOWS_1252;
            }
            return (charset, 0);
        }
    }
    (xml_encoding(bytes).unwrap_or(WINDOWS_1252), 0)
}
fn xml_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    if !bytes.starts_with(b"<?xml") {
        return None;
    }
    let declaration = &bytes[..bytes.iter().position(|b| *b == b'>')?];
    let mut at = declaration.windows(8).position(|s| s == b"encoding")? + 8;
    while declaration.get(at).is_some_and(|b| *b <= 0x20) {
        at += 1;
    }
    if declaration.get(at) != Some(&b'=') {
        return None;
    }
    at += 1;
    while declaration.get(at).is_some_and(|b| *b <= 0x20) {
        at += 1;
    }
    let quote = *declaration.get(at)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    at += 1;
    let end = declaration[at..].iter().position(|b| *b == quote)? + at;
    let encoding = Encoding::for_label(&declaration[at..end])?;
    Some(if encoding == UTF_16LE || encoding == UTF_16BE { UTF_8 } else { encoding })
}
fn space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}
fn content_encoding(value: &[u8]) -> Option<&'static Encoding> {
    let mut at = 0;
    while at + 7 <= value.len() {
        if !value[at..at + 7].eq_ignore_ascii_case(b"charset") {
            at += 1;
            continue;
        }
        at += 7;
        while at < value.len() && space(value[at]) {
            at += 1;
        }
        if value.get(at) != Some(&b'=') {
            continue;
        }
        at += 1;
        while at < value.len() && space(value[at]) {
            at += 1;
        }
        let quote = value.get(at).copied().filter(|b| matches!(b, b'\'' | b'"'));
        if quote.is_some() {
            at += 1;
        }
        let start = at;
        while at < value.len()
            && match quote {
                Some(q) => value[at] != q,
                None => !space(value[at]) && value[at] != b';',
            }
        {
            at += 1;
        }
        return Encoding::for_label(&value[start..at]);
    }
    None
}
