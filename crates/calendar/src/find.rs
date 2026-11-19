//! Finding events from outside Calendar (the command palette, the
//! assistant): from what's kept on this computer, by words in an
//! event's title, place or notes, each event once, at the occurrence
//! that matters now: its next, or with none to come its last.

use jiff::{Zoned, civil::DateTime, tz::TimeZone};

use crate::{
    event::{self, Event, Status},
    store::{Kept, Stored},
};

/// A calendar read to find in: its name, its address and its events
/// (read once and kept by whoever asks more than once: reading is the
/// costly part).
#[derive(Debug, Clone)]
pub struct Read {
    pub name: String,
    pub href: String,
    pub events: Vec<Stored>,
}

impl Read {
    pub fn of(kept: &Kept) -> Read {
        Read { name: kept.name.clone(), href: kept.href.clone(), events: kept.events() }
    }
}

/// How far ahead an event's next occurrence is looked for, and how far
/// back its last (days).
pub const AHEAD_DAYS: i64 = 2 * 366;
pub const BACK_DAYS: i64 = 366;

/// An event found, at one of its occurrences.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Its calendar's name, and its own address.
    pub calendar: String,
    pub href: String,
    pub title: String,
    pub location: String,
    /// The occurrence, on the clock of who's looking.
    pub start: DateTime,
    pub end: DateTime,
    pub all_day: bool,
    /// It isn't over yet (false: the last time it happened).
    pub upcoming: bool,
    pub repeats: bool,
}

/// Whether `event` has every one of `words` (lower case) in its title,
/// its place or its notes, whatever their case.
fn has(event: &Event, words: &[String]) -> bool {
    let text = format!("{}\n{}\n{}", event.summary, event.location, event.description).to_lowercase();
    words.iter().all(|w| text.contains(w.as_str()))
}

/// The events of `cals` (less the calendars `hidden` says, by address)
/// with every one of `words` (lower case; none: nothing found): each
/// once, at its next occurrence from `now`, or with none within
/// [`AHEAD_DAYS`] its last within [`BACK_DAYS`]. Those to come first,
/// the soonest first; then those past, the latest first; `most` at the
/// most. An occurrence changed by itself is found by its own words, and
/// shown with them.
pub fn find(cals: &[Read], hidden: &dyn Fn(&str) -> bool, words: &[String], now: &Zoned, most: usize) -> Vec<Found> {
    if words.is_empty() {
        return Vec::new();
    }
    let zone: TimeZone = now.time_zone().clone();
    let days = |n: i64| now.checked_add(jiff::Span::new().days(n)).unwrap_or_else(|_| now.clone());
    let (soon, far, back) = (days(35), days(AHEAD_DAYS), days(-BACK_DAYS));
    let mut found = Vec::new();
    for cal in cals.iter().filter(|k| !hidden(&k.href)) {
        for stored in &cal.events {
            if stored.event.status == Status::Cancelled || !(has(&stored.event, words) || stored.overrides.iter().any(|o| has(o, words))) {
                continue;
            }
            // The occurrence's own event (itself, changed) must be what
            // has the words.
            let of = |o: &event::Occurrence| o.changed.and_then(|i| stored.overrides.get(i)).unwrap_or(&stored.event);
            let matching = |from: &Zoned, to: &Zoned| -> Vec<event::Occurrence> {
                event::occurrences(&stored.event, &stored.overrides, from, to, &zone).into_iter().filter(|o| has(of(o), words) && of(o).status != Status::Cancelled).collect()
            };
            // Its next (looked for near first: most are), else its last.
            let next = [&soon, &far].into_iter().find_map(|to| matching(now, to).into_iter().find(|o| o.end > *now));
            let (at, upcoming) = match next {
                Some(o) => (o, true),
                None => match matching(&back, now).into_iter().rfind(|o| o.end <= *now) {
                    Some(o) => (o, false),
                    None => continue,
                },
            };
            let e = of(&at);
            found.push(Found {
                calendar: cal.name.clone(),
                href: stored.href.clone(),
                title: if e.summary.trim().is_empty() { "(No title)".to_owned() } else { e.summary.trim().to_owned() },
                location: e.location.trim().to_owned(),
                start: at.start.datetime(),
                end: at.end.datetime(),
                all_day: at.all_day,
                upcoming,
                repeats: stored.event.repeats || !stored.event.rdates.is_empty(),
            });
        }
    }
    found.sort_by(|a, b| match (a.upcoming, b.upcoming) {
        (true, true) => (a.start, &a.title).cmp(&(b.start, &b.title)),
        (false, false) => (b.start, &a.title).cmp(&(a.start, &b.title)),
        (x, y) => y.cmp(&x),
    });
    found.truncate(most);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Item;

    fn kept(name: &str, events: &[&str]) -> Kept {
        let mut k = Kept { account: "a".into(), href: format!("https://x/{name}/"), name: name.into(), ..Default::default() };
        for (i, body) in events.iter().enumerate() {
            k.items.insert(format!("https://x/{name}/{i}.ics"), Item { etag: "e".into(), ics: format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:{name}{i}\n{body}\nEND:VEVENT\nEND:VCALENDAR\n") });
        }
        k
    }

    /// Each event once: a repeating one at its next occurrence, one
    /// that's over at its last; by words in title, place and notes,
    /// whatever their case; a hidden calendar's and a cancelled event
    /// not found; those to come first.
    #[test]
    fn events_found_by_their_words() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let now = jiff::civil::date(2026, 11, 18).at(10, 30, 0, 0).to_zoned(zone).unwrap();
        let work = kept(
            "Work",
            &[
                "DTSTART;TZID=Europe/Amsterdam:20261109T093000\nDTEND;TZID=Europe/Amsterdam:20261109T094500\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR\nSUMMARY:Standup\nDESCRIPTION:The team's review of the day",
                "DTSTART;TZID=Europe/Amsterdam:20261118T140000\nDTEND;TZID=Europe/Amsterdam:20261118T153000\nSUMMARY:Design Review\nLOCATION:Room 2",
                "DTSTART;TZID=Europe/Amsterdam:20261002T140000\nDTEND;TZID=Europe/Amsterdam:20261002T150000\nSUMMARY:Quarterly review",
                "DTSTART;TZID=Europe/Amsterdam:20261120T140000\nDTEND;TZID=Europe/Amsterdam:20261120T150000\nSUMMARY:Review, called off\nSTATUS:CANCELLED",
                "DTSTART;VALUE=DATE:20270301\nDTEND;VALUE=DATE:20270304\nSUMMARY:Conference\nLOCATION:Lisbon",
                "DTSTART;TZID=Europe/Amsterdam:20240105T140000\nDTEND;TZID=Europe/Amsterdam:20240105T150000\nSUMMARY:Review of 2023",
            ],
        );
        let home = kept("Home", &["DTSTART;TZID=Europe/Amsterdam:20261119T190000\nDTEND;TZID=Europe/Amsterdam:20261119T200000\nSUMMARY:Book review club"]);
        let all = [Read::of(&work), Read::of(&home)];
        let look = |text: &str, hidden: &dyn Fn(&str) -> bool| -> Vec<String> {
            let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
            find(&all, hidden, &words, &now, 10).iter().map(|f| format!("{} {} {}{}", f.start.strftime("%m-%d %H:%M"), f.title, f.calendar, if f.upcoming { "" } else { " (past)" })).collect()
        };
        let none = |_: &str| false;
        // To come, soonest first (today's Standup is over: tomorrow's);
        // then what's past, the latest first; long ago, not at all.
        assert_eq!(look("REVIEW", &none), ["11-18 14:00 Design Review Work", "11-19 09:30 Standup Work", "11-19 19:00 Book review club Home", "10-02 14:00 Quarterly review Work (past)"]);
        assert_eq!(look("room review", &none), ["11-18 14:00 Design Review Work"]);
        assert_eq!(look("lisbon", &none), ["03-01 00:00 Conference Work"]);
        assert_eq!(look("review", &|href: &str| href.contains("/Work/")), ["11-19 19:00 Book review club Home"]);
        assert!(look("", &none).is_empty() && look("nothing-has-this", &none).is_empty());
        // (The most asked for.)
        assert_eq!(find(&all, &none, &["review".to_owned()], &now, 2).len(), 2);
        let found = find(&all, &none, &["standup".to_owned()], &now, 5);
        assert!(found[0].repeats && found[0].href == "https://x/Work/0.ics" && !found[0].all_day);
    }
}
