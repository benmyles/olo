//! Calendars in step with their servers, all of it on workers: every
//! account's calendars read from disk as the app opens, each account
//! brought in step (its calendars listed, each asked what changed), and
//! what a person changes sent to its server. And the demo: a server of
//! the app's own with made-up calendars (`OLO_CALENDAR_DEMO=1`: tests,
//! screenshots, agent screens), which the app is run against as it is
//! against any other.

use std::{path::PathBuf, sync::OnceLock};

use jiff::civil::Date;
use olo_calendar::{
    account::{self, Account, Provider, Secret},
    dav::{self, same_href},
    standin::Standin,
    store::Kept,
};

use crate::model::Cal;

/// An account and what signs it in (None: not in the keyring).
#[derive(Debug, Clone)]
pub struct Source {
    pub account: Account,
    pub secret: Option<Secret>,
}

/// The demo, if asked for.
pub fn demo() -> bool {
    std::env::var("OLO_CALENDAR_DEMO").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Where calendars are kept: a cache of what the servers have (a write
/// goes to its server before it's here), so under the cache.
pub fn store_dir() -> Option<PathBuf> {
    (!demo()).then(|| olo_xdg::cache_dir().join("calendar"))
}

/// The accounts set up with what signs each in, and every calendar kept,
/// read. Blocking (files, the keyring).
pub fn load(today: Date) -> (Vec<Source>, Vec<Cal>) {
    if demo() {
        return (vec![demo_source(today)], Vec::new());
    }
    let sources: Vec<Source> = account::accounts().into_iter().map(|a| Source { secret: account::secret(&a), account: a }).collect();
    let mut kept = store_dir().map(|d| Kept::load_all(&d)).unwrap_or_default();
    // (An account removed: its calendars go with it.)
    kept.retain(|k| {
        let known = sources.iter().any(|s| s.account.id() == k.account);
        if !known && let Some(dir) = store_dir() {
            k.forget(&dir);
        }
        known
    });
    (sources, kept.into_iter().map(Cal::read).collect())
}

/// `source`'s account in step with its server: its calendars as the
/// server lists them now, each with what changed since `known` taken in
/// and saved; one the server no longer has is forgotten. None: nothing
/// changed (what the window has is right). A calendar that can't be
/// read keeps what it had (and the first such error is said, with what
/// did work). Blocking.
pub fn sync(source: &Source, known: Vec<Kept>) -> (Option<Vec<Cal>>, Result<(), dav::Error>) {
    let (kept, changed, result) = sync_kept(source, known);
    (changed.then(|| kept.into_iter().map(Cal::read).collect()), result)
}

fn sync_kept(source: &Source, mut known: Vec<Kept>) -> (Vec<Kept>, bool, Result<(), dav::Error>) {
    let Some(secret) = &source.secret else { return (known, false, Err(dav::Error::SignIn)) };
    let id = source.account.id();
    let server = account::connect(&source.account, secret);
    let listed = match server.calendars(&source.account.start()) {
        Ok(l) => l,
        Err(e) => return (known, false, Err(e)),
    };
    let dir = store_dir();
    let (mut out, mut wrong) = (Vec::new(), Ok(()));
    let mut changed = false;
    for c in &listed {
        let mut kept = match known.iter().position(|k| same_href(&k.href, &c.href)) {
            Some(i) => known.swap_remove(i),
            None => {
                changed = true;
                Kept::new(&id, c)
            }
        };
        let said = (kept.name.clone(), kept.color.clone(), kept.writable);
        kept.describe(c);
        changed |= said != (kept.name.clone(), kept.color.clone(), kept.writable);
        let token = kept.token.clone();
        match kept.sync(&server) {
            Ok(step) => changed |= step.any(),
            Err(e) => {
                tracing::warn!("calendar: {} of {id}: {e}", kept.name);
                if wrong.is_ok() {
                    wrong = Err(e);
                }
            }
        }
        // (Saved when something of it is new: its events, what the
        // server calls it, the token to ask with next.)
        if (changed || token != kept.token)
            && let Some(dir) = &dir
            && let Err(e) = kept.save(dir)
        {
            tracing::warn!("calendar: keeping {}: {e}", kept.name);
        }
        out.push(kept);
    }
    // Gone from the server.
    changed |= !known.is_empty();
    if let Some(dir) = &dir {
        for gone in &known {
            gone.forget(dir);
        }
    }
    (out, changed, wrong)
}

/// Something to write to a calendar's server.
pub enum Job {
    /// A new event.
    Create(Box<olo_calendar::event::Event>),
    /// The event at this address changed (given its whole `VCALENDAR`).
    Change(String, Box<dyn FnOnce(&mut olo_calendar::ical::Component) -> Result<(), &'static str> + Send>),
    /// The event at this address removed.
    Remove(String),
}

/// `job` done on `kept`'s server, then here: the calendar as it is after
/// (saved), and how it went. Changed somewhere else meanwhile, nothing
/// was written and the calendar is as that change left it
/// (`dav::Error::Changed`). Blocking.
pub fn write(source: &Source, mut kept: Kept, job: Job) -> (Cal, Result<(), dav::Error>) {
    let Some(secret) = &source.secret else { return (Cal::read(kept), Err(dav::Error::SignIn)) };
    let server = account::connect(&source.account, secret);
    let result = match job {
        Job::Create(event) => kept.create(&server, &event).map(|_| ()),
        Job::Change(href, change) => kept.change(&server, &href, |cal| change(cal).map_err(|e| dav::Error::Answer(e.into()))),
        Job::Remove(href) => kept.remove(&server, &href),
    };
    if let Some(dir) = store_dir()
        && let Err(e) = kept.save(&dir)
    {
        tracing::warn!("calendar: keeping {}: {e}", kept.name);
    }
    (Cal::read(kept), result)
}

static DEMO: OnceLock<Standin> = OnceLock::new();

/// The demo's account: a server in this process, started once, with
/// calendars made for the weeks round `today`.
fn demo_source(today: Date) -> Source {
    let there = DEMO.get_or_init(|| {
        let s = Standin::start("demo", "demo");
        fill_demo(&s, today);
        s
    });
    Source { account: Account { provider: Provider::Other, address: "demo".into(), server: there.url() }, secret: Some(Secret::Password("demo".into())) }
}

/// The demo's calendars: a life's worth of a fortnight, in each kind of
/// event the views draw (all-day and across days, repeating, side by
/// side, short, late).
fn fill_demo(s: &Standin, today: Date) {
    let monday = crate::layout::week_of(today, 0);
    let day = |n: i32| crate::layout::day_of(crate::layout::day_number(monday) + n);
    let date = |n: i32| day(n).strftime("%Y%m%d").to_string();
    let at = |n: i32, h: u8, m: u8| format!("{}T{h:02}{m:02}00", date(n));
    let mut n = 0;
    let mut put = |folder: &str, body: String| {
        n += 1;
        s.put(folder, &format!("{n}.ics"), &format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:demo-{n}\r\n{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", body.replace('\n', "\r\n")));
    };
    let timed = |a: String, b: String, title: &str, more: &str| format!("DTSTART:{a}\nDTEND:{b}\nSUMMARY:{title}{}{more}", if more.is_empty() { "" } else { "\n" });
    let days = |a: String, b: String, title: &str, more: &str| format!("DTSTART;VALUE=DATE:{a}\nDTEND;VALUE=DATE:{b}\nSUMMARY:{title}{}{more}", if more.is_empty() { "" } else { "\n" });
    s.add_calendar("personal", "Personal", Some("#0B8043"));
    s.add_calendar("work", "Work", Some("#4285F4"));
    s.add_calendar("family", "Family", Some("#F4511E"));
    s.add_calendar("holidays", "Holidays", Some("#8E24AA"));
    s.set_read_only("holidays", true);
    put("personal", timed(at(-7, 7, 0), at(-7, 7, 45), "Run", "RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR"));
    put("personal", timed(at(1, 15, 0), at(1, 16, 0), "Dentist", "LOCATION:Tandartspraktijk Centrum"));
    put("personal", timed(at(4, 19, 30), at(4, 22, 0), "Dinner with Dana", "LOCATION:De Kas"));
    put("personal", days(date(5), date(7), "Weekend on Texel", ""));
    put("personal", days(date(3), date(4), "Sam’s birthday", "RRULE:FREQ=YEARLY"));
    put("personal", timed(at(2, 20, 0), at(2, 21, 30), "Choir", "RRULE:FREQ=WEEKLY"));
    put("personal", timed(at(9, 18, 10), at(9, 20, 25), "Flight to Lisbon", "LOCATION:AMS → LIS"));
    put("work", timed(at(-7, 9, 30), at(-7, 9, 45), "Standup", "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"));
    put("work", timed(at(0, 13, 0), at(0, 15, 0), "Planning", "LOCATION:Room 4"));
    put("work", timed(at(0, 14, 0), at(0, 15, 0), "Budget call", ""));
    put("work", timed(at(1, 11, 0), at(1, 11, 30), "1:1 with Kim", "RRULE:FREQ=WEEKLY"));
    put("work", timed(at(2, 14, 0), at(2, 15, 30), "Design review", "LOCATION:Room 2\nDESCRIPTION:The new month view\\, and what’s left of the week’s."));
    put("work", timed(at(2, 15, 0), at(2, 15, 45), "Interview", ""));
    put("work", timed(at(2, 15, 30), at(2, 16, 30), "Retro", ""));
    put("work", days(date(3), date(4), "Release 2.4", ""));
    put("work", days(date(8), date(11), "Conference", "LOCATION:Lisbon"));
    put("work", timed(at(4, 16, 0), at(4, 16, 15), "Timesheets", "RRULE:FREQ=WEEKLY"));
    put("family", timed(at(5, 12, 0), at(6, 16, 0), "Parents visiting", ""));
    put("family", timed(at(3, 17, 30), at(3, 18, 30), "Swimming lesson", "RRULE:FREQ=WEEKLY"));
    put("family", days(date(14), date(19), "School holiday", ""));
    put("holidays", days(date(-3), date(-2), "Bank holiday", ""));
    put("holidays", days(date(17), date(18), "Sinterklaas", ""));
}
