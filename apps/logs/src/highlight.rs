//! Syntax highlighting for log text: what a line is made of, as spans.
//! The module path a `tracing` line starts with, `key=value` keys, quoted
//! strings, numbers (and hex, durations, sizes), paths and URLs, type
//! names (`TemporaryFailure(`, `Some {`), and the words that say
//! something went wrong.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    /// `app_server::udev:` in front.
    Target,
    Key,
    Str,
    Number,
    Path,
    Type,
    Bad,
    /// Punctuation: brackets, `=`, `:`.
    Punct,
}

/// Words that mark trouble.
const BAD: &[&str] =
    &["error", "errors", "failed", "failure", "fail", "fails", "couldn't", "couldn’t", "can't", "can’t", "cannot", "panicked", "panic", "fatal", "denied", "refused", "invalid", "unable", "missing", "crashed", "timeout", "timed"];

/// The spans of `text` and what each is; they cover it in order.
pub fn spans(text: &str) -> Vec<(Range<usize>, Kind)> {
    let b = text.as_bytes();
    let mut out: Vec<(Range<usize>, Kind)> = Vec::new();
    let push = |out: &mut Vec<(Range<usize>, Kind)>, r: Range<usize>, k: Kind| {
        if r.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((last, lk)) if *lk == k && last.end == r.start => last.end = r.end,
            _ => out.push((r, k)),
        }
    };
    let mut i = 0;
    // A tracing target: `word::word…:` then a space, at the very start.
    if let Some(end) = target_end(text) {
        push(&mut out, 0..end, Kind::Target);
        i = end;
    }
    while i < b.len() {
        let c = b[i];
        let rest = &text[i..];
        // Quoted strings ("…", '…', “…”).
        if c == b'"' || (c == b'\'' && (i == 0 || !b[i - 1].is_ascii_alphanumeric())) || rest.starts_with('“') {
            let (open_len, close) = if rest.starts_with('“') { ('“'.len_utf8(), '”') } else { (1, c as char) };
            let mut j = i + open_len;
            let mut escaped = false;
            let mut end = None;
            for (k, ch) in text[j..].char_indices() {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == close {
                    end = Some(j + k + ch.len_utf8());
                    break;
                } else if ch == '\n' {
                    break;
                }
            }
            if let Some(e) = end {
                push(&mut out, i..e, Kind::Str);
                i = e;
                continue;
            }
            j = i + open_len;
            push(&mut out, i..j, Kind::Plain);
            i = j;
            continue;
        }
        // URLs and paths.
        if rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("file://") || ((c == b'/' || rest.starts_with("~/")) && (i == 0 || b" (=[\"'".contains(&b[i - 1])) && rest.len() > 1 && !rest[1..].starts_with([' ', '/'])) {
            let e = i + rest.find(|ch: char| ch.is_whitespace() || "\"'(),;<>[]{}".contains(ch)).unwrap_or(rest.len());
            let e = e - text[i..e].len() + text[i..e].trim_end_matches(['.', ':']).len();
            push(&mut out, i..e, Kind::Path);
            i = e;
            continue;
        }
        // Words: keys, types, numbers, bad words.
        let word_len = rest.find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '\'' || ch == '’' || ch == '.' || ch == '-')).unwrap_or(rest.len());
        if word_len > 0 && rest.starts_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
            let e = i + word_len;
            // A trailing dot or hyphen is punctuation, not the word.
            let e = e - text[i..e].len() + text[i..e].trim_end_matches(['.', '-', '\'', '’']).len();
            let word = &text[i..e];
            let after = text[e..].trim_start_matches(' ');
            let kind = if is_number(word) {
                Kind::Number
            } else if after.starts_with('=') && !after.starts_with("==") || (after.starts_with(':') && !after.starts_with("::") && word.chars().all(|c| c.is_ascii_lowercase() || c == '_') && word.contains('_')) {
                Kind::Key
            } else if word.chars().next().is_some_and(char::is_uppercase) && (after.starts_with('(') || after.starts_with('{')) {
                Kind::Type
            } else if BAD.contains(&word.to_lowercase().as_str()) {
                Kind::Bad
            } else {
                Kind::Plain
            };
            if e > i {
                push(&mut out, i..e, kind);
                i = e;
                continue;
            }
        }
        let ch_len = rest.chars().next().map_or(1, char::len_utf8);
        let kind = if "()[]{}=:,<>|".contains(c as char) { Kind::Punct } else { Kind::Plain };
        push(&mut out, i..i + ch_len, kind);
        i += ch_len;
    }
    out
}

/// Where a leading `crate::module:` target ends (after its colon and space).
fn target_end(text: &str) -> Option<usize> {
    let head = text.split_once(": ")?.0;
    if head.len() > 80 || !head.contains("::") || !head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        return None;
    }
    Some(head.len() + 1)
}

/// `42`, `-3`, `0x1f`, `1.5`, `12ms`, `3.2s`, `640x480`, `4KiB`, a version.
fn is_number(word: &str) -> bool {
    let w = word.strip_prefix('-').unwrap_or(word);
    if let Some(h) = w.strip_prefix("0x") {
        return !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit());
    }
    let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &w[digits.len()..];
    !digits.is_empty()
        && digits.starts_with(|c: char| c.is_ascii_digit())
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 'x')
        && matches!(unit, "" | "ms" | "us" | "µs" | "ns" | "s" | "m" | "h" | "d" | "px" | "b" | "B" | "KB" | "MB" | "GB" | "KiB" | "MiB" | "GiB" | "K" | "M" | "G" | "Hz" | "x" | "fps" | "pt")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Kind)> {
        spans(text).into_iter().filter(|(_, k)| *k != Kind::Plain && *k != Kind::Punct).map(|(r, k)| (&text[r], k)).collect()
    }

    #[test]
    fn a_tracing_line() {
        assert_eq!(
            kinds("app_server::udev: Error during rendering: TemporaryFailure(5ms) path=/dev/dri/card0 name=\"HDMI-1\""),
            vec![
                ("app_server::udev:", Kind::Target),
                ("Error", Kind::Bad),
                ("TemporaryFailure", Kind::Type),
                ("5ms", Kind::Number),
                ("path", Kind::Key),
                ("/dev/dri/card0", Kind::Path),
                ("name", Kind::Key),
                ("\"HDMI-1\"", Kind::Str),
            ]
        );
    }

    #[test]
    fn spans_cover_the_text() {
        for text in ["", "plain words.", "a “quoted” bit and 'x' and don't", "see https://example.com/a?b=1, then ~/x", "0x1f -3 1.5 v1.2.3 ünïcode", "unterminated \"quote", "an… ellipsis — and ’quotes’ ✓"] {
            let s = spans(text);
            let mut at = 0;
            for (r, _) in &s {
                assert_eq!(r.start, at, "{text}");
                at = r.end;
            }
            assert_eq!(at, text.len(), "{text}");
        }
        assert_eq!(kinds("don't panic"), vec![("panic", Kind::Bad)]);
        assert_eq!(kinds("see https://example.com/a?b=1, then"), vec![("https://example.com/a?b=1", Kind::Path)]);
    }
}
