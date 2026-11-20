//! Days and times as To-Do says them: when a to-do is due ("Today",
//! "Tomorrow, 3:00 PM", "Fri, Nov 21"), its reminder, and its places'
//! headings. (The names of days and months are English, as Calendar's
//! are: docs/TODO.md, days and times in every language.)

use jiff::{
    Zoned,
    civil::{Date, Time},
};
use olo_calendar::event::When;
use olo_kit::tr;

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn weekday(d: Date) -> &'static str {
    WEEKDAYS[d.weekday().to_monday_zero_offset() as usize]
}

pub fn short_weekday(d: Date) -> &'static str {
    &weekday(d)[..3]
}

/// "Nov 28" ("Nov 28, 2027" in another year).
pub fn short_date(d: Date, today: Date) -> String {
    if d.year() == today.year() { format!("{} {}", short_month(d), d.day()) } else { format!("{} {}, {}", short_month(d), d.day(), d.year()) }
}

fn month(d: Date) -> &'static str {
    MONTHS[(d.month() - 1) as usize]
}

fn short_month(d: Date) -> &'static str {
    &month(d)[..3]
}

/// A day's name ("Saturday"), for a heading beside its number.
pub fn weekday_of(d: Date) -> &'static str {
    weekday(d)
}

/// "Thursday, November 19".
pub fn long_day(d: Date) -> String {
    format!("{}, {} {}", weekday(d), month(d), d.day())
}

/// "14:05", or "2:05 PM" ("2 PM" on the hour).
pub fn time(t: Time, h24: bool) -> String {
    if h24 {
        return format!("{:02}:{:02}", t.hour(), t.minute());
    }
    let (h, pm) = (t.hour(), t.hour() >= 12);
    let h12 = match h % 12 {
        0 => 12,
        h => h,
    };
    let half = if pm { "PM" } else { "AM" };
    if t.minute() == 0 { format!("{h12} {half}") } else { format!("{h12}:{:02} {half}", t.minute()) }
}

/// A day, said by name next to today, else by its date with its
/// weekday: "Today", "Tomorrow", "Yesterday", "Wed, Nov 25", "Nov 28,
/// 2027". (A weekday alone, "Wednesday", left which one to be worked
/// out.)
pub fn day(d: Date, today: Date) -> String {
    let days = (d - today).get_days();
    match days {
        0 => olo_kit::words::tr("Today").into(),
        1 => olo_kit::words::tr("Tomorrow").into(),
        -1 => olo_kit::words::tr("Yesterday").into(),
        _ if d.year() == today.year() => format!("{}, {} {}", short_weekday(d), short_month(d), d.day()),
        _ => format!("{} {}, {}", short_month(d), d.day(), d.year()),
    }
}

/// A day inside a sentence ("Done today", "Was due yesterday, 3 PM"):
/// as [`day`], its near days not capitals.
pub fn in_words(d: Date, today: Date) -> String {
    match (d - today).get_days() {
        0 => olo_kit::words::tr("today").into(),
        1 => olo_kit::words::tr("tomorrow").into(),
        -1 => olo_kit::words::tr("yesterday").into(),
        _ => day(d, today),
    }
}

/// When a to-do is due, inside a sentence: "today", "tomorrow, 3 PM",
/// "Fri, Nov 21".
pub fn due_in_words(due: &When, now: &Zoned, h24: bool) -> String {
    match due {
        When::Date(d) => in_words(*d, now.date()),
        other => {
            let at = other.moment(now.time_zone());
            tr!("{}, {}", in_words(at.date(), now.date()), time(at.time(), h24))
        }
    }
}

/// A day as a heading: "Today", "Tomorrow", "Friday, November 21",
/// "Saturday, January 2, 2027".
pub fn day_heading(d: Date, today: Date) -> String {
    match (d - today).get_days() {
        0 => olo_kit::words::tr("Today").into(),
        1 => olo_kit::words::tr("Tomorrow").into(),
        -1 => olo_kit::words::tr("Yesterday").into(),
        _ if d.year() == today.year() => format!("{}, {} {}", weekday(d), month(d), d.day()),
        _ => format!("{}, {} {}, {}", weekday(d), month(d), d.day(), d.year()),
    }
}

/// When a to-do is due, said short: "Today", "Tomorrow, 3 PM",
/// "Fri, Nov 21".
pub fn due(due: &When, now: &Zoned, h24: bool) -> String {
    match due {
        When::Date(d) => day(*d, now.date()),
        other => {
            let at = other.moment(now.time_zone());
            tr!("{}, {}", day(at.date(), now.date()), time(at.time(), h24))
        }
    }
}

/// A day as it's typed in a field: "Nov 21, 2026".
pub fn day_field(d: Date) -> String {
    format!("{} {}, {}", short_month(d), d.day(), d.year())
}

/// The reminders offered for a to-do due at a time: none, then seconds
/// before (what's kept), each with its words.
pub const BEFORE_A_TIME: [(i64, &str); 7] = [
    (0, "At the time it's due"),
    (5 * 60, "5 minutes before"),
    (15 * 60, "15 minutes before"),
    (30 * 60, "30 minutes before"),
    (3600, "1 hour before"),
    (2 * 3600, "2 hours before"),
    (86_400, "1 day before"),
];

/// …and for one due on a day: seconds before its start (negative: into
/// the day).
pub const BEFORE_A_DAY: [(i64, &str); 3] = [(-9 * 3600, "On the day, at 9 AM"), (15 * 3600, "The day before, at 9 AM"), (7 * 86_400 - 9 * 3600, "A week before, at 9 AM")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_near_and_far() {
        let today = jiff::civil::date(2026, 11, 19);
        let at = |m, d| jiff::civil::date(2026, m, d);
        assert_eq!(day(today, today), "Today");
        assert_eq!(day(at(11, 20), today), "Tomorrow");
        assert_eq!(day(at(11, 18), today), "Yesterday");
        assert_eq!(day(at(11, 22), today), "Sun, Nov 22");
        assert_eq!(day(at(11, 26), today), "Thu, Nov 26");
        assert_eq!(day(jiff::civil::date(2027, 1, 2), today), "Jan 2, 2027");
        assert_eq!(day_heading(at(11, 21), today), "Saturday, November 21");
        assert_eq!(time(Time::new(15, 0, 0, 0).unwrap(), false), "3 PM");
        assert_eq!(time(Time::new(0, 5, 0, 0).unwrap(), false), "12:05 AM");
        assert_eq!(time(Time::new(9, 30, 0, 0).unwrap(), true), "09:30");
        let now = today.at(10, 0, 0, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap();
        let three = When::Zoned(at(11, 20).at(15, 0, 0, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap());
        assert_eq!(due(&three, &now, false), "Tomorrow, 3 PM");
        assert_eq!(due_in_words(&three, &now, false), "tomorrow, 3 PM");
        assert_eq!((in_words(today, today), in_words(at(11, 22), today)), ("today".to_owned(), "Sun, Nov 22".to_owned()));
    }
}
