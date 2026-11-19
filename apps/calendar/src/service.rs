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
                        title: if e.summary.trim().is_empty() { olo_kit::words::tr("(No title)").to_owned() } else { e.summary.trim().to_owned() },
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

/// What's to come, for whoever shows it without being Calendar (Deskbar's
/// clock's menu, agents: `olo_protocol::calendar`): each occurrence of
/// the calendars shown that isn't over at `now`, from today for
/// [`olo_protocol::calendar::DAYS`] days, in order (a whole day's first
/// on its day; one begun on a day before, on today); an invitation
/// declined left out. With when each is reminded of here (`reminders`,
/// the list just worked out), for whoever else would remind of it too
/// (the phone's calendar, relayed here).
pub fn coming(book: &Book, choices: &Choices, me: &[String], now: &Zoned, zone: &TimeZone, set_up: bool, reminders: &[Reminder]) -> olo_protocol::calendar::Coming {
    use olo_protocol::calendar::{Coming, DAYS, Event, MOST};
    let at = now.datetime();
    let today = at.date();
    let last = crate::layout::day_of(crate::layout::day_number(today) + DAYS as i32);
    let unix = |t: jiff::civil::DateTime| t.to_zoned(zone.clone()).map_or(0, |z| z.timestamp().as_second());
    let mut shown: Vec<crate::model::Shown> = book.shown(today, last, zone, me, choices).into_iter().filter(|s| s.end > at && s.going != Some(event::Answer::No)).collect();
    shown.sort_by(|a, b| (a.start.date().max(today), !a.all_day, a.start, &a.title).cmp(&(b.start.date().max(today), !b.all_day, b.start, &b.title)));
    shown.truncate(MOST);
    // When each is reminded of here (by event and start).
    let mut reminds: HashMap<(String, i64), Vec<i64>> = HashMap::new();
    for r in reminders {
        reminds.entry((r.href.clone(), r.start)).or_default().push(r.at);
    }
    let events = shown
        .into_iter()
        .map(|s| Event {
            href: s.href.to_string(),
            starts: s.start.to_string(),
            title: s.title.to_string(),
            location: s.location.to_string(),
            calendar: book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default(),
            hue: Some(s.hue),
            start: unix(s.start),
            end: unix(s.end),
            all_day: s.all_day,
            reminds: reminds.remove(&(s.href.to_string(), unix(s.start))).unwrap_or_default(),
        })
        .collect();
    Coming { set_up, until: unix(last.at(0, 0, 0, 0)), events }
}

/// `coming` written where it's read, if it says anything the file there
/// doesn't (whole, then put in place: never read half written).
fn keep_coming(coming: &olo_protocol::calendar::Coming) {
    let path = olo_protocol::calendar::Coming::path();
    let Ok(bytes) = serde_json::to_vec(coming) else { return };
    if std::fs::read(&path).ok().as_deref() == Some(&bytes[..]) {
        return;
    }
    let new = path.with_extension("json.new");
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&new, &bytes)).and_then(|()| std::fs::rename(&new, &path));
    if let Err(e) = written {
        tracing::warn!("calendar: {}: {e}", path.display());
    }
}

/// How far off `start` is at `now`, in words: "Now", "In 15 minutes",
/// "In 2 hours", "Tomorrow", "In 3 days"; "Started 5 minutes ago".
pub fn ahead(start: i64, now: i64) -> String {
    use olo_kit::words::{count, tr};
    let n = |v: i64| v.max(0) as u64;
    match start - now {
        s if s <= -3600 => count(n((-s + 1800) / 3600), "Started 1 hour ago", "Started {n} hours ago"),
        s if s <= -60 => count(n((-s + 30) / 60), "Started 1 minute ago", "Started {n} minutes ago"),
        s if s < 60 => tr("Now").to_owned(),
        s if s < 3600 => count(n((s + 30) / 60), "In 1 minute", "In {n} minutes"),
        s if s < 20 * 3600 => count(n((s + 1800) / 3600), "In 1 hour", "In {n} hours"),
        s if s < 36 * 3600 => tr("Tomorrow").to_owned(),
        s => count(n((s + 43_200) / 86_400), "In 1 day", "In {n} days"),
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
    let mut followed = olo_calendar::account::watched();
    followed.push(crate::settings_path());
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
                let _ = cmd.spawn().map(olo_xdg::reap_child);
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
        // (The reminders in the person's language, as it is now.)
        olo_kit::words::choose();
        let list = reminders(&book, &settings.choices, &now, &zone);
        // What's to come, for the clock's menu (written when it changes).
        let me: Vec<String> = sources.iter().map(|s| s.account.address.clone()).collect();
        keep_coming(&coming(&book, &settings.choices, &me, &now, &zone, !sources.is_empty(), &list));
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
                .action("show", olo_kit::words::tr("Show"))
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

    /// What's to come, as the clock's menu reads it: not what's over or
    /// declined; a whole day's first on its day; one begun before on
    /// today; what opens Calendar on each; a hidden calendar's nothing.
    #[test]
    fn what_is_to_come() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let now = jiff::civil::date(2026, 11, 18).at(10, 30, 0, 0).to_zoned(zone.clone()).unwrap();
        let book = book_of(&[
            &format!("DTSTART;TZID=Europe/Amsterdam:20261118T140000\nDTEND;TZID=Europe/Amsterdam:20261118T153000\nSUMMARY:Design review\nLOCATION:Room 2\n{}", alarm("PT15M")),
            "DTSTART;TZID=Europe/Amsterdam:20261116T093000\nDTEND;TZID=Europe/Amsterdam:20261116T094500\nRRULE:FREQ=DAILY;COUNT=4\nSUMMARY:Standup",
            "DTSTART;TZID=Europe/Amsterdam:20261118T100000\nDTEND;TZID=Europe/Amsterdam:20261118T110000\nSUMMARY:Under way",
            "DTSTART;TZID=Europe/Amsterdam:20261118T080000\nDTEND;TZID=Europe/Amsterdam:20261118T090000\nSUMMARY:Over",
            "DTSTART;VALUE=DATE:20261118\nDTEND;VALUE=DATE:20261119\nSUMMARY:Release",
            "DTSTART;VALUE=DATE:20261116\nDTEND;VALUE=DATE:20261121\nSUMMARY:Away",
            "DTSTART;TZID=Europe/Amsterdam:20261118T160000\nDTEND;TZID=Europe/Amsterdam:20261118T170000\nSUMMARY:Declined\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.org",
            "DTSTART;TZID=Europe/Amsterdam:20261218T160000\nDTEND;TZID=Europe/Amsterdam:20261218T170000\nSUMMARY:A month on",
        ]);
        let me = ["me@example.org".to_owned()];
        let list = reminders(&book, &Choices::default(), &now, &zone);
        let c = coming(&book, &Choices::default(), &me, &now, &zone, true, &list);
        let unix = now.timestamp().as_second();
        let said: Vec<String> = c.events.iter().map(|e| format!("{} {}", &e.starts[5..16], e.title)).collect();
        assert_eq!(said, ["11-16T00:00 Away", "11-18T00:00 Release", "11-18T10:00 Under way", "11-18T14:00 Design review", "11-19T09:30 Standup"]);
        assert_eq!(c.until - unix, 13 * 86_400 + 13 * 3600 + 1800);
        let review = &c.events[3];
        assert_eq!((review.start - unix, review.end - review.start, review.all_day, review.location.as_str(), review.calendar.as_str()), (3 * 3600 + 1800, 5400, false, "Room 2", "Work"));
        assert_eq!(review.shows(), ["olo-calendar", "--show", "https://x/work/0.ics", "2026-11-18T14:00:00"]);
        // When Calendar reminds of each (none: no reminder set), and
        // whether it does of an event by its title, about now.
        assert_eq!((review.reminds.as_slice(), c.events[4].reminds.len()), (&[review.start - 900][..], 0));
        assert!(c.reminds(" design Review ", review.start - 900 + 40, 120) && !c.reminds("Design review", review.start - 300, 120) && !c.reminds("Standup", review.start - 900, 120));
        assert!(c.events[1].all_day && c.events[1].end - c.events[1].start == 86_400);
        // (Read later, what has ended since isn't said.)
        assert_eq!(c.from(unix + 3600).map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Away", "Release", "Design review", "Standup"]);
        // Written and read back the same.
        assert_eq!(serde_json::from_slice::<olo_protocol::calendar::Coming>(&serde_json::to_vec(&c).unwrap()).unwrap(), c);
        let mut hidden = Choices::default();
        hidden.hidden.insert("https://x/work/".into());
        assert!(coming(&book, &hidden, &me, &now, &zone, true, &[]).events.is_empty());
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
