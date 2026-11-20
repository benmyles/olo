//! What each of To-Do's places shows, in order: the four it works out
//! (Today, Upcoming, All, Done) and each list's own, by its sections; a
//! list's board, its sections side by side; and where a to-do moved
//! among them goes (its section, and an order between its neighbours').
//!
//! What's shown is data (days, sections, lists): the app says it in the
//! person's words.

use jiff::{Zoned, civil::Date, tz::TimeZone};
use olo_calendar::event::When;

use crate::store::{Book, Item};

/// Where in To-Do the person is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    /// Due today, and what's overdue.
    Today,
    /// Due after today, by day.
    Upcoming,
    /// Everything still to do, by list.
    All,
    /// What's done, the newest first, by the day it was done.
    Done,
    /// A list (its id).
    List(String),
}

/// A heading among a place's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heading {
    /// What's overdue (Today's first).
    Overdue,
    /// A day (Upcoming's, Done's; Today's own, after the overdue).
    Day(Date),
    /// A list (All's): its id.
    List(String),
    /// A section of a list (None: those in none, where the list has
    /// sections).
    Section(Option<String>),
    /// What's done of a list, at its end (its board's last lane).
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Heading(Heading),
    /// A to-do: its index in the book's items.
    ToDo(usize),
    /// Under a section's heading that has nothing to do: said so (a
    /// heading with nothing under it looked like something had gone
    /// wrong).
    Nothing,
}

/// The day a due falls on, on `zone`'s clock.
pub fn due_day(due: &When, zone: &TimeZone) -> Date {
    match due {
        When::Date(d) => *d,
        other => other.moment(zone).date(),
    }
}

/// Whether a due is past: a day's once it's over, a time's once it's
/// gone.
pub fn overdue(due: &When, now: &Zoned) -> bool {
    match due {
        When::Date(d) => *d < now.date(),
        other => other.moment(now.time_zone()) < *now,
    }
}

/// A to-do's due as a moment to sort by (a day's: its start, before
/// that day's times).
fn due_key(item: &Item, zone: &TimeZone) -> (Date, i64) {
    match &item.todo.due {
        Some(When::Date(d)) => (*d, i64::MIN),
        Some(other) => {
            let m = other.moment(zone);
            (m.date(), m.timestamp().as_second())
        }
        None => (Date::MAX, i64::MAX),
    }
}

/// A list's to-dos in their own order: by place, then as made, then
/// by title.
fn by_order(book: &Book, a: usize, b: usize) -> std::cmp::Ordering {
    let (x, y) = (&book.items[a].todo, &book.items[b].todo);
    x.order.cmp(&y.order).then(x.created.cmp(&y.created)).then_with(|| x.title.cmp(&y.title)).then_with(|| x.uid.cmp(&y.uid))
}

/// A list's sections as shown: its own, in order, then any its to-dos
/// name that it doesn't (another program's, a section renamed
/// elsewhere), in the order first met.
pub fn sections(book: &Book, list: &str) -> Vec<String> {
    let mut out: Vec<String> = book.list(list).map(|l| l.sections.clone()).unwrap_or_default();
    let mut items: Vec<usize> = (0..book.items.len()).filter(|&i| book.items[i].list == list).collect();
    items.sort_by(|&a, &b| by_order(book, a, b));
    for i in items {
        if let Some(s) = &book.items[i].todo.section
            && !out.contains(s)
        {
            out.push(s.clone());
        }
    }
    out
}

/// What a place shows, in order. `done`: a list's own place ends with
/// what's done of it, the newest first (as its board's last lane has
/// them).
pub fn rows(book: &Book, place: &Place, now: &Zoned, done: bool) -> Vec<Row> {
    rows_where(book, place, now, done, &|item| item.todo.open())
}

/// [`rows`], with `open` saying which to-dos are still to do: the
/// window keeps one just checked off where it was a moment, as if it
/// weren't yet (without a copy of the book made to say so).
pub fn rows_where(book: &Book, place: &Place, now: &Zoned, done: bool, is_open: &dyn Fn(&Item) -> bool) -> Vec<Row> {
    let zone = now.time_zone();
    let today = now.date();
    let open: Vec<usize> = (0..book.items.len()).filter(|&i| is_open(&book.items[i])).collect();
    // (When each is due on the person's clock, worked out once: asked of
    // each pair a sort compares, it was most of what ordering five
    // thousand to-dos cost.)
    let keys: Vec<(Date, i64)> = match place {
        Place::Today | Place::Upcoming | Place::All => book.items.iter().map(|item| due_key(item, zone)).collect(),
        Place::Done | Place::List(_) => Vec::new(),
    };
    let dated = |i: usize| book.items[i].todo.due.is_some();
    let mut out = Vec::new();
    match place {
        Place::Today => {
            let mut due: Vec<usize> = open.into_iter().filter(|&i| dated(i) && keys[i].0 <= today).collect();
            due.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then_with(|| by_order(book, a, b)));
            let (late, on): (Vec<usize>, Vec<usize>) = due.into_iter().partition(|&i| keys[i].0 < today);
            if !late.is_empty() {
                out.push(Row::Heading(Heading::Overdue));
                out.extend(late.into_iter().map(Row::ToDo));
                if !on.is_empty() {
                    out.push(Row::Heading(Heading::Day(today)));
                }
            }
            out.extend(on.into_iter().map(Row::ToDo));
        }
        Place::Upcoming => {
            let mut later: Vec<usize> = open.into_iter().filter(|&i| dated(i) && keys[i].0 > today).collect();
            later.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then_with(|| by_order(book, a, b)));
            let mut day = None;
            for i in later {
                let d = Some(keys[i].0);
                if d != day {
                    day = d;
                    out.push(Row::Heading(Heading::Day(d.unwrap_or(today))));
                }
                out.push(Row::ToDo(i));
            }
        }
        Place::All => {
            for list in &book.lists {
                let mut mine: Vec<usize> = open.iter().copied().filter(|&i| book.items[i].list == list.id).collect();
                if mine.is_empty() {
                    continue;
                }
                let order = sections(book, &list.id);
                let place_of = |i: usize| book.items[i].todo.section.as_ref().and_then(|s| order.iter().position(|o| o == s)).map_or(0, |p| p + 1);
                // (By when they're due, those with no due after, in the
                // list's own order: its sections aren't said here, and in
                // their order a to-do due today under one due next week
                // read as a list unsorted.)
                mine.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then_with(|| place_of(a).cmp(&place_of(b))).then_with(|| by_order(book, a, b)));
                out.push(Row::Heading(Heading::List(list.id.clone())));
                out.extend(mine.into_iter().map(Row::ToDo));
            }
        }
        Place::Done => {
            let mut gone: Vec<usize> = (0..book.items.len()).filter(|&i| !is_open(&book.items[i])).collect();
            let when = |i: usize| book.items[i].todo.done_at.or(book.items[i].todo.modified);
            gone.sort_by(|&a, &b| when(b).cmp(&when(a)).then_with(|| by_order(book, a, b)));
            let mut day = None;
            for i in gone {
                let d = when(i).map(|t| t.to_zoned(zone.clone()).date());
                if d != day {
                    day = d;
                    out.push(Row::Heading(Heading::Day(d.unwrap_or(today))));
                }
                out.push(Row::ToDo(i));
            }
        }
        Place::List(id) => {
            for (section, items) in columns_where(book, id, is_open) {
                // (No heading over those in no section, first.)
                if section.is_some() {
                    out.push(Row::Heading(Heading::Section(section)));
                    if items.is_empty() {
                        out.push(Row::Nothing);
                    }
                }
                out.extend(items.into_iter().map(Row::ToDo));
            }
            // What's done, last, as on its board.
            let gone = done_of_where(book, id, is_open);
            if done && !gone.is_empty() {
                out.push(Row::Heading(Heading::Done));
                out.extend(gone.into_iter().map(Row::ToDo));
            }
        }
    }
    out
}

/// A list's done to-dos, the newest done first.
pub fn done_of(book: &Book, list: &str) -> Vec<usize> {
    done_of_where(book, list, &|item| item.todo.open())
}

/// [`done_of`], with `is_open` saying which are still to do ([`rows_where`]).
pub fn done_of_where(book: &Book, list: &str, is_open: &dyn Fn(&Item) -> bool) -> Vec<usize> {
    let mut gone: Vec<usize> = (0..book.items.len()).filter(|&i| book.items[i].list == list && !is_open(&book.items[i])).collect();
    let when = |i: usize| book.items[i].todo.done_at.or(book.items[i].todo.modified);
    gone.sort_by(|&a, &b| when(b).cmp(&when(a)).then_with(|| by_order(book, a, b)));
    gone
}

/// A list's board: its sections side by side, each with what it has to
/// do, in order. Those in no section come first, where there are any, or
/// where the list has no sections at all. (What's done is a lane of its
/// own: [`done_of`].)
pub fn columns(book: &Book, list: &str) -> Vec<(Option<String>, Vec<usize>)> {
    columns_where(book, list, &|item| item.todo.open())
}

/// [`columns`], with `is_open` saying which are still to do ([`rows_where`]).
pub fn columns_where(book: &Book, list: &str, is_open: &dyn Fn(&Item) -> bool) -> Vec<(Option<String>, Vec<usize>)> {
    let order = sections(book, list);
    let mine: Vec<usize> = (0..book.items.len()).filter(|&i| book.items[i].list == list && is_open(&book.items[i])).collect();
    let column = |s: Option<&String>| {
        let mut items: Vec<usize> = mine.iter().copied().filter(|&i| book.items[i].todo.section.as_ref() == s).collect();
        items.sort_by(|&a, &b| by_order(book, a, b));
        items
    };
    let mut out = Vec::new();
    let loose = column(None);
    if !loose.is_empty() || order.is_empty() {
        out.push((None, loose));
    }
    for s in &order {
        out.push((Some(s.clone()), column(Some(s))));
    }
    out
}

/// Where the to-do `uid` goes, put at `index` among the to-dos of a
/// section (`others`, in order, without it): the orders to write, its
/// own and, where there's no room between its neighbours, theirs again,
/// spaced out. (Orders are whole numbers 1024 apart, as they're made.)
pub fn place(book: &Book, others: &[usize], index: usize, uid: &str) -> Vec<(String, i64)> {
    const STEP: i64 = 1024;
    let order = |i: usize| book.items[i].todo.order;
    let index = index.min(others.len());
    let before = index.checked_sub(1).map(|i| order(others[i]));
    let after = others.get(index).map(|&i| order(i));
    let room = match (before, after) {
        (None, None) => Some(0),
        (None, Some(a)) => a.checked_sub(STEP),
        (Some(b), None) => b.checked_add(STEP),
        (Some(b), Some(a)) if a - b >= 2 => Some(b + (a - b) / 2),
        _ => None,
    };
    if let Some(o) = room {
        return vec![(uid.to_owned(), o)];
    }
    // No room: the section numbered again, this one in its place.
    let mut uids: Vec<String> = others.iter().map(|&i| book.items[i].todo.uid.clone()).collect();
    uids.insert(index, uid.to_owned());
    uids.into_iter().enumerate().map(|(n, u)| (u, (n as i64 + 1) * STEP)).filter(|(u, o)| u == uid || book.item(u).is_none_or(|i| i.todo.order != *o)).collect()
}

/// The order for a new to-do at the top of a section (`others`, in
/// order).
pub fn top(book: &Book, others: &[usize]) -> i64 {
    others.iter().map(|&i| book.items[i].todo.order).min().map_or(0, |o| o.saturating_sub(1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::{Status, ToDo},
        store::{Item, List},
    };
    use olo_theme::Hue;

    fn now() -> Zoned {
        jiff::civil::date(2026, 11, 19).at(10, 30, 0, 0).to_zoned(TimeZone::UTC).unwrap()
    }

    fn todo(list: &str, uid: &str, order: i64, section: Option<&str>, due: Option<When>) -> Item {
        let mut t = ToDo::new(uid, uid);
        t.order = order;
        t.section = section.map(str::to_owned);
        t.due = due;
        Item { list: list.into(), file: format!("/x/{list}/{uid}.ics").into(), changed: None, todo: t }
    }

    fn day(m: i8, d: i8) -> Option<When> {
        Some(When::Date(jiff::civil::date(2026, m, d)))
    }

    fn book() -> Book {
        let mut done = todo("home", "swept", 5, None, day(11, 18));
        done.todo.status = Status::Done;
        done.todo.done_at = crate::item::moment("20261118T200000Z");
        Book {
            lists: vec![
                List { id: "home".into(), name: "Home".into(), hue: Hue::Green, sections: vec!["Doing".into()] },
                List { id: "work".into(), name: "Work".into(), hue: Hue::Blue, sections: vec![] },
            ],
            items: vec![
                todo("home", "gate", 2048, None, day(11, 18)),
                todo("home", "paint", 1024, Some("Doing"), day(11, 21)),
                todo("work", "report", 0, None, Some(When::Zoned(jiff::civil::date(2026, 11, 19).at(15, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap()))),
                todo("work", "review", 1024, None, None),
                todo("home", "bins", 1024, None, day(11, 19)),
                done,
                todo("home", "shelf", 0, Some("Elsewhere"), None),
            ],
        }
    }

    /// What's done is filed under the day it was done on the person's own
    /// clock: late in the evening west of Greenwich is today, not the
    /// next day (the day UTC is on by then).
    #[test]
    fn done_is_filed_by_the_local_day() {
        let zone = TimeZone::get("America/Los_Angeles").expect("the zone");
        let evening = jiff::civil::date(2026, 11, 19).at(23, 0, 0, 0).to_zoned(zone).unwrap();
        let mut b = book();
        let bins = b.items.iter_mut().find(|i| i.todo.uid == "bins").expect("bins");
        bins.todo.status = Status::Done;
        bins.todo.done_at = Some(evening.timestamp());
        assert_eq!(evening.timestamp().to_zoned(TimeZone::UTC).date(), jiff::civil::date(2026, 11, 20), "the next day already, in UTC");
        let rows = rows(&b, &Place::Done, &evening, false);
        assert_eq!(uids(&b, &rows)[..2], ["# 2026-11-19", "bins"]);
    }

    fn uids(book: &Book, rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::ToDo(i) => book.items[*i].todo.uid.clone(),
                Row::Heading(Heading::Overdue) => "# overdue".into(),
                Row::Heading(Heading::Day(d)) => format!("# {d}"),
                Row::Heading(Heading::List(l)) => format!("# {l}"),
                Row::Heading(Heading::Section(s)) => format!("# {}", s.as_deref().unwrap_or("-")),
                Row::Heading(Heading::Done) => "# done".into(),
                Row::Nothing => "(nothing)".into(),
            })
            .collect()
    }

    #[test]
    fn each_place_in_order() {
        let b = book();
        let n = now();
        assert_eq!(uids(&b, &rows(&b, &Place::Today, &n, false)), ["# overdue", "gate", "# 2026-11-19", "bins", "report"]);
        assert_eq!(uids(&b, &rows(&b, &Place::Upcoming, &n, false)), ["# 2026-11-21", "paint"]);
        // All: each list's by when they're due (what's overdue first),
        // those with no due after, in the list's own order.
        assert_eq!(uids(&b, &rows(&b, &Place::All, &n, true)), ["# home", "gate", "bins", "paint", "shelf", "# work", "report", "review"]);
        assert_eq!(uids(&b, &rows(&b, &Place::Done, &n, false)), ["# 2026-11-18", "swept"]);
        // A list's own: those in no section, then each section (one its
        // to-dos name that it doesn't, last); then what's done, where
        // asked, as its board's last lane has it.
        assert_eq!(uids(&b, &rows(&b, &Place::List("home".into()), &n, false)), ["bins", "gate", "# Doing", "paint", "# Elsewhere", "shelf"]);
        assert_eq!(uids(&b, &rows(&b, &Place::List("home".into()), &n, true)), ["bins", "gate", "# Doing", "paint", "# Elsewhere", "shelf", "# done", "swept"]);
        assert_eq!(done_of(&b, "home").len(), 1);
        // A list of no sections: its board is one column.
        assert_eq!(columns(&b, "work").len(), 1);
        // A section with nothing to do says so under its heading.
        let mut empty = b.clone();
        empty.lists[0].sections.push("Later".into());
        assert_eq!(uids(&empty, &rows(&empty, &Place::List("home".into()), &n, false)), ["bins", "gate", "# Doing", "paint", "# Later", "(nothing)", "# Elsewhere", "shelf"]);
    }

    #[test]
    fn moved_between_its_neighbours() {
        let b = book();
        // home's loose ones: bins (1024), gate (2048).
        let loose: Vec<usize> = columns(&b, "home")[0].1.clone();
        assert_eq!(place(&b, &loose, 0, "new"), [("new".to_owned(), 0)]);
        assert_eq!(place(&b, &loose, 1, "new"), [("new".to_owned(), 1536)]);
        assert_eq!(place(&b, &loose, 2, "new"), [("new".to_owned(), 3072)]);
        assert_eq!(top(&b, &loose), 0);
        // No room between them: numbered again.
        let mut tight = b.clone();
        tight.items[0].todo.order = 1025;
        let loose: Vec<usize> = columns(&tight, "home")[0].1.clone();
        assert_eq!(place(&tight, &loose, 1, "new"), [("new".to_owned(), 2048), ("gate".to_owned(), 3072)]);
    }
}
