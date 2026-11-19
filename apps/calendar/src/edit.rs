//! Changing events, apart from any window: what the editor edits (a
//! [`Draft`]: an event's words, its time, how it repeats, its reminder),
//! read from an event and written back into it; what a change does to an
//! event's text on its server ([`change_all`], [`change_one`],
//! [`remove_one`]: every occurrence, or only the one); and days and times
//! as a person types them ([`date`], [`time`]).

use jiff::{
    Span,
    civil::{Date, DateTime, Time},
    tz::TimeZone,
};
use olo_calendar::{
    event::{self, Event, When},
    ical::Component,
    rrule::{Freq, Rule, Until},
};

use crate::words;

/// How an event repeats, as the editor offers it.
#[derive(Debug, Clone, PartialEq)]
pub enum Repeat {
    Never,
    Daily,
    Weekdays,
    Weekly,
    Fortnightly,
    Monthly,
    Yearly,
    /// Some other way (said in words): left as it is.
    Other(String),
}

impl Repeat {
    pub const OFFERED: [Repeat; 7] = [Repeat::Never, Repeat::Daily, Repeat::Weekdays, Repeat::Weekly, Repeat::Fortnightly, Repeat::Monthly, Repeat::Yearly];

    /// Its name in a sentence, as a fact says it (the preview's "Every
    /// week"; the menu's is "Every Week").
    pub fn said(&self) -> String {
        match self {
            Repeat::Never => "Never".into(),
            Repeat::Daily => "Every day".into(),
            Repeat::Weekdays => "Every weekday".into(),
            Repeat::Weekly => "Every week".into(),
            Repeat::Fortnightly => "Every 2 weeks".into(),
            Repeat::Monthly => "Every month".into(),
            Repeat::Yearly => "Every year".into(),
            Repeat::Other(said) => said.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Repeat::Never => "Never".into(),
            Repeat::Daily => "Every Day".into(),
            Repeat::Weekdays => "Every Weekday".into(),
            Repeat::Weekly => "Every Week".into(),
            Repeat::Fortnightly => "Every 2 Weeks".into(),
            Repeat::Monthly => "Every Month".into(),
            Repeat::Yearly => "Every Year".into(),
            Repeat::Other(said) => said.clone(),
        }
    }

    fn rule(&self) -> Option<&'static str> {
        Some(match self {
            Repeat::Daily => "FREQ=DAILY",
            Repeat::Weekdays => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
            Repeat::Weekly => "FREQ=WEEKLY",
            Repeat::Fortnightly => "FREQ=WEEKLY;INTERVAL=2",
            Repeat::Monthly => "FREQ=MONTHLY",
            Repeat::Yearly => "FREQ=YEARLY",
            Repeat::Never | Repeat::Other(_) => return None,
        })
    }

    /// How `event` repeats, as one of those offered if it is one.
    fn of(event: &Event) -> Repeat {
        let Some(rule) = &event.rule else { return if event.repeats { Repeat::Other("In its own way".into()) } else { Repeat::Never } };
        let plain = rule.count.is_none() && rule.by_month_day.is_empty() && rule.by_month.is_empty() && rule.by_set_pos.is_empty();
        let weekday = event.start.local().weekday();
        // (A weekly rule that names its start's own day says no more than one that names none.)
        let own_day = rule.by_day.is_empty() || (rule.by_day.len() == 1 && rule.by_day[0] == (None, weekday));
        let weekdays = rule.by_day.len() == 5 && rule.by_day.iter().all(|(n, d)| n.is_none() && d.to_monday_zero_offset() < 5);
        match (rule.freq, rule.interval) {
            (Freq::Daily, 1) if plain && rule.by_day.is_empty() => Repeat::Daily,
            (Freq::Weekly, 1) if plain && weekdays => Repeat::Weekdays,
            (Freq::Weekly, 1) if plain && own_day => Repeat::Weekly,
            (Freq::Weekly, 2) if plain && own_day => Repeat::Fortnightly,
            (Freq::Monthly, 1) if plain && rule.by_day.is_empty() => Repeat::Monthly,
            (Freq::Yearly, 1) if plain && rule.by_day.is_empty() => Repeat::Yearly,
            _ => Repeat::Other(said(rule)),
        }
    }
}

/// A rule the editor doesn't offer, in a few words.
fn said(rule: &Rule) -> String {
    let unit = match rule.freq {
        Freq::Daily => "day",
        Freq::Weekly => "week",
        Freq::Monthly => "month",
        Freq::Yearly => "year",
    };
    let every = if rule.interval == 1 { format!("Every {unit}") } else { format!("Every {} {unit}s", rule.interval) };
    match rule.count {
        Some(n) => format!("{every}, {n} times"),
        None if !rule.by_day.is_empty() || !rule.by_month_day.is_empty() || !rule.by_set_pos.is_empty() || !rule.by_month.is_empty() => format!("{every}, on set days"),
        None => every,
    }
}

/// The reminders the editor offers: seconds before the start, and each
/// in a sentence, as a fact says it (the preview's "15 minutes before";
/// the menu's is "15 Minutes Before").
pub const REMINDERS_SAID: [&str; 9] = ["None", "At the start", "5 minutes before", "10 minutes before", "15 minutes before", "30 minutes before", "1 hour before", "2 hours before", "1 day before"];

/// The reminders the editor offers: seconds before the start.
pub const REMINDERS: [(Option<i64>, &str); 9] = [
    (None, "None"),
    (Some(0), "At the Start"),
    (Some(300), "5 Minutes Before"),
    (Some(600), "10 Minutes Before"),
    (Some(900), "15 Minutes Before"),
    (Some(1800), "30 Minutes Before"),
    (Some(3600), "1 Hour Before"),
    (Some(7200), "2 Hours Before"),
    (Some(86400), "1 Day Before"),
];

/// What the editor edits of an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub location: String,
    pub notes: String,
    pub all_day: bool,
    /// On the event's own clock. An all-day event's `end` is its last
    /// day (at midnight): as people say it, not the day after.
    pub start: DateTime,
    pub end: DateTime,
    pub repeat: Repeat,
    /// The last day it repeats on (None: for good).
    pub until: Option<Date>,
    /// Seconds before the start (None: no reminder).
    pub reminder: Option<i64>,
    /// Who's invited, where that's the person's to say (None: its
    /// guests stay as they are).
    pub invite: Option<Invite>,
}

/// Who's invited to an event the person organizes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Invite {
    /// The guests' addresses (not the person's own).
    pub guests: Vec<String>,
    /// Who invites: the address the calendar's account is signed in
    /// with (filled in as it's saved: the window doesn't know it).
    pub organizer: String,
}

impl Invite {
    /// The addresses typed in `text`, each once: apart by commas,
    /// semicolons, new lines or spaces; of "Dana Jansen
    /// <dana@example.org>" (as mail writes one), the address. Or the
    /// first thing typed that isn't an address.
    pub fn read(text: &str) -> Result<Vec<String>, String> {
        let address = |word: &str| {
            let (name, domain) = word.split_once('@').unwrap_or(("", ""));
            !name.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !domain.contains('@')
        };
        let mut out: Vec<String> = Vec::new();
        for entry in text.split([',', ';', '\n']).map(str::trim).filter(|e| !e.is_empty()) {
            // (Mail's way: the address is what's between the angles.)
            let words: Vec<&str> = match (entry.find('<'), entry.rfind('>')) {
                (Some(a), Some(b)) if a < b => vec![entry[a + 1..b].trim()],
                _ => entry.split_whitespace().collect(),
            };
            for word in words {
                if !address(word) {
                    return Err(word.to_owned());
                }
                if !out.iter().any(|a| a.eq_ignore_ascii_case(word)) {
                    out.push(word.to_owned());
                }
            }
        }
        Ok(out)
    }

    fn organizer(&self) -> olo_calendar::event::Person {
        olo_calendar::event::Person { address: self.organizer.clone(), answer: olo_calendar::event::Answer::Yes, ..Default::default() }
    }
}

impl Draft {
    /// A new event's: an hour from `start`, or that whole day.
    pub fn new(start: DateTime, all_day: bool) -> Draft {
        let start = if all_day { start.date().at(0, 0, 0, 0) } else { start };
        let end = if all_day { start } else { start.checked_add(Span::new().hours(1)).unwrap_or(start) };
        Draft {
            title: String::new(),
            location: String::new(),
            notes: String::new(),
            all_day,
            start,
            end,
            repeat: Repeat::Never,
            until: None,
            reminder: if all_day { None } else { Some(900) },
            invite: None,
        }
    }

    /// `event`'s, at the times `start` and `end` (its own, or one of its
    /// occurrences': the end not part of it).
    pub fn of(event: &Event, start: DateTime, end: DateTime) -> Draft {
        let all_day = event.all_day();
        let end = if all_day { end.date().yesterday().unwrap_or(end.date()).max(start.date()).at(0, 0, 0, 0) } else { end };
        let until = event.rule.as_ref().and_then(|r| r.until).map(|u| match u {
            Until::Date(d) => d,
            Until::Local(t) => t.date(),
            Until::Instant(secs) => {
                let zone = match &event.start {
                    When::Zoned(z) => z.time_zone().clone(),
                    _ => TimeZone::UTC,
                };
                jiff::Timestamp::from_second(secs).map(|t| t.to_zoned(zone).date()).unwrap_or(start.date())
            }
        });
        Draft {
            title: event.summary.clone(),
            location: event.location.clone(),
            notes: event.description.clone(),
            all_day,
            start,
            end,
            repeat: Repeat::of(event),
            until,
            reminder: event.alarms.first().copied(),
            invite: None,
        }
    }

    /// What can't be saved as it is, in words.
    pub fn wrong(&self) -> Option<&'static str> {
        if self.end < self.start {
            return Some("It ends before it starts.");
        }
        if self.until.is_some_and(|u| u < self.start.date()) {
            return Some("It stops repeating before it starts.");
        }
        None
    }

    /// Written into `event`, starting at `start` and ending at `end` on
    /// its own clock (the draft's own times, or those shifted for the
    /// event an occurrence is of): what the draft holds, and nothing
    /// else of the event. A time keeps the kind the event's had (its
    /// zone, or none); a new event's, or one that was a whole day's, is
    /// in `zone` (the person's).
    pub fn apply(&self, event: &mut Event, start: DateTime, end: DateTime, zone: &TimeZone) {
        event.summary = self.title.trim().to_owned();
        event.location = self.location.trim().to_owned();
        event.description = self.notes.trim_end().to_owned();
        let kind = event.start.clone();
        let at = |t: DateTime| match &kind {
            When::Zoned(z) => t.to_zoned(z.time_zone().clone()).map(When::Zoned).unwrap_or(When::Floating(t)),
            When::Floating(_) => When::Floating(t),
            When::Date(_) => t.to_zoned(zone.clone()).map(When::Zoned).unwrap_or(When::Floating(t)),
        };
        if self.all_day {
            event.start = When::Date(start.date());
            event.end = When::Date(end.date().tomorrow().unwrap_or(end.date()));
        } else {
            event.start = at(start);
            event.end = at(end.max(start));
        }
        match &self.repeat {
            // (Left as it is, its end too.)
            Repeat::Other(_) => {}
            Repeat::Never => {
                event.rule = None;
                event.repeats = false;
            }
            offered => {
                let mut rule = offered.rule().and_then(Rule::parse).expect("the editor's own rules");
                rule.until = self.until.map(|day| match &event.start {
                    When::Date(_) => Until::Date(day),
                    When::Floating(_) => Until::Local(day.at(23, 59, 59, 0)),
                    // (To the end of that day where the event is.)
                    When::Zoned(z) => day.at(23, 59, 59, 0).to_zoned(z.time_zone().clone()).map(|t| Until::Instant(t.timestamp().as_second())).unwrap_or(Until::Date(day)),
                });
                event.rule = Some(rule);
                event.repeats = true;
            }
        }
        // The first reminder is the editor's; any others stay.
        match (self.reminder, event.alarms.first_mut()) {
            (Some(before), Some(first)) => *first = before,
            (Some(before), None) => event.alarms.push(before),
            (None, Some(_)) => {
                event.alarms.remove(0);
            }
            (None, None) => {}
        }
        // (A new event is written with these; one there has its lines
        // changed by `invited`, which keeps what each guest answered.)
        if let Some(invite) = self.invite.as_ref().filter(|i| !i.organizer.is_empty()) {
            event.organizer = (!invite.guests.is_empty()).then(|| invite.organizer());
            event.attendees = invite.guests.iter().map(|g| olo_calendar::event::Person { address: g.clone(), ..Default::default() }).collect();
        }
    }

    /// Who's invited written into the `VEVENT` `c`, where that's the
    /// draft's to say.
    fn invited(&self, c: &mut Component) {
        if let Some(invite) = self.invite.as_ref().filter(|i| !i.organizer.is_empty()) {
            olo_calendar::event::invite(c, &invite.organizer(), &invite.guests);
        }
    }
}

/// The `VEVENT` of a calendar's text that is the event itself (not one
/// of its occurrences changed by itself).
fn main_event(cal: &Component) -> Option<usize> {
    cal.children.iter().position(|c| c.name.eq_ignore_ascii_case("VEVENT") && c.get("RECURRENCE-ID").is_none()).or_else(|| cal.children.iter().position(|c| c.name.eq_ignore_ascii_case("VEVENT")))
}

/// The occurrence changed by itself that stands for `id`, if there is one.
fn override_of(cal: &Component, id: &When, zone: &TimeZone) -> Option<usize> {
    cal.children.iter().position(|c| c.name.eq_ignore_ascii_case("VEVENT") && c.get("RECURRENCE-ID").and_then(When::read).is_some_and(|w| event::same(&w, id, zone)))
}

/// The whole event changed to `draft`. `from` is the start of the
/// occurrence the draft was made of: the event itself moves as far as
/// the draft moved from there (every Monday's standup dragged from 9 to
/// 10 on one Monday is at 10 every Monday), and lasts as long as the
/// draft does. What names its occurrences by their starts moves with
/// it: the dates taken out, and each occurrence changed by itself
/// (left behind, those would be events of their own beside the ones
/// they stood for). And if the occurrence the change was made on
/// (`id`) has a text of its own, what was typed is that one's too: it's
/// what the person was looking at.
pub fn change_all(cal: &mut Component, draft: &Draft, from: DateTime, id: Option<&When>, zone: &TimeZone) -> Result<(), &'static str> {
    let at = main_event(cal).ok_or("the event isn't there")?;
    let mut event = Event::read(&cal.children[at]).ok_or("the event can't be read")?;
    let own = event.start.local();
    let moved = draft.start.since(from).unwrap_or_default();
    let later = |w: &When| w.at(w.local().checked_add(moved).unwrap_or(w.local()));
    let start = own.checked_add(moved).unwrap_or(own);
    let lasts = draft.end.since(draft.start).unwrap_or_default();
    let end = start.checked_add(lasts).unwrap_or(start);
    if !moved.is_zero() {
        event.exdates = event.exdates.iter().map(later).collect();
    }
    draft.apply(&mut event, start, end, zone);
    event.write_into(&mut cal.children[at]);
    // (Who's invited to the event is who's invited to each occurrence
    // changed by itself.)
    for child in cal.children.iter_mut().filter(|c| c.name.eq_ignore_ascii_case("VEVENT")) {
        draft.invited(child);
    }
    event::stamp(&mut cal.children[at], jiff::Timestamp::now());
    if !moved.is_zero() {
        for (i, child) in cal.children.iter_mut().enumerate() {
            if i != at
                && child.name.eq_ignore_ascii_case("VEVENT")
                && let Some(which) = child.get("RECURRENCE-ID").and_then(When::read)
            {
                child.set(later(&which).line("RECURRENCE-ID"));
            }
        }
    }
    if let Some(id) = id.map(later)
        && let Some(own) = override_of(cal, &id, zone)
        && let Some(mut event) = Event::read(&cal.children[own])
    {
        let draft = Draft { repeat: Repeat::Never, until: None, ..draft.clone() };
        draft.apply(&mut event, draft.start, draft.end, zone);
        event.recurrence_id = Some(id);
        event.write_into(&mut cal.children[own]);
        event::stamp(&mut cal.children[own], jiff::Timestamp::now());
    }
    Ok(())
}

/// Only the occurrence `id` changed to `draft`: an occurrence of its own
/// in the event's text (`RECURRENCE-ID`), made from the event itself
/// the first time, with everything of it that isn't about repeating.
pub fn change_one(cal: &mut Component, id: &When, draft: &Draft, zone: &TimeZone) -> Result<(), &'static str> {
    let at = match override_of(cal, id, zone) {
        Some(at) => at,
        None => {
            let main = main_event(cal).ok_or("the event isn't there")?;
            let mut copy = cal.children[main].clone();
            for name in ["RRULE", "EXDATE", "RDATE", "EXRULE"] {
                copy.remove(name);
            }
            copy.set(id.line("RECURRENCE-ID"));
            cal.children.push(copy);
            cal.children.len() - 1
        }
    };
    let mut event = Event::read(&cal.children[at]).ok_or("the event can't be read")?;
    // (One occurrence doesn't repeat.)
    let draft = Draft { repeat: Repeat::Never, until: None, ..draft.clone() };
    draft.apply(&mut event, draft.start, draft.end, zone);
    event.recurrence_id = Some(id.clone());
    event.write_into(&mut cal.children[at]);
    draft.invited(&mut cal.children[at]);
    event::stamp(&mut cal.children[at], jiff::Timestamp::now());
    Ok(())
}

/// Only the occurrence `id` removed: taken out of the event's dates
/// (`EXDATE`), and its own text, if it was changed by itself, with it.
pub fn remove_one(cal: &mut Component, id: &When, zone: &TimeZone) -> Result<(), &'static str> {
    if let Some(at) = override_of(cal, id, zone) {
        cal.children.remove(at);
    }
    let at = main_event(cal).ok_or("the event isn't there")?;
    let mut event = Event::read(&cal.children[at]).ok_or("the event can't be read")?;
    if !event.exdates.iter().any(|x| event::same(x, id, zone)) {
        event.exdates.push(id.clone());
    }
    event.write_into(&mut cal.children[at]);
    event::stamp(&mut cal.children[at], jiff::Timestamp::now());
    Ok(())
}

/// "Fri, Nov 20, 2026": a day as the editor's fields show it.
pub fn date_text(d: Date) -> String {
    format!("{}, {} {}, {}", &words::weekday_name(d)[..3], &words::month_name(d)[..3], d.day(), d.year())
}

fn month_named(word: &str) -> Option<i8> {
    let w = word.trim_end_matches(['.', ',']).to_ascii_lowercase();
    (w.len() >= 3).then(|| words::MONTHS.iter().position(|m| m.to_ascii_lowercase().starts_with(&w)).map(|i| i as i8 + 1)).flatten()
}

fn weekday_named(word: &str) -> Option<i8> {
    let w = word.trim_end_matches(['.', ',']).to_ascii_lowercase();
    (w.len() >= 3).then(|| words::WEEKDAYS.iter().position(|d| d.to_ascii_lowercase().starts_with(&w)).map(|i| i as i8)).flatten()
}

/// A day as typed: "2026-11-20", "Nov 20", "20 November 2026", "Fri, Nov
/// 20, 2026", "20/11" or "11/20" (the month first where `month_first`),
/// "today", "tomorrow", a weekday's name (the next one). With no year,
/// the one that puts it nearest `today`, looking forward.
pub fn date(text: &str, today: Date, month_first: bool) -> Option<Date> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let after = |days: i32| crate::layout::day_of(crate::layout::day_number(today) + days);
    match lower.as_str() {
        "today" | "now" => return Some(today),
        "tomorrow" => return Some(after(1)),
        "yesterday" => return Some(after(-1)),
        _ => {}
    }
    if let Ok(d) = text.parse::<Date>() {
        return Some(d);
    }
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    // A weekday alone: the next one (today's own name: a week on).
    if let [word] = words.as_slice()
        && let Some(day) = weekday_named(word)
    {
        let ahead = (day as i32 - today.weekday().to_monday_zero_offset() as i32).rem_euclid(7);
        return Some(after(if ahead == 0 { 7 } else { ahead }));
    }
    let this_year = |month: i8, day: i8| {
        let d = jiff::civil::Date::new(today.year(), month, day).ok()?;
        // (More than three months gone: next year's.)
        if crate::layout::day_number(d) < crate::layout::day_number(today) - 92 { jiff::civil::Date::new(today.year() + 1, month, day).ok() } else { Some(d) }
    };
    let year_of = |n: i32| if n < 100 { 2000 + n } else { n } as i16;
    // With a month's name: its day and maybe a year, in any order (a
    // weekday's name beside them says nothing more).
    if let Some(month) = words.iter().find_map(|w| month_named(w)) {
        let numbers: Vec<i32> = words.iter().filter_map(|w| w.trim_end_matches(['.', ',']).trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()).collect();
        return match numbers.as_slice() {
            [day] if (1..=31).contains(day) => this_year(month, *day as i8),
            [a, b] if (1..=31).contains(a) && *b > 31 => jiff::civil::Date::new(year_of(*b), month, *a as i8).ok(),
            [a, b] if (1..=31).contains(b) && *a > 31 => jiff::civil::Date::new(year_of(*a), month, *b as i8).ok(),
            _ => None,
        };
    }
    // Numbers alone: 20/11, 20/11/2026, 11/20/26, 20.11.2026, 20-11-2026.
    let parts: Vec<i32> = text.split(['/', '.', '-']).map(|p| p.trim().parse::<i32>()).collect::<Result<_, _>>().ok()?;
    let (day, month) = match parts.as_slice() {
        [a, b] | [a, b, _] => {
            if *a > 12 {
                (*a, *b)
            } else if *b > 12 || month_first {
                (*b, *a)
            } else {
                (*a, *b)
            }
        }
        _ => return None,
    };
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return None;
    }
    match parts.as_slice() {
        [_, _, year] => jiff::civil::Date::new(year_of(*year), month as i8, day as i8).ok(),
        _ => this_year(month as i8, day as i8),
    }
}

/// A time as typed: "14:00", "1400", "14", "2pm", "2:30 PM", "2.30pm",
/// "noon", "midnight".
pub fn time(text: &str) -> Option<Time> {
    let lower = text.trim().to_ascii_lowercase();
    match lower.as_str() {
        "noon" | "midday" => return Some(Time::constant(12, 0, 0, 0)),
        "midnight" => return Some(Time::midnight()),
        _ => {}
    }
    // "2 PM", "2pm", "2p", "2 p.m.": which half of the day, if said.
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect::<String>().replace("a.m.", "am").replace("p.m.", "pm");
    let (digits, half) = if let Some(d) = compact.strip_suffix("am").or_else(|| compact.strip_suffix('a')) {
        (d, Some(false))
    } else if let Some(d) = compact.strip_suffix("pm").or_else(|| compact.strip_suffix('p')) {
        (d, Some(true))
    } else {
        (compact.as_str(), None)
    };
    let (hour, minute): (i32, i32) = match digits.split_once([':', '.', 'h']) {
        Some((h, m)) => (h.parse().ok()?, if m.is_empty() { 0 } else { m.parse().ok()? }),
        None if digits.len() >= 3 && digits.chars().all(|c| c.is_ascii_digit()) => (digits[..digits.len() - 2].parse().ok()?, digits[digits.len() - 2..].parse().ok()?),
        None => (digits.parse().ok()?, 0),
    };
    let hour = match half {
        Some(_) if !(1..=12).contains(&hour) => return None,
        Some(true) => hour % 12 + 12,
        Some(false) => hour % 12,
        None => hour,
    };
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then(|| Time::constant(hour as i8, minute as i8, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Addresses as they're typed: apart by commas, semicolons or
    /// spaces, mail's "Name <address>" too, each once; what isn't an
    /// address is said. And a draft's guests written into an event
    /// there: asked, the others kept, with the calendar's own address
    /// as who invites.
    #[test]
    fn guests_typed_and_written() {
        assert_eq!(Invite::read("dana@example.org, Sam Okafor <sam@example.org>; kim@example.org DANA@example.org\n").unwrap(), ["dana@example.org", "sam@example.org", "kim@example.org"]);
        assert_eq!(Invite::read("  ").unwrap(), Vec::<String>::new());
        assert_eq!(Invite::read("dana@example.org, sam"), Err("sam".into()));
        assert_eq!(Invite::read("dana@example"), Err("dana@example".into()));
        assert_eq!(Invite::read("Sam <sam at example>"), Err("sam at example".into()));
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let mut cal = olo_calendar::ical::parse(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:lunch\nDTSTART;TZID=Europe/Amsterdam:20261120T120000\nDTEND;TZID=Europe/Amsterdam:20261120T130000\nSUMMARY:Lunch\nEND:VEVENT\nEND:VCALENDAR\n",
        )
        .remove(0);
        let event = Event::read(&cal.children[0]).unwrap();
        let at = jiff::civil::date(2026, 11, 20).at(12, 0, 0, 0);
        let mut draft = Draft::of(&event, at, jiff::civil::date(2026, 11, 20).at(13, 0, 0, 0));
        // (Not the draft's to say: nothing of guests written.)
        change_all(&mut cal, &draft, at, None, &zone).unwrap();
        assert!(!cal.write().contains("ATTENDEE"));
        draft.invite = Some(Invite { guests: vec!["dana@example.org".into()], organizer: "ben@example.org".into() });
        change_all(&mut cal, &draft, at, None, &zone).unwrap();
        let after = Event::read(&cal.children[0]).unwrap();
        assert_eq!(after.organizer.as_ref().map(|o| o.address.as_str()), Some("ben@example.org"));
        assert_eq!(
            after.attendees.iter().map(|p| (p.address.as_str(), p.answer)).collect::<Vec<_>>(),
            [("ben@example.org", olo_calendar::event::Answer::Yes), ("dana@example.org", olo_calendar::event::Answer::NotYet)]
        );
        // A new event's guests go with it.
        let mut new = Event::new("new", "", When::Date(at.date()), When::Date(at.date()));
        Draft { invite: draft.invite.clone(), ..Draft::new(at, false) }.apply(&mut new, at, at, &zone);
        assert_eq!((new.organizer.map(|o| o.address), new.attendees.len()), (Some("ben@example.org".into()), 1));
    }
    use jiff::civil::date as day;
    use olo_calendar::ical;

    #[test]
    fn days_and_times_as_typed() {
        let today = day(2026, 11, 18); // a Wednesday
        for (text, want) in [
            ("2026-11-20", Some(day(2026, 11, 20))),
            ("Fri, Nov 20, 2026", Some(day(2026, 11, 20))),
            ("Nov 20", Some(day(2026, 11, 20))),
            ("20 November", Some(day(2026, 11, 20))),
            ("20th november 2027", Some(day(2027, 11, 20))),
            ("December 3, 26", None),
            ("Jan 5", Some(day(2027, 1, 5))),
            ("Sep 30", Some(day(2026, 9, 30))),
            ("today", Some(today)),
            ("Tomorrow", Some(day(2026, 11, 19))),
            ("fri", Some(day(2026, 11, 20))),
            ("Wednesday", Some(day(2026, 11, 25))),
            ("20/11", Some(day(2026, 11, 20))),
            ("20.11.2026", Some(day(2026, 11, 20))),
            ("3/4", Some(day(2027, 4, 3))),
            ("3/4/27", Some(day(2027, 4, 3))),
            ("31/2", None),
            ("nonsense", None),
            ("", None),
        ] {
            assert_eq!(date(text, today, false), want, "{text}");
        }
        // The month first, where people write it so; a day past 12 says which is which anyway.
        assert_eq!((date("3/4", today, true), date("11/20", today, true), date("20/11", today, true)), (Some(day(2027, 3, 4)), Some(day(2026, 11, 20)), Some(day(2026, 11, 20))));
        assert_eq!(date(&date_text(today), today, false), Some(today));
        let t = |h, m| Some(Time::constant(h, m, 0, 0));
        for (text, want) in [
            ("14:00", t(14, 0)),
            ("1400", t(14, 0)),
            ("930", t(9, 30)),
            ("14", t(14, 0)),
            ("2pm", t(14, 0)),
            ("2 PM", t(14, 0)),
            ("2:30 pm", t(14, 30)),
            ("2.30pm", t(14, 30)),
            ("12 AM", t(0, 0)),
            ("12pm", t(12, 0)),
            ("9a", t(9, 0)),
            ("9:05 a.m.", t(9, 5)),
            ("noon", t(12, 0)),
            ("midnight", t(0, 0)),
            ("14h30", t(14, 30)),
            ("25:00", None),
            ("13pm", None),
            ("9:75", None),
            ("soon", None),
        ] {
            assert_eq!(time(text), want, "{text}");
        }
        // What the fields show reads back as what it was.
        for (h, m) in [(0, 0), (9, 5), (12, 0), (14, 30), (23, 59)] {
            let at = Time::constant(h, m, 0, 0);
            assert_eq!((time(&words::time(at, true)), time(&words::time(at, false))), (Some(at), Some(at)));
        }
    }

    const STANDUP: &str = "BEGIN:VCALENDAR
PRODID:-//Example//EN
BEGIN:VEVENT
UID:standup@example.org
DTSTART;TZID=Europe/Amsterdam:20261116T090000
DTEND;TZID=Europe/Amsterdam:20261116T093000
RRULE:FREQ=WEEKLY
SUMMARY:Standup
ATTENDEE;PARTSTAT=ACCEPTED:mailto:dana@example.org
X-SERVER:kept
BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:-PT10M
END:VALARM
END:VEVENT
END:VCALENDAR
";

    fn read(cal: &Component) -> (Event, Vec<Event>) {
        let mut all: Vec<Event> = cal.children_named("VEVENT").filter_map(Event::read).collect();
        let at = all.iter().position(|e| e.recurrence_id.is_none()).unwrap();
        let main = all.remove(at);
        (main, all)
    }

    fn zone() -> TimeZone {
        TimeZone::get("Europe/Amsterdam").unwrap()
    }

    fn starts(cal: &Component, from: i8, to: i8) -> Vec<String> {
        let (main, others) = read(cal);
        // (Days of November, counted on past its end.)
        let at = |d: i8| day(2026, 11, 1).checked_add(Span::new().days(d as i64 - 1)).unwrap().at(0, 0, 0, 0).to_zoned(zone()).unwrap();
        event::occurrences(&main, &others, &at(from), &at(to), &zone())
            .iter()
            .map(|o| format!("{} {}", o.start.strftime("%d %H:%M"), if o.changed.is_some() { others[o.changed.unwrap()].summary.clone() } else { main.summary.clone() }))
            .collect()
    }

    /// An event read into a draft says what the editor shows; written
    /// back unchanged it's the same event.
    #[test]
    fn a_draft_of_an_event() {
        let mut cal = ical::parse(STANDUP).remove(0);
        let (event, _) = read(&cal);
        let at = day(2026, 11, 23).at(9, 0, 0, 0);
        let draft = Draft::of(&event, at, day(2026, 11, 23).at(9, 30, 0, 0));
        assert_eq!((draft.title.as_str(), draft.all_day, &draft.repeat, draft.until, draft.reminder), ("Standup", false, &Repeat::Weekly, None, Some(600)));
        assert_eq!(draft.wrong(), None);
        // Nothing changed, saved for every occurrence: the event as it was (but for its stamps).
        change_all(&mut cal, &draft, at, None, &zone()).unwrap();
        let (after, _) = read(&cal);
        assert_eq!((&after.start, &after.end, &after.rule, &after.alarms, after.attendees.len()), (&event.start, &event.end, &event.rule, &event.alarms, 1));
        assert!(cal.write().contains("X-SERVER:kept") && cal.write().contains("SEQUENCE:1"));
        // Ways of repeating the editor doesn't offer are said, and left alone.
        for (rule, said) in [
            ("FREQ=MONTHLY;BYDAY=2TU", "Every month, on set days"),
            ("FREQ=DAILY;COUNT=5", "Every day, 5 times"),
            ("FREQ=WEEKLY;INTERVAL=3", "Every 3 weeks"),
            ("FREQ=WEEKLY;BYDAY=MO,WE", "Every week, on set days"),
        ] {
            let mut e = event.clone();
            e.rule = Rule::parse(rule);
            let d = Draft::of(&e, at, at);
            assert_eq!(d.repeat, Repeat::Other(said.into()), "{rule}");
            d.apply(&mut e, at, at, &zone());
            assert_eq!(e.rule, Rule::parse(rule));
        }
        let mut e = event.clone();
        e.rule = Rule::parse("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;UNTIL=20261231T225959Z");
        let d = Draft::of(&e, at, at);
        assert_eq!((d.repeat, d.until), (Repeat::Weekdays, Some(day(2026, 12, 31))));
        // A day's: its last day as people say it, the day after as it's kept.
        let mut away = Event::new("a", "Away", When::Date(day(2026, 12, 24)), When::Date(day(2026, 12, 27)));
        let mut d = Draft::of(&away, day(2026, 12, 24).at(0, 0, 0, 0), day(2026, 12, 27).at(0, 0, 0, 0));
        assert_eq!((d.all_day, d.start.date(), d.end.date()), (true, day(2026, 12, 24), day(2026, 12, 26)));
        d.end = day(2026, 12, 28).at(0, 0, 0, 0);
        d.repeat = Repeat::Yearly;
        d.until = Some(day(2030, 1, 1));
        d.apply(&mut away, d.start, d.end, &zone());
        assert_eq!((away.end.clone(), away.rule.as_ref().map(|r| r.write())), (When::Date(day(2026, 12, 29)), Some("FREQ=YEARLY;UNTIL=20300101".into())));
        // Made a meeting: a time in the person's zone.
        d.all_day = false;
        d.start = day(2026, 12, 24).at(10, 0, 0, 0);
        d.end = day(2026, 12, 24).at(11, 0, 0, 0);
        d.repeat = Repeat::Never;
        d.apply(&mut away, d.start, d.end, &zone());
        assert_eq!(away.start.line("DTSTART").write(), "DTSTART;TZID=Europe/Amsterdam:20261224T100000");
        assert!(away.rule.is_none() && !away.repeats);
        assert_eq!(Draft { end: day(2026, 12, 23).at(0, 0, 0, 0), ..d.clone() }.wrong(), Some("It ends before it starts."));
    }

    /// One Monday's standup moved to ten: for every Monday, the event
    /// itself moves; for that one only, it gets an occurrence of its own
    /// and the others stay; removed alone, it's taken out and the rest go
    /// on.
    #[test]
    fn every_occurrence_or_only_the_one() {
        let from = day(2026, 11, 23).at(9, 0, 0, 0);
        let id = When::Zoned(from.to_zoned(zone()).unwrap());
        let (event, _) = read(&ical::parse(STANDUP)[0]);
        let mut draft = Draft::of(&event, from, day(2026, 11, 23).at(9, 30, 0, 0));
        draft.start = day(2026, 11, 23).at(10, 0, 0, 0);
        draft.end = day(2026, 11, 23).at(11, 0, 0, 0);
        draft.title = "Standup, later".into();
        // All of them: from the first, at ten, an hour.
        let mut all = ical::parse(STANDUP).remove(0);
        change_all(&mut all, &draft, from, Some(&id), &zone()).unwrap();
        assert_eq!(starts(&all, 15, 31), ["16 10:00 Standup, later", "23 10:00 Standup, later", "30 10:00 Standup, later"]);
        assert_eq!(read(&all).0.end.local(), day(2026, 11, 16).at(11, 0, 0, 0));
        // Only that one.
        let mut one = ical::parse(STANDUP).remove(0);
        change_one(&mut one, &id, &draft, &zone()).unwrap();
        assert_eq!(starts(&one, 15, 31), ["16 09:00 Standup", "23 10:00 Standup, later", "30 09:00 Standup"]);
        let text = one.write();
        assert!(text.contains("RECURRENCE-ID;TZID=Europe/Amsterdam:20261123T090000") && text.matches("RRULE").count() == 1 && text.matches("X-SERVER:kept").count() == 2);
        // Changed again: the same occurrence of its own, not another.
        draft.title = "Standup, later still".into();
        draft.start = day(2026, 11, 24).at(8, 0, 0, 0);
        draft.end = day(2026, 11, 24).at(8, 30, 0, 0);
        change_one(&mut one, &id, &draft, &zone()).unwrap();
        assert_eq!(one.children_named("VEVENT").count(), 2);
        assert_eq!(starts(&one, 15, 31), ["16 09:00 Standup", "24 08:00 Standup, later still", "30 09:00 Standup"]);
        // Removed alone: gone, with its own text; the rest go on. Twice is once.
        remove_one(&mut one, &id, &zone()).unwrap();
        remove_one(&mut one, &id, &zone()).unwrap();
        assert_eq!(starts(&one, 15, 31), ["16 09:00 Standup", "30 09:00 Standup"]);
        assert_eq!((one.children_named("VEVENT").count(), one.write().matches("EXDATE").count()), (1, 1));
        // The whole thing reads back as iCalendar.
        assert_eq!(ical::parse(&one.write()).len(), 1);
        // An occurrence changed by itself, one taken out, and then all of
        // them changed from the changed one (as the editor does: its
        // draft says how the event repeats, not how that one does): the
        // event still repeats, under its new name, the changed one too;
        // moved an hour, every occurrence is, the one taken out stays
        // out, and nothing shows twice.
        let mut both = ical::parse(STANDUP).remove(0);
        let mut own = Draft::of(&event, from, day(2026, 11, 23).at(9, 30, 0, 0));
        own.title = "Standup (short)".into();
        change_one(&mut both, &id, &own, &zone()).unwrap();
        remove_one(&mut both, &When::Zoned(day(2026, 11, 30).at(9, 0, 0, 0).to_zoned(zone()).unwrap()), &zone()).unwrap();
        assert_eq!(starts(&both, 15, 38), ["16 09:00 Standup", "23 09:00 Standup (short)", "07 09:00 Standup"]);
        let mut all = Draft { title: "Daily".into(), ..own.clone() };
        all.start = day(2026, 11, 23).at(10, 0, 0, 0);
        all.end = day(2026, 11, 23).at(10, 30, 0, 0);
        assert_eq!(all.repeat, Repeat::Weekly);
        change_all(&mut both, &all, from, Some(&id), &zone()).unwrap();
        assert_eq!(starts(&both, 15, 38), ["16 10:00 Daily", "23 10:00 Daily", "07 10:00 Daily"]);
        let text = both.write();
        assert!(text.contains("RECURRENCE-ID;TZID=Europe/Amsterdam:20261123T100000") && text.contains("EXDATE;TZID=Europe/Amsterdam:20261130T100000"), "{text}");
    }
}
