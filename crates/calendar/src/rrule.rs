//! Repeating: a recurrence rule (RFC 5545's RRULE) read, written, and
//! turned into the starts it stands for.
//!
//! A rule is worked out on the clock of the event's own time zone ("every
//! Monday at 9" stays at 9 through a change of the clocks); what its
//! starts are as instants is the caller's to say ([`crate::event`]).
//! What is read: FREQ (DAILY, WEEKLY, MONTHLY, YEARLY), INTERVAL, COUNT,
//! UNTIL, BYDAY (with "the 2nd", "the last"), BYMONTHDAY (from the end
//! too), BYMONTH, BYSETPOS and WKST: what calendars' own editors make. A
//! rule with a part that isn't (BYWEEKNO, BYYEARDAY, the BYHOUR family,
//! SECONDLY to HOURLY) isn't guessed at: [`Rule::parse`] says no, and the
//! event shows once, at its start, its rule kept as written.

use jiff::civil::{Date, DateTime, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// Where a rule stops by date: a day (the whole of it), a local time, or
/// an instant (seconds since the epoch: `…Z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    Date(Date),
    Local(DateTime),
    Instant(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<Until>,
    /// Each with which one of its period, if said (2: the second; -1: the
    /// last).
    pub by_day: Vec<(Option<i8>, Weekday)>,
    /// Days of the month; negative, from its end (-1: the last).
    pub by_month_day: Vec<i8>,
    pub by_month: Vec<i8>,
    /// Which of a period's days, in order (1: the first; -1: the last).
    pub by_set_pos: Vec<i32>,
    pub week_start: Weekday,
}

fn weekday(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

fn weekday_code(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

/// "20261120" or "20261120T140000" (a trailing Z left to the caller).
pub fn civil(text: &str) -> Option<DateTime> {
    let text = text.trim().trim_end_matches('Z');
    let digits = |s: &str| s.len() == s.bytes().filter(u8::is_ascii_digit).count();
    let (date, time) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (text, None),
    };
    if date.len() != 8 || !digits(date) {
        return None;
    }
    let n = |s: &str| s.parse::<i16>().ok();
    let day = Date::new(n(&date[..4])?, n(&date[4..6])? as i8, n(&date[6..8])? as i8).ok()?;
    match time {
        None => Some(day.at(0, 0, 0, 0)),
        Some(t) if t.len() == 6 && digits(t) => Some(day.at(n(&t[..2])? as i8, n(&t[2..4])? as i8, (n(&t[4..6])? as i8).min(59), 0)),
        Some(_) => None,
    }
}

/// As [`civil`] reads them.
pub fn civil_text(at: DateTime, date_only: bool) -> String {
    if date_only {
        format!("{:04}{:02}{:02}", at.year(), at.month(), at.day())
    } else {
        format!("{:04}{:02}{:02}T{:02}{:02}{:02}", at.year(), at.month(), at.day(), at.hour(), at.minute(), at.second())
    }
}

impl Rule {
    pub fn every(freq: Freq) -> Rule {
        Rule { freq, interval: 1, count: None, until: None, by_day: Vec::new(), by_month_day: Vec::new(), by_month: Vec::new(), by_set_pos: Vec::new(), week_start: Weekday::Monday }
    }

    /// An RRULE's value read. None: not a rule, or one with a part this
    /// doesn't work out (see the module's words).
    pub fn parse(value: &str) -> Option<Rule> {
        let mut rule = Rule::every(Freq::Daily);
        let mut freq = None;
        for part in value.trim().split(';').filter(|p| !p.trim().is_empty()) {
            let (key, v) = part.split_once('=')?;
            let v = v.trim();
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.to_ascii_uppercase().as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    })
                }
                "INTERVAL" => rule.interval = v.parse().ok().filter(|n| *n >= 1)?,
                "COUNT" => rule.count = Some(v.parse().ok()?),
                "UNTIL" => {
                    let at = civil(v)?;
                    rule.until = Some(if v.ends_with('Z') || v.ends_with('z') {
                        Until::Instant(at.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp().as_second())
                    } else if v.contains('T') {
                        Until::Local(at)
                    } else {
                        Until::Date(at.date())
                    });
                }
                "BYDAY" => {
                    for d in v.split(',') {
                        let d = d.trim().to_ascii_uppercase();
                        if d.len() < 2 {
                            return None;
                        }
                        let (n, code) = d.split_at(d.len() - 2);
                        let nth = if n.is_empty() { None } else { Some(n.parse::<i8>().ok().filter(|n| *n != 0 && n.abs() <= 53)?) };
                        rule.by_day.push((nth, weekday(code)?));
                    }
                }
                "BYMONTHDAY" => {
                    for d in v.split(',') {
                        rule.by_month_day.push(d.trim().parse::<i8>().ok().filter(|n| *n != 0 && n.abs() <= 31)?);
                    }
                }
                "BYMONTH" => {
                    for m in v.split(',') {
                        rule.by_month.push(m.trim().parse::<i8>().ok().filter(|n| (1..=12).contains(n))?);
                    }
                }
                "BYSETPOS" => {
                    for p in v.split(',') {
                        rule.by_set_pos.push(p.trim().parse::<i32>().ok().filter(|n| *n != 0 && n.abs() <= 366)?);
                    }
                }
                "WKST" => rule.week_start = weekday(&v.to_ascii_uppercase())?,
                // (Parts this doesn't work out: not guessed at.)
                _ => return None,
            }
        }
        rule.freq = freq?;
        // ("The 2nd Monday" needs a month or a year to be the 2nd of.)
        if rule.by_day.iter().any(|(n, _)| n.is_some()) && matches!(rule.freq, Freq::Daily | Freq::Weekly) {
            return None;
        }
        Some(rule)
    }

    /// As an RRULE's value.
    pub fn write(&self) -> String {
        let mut out = format!(
            "FREQ={}",
            match self.freq {
                Freq::Daily => "DAILY",
                Freq::Weekly => "WEEKLY",
                Freq::Monthly => "MONTHLY",
                Freq::Yearly => "YEARLY",
            }
        );
        let list = |items: Vec<String>| items.join(",");
        if self.interval > 1 {
            out += &format!(";INTERVAL={}", self.interval);
        }
        if let Some(n) = self.count {
            out += &format!(";COUNT={n}");
        }
        match self.until {
            Some(Until::Date(d)) => out += &format!(";UNTIL={}", civil_text(d.at(0, 0, 0, 0), true)),
            Some(Until::Local(t)) => out += &format!(";UNTIL={}", civil_text(t, false)),
            Some(Until::Instant(s)) => {
                if let Ok(t) = jiff::Timestamp::from_second(s) {
                    out += &format!(";UNTIL={}Z", civil_text(t.to_zoned(jiff::tz::TimeZone::UTC).datetime(), false));
                }
            }
            None => {}
        }
        if !self.by_month.is_empty() {
            out += &format!(";BYMONTH={}", list(self.by_month.iter().map(i8::to_string).collect()));
        }
        if !self.by_month_day.is_empty() {
            out += &format!(";BYMONTHDAY={}", list(self.by_month_day.iter().map(i8::to_string).collect()));
        }
        if !self.by_day.is_empty() {
            out += &format!(";BYDAY={}", list(self.by_day.iter().map(|(n, d)| format!("{}{}", n.map(|n| n.to_string()).unwrap_or_default(), weekday_code(*d))).collect()));
        }
        if !self.by_set_pos.is_empty() {
            out += &format!(";BYSETPOS={}", list(self.by_set_pos.iter().map(i32::to_string).collect()));
        }
        if self.week_start != Weekday::Monday {
            out += &format!(";WKST={}", weekday_code(self.week_start));
        }
        out
    }

    /// The local starts the rule stands for, an event first starting at
    /// `start` (always the first of them), in order, each once, for as
    /// long as they're asked for (a rule with no end has none). `ended`:
    /// whether a start is past the rule's UNTIL, which only the caller
    /// can say of an instant.
    pub fn starts<'a>(&'a self, start: DateTime, ended: impl Fn(DateTime) -> bool + 'a) -> impl Iterator<Item = DateTime> + 'a {
        self.starts_from(start, start, ended)
    }

    /// [`Rule::starts`] for someone who wants only those from
    /// `not_before` on: `start` itself first, as ever, then from a period
    /// or so before `not_before` (not every week since the event began,
    /// to show this one: a weekly event of 2019 was four hundred steps
    /// to this week). A rule that counts its starts (COUNT) is counted
    /// from the first, as it must be.
    pub fn starts_from<'a>(&'a self, start: DateTime, not_before: DateTime, ended: impl Fn(DateTime) -> bool + 'a) -> impl Iterator<Item = DateTime> + 'a {
        let skipped = if self.count.is_some() || not_before <= start {
            0
        } else {
            let days = not_before.date().since(start.date()).map(|s| s.get_days() as i64).unwrap_or(0);
            let months = (not_before.year() as i64 * 12 + not_before.month() as i64) - (start.year() as i64 * 12 + start.month() as i64);
            let interval = self.interval.max(1) as i64;
            let periods = match self.freq {
                Freq::Daily => days / interval,
                Freq::Weekly => days / (7 * interval),
                Freq::Monthly => months / interval,
                Freq::Yearly => months / (12 * interval),
            };
            // (One before, so that the period holding `not_before` is whole.)
            u32::try_from((periods - 1).max(0)).unwrap_or(0)
        };
        let mut period = skipped;
        let mut pending: std::collections::VecDeque<DateTime> = std::collections::VecDeque::new();
        let mut given = 0u32;
        let mut first = true;
        let mut over = false;
        // (A rule that matches nothing, or nearly: not looked for for ever.)
        let mut barren = 0u32;
        std::iter::from_fn(move || {
            if over || self.count.is_some_and(|c| given >= c) {
                return None;
            }
            if std::mem::take(&mut first) {
                given += 1;
                return Some(start);
            }
            loop {
                if let Some(next) = pending.pop_front() {
                    if ended(next) {
                        over = true;
                        return None;
                    }
                    given += 1;
                    return Some(next);
                }
                if barren > 2000 {
                    over = true;
                    return None;
                }
                let mut days = self.period(start, period)?;
                period += 1;
                days.retain(|d| *d > start.date());
                barren = if days.is_empty() { barren + 1 } else { 0 };
                pending.extend(days.into_iter().map(|d| d.at(start.hour(), start.minute(), start.second(), 0)));
            }
        })
    }

    /// The days of the rule's `n`th period from `start`'s (its day, its
    /// week, its month, its year), in order. None: past what dates go to.
    fn period(&self, start: DateTime, n: u32) -> Option<Vec<Date>> {
        let step = (n as i64).checked_mul(self.interval as i64)?;
        let first = start.date();
        let month_ok = |d: &Date| self.by_month.is_empty() || self.by_month.contains(&d.month());
        let month_day_ok = |d: &Date| self.by_month_day.is_empty() || self.by_month_day.iter().any(|&m| nth_month_day(d.year(), d.month(), m) == Some(*d));
        let mut days: Vec<Date> = match self.freq {
            Freq::Daily => {
                let d = first.checked_add(jiff::Span::new().try_days(step).ok()?).ok()?;
                let day_ok = self.by_day.is_empty() || self.by_day.iter().any(|(_, w)| *w == d.weekday());
                if month_ok(&d) && month_day_ok(&d) && day_ok { vec![d] } else { Vec::new() }
            }
            Freq::Weekly => {
                // The week of `start`, begun on the rule's first day.
                let back = first.weekday().since(self.week_start) as i64;
                let week = first.checked_add(jiff::Span::new().try_days(step * 7 - back).ok()?).ok()?;
                let wanted: Vec<Weekday> = if self.by_day.is_empty() { vec![first.weekday()] } else { self.by_day.iter().map(|(_, w)| *w).collect() };
                (0..7).filter_map(|i| week.checked_add(jiff::Span::new().days(i)).ok()).filter(|d| wanted.contains(&d.weekday()) && month_ok(d)).collect()
            }
            Freq::Monthly => {
                let months = first.year() as i64 * 12 + first.month() as i64 - 1 + step;
                let (year, month) = (i16::try_from(months.div_euclid(12)).ok()?, months.rem_euclid(12) as i8 + 1);
                if !self.by_month.is_empty() && !self.by_month.contains(&month) { Vec::new() } else { self.month_days(year, month, first) }
            }
            Freq::Yearly => {
                let year = i16::try_from(first.year() as i64 + step).ok()?;
                let months: Vec<i8> = if self.by_month.is_empty() {
                    // ("The 20th Monday of the year": the year's own weekdays.)
                    if self.by_day.iter().any(|(n, _)| n.is_some()) {
                        return Some(self.pick(year_weekdays(year, &self.by_day)));
                    }
                    if self.by_day.is_empty() && self.by_month_day.is_empty() { vec![first.month()] } else { (1..=12).collect() }
                } else {
                    let mut m = self.by_month.clone();
                    m.sort_unstable();
                    m.dedup();
                    m
                };
                months.into_iter().flat_map(|m| self.month_days(year, m, first)).collect()
            }
        };
        days.sort_unstable();
        days.dedup();
        Some(if matches!(self.freq, Freq::Monthly | Freq::Yearly) { self.pick(days) } else { days })
    }

    /// The rule's days in one month: by its days of the month, by its
    /// weekdays there (the nth, or every one), both where both are said,
    /// or `first`'s own day of the month (none where the month hasn't it).
    fn month_days(&self, year: i16, month: i8, first: Date) -> Vec<Date> {
        let by_month_day: Vec<Date> = self.by_month_day.iter().filter_map(|&m| nth_month_day(year, month, m)).collect();
        if self.by_day.is_empty() {
            return if self.by_month_day.is_empty() { Date::new(year, month, first.day()).into_iter().collect() } else { by_month_day };
        }
        let Ok(start) = Date::new(year, month, 1) else { return Vec::new() };
        let all: Vec<Date> = (0..start.days_in_month()).filter_map(|i| start.checked_add(jiff::Span::new().days(i as i64)).ok()).collect();
        let mut days = Vec::new();
        for (nth, w) in &self.by_day {
            let of: Vec<Date> = all.iter().copied().filter(|d| d.weekday() == *w).collect();
            match nth {
                None => days.extend(of),
                Some(n) if *n > 0 => days.extend(of.get(*n as usize - 1)),
                Some(n) => days.extend(of.len().checked_sub(n.unsigned_abs() as usize).and_then(|i| of.get(i))),
            }
        }
        if !self.by_month_day.is_empty() {
            days.retain(|d| by_month_day.contains(d));
        }
        days
    }

    /// Of a period's days in order, those BYSETPOS picks (all, with none).
    fn pick(&self, mut days: Vec<Date>) -> Vec<Date> {
        days.sort_unstable();
        days.dedup();
        if self.by_set_pos.is_empty() {
            return days;
        }
        let n = days.len() as i32;
        let mut picked: Vec<Date> = self.by_set_pos.iter().filter_map(|&p| if p > 0 { days.get(p as usize - 1) } else { usize::try_from(n + p).ok().and_then(|i| days.get(i)) }).copied().collect();
        picked.sort_unstable();
        picked.dedup();
        picked
    }
}

/// The `n`th day of a month; negative, from its end. None where the
/// month hasn't it (the 31st of April).
fn nth_month_day(year: i16, month: i8, n: i8) -> Option<Date> {
    let first = Date::new(year, month, 1).ok()?;
    let day = if n > 0 { n } else { first.days_in_month() + 1 + n };
    Date::new(year, month, day).ok()
}

/// A year's weekdays by number ("the 20th Monday", "the last Friday").
fn year_weekdays(year: i16, by_day: &[(Option<i8>, Weekday)]) -> Vec<Date> {
    let Ok(start) = Date::new(year, 1, 1) else { return Vec::new() };
    let all: Vec<Date> = (0..start.days_in_year()).filter_map(|i| start.checked_add(jiff::Span::new().days(i as i64)).ok()).collect();
    let mut days = Vec::new();
    for (nth, w) in by_day {
        let of: Vec<Date> = all.iter().copied().filter(|d| d.weekday() == *w).collect();
        match nth {
            None => days.extend(of),
            Some(n) if *n > 0 => days.extend(of.get(*n as usize - 1)),
            Some(n) => days.extend(of.len().checked_sub(n.unsigned_abs() as usize).and_then(|i| of.get(i))),
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime {
        civil(text).unwrap()
    }

    /// The first `n` starts of `rule` from `start`, as days and times.
    fn first(rule: &str, start: &str, n: usize) -> Vec<String> {
        let rule = Rule::parse(rule).unwrap_or_else(|| panic!("{rule}"));
        rule.starts(at(start), |_| false).take(n).map(|t| civil_text(t, t.hour() == 0 && t.minute() == 0)).collect()
    }

    /// With the rule's own UNTIL, as a local time.
    fn all(rule: &str, start: &str) -> Vec<String> {
        let r = Rule::parse(rule).unwrap();
        let until = r.until;
        r.starts(at(start), move |t| match until {
            Some(Until::Date(d)) => t.date() > d,
            Some(Until::Local(u)) => t > u,
            _ => false,
        })
        .take(400)
        .map(|t| civil_text(t, true))
        .collect()
    }

    /// The examples of RFC 5545 (section 3.8.5.3), each with the dates it
    /// gives, on the clock of the event's own zone.
    #[test]
    fn the_rfcs_examples() {
        // Daily for 10 occurrences.
        assert_eq!(all("FREQ=DAILY;COUNT=10", "19970902T090000"), ["19970902", "19970903", "19970904", "19970905", "19970906", "19970907", "19970908", "19970909", "19970910", "19970911"]);
        // Every other day, for ever.
        assert_eq!(first("FREQ=DAILY;INTERVAL=2", "19970902T090000", 4), ["19970902T090000", "19970904T090000", "19970906T090000", "19970908T090000"]);
        // Every 10 days, 5 occurrences.
        assert_eq!(all("FREQ=DAILY;INTERVAL=10;COUNT=5", "19970902T090000"), ["19970902", "19970912", "19970922", "19971002", "19971012"]);
        // Weekly until a date.
        assert_eq!(all("FREQ=WEEKLY;UNTIL=19971007", "19970902T090000"), ["19970902", "19970909", "19970916", "19970923", "19970930", "19971007"]);
        // Every other week on Monday, Wednesday and Friday, the week begun on Sunday.
        assert_eq!(
            all("FREQ=WEEKLY;INTERVAL=2;UNTIL=19971001;WKST=SU;BYDAY=MO,WE,FR", "19970901T090000"),
            ["19970901", "19970903", "19970905", "19970915", "19970917", "19970919", "19970929", "19971001"]
        );
        // Every other week on Tuesday and Thursday, 8 occurrences.
        assert_eq!(all("FREQ=WEEKLY;INTERVAL=2;COUNT=8;WKST=SU;BYDAY=TU,TH", "19970902T090000"), ["19970902", "19970904", "19970916", "19970918", "19970930", "19971002", "19971014", "19971016"]);
        // Monthly on the first Friday, 10 occurrences.
        assert_eq!(all("FREQ=MONTHLY;COUNT=10;BYDAY=1FR", "19970905T090000"), ["19970905", "19971003", "19971107", "19971205", "19980102", "19980206", "19980306", "19980403", "19980501", "19980605"]);
        // Every other month on the first and last Sunday, 10 occurrences.
        assert_eq!(
            all("FREQ=MONTHLY;INTERVAL=2;COUNT=10;BYDAY=1SU,-1SU", "19970907T090000"),
            ["19970907", "19970928", "19971102", "19971130", "19980104", "19980125", "19980301", "19980329", "19980503", "19980531"]
        );
        // Monthly on the second-to-last Monday, 6 occurrences.
        assert_eq!(all("FREQ=MONTHLY;COUNT=6;BYDAY=-2MO", "19970922T090000"), ["19970922", "19971020", "19971117", "19971222", "19980119", "19980216"]);
        // Monthly on the third-to-last day.
        assert_eq!(first("FREQ=MONTHLY;BYMONTHDAY=-3", "19970928T090000", 6), ["19970928T090000", "19971029T090000", "19971128T090000", "19971229T090000", "19980129T090000", "19980226T090000"]);
        // Monthly on the 2nd and 15th, 10 occurrences.
        assert_eq!(
            all("FREQ=MONTHLY;COUNT=10;BYMONTHDAY=2,15", "19970902T090000"),
            ["19970902", "19970915", "19971002", "19971015", "19971102", "19971115", "19971202", "19971215", "19980102", "19980115"]
        );
        // Monthly on the first and last day, 10 occurrences.
        assert_eq!(
            all("FREQ=MONTHLY;COUNT=10;BYMONTHDAY=1,-1", "19970930T090000"),
            ["19970930", "19971001", "19971031", "19971101", "19971130", "19971201", "19971231", "19980101", "19980131", "19980201"]
        );
        // Every 18 months on the 10th to the 15th, 10 occurrences.
        assert_eq!(
            all("FREQ=MONTHLY;INTERVAL=18;COUNT=10;BYMONTHDAY=10,11,12,13,14,15", "19970910T090000"),
            ["19970910", "19970911", "19970912", "19970913", "19970914", "19970915", "19990310", "19990311", "19990312", "19990313"]
        );
        // Every Tuesday, every other month.
        assert_eq!(
            first("FREQ=MONTHLY;INTERVAL=2;BYDAY=TU", "19970902T090000", 7),
            ["19970902T090000", "19970909T090000", "19970916T090000", "19970923T090000", "19970930T090000", "19971104T090000", "19971111T090000"]
        );
        // Yearly in June and July, 10 occurrences.
        assert_eq!(
            all("FREQ=YEARLY;COUNT=10;BYMONTH=6,7", "19970610T090000"),
            ["19970610", "19970710", "19980610", "19980710", "19990610", "19990710", "20000610", "20000710", "20010610", "20010710"]
        );
        // Every other year in January, February and March, 10 occurrences.
        assert_eq!(
            all("FREQ=YEARLY;INTERVAL=2;COUNT=10;BYMONTH=1,2,3", "19970310T090000"),
            ["19970310", "19990110", "19990210", "19990310", "20010110", "20010210", "20010310", "20030110", "20030210", "20030310"]
        );
        // The 20th Monday of the year.
        assert_eq!(first("FREQ=YEARLY;BYDAY=20MO", "19970519T090000", 3), ["19970519T090000", "19980518T090000", "19990517T090000"]);
        // Every Thursday in March.
        assert_eq!(first("FREQ=YEARLY;BYMONTH=3;BYDAY=TH", "19970313T090000", 5), ["19970313T090000", "19970320T090000", "19970327T090000", "19980305T090000", "19980312T090000"]);
        // Friday the 13th.
        assert_eq!(first("FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13", "19970902T090000", 4), ["19970902T090000", "19980213T090000", "19980313T090000", "19981113T090000"]);
        // The first Saturday after the first Sunday of the month.
        assert_eq!(first("FREQ=MONTHLY;BYDAY=SA;BYMONTHDAY=7,8,9,10,11,12,13", "19970913T090000", 3), ["19970913T090000", "19971011T090000", "19971108T090000"]);
        // US presidential election day: every four years, the first Tuesday after a Monday in November.
        assert_eq!(first("FREQ=YEARLY;INTERVAL=4;BYMONTH=11;BYDAY=TU;BYMONTHDAY=2,3,4,5,6,7,8", "19961105T090000", 3), ["19961105T090000", "20001107T090000", "20041102T090000"]);
        // The third of Tuesday, Wednesday and Thursday of the month, 3 occurrences.
        assert_eq!(all("FREQ=MONTHLY;COUNT=3;BYDAY=TU,WE,TH;BYSETPOS=3", "19970904T090000"), ["19970904", "19971007", "19971106"]);
        // The second-to-last weekday of the month.
        assert_eq!(first("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-2", "19970929T090000", 4), ["19970929T090000", "19971030T090000", "19971127T090000", "19971230T090000"]);
        // The 15th and the 30th: no 30th of February.
        assert_eq!(all("FREQ=MONTHLY;BYMONTHDAY=15,30;COUNT=5", "20070115T090000"), ["20070115", "20070130", "20070215", "20070315", "20070330"]);
        // A week's first day matters: the two examples that differ only by it.
        assert_eq!(all("FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=MO", "19970805T090000"), ["19970805", "19970810", "19970819", "19970824"]);
        assert_eq!(all("FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=SU", "19970805T090000"), ["19970805", "19970817", "19970819", "19970831"]);
    }

    /// What calendars' editors make of the plain cases, and their corners.
    /// Begun near a later day, a rule gives the starts it gives from its
    /// first, less those before: the event's own start first, then
    /// everything from that day on, nothing missed at the join.
    #[test]
    fn begun_near_a_later_day_nothing_is_missed() {
        let start = civil("20190305T093000").unwrap();
        for rule in [
            "FREQ=DAILY",
            "FREQ=DAILY;INTERVAL=3",
            "FREQ=WEEKLY",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TU,FR",
            "FREQ=WEEKLY;BYDAY=SU;WKST=SU",
            "FREQ=MONTHLY",
            "FREQ=MONTHLY;BYDAY=-1FR",
            "FREQ=MONTHLY;INTERVAL=5;BYMONTHDAY=31",
            "FREQ=YEARLY",
            "FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29",
            "FREQ=YEARLY;INTERVAL=3;BYDAY=20MO",
        ] {
            let r = Rule::parse(rule).unwrap();
            for from in ["20190101T000000", "20190305T093000", "20190306T000000", "20240229T120000", "20261116T000000", "20261231T235959"] {
                let from = civil(from).unwrap();
                let until = from.checked_add(jiff::Span::new().days(800)).unwrap();
                let all: Vec<DateTime> = r.starts(start, |_| false).take_while(|d| *d < until).filter(|d| *d == start || *d >= from).collect();
                let near: Vec<DateTime> = r.starts_from(start, from, |_| false).take_while(|d| *d < until).filter(|d| *d == start || *d >= from).collect();
                assert_eq!(near, all, "{rule} from {from}");
            }
        }
        // One that counts its starts is counted from the first.
        let r = Rule::parse("FREQ=WEEKLY;COUNT=5").unwrap();
        assert_eq!(r.starts_from(start, civil("20190320T000000").unwrap(), |_| false).count(), 5);
    }

    #[test]
    fn plain_rules_and_their_corners() {
        // Monthly on the 31st: only the months that have one.
        assert_eq!(first("FREQ=MONTHLY", "20260131T100000", 4), ["20260131T100000", "20260331T100000", "20260531T100000", "20260731T100000"]);
        // Yearly on the 29th of February: leap years.
        assert_eq!(first("FREQ=YEARLY", "20240229", 3), ["20240229", "20280229", "20320229"]);
        // Weekdays.
        assert_eq!(first("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "20261119T090000", 4), ["20261119T090000", "20261120T090000", "20261123T090000", "20261124T090000"]);
        // A start the rule doesn't name is still the first.
        assert_eq!(first("FREQ=WEEKLY;BYDAY=MO", "20261118T090000", 3), ["20261118T090000", "20261123T090000", "20261130T090000"]);
        // COUNT counts the start; one occurrence is only the start.
        assert_eq!(all("FREQ=DAILY;COUNT=1", "20261118T090000"), ["20261118"]);
        // UNTIL before the second: only the start.
        assert_eq!(all("FREQ=DAILY;UNTIL=20261118", "20261118T090000"), ["20261118"]);
        // A rule that names nothing that exists ends, rather than looking for ever.
        assert_eq!(all("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30", "20260101T090000"), ["20260101"]);
    }

    #[test]
    fn rules_read_and_written() {
        for text in [
            "FREQ=DAILY",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR;WKST=SU",
            "FREQ=MONTHLY;COUNT=10;BYDAY=1FR,-1SU",
            "FREQ=YEARLY;UNTIL=20301231;BYMONTH=6,7;BYMONTHDAY=-1",
            "FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=-2",
            "FREQ=WEEKLY;UNTIL=20261231T225959Z",
            "FREQ=DAILY;UNTIL=20261231T120000",
        ] {
            let rule = Rule::parse(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(rule.write(), text);
            assert_eq!(Rule::parse(&rule.write()), Some(rule));
        }
        // Lower case, spaces, an empty part: read all the same.
        assert_eq!(Rule::parse("freq=weekly; byday=mo,we;").map(|r| r.write()).as_deref(), Some("FREQ=WEEKLY;BYDAY=MO,WE"));
        // Not rules, and rules with parts not worked out: said so.
        for text in
            ["", "BYDAY=MO", "FREQ=HOURLY", "FREQ=DAILY;BYHOUR=9,10", "FREQ=YEARLY;BYWEEKNO=20", "FREQ=WEEKLY;BYDAY=2MO", "FREQ=DAILY;INTERVAL=0", "FREQ=DAILY;BYMONTH=13", "FREQ=DAILY;NONSENSE"]
        {
            assert_eq!(Rule::parse(text), None, "{text}");
        }
    }
}
