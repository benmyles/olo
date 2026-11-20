//! A to-do as it's typed on one line ("Call Ann tomorrow at 3pm"): its
//! title, and when it's due, from words at its end that say a day, a
//! time or both. Only at its end, and only words that are wholly a day or
//! a time: "Read Tomorrow and Tomorrow" stays as typed. As Things' and
//! Todoist's quick entry read a line; what was read is shown as it's
//! typed, so nothing is taken by surprise.
//!
//! The words are English, as Calendar's are (docs/TODO.md: days and
//! times in every language); numbers are anyone's ("11/21", "2026-11-21",
//! "15:00").

use jiff::{
    Zoned,
    civil::{Date, Time, Weekday},
};
use olo_calendar::event::When;

/// What a line typed says.
#[derive(Debug, Clone, PartialEq)]
pub struct Typed {
    pub title: String,
    pub due: Option<When>,
    /// The words read as the due ("tomorrow at 3pm"), as typed.
    pub said: Option<String>,
}

impl Typed {
    /// The to-do a line typed makes (`uid`: its own): its title, its
    /// due, and, where its words gave it a time, a reminder at that time
    /// ("Call Ann tomorrow at 3pm" is meant to be heard at 3, as
    /// Reminders has it; one due on a day alone reminds only if asked).
    pub fn to_do(&self, uid: &str) -> crate::ToDo {
        let mut todo = crate::ToDo::new(uid, &self.title);
        todo.due = self.due.clone();
        if todo.due.as_ref().is_some_and(|d| !d.is_date()) {
            todo.remind = Some(crate::Remind::Before(0));
        }
        todo
    }
}

/// A line typed, read on `now`'s clock; `month_first`: "3/4" is March
/// 4th (the person's region writes it so).
pub fn read(text: &str, now: &Zoned, month_first: bool) -> Typed {
    let words: Vec<&str> = text.split_whitespace().collect();
    let whole = |title: &[&str]| title.join(" ").trim_end_matches([',', ';', ':', '-', '–', '—']).trim().to_owned();
    // The longest run of words at the end that's wholly a day or a time,
    // with something before it to be the title.
    for start in 1..words.len() {
        let tail = &words[start..];
        if tail.len() > 7 {
            continue;
        }
        let Some(due) = phrase(tail, now, month_first) else { continue };
        // (A day joined to the words before it is the title's: "Tomorrow
        // and Tomorrow", "The Day After Tomorrow", "Ten of Wednesday".)
        let joined = words[start - 1].to_ascii_lowercase();
        if ["and", "or", "of", "the", "a", "an", "after", "before", "until", "till", "since", "from", "than", "with", "without", "&", "+"].contains(&joined.as_str()) {
            continue;
        }
        let mut title = &words[..start];
        // ("…by Friday", "…on the 3rd", "…due tomorrow": the little word
        // goes with the day.)
        while let Some((last, rest)) = title.split_last() {
            if rest.is_empty() || !["on", "at", "by", "due", "for", "this"].contains(&last.to_ascii_lowercase().as_str()) {
                break;
            }
            title = rest;
        }
        let title = whole(title);
        if title.is_empty() {
            continue;
        }
        return Typed { title, due: Some(due), said: Some(words[start..].join(" ")) };
    }
    Typed { title: whole(&words), due: None, said: None }
}

/// Which of a due's two fields can't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Day,
    Time,
}

/// A due typed in two fields: a day ("tomorrow", "Nov 21", "11/21",
/// "Friday") and a time ("3pm", "15:00"; empty: any time that day). No
/// due where both are empty; a time alone is today's, or tomorrow's
/// once it's gone.
pub fn fields(day: &str, time: &str, now: &Zoned, month_first: bool) -> Result<Option<When>, Field> {
    let (day, time) = (day.trim(), time.trim());
    let at = if time.is_empty() {
        None
    } else {
        let lower = time.to_ascii_lowercase();
        let words: Vec<&str> = lower.split_whitespace().filter(|w| *w != "at").collect();
        Some(time_of(&words, true).ok_or(Field::Time)?)
    };
    if day.is_empty() {
        return Ok(match at {
            None => None,
            Some(t) => {
                let today = now.date();
                let d = if today.to_datetime(t) > now.datetime() { today } else { add_days(today, 1) };
                Some(When::Zoned(d.to_datetime(t).to_zoned(now.time_zone().clone()).map_err(|_| Field::Time)?))
            }
        });
    }
    let words: Vec<&str> = day.split_whitespace().collect();
    let read = phrase(&words, now, month_first).ok_or(Field::Day)?;
    Ok(Some(match (read, at) {
        (When::Date(d), Some(t)) => When::Zoned(d.to_datetime(t).to_zoned(now.time_zone().clone()).map_err(|_| Field::Time)?),
        // (A time typed with the day, and none in its own field: the day's.)
        (other, _) => other,
    }))
}

fn weekday_named(word: &str) -> Option<Weekday> {
    Some(match word {
        "monday" | "mon" => Weekday::Monday,
        "tuesday" | "tue" | "tues" => Weekday::Tuesday,
        "wednesday" | "wed" => Weekday::Wednesday,
        "thursday" | "thu" | "thur" | "thurs" => Weekday::Thursday,
        "friday" | "fri" => Weekday::Friday,
        "saturday" | "sat" => Weekday::Saturday,
        "sunday" | "sun" => Weekday::Sunday,
        _ => return None,
    })
}

fn month_named(word: &str) -> Option<i8> {
    const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    let word = word.trim_end_matches('.');
    MONTHS.iter().position(|m| *m == word || (word.len() >= 3 && m.starts_with(word) && (word.len() == 3 || word == "sept"))).map(|i| i as i8 + 1)
}

fn number_named(word: &str) -> Option<i64> {
    const NUMBERS: [&str; 13] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
    match word {
        "a" | "an" => Some(1),
        _ => word.parse().ok().or_else(|| NUMBERS.iter().position(|n| *n == word).map(|i| i as i64)),
    }
}

/// "21", "21st", "2nd": a day of a month.
fn day_number(word: &str) -> Option<i8> {
    let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ',');
    let rest = &word[digits.len()..].trim_end_matches(',');
    if !(rest.is_empty() || ["st", "nd", "rd", "th"].contains(rest)) {
        return None;
    }
    digits.parse::<i8>().ok().filter(|d| (1..=31).contains(d))
}

/// The day `weekday` next falls on, from `today` (today itself if it's
/// that day).
fn coming(today: Date, weekday: Weekday) -> Date {
    let ahead = (weekday.to_monday_zero_offset() - today.weekday().to_monday_zero_offset()).rem_euclid(7);
    add_days(today, i64::from(ahead))
}

fn add_days(d: Date, n: i64) -> Date {
    d.checked_add(jiff::Span::new().days(n)).unwrap_or(d)
}

/// A day of a month, the next one on or after `today` where no year is
/// said.
fn month_day(today: Date, month: i8, day: i8, year: Option<i16>) -> Option<Date> {
    match year {
        Some(y) => Date::new(y, month, day).ok(),
        None => {
            let this = Date::new(today.year(), month, day).ok();
            match this {
                Some(d) if d >= today => Some(d),
                _ => Date::new(today.year() + 1, month, day).ok(),
            }
        }
    }
}

/// "3pm", "3:30", "15:00", "noon": a time (`bare`: a number alone is one,
/// after "at").
fn time_of(words: &[&str], bare: bool) -> Option<Time> {
    let joined = words.concat();
    match joined.as_str() {
        "noon" | "midday" => return Time::new(12, 0, 0, 0).ok(),
        "midnight" => return Time::new(23, 59, 0, 0).ok(),
        "morning" => return Time::new(9, 0, 0, 0).ok(),
        "afternoon" => return Time::new(15, 0, 0, 0).ok(),
        "evening" | "tonight" => return Time::new(18, 0, 0, 0).ok(),
        _ => {}
    }
    let (clock, half) = if let Some(c) = joined.strip_suffix("am").or_else(|| joined.strip_suffix("a.m.")).or_else(|| joined.strip_suffix('a')) {
        (c, Some(false))
    } else if let Some(c) = joined.strip_suffix("pm").or_else(|| joined.strip_suffix("p.m.")).or_else(|| joined.strip_suffix('p')) {
        (c, Some(true))
    } else {
        (joined.as_str(), None)
    };
    let (h, m) = match clock.split_once([':', '.']) {
        Some((h, m)) if m.len() == 2 => (h.parse::<i8>().ok()?, m.parse::<i8>().ok()?),
        None if half.is_some() || bare => (clock.parse::<i8>().ok()?, 0),
        _ => return None,
    };
    let h = match half {
        Some(pm) if (1..=12).contains(&h) => (h % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None => h,
    };
    Time::new(h, m, 0, 0).ok()
}

/// A run of words that's wholly a day, a time or both.
fn phrase(words: &[&str], now: &Zoned, month_first: bool) -> Option<When> {
    let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase().trim_end_matches(',').to_owned()).collect();
    let mut w: Vec<&str> = lower.iter().map(String::as_str).collect();
    let today = now.date();
    // ("on", "by", "at"… before it.)
    while w.first().is_some_and(|f| ["on", "by", "due", "at", "this"].contains(f)) && w.len() > 1 {
        w.remove(0);
    }
    // Its time, at the end: "at 3pm", "3:30 pm", "noon".
    let mut time = None;
    for take in (1..=w.len().min(3)).rev() {
        let (day, t) = w.split_at(w.len() - take);
        let at = day.last() == Some(&"at");
        if let Some(found) = time_of(t, at) {
            time = Some(found);
            w = day.to_vec();
            if w.last() == Some(&"at") {
                w.pop();
            }
            break;
        }
    }
    let day = match w.as_slice() {
        [] if time.is_some() => {
            // A time alone: today's, or tomorrow's once it's gone.
            let t = time?;
            if today.to_datetime(t) > now.datetime() { today } else { add_days(today, 1) }
        }
        [] => return None,
        ["today"] => today,
        ["tonight"] => {
            time = time.or_else(|| Time::new(18, 0, 0, 0).ok());
            today
        }
        ["tomorrow" | "tmrw" | "tmr"] => add_days(today, 1),
        ["tomorrow" | "tmrw" | "tmr", part @ ("morning" | "afternoon" | "evening" | "night")] => {
            time = time.or_else(|| time_of(&[if *part == "night" { "evening" } else { part }], false));
            add_days(today, 1)
        }
        [wd] if weekday_named(wd).is_some() => coming(today, weekday_named(wd)?),
        ["next", wd] if weekday_named(wd).is_some() => {
            // That day of next week (weeks from Monday), as Todoist reads it.
            let monday = add_days(today, 7 - i64::from(today.weekday().to_monday_zero_offset()));
            coming(monday, weekday_named(wd)?)
        }
        ["next", "week"] => add_days(today, 7 - i64::from(today.weekday().to_monday_zero_offset())),
        ["next", "month"] => today.first_of_month().checked_add(jiff::Span::new().months(1)).ok()?,
        ["weekend"] | ["the", "weekend"] => coming(today, Weekday::Saturday),
        ["in", n, unit] => {
            let n = number_named(n)?;
            match unit.trim_end_matches('s') {
                "day" => add_days(today, n),
                "week" => add_days(today, 7 * n),
                "month" => today.checked_add(jiff::Span::new().months(n)).ok()?,
                "hour" | "minute" | "min" if time.is_none() => {
                    let span = if unit.starts_with('h') { jiff::Span::new().hours(n) } else { jiff::Span::new().minutes(n) };
                    let at = now.checked_add(span).ok()?;
                    return Some(When::Zoned(at.round(jiff::Unit::Minute).ok()?));
                }
                _ => return None,
            }
        }
        // "Nov 21", "November 21st", "Nov 21 2027".
        [m, d] | [m, d, _] if month_named(m).is_some() && day_number(d).is_some() => {
            let year = if w.len() == 3 { Some(w[2].parse::<i16>().ok().filter(|y| (1970..=2200).contains(y))?) } else { None };
            month_day(today, month_named(m)?, day_number(d)?, year)?
        }
        // "21 Nov", "the 21st of November".
        [d, m] | ["the", d, "of", m] | [d, "of", m] if day_number(d).is_some() && month_named(m).is_some() => month_day(today, month_named(m)?, day_number(d)?, None)?,
        // "the 21st": this month's, or next month's once it's gone.
        ["the", d] if day_number(d).is_some() => {
            let d = day_number(d)?;
            let this = Date::new(today.year(), today.month(), d).ok().filter(|x| *x >= today);
            this.or_else(|| {
                let next = today.first_of_month().checked_add(jiff::Span::new().months(1)).ok()?;
                Date::new(next.year(), next.month(), d).ok()
            })?
        }
        [numeric] => numeric_date(numeric, today, month_first)?,
        _ => return None,
    };
    Some(match time {
        Some(t) => When::Zoned(day.to_datetime(t).to_zoned(now.time_zone().clone()).ok()?),
        None => When::Date(day),
    })
}

/// "2026-11-21", "11/21", "11/21/2026", "21/11" (where the day's first).
fn numeric_date(word: &str, today: Date, month_first: bool) -> Option<Date> {
    if let Ok(d) = word.parse::<Date>() {
        return Some(d);
    }
    let parts: Vec<&str> = word.split('/').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let (a, b) = (parts[0].parse::<i8>().ok()?, parts[1].parse::<i8>().ok()?);
    let (month, day) = if month_first { (a, b) } else { (b, a) };
    let year = match parts.get(2) {
        Some(y) if y.len() == 2 => Some(2000 + y.parse::<i16>().ok()?),
        Some(y) => Some(y.parse::<i16>().ok()?),
        None => None,
    };
    month_day(today, month, day, year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::TimeZone;

    /// Thursday, November 19, 2026, 10:30 AM.
    fn now() -> Zoned {
        jiff::civil::date(2026, 11, 19).at(10, 30, 0, 0).to_zoned(TimeZone::get("America/New_York").unwrap()).unwrap()
    }

    fn at(y: i16, m: i8, d: i8, h: i8, min: i8) -> Option<When> {
        Some(When::Zoned(jiff::civil::date(y, m, d).at(h, min, 0, 0).to_zoned(TimeZone::get("America/New_York").unwrap()).unwrap()))
    }

    fn day(y: i16, m: i8, d: i8) -> Option<When> {
        Some(When::Date(jiff::civil::date(y, m, d)))
    }

    fn due(text: &str) -> (String, Option<When>) {
        let t = read(text, &now(), true);
        (t.title, t.due)
    }

    #[test]
    fn days_and_times_at_the_end() {
        assert_eq!(due("Call Ann tomorrow at 3pm"), ("Call Ann".into(), at(2026, 11, 20, 15, 0)));
        assert_eq!(due("Call Ann tomorrow"), ("Call Ann".into(), day(2026, 11, 20)));
        assert_eq!(due("Pay the rent by Dec 1"), ("Pay the rent".into(), day(2026, 12, 1)));
        assert_eq!(due("Pay the rent on the 1st"), ("Pay the rent".into(), day(2026, 12, 1)));
        assert_eq!(due("Book flights Friday"), ("Book flights".into(), day(2026, 11, 20)));
        // Today's Thursday: Thursday is today; next Thursday, next week's.
        assert_eq!(due("Gym thu 6:30pm"), ("Gym".into(), at(2026, 11, 19, 18, 30)));
        assert_eq!(due("Gym next thursday"), ("Gym".into(), day(2026, 11, 26)));
        assert_eq!(due("Plan the sprint next week"), ("Plan the sprint".into(), day(2026, 11, 23)));
        assert_eq!(due("Renew passport in 3 weeks"), ("Renew passport".into(), day(2026, 12, 10)));
        assert_eq!(due("Stretch in 20 minutes"), ("Stretch".into(), at(2026, 11, 19, 10, 50)));
        assert_eq!(due("Report due 11/24 at 9:30"), ("Report".into(), at(2026, 11, 24, 9, 30)));
        assert_eq!(due("Taxes 2027-04-15"), ("Taxes".into(), day(2027, 4, 15)));
        assert_eq!(due("Dinner tonight"), ("Dinner".into(), at(2026, 11, 19, 18, 0)));
        assert_eq!(due("Bins tomorrow morning"), ("Bins".into(), at(2026, 11, 20, 9, 0)));
        // A month and day gone this year: next year's.
        assert_eq!(due("Dentist March 3"), ("Dentist".into(), day(2027, 3, 3)));
        // A time alone: today's, or tomorrow's once it's past.
        assert_eq!(due("Stand-up at 9am"), ("Stand-up".into(), at(2026, 11, 20, 9, 0)));
        assert_eq!(due("Lunch at noon"), ("Lunch".into(), at(2026, 11, 19, 12, 0)));
    }

    #[test]
    fn titles_kept_whole() {
        // Not wholly a day at the end, or nothing left for a title.
        for text in ["Read Tomorrow and Tomorrow", "Watch The Day After Tomorrow", "Buy 3 apples", "Tomorrow", "Watch Friday Night Lights", "Fix the 12 o'clock bug", "Email the may team"] {
            assert_eq!(read(text, &now(), true).due, None, "{text}");
            assert_eq!(read(text, &now(), true).title, text);
        }
        // Where the day comes first in the region.
        assert_eq!(read("Bake 24/11", &now(), false).due, day(2026, 11, 24));
        assert_eq!(read("Bake 24/11", &now(), true).due, None);
        let t = read("Call Ann  tomorrow at 3pm", &now(), true);
        assert_eq!(t.said.as_deref(), Some("tomorrow at 3pm"));
        // A time typed is a time to be reminded at; a day alone isn't.
        assert_eq!(t.to_do("a").remind, Some(crate::Remind::Before(0)));
        assert_eq!(read("Call Ann tomorrow", &now(), true).to_do("a").remind, None);
    }

    #[test]
    fn a_due_in_two_fields() {
        let f = |day: &str, time: &str| fields(day, time, &now(), true);
        assert_eq!(f("", ""), Ok(None));
        assert_eq!(f("Nov 21, 2026", ""), Ok(day(2026, 11, 21)));
        assert_eq!(f("Nov 21, 2026", "3 PM"), Ok(at(2026, 11, 21, 15, 0)));
        assert_eq!(f("tomorrow", "15:30"), Ok(at(2026, 11, 20, 15, 30)));
        assert_eq!(f("friday", "9"), Ok(at(2026, 11, 20, 9, 0)));
        assert_eq!(f("", "noon"), Ok(at(2026, 11, 19, 12, 0)));
        assert_eq!(f("someday", ""), Err(Field::Day));
        assert_eq!(f("tomorrow", "soon"), Err(Field::Time));
    }
}
