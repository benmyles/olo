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

use olo_protocol::notifications::{Attention, InboxCard};
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
}

impl Waiting {
    /// Put off, and nothing more waiting than when it was.
    pub fn put_off(&self) -> bool {
        self.later.is_some_and(|(count, urgent)| self.count <= count && (!self.urgent || urgent))
    }

    /// "3 waiting in Slack", "1 waiting in Slack", "Slack wants your
    /// attention": the same words on every surface.
    pub fn said(&self) -> String {
        match self.count {
            0 => format!("{} wants your attention", self.name),
            n => format!("{n} waiting in {}", self.name),
        }
    }

    /// Beside the app's name: "3 waiting", "Wants attention".
    pub fn short(&self) -> String {
        match self.count {
            0 => "Wants attention".to_owned(),
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
                }
                None => {
                    // (Put off before a restart: still put off, for as much.)
                    let later = self.remembered.iter().find(|w| w.app == badge.app).and_then(|w| w.later).map(|(count, urgent)| (count.min(badge.count), urgent && badge.urgent));
                    self.apps.push(Waiting { id: id(), app: badge.app.clone(), name: name(&badge.app), count: badge.count, urgent: badge.urgent, time: now, later });
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

    pub fn get(&self, id: u32) -> Option<&Waiting> {
        self.apps.iter().find(|w| w.id == id)
    }

    /// Put `id` off (Later): out of the Inbox until more is waiting.
    /// Whether it changed.
    pub fn later(&mut self, id: u32) -> bool {
        let Some(w) = self.apps.iter_mut().find(|w| w.id == id) else { return false };
        Self::put_off(w) && {
            self.save();
            true
        }
    }

    fn put_off(w: &mut Waiting) -> bool {
        let was = w.later;
        w.later = Some((w.count, w.urgent));
        w.later != was
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

    /// Deskbar's Inbox cards: one for each app not put off, the urgent
    /// first, then the newest (`icons`: the names an app's icon may
    /// have).
    pub fn cards(&self, icons: impl Fn(&Waiting) -> Vec<String>) -> Vec<InboxCard> {
        let mut shown: Vec<&Waiting> = self.apps.iter().filter(|w| !w.put_off()).collect();
        shown.sort_by_key(|w| (!w.urgent, std::cmp::Reverse((w.time, w.id))));
        shown
            .into_iter()
            .map(|w| InboxCard {
                id: w.id,
                app: w.name.clone(),
                icons: icons(w),
                title: w.said(),
                detail: match w.count {
                    0 | 1 => format!("Open {} to see it.", w.name),
                    _ => format!("Open {} to see them.", w.name),
                },
                attention: true,
                time: w.time,
                fade_minutes: None,
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
        Attention { app: app.into(), count, urgent }
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
