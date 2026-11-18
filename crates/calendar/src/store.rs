//! Calendars kept: what each calendar held when it was last in step with
//! its server, on disk, so the app opens with everything there at once
//! and asks its servers only for what changed since; and what a person
//! changes written to the server first, then here.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    dav::{Collection, Error, Server},
    event::Event,
    ical::{self, Component},
};

/// One event as its server has it: its address, its tag, its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub etag: String,
    pub ics: String,
}

/// A calendar kept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// Whose it is (the account's id).
    pub account: String,
    pub href: String,
    pub name: String,
    /// The server's colour for it, and the person's own, if they chose one.
    pub color: Option<String>,
    pub own_color: Option<String>,
    pub writable: bool,
    /// Not shown (the person's choice): still kept in step.
    pub hidden: bool,
    /// What to tell the server to hear only what changed since.
    pub token: Option<String>,
    /// By address.
    pub items: BTreeMap<String, Item>,
}

/// What a step with the server changed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Step {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

impl Step {
    pub fn any(&self) -> bool {
        self.added + self.changed + self.removed > 0
    }
}

/// An event kept, read: the event itself, and its occurrences changed by
/// themselves.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub href: String,
    pub event: Event,
    pub overrides: Vec<Event>,
}

impl Kept {
    /// A calendar of `account`'s, as the server lists it; nothing of it
    /// read yet.
    pub fn new(account: &str, c: &Collection) -> Kept {
        Kept { account: account.to_owned(), href: c.href.clone(), name: c.name.clone(), color: c.color.clone(), writable: c.writable, ..Default::default() }
    }

    /// The colour it shows in.
    pub fn shown_color(&self) -> Option<&str> {
        self.own_color.as_deref().or(self.color.as_deref())
    }

    /// What the server says of it now (its name, colour, whether it may
    /// be written to) taken in; what the person chose stays.
    pub fn describe(&mut self, c: &Collection) {
        (self.name, self.color, self.writable) = (c.name.clone(), c.color.clone(), c.writable);
    }

    /// In step with the server: what changed since the token is asked for
    /// (everything, with none), the changed events' text fetched, and all
    /// of it taken in. A server with no tokens, or one that forgot ours,
    /// is asked for every event's tag instead, and those that differ are
    /// fetched.
    pub fn sync(&mut self, server: &Server) -> Result<Step, Error> {
        let (wanted, removed, token) = match server.changes(&self.href, self.token.as_deref()) {
            Ok(c) => {
                // (Told of ones we have at that tag already: nothing to fetch.)
                let wanted: Vec<String> = c.changed.into_iter().filter(|r| self.items.get(&r.href).is_none_or(|i| i.etag != r.etag)).map(|r| r.href).collect();
                (wanted, c.removed, c.token)
            }
            // No tokens here (or ours forgotten): every tag, compared.
            Err(Error::StaleToken | Error::Status(_)) => {
                let listed = server.list(&self.href)?;
                let removed: Vec<String> = self.items.keys().filter(|h| !listed.iter().any(|r| crate::dav::same_href(&r.href, h))).cloned().collect();
                let wanted = listed.into_iter().filter(|r| self.items.get(&r.href).is_none_or(|i| i.etag != r.etag)).map(|r| r.href).collect();
                // (A fresh token, if it gives them at all, for next time.)
                let token = server.changes(&self.href, None).ok().and_then(|c| c.token);
                (wanted, removed, token)
            }
            Err(e) => return Err(e),
        };
        let mut step = Step::default();
        for r in server.fetch(&self.href, &wanted)? {
            let Some(ics) = r.data else { continue };
            match self.items.insert(r.href, Item { etag: r.etag, ics }) {
                Some(_) => step.changed += 1,
                None => step.added += 1,
            }
        }
        for href in removed {
            let key = self.items.keys().find(|h| crate::dav::same_href(h, &href)).cloned();
            if key.and_then(|k| self.items.remove(&k)).is_some() {
                step.removed += 1;
            }
        }
        self.token = token;
        Ok(step)
    }

    /// Every event kept, read (one that can't be read is passed over).
    pub fn events(&self) -> Vec<Stored> {
        self.items.iter().filter_map(|(href, item)| read(href, &item.ics)).collect()
    }

    /// A new event put on the server, then kept: its address.
    pub fn create(&mut self, server: &Server, event: &Event) -> Result<String, Error> {
        let mut vevent = Component::default();
        event.write_into(&mut vevent);
        let mut cal = Component::new("VCALENDAR");
        cal.lines.push(ical::Line::new("VERSION", "2.0"));
        cal.lines.push(ical::Line::new("PRODID", "-//Olo//Calendar//EN"));
        cal.children.push(vevent);
        // (Named for its UID, less what an address can't hold.)
        let name: String = event.uid.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') { c } else { '-' }).collect();
        let href = format!("{}/{name}.ics", self.href.trim_end_matches('/'));
        let ics = cal.write();
        let etag = server.put(&href, &ics, None)?;
        self.keep(server, &href, ics, etag)?;
        Ok(href)
    }

    /// The event at `href` changed by `change` (given its whole
    /// `VCALENDAR`, to change what it will) and put back on the server
    /// as it was read here, then kept. If it was changed somewhere else
    /// meanwhile nothing is written: that change is read in, and the
    /// error says so, for the person to look again.
    pub fn change(&mut self, server: &Server, href: &str, change: impl FnOnce(&mut Component)) -> Result<(), Error> {
        let item = self.items.get(href).ok_or(Error::Gone)?;
        let mut cal = ical::parse(&item.ics).into_iter().next().ok_or_else(|| Error::Answer("what's kept of it can't be read".into()))?;
        change(&mut cal);
        let ics = cal.write();
        match server.put(href, &ics, Some(&item.etag)) {
            Ok(etag) => self.keep(server, href, ics, etag),
            Err(Error::Changed) => {
                let _ = self.sync(server);
                Err(Error::Changed)
            }
            Err(e) => Err(e),
        }
    }

    /// The event at `href` removed from the server, then from here.
    /// Changed somewhere else meanwhile: as [`Kept::change`].
    pub fn remove(&mut self, server: &Server, href: &str) -> Result<(), Error> {
        let item = self.items.get(href).ok_or(Error::Gone)?;
        match server.delete(href, &item.etag) {
            Ok(()) => {
                self.items.remove(href);
                Ok(())
            }
            Err(Error::Changed) => {
                let _ = self.sync(server);
                Err(Error::Changed)
            }
            Err(e) => Err(e),
        }
    }

    /// What was just put, kept with its tag: the one the server gave, or
    /// (it gave none, or may have rewritten the text) read back from it.
    fn keep(&mut self, server: &Server, href: &str, ics: String, etag: Option<String>) -> Result<(), Error> {
        let item = match etag {
            Some(etag) => Item { etag, ics },
            None => match server.fetch(&self.href, &[href.to_owned()])?.into_iter().next() {
                Some(r) => Item { etag: r.etag, ics: r.data.unwrap_or(ics) },
                None => return Err(Error::Gone),
            },
        };
        self.items.insert(href.to_owned(), item);
        Ok(())
    }
}

/// An event's text read: its `VEVENT` with no `RECURRENCE-ID`, and those
/// with one.
pub fn read(href: &str, ics: &str) -> Option<Stored> {
    let cal = ical::parse(ics).into_iter().next()?;
    let mut all: Vec<Event> = cal.children_named("VEVENT").filter_map(Event::read).collect();
    // (All of them changed-by-themselves, the event itself not sent: the
    // first stands for it.)
    let at = all.iter().position(|e| e.recurrence_id.is_none()).unwrap_or(0);
    if all.is_empty() {
        return None;
    }
    let event = all.remove(at);
    Some(Stored { href: href.to_owned(), event, overrides: all })
}

/// Where a calendar is kept under `dir`: a file named for its account
/// and its address.
pub fn path(dir: &Path, account: &str, href: &str) -> PathBuf {
    let safe = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') { c } else { '_' }).collect::<String>();
    // (The address's end is what tells an account's calendars apart; its
    // whole, hashed, what makes the name its own.)
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in href.bytes() {
        hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let tail = href.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    dir.join(safe(account)).join(format!("{}-{hash:016x}.json", safe(tail)))
}

impl Kept {
    /// Written whole (to a file beside it, then put in its place).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = path(dir, &self.account, &self.href);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.saving");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn load(dir: &Path, account: &str, href: &str) -> Option<Kept> {
        serde_json::from_slice(&std::fs::read(path(dir, account, href)).ok()?).ok()
    }

    /// Every calendar kept under `dir`.
    pub fn load_all(dir: &Path) -> Vec<Kept> {
        let mut out = Vec::new();
        for account in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(account.path()).into_iter().flatten().flatten() {
                if file.path().extension().is_some_and(|e| e == "json")
                    && let Some(k) = std::fs::read(file.path()).ok().and_then(|b| serde_json::from_slice::<Kept>(&b).ok())
                {
                    out.push(k);
                }
            }
        }
        out.sort_by(|a, b| (&a.account, &a.name, &a.href).cmp(&(&b.account, &b.name, &b.href)));
        out
    }

    /// No longer kept (its account removed, or the calendar gone from it).
    pub fn forget(&self, dir: &Path) {
        let _ = std::fs::remove_file(path(dir, &self.account, &self.href));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event::When, ical::Line, rrule::civil, standin::Standin};

    fn ics(uid: &str, summary: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261120T140000Z\r\nDTEND:20261120T150000Z\r\nSUMMARY:{summary}\r\nX-KEPT:as it is\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    fn summaries(k: &Kept) -> Vec<String> {
        let mut s: Vec<String> = k.events().into_iter().map(|e| e.event.summary).collect();
        s.sort();
        s
    }

    /// A calendar is read whole the first time, then only what changed:
    /// another device's changes, additions and removals come in, and the
    /// requests show nothing else was asked for.
    #[test]
    fn kept_in_step_asking_only_for_what_changed() {
        let there = Standin::start("ben@example.org", "pw");
        there.add_calendar("home", "Home", Some("#3366FF"));
        there.put("home", "a.ics", &ics("a", "Lunch"));
        there.put("home", "b.ics", &ics("b", "Dentist"));
        let server = Server::new(there.auth());
        let found = server.calendars(&there.url()).unwrap();
        let mut kept = Kept::new("ben", &found[0]);
        assert_eq!((kept.name.as_str(), kept.shown_color(), kept.writable), ("Home", Some("#3366FF"), true));
        there.asked();
        assert_eq!(kept.sync(&server), Ok(Step { added: 2, changed: 0, removed: 0 }));
        assert_eq!(summaries(&kept), ["Dentist", "Lunch"]);
        assert_eq!(there.asked(), ["REPORT /cal/home/", "REPORT /cal/home/"], "what changed, then their text");
        // Nothing changed: one request, nothing fetched.
        assert_eq!(kept.sync(&server), Ok(Step::default()));
        assert_eq!(there.asked(), ["REPORT /cal/home/"]);
        // Another device: one changed, one added, one removed.
        there.put("home", "a.ics", &ics("a", "Lunch, later"));
        there.put("home", "c.ics", &ics("c", "Run"));
        there.remove("home", "b.ics");
        assert_eq!(kept.sync(&server), Ok(Step { added: 1, changed: 1, removed: 1 }));
        assert_eq!(summaries(&kept), ["Lunch, later", "Run"]);
        // The server forgot our token: every tag compared, only what differs fetched.
        there.forget_changes();
        there.put("home", "d.ics", &ics("d", "Swim"));
        there.asked();
        assert_eq!(kept.sync(&server), Ok(Step { added: 1, changed: 0, removed: 0 }));
        assert!(there.asked().contains(&"PROPFIND /cal/home/".to_owned()));
        assert_eq!(summaries(&kept), ["Lunch, later", "Run", "Swim"]);
        // And in step again by token after.
        there.remove("home", "c.ics");
        assert_eq!(kept.sync(&server), Ok(Step { added: 0, changed: 0, removed: 1 }));
    }

    /// What a person changes goes to the server first, then here: a new
    /// event, one changed (what Olo doesn't know of it kept), one
    /// removed; and one changed somewhere else meanwhile isn't written
    /// over: that change comes in, and it's said.
    #[test]
    fn changes_go_to_the_server_first_and_never_over_anothers() {
        let there = Standin::start("ben@example.org", "pw");
        there.add_calendar("home", "Home", None);
        there.put("home", "a.ics", &ics("a", "Lunch"));
        let server = Server::new(there.auth());
        let mut kept = Kept::new("ben", &server.calendars(&there.url()).unwrap()[0]);
        kept.sync(&server).unwrap();
        // New.
        let day = |d: &str| When::Date(civil(d).unwrap().date());
        let href = kept.create(&server, &Event::new("new one/2026", "Holiday", day("20261224"), day("20261227"))).unwrap();
        assert!(href.ends_with("/cal/home/new-one-2026.ics"), "{href}");
        assert!(there.get("home", "new-one-2026.ics").unwrap().contains("SUMMARY:Holiday"));
        assert_eq!(summaries(&kept), ["Holiday", "Lunch"]);
        // Changed: its title; the server's own line stays.
        let a = kept.items.keys().find(|h| h.ends_with("a.ics")).cloned().unwrap();
        kept.change(&server, &a, |cal| cal.children[0].set(Line::text("SUMMARY", "Lunch at one"))).unwrap();
        let on_server = there.get("home", "a.ics").unwrap();
        assert!(on_server.contains("SUMMARY:Lunch at one") && on_server.contains("X-KEPT:as it is"));
        // In step: nothing to fetch (our own changes are what we have).
        assert_eq!(kept.sync(&server), Ok(Step::default()));
        // Changed on another device meanwhile: ours isn't written; theirs comes in.
        there.put("home", "a.ics", &ics("a", "Lunch moved by Dana"));
        assert_eq!(kept.change(&server, &a, |cal| cal.children[0].set(Line::text("SUMMARY", "Mine"))), Err(Error::Changed));
        assert!(there.get("home", "a.ics").unwrap().contains("Lunch moved by Dana"));
        assert_eq!(summaries(&kept), ["Holiday", "Lunch moved by Dana"]);
        // Removed; and one changed elsewhere meanwhile isn't removed.
        kept.remove(&server, &href).unwrap();
        assert_eq!((there.files("home"), summaries(&kept)), (vec!["a.ics".to_owned()], vec!["Lunch moved by Dana".to_owned()]));
        there.put("home", "a.ics", &ics("a", "Changed again"));
        assert_eq!(kept.remove(&server, &a), Err(Error::Changed));
        assert_eq!(summaries(&kept), ["Changed again"]);
        assert_eq!(kept.remove(&server, "http://nowhere/x.ics"), Err(Error::Gone));
    }

    /// Kept on disk as it is, a file a calendar, and read back; a
    /// calendar forgotten is gone from it.
    #[test]
    fn kept_on_disk() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-calendar-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut home = Kept {
            account: "ben@example.org".into(),
            href: "https://caldav.example.org/dav/calendars/user/ben@example.org/Default/".into(),
            name: "Home".into(),
            token: Some("t7".into()),
            writable: true,
            ..Default::default()
        };
        home.items.insert(format!("{}a.ics", home.href), Item { etag: "\"e1\"".into(), ics: ics("a", "Lunch") });
        let work = Kept {
            account: "ben@example.org".into(),
            href: "https://caldav.example.org/dav/calendars/user/ben@example.org/Work/".into(),
            name: "Work".into(),
            hidden: true,
            own_color: Some("#FF0000".into()),
            ..Default::default()
        };
        home.save(&dir).unwrap();
        work.save(&dir).unwrap();
        assert_ne!(path(&dir, &home.account, &home.href), path(&dir, &work.account, &work.href));
        assert_eq!(Kept::load(&dir, &home.account, &home.href), Some(home.clone()));
        assert_eq!(Kept::load_all(&dir), vec![home.clone(), work.clone()]);
        work.forget(&dir);
        assert_eq!(Kept::load_all(&dir), vec![home]);
        assert!(Kept::load_all(&dir.join("nothing-here")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
