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

/// Where what's kept changes: the folder of accounts and each account's
/// (a calendar is a file in its account's: a watch of the first tells of
/// accounts coming and going, not of their calendars).
pub fn kept_folders() -> Vec<PathBuf> {
    let Some(dir) = store_dir() else { return Vec::new() };
    let accounts: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    std::iter::once(dir).chain(accounts).collect()
}

/// The accounts set up and every calendar kept of theirs, read: for
/// whoever only shows them (Deskbar's widgets). Nothing is asked of the
/// keyring (what signs an account in is for asking its server), and
/// nothing is written (an account removed: its calendars are passed
/// over, and forgotten by who keeps them in step). Not for the demo,
/// whose calendars are on its server. Blocking (files).
pub fn kept() -> (Vec<Account>, Vec<Cal>) {
    let accounts = account::accounts();
    let mut kept = store_dir().map(|d| Kept::load_all(&d)).unwrap_or_default();
    kept.retain(|k| accounts.iter().any(|a| a.id() == k.account));
    (accounts, kept.into_iter().map(Cal::read).collect())
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
    /// The event at this address put (back) as this text has it: what
    /// undoes a change or a removal.
    PutBack(String, String),
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
        Job::PutBack(href, ics) => kept.put_back(&server, &href, &ics),
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
        s.fill_demo(today);
        s
    });
    Source { account: Account { provider: Provider::Other, address: "demo".into(), server: there.url() }, secret: Some(Secret::Password("demo".into())) }
}
