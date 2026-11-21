//! To-Do's Deskbar widgets (`olo_protocol::glance`; Ben, 2026-11-18:
//! "beautiful styled deskbar widgets"). `olo-todo --glance`, started by
//! Deskbar, no window:
//!
//! - **Today**: how many are due today ("3 due today", "2 overdue" under
//!   it where any are), and the first of them in order, the overdue
//!   first, each with its time ("3 PM", "Today", the day it was due) and
//!   a mark in its list's colour.
//! - **Upcoming**: what's due after today, the soonest first, each by
//!   its day ("Fri", "Nov 28").
//!
//! A click opens To-Do on the first of them. Read from the lists' files:
//! each a timeline, an entry wherever what it says changes (a due time
//! passing, midnight) for the next day, so it goes on with nothing run;
//! made again when a list or a to-do changes, and every few hours.

use std::time::Duration;

use jiff::{Zoned, civil::DateTime, tz::TimeZone};
use olo_calendar::event::When;
use olo_protocol::glance::{Body, Entry, Glance, Line, Request, Timeline};
use olo_todo::{
    Book, store,
    view::{Place, Row, due_day, overdue, rows},
};

use crate::words;

/// How far ahead a timeline goes (seconds), and the most entries in one.
const AHEAD: i64 = 24 * 3600;
const MOST: usize = 48;
/// How often they're made again with nothing changed.
const AGAIN: Duration = Duration::from_secs(6 * 3600);

/// Serve Deskbar's To-Do widgets until it asks for none.
pub fn serve() -> std::process::ExitCode {
    let root = store::root();
    // (The folder of lists and each list's, a to-do being a file in its
    // list's: as the app and its reminders follow them. The clock's
    // settings are the desktop's, which every provider follows.)
    let lists = root.clone();
    let follow = move || {
        let folders: Vec<std::path::PathBuf> = std::fs::read_dir(&lists).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        std::iter::once(lists.clone()).chain(folders).collect()
    };
    olo_kit::glance::serve_following("todo", follow, move |asked| {
        let zone = TimeZone::system();
        let now = crate::now(&zone);
        let book = Book::load(&root);
        let h24 = olo_kit::settings::current().is_some_and(|s| s.clock.twenty_four_hour);
        (asked.iter().map(|r| (r.widget, timeline(r, &book, &now, h24))).collect(), Some(AGAIN))
    })
}

/// The widget asked for, from `now` on.
pub fn timeline(r: &Request, book: &Book, now: &Zoned, h24: bool) -> Timeline {
    let made = now.timestamp().as_second();
    let kind = olo_protocol::glance::kind("todo", &r.kind);
    let option = |id: &str, or: &str| kind.map_or(or.to_owned(), |k| k.value(&r.options, id).to_owned());
    let count: usize = option("count", "5").parse().unwrap_or(5).clamp(1, 12);
    let zone = now.time_zone();
    // Wherever what it says changes within the day ahead: now, each due
    // time passing (it's overdue from then), midnight.
    let at = now.datetime();
    let until = at.checked_add(jiff::Span::new().seconds(AHEAD)).unwrap_or(at);
    let mut times: Vec<DateTime> = vec![at];
    // (Overdue from the minute after it was due.)
    times.extend(book.items.iter().filter(|i| i.todo.open()).filter_map(|i| match &i.todo.due {
        Some(d) if !d.is_date() => d.moment(zone).datetime().checked_add(jiff::Span::new().minutes(1)).ok(),
        _ => None,
    }));
    if let Ok(tomorrow) = at.date().tomorrow() {
        times.push(tomorrow.at(0, 0, 0, 0));
    }
    times.retain(|t| *t >= at && *t <= until);
    times.sort();
    times.dedup();
    times.truncate(MOST);
    let entries = times
        .into_iter()
        .filter_map(|t| {
            let then = t.to_zoned(zone.clone()).ok()?;
            let from = then.timestamp().as_second();
            let glance = if r.kind == "upcoming" { upcoming(book, &then, count) } else { today(book, &then, count, h24) };
            Some(Entry { from: from - from.rem_euclid(60), glance })
        })
        .collect();
    Timeline { entries, made }
}

fn items<'a>(book: &'a Book, place: &Place, now: &Zoned) -> Vec<&'a olo_todo::Item> {
    rows(book, place, now, false)
        .into_iter()
        .filter_map(|r| match r {
            Row::ToDo(i) => book.items.get(i),
            Row::Heading(_) | Row::Nothing => None,
        })
        .collect()
}

fn hue_of(book: &Book, item: &olo_todo::Item) -> Option<olo_theme::Hue> {
    book.list(&item.list).map(|l| l.hue)
}

/// Today at `now`: how many are due, and the first of them.
fn today(book: &Book, now: &Zoned, count: usize, h24: bool) -> Glance {
    let due = items(book, &Place::Today, now);
    if due.is_empty() {
        return Glance {
            icon: Some("todo".into()),
            plain: true,
            title: olo_kit::words::tr("Nothing due today").into(),
            tip: olo_kit::words::tr("To-Do: nothing due today.").into(),
            ..Glance::default()
        };
    }
    let late = due.iter().filter(|i| i.todo.due.as_ref().is_some_and(|d| due_day(d, now.time_zone()) < now.date())).count();
    let (lines, said): (Vec<Line>, Vec<String>) = due
        .iter()
        .take(count)
        .map(|i| {
            let d = i.todo.due.as_ref();
            // (Short: a Deskbar is narrow, and the words are what's read.
            // One overdue by its day, a week's by name; one of today's by
            // its time, or Today.)
            let label = match d {
                Some(d) if due_day(d, now.time_zone()) < now.date() => past_day(due_day(d, now.time_zone()), now.date()),
                Some(When::Date(_)) | None => olo_kit::words::tr("Today").into(),
                Some(other) => words::time(other.moment(now.time_zone()).time(), h24),
            };
            let past = d.is_some_and(|d| overdue(d, now));
            (
                Line { label: label.clone(), text: i.todo.title.clone(), hue: if past { Some(olo_theme::Hue::Red) } else { hue_of(book, i) }, dim: false, ring: true },
                format!("{label} {}", i.todo.title),
            )
        })
        .unzip();
    // (How many are today's; the overdue said under them, or alone
    // where nothing else is due.)
    let overdue_said = olo_kit::words::count(late as u64, "1 overdue", "{n} overdue");
    let (title, detail) = match (due.len() - late, late) {
        (0, _) => (overdue_said, String::new()),
        (n, 0) => (olo_kit::words::count(n as u64, "1 due today", "{n} due today"), String::new()),
        (n, _) => (olo_kit::words::count(n as u64, "1 due today", "{n} due today"), overdue_said),
    };
    let head = if detail.is_empty() { title.clone() } else { olo_kit::tr!("{} ({})", title, detail) };
    let tip = olo_kit::tr!("To-Do: {}: {}.", head, said.join("; "));
    Glance { icon: Some("todo".into()), value: String::new(), title, detail, note: None, body: Some(Body::Lines { lines }), tip, plain: true, open: vec!["--show".to_owned(), due[0].todo.uid.clone()] }
}

/// Upcoming at `now`: what's due after today, the soonest first.
fn upcoming(book: &Book, now: &Zoned, count: usize) -> Glance {
    let later = items(book, &Place::Upcoming, now);
    if later.is_empty() {
        return Glance {
            icon: Some("todo".into()),
            plain: true,
            title: olo_kit::words::tr("Nothing coming up").into(),
            tip: olo_kit::words::tr("To-Do: nothing due after today.").into(),
            ..Glance::default()
        };
    }
    let (lines, said): (Vec<Line>, Vec<String>) = later
        .iter()
        .take(count)
        .map(|i| {
            let day = i.todo.due.as_ref().map(|d| due_day(d, now.time_zone())).unwrap_or(now.date());
            let label = short_day(day, now.date());
            (Line { label: label.clone(), text: i.todo.title.clone(), hue: hue_of(book, i), dim: false, ring: true }, format!("{label} {}", i.todo.title))
        })
        .unzip();
    let title = olo_kit::words::count(later.len() as u64, "1 coming up", "{n} coming up");
    let tip = olo_kit::tr!("To-Do: {}: {}.", title, said.join("; "));
    Glance {
        icon: Some("todo".into()),
        value: String::new(),
        title,
        detail: String::new(),
        note: None,
        body: Some(Body::Lines { lines }),
        tip,
        plain: true,
        open: vec!["--show".to_owned(), later[0].todo.uid.clone()],
    }
}

/// A day gone by, in a line's label: its weekday within the week
/// ("Wed"), else its date ("Nov 12").
fn past_day(day: jiff::civil::Date, today: jiff::civil::Date) -> String {
    match (today - day).get_days() {
        1..=6 => words::short_weekday(day).to_owned(),
        _ => words::short_date(day, today),
    }
}

/// A day in a line's label: its weekday within the week ("Fri"), else
/// its date ("Nov 28").
fn short_day(day: jiff::civil::Date, today: jiff::civil::Date) -> String {
    match (day - today).get_days() {
        1..=6 => words::short_weekday(day).to_owned(),
        _ => words::short_date(day, today),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_todo::{Item, List, ToDo};

    fn book() -> Book {
        let item = |uid: &str, title: &str, due: When| {
            let mut t = ToDo::new(uid, title);
            t.due = Some(due);
            Item { list: "home".into(), file: format!("/x/{uid}.ics").into(), changed: None, todo: t }
        };
        let at = |d: i8, h: i8| When::Zoned(jiff::civil::date(2026, 11, d).at(h, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap());
        let day = |d: i8| When::Date(jiff::civil::date(2026, 11, d));
        Book {
            lists: vec![List { id: "home".into(), name: "Home".into(), hue: olo_theme::Hue::Green, sections: vec![] }],
            items: vec![item("a", "Fix the gate", day(18)), item("b", "Call Ann", at(19, 15)), item("c", "Bins", day(19)), item("d", "Paint", day(21)), item("e", "Dentist", day(30))],
        }
    }

    #[test]
    fn today_and_what_is_coming() {
        let now = jiff::civil::date(2026, 11, 19).at(10, 30, 0, 0).to_zoned(TimeZone::UTC).unwrap();
        let asked = |widget: u32, kind: &str| Request { widget, kind: kind.into(), options: Default::default(), size: Default::default() };
        let r = asked(1, "today");
        let t = timeline(&r, &book(), &now, false);
        let g = &t.entries[0].glance;
        assert_eq!((g.title.as_str(), g.detail.as_str()), ("2 due today", "1 overdue"));
        let Some(Body::Lines { lines }) = &g.body else { panic!("no lines") };
        assert_eq!(lines.iter().map(|l| (l.label.as_str(), l.text.as_str())).collect::<Vec<_>>(), [("Wed", "Fix the gate"), ("Today", "Bins"), ("3 PM", "Call Ann")]);
        assert_eq!(g.open, ["--show", "a"]);
        // Once 3 PM passes, the call is overdue (its mark red); at
        // midnight, what was today's is yesterday's.
        assert_eq!(t.entries.len(), 3);
        let Some(Body::Lines { lines }) = &t.entries[1].glance.body else { panic!("no lines") };
        assert_eq!(lines[2].hue, Some(olo_theme::Hue::Red));
        assert_eq!((t.entries[2].glance.title.as_str(), t.entries[2].glance.detail.as_str()), ("3 overdue", ""));
        let up = timeline(&asked(2, "upcoming"), &book(), &now, false);
        let g = &up.entries[0].glance;
        assert_eq!(g.title, "2 coming up");
        let Some(Body::Lines { lines }) = &g.body else { panic!("no lines") };
        assert_eq!(lines.iter().map(|l| (l.label.as_str(), l.text.as_str())).collect::<Vec<_>>(), [("Sat", "Paint"), ("Nov 30", "Dentist")]);
    }
}
