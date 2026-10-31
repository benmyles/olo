//! Times as the chat list and conversations show them, in the person's
//! time zone and the Deskbar clock's 12- or 24-hour style.

use chrono::{DateTime, Datelike, Local, TimeZone};

fn local(unix: i64) -> DateTime<Local> {
    Local.timestamp_opt(unix, 0).single().unwrap_or_else(Local::now)
}

/// "1:05 PM" or "13:05".
pub fn time(unix: i64, h24: bool) -> String {
    let t = local(unix);
    if h24 { t.format("%H:%M").to_string() } else { t.format("%-I:%M %p").to_string() }
}

/// A chat row's time: today's as a time, this week's as a day, this
/// year's as a date, else with the year.
pub fn short(unix: i64, now: i64, h24: bool) -> String {
    let (t, n) = (local(unix), local(now));
    let days = n.date_naive().signed_duration_since(t.date_naive()).num_days();
    match days {
        ..=0 => time(unix, h24),
        1..=6 => t.format("%a").to_string(),
        _ if t.year() == n.year() => t.format("%b %-d").to_string(),
        _ => t.format("%-m/%-d/%y").to_string(),
    }
}

/// A conversation's day heading: "Today", "Yesterday", "Monday",
/// "October 12", "October 12, 2025".
pub fn day(unix: i64, now: i64) -> String {
    let (t, n) = (local(unix), local(now));
    let days = n.date_naive().signed_duration_since(t.date_naive()).num_days();
    match days {
        ..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => t.format("%A").to_string(),
        _ if t.year() == n.year() => t.format("%B %-d").to_string(),
        _ => t.format("%B %-d, %Y").to_string(),
    }
}

/// The day `unix` falls on (to tell days apart).
pub fn day_number(unix: i64) -> i32 {
    local(unix).date_naive().num_days_from_ce()
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days() {
        let now = now();
        assert_eq!(day(now, now), "Today");
        assert_eq!(day(now - 86_400, now), "Yesterday");
        assert!(short(now, now, true).contains(':'));
        assert_eq!(short(now - 3 * 86_400, now, true).len(), 3);
    }
}
