//! A to-do as To-Do shows and changes it, read from a `VTODO` and
//! written back into it. What Olo doesn't know of it (another program's
//! `X-` lines, a sub-task's link, a geographic reminder) stays as it was:
//! the file is read whole and written back whole (`olo_calendar::ical`).

use jiff::{Timestamp, tz::TimeZone};
use olo_calendar::{
    event::{When, duration, duration_text, stamp},
    ical::{Component, Line},
    rrule::{civil, civil_text},
};

/// Where a to-do is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// To be done (`NEEDS-ACTION`, or no status).
    #[default]
    Open,
    /// Under way (`IN-PROCESS`): still to be done.
    Doing,
    /// Done (`COMPLETED`), when.
    Done,
    /// Not to be done after all (`CANCELLED`): shown with the done ones.
    Cancelled,
}

/// When a to-do reminds of itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Remind {
    /// This many seconds before it's due (negative: after). For a to-do
    /// due on a day, from that day's start: 9 AM on the day is -9 hours.
    /// (`TRIGGER;RELATED=END`, as a to-do's due is its end.)
    Before(i64),
    /// At a moment of its own.
    At(When),
}

/// A link, a picture or a file attached (`ATTACH`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// `https://…`, or `file:///…` for a file kept with the to-do.
    pub uri: String,
    /// What it's called (`FILENAME`, RFC 8607; a link's own words).
    pub name: String,
    /// Its media type (`FMTTYPE`), where known.
    pub mime: Option<String>,
}

/// What an attachment is, for how it's shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Link,
    Picture,
    File,
}

impl Attachment {
    pub fn kind(&self) -> Kind {
        if !self.uri.starts_with("file:") {
            return Kind::Link;
        }
        let picture = self.mime.as_deref().is_some_and(|m| m.starts_with("image/"))
            || [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic", ".avif", ".bmp", ".tiff", ".svg"].iter().any(|x| self.name.to_ascii_lowercase().ends_with(x));
        if picture { Kind::Picture } else { Kind::File }
    }

    /// A file attached: where it is (None: a link).
    pub fn path(&self) -> Option<std::path::PathBuf> {
        let rest = self.uri.strip_prefix("file://")?;
        Some(std::path::PathBuf::from(percent_decode(rest)))
    }

    fn read(line: &Line) -> Option<Attachment> {
        // (Only what's a URI: a file written into the line itself, in
        // base 64, is another program's to show.)
        if line.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("BINARY")) || line.param("ENCODING").is_some() {
            return None;
        }
        let uri = line.value.trim().to_owned();
        if uri.is_empty() {
            return None;
        }
        let name = line.param("FILENAME").or_else(|| line.param("X-APPLE-FILENAME")).or_else(|| line.param("X-OLO-NAME")).map(str::to_owned).unwrap_or_else(|| name_of(&uri));
        Some(Attachment { uri, name, mime: line.param("FMTTYPE").map(str::to_owned) })
    }

    fn line(&self) -> Line {
        let mut line = Line::new("ATTACH", self.uri.clone());
        if let Some(m) = &self.mime {
            line = line.with("FMTTYPE", m.clone());
        }
        if self.name != name_of(&self.uri) {
            line = line.with("FILENAME", self.name.clone());
        }
        line
    }
}

/// What a URI is called with no name given: a file's name, a link's
/// host and path.
fn name_of(uri: &str) -> String {
    if let Some(path) = uri.strip_prefix("file://") {
        return percent_decode(path.rsplit('/').next().unwrap_or(path));
    }
    let bare = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    bare.trim_end_matches('/').to_owned()
}

/// `%20` and the like undone (a file URI's path).
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path as a file URI: what isn't plain in a path is `%`-escaped.
pub fn file_uri(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A to-do: what's shown of it, and what changing it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct ToDo {
    pub uid: String,
    pub title: String,
    /// Its notes (`DESCRIPTION`): any length, links in them live.
    pub notes: String,
    pub status: Status,
    /// When it was done (`COMPLETED`).
    pub done_at: Option<Timestamp>,
    /// When it's due (`DUE`): a day, or a time on one.
    pub due: Option<When>,
    pub remind: Option<Remind>,
    /// The section of its list it's in (`X-OLO-SECTION`): a column of
    /// the list's board, a heading of its list. None: no section.
    pub section: Option<String>,
    /// Where it is among its list's (`X-OLO-ORDER`): lower first.
    pub order: i64,
    pub attachments: Vec<Attachment>,
    /// When it was made and last changed (`CREATED`, `LAST-MODIFIED`).
    pub created: Option<Timestamp>,
    pub modified: Option<Timestamp>,
}

impl ToDo {
    pub fn new(uid: &str, title: &str) -> ToDo {
        ToDo {
            uid: uid.to_owned(),
            title: title.to_owned(),
            notes: String::new(),
            status: Status::Open,
            done_at: None,
            due: None,
            remind: None,
            section: None,
            order: 0,
            attachments: Vec::new(),
            created: None,
            modified: None,
        }
    }

    /// Still to be done (open, or under way).
    pub fn open(&self) -> bool {
        matches!(self.status, Status::Open | Status::Doing)
    }

    /// The `VTODO` of a file's text (the first one).
    pub fn read(c: &Component) -> Option<ToDo> {
        if c.name != "VTODO" {
            return None;
        }
        let uid = c.text("UID")?;
        let status = match c.get("STATUS").map(|l| l.value.trim().to_ascii_uppercase()).as_deref() {
            Some("COMPLETED") => Status::Done,
            Some("IN-PROCESS") => Status::Doing,
            Some("CANCELLED") => Status::Cancelled,
            // (Done with no status said: some programs write only when.)
            _ if c.get("COMPLETED").is_some() => Status::Done,
            _ => Status::Open,
        };
        let stamp_of = |name: &str| c.get(name).and_then(|l| When::read(l)).map(|w| w.moment(&TimeZone::UTC).timestamp());
        let due = c.get("DUE").and_then(When::read);
        let remind = c.children_named("VALARM").find_map(|a| {
            let t = a.get("TRIGGER")?;
            if t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) {
                return When::read(t).map(Remind::At);
            }
            // (Relative to its start where it has one and says so; to
            // its due otherwise: a to-do's alarms are of its due.)
            let seconds = duration(&t.value)?;
            Some(Remind::Before(-seconds))
        });
        Some(ToDo {
            uid,
            title: c.text("SUMMARY").unwrap_or_default(),
            notes: c.text("DESCRIPTION").unwrap_or_default(),
            status,
            done_at: stamp_of("COMPLETED"),
            due,
            remind,
            section: c.text("X-OLO-SECTION").filter(|s| !s.is_empty()),
            order: c.get("X-OLO-ORDER").and_then(|l| l.value.trim().parse().ok()).or_else(|| c.get("X-APPLE-SORT-ORDER").and_then(|l| l.value.trim().parse().ok())).unwrap_or(0),
            attachments: c.all("ATTACH").filter_map(Attachment::read).collect(),
            created: stamp_of("CREATED"),
            modified: stamp_of("LAST-MODIFIED").or_else(|| stamp_of("DTSTAMP")),
        })
    }

    /// Written into `c` (its `VTODO`), as changed from `was` (what was
    /// read from it): only what changed is touched, so what another
    /// program put there stays (its alarms, unless the reminder changed).
    pub fn write_into(&self, was: Option<&ToDo>, c: &mut Component, now: Timestamp) {
        if was.is_none() {
            c.set(Line::text("UID", &self.uid));
        }
        if was.is_none_or(|w| w.title != self.title) {
            c.set(Line::text("SUMMARY", &self.title));
        }
        if was.is_none_or(|w| w.notes != self.notes) {
            if self.notes.is_empty() {
                c.remove("DESCRIPTION");
            } else {
                c.set(Line::text("DESCRIPTION", &self.notes));
            }
        }
        if was.is_none_or(|w| w.status != self.status || w.done_at != self.done_at) {
            let utc = |t: Timestamp| format!("{}Z", civil_text(t.to_zoned(TimeZone::UTC).datetime(), false));
            match self.status {
                Status::Done => {
                    c.set(Line::new("STATUS", "COMPLETED"));
                    c.set(Line::new("COMPLETED", utc(self.done_at.unwrap_or(now))));
                    c.set(Line::new("PERCENT-COMPLETE", "100"));
                }
                Status::Cancelled => {
                    c.set(Line::new("STATUS", "CANCELLED"));
                    c.remove("COMPLETED");
                    c.remove("PERCENT-COMPLETE");
                }
                Status::Open | Status::Doing => {
                    c.set(Line::new("STATUS", if self.status == Status::Doing { "IN-PROCESS" } else { "NEEDS-ACTION" }));
                    c.remove("COMPLETED");
                    c.remove("PERCENT-COMPLETE");
                }
            }
        }
        if was.is_none_or(|w| w.due != self.due) {
            match &self.due {
                Some(d) => c.set(d.line("DUE")),
                None => c.remove("DUE"),
            }
            // (A start after its due is wrong: a start some program set
            // goes where the due went, if it was after.)
            if let (Some(due), Some(start)) = (&self.due, c.get("DTSTART").and_then(When::read))
                && start.moment(&TimeZone::UTC) > due.moment(&TimeZone::UTC)
            {
                c.remove("DTSTART");
            }
        }
        if was.is_none_or(|w| w.remind != self.remind) {
            c.children.retain(|a| a.name != "VALARM");
            if let Some(r) = &self.remind {
                let mut alarm = Component::new("VALARM");
                alarm.lines.push(Line::new("ACTION", "DISPLAY"));
                alarm.lines.push(Line::text("DESCRIPTION", &self.title));
                alarm.lines.push(match r {
                    Remind::Before(seconds) => Line::new("TRIGGER", duration_text(-seconds)).with("RELATED", "END"),
                    Remind::At(at) => {
                        // (An alarm's time is a moment: in UTC.)
                        let utc = at.moment(&TimeZone::system()).with_time_zone(TimeZone::UTC);
                        Line::new("TRIGGER", format!("{}Z", civil_text(utc.datetime(), false))).with("VALUE", "DATE-TIME")
                    }
                });
                c.children.push(alarm);
            }
        }
        if was.is_none_or(|w| w.section != self.section) {
            match &self.section {
                Some(s) => c.set(Line::text("X-OLO-SECTION", s)),
                None => c.remove("X-OLO-SECTION"),
            }
        }
        if was.is_none_or(|w| w.order != self.order) {
            c.set(Line::new("X-OLO-ORDER", self.order.to_string()));
        }
        if was.is_none_or(|w| w.attachments != self.attachments) {
            // (Those it still has stay where they were, as they were;
            // what's gone goes; what's new goes at the end.)
            let keep: Vec<Line> = c.all("ATTACH").filter(|l| Attachment::read(l).is_none_or(|a| self.attachments.contains(&a))).cloned().collect();
            let had: Vec<Attachment> = keep.iter().filter_map(Attachment::read).collect();
            c.lines.retain(|l| l.name != "ATTACH");
            c.lines.extend(keep);
            c.lines.extend(self.attachments.iter().filter(|a| !had.contains(a)).map(Attachment::line));
        }
        stamp(c, now);
    }
}

/// A new file for `todo`: a calendar of the one to-do.
pub fn new_file(todo: &ToDo, now: Timestamp) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.lines.push(Line::new("VERSION", "2.0"));
    cal.lines.push(Line::new("PRODID", "-//Olo//To-Do//EN"));
    let mut vtodo = Component::new("VTODO");
    todo.write_into(None, &mut vtodo, now);
    cal.children.push(vtodo);
    cal.write()
}

/// The to-do in a file's text, and where it is in the file's tree (the
/// calendar, then its `VTODO`): None if the file holds none.
pub fn read_file(text: &str) -> Option<(ToDo, Vec<Component>)> {
    let cals = olo_calendar::ical::parse(text);
    let todo = cals.iter().flat_map(|c| c.children_named("VTODO")).find_map(ToDo::read)?;
    Some((todo, cals))
}

/// `todo`, changed from `was`, written into the text of its file (`text`,
/// as read): the new text.
pub fn rewrite(text: &str, was: &ToDo, todo: &ToDo, now: Timestamp) -> Option<String> {
    let mut cals = olo_calendar::ical::parse(text);
    let vtodo = cals.iter_mut().flat_map(|c| c.children.iter_mut()).find(|c| c.name == "VTODO" && c.text("UID").as_deref() == Some(was.uid.as_str()))?;
    todo.write_into(Some(was), vtodo, now);
    Some(cals.iter().map(Component::write).collect())
}

/// A due's moment, on `zone`'s clock: a day's is its start.
pub fn due_moment(due: &When, zone: &TimeZone) -> jiff::Zoned {
    due.moment(zone)
}

/// When a to-do's reminder is (None: it has none, or it's relative to a
/// due it doesn't have).
pub fn remind_at(todo: &ToDo, zone: &TimeZone) -> Option<Timestamp> {
    match todo.remind.as_ref()? {
        Remind::At(at) => Some(at.moment(zone).timestamp()),
        Remind::Before(seconds) => {
            let due = todo.due.as_ref()?.moment(zone).timestamp();
            due.checked_sub(jiff::SignedDuration::from_secs(*seconds)).ok()
        }
    }
}

/// A moment read from iCalendar text (`20261120T080000Z`), for tests and
/// stand-ins.
pub fn moment(text: &str) -> Option<Timestamp> {
    let at = civil(text.trim_end_matches(['Z', 'z']))?;
    at.to_zoned(TimeZone::UTC).ok().map(|z| z.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//iOS 26//EN\r\nBEGIN:VTODO\r\nUID:6C3F-11\r\nDTSTAMP:20261118T090000Z\r\nCREATED:20261117T090000Z\r\nSUMMARY:Pay the rent\r\nDESCRIPTION:Before the 1st\\, by transfer\r\nDUE;VALUE=DATE:20261201\r\nX-APPLE-SORT-ORDER:300\r\nX-APPLE-FLAGGED:1\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;RELATED=END:PT9H\r\nX-APPLE-DEFAULT-ALARM:TRUE\r\nEND:VALARM\r\nATTACH:https://bank.example/pay\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    #[test]
    fn another_programs_to_do_read() {
        let (todo, _) = read_file(APPLE).unwrap();
        assert_eq!(todo.uid, "6C3F-11");
        assert_eq!(todo.title, "Pay the rent");
        assert_eq!(todo.notes, "Before the 1st, by transfer");
        assert_eq!(todo.due, Some(When::Date(jiff::civil::date(2026, 12, 1))));
        // 9 AM on the day it's due: nine hours after it starts.
        assert_eq!(todo.remind, Some(Remind::Before(-9 * 3600)));
        assert_eq!(todo.order, 300);
        assert_eq!(todo.attachments, vec![Attachment { uri: "https://bank.example/pay".into(), name: "bank.example/pay".into(), mime: None }]);
        assert!(todo.open());
    }

    #[test]
    fn changed_is_all_that_changes() {
        let (was, _) = read_file(APPLE).unwrap();
        let now = moment("20261119T120000Z").unwrap();
        let mut done = was.clone();
        done.status = Status::Done;
        let text = rewrite(APPLE, &was, &done, now).unwrap();
        let (back, _) = read_file(&text).unwrap();
        assert_eq!(back.status, Status::Done);
        assert_eq!(back.done_at, Some(now));
        // What another program put there is still there, as it was: its
        // flag, its alarm, its order.
        for kept in ["X-APPLE-FLAGGED:1", "X-APPLE-DEFAULT-ALARM:TRUE", "TRIGGER;RELATED=END:PT9H", "X-APPLE-SORT-ORDER:300", "ATTACH:https://bank.example/pay"] {
            assert!(text.contains(kept), "{kept} kept");
        }
        assert!(text.contains("LAST-MODIFIED:20261119T120000Z"));
        // Opened again: no longer done.
        let mut open = back.clone();
        open.status = Status::Open;
        open.done_at = None;
        let again = rewrite(&text, &back, &open, now).unwrap();
        assert!(!again.contains("COMPLETED:") && again.contains("STATUS:NEEDS-ACTION"));
    }

    #[test]
    fn a_new_one_round_trips() {
        let now = moment("20261119T120000Z").unwrap();
        let zone = TimeZone::get("America/Los_Angeles").unwrap();
        let mut todo = ToDo::new("a1@olo", "Call Ann, about the trip");
        todo.notes = "Her number's in Phone\nAsk about Tuesday".into();
        todo.due = Some(When::Zoned(jiff::civil::date(2026, 11, 20).at(15, 0, 0, 0).to_zoned(zone.clone()).unwrap()));
        todo.remind = Some(Remind::Before(15 * 60));
        todo.section = Some("Doing".into());
        todo.order = 2048;
        todo.attachments = vec![
            Attachment { uri: "https://example.com/trip".into(), name: "The trip".into(), mime: None },
            Attachment { uri: file_uri(std::path::Path::new("/home/u/.local/share/olo/todo/.files/a1@olo/map one.png")), name: "map one.png".into(), mime: Some("image/png".into()) },
        ];
        let text = new_file(&todo, now);
        assert!(text.contains("TRIGGER;RELATED=END:-PT15M"), "{text}");
        assert!(text.contains("DUE;TZID=America/Los_Angeles:20261120T150000"));
        assert!(text.contains("map%20one.png"));
        let (back, _) = read_file(&text).unwrap();
        assert_eq!(back.created, Some(now));
        assert_eq!(ToDo { created: None, modified: None, ..back.clone() }, todo);
        assert_eq!(back.attachments[1].kind(), Kind::Picture);
        assert_eq!(back.attachments[1].path().unwrap(), std::path::Path::new("/home/u/.local/share/olo/todo/.files/a1@olo/map one.png"));
        assert_eq!(back.attachments[0].kind(), Kind::Link);
        // Reminded 15 minutes before 3 PM there.
        assert_eq!(remind_at(&back, &zone), Some(moment("20261120T224500Z").unwrap()));
    }

    #[test]
    fn a_reminder_at_a_time_of_its_own() {
        let now = moment("20261119T120000Z").unwrap();
        let mut todo = ToDo::new("b2@olo", "Water the plants");
        let at = jiff::civil::date(2026, 11, 21).at(8, 30, 0, 0).to_zoned(TimeZone::UTC).unwrap();
        todo.remind = Some(Remind::At(When::Zoned(at.clone())));
        let text = new_file(&todo, now);
        assert!(text.contains("TRIGGER;VALUE=DATE-TIME:20261121T083000Z"), "{text}");
        let (back, _) = read_file(&text).unwrap();
        assert_eq!(remind_at(&back, &TimeZone::UTC), Some(at.timestamp()));
    }
}
