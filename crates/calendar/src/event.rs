//! Events: a `VEVENT` read into what a calendar shows and changes (when,
//! what, where, who, how it repeats), written back into the component it
//! came from without touching what else is there, and worked out into
//! its occurrences between two moments.

use jiff::{
    Span, Zoned,
    civil::{Date, DateTime},
    tz::TimeZone,
};

use crate::{
    ical::{Component, Line},
    rrule::{Rule, Until, civil, civil_text},
};

/// A start or an end, as iCalendar has it.
#[derive(Debug, Clone, PartialEq)]
pub enum When {
    /// A day (an all-day event's): the same day wherever it's looked at.
    Date(Date),
    /// A time on the wall's clock, wherever the wall is ("floating").
    Floating(DateTime),
    /// A time in a zone.
    Zoned(Zoned),
}

/// The zone iCalendar's `TZID` names: tz database names as they are; the
/// same behind a prefix some programs add ("/citadel.org/…/Europe/Paris");
/// and the names Windows and Exchange use, for the zones most met.
pub fn zone(tzid: &str) -> Option<TimeZone> {
    let name = tzid.trim().trim_matches('"');
    if let Ok(tz) = TimeZone::get(name) {
        return Some(tz);
    }
    // The last three, or two, parts of a path.
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    for take in [3, 2] {
        if parts.len() > take
            && let Ok(tz) = TimeZone::get(&parts[parts.len() - take..].join("/"))
        {
            return Some(tz);
        }
    }
    const WINDOWS: &[(&str, &str)] = &[
        ("UTC", "UTC"),
        ("GMT Standard Time", "Europe/London"),
        ("Greenwich Standard Time", "Atlantic/Reykjavik"),
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("Central European Standard Time", "Europe/Warsaw"),
        ("Romance Standard Time", "Europe/Paris"),
        ("E. Europe Standard Time", "Europe/Chisinau"),
        ("FLE Standard Time", "Europe/Helsinki"),
        ("GTB Standard Time", "Europe/Bucharest"),
        ("Russian Standard Time", "Europe/Moscow"),
        ("Turkey Standard Time", "Europe/Istanbul"),
        ("Israel Standard Time", "Asia/Jerusalem"),
        ("South Africa Standard Time", "Africa/Johannesburg"),
        ("Arabian Standard Time", "Asia/Dubai"),
        ("India Standard Time", "Asia/Kolkata"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Singapore Standard Time", "Asia/Singapore"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("Korea Standard Time", "Asia/Seoul"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("New Zealand Standard Time", "Pacific/Auckland"),
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("US Mountain Standard Time", "America/Phoenix"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("Alaskan Standard Time", "America/Anchorage"),
        ("Hawaiian Standard Time", "Pacific/Honolulu"),
        ("Atlantic Standard Time", "America/Halifax"),
        ("E. South America Standard Time", "America/Sao_Paulo"),
        ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ];
    WINDOWS.iter().find(|(w, _)| w.eq_ignore_ascii_case(name)).and_then(|(_, iana)| TimeZone::get(iana).ok())
}

impl When {
    /// A `DTSTART`-like line read: a day (`VALUE=DATE`, or eight digits),
    /// a time in UTC (`…Z`), in a zone (`TZID=`; one not known reads as
    /// floating), or floating.
    pub fn read(line: &Line) -> Option<When> {
        let value = line.value.trim();
        let at = civil(value)?;
        if line.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || !value.contains('T') {
            return Some(When::Date(at.date()));
        }
        if value.ends_with(['Z', 'z']) {
            return at.to_zoned(TimeZone::UTC).ok().map(When::Zoned);
        }
        match line.param("TZID").and_then(zone) {
            Some(tz) => at.to_zoned(tz).ok().map(When::Zoned),
            None => Some(When::Floating(at)),
        }
    }

    /// As a line named `name`.
    pub fn line(&self, name: &str) -> Line {
        match self {
            When::Date(d) => Line::new(name, civil_text(d.at(0, 0, 0, 0), true)).with("VALUE", "DATE"),
            When::Floating(t) => Line::new(name, civil_text(*t, false)),
            When::Zoned(z) if z.time_zone() == &TimeZone::UTC => Line::new(name, format!("{}Z", civil_text(z.datetime(), false))),
            When::Zoned(z) => match z.time_zone().iana_name() {
                Some(tz) => Line::new(name, civil_text(z.datetime(), false)).with("TZID", tz),
                // (A zone with no name: as UTC.)
                None => Line::new(name, format!("{}Z", civil_text(z.with_time_zone(TimeZone::UTC).datetime(), false))),
            },
        }
    }

    pub fn is_date(&self) -> bool {
        matches!(self, When::Date(_))
    }

    /// On its own clock: the day at midnight, the wall's time, or the
    /// time in its zone.
    pub fn local(&self) -> DateTime {
        match self {
            When::Date(d) => d.at(0, 0, 0, 0),
            When::Floating(t) => *t,
            When::Zoned(z) => z.datetime(),
        }
    }

    /// The moment it is, on the clock of someone in `viewer`'s zone: a
    /// day and a floating time are theirs.
    pub fn moment(&self, viewer: &TimeZone) -> Zoned {
        match self {
            When::Zoned(z) => z.with_time_zone(viewer.clone()),
            other => other.local().to_zoned(viewer.clone()).unwrap_or_else(|_| Zoned::new(jiff::Timestamp::UNIX_EPOCH, viewer.clone())),
        }
    }

    /// The same kind of time (a day, floating, in the same zone) at
    /// `local` on its clock.
    pub fn at(&self, local: DateTime) -> When {
        match self {
            When::Date(_) => When::Date(local.date()),
            When::Floating(_) => When::Floating(local),
            When::Zoned(z) => local.to_zoned(z.time_zone().clone()).map(When::Zoned).unwrap_or_else(|_| When::Floating(local)),
        }
    }
}

/// A duration as iCalendar writes one ("PT1H30M", "P1D", "-PT15M",
/// "P2W"), in seconds.
pub fn duration(text: &str) -> Option<i64> {
    let text = text.trim();
    let (sign, rest) = match text.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let rest = rest.strip_prefix(['P', 'p'])?;
    let (mut total, mut number, mut time, mut any) = (0i64, String::new(), false, false);
    for c in rest.chars() {
        match c.to_ascii_uppercase() {
            'T' => time = true,
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, time) {
                    ('W', false) => 7 * 86400,
                    ('D', false) => 86400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
                any = true;
            }
        }
    }
    (any && number.is_empty()).then_some(sign * total)
}

/// Seconds as a duration ("PT15M", "P1D", "-PT1H30M").
pub fn duration_text(seconds: i64) -> String {
    let (sign, mut s) = (if seconds < 0 { "-" } else { "" }, seconds.unsigned_abs());
    let mut out = format!("{sign}P");
    if s >= 86400 && s % 86400 == 0 {
        return format!("{out}{}D", s / 86400);
    }
    if s >= 86400 {
        out += &format!("{}D", s / 86400);
        s %= 86400;
    }
    out.push('T');
    for (unit, size) in [('H', 3600), ('M', 60), ('S', 1)] {
        if s >= size {
            out += &format!("{}{unit}", s / size);
            s %= size;
        }
    }
    if out.ends_with('T') {
        out.push_str("0S");
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Confirmed,
    Tentative,
    Cancelled,
}

/// Whether someone invited is coming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Answer {
    #[default]
    NotYet,
    Yes,
    No,
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Person {
    /// Without "mailto:".
    pub address: String,
    pub name: String,
    pub answer: Answer,
    /// Needn't come (`ROLE=OPT-PARTICIPANT`).
    pub optional: bool,
}

impl Person {
    fn read(line: &Line) -> Person {
        let value = line.value.trim();
        let address = if value.len() >= 7 && value[..7].eq_ignore_ascii_case("mailto:") { &value[7..] } else { value };
        Person {
            address: address.to_owned(),
            name: line.param("CN").unwrap_or_default().to_owned(),
            answer: match line.param("PARTSTAT").unwrap_or_default().to_ascii_uppercase().as_str() {
                "ACCEPTED" => Answer::Yes,
                "DECLINED" => Answer::No,
                "TENTATIVE" => Answer::Maybe,
                _ => Answer::NotYet,
            },
            optional: line.param("ROLE").is_some_and(|r| r.eq_ignore_ascii_case("OPT-PARTICIPANT")),
        }
    }
}

impl Answer {
    /// As iCalendar says it (`PARTSTAT`).
    pub fn partstat(self) -> &'static str {
        match self {
            Answer::NotYet => "NEEDS-ACTION",
            Answer::Yes => "ACCEPTED",
            Answer::No => "DECLINED",
            Answer::Maybe => "TENTATIVE",
        }
    }
}

/// Whether `line` (an `ATTENDEE`, an `ORGANIZER`) is of one of the
/// addresses `me`.
fn is_of(line: &Line, me: &[String]) -> bool {
    let value = line.value.trim();
    let address = if value.len() >= 7 && value[..7].eq_ignore_ascii_case("mailto:") { &value[7..] } else { value };
    me.iter().any(|m| m.eq_ignore_ascii_case(address))
}

/// An invitation answered: in every event of `cal` (a whole `VCALENDAR`:
/// an event and the occurrences changed by themselves), the `ATTENDEE`
/// that is one of the addresses `me` says `answer` (`PARTSTAT`), and no
/// longer asks for one (`RSVP`). Nothing else changes: on a server that
/// schedules (RFC 6638: Google's, Fastmail's, iCloud's), writing this is
/// what sends the organizer the reply. Err: none of `me` is invited.
pub fn answer(cal: &mut Component, me: &[String], answer: Answer) -> Result<(), &'static str> {
    let mut any = false;
    for event in cal.children.iter_mut().filter(|c| c.name == "VEVENT") {
        for line in event.lines.iter_mut().filter(|l| l.name == "ATTENDEE" && is_of(l, me)) {
            line.params.retain(|(n, _)| n != "PARTSTAT" && n != "RSVP");
            line.params.push(("PARTSTAT".to_owned(), vec![answer.partstat().to_owned()]));
            any = true;
        }
    }
    if any { Ok(()) } else { Err("You aren’t among this event’s guests.") }
}

/// An event as a calendar shows and changes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub url: String,
    pub start: When,
    /// Where it ends (not part of it): the day after an all-day event's
    /// last, the moment a meeting is over.
    pub end: When,
    /// How it repeats, where the rule is one that's worked out.
    pub rule: Option<Rule>,
    /// It has a rule (worked out or not: one that isn't shows once).
    pub repeats: bool,
    /// Starts taken out of the rule's, and added to them.
    pub exdates: Vec<When>,
    pub rdates: Vec<When>,
    /// Which of a repeating event's occurrences this stands in for (by
    /// that one's start as the rule has it), if it's one changed by itself.
    pub recurrence_id: Option<When>,
    pub status: Status,
    /// Takes no time (shown, but one's still free: `TRANSP:TRANSPARENT`).
    pub free: bool,
    pub organizer: Option<Person>,
    pub attendees: Vec<Person>,
    /// Reminders: seconds before the start (negative: after).
    pub alarms: Vec<i64>,
    pub sequence: u32,
}

impl Event {
    /// A new event, starting and ending then.
    pub fn new(uid: &str, summary: &str, start: When, end: When) -> Event {
        Event {
            uid: uid.to_owned(),
            summary: summary.to_owned(),
            description: String::new(),
            location: String::new(),
            url: String::new(),
            start,
            end,
            rule: None,
            repeats: false,
            exdates: Vec::new(),
            rdates: Vec::new(),
            recurrence_id: None,
            status: Status::Confirmed,
            free: false,
            organizer: None,
            attendees: Vec::new(),
            alarms: Vec::new(),
            sequence: 0,
        }
    }

    pub fn all_day(&self) -> bool {
        self.start.is_date()
    }

    /// A `VEVENT` read. None: it has no start that can be read.
    pub fn read(c: &Component) -> Option<Event> {
        let start = When::read(c.get("DTSTART")?)?;
        let end = match (c.get("DTEND").and_then(When::read), c.text("DURATION").as_deref().and_then(duration)) {
            (Some(end), _) => end,
            (None, Some(secs)) => match &start {
                When::Zoned(z) => When::Zoned(z.checked_add(Span::new().try_seconds(secs).ok()?).ok()?),
                other => other.at(other.local().checked_add(Span::new().try_seconds(secs).ok()?).ok()?),
            },
            // (No end said: a day's is the next day; a time's is itself.)
            (None, None) => match &start {
                When::Date(d) => When::Date(d.tomorrow().ok()?),
                other => other.clone(),
            },
        };
        let many = |name: &str| -> Vec<When> {
            c.all(name)
                .flat_map(|l| {
                    // (Several in one line, comma between.)
                    l.value.split(',').filter_map(|v| When::read(&Line { name: l.name.clone(), params: l.params.clone(), value: v.trim().to_owned() })).collect::<Vec<_>>()
                })
                .collect()
        };
        let rule_text = c.text("RRULE");
        // An alarm that says how long before the start (most do).
        let alarms = c
            .children_named("VALARM")
            .filter_map(|a| {
                let t = a.get("TRIGGER")?;
                let related_end = t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
                (!related_end && !t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))).then(|| duration(&t.value)).flatten().map(|s| -s)
            })
            .collect();
        Some(Event {
            uid: c.text("UID").unwrap_or_default(),
            summary: c.text("SUMMARY").unwrap_or_default(),
            description: c.text("DESCRIPTION").unwrap_or_default(),
            location: c.text("LOCATION").unwrap_or_default(),
            url: c.text("URL").unwrap_or_default(),
            rule: rule_text.as_deref().and_then(Rule::parse),
            repeats: rule_text.is_some(),
            exdates: many("EXDATE"),
            rdates: many("RDATE"),
            recurrence_id: c.get("RECURRENCE-ID").and_then(When::read),
            status: match c.text("STATUS").unwrap_or_default().to_ascii_uppercase().as_str() {
                "TENTATIVE" => Status::Tentative,
                "CANCELLED" => Status::Cancelled,
                _ => Status::Confirmed,
            },
            free: c.text("TRANSP").is_some_and(|t| t.eq_ignore_ascii_case("TRANSPARENT")),
            organizer: c.get("ORGANIZER").map(Person::read),
            attendees: c.all("ATTENDEE").map(Person::read).collect(),
            alarms,
            sequence: c.text("SEQUENCE").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            start,
            end,
        })
    }

    /// What a person changes of an event (its words, when, how it
    /// repeats, whether it takes time, its reminders) written into the
    /// `VEVENT` it was read from, or a new one: everything else there
    /// (who's invited and what they said, the server's own lines) stays
    /// as it is. Its sequence goes up, as a change is told to others.
    pub fn write_into(&self, c: &mut Component) {
        let fresh = c.lines.is_empty();
        c.name = "VEVENT".into();
        let mut text = |name: &str, value: &str| {
            if value.is_empty() {
                c.remove(name);
            } else {
                c.set(Line::text(name, value));
            }
        };
        text("UID", &self.uid);
        text("SUMMARY", &self.summary);
        text("DESCRIPTION", &self.description);
        text("LOCATION", &self.location);
        text("URL", &self.url);
        c.set(self.start.line("DTSTART"));
        c.remove("DURATION");
        c.set(self.end.line("DTEND"));
        // (A rule that isn't worked out wasn't changed: left as written.)
        match (&self.rule, self.repeats) {
            (Some(rule), _) => c.set(Line::new("RRULE", rule.write())),
            (None, false) => c.remove("RRULE"),
            (None, true) => {}
        }
        c.remove("EXDATE");
        for x in &self.exdates {
            c.lines.push(x.line("EXDATE"));
        }
        c.remove("RDATE");
        for x in &self.rdates {
            c.lines.push(x.line("RDATE"));
        }
        match &self.recurrence_id {
            Some(id) => c.set(id.line("RECURRENCE-ID")),
            None => c.remove("RECURRENCE-ID"),
        }
        match self.status {
            Status::Confirmed => c.remove("STATUS"),
            Status::Tentative => c.set(Line::new("STATUS", "TENTATIVE")),
            Status::Cancelled => c.set(Line::new("STATUS", "CANCELLED")),
        }
        if self.free {
            c.set(Line::new("TRANSP", "TRANSPARENT"));
        } else {
            c.remove("TRANSP");
        }
        if !fresh {
            c.set(Line::new("SEQUENCE", (self.sequence + 1).to_string()));
        }
        // Reminders: those that say how long before the start are ours to
        // set; any other kind (at a date, before the end) stays.
        let ours = |a: &Component| {
            a.name == "VALARM"
                && a.get("TRIGGER").is_some_and(|t| {
                    !t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END")) && !t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) && duration(&t.value).is_some()
                })
        };
        c.children.retain(|a| !ours(a));
        for before in &self.alarms {
            let mut alarm = Component::new("VALARM");
            alarm.lines.push(Line::new("ACTION", "DISPLAY"));
            alarm.lines.push(Line::text("DESCRIPTION", if self.summary.is_empty() { "Reminder" } else { &self.summary }));
            alarm.lines.push(Line::new("TRIGGER", duration_text(-before)));
            c.children.push(alarm);
        }
    }
}

/// A UID for a new event: random, and Olo's.
pub fn new_uid() -> String {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).expect("the system's random source failed");
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}@olo", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// `vevent` marked as written now (`DTSTAMP`, `LAST-MODIFIED`; `CREATED`
/// if it has none): what servers and other programs go by to tell the
/// newer of two.
pub fn stamp(vevent: &mut Component, now: jiff::Timestamp) {
    let text = format!("{}Z", civil_text(now.to_zoned(TimeZone::UTC).datetime(), false));
    vevent.set(Line::new("DTSTAMP", text.clone()));
    vevent.set(Line::new("LAST-MODIFIED", text.clone()));
    if vevent.get("CREATED").is_none() {
        vevent.set(Line::new("CREATED", text));
    }
}

/// One time an event happens.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub start: Zoned,
    pub end: Zoned,
    pub all_day: bool,
    /// Which of a repeating event's it is: its start as the rule has it
    /// (what changing "only this one" names). None: an event that
    /// happens once.
    pub id: Option<When>,
    /// It was changed by itself: which of the overrides given it is.
    pub changed: Option<usize>,
}

/// The same moment, whichever zone each is said in (a day against a day).
pub fn same(a: &When, b: &When, viewer: &TimeZone) -> bool {
    match (a, b) {
        (When::Date(x), When::Date(y)) => x == y,
        (When::Date(x), other) | (other, When::Date(x)) => other.local().date() == *x,
        _ => a.moment(viewer).timestamp() == b.moment(viewer).timestamp(),
    }
}

/// The times `event` happens that touch the span from `from` to `to`
/// (not including `to`), in order, for someone in `viewer`'s zone (whose
/// days and floating times are theirs): once, or as its rule and added
/// dates say, less the dates taken out; each occurrence that one of
/// `overrides` (the events of the same UID changed by themselves) stands
/// in for is that one's time instead, and a cancelled one is left out.
pub fn occurrences(event: &Event, overrides: &[Event], from: &Zoned, to: &Zoned, viewer: &TimeZone) -> Vec<Occurrence> {
    let mut out = Vec::new();
    let all_day = event.all_day();
    // How long it lasts: on the clock for days and floating times, in
    // seconds for a time in a zone (an hour's meeting is an hour, even
    // the night the clocks change).
    let local_span = event.end.local().since(event.start.local()).unwrap_or_default();
    let seconds = match (&event.start, &event.end) {
        (When::Zoned(a), _) => event.end.moment(viewer).timestamp().as_second() - a.timestamp().as_second(),
        _ => 0,
    };
    let end_of = |start: &When| -> When {
        match start {
            When::Zoned(z) => When::Zoned(z.checked_add(Span::new().seconds(seconds)).unwrap_or_else(|_| z.clone())),
            other => other.at(other.local().checked_add(local_span).unwrap_or(other.local())),
        }
    };
    let put = |start: When, repeating: bool, out: &mut Vec<Occurrence>| -> bool {
        // Changed by itself: that one's time (or nothing, if it was cancelled).
        if repeating && let Some(i) = overrides.iter().position(|o| o.recurrence_id.as_ref().is_some_and(|id| same(id, &start, viewer))) {
            let o = &overrides[i];
            let (s, e) = (o.start.moment(viewer), o.end.moment(viewer));
            if o.status != Status::Cancelled && s < *to && (e > *from || (s == e && s >= *from)) {
                out.push(Occurrence { start: s, end: e, all_day: o.all_day(), id: Some(start.clone()), changed: Some(i) });
            }
            return start.moment(viewer) < *to;
        }
        let (s, e) = (start.moment(viewer), end_of(&start).moment(viewer));
        if s >= *to {
            return false;
        }
        if e > *from || (s == e && s >= *from) {
            out.push(Occurrence { start: s, end: e, all_day, id: repeating.then(|| start.clone()), changed: None });
        }
        true
    };
    let excluded = |start: &When| event.exdates.iter().any(|x| same(x, start, viewer));
    // The span on the event's own clock, with two days to spare either
    // side (two clocks are never further apart): what can't touch it is
    // passed over without a zone asked anything, which is what costs
    // (ten thousand events, most of them long past, were 145 ms a week).
    let on_its_clock = |z: &Zoned| match &event.start {
        When::Zoned(own) => z.with_time_zone(own.time_zone().clone()).datetime(),
        _ => z.datetime(),
    };
    let spare = Span::new().days(2);
    let (near, far) = (on_its_clock(from).checked_sub(spare).unwrap_or(DateTime::MIN), on_its_clock(to).checked_add(spare).unwrap_or(DateTime::MAX));
    let lasts = event.end.local().since(event.start.local()).unwrap_or_default();
    let before = |local: DateTime| local.checked_add(lasts).is_ok_and(|end| end < near);
    match &event.rule {
        None => {
            if !before(event.start.local()) && event.start.local() <= far {
                put(event.start.clone(), false, &mut out);
            }
        }
        Some(rule) => {
            let until = rule.until;
            let zone_of = event.start.clone();
            // An UNTIL said as an instant, on the event's clock, less the
            // spare: before it, no start is past it.
            let until_near = match (until, &event.start) {
                (Some(Until::Instant(secs)), When::Zoned(own)) => jiff::Timestamp::from_second(secs).ok().and_then(|t| t.to_zoned(own.time_zone().clone()).datetime().checked_sub(spare).ok()),
                _ => None,
            };
            let ended = |local: DateTime| match until {
                Some(Until::Date(d)) => local.date() > d,
                Some(Until::Local(u)) => local > u,
                // (Asked of the zone only near its end.)
                Some(Until::Instant(secs)) => until_near.is_none_or(|u| local >= u) && zone_of.at(local).moment(viewer).timestamp().as_second() > secs,
                None => false,
            };
            for local in rule.starts_from(event.start.local(), near, ended) {
                if before(local) {
                    continue;
                }
                if local > far {
                    break;
                }
                let start = event.start.at(local);
                if excluded(&start) {
                    // (Past the span: no need to go on.)
                    if start.moment(viewer) >= *to {
                        break;
                    }
                    continue;
                }
                if !put(start, true, &mut out) {
                    break;
                }
            }
        }
    }
    // Dates added to the rule's (or to a single event's).
    for extra in &event.rdates {
        if !excluded(extra) && !same(extra, &event.start, viewer) {
            put(extra.clone(), true, &mut out);
        }
    }
    // One changed by itself to a time the rule's own no longer falls in
    // the span for (moved into it from outside) is still found by where
    // it is now.
    for (i, o) in overrides.iter().enumerate() {
        if o.status == Status::Cancelled || out.iter().any(|x| x.changed == Some(i)) {
            continue;
        }
        let (s, e) = (o.start.moment(viewer), o.end.moment(viewer));
        if s < *to && (e > *from || (s == e && s >= *from)) {
            out.push(Occurrence { start: s, end: e, all_day: o.all_day(), id: o.recurrence_id.clone(), changed: Some(i) });
        }
    }
    out.sort_by(|a, b| a.start.timestamp().cmp(&b.start.timestamp()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ical;

    fn tz(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    fn moment(text: &str, zone: &str) -> Zoned {
        civil(text).unwrap().to_zoned(tz(zone)).unwrap()
    }

    /// The events of a file's first calendar: the one with no
    /// RECURRENCE-ID, and those with one.
    fn events(text: &str) -> (Event, Vec<Event>) {
        let cal = ical::parse(text).remove(0);
        let mut all: Vec<Event> = cal.children_named("VEVENT").filter_map(Event::read).collect();
        let at = all.iter().position(|e| e.recurrence_id.is_none()).unwrap();
        let master = all.remove(at);
        (master, all)
    }

    /// Each occurrence's start and end on `zone`'s clock.
    fn shown(event: &Event, overrides: &[Event], from: &str, to: &str, zone: &str) -> Vec<String> {
        let z = tz(zone);
        occurrences(event, overrides, &moment(from, zone), &moment(to, zone), &z)
            .iter()
            .map(|o| format!("{}-{}{}", civil_text(o.start.datetime(), o.all_day), civil_text(o.end.datetime(), o.all_day), if o.changed.is_some() { " changed" } else { "" }))
            .collect()
    }

    const WEEKLY: &str = "BEGIN:VCALENDAR
BEGIN:VEVENT
UID:standup@example.org
DTSTART;TZID=Europe/Amsterdam:20261012T090000
DTEND;TZID=Europe/Amsterdam:20261012T093000
RRULE:FREQ=WEEKLY;BYDAY=MO
EXDATE;TZID=Europe/Amsterdam:20261019T090000
SUMMARY:Standup
END:VEVENT
BEGIN:VEVENT
UID:standup@example.org
RECURRENCE-ID;TZID=Europe/Amsterdam:20261102T090000
DTSTART;TZID=Europe/Amsterdam:20261103T140000
DTEND;TZID=Europe/Amsterdam:20261103T150000
SUMMARY:Standup (moved)
END:VEVENT
BEGIN:VEVENT
UID:standup@example.org
RECURRENCE-ID;TZID=Europe/Amsterdam:20261109T090000
DTSTART;TZID=Europe/Amsterdam:20261109T090000
DTEND;TZID=Europe/Amsterdam:20261109T093000
STATUS:CANCELLED
SUMMARY:Standup
END:VEVENT
END:VCALENDAR
";

    /// Every Monday at nine in Amsterdam is at nine there through the
    /// change of the clocks (and an hour earlier by New York's clock for
    /// the week the two changes are apart); a date taken out is gone, one
    /// changed by itself is where it was moved to, one cancelled is gone.
    #[test]
    fn a_weekly_event_through_the_clocks_change_with_its_exceptions() {
        let (event, overrides) = events(WEEKLY);
        assert_eq!((event.summary.as_str(), event.repeats, overrides.len()), ("Standup", true, 2));
        assert_eq!(
            shown(&event, &overrides, "20261001T000000", "20261201T000000", "Europe/Amsterdam"),
            [
                "20261012T090000-20261012T093000",
                // (The 19th taken out; the clocks go back on the 25th.)
                "20261026T090000-20261026T093000",
                "20261103T140000-20261103T150000 changed",
                // (The 9th cancelled.)
                "20261116T090000-20261116T093000",
                "20261123T090000-20261123T093000",
                "20261130T090000-20261130T093000",
            ]
        );
        // New York changes a week after Amsterdam: nine there is 03:00, 04:00 for that week, then 03:00.
        assert_eq!(
            shown(&event, &[], "20261020T000000", "20261110T000000", "America/New_York"),
            ["20261026T040000-20261026T043000", "20261102T030000-20261102T033000", "20261109T030000-20261109T033000"]
        );
        // Only what touches the span: one under way at its start counts, one that starts at its end doesn't.
        assert_eq!(shown(&event, &[], "20261012T091500", "20261026T090000", "Europe/Amsterdam"), ["20261012T090000-20261012T093000"]);
        // "Only this one" is named by its start as the rule has it, moved or not.
        let z = tz("Europe/Amsterdam");
        let all = occurrences(&event, &overrides, &moment("20261101T000000", "Europe/Amsterdam"), &moment("20261108T000000", "Europe/Amsterdam"), &z);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id.as_ref().map(|w| civil_text(w.local(), false)).as_deref(), Some("20261102T090000"));
        assert_eq!(all[0].changed.map(|i| overrides[i].summary.as_str()), Some("Standup (moved)"));
    }

    /// A day is the same day wherever it's looked at, its end the day
    /// after its last; a rule's UNTIL said as an instant ends it there;
    /// an event with no end, or with how long it lasts, has one.
    #[test]
    fn days_untils_and_ends() {
        let text = "BEGIN:VCALENDAR
BEGIN:VEVENT
UID:holiday
DTSTART;VALUE=DATE:20261224
DTEND;VALUE=DATE:20261227
SUMMARY:Away
END:VEVENT
END:VCALENDAR
";
        let (away, _) = events(text);
        assert!(away.all_day());
        for zone in ["Europe/Amsterdam", "America/Los_Angeles", "Pacific/Auckland"] {
            assert_eq!(shown(&away, &[], "20261201T000000", "20270101T000000", zone), ["20261224-20261227"], "{zone}");
        }
        // Yearly, a day: birthdays.
        let mut birthday = Event::new("b", "Birthday", When::Date(civil("20200229").unwrap().date()), When::Date(civil("20200301").unwrap().date()));
        birthday.rule = Rule::parse("FREQ=YEARLY");
        assert_eq!(shown(&birthday, &[], "20200101T000000", "20330101T000000", "UTC"), ["20200229-20200301", "20240229-20240301", "20280229-20280301", "20320229-20320301"]);
        // UNTIL as an instant: 22:59:59 UTC on the 26th is the end of that day in Amsterdam.
        let mut daily = Event::new("d", "Daily", When::Zoned(moment("20261024T230000", "Europe/Amsterdam")), When::Zoned(moment("20261024T233000", "Europe/Amsterdam")));
        daily.rule = Rule::parse("FREQ=DAILY;UNTIL=20261026T225959Z");
        assert_eq!(
            shown(&daily, &[], "20261001T000000", "20261201T000000", "Europe/Amsterdam"),
            ["20261024T230000-20261024T233000", "20261025T230000-20261025T233000", "20261026T230000-20261026T233000"]
        );
        // An hour's meeting the night the clocks go back is an hour: 02:30 comes twice, it is the first.
        let night =
            Event::new("n", "Night", When::Zoned(moment("20261025T013000", "Europe/Amsterdam")), When::Zoned(moment("20261025T013000", "Europe/Amsterdam").checked_add(Span::new().hours(1)).unwrap()));
        assert_eq!(shown(&night, &[], "20261025T000000", "20261026T000000", "Europe/Amsterdam"), ["20261025T013000-20261025T023000"]);
        // How long it lasts, said; and nothing said.
        let c = &ical::parse(
            "BEGIN:VEVENT
UID:x
DTSTART:20261120T140000Z
DURATION:PT1H30M
END:VEVENT
",
        )[0];
        let e = Event::read(c).unwrap();
        assert_eq!(civil_text(e.end.local(), false), "20261120T153000");
        let c = &ical::parse(
            "BEGIN:VEVENT
UID:x
DTSTART;VALUE=DATE:20261120
END:VEVENT
",
        )[0];
        assert_eq!(Event::read(c).unwrap().end, When::Date(civil("20261121").unwrap().date()));
        let c = &ical::parse(
            "BEGIN:VEVENT
UID:x
DTSTART:20261120T140000
END:VEVENT
",
        )[0];
        let e = Event::read(c).unwrap();
        assert_eq!((e.start.clone(), e.end.clone()), (When::Floating(civil("20261120T140000").unwrap()), When::Floating(civil("20261120T140000").unwrap())));
        // A floating time is at that time on the viewer's own clock; an event of no length shows.
        assert_eq!(shown(&e, &[], "20261120T000000", "20261121T000000", "Asia/Tokyo"), ["20261120T140000-20261120T140000"]);
        assert!(
            Event::read(
                &ical::parse(
                    "BEGIN:VEVENT
UID:x
END:VEVENT
"
                )[0]
            )
            .is_none()
        );
    }

    #[test]
    fn zones_by_the_names_programs_give_them() {
        for name in [
            "Europe/Amsterdam",
            "/freeassociation.sourceforge.net/Europe/Amsterdam",
            "/citadel.org/20210210_1/America/Argentina/Buenos_Aires",
            "W. Europe Standard Time",
            "Pacific Standard Time",
            "UTC",
        ] {
            assert!(zone(name).is_some(), "{name}");
        }
        assert_eq!(zone("W. Europe Standard Time").unwrap().iana_name(), Some("Europe/Berlin"));
        assert!(zone("Nowhere/At_All").is_none() && zone("").is_none());
        // A zone not known reads as a time on the wall's clock, not as nothing.
        let l = Line::new("DTSTART", "20261120T140000").with("TZID", "Nowhere/At_All");
        assert_eq!(When::read(&l), Some(When::Floating(civil("20261120T140000").unwrap())));
        // Each kind of time comes back out as it went in.
        for text in ["DTSTART;VALUE=DATE:20261120", "DTSTART:20261120T140000", "DTSTART:20261120T140000Z", "DTSTART;TZID=America/New_York:20261120T140000"] {
            let line = Line::parse(text).unwrap();
            assert_eq!(When::read(&line).unwrap().line("DTSTART").write(), text);
        }
    }

    #[test]
    fn durations() {
        for (text, secs) in [("PT15M", 900), ("-PT15M", -900), ("PT1H30M", 5400), ("P1D", 86400), ("P2W", 1_209_600), ("P1DT2H", 93_600), ("PT0S", 0), ("+PT45S", 45)] {
            assert_eq!(duration(text), Some(secs), "{text}");
        }
        for bad in ["", "P", "PT", "15M", "PT15", "P1H", "PTxM"] {
            assert_eq!(duration(bad), None, "{bad}");
        }
        for secs in [900, -900, 5400, 86400, 93_600, 0, 45, 172_800] {
            assert_eq!(duration(&duration_text(secs)), Some(secs), "{}", duration_text(secs));
        }
        assert_eq!((duration_text(900).as_str(), duration_text(-5400).as_str(), duration_text(86400).as_str()), ("PT15M", "-PT1H30M", "P1D"));
    }

    /// An invitation answered: my line says so, in the event and in an
    /// occurrence changed by itself, whatever the address's case; it no
    /// longer asks for an answer; nobody else's line, and nothing else,
    /// changes. Not invited: nothing written.
    #[test]
    fn an_invitation_answered() {
        let text = "BEGIN:VCALENDAR
BEGIN:VEVENT
UID:review@example.org
DTSTART:20261120T140000Z
DTEND:20261120T150000Z
RRULE:FREQ=WEEKLY
SUMMARY:Review
ORGANIZER;CN=Kim:mailto:kim@example.org
ATTENDEE;CN=Ben;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;ROLE=REQ-PARTICIPANT:mailto:Ben@Example.org
ATTENDEE;CN=Dana;PARTSTAT=ACCEPTED:mailto:dana@example.org
X-KEPT:as it is
END:VEVENT
BEGIN:VEVENT
UID:review@example.org
RECURRENCE-ID:20261127T140000Z
DTSTART:20261127T150000Z
DTEND:20261127T160000Z
SUMMARY:Review, later
ATTENDEE;CN=Ben;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:ben@example.org
END:VEVENT
END:VCALENDAR
";
        let me = ["other@example.org".to_owned(), "ben@example.org".to_owned()];
        let mut cal = ical::parse(text).remove(0);
        assert_eq!(answer(&mut cal, &me, Answer::Maybe), Ok(()));
        // (As written, its long lines' folds undone.)
        let written = ical::unfold(&cal.write()).join("\n");
        assert!(written.contains("ATTENDEE;CN=Ben;ROLE=REQ-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:Ben@Example.org"), "{written}");
        assert!(written.contains("ATTENDEE;CN=Ben;PARTSTAT=TENTATIVE:mailto:ben@example.org"), "{written}");
        assert!(written.contains("ATTENDEE;CN=Dana;PARTSTAT=ACCEPTED:mailto:dana@example.org") && written.contains("X-KEPT:as it is") && !written.contains("RSVP"), "{written}");
        // (Read back: that's my answer; and changed again.)
        let mine = |cal: &ical::Component| Event::read(&cal.children[0]).unwrap().attendees.iter().find(|p| p.address.eq_ignore_ascii_case("ben@example.org")).map(|p| p.answer);
        assert_eq!(mine(&cal), Some(Answer::Maybe));
        answer(&mut cal, &me, Answer::No).unwrap();
        assert_eq!(mine(&cal), Some(Answer::No));
        assert_eq!(cal.write().matches("PARTSTAT=DECLINED").count(), 2);
        // Not one of the guests: nothing changes.
        let before = cal.clone();
        assert!(answer(&mut cal, &["sam@example.org".to_owned()], Answer::Yes).is_err());
        assert_eq!(cal, before);
    }

    /// An event changed is written back into the component it came from:
    /// its words, its time, its reminders; who's invited, what they said
    /// and every line Olo doesn't know stay as they were, in their places.
    #[test]
    fn an_event_changed_keeps_what_else_was_there() {
        let text = "BEGIN:VCALENDAR
PRODID:-//Example//EN
BEGIN:VEVENT
UID:review@example.org
DTSTAMP:20261101T120000Z
DTSTART;TZID=Europe/Amsterdam:20261120T140000
DTEND;TZID=Europe/Amsterdam:20261120T150000
SUMMARY:Review
LOCATION:Room 4
ORGANIZER;CN=Ben:mailto:ben@example.org
ATTENDEE;CN=\"Jansen, Dana\";PARTSTAT=ACCEPTED;ROLE=OPT-PARTICIPANT:mailto:dana@example.org
ATTENDEE;PARTSTAT=NEEDS-ACTION:MAILTO:sam@example.org
X-GOOGLE-CONFERENCE:https://meet.example/abc
SEQUENCE:2
BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:-PT15M
END:VALARM
BEGIN:VALARM
ACTION:EMAIL
TRIGGER;VALUE=DATE-TIME:20261120T080000Z
END:VALARM
END:VEVENT
END:VCALENDAR
";
        let mut cal = ical::parse(text).remove(0);
        let mut event = Event::read(&cal.children[0]).unwrap();
        assert_eq!((event.alarms.clone(), event.sequence, event.attendees.len()), (vec![900], 2, 2));
        assert_eq!(event.organizer.as_ref().map(|o| (o.name.as_str(), o.address.as_str())), Some(("Ben", "ben@example.org")));
        assert_eq!((event.attendees[0].name.as_str(), event.attendees[0].answer, event.attendees[0].optional), ("Jansen, Dana", Answer::Yes, true));
        assert_eq!((event.attendees[1].address.as_str(), event.attendees[1].answer), ("sam@example.org", Answer::NotYet));
        event.summary = "Review; part two".into();
        event.location.clear();
        event.start = When::Zoned(moment("20261121T100000", "Europe/Amsterdam"));
        event.end = When::Zoned(moment("20261121T113000", "Europe/Amsterdam"));
        event.alarms = vec![600, 3600];
        event.write_into(&mut cal.children[0]);
        let (before, after) = (ical::unfold(text), ical::unfold(&cal.write()));
        let gone: Vec<&String> = before.iter().filter(|l| !after.contains(l)).collect();
        let new: Vec<&String> = after.iter().filter(|l| !before.contains(l)).collect();
        assert_eq!(gone, ["DTSTART;TZID=Europe/Amsterdam:20261120T140000", "DTEND;TZID=Europe/Amsterdam:20261120T150000", "SUMMARY:Review", "LOCATION:Room 4", "SEQUENCE:2", "TRIGGER:-PT15M"]);
        assert_eq!(
            new,
            [
                "DTSTART;TZID=Europe/Amsterdam:20261121T100000",
                "DTEND;TZID=Europe/Amsterdam:20261121T113000",
                "SUMMARY:Review\\; part two",
                "SEQUENCE:3",
                "DESCRIPTION:Review\\; part two",
                "TRIGGER:-PT10M",
                "DESCRIPTION:Review\\; part two",
                "TRIGGER:-PT1H"
            ]
        );
        // Read again: what was written.
        let again = Event::read(&ical::parse(&cal.write())[0].children[0]).unwrap();
        assert_eq!((again.summary.as_str(), again.location.as_str(), again.alarms.clone(), again.sequence, again.attendees.len()), ("Review; part two", "", vec![600, 3600], 3, 2));
        assert_eq!(again.start, event.start);
        // The reminder at a date, which isn't ours to set, is still there.
        assert!(cal.write().contains("TRIGGER;VALUE=DATE-TIME:20261120T080000Z"));
        // A new event: a VEVENT of its own, no sequence yet.
        let mut fresh = Component::default();
        Event::new("new@olo", "Lunch", When::Date(civil("20261201").unwrap().date()), When::Date(civil("20261202").unwrap().date())).write_into(&mut fresh);
        assert_eq!(ical::unfold(&fresh.write()), ["BEGIN:VEVENT", "UID:new@olo", "SUMMARY:Lunch", "DTSTART;VALUE=DATE:20261201", "DTEND;VALUE=DATE:20261202", "END:VEVENT"]);
    }

    /// Dates added to a rule's show too; and one moved out of the span
    /// it would have been in isn't shown in it, while one moved into the
    /// span from outside is.
    #[test]
    fn added_dates_and_ones_moved_across_the_span() {
        let (mut event, overrides) = events(WEEKLY);
        event.rdates = vec![When::Zoned(moment("20261014T160000", "Europe/Amsterdam"))];
        assert_eq!(shown(&event, &[], "20261012T000000", "20261018T000000", "Europe/Amsterdam"), ["20261012T090000-20261012T093000", "20261014T160000-20261014T163000"]);
        // The 2nd of November's was moved to the 3rd: asked for the 2nd alone, nothing; for the 3rd alone, it.
        assert!(shown(&event, &overrides, "20261102T000000", "20261103T000000", "Europe/Amsterdam").is_empty());
        assert_eq!(shown(&event, &overrides, "20261103T000000", "20261104T000000", "Europe/Amsterdam"), ["20261103T140000-20261103T150000 changed"]);
    }
}
