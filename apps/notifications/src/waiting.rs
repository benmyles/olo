//! What apps say is waiting in them (red): an app's own count, which it
//! says on its Deskbar row (`com.canonical.Unity.LauncherEntry`'s count,
//! or that it wants attention). **Not a notification**: nothing
//! happened, the app is telling how it stands ("Slack has 3 unread").
//! So it isn't in the list of notifications, isn't counted on the bell,
//! can't be cleared, and doesn't colour the app's notifications: it's
//! there while the app says so and gone when it stops.
//!
//! Where it shows: the red dot beside the app in Deskbar (Deskbar's
//! own, from the app); a "Waiting in apps" strip at the head of the
//! Notifications window; and a red card for each app in Deskbar's Inbox
//! widget, with Later on it: put off until more is waiting than there
//! was, or it turns urgent. Going to the app puts it off too (what's
//! seen is seen). What was put off is kept across a restart.
//!
//! (It was an item in the list of notifications, titled "3 waiting",
//! which turned its app's notifications red while it was there, was
//! counted on the bell, said "1 more waiting" in one place and "2
//! waiting" in another, and was deleted, not cleared, by ×: Ben,
//! 2026-11-18: "the red vs blue types … is all very confusing".)

use std::path::PathBuf;

use olo_protocol::notifications::{Attention, InboxCard, Waits};
use serde::{Deserialize, Serialize};

use crate::store;

/// One app, and what it says is waiting in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Waiting {
    /// What Deskbar's Inbox and agents name it by (among the
    /// notifications' ids, and none of theirs).
    pub id: u32,
    /// Its desktop entry, as its launcher badge names it ("olo-slack").
    pub app: String,
    /// Its name for people ("Slack").
    pub name: String,
    /// How many (0: it only wants attention).
    pub count: u64,
    pub urgent: bool,
    /// Since when (seconds since the epoch): when it began, or last
    /// grew or turned urgent.
    pub time: u64,
    /// Put off (Later, or the app gone to): the count and urgency then.
    /// Hidden from the Inbox until it's more than that.
    pub later: Option<(u64, bool)>,
    /// Each thing waiting and where it is, where the app says (an Olo
    /// app's: Terminal's tabs), in its order: each a row of the strip and
    /// a card of the Inbox, going to itself. None: only a count.
    pub waits: Vec<Wait>,
}

/// One thing waiting in an app, and where (`Waits`), with an id of its
/// own among the notifications' (its Inbox card's): kept while the app
/// says it, as long as it shows the same place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Wait {
    pub id: u32,
    #[serde(flatten)]
    pub waits: Waits,
    /// Gone to, or put off (Later): out of the Inbox while it stays as it
    /// is (what it says changing, it's back).
    pub seen: bool,
}

impl Wait {
    /// "Claude Code needs your answer, in “Fix login redirect”": the same
    /// words on every surface.
    pub fn said(&self) -> String {
        match self.waits.place.as_str() {
            "" => self.waits.what.clone(),
            place => olo_kit::tr!("{}, in “{}”", self.waits.what, place),
        }
    }
}

impl Waiting {
    /// Put off, and nothing more waiting than when it was (where it says
    /// what waits: each of those put off).
    pub fn put_off(&self) -> bool {
        if !self.waits.is_empty() {
            return self.waits.iter().all(|w| w.seen);
        }
        self.later.is_some_and(|(count, urgent)| self.count <= count && (!self.urgent || urgent))
    }

    /// "3 waiting in Slack", "1 waiting in Slack", "Slack wants your
    /// attention": the same words on every surface.
    pub fn said(&self) -> String {
        match self.count {
            0 => olo_kit::tr!("{} wants your attention", self.name),
            n => olo_kit::tr!("{} waiting in {}", n, self.name),
        }
    }

    /// Beside the app's name: "3 waiting", "Wants attention".
    pub fn short(&self) -> String {
        match self.count {
            0 => olo_kit::words::tr("Wants attention").to_owned(),
            n => format!("{n} waiting"),
        }
    }
}

/// Every app with something waiting in it, in the order they began.
#[derive(Debug, Default)]
pub struct WaitingApps {
    pub apps: Vec<Waiting>,
    /// What was put off before a restart, by app, until its app says
    /// again what waits in it.
    remembered: Vec<Waiting>,
    path: Option<PathBuf>,
}

impl WaitingApps {
    /// Beside the list of notifications.
    pub fn path() -> PathBuf {
        store::History::path().with_file_name("waiting.json")
    }

    /// With what was put off before (`path` None: nothing kept, a test's).
    pub fn load(path: Option<PathBuf>) -> WaitingApps {
        let remembered = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Vec<Waiting>>(&b).ok()).unwrap_or_default();
        WaitingApps { apps: Vec::new(), remembered, path }
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            olo_kit::store::save(path.clone(), &self.apps);
        }
    }

    /// What the apps say now (`badges`: each one's count and urgency):
    /// one that stopped is gone; one that began, or grew, or turned
    /// urgent has its time from `now`; what was put off stays put off
    /// for no more than was there then (fewer waiting: put off at that).
    /// `name` names an app for people; `id` gives a new one its id.
    /// Whether anything changed.
    pub fn update(&mut self, badges: &[Attention], now: u64, name: impl Fn(&str) -> String, mut id: impl FnMut() -> u32) -> bool {
        let before = self.apps.clone();
        self.apps.retain(|w| badges.iter().any(|b| b.app == w.app));
        for badge in badges {
            match self.apps.iter_mut().find(|w| w.app == badge.app) {
                Some(w) => {
                    if badge.count > w.count || (badge.urgent && !w.urgent) {
                        w.time = now;
                    }
                    (w.count, w.urgent) = (badge.count, badge.urgent);
                    w.later = w.later.map(|(count, urgent)| (count.min(badge.count), urgent && badge.urgent));
                    w.waits = Self::waits(&w.waits, &badge.waits, true, &mut id);
                }
                None => {
                    // (Put off before a restart: still put off, for as much.)
                    let before = self.remembered.iter().find(|w| w.app == badge.app);
                    let later = before.and_then(|w| w.later).map(|(count, urgent)| (count.min(badge.count), urgent && badge.urgent));
                    let waits = Self::waits(before.map_or(&[][..], |w| &w.waits), &badge.waits, false, &mut id);
                    self.apps.push(Waiting { id: id(), app: badge.app.clone(), name: name(&badge.app), count: badge.count, urgent: badge.urgent, time: now, later, waits });
                }
            }
        }
        self.remembered.clear();
        let changed = self.apps != before;
        if changed {
            self.save();
        }
        changed
    }

    /// What the app says waits where (`said`): each one it said before
    /// (`had`, the same place) keeps whether it was seen while it says the
    /// same, and its id (`same_ids`: from this run, not before a restart);
    /// a new one is given one.
    fn waits(had: &[Wait], said: &[Waits], same_ids: bool, id: &mut impl FnMut() -> u32) -> Vec<Wait> {
        said.iter()
            .map(|w| {
                let before = had.iter().find(|h| h.waits.shows == w.shows);
                Wait { id: before.filter(|_| same_ids).map_or_else(&mut *id, |h| h.id), waits: w.clone(), seen: before.is_some_and(|h| h.seen && h.waits.what == w.what) }
            })
            .collect()
    }

    /// The app `id` is of: its own id, or one of what waits in it.
    pub fn get(&self, id: u32) -> Option<&Waiting> {
        self.apps.iter().find(|w| w.id == id || w.waits.iter().any(|x| x.id == id))
    }

    /// Put `id` off (Later, or gone to): out of the Inbox until more is
    /// waiting; one thing waiting in an app (its id), that one alone.
    /// Whether it changed.
    pub fn later(&mut self, id: u32) -> bool {
        let Some(w) = self.apps.iter_mut().find(|w| w.id == id || w.waits.iter().any(|x| x.id == id)) else { return false };
        let changed = match w.waits.iter_mut().find(|x| x.id == id) {
            Some(x) => !std::mem::replace(&mut x.seen, true),
            None => Self::put_off(w),
        };
        changed && {
            self.save();
            true
        }
    }

    fn put_off(w: &mut Waiting) -> bool {
        let was = (w.later, w.waits.iter().filter(|x| x.seen).count());
        w.later = Some((w.count, w.urgent));
        for x in &mut w.waits {
            x.seen = true;
        }
        (w.later, w.waits.iter().filter(|x| x.seen).count()) != was
    }

    /// The person went to a window of app `app_id`: what waits in it has
    /// been seen, as much as there is now. Whether anything changed.
    pub fn visited_app(&mut self, app_id: &str) -> bool {
        let mut changed = false;
        for w in self.apps.iter_mut().filter(|w| store::is_app(&store::app_ids(Some(&w.app), &w.name), app_id)) {
            changed |= Self::put_off(w);
        }
        if changed {
            self.save();
        }
        changed
    }

    /// Deskbar's Inbox cards: one for each app not put off (one for each
    /// thing waiting in it, where it says where), the urgent first, then
    /// the newest (`icons`: the names an app's icon may have).
    pub fn cards(&self, icons: impl Fn(&Waiting) -> Vec<String>) -> Vec<InboxCard> {
        let mut shown: Vec<&Waiting> = self.apps.iter().filter(|w| !w.put_off()).collect();
        shown.sort_by_key(|w| (!w.urgent, std::cmp::Reverse((w.time, w.id))));
        shown
            .into_iter()
            .flat_map(|w| {
                let waits: Vec<&Wait> = w.waits.iter().filter(|x| !x.seen).collect();
                let card = |id: u32, title: String, detail: String| InboxCard {
                    id,
                    app: w.name.clone(),
                    icons: icons(w),
                    title,
                    detail,
                    attention: true,
                    time: w.time,
                    fade_minutes: None,
                    pinned: false,
                    instance: 0,
                };
                if w.waits.is_empty() {
                    let detail = match w.count {
                        0 | 1 => olo_kit::tr!("Open {} to see it.", w.name),
                        _ => olo_kit::tr!("Open {} to see them.", w.name),
                    };
                    vec![card(w.id, w.said(), detail)]
                } else {
                    waits
                        .into_iter()
                        .map(|x| {
                            // (Its app's name heads the card; the place
                            // under what waits, bare, cut short cleanly:
                            // "In “Token Pool — capture cli…" opened a
                            // quote it never closed, Codex's look.)
                            let detail = match x.waits.place.as_str() {
                                "" => olo_kit::tr!("Open {} to see it.", w.name),
                                place => place.to_owned(),
                            };
                            card(x.id, x.waits.what.clone(), detail)
                        })
                        .collect()
                }
            })
            .collect()
    }
}

/// What waits in app `app` (its launcher badge's name), as the app itself
/// said it in a notification, to go to instead of to the app as it was
/// last left (Ben, 2026-11-18: "it should go to the specific window and
/// specific tab in that window for which it is referencing … it only
/// took me to the last active window/tab in terminal"). The one still
/// open at the app that has waited longest (an app closes its note as
/// the thing stops waiting: Terminal's tab looked at, its agent gone
/// on); else the newest unread one that says how to show what it's
/// about. None: the app said nothing of it, and it's the app that's
/// gone to.
pub fn about<'a>(entries: &'a [store::Entry], app: &str) -> Option<&'a store::Entry> {
    let its = |e: &&store::Entry| store::is_app(&store::app_ids(e.desktop_entry.as_deref(), &e.app), app);
    let open = entries.iter().filter(its).filter(|e| e.live && e.actions.iter().any(|(key, _)| key == "default")).min_by_key(|e| (e.time, e.id));
    open.or_else(|| entries.iter().filter(its).filter(|e| e.unread && !e.show.is_empty()).max_by_key(|e| (e.time, e.id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click on what waits in an app goes to the thing that waits:
    /// the app's own open note about it (the one waiting longest), else
    /// its newest unread one that says how to show it; never another
    /// app's, nor one read and closed (what it was about has moved on).
    #[test]
    fn what_waits_is_gone_to_itself() {
        let note = |id: u32, app: &str, time: u64, live: bool, unread: bool, show: &[&str]| store::Entry {
            id,
            app: app.into(),
            desktop_entry: Some(format!("olo-{}", app.to_lowercase())),
            time,
            live,
            unread,
            actions: if live { vec![("default".into(), "Show".into())] } else { Vec::new() },
            show: show.iter().map(|s| (*s).to_owned()).collect(),
            ..store::Entry::default()
        };
        let entries = vec![
            note(1, "Terminal", 100, false, false, &["--show-shell", "7"]),
            note(2, "Terminal", 300, true, true, &["--show-shell", "9"]),
            note(3, "Terminal", 200, true, true, &["--show-shell", "8"]),
            note(4, "Slack", 400, true, true, &[]),
            note(5, "Calendar", 500, false, true, &["--show", "x"]),
            note(6, "Calendar", 600, false, true, &[]),
        ];
        assert_eq!(about(&entries, "olo-terminal").map(|e| e.id), Some(3), "the one waiting longest");
        assert_eq!(about(&entries, "olo-slack").map(|e| e.id), Some(4));
        assert_eq!(about(&entries, "olo-calendar").map(|e| e.id), Some(5), "closed at its app: the unread one that says how to show it");
        assert_eq!(about(&entries[..1], "olo-terminal").map(|e| e.id), None, "read and closed: the app itself");
        assert_eq!(about(&entries, "olo-mail").map(|e| e.id), None);
    }

    fn badge(app: &str, count: u64, urgent: bool) -> Attention {
        Attention { app: app.into(), count, urgent, waits: Vec::new() }
    }

    /// What waits in apps follows what they say: there while they say so,
    /// said in the same words everywhere, gone when they stop; put off
    /// (Later, or the app gone to) it's out of the Inbox until more
    /// waits, and that's remembered across a restart.
    #[test]
    fn what_waits_in_apps_is_the_apps_own() {
        let path = PathBuf::from(format!("/var/tmp/olo-waiting-test-{}/waiting.json", std::process::id()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let name = |app: &str| if app == "olo-slack" { "Slack".to_owned() } else { "Terminal".to_owned() };
        let mut next = 100;
        let mut id = || {
            next += 1;
            next
        };
        let mut w = WaitingApps::load(Some(path.clone()));
        assert!(w.update(&[badge("olo-slack", 3, false), badge("olo-terminal", 0, true)], 1000, name, &mut id));
        assert!(!w.update(&[badge("olo-slack", 3, false), badge("olo-terminal", 0, true)], 1100, name, &mut id), "the same said again: nothing changed");
        assert_eq!(
            w.apps.iter().map(|a| (a.id, a.said(), a.short())).collect::<Vec<_>>(),
            [(101, "3 waiting in Slack".into(), "3 waiting".into()), (102, "Terminal wants your attention".into(), "Wants attention".into())]
        );
        // Cards: the urgent one first; each red, never fading, opening its app.
        let cards = w.cards(|a| vec![a.app.clone()]);
        assert_eq!(
            cards.iter().map(|c| (c.id, c.title.as_str(), c.detail.as_str(), c.attention, c.fade_minutes)).collect::<Vec<_>>(),
            [(102, "Terminal wants your attention", "Open Terminal to see it.", true, None), (101, "3 waiting in Slack", "Open Slack to see them.", true, None)]
        );
        // Later: out of the Inbox, still said (the strip, Deskbar's dot).
        assert!(w.later(101) && !w.later(101) && !w.later(999));
        assert_eq!(w.cards(|_| Vec::new()).iter().map(|c| c.id).collect::<Vec<_>>(), [102]);
        assert!(w.get(101).is_some_and(|a| a.put_off() && a.count == 3));
        // Fewer waiting: still put off (and for no more than that). More
        // than was put off: back, with its time from then.
        assert!(w.update(&[badge("olo-slack", 2, false), badge("olo-terminal", 0, true)], 1200, name, &mut id));
        assert!(w.get(101).unwrap().put_off());
        assert!(w.update(&[badge("olo-slack", 3, false), badge("olo-terminal", 0, true)], 1300, name, &mut id));
        assert_eq!((w.get(101).unwrap().put_off(), w.get(101).unwrap().time, w.get(101).unwrap().said()), (false, 1300, "3 waiting in Slack".to_owned()));
        // Going to the app: seen, as much as there is (by its window's app id).
        assert!(w.visited_app("olo.slack") && !w.visited_app("olo.slack") && !w.visited_app("org.other.App"));
        assert!(w.get(101).unwrap().put_off());
        // Put off, then urgent: back.
        assert!(w.update(&[badge("olo-slack", 3, true), badge("olo-terminal", 0, true)], 1400, name, &mut id));
        assert!(!w.get(101).unwrap().put_off());
        assert!(w.later(101));
        // The app stops: gone. A restart: what was put off still is, when
        // its app says again; not for more than there was.
        assert!(w.update(&[badge("olo-slack", 3, true)], 1500, name, &mut id));
        assert_eq!(w.apps.len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut again = WaitingApps::load(Some(path.clone()));
        assert!(again.apps.is_empty(), "nothing waits until the apps say so");
        assert!(again.update(&[badge("olo-slack", 3, true)], 2000, name, &mut id));
        assert!(again.apps[0].put_off() && again.apps[0].id == 103);
        assert!(again.update(&[badge("olo-slack", 4, true)], 2100, name, &mut id));
        assert!(!again.apps[0].put_off());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(test)]
mod waits {
    use super::*;

    fn tab(what: &str, place: &str, shell: u32) -> Waits {
        Waits { what: what.into(), place: place.into(), shows: vec!["--show-shell".into(), shell.to_string()] }
    }

    fn terminal(waits: Vec<Waits>) -> Attention {
        Attention { app: "olo-terminal".into(), count: waits.len() as u64, urgent: false, waits }
    }

    /// Where an app says what waits where (Terminal's tabs), each is its
    /// own: an Inbox card of its own (its words, its place), its id kept
    /// while the app says it, gone to and put off alone, back when what it
    /// says changes; the app is out of the Inbox once each is seen.
    #[test]
    fn each_thing_waiting_is_its_own() {
        let mut next = 100;
        let mut id = || {
            next += 1;
            next
        };
        let mut w = WaitingApps::load(None);
        let (a, b) = (tab("Claude Code needs your answer", "Fix login", 7), tab("Codex is waiting for you", "Pool", 9));
        assert!(w.update(&[terminal(vec![a.clone(), b.clone()])], 1000, |_| "Terminal".into(), &mut id));
        let ids: Vec<u32> = w.apps[0].waits.iter().map(|x| x.id).collect();
        let cards = w.cards(|_| Vec::new());
        assert_eq!(
            cards.iter().map(|c| (c.id, c.title.as_str(), c.detail.as_str())).collect::<Vec<_>>(),
            [(ids[0], "Claude Code needs your answer", "Fix login"), (ids[1], "Codex is waiting for you", "Pool")]
        );
        assert_eq!(w.apps[0].waits[0].said(), "Claude Code needs your answer, in “Fix login”");
        // Each found by its own id, and the app by its.
        assert_eq!(w.get(ids[1]).map(|x| x.app.as_str()), Some("olo-terminal"));
        // One gone to: out of the Inbox alone.
        assert!(w.later(ids[0]));
        assert_eq!(w.cards(|_| Vec::new()).iter().map(|c| c.id).collect::<Vec<_>>(), [ids[1]]);
        // Said again, the same places: the same ids, the seen one still seen.
        w.update(&[terminal(vec![a.clone(), b.clone()])], 1100, |_| "Terminal".into(), &mut id);
        assert_eq!(w.apps[0].waits.iter().map(|x| (x.id, x.seen)).collect::<Vec<_>>(), [(ids[0], true), (ids[1], false)]);
        // What it says changes (answered, then waiting again): back.
        let a2 = Waits { what: "Claude Code is waiting for you".into(), ..a.clone() };
        w.update(&[terminal(vec![a2.clone(), b.clone()])], 1200, |_| "Terminal".into(), &mut id);
        assert!(!w.apps[0].waits[0].seen);
        // The app gone to: each seen, the app out of the Inbox; a new one
        // brings it back.
        assert!(w.visited_app("olo-terminal"));
        assert!(w.apps[0].put_off() && w.cards(|_| Vec::new()).is_empty());
        w.update(&[terminal(vec![a2, b.clone(), tab("Claude Code is waiting for you", "Docs", 11)])], 1300, |_| "Terminal".into(), &mut id);
        assert_eq!(w.cards(|_| Vec::new()).iter().map(|c| c.detail.as_str()).collect::<Vec<_>>(), ["Docs"]);
        // Nothing waiting: gone.
        assert!(w.update(&[], 1400, |_| "Terminal".into(), &mut id));
        assert!(w.apps.is_empty());
    }
}
