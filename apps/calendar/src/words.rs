//! Days and times in words, as the window says them.

use jiff::civil::{Date, DateTime, Time};

pub use olo_kit::widgets::month::{Day, MONTHS, WEEKDAYS};

pub fn day_of(d: Date) -> Day {
    Day::new(d.year() as i32, d.month() as u8, d.day() as u8)
}

pub fn date_of(d: Day) -> Date {
    jiff::civil::date(d.year as i16, d.month as i8, d.day as i8)
}

pub fn month_name(d: Date) -> &'static str {
    MONTHS[(d.month() as usize + 11) % 12]
}

pub fn weekday_name(d: Date) -> &'static str {
    WEEKDAYS[d.weekday().to_monday_zero_offset() as usize]
}

/// "November 2026".
pub fn month_year(d: Date) -> String {
    format!("{} {}", month_name(d), d.year())
}

/// "Wednesday, November 18, 2026".
pub fn long_day(d: Date) -> String {
    day_of(d).said()
}

/// A month's short name: "Nov".
fn short_month(d: Date) -> &'static str {
    &month_name(d)[..3]
}

/// "November 2026", "Nov 2026": longest first, for a place with less
/// room.
pub fn month_forms(d: Date) -> Vec<String> {
    vec![month_year(d), format!("{} {}", short_month(d), d.year())]
}

/// "Wednesday, November 18, 2026", "Wed, Nov 18, 2026", "Nov 18, 2026",
/// "Nov 18": longest first.
pub fn day_forms(d: Date) -> Vec<String> {
    vec![
        long_day(d),
        format!("{}, {} {}, {}", &weekday_name(d)[..3], short_month(d), d.day(), d.year()),
        format!("{} {}, {}", short_month(d), d.day(), d.year()),
        format!("{} {}", short_month(d), d.day()),
    ]
}

/// [`days_span`], then with short months, then without the year
/// ("Nov 16 – 22"): longest first.
pub fn span_forms(first: Date, last: Date) -> Vec<String> {
    let (a, b) = (short_month(first), short_month(last));
    let short = if first.year() != last.year() {
        format!("{a} {}, {} – {b} {}, {}", first.day(), first.year(), last.day(), last.year())
    } else if first.month() != last.month() {
        format!("{a} {} – {b} {}, {}", first.day(), last.day(), last.year())
    } else {
        format!("{a} {} – {}, {}", first.day(), last.day(), last.year())
    };
    let shortest = if first.month() != last.month() { format!("{a} {} – {b} {}", first.day(), last.day()) } else { format!("{a} {} – {}", first.day(), last.day()) };
    vec![days_span(first, last), short, shortest]
}

/// "November 16 – 22, 2026", "November 30 – December 6, 2026",
/// "December 28, 2026 – January 3, 2027".
pub fn days_span(first: Date, last: Date) -> String {
    if first.year() != last.year() {
        format!("{} {}, {} – {} {}, {}", month_name(first), first.day(), first.year(), month_name(last), last.day(), last.year())
    } else if first.month() != last.month() {
        format!("{} {} – {} {}, {}", month_name(first), first.day(), month_name(last), last.day(), last.year())
    } else {
        format!("{} {} – {}, {}", month_name(first), first.day(), last.day(), last.year())
    }
}

/// "14:05", or "2:05 PM" ("2 PM" on the hour).
pub fn time(t: Time, h24: bool) -> String {
    if h24 {
        return format!("{:02}:{:02}", t.hour(), t.minute());
    }
    let (h, half) = (if t.hour() % 12 == 0 { 12 } else { t.hour() % 12 }, if t.hour() < 12 { "AM" } else { "PM" });
    if t.minute() == 0 { format!("{h} {half}") } else { format!("{h}:{:02} {half}", t.minute()) }
}

/// An hour down the side of a day: "14:00", or "2 PM".
pub fn hour(h: i8, h24: bool) -> String {
    time(Time::constant(h.clamp(0, 23), 0, 0, 0), h24)
}

/// "14:00 – 15:30", for an event on one day; with the days where it's on
/// more than one, or all day.
pub fn when(start: DateTime, end: DateTime, all_day: bool, h24: bool) -> String {
    let short = |d: Date| format!("{} {} {}", &weekday_name(d)[..3], &month_name(d)[..3], d.day());
    if all_day {
        let last = end.date().yesterday().unwrap_or(end.date()).max(start.date());
        return if last == start.date() { olo_kit::tr!("{}, all day", short(last)) } else { format!("{} – {}", short(start.date()), short(last)) };
    }
    if start.date() == end.date() || (end.time() == Time::midnight() && end.date().yesterday().ok() == Some(start.date())) {
        format!("{} – {}", time(start.time(), h24), time(end.time(), h24))
    } else {
        format!("{} {} – {} {}", short(start.date()), time(start.time(), h24), short(end.date()), time(end.time(), h24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn said() {
        let d = date(2026, 11, 18);
        assert_eq!((month_year(d), long_day(d)), ("November 2026".to_owned(), "Wednesday, November 18, 2026".to_owned()));
        assert_eq!(date_of(day_of(d)), d);
        assert_eq!(days_span(date(2026, 11, 16), date(2026, 11, 22)), "November 16 – 22, 2026");
        assert_eq!(days_span(date(2026, 11, 30), date(2026, 12, 6)), "November 30 – December 6, 2026");
        assert_eq!(days_span(date(2026, 12, 28), date(2027, 1, 3)), "December 28, 2026 – January 3, 2027");
        let t = |h, m| Time::constant(h, m, 0, 0);
        assert_eq!(
            [time(t(0, 0), false), time(t(9, 5), false), time(t(12, 0), false), time(t(14, 30), false), time(t(14, 30), true), time(t(0, 5), true)],
            ["12 AM", "9:05 AM", "12 PM", "2:30 PM", "14:30", "00:05"]
        );
        assert_eq!((hour(14, true), hour(14, false)), ("14:00".to_owned(), "2 PM".to_owned()));
        assert_eq!(when(d.at(14, 0, 0, 0), d.at(15, 30, 0, 0), false, true), "14:00 – 15:30");
        assert_eq!(when(d.at(19, 0, 0, 0), date(2026, 11, 19).at(0, 0, 0, 0), false, true), "19:00 – 00:00");
        assert_eq!(when(d.at(22, 0, 0, 0), date(2026, 11, 19).at(6, 0, 0, 0), false, false), "Wed Nov 18 10 PM – Thu Nov 19 6 AM");
        assert_eq!(when(d.at(0, 0, 0, 0), date(2026, 11, 19).at(0, 0, 0, 0), true, true), "Wed Nov 18, all day");
        assert_eq!(when(d.at(0, 0, 0, 0), date(2026, 11, 21).at(0, 0, 0, 0), true, true), "Wed Nov 18 – Fri Nov 20");
    }
}
