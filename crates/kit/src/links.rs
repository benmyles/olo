//! Links found in plain text, as people write them: web addresses
//! (`https://…`, any scheme's `…://…`, `www.…`), mail addresses
//! (`name@example.org`, `mailto:…`) and telephone numbers
//! (`+1 650-555-0100`, `(650) 555-0100`, `650-555-0100`, `tel:…`):
//! where each is, and where it goes. A text view shows them as links
//! ([`TextView::links`](crate::widgets::TextView::links)), and a click
//! opens one ([`open`]): a page in the browser, an address in the mail
//! app, a number in Phone, to call it.
//!
//! What's found is what a reader takes for a link, no more. A sentence's
//! full stop or a closing bracket after an address isn't part of it (one
//! opened inside it is: Wikipedia's `…_(disambiguation)`). A number is
//! one only as numbers to dial are written: with its country code (`+…`,
//! as invitations give their dial-ins), or as a North American one is.
//! A meeting's ID or PIN (`845 1234 5678`), a date, a time or a version
//! isn't one.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Web,
    Mail,
    Phone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Its bytes in the text.
    pub range: Range<usize>,
    pub kind: Kind,
    /// Where it goes: a web address (`https://` before a bare `www.`),
    /// `mailto:…`, `tel:` and the number's groups of digits as it groups
    /// them, a `-` apart (RFC 3966's visual separator: `tel:+1-650-555-0100`,
    /// shown so where it's opened), a trunk's `(0)` left out.
    pub target: String,
}

impl Link {
    /// What Copy Link copies: the web address, the mail address (not
    /// `mailto:`), the number as it's written.
    pub fn copied(&self, text: &str) -> String {
        match self.kind {
            Kind::Web => self.target.clone(),
            Kind::Mail => {
                let address = self.target.trim_start_matches("mailto:");
                address.split('?').next().unwrap_or(address).to_owned()
            }
            Kind::Phone => {
                let written = text.get(self.range.clone()).unwrap_or("");
                strip_prefix_ci(written, "tel:").unwrap_or(written).to_owned()
            }
        }
    }
}

/// The links in `text`, in order and apart. Done once for a text, not as
/// it's drawn: a page of notes takes microseconds (`finding_cost`).
pub fn find(text: &str) -> Vec<Link> {
    // Addresses, word by word (none holds a space)…
    let mut addresses = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                addresses.extend(address_in(text, s..i));
                start = None;
            }
            _ => {}
        }
    }
    // …then numbers, in what's between them (a number may hold spaces).
    let mut found = Vec::with_capacity(addresses.len());
    let mut at = 0;
    for link in addresses {
        numbers_in(text, at..link.range.start, &mut found);
        at = link.range.end;
        found.push(link);
    }
    numbers_in(text, at..text.len(), &mut found);
    found
}

/// Opens `link` with what opens its kind (`olo_xdg::open`): on a thread
/// of its own, as finding the app reads files.
pub fn open(link: &Link) {
    let target = link.target.clone();
    std::thread::spawn(move || {
        if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(target)) {
            tracing::warn!("open a link: {e}");
        }
    });
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

/// What can't be in an address written in text: it's quoting or
/// bracketing it.
fn ends_address(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^' | '“' | '”' | '‘' | '’' | '«' | '»')
}

/// `s` without what ends a sentence or closes a bracket after it: a
/// closing bracket stays if it closes one opened inside.
fn trim_end(s: &str) -> &str {
    let mut s = s;
    while let Some(last) = s.chars().last() {
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '…' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            _ => false,
        };
        if !drop {
            break;
        }
        s = &s[..s.len() - last.len_utf8()];
    }
    s
}

/// The schemes a web address found in text may have: what a browser
/// or a calendar opens. (Text can come from anyone, an invitation's
/// notes: another app's scheme isn't a click away.)
const SCHEMES: &[&str] = &["https", "http", "ftp", "webcal", "file"];

/// The address in the word at `word` of `text`, if it holds one.
fn address_in(text: &str, word: Range<usize>) -> Option<Link> {
    let w = &text[word.clone()];
    // From the first character that can start one (past an opening
    // bracket or quote), to the first that can't be in one; each kind
    // then from where it starts, less what ends a sentence after it.
    let lead = w.find(|c: char| c.is_alphanumeric() || c == '+').unwrap_or(w.len());
    let end = w[lead..].find(ends_address).map_or(w.len(), |e| lead + e);
    let core = &w[lead..end];
    let at = word.start + lead;
    let link = |offset: usize, len: usize, kind, target| Link { range: at + offset..at + offset + len, kind, target };

    if let Some(sep) = core.find("://") {
        // Its scheme: the letters before (`Join:https://…`).
        let start = core[..sep].rfind(|c: char| !c.is_ascii_alphabetic()).map_or(0, |i| i + 1);
        let url = trim_end(&core[start..]);
        let scheme = &core[start..sep];
        let rest = url.get(sep - start + 3..).unwrap_or("");
        if !SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)) || rest.is_empty() || rest.starts_with('/') && !scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        return Some(link(start, url.len(), Kind::Web, url.to_owned()));
    }
    let core = trim_end(core);
    if let Some(rest) = strip_prefix_ci(core, "mailto:") {
        return mail_address(rest).map(|_| link(0, core.len(), Kind::Mail, format!("mailto:{rest}")));
    }
    if let Some(rest) = strip_prefix_ci(core, "tel:") {
        let (digits, len) = international(rest).or_else(|| north_american(rest))?;
        return Some(link(0, 4 + len, Kind::Phone, format!("tel:{digits}")));
    }
    if strip_prefix_ci(core, "www.").is_some_and(|rest| rest.contains('.') && !rest.starts_with('.')) {
        return Some(link(0, core.len(), Kind::Web, format!("https://{core}")));
    }
    // A mail address: its name's characters before the `@` (from where
    // they start), a domain after.
    let at_sign = core.find('@')?;
    let name_start = core[..at_sign].rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))).map_or(0, |i| i + 1);
    let address = &core[name_start..];
    let len = mail_address(address)?;
    Some(link(name_start, len, Kind::Mail, format!("mailto:{}", &address[..len])))
}

/// How long the mail address `s` starts with is (a name, `@`, a domain
/// of two labels or more, the last of letters), if it starts with one.
fn mail_address(s: &str) -> Option<usize> {
    let (name, rest) = s.split_once('@')?;
    if name.is_empty() || name.starts_with('.') || name.ends_with('.') || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')) {
        return None;
    }
    let domain_len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')).unwrap_or(rest.len());
    let domain = rest[..domain_len].trim_end_matches(['.', '-']);
    let labels: Vec<&str> = domain.split('.').collect();
    let tld = labels.last()?;
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty() || l.starts_with('-')) || tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(name.len() + 1 + domain.len())
}

/// Whether `c` joins what's before or after it into one word with it (a
/// number can't start or end against one).
fn joins(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The numbers to dial in `range` of `text`, added to `out`.
fn numbers_in(text: &str, range: Range<usize>, out: &mut Vec<Link>) {
    let s = &text[range.clone()];
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let before = text[..range.start + i].chars().next_back();
        let starts = before.is_none_or(|b| !joins(b) && !matches!(b, '+' | '-' | '.' | '/' | '#' | '$' | '€' | '£'));
        if starts && (c == '+' || c == '(' || c.is_ascii_digit()) {
            let found = if c == '+' { international(rest) } else { north_american(rest) };
            // (Against the next word, or more digits: not a number.)
            if let Some((digits, len)) = found.filter(|(_, len)| s[i + len..].chars().next().is_none_or(|n| !joins(n) && n != '@'))
                && range.start + i + len <= range.end
            {
                out.push(Link { range: range.start + i..range.start + i + len, kind: Kind::Phone, target: format!("tel:{digits}") });
                i += len;
                continue;
            }
        }
        i += c.len_utf8();
    }
}

/// A number with its country code at the start of `s` (`+`, then groups
/// of digits a space, `-` or `.` apart, one in brackets as `+1 (650)` or
/// a trunk's `(0)`, which isn't dialled): its groups after a `+`, a `-`
/// apart, and its length in `s`. Seven digits to fifteen (E.164's most); a North
/// American one (`+1`) ends at its eleventh.
fn international(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'+') || !b.get(1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut digits = String::from("+");
    let mut grouped = String::from("+");
    let (mut i, mut end) = (1, 1);
    loop {
        // A group: digits, or digits in brackets.
        let bracketed = b.get(i) == Some(&b'(');
        let from = i + bracketed as usize;
        let mut j = from;
        while b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if j == from || bracketed && b.get(j) != Some(&b')') {
            break;
        }
        let group = &s[from..j];
        let trunk = bracketed && group == "0" && digits.len() > 1;
        if !trunk {
            if digits.len() - 1 + group.len() > 15 {
                break;
            }
            digits.push_str(group);
            if grouped.len() > 1 {
                grouped.push('-');
            }
            grouped.push_str(group);
        }
        i = j + bracketed as usize;
        end = i;
        if digits.starts_with("+1") && digits.len() - 1 >= 11 {
            break;
        }
        // One separator, then the next group (none after a bracket's
        // close is needed: `+1 (650)555-0100`).
        match b.get(i) {
            Some(b' ' | b'-' | b'.') => i += 1,
            Some(b'(') if bracketed => {}
            _ if bracketed && b.get(i).is_some_and(u8::is_ascii_digit) => {}
            _ => break,
        }
        if !(b.get(i).is_some_and(u8::is_ascii_digit) || b.get(i) == Some(&b'(')) {
            break;
        }
    }
    (digits.len() - 1 >= 7).then_some((grouped, end))
}

/// A North American number at the start of `s`: `(650) 555-0100`,
/// `650-555-0100` or `650.555.0100` (the same separator twice), after a
/// `1-` or `1 ` or not: its groups a `-` apart, and its length in `s`.
fn north_american(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let digits_at = |at: usize, n: usize| (at + n <= b.len() && b[at..at + n].iter().all(u8::is_ascii_digit)).then(|| &s[at..at + n]);
    let (country, mut i) = match (b.first(), b.get(1)) {
        (Some(b'1'), Some(b'-' | b' ' | b'.')) if b.get(2).is_some_and(|c| c.is_ascii_digit() || *c == b'(') => ("1", 2),
        _ => ("", 0),
    };
    let mut digits = if country.is_empty() { String::new() } else { format!("{country}-") };
    if b.get(i) == Some(&b'(') {
        // `(650) 555-0100`, `(650)555-0100`, `(650) 555 0100`.
        digits.push_str(digits_at(i + 1, 3)?);
        digits.push('-');
        if b.get(i + 4) != Some(&b')') {
            return None;
        }
        i += 5;
        if matches!(b.get(i), Some(b' ' | b'-')) {
            i += 1;
        }
        digits.push_str(digits_at(i, 3)?);
        digits.push('-');
        i += 3;
        b.get(i).filter(|c| matches!(c, b'-' | b'.' | b' '))?;
    } else {
        // `650-555-0100`, `650.555.0100`: the same separator twice.
        digits.push_str(digits_at(i, 3)?);
        i += 3;
        let sep = *b.get(i).filter(|c| matches!(c, b'-' | b'.'))?;
        digits.push('-');
        digits.push_str(digits_at(i + 1, 3)?);
        digits.push('-');
        i += 4;
        if b.get(i) != Some(&sep) {
            return None;
        }
    }
    digits.push_str(digits_at(i + 1, 4)?);
    Some((digits, i + 5))
}

/// How near the right edge a piece of a broken word ends: a program
/// wrapping its own text (Claude Code) may keep a column or two free.
const EDGE: usize = 4;
/// How far in the rest of a broken word may start (its text's indent).
const INDENT: usize = 8;
/// How many rows a broken word may cover.
const PIECES: usize = 24;

/// A word a program broke over rows itself, put together again (for a
/// terminal's links: a long address at the edge of a program's text,
/// the rest of it on the next rows, as Claude Code writes them).
///
/// The word at column `col` of row `row`, with the rest of it a program
/// broke over the rows around (`rows`, each a terminal row's characters,
/// `cols` wide): a piece ends at its row's last character, within
/// [`EDGE`] of the right, and the next is the first word of the next row,
/// indented at most [`INDENT`]. The pieces joined, where the click is in
/// that, and each character's (row, column). None unless there are two or
/// more pieces.
pub fn broken(rows: &[Vec<char>], row: usize, col: usize, cols: usize) -> Option<(String, usize, Vec<(usize, usize)>)> {
    let blank = |c: char| c == ' ' || c == '\0';
    let line = rows.get(row)?;
    if line.get(col).is_none_or(|c| blank(*c)) {
        return None;
    }
    let first = |r: &Vec<char>| r.iter().position(|c| !blank(*c));
    let end = |r: &Vec<char>| r.iter().rposition(|c| !blank(*c)).map(|i| i + 1);
    let word = |r: &Vec<char>, at: usize| {
        let s = (0..=at).rev().take_while(|&i| !blank(r[i])).last().unwrap_or(at);
        let e = (at..r.len()).take_while(|&i| !blank(r[i])).last().map_or(at, |i| i + 1);
        (s, e)
    };
    // Box drawing (a TUI's frames, Claude Code's ⎿) is never part of one.
    let plain = |r: &Vec<char>, (s, e): (usize, usize)| r[s..e].iter().all(|c| !('\u{2500}'..='\u{257f}').contains(c) && *c != '⎿' && *c != '⏺');
    let reaches = |r: &Vec<char>, e: usize| end(r) == Some(e) && e + EDGE >= cols;
    let mut pieces = vec![(row, word(line, col))];
    // Back: the clicked word starts its row, and the row before ends on a word.
    while pieces.len() < PIECES {
        let (r, (s, _)) = pieces[0];
        if r == 0 || first(&rows[r]) != Some(s) || s > INDENT {
            break;
        }
        let prev = &rows[r - 1];
        let Some(e) = end(prev).filter(|&e| reaches(prev, e)) else { break };
        let w = word(prev, e - 1);
        if !plain(prev, w) {
            break;
        }
        pieces.insert(0, (r - 1, w));
    }
    // On: this piece ends its row at the edge, and the next row starts with a word.
    while pieces.len() < PIECES {
        let &(r, (_, e)) = pieces.last().unwrap();
        let Some(next) = rows.get(r + 1) else { break };
        let Some(s) = first(next).filter(|&s| s <= INDENT) else { break };
        if !reaches(&rows[r], e) {
            break;
        }
        let w = word(next, s);
        if !plain(next, w) {
            break;
        }
        pieces.push((r + 1, w));
    }
    if pieces.len() < 2 || !pieces.iter().all(|&(r, w)| plain(&rows[r], w)) {
        return None;
    }
    let mut text = String::new();
    let mut cells = Vec::new();
    let mut at = 0;
    for &(r, (s, e)) in &pieces {
        if r == row {
            at = cells.len() + (col - s);
        }
        for c in s..e {
            text.push(rows[r][c]);
            cells.push((r, c));
        }
    }
    Some((text, at, cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What's found in `text`: each link's words and where it goes.
    fn found(text: &str) -> Vec<(&str, String)> {
        find(text).into_iter().map(|l| (&text[l.range.clone()], l.target)).collect()
    }

    #[test]
    fn web_addresses() {
        assert_eq!(found("See https://example.com/a?b=c#d."), [("https://example.com/a?b=c#d", "https://example.com/a?b=c#d".into())]);
        assert_eq!(found("(https://example.com/x)"), [("https://example.com/x", "https://example.com/x".into())]);
        assert_eq!(found("https://en.wikipedia.org/wiki/Mercury_(planet), then"), [("https://en.wikipedia.org/wiki/Mercury_(planet)", "https://en.wikipedia.org/wiki/Mercury_(planet)".into())]);
        assert_eq!(found("<https://a.org/b>"), [("https://a.org/b", "https://a.org/b".into())]);
        assert_eq!(found("“https://a.org/b”"), [("https://a.org/b", "https://a.org/b".into())]);
        assert_eq!(found("go to www.rust-lang.org."), [("www.rust-lang.org", "https://www.rust-lang.org".into())]);
        assert_eq!(found("[the notes](https://a.org/n)"), [("https://a.org/n", "https://a.org/n".into())]);
        assert_eq!(found("Join:https://meet.google.com/abc-defg-hij"), [("https://meet.google.com/abc-defg-hij", "https://meet.google.com/abc-defg-hij".into())]);
        assert_eq!(found("webcal://example.com/cal.ics"), [("webcal://example.com/cal.ics", "webcal://example.com/cal.ics".into())]);
        // Not addresses, nor another app's scheme.
        assert!(found("a www. b, :// alone, https:// and nothing, www.example, https:///x").is_empty());
        assert!(found("zoommtg://zoom.us/join?confno=1 x-app://do-it javascript://%0aalert(1)").is_empty());
    }

    #[test]
    fn mail_addresses() {
        assert_eq!(found("Write to sam.lee+notes@example.co.uk."), [("sam.lee+notes@example.co.uk", "mailto:sam.lee+notes@example.co.uk".into())]);
        assert_eq!(found("(mailto:help@example.org?subject=Hi)"), [("mailto:help@example.org?subject=Hi", "mailto:help@example.org?subject=Hi".into())]);
        assert_eq!(found("email:ops@example.org"), [("ops@example.org", "mailto:ops@example.org".into())]);
        assert!(found("@example.org a@b @here x@y.1 user@localhost").is_empty());
        let l = &find("mailto:help@example.org?subject=Hi")[0];
        assert_eq!(l.copied("mailto:help@example.org?subject=Hi"), "help@example.org");
    }

    #[test]
    fn numbers_to_dial() {
        // As invitations give their dial-ins.
        assert_eq!(found("Join by phone (US) +1 650-555-0100 PIN: 123 456 789#"), [("+1 650-555-0100", "tel:+1-650-555-0100".into())]);
        assert_eq!(found("+16465550100,,84512345678# US (New York)"), [("+16465550100", "tel:+16465550100".into())]);
        assert_eq!(found("+1 646 555 0100 US (New York)"), [("+1 646 555 0100", "tel:+1-646-555-0100".into())]);
        assert_eq!(found("+44 (0)20 7946 0018"), [("+44 (0)20 7946 0018", "tel:+44-20-7946-0018".into())]);
        assert_eq!(found("+1 (650) 555-0100."), [("+1 (650) 555-0100", "tel:+1-650-555-0100".into())]);
        assert_eq!(found("(+1 650-555-0100)"), [("+1 650-555-0100", "tel:+1-650-555-0100".into())]);
        assert_eq!(found("+49 30 901820"), [("+49 30 901820", "tel:+49-30-901820".into())]);
        assert_eq!(found("tel:+15550100123"), [("tel:+15550100123", "tel:+15550100123".into())]);
        // North American, as they're written there.
        assert_eq!(
            found("Call (650) 555-0100 or 650.555.0101, or 1-650-555-0102"),
            [("(650) 555-0100", "tel:650-555-0100".into()), ("650.555.0101", "tel:650-555-0101".into()), ("1-650-555-0102", "tel:1-650-555-0102".into()),]
        );
        let l = &find("tel:+15550100123")[0];
        assert_eq!(l.copied("tel:+15550100123"), "+15550100123");
    }

    #[test]
    fn what_numbers_arent() {
        for text in [
            "Meeting ID: 845 1234 5678",
            "Passcode: 123456",
            "on 2026-11-19 at 14:30",
            "version 1.2.3, build 650-555",
            "+5 points, +100%, C++11",
            "ID 6505550100",
            "650-555.0100",
            "x650-555-0100",
            "$650-555-0100",
            "+1 650-555-0100abc",
            "pid 1659725 took 0.31 µs",
        ] {
            assert!(found(text).is_empty(), "{text}: {:?}", found(text));
        }
    }

    #[test]
    fn an_invitations_notes() {
        let notes = "Join Zoom Meeting\nhttps://us02web.zoom.us/j/84512345678?pwd=abc\n\nMeeting ID: 845 1234 5678\nPasscode: 123456\n\nOne tap mobile\n+16465550100,,84512345678#,,,,*123456# US (New York)\n\nDial by your location\n• +1 646 555 0100 US (New York)\n\nFind your local number: https://us02web.zoom.us/u/kd8xyz\nQuestions? help@example.com or www.example.com/help";
        assert_eq!(
            found(notes),
            [
                ("https://us02web.zoom.us/j/84512345678?pwd=abc", "https://us02web.zoom.us/j/84512345678?pwd=abc".into()),
                ("+16465550100", "tel:+16465550100".into()),
                ("+1 646 555 0100", "tel:+1-646-555-0100".into()),
                ("https://us02web.zoom.us/u/kd8xyz", "https://us02web.zoom.us/u/kd8xyz".into()),
                ("help@example.com", "mailto:help@example.com".into()),
                ("www.example.com/help", "https://www.example.com/help".into()),
            ]
        );
    }

    /// Finding them in a page of notes (an invitation's, ten times over:
    /// 5 KB): what a text view does once for a text.
    #[test]
    #[ignore = "a measurement: cargo test --release -p olo-kit finding_cost -- --ignored --nocapture"]
    fn finding_cost() {
        let page = "Join Zoom Meeting\nhttps://us02web.zoom.us/j/84512345678?pwd=abc\n\nMeeting ID: 845 1234 5678\nPasscode: 123456\n\nOne tap mobile\n+16465550100,,84512345678#,,,,*123456# US (New York)\n\nDial by your location\n• +1 646 555 0100 US (New York)\n\nFind your local number: https://us02web.zoom.us/u/kd8xyz\nQuestions? help@example.com or www.example.com/help\n".repeat(10);
        let n = 2000;
        let t = std::time::Instant::now();
        let mut links = 0;
        for _ in 0..n {
            links += std::hint::black_box(find(std::hint::black_box(&page))).len();
        }
        let each = t.elapsed() / n;
        println!("{} bytes, {} links: {each:?} each", page.len(), links / n as usize);
    }

    /// Rows of `cols` columns, as a terminal shows them.
    fn screen(lines: &[&str], cols: usize) -> Vec<Vec<char>> {
        lines.iter().map(|l| l.chars().chain(std::iter::repeat(' ')).take(cols).collect()).collect()
    }

    #[test]
    fn a_word_a_program_broke_over_rows_is_put_together() {
        // Claude Code's way: a long URL at its text's edge, the rest
        // indented on the next rows.
        let rows = screen(&["⏺ See https://github.com/anthropics/claude-", "  code/issues/1234?q=a and more.", "  next"], 44);
        let (text, at, cells) = broken(&rows, 0, 10, 44).unwrap();
        assert_eq!(text, "https://github.com/anthropics/claude-code/issues/1234?q=a");
        assert_eq!(at, 4);
        assert_eq!(cells[0], (0, 6));
        assert_eq!(*cells.last().unwrap(), (1, 21));
        // From the second piece too.
        let (text, at, _) = broken(&rows, 1, 4, 44).unwrap();
        assert_eq!(text, "https://github.com/anthropics/claude-code/issues/1234?q=a");
        assert_eq!(at, 39);
        // (And it is a link, whole.)
        let found = find(&text);
        assert_eq!((found.len(), found[0].range.clone(), found[0].target.as_str()), (1, 0..text.len(), text.as_str()));
        // A row that ends short of the edge isn't broken.
        let rows = screen(&["see https://x.org/a", "  next row"], 44);
        assert!(broken(&rows, 0, 6, 44).is_none());
        // Nor across a frame.
        let rows = screen(&["│ https://example.com/aaaaaaaaaaaaaaaaaa │", "│ bbb                                   │"], 42);
        assert!(broken(&rows, 0, 4, 42).is_none());
    }
}
