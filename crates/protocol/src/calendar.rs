//! What's coming in Calendar, for whoever shows it without being
//! Calendar: Deskbar's clock's menu, agents (Ben, 2026-11-17: Calendar
//! "integrates with all the olo stuff").
//!
//! Calendar's service (`olo-calendar --service`, which the session keeps
//! running) writes the events to come to a small JSON file beside what
//! it keeps of the calendars (the cache directory: a test session's is
//! its own) whenever what it would say changes: the calendars
//! shown in Calendar, each occurrence not yet over for the next
//! [`DAYS`] days, in order, invitations declined and cancelled events
//! left out. Whoever wants them reads the file when asked (a click on
//! the clock): nobody asks Calendar anything, nothing reads a calendar
//! twice, and nothing runs on a timer for it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How many days ahead are written (today and the days after).
pub const DAYS: u32 = 14;
/// The most events written.
pub const MOST: usize = 200;

/// The events to come, as Calendar's service last wrote them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Coming {
    /// An account is set up (none: there's nothing to come, and
    /// whoever shows this needn't say "nothing today").
    pub set_up: bool,
    /// The first moment not looked at (seconds since 1970; the end of
    /// the last day): what's after it isn't here.
    pub until: i64,
    /// In order: by start, a whole day's events first on their day.
    pub events: Vec<Event>,
}

/// One occurrence of an event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    /// Its event's address, and the occurrence's start on the person's
    /// clock ("2026-11-18T14:00:00"): `olo-calendar --show HREF STARTS`
    /// opens Calendar on it.
    pub href: String,
    pub starts: String,
    pub title: String,
    pub location: String,
    /// Its calendar's name, and the colour Calendar draws it in.
    pub calendar: String,
    pub hue: Option<olo_theme::Hue>,
    /// When it starts and is over (seconds since 1970; a whole day's:
    /// its first midnight and the midnight after its last day).
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// When Calendar reminds of it (seconds since 1970), as far as it
    /// has worked that out (the next two days, and those already shown
    /// for an event not over): whoever else would remind of the same
    /// event then (the phone's own calendar, relayed here) needn't.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reminds: Vec<i64>,
}

impl Event {
    /// What opens Calendar on it.
    pub fn shows(&self) -> Vec<String> {
        vec!["olo-calendar".to_owned(), "--show".to_owned(), self.href.clone(), self.starts.clone()]
    }
}

impl Coming {
    /// Where it's kept.
    pub fn path() -> PathBuf {
        olo_xdg::cache_dir().join("calendar-coming.json")
    }

    /// As last written (None: never, or not by a Calendar this reads).
    pub fn read() -> Option<Coming> {
        serde_json::from_slice(&std::fs::read(Self::path()).ok()?).ok()
    }

    /// Those not over at `now`.
    pub fn from(&self, now: i64) -> impl Iterator<Item = &Event> {
        self.events.iter().filter(move |e| e.end > now)
    }

    /// Whether Calendar itself reminds, within `within` seconds of
    /// `now`, of an event called `title` (whatever its case, its spaces
    /// at the ends and the kind of its apostrophes): a reminder of it
    /// from somewhere else then says nothing new.
    pub fn reminds(&self, title: &str, now: i64, within: i64) -> bool {
        let plain = |s: &str| s.trim().to_lowercase().replace(['’', '‘'], "'");
        let title = plain(title);
        !title.is_empty() && self.events.iter().any(|e| e.reminds.iter().any(|at| (at - now).abs() <= within) && plain(&e.title) == title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calendar reminds of an event by its title, near the moment it
    /// does: whatever the title's case, the spaces at its ends and the
    /// kind of its apostrophes; not at another time, of another event,
    /// or of one with no reminder.
    #[test]
    fn reminded_of_here() {
        let event = |title: &str, reminds: &[i64]| Event { title: title.into(), start: 10_000, end: 13_600, reminds: reminds.to_vec(), ..Event::default() };
        let coming = Coming { set_up: true, until: 99_999, events: vec![event("Sam’s birthday", &[9_100, 6_400]), event("Standup", &[])] };
        assert!(coming.reminds(" sam's Birthday ", 9_100, 120) && coming.reminds("Sam’s birthday", 9_215, 120) && coming.reminds("Sam's birthday", 6_300, 120));
        assert!(!coming.reminds("Sam's birthday", 9_400, 120) && !coming.reminds("Standup", 9_100, 120) && !coming.reminds("", 9_100, 120) && !coming.reminds("Dentist", 9_100, 120));
        // (A file from before reminders were written reads, with none.)
        let old: Coming = serde_json::from_str(r#"{"set_up":true,"until":5,"events":[{"href":"h","starts":"s","title":"T","start":1,"end":2}]}"#).unwrap();
        assert!(old.events[0].reminds.is_empty() && !old.reminds("T", 1, 120));
    }
}
