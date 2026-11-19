//! Where occurrences go in the views, worked out apart from any drawing:
//! a month's weeks (bars across their days in lanes, single events under
//! them, what doesn't fit counted), and a day's hours (events side by
//! side where they overlap).

use jiff::civil::Date;

use crate::model::Shown;

/// Days from 1970-01-01.
pub fn day_number(d: Date) -> i32 {
    d.since(jiff::civil::date(1970, 1, 1)).map(|s| s.get_days()).unwrap_or(0)
}

pub fn day_of(number: i32) -> Date {
    jiff::civil::date(1970, 1, 1).checked_add(jiff::Span::new().days(number as i64)).unwrap_or(jiff::civil::date(1970, 1, 1))
}

/// The first day of the week holding `d`, where weeks start on
/// `week_start` (0 Monday … 6 Sunday).
pub fn week_of(d: Date, week_start: u8) -> Date {
    let back = (d.weekday().to_monday_zero_offset() as i32 - week_start as i32).rem_euclid(7);
    day_of(day_number(d) - back)
}

/// The first day a month's grid shows (the start of the week its 1st is
/// in), and how many weeks it takes.
pub fn month_grid(year: i16, month: i8, week_start: u8) -> (Date, usize) {
    let first = jiff::civil::date(year, month, 1);
    let start = week_of(first, week_start);
    let days = day_number(first) - day_number(start) + first.days_in_month() as i32;
    (start, (days as usize).div_ceil(7))
}

/// One piece of an occurrence in a row of days (a week of a month, the
/// whole-day strip over a week's hours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    /// Which occurrence (its place in what was laid out).
    pub item: usize,
    /// The columns it covers (from 0), both ends included.
    pub from: usize,
    pub to: usize,
    /// Which row under the day's number.
    pub lane: usize,
    /// It goes on from before the row's first day, and past its last.
    pub before: bool,
    pub after: bool,
}

/// A row of days laid out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub pieces: Vec<Piece>,
    /// For each day, how many didn't fit.
    pub more: Vec<usize>,
    /// How many rows it took (before any were left out).
    pub lanes: usize,
}

/// The `days` days from `start` with room for `lanes` rows a day: bars
/// first (the earlier, then the longer), each in the first row free
/// across its days; then the day's other events in the rows left
/// (`bars_only`: those are someone else's to draw, among the hours). A
/// day with more than fit keeps its last row for the count.
pub fn row(items: &[Shown], start: Date, days: usize, lanes: usize, bars_only: bool) -> Row {
    let days = days.clamp(1, 7);
    let (first, last) = (day_number(start), day_number(start) + days as i32 - 1);
    let mut taken: Vec<[bool; 7]> = Vec::new();
    let mut placed: Vec<Piece> = Vec::new();
    let mut order: Vec<(usize, i32, i32, bool)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let (a, b) = s.days();
            let (a, b) = (day_number(a), day_number(b));
            (a <= last && b >= first && (s.bar() || !bars_only)).then_some((i, a, b, s.bar()))
        })
        .collect();
    // Bars before single events; among bars the earlier, then the longer.
    order.sort_by_key(|&(i, a, b, bar)| (!bar, if bar { a.max(first) } else { 0 }, if bar { -(b - a) } else { 0 }, i));
    for (item, a, b, _) in order {
        let (from, to) = ((a.max(first) - first) as usize, (b.min(last) - first) as usize);
        let lane = (0..).find(|&l| taken.get(l).is_none_or(|row: &[bool; 7]| !row[from..=to].iter().any(|t| *t))).unwrap_or(0);
        if taken.len() <= lane {
            taken.resize(lane + 1, [false; 7]);
        }
        taken[lane][from..=to].fill(true);
        placed.push(Piece { item, from, to, lane, before: a < first, after: b > last });
    }
    // What fits: every row if the day's all do, else all but the last
    // (the count's).
    let mut week = Row { more: vec![0; days], lanes: taken.len(), ..Default::default() };
    let mut fits = [lanes; 7];
    for col in 0..days {
        let used = taken.iter().rposition(|row| row[col]).map_or(0, |l| l + 1);
        if used > lanes {
            fits[col] = lanes.saturating_sub(1);
        }
    }
    for p in placed {
        // (A bar shows where every day it covers has room for its row;
        // where one hasn't, it's counted on each day it's hidden from.)
        if (p.from..=p.to).all(|col| p.lane < fits[col]) {
            week.pieces.push(p);
        } else {
            for col in p.from..=p.to {
                week.more[col] += 1;
            }
        }
    }
    week.pieces.sort_by_key(|p| (p.lane, p.from));
    week
}

/// An event's place in a day's hours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub item: usize,
    /// Minutes from the day's midnight: where it starts and ends on this
    /// day (one from the day before starts at 0).
    pub from: f32,
    pub to: f32,
    /// Its share of the day's width: from `left` to `right` (0 to 1).
    pub left: f32,
    pub right: f32,
}

/// The shortest an event is drawn (minutes): room for its title.
pub const SHORTEST: f32 = 20.0;

/// The timed events on `day` side by side where they overlap: each group
/// of events that touch shares the width in columns, an event taking the
/// first column free, and widening over the columns to its right that
/// nothing it overlaps uses.
pub fn day(items: &[Shown], day: Date) -> Vec<Slot> {
    let midnight = day.at(0, 0, 0, 0);
    let minutes = |t: jiff::civil::DateTime| t.since(midnight).map(|s| s.total(jiff::Unit::Minute).unwrap_or(0.0) as f32).unwrap_or(0.0);
    let mut spans: Vec<(usize, f32, f32)> = items
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.bar())
        .filter_map(|(i, s)| {
            let (a, b) = (minutes(s.start).max(0.0), minutes(s.end).min(1440.0));
            // (As tall as it's drawn: what's under a short event's title
            // overlaps it too.)
            let b = b.max(a + SHORTEST).min(1440.0);
            (s.start.date() <= day && a < 1440.0 && (b > 0.0 && s.end.date() >= day || s.start.date() == day)).then_some((i, a.min(1440.0 - SHORTEST), b))
        })
        .collect();
    spans.sort_by(|x, y| x.1.total_cmp(&y.1).then(y.2.total_cmp(&x.2)));
    let mut out: Vec<Slot> = Vec::new();
    let mut group: Vec<(usize, f32, f32, usize)> = Vec::new();
    let mut group_end = 0.0f32;
    let close = |group: &mut Vec<(usize, f32, f32, usize)>, out: &mut Vec<Slot>| {
        let columns = group.iter().map(|g| g.3 + 1).max().unwrap_or(1);
        for &(item, a, b, col) in group.iter() {
            // Wider, over the columns to its right nothing it overlaps is in.
            let mut reach = col + 1;
            while reach < columns && !group.iter().any(|&(_, oa, ob, oc)| oc == reach && oa < b && ob > a) {
                reach += 1;
            }
            out.push(Slot { item, from: a, to: b, left: col as f32 / columns as f32, right: reach as f32 / columns as f32 });
        }
        group.clear();
    };
    for (item, a, b) in spans {
        if !group.is_empty() && a >= group_end {
            close(&mut group, &mut out);
        }
        let col = (0..).find(|&c| !group.iter().any(|&(_, _, ob, oc)| oc == c && ob > a)).unwrap_or(0);
        group_end = if group.is_empty() { b } else { group_end.max(b) };
        group.push((item, a, b, col));
    }
    close(&mut group, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{DateTime, date};
    use olo_theme::Hue;
    use std::sync::Arc;

    fn at(day: i8, h: i8, m: i8) -> DateTime {
        date(2026, 11, day).at(h, m, 0, 0)
    }

    fn timed(title: &str, start: DateTime, end: DateTime) -> Shown {
        Shown {
            cal: 0,
            href: Arc::from(title),
            title: Arc::from(title),
            location: Arc::from(""),
            start,
            end,
            all_day: false,
            hue: Hue::Blue,
            id: None,
            repeats: false,
            tentative: false,
            going: None,
            free: false,
        }
    }

    fn days(title: &str, first: i8, last: i8) -> Shown {
        Shown { all_day: true, ..timed(title, at(first, 0, 0), at(last + 1, 0, 0)) }
    }

    #[test]
    fn weeks_and_grids() {
        // November 2026 starts on a Sunday.
        assert_eq!(week_of(date(2026, 11, 18), 0), date(2026, 11, 16));
        assert_eq!(week_of(date(2026, 11, 18), 6), date(2026, 11, 15));
        assert_eq!(week_of(date(2026, 11, 16), 0), date(2026, 11, 16));
        assert_eq!(week_of(date(2026, 11, 15), 0), date(2026, 11, 9));
        assert_eq!(month_grid(2026, 11, 0), (date(2026, 10, 26), 6));
        assert_eq!(month_grid(2026, 11, 6), (date(2026, 11, 1), 5));
        assert_eq!(month_grid(2026, 2, 6), (date(2026, 2, 1), 4));
        assert_eq!(month_grid(2027, 2, 0), (date(2027, 2, 1), 4));
        assert_eq!(day_of(day_number(date(2028, 2, 29))), date(2028, 2, 29));
    }

    /// Bars lie across their days above the day's own events, the longer
    /// standing first; one that began before the week or goes on after
    /// says so; a day with more than fit keeps its last row to count
    /// them.
    #[test]
    fn a_week_of_a_month() {
        let items = vec![
            days("Conference", 17, 19), // Tue to Thu
            days("Away", 14, 24),       // from before the week to after it
            timed("Standup", at(17, 9, 0), at(17, 9, 30)),
            timed("Lunch", at(17, 12, 0), at(17, 13, 0)),
            timed("Review", at(17, 15, 0), at(17, 16, 0)),
            timed("Night shift", at(20, 22, 0), at(21, 6, 0)), // Fri into Sat: a bar
            timed("Dinner", at(20, 19, 0), at(21, 0, 0)),      // ends at midnight: Friday's alone
        ];
        let week = |items: &[Shown], start: Date, lanes: usize| row(items, start, 7, lanes, false);
        let w = week(&items, date(2026, 11, 16), 4);
        let said: Vec<String> =
            w.pieces.iter().map(|p| format!("{} {}-{} lane {}{}{}", items[p.item].title, p.from, p.to, p.lane, if p.before { " <" } else { "" }, if p.after { " >" } else { "" })).collect();
        assert_eq!(said, ["Away 0-6 lane 0 < >", "Conference 1-3 lane 1", "Night shift 4-5 lane 1", "Standup 1-1 lane 2", "Dinner 4-4 lane 2"]);
        // Tuesday has five things and four rows: three shown, two counted.
        assert_eq!(w.more, [0, 2, 0, 0, 0, 0, 0]);
        // With room for all, all are there.
        let w = week(&items, date(2026, 11, 16), 6);
        assert_eq!((w.pieces.len(), w.more), (7, vec![0; 7]));
        // The week after: only what goes on into it.
        let w = week(&items, date(2026, 11, 23), 4);
        assert_eq!(w.pieces, [Piece { item: 1, from: 0, to: 1, lane: 0, before: true, after: false }]);
        // No room at all: everything counted.
        assert_eq!(week(&items, date(2026, 11, 16), 0).more, [1, 5, 2, 2, 3, 2, 1]);
        // The whole-day strip over a day's hours: its bars alone, and how many rows they take.
        let strip = row(&items, date(2026, 11, 20), 1, 8, true);
        assert_eq!(strip.pieces.iter().map(|p| (&*items[p.item].title, p.from, p.to, p.before, p.after)).collect::<Vec<_>>(), [("Away", 0, 0, true, true), ("Night shift", 0, 0, false, true)]);
        assert_eq!((strip.lanes, row(&items, date(2026, 11, 12), 1, 8, true).lanes), (2, 0));
    }

    /// Events that overlap stand side by side; one with room to its right
    /// takes it; ones that don't touch each take the whole width.
    #[test]
    fn a_days_hours() {
        let items = vec![
            timed("A", at(18, 9, 0), at(18, 11, 0)),
            timed("B", at(18, 9, 30), at(18, 10, 30)),
            timed("C", at(18, 10, 45), at(18, 12, 0)),
            timed("D", at(18, 14, 0), at(18, 15, 0)),
            timed("Quick", at(18, 16, 0), at(18, 16, 5)),
            timed("After", at(18, 16, 10), at(18, 17, 0)),
            days("Holiday", 18, 18),
            timed("Overnight", at(17, 22, 0), at(18, 6, 0)), // a bar: not among the hours
            timed("Other day", at(19, 9, 0), at(19, 10, 0)),
        ];
        let slots = day(&items, date(2026, 11, 18));
        let said: Vec<String> = slots.iter().map(|s| format!("{} {}-{} {:.2}-{:.2}", items[s.item].title, s.from, s.to, s.left, s.right)).collect();
        assert_eq!(
            said,
            [
                "A 540-660 0.00-0.50",
                "B 570-630 0.50-1.00",
                // (C overlaps A, not B: beside A, where B was.)
                "C 645-720 0.50-1.00",
                "D 840-900 0.00-1.00",
                // A five-minute event is drawn twenty tall, so what follows at once stands beside it.
                "Quick 960-980 0.00-0.50",
                "After 970-1020 0.50-1.00",
            ]
        );
        assert!(day(&items, date(2026, 11, 20)).is_empty());
    }
}
