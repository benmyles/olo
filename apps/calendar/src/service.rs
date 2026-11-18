//! Reminders: `olo-calendar --service`, which the session keeps running
//! (with no window). It keeps the calendars in step with their servers
//! while the window is shut, and shows each reminder at its time as a
//! notification: the event's title, when ("In 15 minutes · 2:00 PM –
//! 3:30 PM"), where, its calendar; Show opens Calendar on it.
//!
//! It sleeps until the next reminder is due or the servers are to be
//! asked again, on the wall's clock (`olo_kit::clock::Alarm`: through a
//! sleep of the computer, and a clock set), and is woken when the
//! accounts, the person's choices or what's kept change. With no
//! account it waits for one, asking nothing.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use jiff::{Zoned, tz::TimeZone};
use olo_calendar::event;
use olo_kit::{
    clock::Alarm,
    notify::{Note, NoteEvent, NoteId, NoteKind, Notifier},
};

use crate::{
    Settings,
    model::{Book, Choices},
    sync, words,
};

/// How often the servers are asked what changed (seconds).
const SYNC_EVERY: i64 = 15 * 60;
/// How far ahead reminders are worked out (they're worked out again at
/// every waking: this only bounds one look).
const AHEAD_DAYS: i64 = 2;
/// A reminder missed (the computer off or asleep) is still shown while
/// its event isn't over, unless the event began this long ago (seconds).
const STALE: i64 = 12 * 3600;

/// A reminder: when it's due, and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    /// When it shows (seconds since 1970).
    pub at: i64,
    /// What tells it from every other (its event, which occurrence,
    /// how long before): shown once.
    pub key: String,
    /// Its event's address.
    pub href: String,
    pub title: String,
    pub calendar: String,
    pub hue: olo_theme::Hue,
    /// The occurrence's own start and end (seconds since 1970), and on
    /// the person's clock.
    pub start: i64,
    pub end: i64,
    pub starts: jiff::civil::DateTime,
    pub ends: jiff::civil::DateTime,
    pub all_day: bool,
    pub location: String,
}

/// The reminders of the calendars shown that fall due from `from` for
/// `AHEAD_DAYS` days, soonest first.
pub fn reminders(book: &Book, choices: &Choices, from: &Zoned, zone: &TimeZone) -> Vec<Reminder> {
    let Ok(to) = from.checked_add(jiff::Span::new().days(AHEAD_DAYS)) else { return Vec::new() };
    let (first, last) = (from.timestamp().as_second(), to.timestamp().as_second());
    let mut out = Vec::new();
    for (i, cal) in book.cals.iter().enumerate() {
        if choices.hidden.contains(&cal.kept.href) {
            continue;
        }
        for stored in &cal.events {
            // (A reminder can be days before its event: looked for that much further on.)
            let longest = stored.event.alarms.iter().chain(stored.overrides.iter().flat_map(|o| &o.alarms)).copied().max().unwrap_or(-1);
            if longest < 0 && stored.event.alarms.is_empty() && stored.overrides.iter().all(|o| o.alarms.is_empty()) {
                continue;
            }
            let Ok(until) = to.checked_add(jiff::Span::new().seconds(longest.max(0))) else { continue };
            for o in event::occurrences(&stored.event, &stored.overrides, from, &until, zone) {
                let e = o.changed.and_then(|i| stored.overrides.get(i)).unwrap_or(&stored.event);
                if e.status == event::Status::Cancelled {
                    continue;
                }
                let (start, end) = (o.start.timestamp().as_second(), o.end.timestamp().as_second());
                for &before in &e.alarms {
                    let at = start - before;
                    // (Due in the span; or due already for something not
                    // yet over, nor long under way.)
                    if at > last || end <= first || first - start > STALE {
                        continue;
                    }
                    out.push(Reminder {
                        at,
                        key: format!("{}|{start}|{before}", stored.href),
                        href: stored.href.clone(),
                        title: if e.summary.trim().is_empty() { "(No title)".to_owned() } else { e.summary.trim().to_owned() },
                        calendar: cal.kept.name.clone(),
                        hue: book.hue(i, choices),
                        start,
                        end,
                        starts: o.start.datetime(),
                        ends: o.end.datetime(),
                        all_day: o.all_day,
                        location: e.location.trim().to_owned(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (a.at, &a.key).cmp(&(b.at, &b.key)));
    out
}

/// How far off `start` is at `now`, in words: "Now", "In 15 minutes",
/// "In 2 hours", "Tomorrow", "In 3 days"; "Started 5 minutes ago".
pub fn ahead(start: i64, now: i64) -> String {
    let s = start - now;
    let count = |n: i64, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    match s {
        s if s <= -3600 => format!("Started {} ago", count((-s + 1800) / 3600, "hour", "hours")),
        s if s <= -60 => format!("Started {} ago", count((-s + 30) / 60, "minute", "minutes")),
        s if s < 60 => "Now".to_owned(),
        s if s < 3600 => format!("In {}", count((s + 30) / 60, "minute", "minutes")),
        s if s < 20 * 3600 => format!("In {}", count((s + 1800) / 3600, "hour", "hours")),
        s if s < 36 * 3600 => "Tomorrow".to_owned(),
        s => format!("In {}", count((s + 43_200) / 86_400, "day", "days")),
    }
}

/// What a reminder says under its title.
pub fn said(r: &Reminder, now: i64, h24: bool) -> String {
    // (Each part kept whole where the card wraps: "1:08 PM – 1:38 PM"
    // isn't broken after its first 1.)
    let whole = |s: String| s.replace(' ', "\u{a0}");
    let mut parts = vec![whole(ahead(r.start, now)), whole(words::when(r.starts, r.ends, r.all_day, h24))];
    if !r.location.is_empty() {
        parts.push(r.location.clone());
    }
    parts.join(" · ")
}

fn fired_path() -> std::path::PathBuf {
    olo_xdg::state_dir().join("calendar").join("reminded.json")
}

/// `olo-calendar --service`.
pub fn run() -> std::process::ExitCode {
    let alarm = match Alarm::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("olo-calendar: no timer to remind with: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Woken when the accounts, the person's choices or what's kept change.
    let waker = alarm.waker();
    let mut followed = vec![olo_calendar::account::accounts_path(), crate::settings_path()];
    followed.extend(sync::store_dir());
    let _watch = olo_kit::watch::watch(followed, move |_| waker.wake());
    let h24 = Arc::new(AtomicBool::new(true));
    {
        let (h24, waker) = (h24.clone(), alarm.waker());
        olo_kit::settings::watch(move |s| {
            if h24.swap(s.clock.twenty_four_hour, Ordering::Relaxed) != s.clock.twenty_four_hour {
                waker.wake();
            }
        });
    }
    // Which reminder each notification is of (to open Calendar on it).
    let showing: Arc<Mutex<HashMap<NoteId, Vec<String>>>> = Arc::default();
    let notes = {
        let showing = showing.clone();
        Notifier::with_events("Calendar", "olo-calendar", move |e| match e {
            NoteEvent::Action { id, key, token } if key == "default" || key == "show" => {
                let Some(which) = showing.lock().unwrap().get(&id).cloned() else { return };
                let Ok(exe) = std::env::current_exe() else { return };
                let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
                let mut cmd = std::process::Command::new(exe);
                cmd.args(&which).stdin(std::process::Stdio::null());
                if let Some(token) = token {
                    cmd.env("XDG_ACTIVATION_TOKEN", token);
                }
                let _ = cmd.spawn();
            }
            NoteEvent::Closed { id } => {
                showing.lock().unwrap().remove(&id);
            }
            _ => {}
        })
    };
    // The reminders shown already (by key, with when their event ends):
    // not shown again after a restart.
    let mut fired: BTreeMap<String, i64> = olo_kit::store::load(&fired_path()).unwrap_or_default();
    let mut last_sync = 0i64;
    let demo = sync::demo();
    // (Held from one waking to the next only for the demo, whose
    // calendars are nowhere but on its server: the others are read from
    // disk at each waking and let go, a service that mostly sleeps
    // holding nothing.)
    let mut held = Book::default();
    loop {
        let zone = TimeZone::system();
        let now = Zoned::now().with_time_zone(zone.clone());
        let unix = now.timestamp().as_second();
        let (sources, kept) = sync::load(now.date());
        let order: Vec<String> = sources.iter().map(|s| s.account.id()).collect();
        let mut book = if demo { std::mem::take(&mut held) } else { Book::new(kept.into_iter().map(Arc::new).collect(), &order) };
        if !sources.is_empty() && unix - last_sync >= SYNC_EVERY {
            last_sync = unix;
            for source in &sources {
                let id = source.account.id();
                let known = book.cals.iter().filter(|c| c.kept.account == id).map(|c| c.kept.clone()).collect();
                let (cals, result) = sync::sync(source, known);
                if let Err(e) = result {
                    tracing::warn!("calendar: {id}: {e}");
                }
                if let Some(cals) = cals {
                    book = book.with(&id, cals, &order);
                }
            }
        }
        let settings: Settings = if demo { Settings::default() } else { olo_kit::store::load(&crate::settings_path()).unwrap_or_default() };
        let list = reminders(&book, &settings.choices, &now, &zone);
        let before = fired.len();
        for r in list.iter().filter(|r| r.at <= unix && unix < r.end) {
            if fired.contains_key(&r.key) {
                continue;
            }
            fired.insert(r.key.clone(), r.end);
            let fill = olo_kit::theme::palette().hue(r.hue);
            let which = vec!["--show".to_owned(), r.href.clone(), r.starts.to_string()];
            let mut note = Note::new(r.title.clone())
                .body(said(r, unix, h24.load(Ordering::Relaxed)))
                .icon("calendar")
                .kind(NoteKind::Important)
                .accent([fill[0], fill[1], fill[2]])
                .action("show", "Show")
                .shows(which.clone())
                .sticky(true);
            note.look.subtitle = Some(r.calendar.clone());
            let id = notes.show(None, note);
            showing.lock().unwrap().insert(id, which);
        }
        // (Those long over are forgotten.)
        fired.retain(|_, end| *end > unix - 86_400);
        if fired.len() != before && !demo {
            olo_kit::store::save(fired_path(), &fired);
        }
        let next = list.iter().filter(|r| r.at > unix).map(|r| r.at).min();
        // (With nothing due in the days looked at, looked at again before
        // they're out.)
        let again = (!book.cals.is_empty()).then_some(unix + (AHEAD_DAYS - 1) * 86_400);
        let asking = (!sources.is_empty()).then_some(last_sync + SYNC_EVERY);
        if demo {
            held = book;
        }
        drop(list);
        alarm.wait([next, again, asking].into_iter().flatten().min());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Cal;
    use olo_calendar::store::{Item, Kept};

    fn book_of(events: &[&str]) -> Book {
        let mut k = Kept { account: "a".into(), href: "https://x/work/".into(), name: "Work".into(), color: Some("#4285F4".into()), ..Default::default() };
        for (i, body) in events.iter().enumerate() {
            let ics = format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:e{i}\n{body}\nEND:VEVENT\nEND:VCALENDAR\n");
            k.items.insert(format!("https://x/work/{i}.ics"), Item { etag: "e".into(), ics });
        }
        Book::new(vec![Arc::new(Cal::read(k))], &["a".to_owned()])
    }

    fn plain(s: String) -> String {
        s.replace('\u{a0}', " ")
    }

    fn alarm(before: &str) -> String {
        format!("BEGIN:VALARM\nACTION:DISPLAY\nTRIGGER:-{before}\nEND:VALARM")
    }

    /// What falls due: each reminder of each occurrence, at its time,
    /// soonest first; one due already for a meeting not yet over; none
    /// for an event with no reminder, a hidden calendar's, or what's past.
    #[test]
    fn what_falls_due() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let now = jiff::civil::date(2026, 11, 18).at(10, 30, 0, 0).to_zoned(zone.clone()).unwrap();
        let book = book_of(&[
            &format!("DTSTART;TZID=Europe/Amsterdam:20261118T140000\nDTEND;TZID=Europe/Amsterdam:20261118T153000\nSUMMARY:Design review\nLOCATION:Room 2\n{}\n{}", alarm("PT15M"), alarm("PT1H")),
            &format!("DTSTART;TZID=Europe/Amsterdam:20261116T093000\nDTEND;TZID=Europe/Amsterdam:20261116T094500\nRRULE:FREQ=DAILY\nSUMMARY:Standup\n{}", alarm("PT5M")),
            "DTSTART;TZID=Europe/Amsterdam:20261118T120000\nDTEND;TZID=Europe/Amsterdam:20261118T130000\nSUMMARY:Lunch",
            &format!("DTSTART;TZID=Europe/Amsterdam:20261118T100000\nDTEND;TZID=Europe/Amsterdam:20261118T110000\nSUMMARY:Under way\n{}", alarm("PT10M")),
            &format!("DTSTART;TZID=Europe/Amsterdam:20261118T080000\nDTEND;TZID=Europe/Amsterdam:20261118T090000\nSUMMARY:Over\n{}", alarm("PT10M")),
            &format!("DTSTART;VALUE=DATE:20261120\nDTEND;VALUE=DATE:20261121\nSUMMARY:Release\n{}", alarm("P1D")),
        ]);
        let choices = Choices::default();
        let list = reminders(&book, &choices, &now, &zone);
        let said_: Vec<String> = list.iter().map(|r| format!("{} at {}", r.title, jiff::Timestamp::from_second(r.at).unwrap().to_zoned(zone.clone()).strftime("%d %H:%M"))).collect();
        assert_eq!(said_, ["Under way at 18 09:50", "Design review at 18 13:00", "Design review at 18 13:45", "Release at 19 00:00", "Standup at 19 09:25", "Standup at 20 09:25"]);
        // Each its own, the same from one look to the next.
        let keys: std::collections::BTreeSet<&str> = list.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys.len(), list.len());
        assert_eq!(reminders(&book, &choices, &now, &zone), list);
        // What they say, at the time they show.
        let review = &list[2];
        assert_eq!(plain(said(review, review.at, true)), "In 15 minutes · 14:00 – 15:30 · Room 2");
        assert_eq!(plain(said(&list[1], list[1].at, false)), "In 1 hour · 2 PM – 3:30 PM · Room 2");
        assert_eq!(plain(said(&list[0], now.timestamp().as_second(), true)), "Started 30 minutes ago · 10:00 – 11:00");
        // Long under way (a week away, begun days ago): not brought up again.
        let away = book_of(&[&format!("DTSTART;VALUE=DATE:20261116\nDTEND;VALUE=DATE:20261123\nSUMMARY:Away\n{}", alarm("P1D"))]);
        assert!(reminders(&away, &choices, &now, &zone).is_empty());
        assert_eq!(plain(said(&list[3], list[3].at, true)), "Tomorrow · Fri Nov 20, all day");
        // A hidden calendar reminds of nothing.
        let mut hidden = Choices::default();
        hidden.hidden.insert("https://x/work/".into());
        assert!(reminders(&book, &hidden, &now, &zone).is_empty());
    }

    #[test]
    fn how_far_off() {
        for (s, want) in [
            (0, "Now"),
            (59, "Now"),
            (60, "In 1 minute"),
            (15 * 60, "In 15 minutes"),
            (3600, "In 1 hour"),
            (5400, "In 2 hours"),
            (19 * 3600, "In 19 hours"),
            (24 * 3600, "Tomorrow"),
            (3 * 86_400, "In 3 days"),
            (-30 * 60, "Started 30 minutes ago"),
            (-60, "Started 1 minute ago"),
            (-3 * 3600, "Started 3 hours ago"),
        ] {
            assert_eq!(ahead(s, 0), want, "{s}");
        }
    }
}
