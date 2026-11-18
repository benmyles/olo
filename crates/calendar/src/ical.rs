//! iCalendar (RFC 5545) as text: content lines unfolded and folded,
//! their parameters and values escaped, and the tree of components they
//! make (`BEGIN:VCALENDAR` … `END:VCALENDAR`).
//!
//! Read whole and written back whole: a property Olo doesn't know (a
//! server's `X-` line, an alarm's oddities, a time zone's rules) comes
//! back out exactly as it went in, in its place, so changing an event's
//! title doesn't cost it anything another program put there.

use std::fmt::Write as _;

/// One content line: `NAME;PARAM=value;…:value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Upper case ("DTSTART").
    pub name: String,
    /// Each parameter: its name (upper case) and its values, unquoted.
    pub params: Vec<(String, Vec<String>)>,
    /// As written (escapes and all: [`unescape`] for a text value).
    pub value: String,
}

impl Line {
    pub fn new(name: &str, value: impl Into<String>) -> Line {
        Line { name: name.to_ascii_uppercase(), params: Vec::new(), value: value.into() }
    }

    /// A text property: its value escaped.
    pub fn text(name: &str, value: &str) -> Line {
        Line::new(name, escape(value))
    }

    pub fn with(mut self, param: &str, value: impl Into<String>) -> Line {
        self.params.push((param.to_ascii_uppercase(), vec![value.into()]));
        self
    }

    /// A parameter's first value.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|(_, v)| v.first()).map(String::as_str)
    }

    /// The value as text: its escapes undone.
    pub fn as_text(&self) -> String {
        unescape(&self.value)
    }

    /// One line read (already unfolded). None: not a content line.
    pub fn parse(line: &str) -> Option<Line> {
        // The name ends at the first ';' or ':'; a parameter's value may be
        // quoted, and then holds ':' and ';' and ',' as they are.
        let end = line.find([';', ':'])?;
        let name = line[..end].trim();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return None;
        }
        let mut params = Vec::new();
        let bytes = line.as_bytes();
        let mut at = end;
        while bytes.get(at) == Some(&b';') {
            at += 1;
            let eq = at + line[at..].find(['=', ':', ';'])?;
            if bytes[eq] != b'=' {
                // (A parameter with no value: kept, empty.)
                params.push((line[at..eq].to_ascii_uppercase(), Vec::new()));
                at = eq;
                continue;
            }
            let pname = line[at..eq].to_ascii_uppercase();
            at = eq + 1;
            let mut values = Vec::new();
            loop {
                if bytes.get(at) == Some(&b'"') {
                    let close = at + 1 + line[at + 1..].find('"')?;
                    values.push(line[at + 1..close].to_owned());
                    at = close + 1;
                } else {
                    let stop = at + line[at..].find([',', ';', ':']).unwrap_or(line.len() - at);
                    values.push(line[at..stop].to_owned());
                    at = stop;
                }
                if bytes.get(at) == Some(&b',') {
                    at += 1;
                } else {
                    break;
                }
            }
            params.push((pname, values));
        }
        if bytes.get(at) != Some(&b':') {
            return None;
        }
        Some(Line { name: name.to_ascii_uppercase(), params, value: line[at + 1..].to_owned() })
    }

    /// As it's written, not folded.
    pub fn write(&self) -> String {
        let mut out = self.name.clone();
        for (name, values) in &self.params {
            let _ = write!(out, ";{name}");
            for (i, v) in values.iter().enumerate() {
                out.push(if i == 0 { '=' } else { ',' });
                // (Quoted where it must be; a quote itself can't be said.)
                if v.contains([':', ';', ',']) {
                    let _ = write!(out, "\"{}\"", v.replace('"', "'"));
                } else {
                    out.push_str(&v.replace('"', "'"));
                }
            }
        }
        out.push(':');
        out.push_str(&self.value);
        out
    }
}

/// A text value's escapes undone: `\n`, `\,`, `\;`, `\\`.
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Text made a value: what must be escaped, escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// The lines of `text` unfolded (a line that starts with a space or a
/// tab goes on from the one before), empty ones left out.
pub fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match raw.as_bytes().first() {
            Some(b' ' | b'\t') if !lines.is_empty() => lines.last_mut().unwrap().push_str(&raw[1..]),
            _ if raw.is_empty() => {}
            _ => lines.push(raw.to_owned()),
        }
    }
    lines
}

/// `line` folded at 75 octets, never inside a character, each piece
/// ended with CRLF.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut room = 75;
    let mut used = 0;
    for c in line.chars() {
        if used + c.len_utf8() > room {
            out.push_str("\r\n ");
            // (The space that says it goes on is one of the next 75.)
            room = 74;
            used = 0;
        }
        out.push(c);
        used += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

/// A component: `BEGIN:NAME`, its lines, the components in it, `END:NAME`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Upper case ("VEVENT").
    pub name: String,
    pub lines: Vec<Line>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Component {
        Component { name: name.to_ascii_uppercase(), ..Default::default() }
    }

    /// The first line named `name`.
    pub fn get(&self, name: &str) -> Option<&Line> {
        self.lines.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Line> + 'a {
        self.lines.iter().filter(move |l| l.name.eq_ignore_ascii_case(name))
    }

    /// A text property's value.
    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(Line::as_text)
    }

    /// `line` in place of the first of its name (where that was), or
    /// added at the end; any others of the name removed.
    pub fn set(&mut self, line: Line) {
        match self.lines.iter().position(|l| l.name == line.name) {
            Some(i) => {
                let name = line.name.clone();
                self.lines[i] = line;
                let mut seen = false;
                self.lines.retain(|l| l.name != name || !std::mem::replace(&mut seen, true));
            }
            None => self.lines.push(line),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.lines.retain(|l| !l.name.eq_ignore_ascii_case(name));
    }

    /// The components in it named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.children.iter().filter(move |c| c.name.eq_ignore_ascii_case(name))
    }

    /// As iCalendar text, folded, CRLF at each line's end.
    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        out.push_str(&fold(&format!("BEGIN:{}", self.name)));
        for l in &self.lines {
            out.push_str(&fold(&l.write()));
        }
        for c in &self.children {
            c.write_into(out);
        }
        out.push_str(&fold(&format!("END:{}", self.name)));
    }
}

/// The components of `text` (a file's `VCALENDAR`s; usually one). Lines
/// that aren't content lines, and an `END` with no `BEGIN`, are passed
/// over; a component never ended is taken as ended with the text.
pub fn parse(text: &str) -> Vec<Component> {
    let mut done = Vec::new();
    let mut open: Vec<Component> = Vec::new();
    for raw in unfold(text) {
        let Some(line) = Line::parse(&raw) else { continue };
        if line.name == "BEGIN" {
            open.push(Component::new(line.value.trim()));
        } else if line.name == "END" {
            // (Ended by name: one not ended inside it is ended with it.)
            let Some(at) = open.iter().rposition(|c| c.name.eq_ignore_ascii_case(line.value.trim())) else { continue };
            while open.len() > at {
                let c = open.pop().unwrap();
                match open.last_mut() {
                    Some(parent) => parent.children.push(c),
                    None => done.push(c),
                }
            }
        } else if let Some(c) = open.last_mut() {
            c.lines.push(line);
        }
    }
    while let Some(c) = open.pop() {
        match open.last_mut() {
            Some(parent) => parent.children.push(c),
            None => done.push(c),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Amsterdam\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc-123@example.org\r\nDTSTART;TZID=Europe/Amsterdam:20261120T140000\r\nDTEND;TZID=Europe/Amsterdam:20261120T150000\r\nSUMMARY:Lunch\\, then a walk\\; maybe\r\nDESCRIPTION:A long line that goes on and on and on and on and on and on an\r\n d is folded here\\nwith a second line of its own.\r\nATTENDEE;CN=\"Jansen, Dana\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:dana@example.org\r\nX-SERVER-THING;X-A=1,2:kept as it is\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn a_file_read_and_written_back_is_the_same_file() {
        let all = parse(FILE);
        assert_eq!(all.len(), 1);
        let cal = &all[0];
        assert_eq!((cal.name.as_str(), cal.lines.len(), cal.children.len()), ("VCALENDAR", 2, 2));
        let event = cal.children_named("VEVENT").next().unwrap();
        assert_eq!(event.text("SUMMARY").as_deref(), Some("Lunch, then a walk; maybe"));
        assert_eq!(event.text("DESCRIPTION").as_deref(), Some("A long line that goes on and on and on and on and on and on and is folded here\nwith a second line of its own."));
        assert_eq!(event.get("DTSTART").unwrap().param("tzid"), Some("Europe/Amsterdam"));
        let attendee = event.get("ATTENDEE").unwrap();
        assert_eq!((attendee.param("CN"), attendee.param("PARTSTAT"), attendee.value.as_str()), (Some("Jansen, Dana"), Some("ACCEPTED"), "mailto:dana@example.org"));
        assert_eq!(event.get("X-SERVER-THING").unwrap().params, vec![("X-A".to_owned(), vec!["1".to_owned(), "2".to_owned()])]);
        assert_eq!(event.children[0].text("TRIGGER").as_deref(), Some("-PT15M"));
        // Written back, and read again: the same tree; and the same text
        // but for where the long line folds.
        let written = cal.write();
        assert_eq!(parse(&written), all);
        assert_eq!(unfold(&written), unfold(FILE));
        assert!(written.lines().all(|l| l.len() <= 75), "a line over 75 octets");
    }

    #[test]
    fn one_property_changed_changes_nothing_else() {
        let mut cal = parse(FILE).remove(0);
        let event = cal.children.iter_mut().find(|c| c.name == "VEVENT").unwrap();
        event.set(Line::text("SUMMARY", "Lunch; later"));
        event.set(Line::text("LOCATION", "Café, upstairs"));
        let written = cal.write();
        let before: Vec<String> = unfold(FILE);
        let after: Vec<String> = unfold(&written);
        let changed: Vec<&String> = after.iter().filter(|l| !before.contains(l)).collect();
        assert_eq!(changed, vec!["SUMMARY:Lunch\\; later", "LOCATION:Café\\, upstairs"]);
        assert_eq!(after.len(), before.len() + 1, "the summary in its place, the location added");
        assert_eq!(after.iter().position(|l| l.starts_with("SUMMARY")), before.iter().position(|l| l.starts_with("SUMMARY")));
    }

    #[test]
    fn folding_never_splits_a_character_and_escapes_come_back() {
        let long = "é".repeat(100);
        let folded = fold(&format!("SUMMARY:{long}"));
        assert!(folded.split("\r\n").all(|l| l.len() <= 75 && std::str::from_utf8(l.as_bytes()).is_ok()));
        assert_eq!(unfold(&folded), vec![format!("SUMMARY:{long}")]);
        for text in ["plain", "a, b; c", "back\\slash", "two\nlines", "", "ends with \\"] {
            assert_eq!(unescape(&escape(text)), text, "{text:?}");
        }
        assert_eq!(unescape("Mixed\\N case \\x kept"), "Mixed\n case x kept");
    }

    #[test]
    fn what_isnt_ical_is_passed_over() {
        assert!(parse("").is_empty() && parse("hello\nworld\n").is_empty());
        // Never ended: taken as ended with the text. An END with no BEGIN: nothing.
        let c = parse("END:VEVENT\nBEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\n:nameless\nnot a line at all\n");
        assert_eq!((c.len(), c[0].children.len(), c[0].children[0].lines.len()), (1, 1, 1));
        // An inner one not ended is ended with its parent.
        let c = parse("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\nEND:VCALENDAR\n");
        assert_eq!((c.len(), c[0].children[0].name.as_str()), (1, "VEVENT"));
        assert_eq!(Line::parse("NAME;FLAG;P=\"a:b\",c:v").unwrap().params, vec![("FLAG".to_owned(), vec![]), ("P".to_owned(), vec!["a:b".to_owned(), "c".to_owned()])]);
        assert!(Line::parse("no colon here").is_none() && Line::parse("BAD NAME:x").is_none());
    }
}
