//! Calendar's Deskbar widgets (`olo_protocol::glance`; Ben, 2026-11-17:
//! "both the calendar and email should also integrate with deskbar and
//! provide widgets"). `olo-calendar --glance`, started by Deskbar, no
//! window:
//!
//! - **Next Event**: what's next, or on now: when ("2 PM", "Now"), its
//!   title, the day and where ("Today · Room 2", "until 3:30 PM"), and
//!   under it what comes after, its time first ("Then 3 PM · Interview":
//!   a Deskbar is narrow, and what's cut is the title's end). Events
//!   with times; a whole day's aren't "next". A click opens Calendar on
//!   the event.
//! - **The Day Ahead**: the day and how much of it is left ("Wed 18 · 3
//!   to come", a line of ordinary words), and what's
//!   still to come in order, each with its time and a mark in its
//!   calendar's colour (3, 5 or 8); with room left, the days after,
//!   each by its weekday (its time is in the tip).
//!
//! Of the calendars shown in Calendar, an invitation declined left out.
//! Read from what's kept on this computer (the reminders service keeps
//! that in step with the servers): nothing is asked of a server here.
//! Each a timeline, an entry wherever what it says changes (an event
//! starting or ending, midnight) for the next day, so it goes on with
//! nothing run; made again when the accounts, Calendar's settings or
//! what's kept change, and every few hours as the day ahead moves on.

use std::{sync::Arc, time::Duration};

use jiff::{
    Zoned,
    civil::{Date, DateTime},
    tz::TimeZone,
};
use olo_calendar::event::Answer;
use olo_protocol::glance::{Body, Entry, Glance, Line, Request, Timeline};

use crate::{
    Settings, layout,
    model::{Book, Choices, Shown},
    sync, words,
};

/// How far ahead a timeline goes (seconds), and the most entries in one.
const AHEAD: i64 = 24 * 3600;
const MOST: usize = 64;
/// How often they're made again with nothing changed (the day ahead
/// moves on; a timeline holds a day).
const AGAIN: Duration = Duration::from_secs(6 * 3600);

/// Serve Deskbar's Calendar widgets until it asks for none.
pub fn serve() -> std::process::ExitCode {
    let demo = sync::demo();
    // (Calendar's settings and the clock's are in the settings' folder.)
    let mut watched = olo_calendar::account::watched();
    watched.push(olo_kit::config_dir());
    watched.extend(sync::store_dir());
    // (The demo's calendars are nowhere but on its server: read once.)
    let mut held: Option<Book> = None;
    olo_kit::glance::serve("calendar", watched, move |asked| (make(asked, demo, &mut held), Some(AGAIN)))
}

fn make(asked: &[Request], demo: bool, held: &mut Option<Book>) -> Vec<(u32, Timeline)> {
    let zone = TimeZone::system();
    let now = crate::now(&zone);
    let (sources, kept) = sync::load(now.date());
    let order: Vec<String> = sources.iter().map(|s| s.account.id()).collect();
    let book = if demo {
        held.get_or_insert_with(|| {
            let mut book = Book::default();
            for source in &sources {
                if let (Some(cals), _) = sync::sync(source, Vec::new()) {
                    book = book.with(&source.account.id(), cals, &order);
                }
            }
            book
        })
        .clone()
    } else {
        Book::new(kept.into_iter().map(Arc::new).collect(), &order)
    };
    let settings: Settings = if demo { Settings::default() } else { olo_kit::store::load(&crate::settings_path()).unwrap_or_default() };
    let me: Vec<String> = sources.iter().map(|s| s.account.address.clone()).collect();
    let h24 = olo_kit::settings::current().is_some_and(|s| s.clock.twenty_four_hour);
    let with = With { book: &book, choices: &settings.choices, me: &me, zone: &zone, h24, set_up: !sources.is_empty() };
    asked.iter().map(|r| (r.widget, timeline(r, &with, &now))).collect()
}

/// What a timeline is made from.
pub struct With<'a> {
    pub book: &'a Book,
    pub choices: &'a Choices,
    pub me: &'a [String],
    pub zone: &'a TimeZone,
    pub h24: bool,
    /// An account is set up (none: the widget says so).
    pub set_up: bool,
}

/// The widget asked for, from `now` on.
pub fn timeline(r: &Request, with: &With, now: &Zoned) -> Timeline {
    let made = now.timestamp().as_second();
    let kind = olo_protocol::glance::kind("calendar", &r.kind);
    let option = |id: &str, or: &str| kind.map_or(or.to_owned(), |k| k.value(&r.options, id).to_owned());
    if !with.set_up {
        let glance = Glance {
            icon: Some("calendar".into()),
            plain: true,
            title: olo_kit::words::tr("No calendars yet").into(),
            detail: olo_kit::words::tr("Add an account in Preferences").into(),
            tip: olo_kit::words::tr("Calendar: no account set up. Add one in Preferences ▸ Integrations.").into(),
            ..Glance::default()
        };
        return Timeline { entries: vec![Entry { from: made - made % 60, glance }], made };
    }
    let day = r.kind == "day";
    let days: i32 = if day { 7 } else { option("ahead", "7").parse().unwrap_or(7).clamp(1, 60) };
    let count: usize = option("count", "5").parse().unwrap_or(5).clamp(1, 12);
    let at = now.datetime();
    let start = at.date();
    // What's on from today: not over, not declined, in order.
    let coming: Vec<Shown> =
        with.book.shown(start, layout::day_of(layout::day_number(start) + days + 1), with.zone, with.me, with.choices).into_iter().filter(|s| s.end > at && s.going != Some(Answer::No)).collect();
    // Wherever what it says changes within the day ahead: now, each
    // start and end, each midnight.
    let until = at.checked_add(jiff::Span::new().seconds(AHEAD)).unwrap_or(at);
    let mut times: Vec<DateTime> = vec![at];
    times.extend(coming.iter().flat_map(|s| [s.start, s.end]).filter(|t| *t > at && *t <= until));
    times.push(layout::day_of(layout::day_number(start) + 1).at(0, 0, 0, 0));
    times.sort();
    times.dedup();
    times.truncate(MOST);
    let entries = times
        .into_iter()
        .filter_map(|t| {
            // (On a minute, as Deskbar's clock ticks.)
            let from = t.to_zoned(with.zone.clone()).ok()?.timestamp().as_second();
            let glance = if day { the_day(&coming, t, count, with) } else { next(&coming, t, days, with) };
            Some(Entry { from: from - from.rem_euclid(60), glance })
        })
        .collect();
    Timeline { entries, made }
}

/// A day as it's said from `today`: "Today", "Tomorrow", its weekday
/// within the week, else "Fri, Nov 27".
fn day_word(day: Date, today: Date) -> String {
    match layout::day_number(day) - layout::day_number(today) {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        2..=6 => words::weekday_name(day).into(),
        _ => format!("{}, {} {}", &words::weekday_name(day)[..3], &words::month_name(day)[..3], day.day()),
    }
}

/// Next Event at `t`: the first event with times that isn't over.
fn next(coming: &[Shown], t: DateTime, days: i32, with: &With) -> Glance {
    let mut timed = coming.iter().filter(|s| !s.all_day && s.end > t);
    let Some(s) = timed.next() else {
        let within = match days {
            1 => olo_kit::words::tr("today or tomorrow").to_owned(),
            7 => olo_kit::words::tr("in the next week").to_owned(),
            n => olo_kit::tr!("in the next {} days", n),
        };
        return Glance {
            icon: Some("calendar".into()),
            plain: true,
            title: olo_kit::words::tr("Nothing coming up").into(),
            detail: within.clone(),
            tip: olo_kit::tr!("Calendar: nothing coming up {}.", within),
            ..Glance::default()
        };
    };
    let calendar = with.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
    let place = |words: String| if s.location.is_empty() { words } else { format!("{words} · {}", s.location) };
    let (value, detail) = if s.start <= t {
        ("Now".to_owned(), place(format!("until {}", words::time(s.end.time(), with.h24))))
    } else {
        (words::time(s.start.time(), with.h24), place(day_word(s.start.date(), t.date())))
    };
    // What comes after it.
    let note = timed.next().map(|n| {
        let when = words::time(n.start.time(), with.h24);
        if n.start.date() == t.date() { format!("Then {when} · {}", n.title) } else { format!("Then {} {when} · {}", &words::weekday_name(n.start.date())[..3], n.title) }
    });
    let said: Vec<String> = [s.title.to_string(), format!("{} {}", day_word(s.start.date(), t.date()).to_lowercase(), words::when(s.start, s.end, false, with.h24)), s.location.to_string(), calendar]
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect();
    // (A click: Calendar on it, as a reminder's Show.)
    let open = vec!["--show".to_owned(), s.href.to_string(), s.start.to_string()];
    Glance { icon: Some("calendar".into()), value, title: s.title.to_string(), detail, note, body: None, tip: format!("Calendar: {}.", said.join(", ")), plain: true, open }
}

/// The Day Ahead at `t`: the day, what's left of it, and the next
/// `count` things in order.
fn the_day(coming: &[Shown], t: DateTime, count: usize, with: &With) -> Glance {
    let today = t.date();
    let left: Vec<&Shown> = coming.iter().filter(|s| s.end > t).collect();
    // (On today: started by its end; a whole day's, or several days', are
    // on every day they cover.)
    let today_end = layout::day_of(layout::day_number(today) + 1).at(0, 0, 0, 0);
    let to_come = left.iter().filter(|s| s.start < today_end).count();
    // Each line, and how it's said in the tip (another day's with its
    // time, which its line has no room for).
    let (lines, said): (Vec<Line>, Vec<String>) = left
        .iter()
        .take(count)
        .map(|s| {
            let on_today = s.start < today_end;
            let weekday = &words::weekday_name(s.start.date())[..3];
            let (label, full) = match (on_today, s.all_day, s.start <= t) {
                (true, true, _) => (olo_kit::words::tr("all day").to_owned(), olo_kit::words::tr("all day").to_owned()),
                (true, false, true) => ("Now".to_owned(), format!("now, until {}", words::time(s.end.time(), with.h24))),
                (true, false, false) => (words::time(s.start.time(), with.h24), words::time(s.start.time(), with.h24)),
                (false, true, _) => (weekday.to_owned(), weekday.to_owned()),
                (false, false, _) => (weekday.to_owned(), format!("{weekday} {}", words::time(s.start.time(), with.h24))),
            };
            (Line { label, text: s.title.to_string(), hue: Some(s.hue), dim: s.going == Some(Answer::NotYet) || s.tentative, ring: false }, format!("{full} {}", s.title))
        })
        .unzip();
    let detail = match to_come {
        0 => olo_kit::words::tr("Nothing more today").to_owned(),
        n => olo_kit::words::count(n as u64, "1 to come", "{n} to come"),
    };
    let tip = format!("Calendar: {}. {}{}", words::long_day(today), detail, if said.is_empty() { ".".to_owned() } else { format!(": {}.", said.join("; ")) });
    // (One line of ordinary words over the list: "Wed 18  3 to come".)
    Glance {
        icon: Some("calendar".into()),
        value: format!("{} {}", &words::weekday_name(today)[..3], today.day()),
        title: detail,
        detail: String::new(),
        note: None,
        body: (!lines.is_empty()).then_some(Body::Lines { lines }),
        tip,
        plain: true,
        // (A click: Calendar on the first of them.)
        open: left.first().map(|s| vec!["--show".to_owned(), s.href.to_string(), s.start.to_string()]).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use olo_calendar::store::{Item, Kept};

    use super::*;
    use crate::model::Cal;

    fn book() -> Book {
        let mut k = Kept { account: "a".into(), href: "https://x/work/".into(), name: "Work".into(), ..Default::default() };
        let events = [
            "DTSTART;TZID=Europe/Amsterdam:20261118T093000\nDTEND;TZID=Europe/Amsterdam:20261118T094500\nSUMMARY:Standup",
            "DTSTART;TZID=Europe/Amsterdam:20261118T140000\nDTEND;TZID=Europe/Amsterdam:20261118T153000\nSUMMARY:Design review\nLOCATION:Room 2",
            "DTSTART;TZID=Europe/Amsterdam:20261118T150000\nDTEND;TZID=Europe/Amsterdam:20261118T154500\nSUMMARY:Interview\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@example.org",
            "DTSTART;TZID=Europe/Amsterdam:20261118T160000\nDTEND;TZID=Europe/Amsterdam:20261118T170000\nSUMMARY:Budget call\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.org",
            "DTSTART;VALUE=DATE:20261118\nDTEND;VALUE=DATE:20261119\nSUMMARY:Release 2.4",
            "DTSTART;TZID=Europe/Amsterdam:20261119T093000\nDTEND;TZID=Europe/Amsterdam:20261119T094500\nSUMMARY:Standup",
            "DTSTART;TZID=Europe/Amsterdam:20261123T110000\nDTEND;TZID=Europe/Amsterdam:20261123T120000\nSUMMARY:Planning",
        ];
        for (i, body) in events.iter().enumerate() {
            k.items.insert(format!("https://x/work/{i}.ics"), Item { etag: "e".into(), ics: format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:w{i}\n{body}\nEND:VEVENT\nEND:VCALENDAR\n") });
        }
        Book::new(vec![Arc::new(Cal::read(k))], &["a".to_owned()])
    }

    fn asked(kind: &str, options: &[(&str, &str)]) -> Request {
        Request { widget: 1, kind: kind.into(), options: options.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<BTreeMap<_, _>>(), size: Default::default() }
    }

    /// Next Event: before it, when and where; while it's on, "Now" and
    /// until when; what comes after under it; an entry at every start
    /// and end; a declined invitation and a whole day's event are never
    /// "next"; with nothing left, it says so.
    #[test]
    fn what_is_next() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let (book, choices, me) = (book(), Choices::default(), ["me@example.org".to_owned()]);
        let with = With { book: &book, choices: &choices, me: &me, zone: &zone, h24: false, set_up: true };
        let now = jiff::civil::date(2026, 11, 18).at(10, 30, 20, 0).to_zoned(zone.clone()).unwrap();
        let t = timeline(&asked("next", &[]), &with, &now);
        let at = |h: i8, m: i8| {
            let unix = jiff::civil::date(2026, 11, 18).at(h, m, 0, 0).to_zoned(zone.clone()).unwrap().timestamp().as_second();
            t.at(unix).cloned().unwrap()
        };
        let head = |g: &Glance| (g.value.clone(), g.title.clone(), g.detail.clone(), g.note.clone().unwrap_or_default());
        assert_eq!(head(&at(10, 31)), ("2 PM".into(), "Design review".into(), "Today · Room 2".into(), "Then 3 PM · Interview".into()));
        assert_eq!(at(10, 31).open, ["--show", "https://x/work/1.ics", "2026-11-18T14:00:00"]);
        assert_eq!(head(&at(14, 0)), ("Now".into(), "Design review".into(), "until 3:30 PM · Room 2".into(), "Then 3 PM · Interview".into()));
        // (Design review over: the one that began meanwhile; then tomorrow's.)
        assert_eq!(head(&at(15, 30)), ("Now".into(), "Interview".into(), "until 3:45 PM".into(), "Then Thu 9:30 AM · Standup".into()));
        assert_eq!(head(&at(15, 45)), ("9:30 AM".into(), "Standup".into(), "Tomorrow".into(), "Then Mon 11 AM · Planning".into()));
        assert!(at(10, 31).tip.contains("Design review, today 2 PM – 3:30 PM, Room 2, Work"), "{}", at(10, 31).tip);
        // Its entries: now (on its minute), then each start and end, and midnight.
        let first = t.entries[0].from;
        assert_eq!((first % 60, now.timestamp().as_second() - first), (0, 20));
        // (Design review's and Interview's; midnight, where the whole
        // day's ends too; tomorrow's Standup's. Not the declined one's.)
        assert_eq!(t.entries.len(), 1 + 4 + 1 + 2, "{:?}", t.entries.iter().map(|e| e.from).collect::<Vec<_>>());
        // Only today and tomorrow looked at, late tomorrow: nothing.
        let late = jiff::civil::date(2026, 11, 19).at(18, 0, 0, 0).to_zoned(zone.clone()).unwrap();
        let none = timeline(&asked("next", &[("ahead", "1")]), &with, &late);
        assert_eq!((none.entries[0].glance.title.as_str(), none.entries[0].glance.detail.as_str()), ("Nothing coming up", "today or tomorrow"));
        // No account: said.
        let bare = With { set_up: false, ..with };
        assert_eq!(timeline(&asked("next", &[]), &bare, &now).entries[0].glance.title, "No calendars yet");
    }

    /// The Day Ahead: the day, how many are to come, and the next few in
    /// order with their times (a whole day's first; one on now, "Now";
    /// another day's with its weekday), one not answered quieter; as
    /// each ends it goes.
    #[test]
    fn the_day_ahead() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let (book, choices, me) = (book(), Choices::default(), ["me@example.org".to_owned()]);
        let with = With { book: &book, choices: &choices, me: &me, zone: &zone, h24: true, set_up: true };
        let now = jiff::civil::date(2026, 11, 18).at(10, 30, 0, 0).to_zoned(zone.clone()).unwrap();
        let t = timeline(&asked("day", &[("count", "3")]), &with, &now);
        let at = |d: i8, h: i8, m: i8| {
            let unix = jiff::civil::date(2026, 11, d).at(h, m, 0, 0).to_zoned(zone.clone()).unwrap().timestamp().as_second();
            t.at(unix).cloned().unwrap()
        };
        let lines = |g: &Glance| match &g.body {
            Some(Body::Lines { lines }) => lines.iter().map(|l| format!("{} {}{}", l.label, l.text, if l.dim { " (quiet)" } else { "" })).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let g = at(18, 10, 30);
        assert_eq!((g.value.as_str(), g.title.as_str(), g.detail.as_str(), g.plain), ("Wed 18", "3 to come", "", true));
        assert_eq!(lines(&g), ["all day Release 2.4", "14:00 Design review", "15:00 Interview (quiet)"]);
        assert_eq!(lines(&at(18, 15, 10)), ["all day Release 2.4", "Now Design review", "Now Interview (quiet)"]);
        // (Today's done but the whole day's: tomorrow's after it.)
        let g = at(18, 16, 0);
        assert_eq!((g.title.as_str(), lines(&g)), ("1 to come", vec!["all day Release 2.4".to_owned(), "Thu Standup".to_owned(), "Mon Planning".to_owned()]));
        assert!(g.tip.ends_with("1 to come: all day Release 2.4; Thu 09:30 Standup; Mon 11:00 Planning."), "{}", g.tip);
        assert_eq!(g.open, ["--show", "https://x/work/4.ics", "2026-11-18T00:00:00"]);
        // Midnight: the next day's own.
        let g = at(19, 0, 0);
        assert_eq!((g.value.as_str(), g.title.as_str()), ("Thu 19", "1 to come"));
        assert_eq!(lines(&g), ["09:30 Standup", "Mon Planning"]);
        assert!(g.tip.starts_with("Calendar: Thursday, November 19"), "{}", g.tip);
    }
}
