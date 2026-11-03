//! The notification history: what came, newest last, kept across a
//! restart in `history.json` under Olo's state directory, with each
//! note's picture (a sender's face, a page's icon) beside it, small, in
//! `pictures/`. Transient notes and apps that asked not to be kept never
//! get here. Each is unread until it's clicked, opened or acted on; with
//! fading on, each goes once it has faded away.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use olo_protocol::NoteLook;
use serde::{Deserialize, Serialize};

use crate::{
    markup::{self, Span},
    server::{Kind, Note, Picture},
};

/// How many are kept.
pub const KEEP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum KindName {
    #[default]
    Information,
    Important,
    Error,
    Progress,
}

impl From<KindName> for Kind {
    fn from(k: KindName) -> Self {
        match k {
            KindName::Information => Kind::Information,
            KindName::Important => Kind::Important,
            KindName::Error => Kind::Error,
            KindName::Progress => Kind::Progress,
        }
    }
}

impl From<Kind> for KindName {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Information => KindName::Information,
            Kind::Important => KindName::Important,
            Kind::Error => KindName::Error,
            Kind::Progress => KindName::Progress,
        }
    }
}

/// One notification as the history keeps it (and as `List` gives it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entry {
    pub id: u32,
    pub app: String,
    pub desktop_entry: Option<String>,
    pub icon: Option<String>,
    pub summary: String,
    pub body: Vec<Span>,
    /// The body as plain text (for agents and search).
    pub text: String,
    /// (key, label).
    pub actions: Vec<(String, String)>,
    pub kind: KindName,
    pub progress: Option<f32>,
    /// Seconds since the epoch.
    pub time: u64,
    /// Not yet read: clicked (on screen or in the history), opened or
    /// acted on. Dismissing one reads it too.
    pub unread: bool,
    /// Still open at the app (its actions can still be run): false once it
    /// closed, expired from the app's side, or after a restart.
    pub live: bool,
    /// On screen as a card right now.
    pub shown: bool,
    pub look: NoteLook,
    /// A website's: the browser it came through.
    pub via: Option<String>,
    /// Its picture, kept small in `pictures/` (a PNG; it may take a moment
    /// to appear).
    pub picture: Option<String>,
    /// Pinned: it stays put, never fading away nor going with Clear All
    /// or a banner group's Close All, until unpinned.
    pub pinned: bool,
    /// Snoozed until then (seconds since the epoch): out of sight and
    /// not counted, then back as new, as a banner and unread.
    pub snoozed_until: Option<u64>,
}

impl Entry {
    pub fn from_note(n: &Note, unread: bool) -> Entry {
        Entry {
            id: n.id,
            app: n.app_name.clone(),
            desktop_entry: n.desktop_entry.clone(),
            icon: n.icon.clone(),
            summary: n.summary.clone(),
            text: markup::plain(&n.body),
            body: n.body.clone(),
            actions: n.actions.clone(),
            kind: n.kind.into(),
            progress: n.progress,
            time: n.time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            unread,
            live: true,
            shown: false,
            look: n.look.clone(),
            via: n.via.clone(),
            picture: None,
            pinned: false,
            snoozed_until: None,
        }
    }

    /// Out of sight for now (snoozed).
    pub fn snoozed(&self) -> bool {
        self.snoozed_until.is_some()
    }

    /// It as a notification to show again (back from snoozing): its
    /// actions only while still open at its app.
    pub fn to_note(&self, timeout: Option<std::time::Duration>) -> Note {
        Note {
            id: self.id,
            replaces: false,
            app_name: self.app.clone(),
            desktop_entry: self.desktop_entry.clone(),
            icon: self.icon.clone(),
            picture: None,
            summary: self.summary.clone(),
            body: self.body.clone(),
            actions: if self.live { self.actions.clone() } else { Vec::new() },
            kind: self.kind.into(),
            progress: self.progress,
            timeout,
            resident: false,
            transient: false,
            sound: None,
            category: None,
            reply: None,
            time: std::time::SystemTime::now(),
            look: self.look.clone(),
            via: self.via.clone(),
        }
    }
}

/// Where a note's picture is kept.
fn picture_path(dir: &Path, id: u32) -> PathBuf {
    dir.join("pictures").join(format!("{id}.png"))
}

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first.
    pub entries: Vec<Entry>,
    path: Option<PathBuf>,
}

impl History {
    pub fn path() -> PathBuf {
        olo_xdg::state_dir().join("notifications").join("history.json")
    }

    /// The history from `path` (None: in memory only, for tests). After a
    /// restart nothing in it is still open at its app, and only what is
    /// pinned is still on screen. Pictures nothing names any more go, off
    /// the UI thread.
    pub fn load(path: Option<PathBuf>) -> History {
        let mut entries: Vec<Entry> = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        for e in &mut entries {
            e.live = false;
            e.shown &= e.pinned;
        }
        if let Some(dir) = path.as_ref().and_then(|p| p.parent()).map(|d| d.join("pictures")) {
            let named: std::collections::HashSet<PathBuf> = entries.iter().filter_map(|e| e.picture.as_ref().map(PathBuf::from)).collect();
            std::thread::spawn(move || {
                for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    if !named.contains(&f.path()) {
                        let _ = std::fs::remove_file(f.path());
                    }
                }
            });
        }
        History { entries, path }
    }

    /// Keep `picture` for the newest entry with `id`: scaled down and
    /// written as a PNG off the UI thread.
    pub fn keep_picture(&mut self, id: u32, picture: &Picture) {
        let Some(dir) = self.path.as_ref().and_then(|p| p.parent()).map(Path::to_owned) else { return };
        let Some(e) = self.entries.iter_mut().rev().find(|e| e.id == id) else { return };
        let path = picture_path(&dir, id);
        e.picture = Some(path.to_string_lossy().into_owned());
        let p = picture.clone();
        std::thread::spawn(move || {
            let Some(img) = image::RgbaImage::from_raw(p.width as u32, p.height as u32, p.rgba) else { return };
            let max = crate::server::PICTURE_MAX;
            let img = if img.width() > max || img.height() > max { image::DynamicImage::ImageRgba8(img).thumbnail(max, max).to_rgba8() } else { img };
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&dir));
            let tmp = path.with_extension("png.new");
            if img.save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        });
    }

    /// An entry's picture goes with it.
    fn drop_picture(e: &Entry) {
        if let Some(p) = e.picture.clone() {
            std::thread::spawn(move || std::fs::remove_file(p));
        }
    }

    /// Write it out (whole), off the UI thread.
    pub fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        // (Compact: it's long.)
        let Ok(bytes) = serde_json::to_vec(&self.entries) else { return };
        olo_ui::store::save_bytes(path, bytes);
    }

    /// Add `e`, or update the entry with its id still open (a replacement).
    pub fn put(&mut self, e: Entry) {
        match self.entries.iter_mut().rev().find(|x| x.id == e.id && x.live) {
            Some(old) => {
                // A replacement keeps being pinned, or snoozed.
                let (unread, pinned, snoozed_until) = (old.unread || e.unread, old.pinned || e.pinned, old.snoozed_until.or(e.snoozed_until));
                *old = Entry { unread, pinned, snoozed_until, ..e };
            }
            None => self.entries.push(e),
        }
        // The oldest go, but those pinned or snoozed.
        let mut extra = self.entries.len().saturating_sub(KEEP);
        if extra > 0 {
            self.entries.retain(|e| {
                let goes = extra > 0 && !e.pinned && !e.snoozed();
                if goes {
                    extra -= 1;
                    Self::drop_picture(e);
                }
                !goes
            });
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| e.id == id && e.live)
    }

    /// The newest entry with `id`, open at its app or not.
    pub fn newest_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| e.id == id)
    }

    pub fn newest(&self, id: u32) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.id == id)
    }

    /// Unread and in sight (not snoozed).
    pub fn unread(&self) -> u32 {
        self.entries.iter().filter(|e| e.unread && !e.snoozed()).count() as u32
    }

    /// The apps with notes unread and in sight, each with its newest
    /// (a website's are its browser's), in the order they last had one.
    pub fn unread_apps(&self) -> Vec<olo_protocol::NoteApp> {
        let mut apps: Vec<olo_protocol::NoteApp> = Vec::new();
        for e in self.entries.iter().filter(|e| e.unread && !e.snoozed()) {
            let name = e.via.clone().unwrap_or_else(|| e.app.clone());
            let newest = (e.time, e.id);
            let a = match apps.iter().position(|a| a.desktop_entry == e.desktop_entry && a.name == name) {
                Some(i) => &mut apps[i],
                None => {
                    apps.push(olo_protocol::NoteApp { desktop_entry: e.desktop_entry.clone(), name, newest, ..Default::default() });
                    apps.last_mut().unwrap()
                }
            };
            a.newest = a.newest.max(newest);
            a.count += 1;
            // (Pinned ones don't fade.)
            if !e.pinned {
                a.fading.push(e.time);
            }
        }
        apps.sort_by_key(|a| std::cmp::Reverse(a.newest));
        apps
    }

    /// Mark them all read; whether any wasn't.
    pub fn mark_read(&mut self) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            changed |= std::mem::take(&mut e.unread);
        }
        changed
    }

    /// Mark the newest entry with `id` read (or unread); whether it changed.
    pub fn set_read(&mut self, id: u32, read: bool) -> bool {
        match self.entries.iter_mut().rev().find(|e| e.id == id) {
            Some(e) if e.unread == read => {
                e.unread = !read;
                true
            }
            _ => false,
        }
    }

    /// Forget the entry at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            Self::drop_picture(&self.entries.remove(index));
        }
    }

    /// Forget all but those `keep` says to.
    pub fn retain(&mut self, keep: impl Fn(&Entry) -> bool) {
        let (kept, gone): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut self.entries).into_iter().partition(|e| keep(e));
        gone.iter().for_each(Self::drop_picture);
        self.entries = kept;
    }

    /// Forget what has faded away by `now` (not what is on screen); the
    /// ones forgotten that were still open at their apps, to tell them.
    pub fn fade_out(&mut self, settings: &olo_protocol::NotificationSettings, now: u64) -> Option<Vec<u32>> {
        let faded = |e: &Entry| !e.shown && !e.pinned && !e.snoozed() && settings.faded(now.saturating_sub(e.time));
        if !self.entries.iter().any(faded) {
            return None;
        }
        let live = self.entries.iter().filter(|e| faded(e) && e.live).map(|e| e.id).collect();
        self.retain(|e| !faded(e));
        Some(live)
    }
}

/// The notifications still active (a banner up, or unread and not pinned
/// in the history) that a new one, `id`, from `app` saying `summary` and
/// `text` repeats (Preferences ▸ Notifications ▸ one of each): the
/// earlier ones it takes the place of.
pub fn same_as(history: &History, cards: &[&Note], app: &str, summary: &str, text: &str, id: u32) -> Vec<u32> {
    let mut out: Vec<u32> = cards.iter().filter(|n| n.id != id && n.app_name == app && n.summary == summary && markup::plain(&n.body) == text).map(|n| n.id).collect();
    for e in history.entries.iter().filter(|e| e.id != id && e.app == app && e.summary == summary && e.text == text && !e.pinned && (e.unread || e.shown)) {
        if !out.contains(&e.id) {
            out.push(e.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, summary: &str) -> Entry {
        Entry { id, app: "App".into(), summary: summary.into(), unread: true, live: true, ..Default::default() }
    }

    #[test]
    fn one_of_each() {
        let mut h = History::load(None);
        let with = |id: u32, summary: &str, text: &str| Entry { text: text.into(), ..entry(id, summary) };
        h.put(with(1, "Build done", "olo built"));
        h.put(Entry { unread: false, ..with(2, "Build done", "olo built") });
        h.put(Entry { pinned: true, ..with(3, "Build done", "olo built") });
        h.put(with(4, "Build done", "other built"));
        h.put(Entry { app: "Other".into(), ..with(5, "Build done", "olo built") });
        // The unread one of the same app, title and text (not the one read
        // and gone from the screen, the pinned one, another text or app).
        assert_eq!(same_as(&h, &[], "App", "Build done", "olo built", 9), vec![1]);
        // One read but still on screen counts; not itself.
        h.get_mut(2).unwrap().shown = true;
        assert_eq!(same_as(&h, &[], "App", "Build done", "olo built", 1), vec![2]);
    }

    #[test]
    fn keeps_replaces_and_persists() {
        let dir = std::env::temp_dir().join(format!("olo-history-{}", std::process::id()));
        let path = dir.join("history.json");
        let mut h = History::load(Some(path.clone()));
        h.put(entry(1, "one"));
        h.put(entry(2, "two"));
        // A replacement updates in place.
        h.put(Entry { unread: false, ..entry(1, "one again") });
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0].summary, "one again");
        assert!(h.entries[0].unread, "still unread: it was never seen");
        assert_eq!(h.unread(), 2);
        assert!(h.mark_read() && h.unread() == 0);
        for i in 0..KEEP as u32 + 5 {
            h.put(entry(100 + i, "many"));
        }
        assert_eq!(h.entries.len(), KEEP);
        // Saved off-thread: write it here and read it back.
        h.save();
        let mut back = None;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let l = History::load(Some(path.clone()));
            if l.entries.len() == KEEP {
                back = Some(l);
                break;
            }
        }
        let mut back = back.expect("saved");
        assert!(back.entries.iter().all(|e| !e.live), "nothing is open after a restart");
        assert!(back.get_mut(100).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_and_faded() {
        let mut h = History::load(None);
        h.put(Entry { time: 1000, ..entry(1, "old") });
        h.put(Entry { time: 4000, ..entry(2, "new") });
        h.put(Entry { time: 900, shown: true, ..entry(3, "on screen") });
        assert!(h.set_read(2, true) && !h.set_read(2, true));
        assert_eq!(h.unread(), 2);
        assert!(h.set_read(2, false) && h.unread() == 3);
        let settings = olo_protocol::NotificationSettings { fade_minutes: 60, ..Default::default() };
        assert_eq!(h.fade_out(&settings, 4000), None, "nothing an hour old yet");
        // An hour after the first: it goes (it was open: its app hears),
        // not the one still on screen.
        assert_eq!(h.fade_out(&settings, 4600), Some(vec![1]));
        assert_eq!(h.entries.iter().map(|e| e.id).collect::<Vec<_>>(), [2, 3]);
        let off = olo_protocol::NotificationSettings { fade: false, ..settings };
        assert_eq!(h.fade_out(&off, 100_000), None);
    }

    #[test]
    fn pinned_and_snoozed_stay() {
        let mut h = History::load(None);
        h.put(Entry { time: 1000, pinned: true, ..entry(1, "pinned") });
        h.put(Entry { time: 1000, snoozed_until: Some(9000), ..entry(2, "snoozed") });
        h.put(Entry { time: 1000, ..entry(3, "neither") });
        // Snoozed: not counted until it is back.
        assert_eq!(h.unread(), 2);
        let settings = olo_protocol::NotificationSettings { fade_minutes: 60, ..Default::default() };
        assert_eq!(h.fade_out(&settings, 100_000), Some(vec![3]), "only the one neither pinned nor snoozed fades");
        // A replacement keeps both.
        h.put(Entry { summary: "pinned again".into(), ..entry(1, "") });
        h.put(entry(2, "snoozed again"));
        assert!(h.entries[0].pinned && h.entries[0].summary == "pinned again");
        assert_eq!(h.entries[1].snoozed_until, Some(9000));
        // Nor do they go when the history is full.
        for i in 0..KEEP as u32 + 5 {
            h.put(entry(100 + i, "many"));
        }
        assert_eq!(h.entries.len(), KEEP);
        assert_eq!(h.entries[..2].iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2]);
        // Back to a note to show again: its actions only while open.
        let mut e = Entry { actions: vec![("default".into(), "Open".into())], kind: KindName::Error, ..entry(4, "back") };
        assert_eq!(e.to_note(None).actions.len(), 1);
        assert_eq!(e.to_note(None).kind, Kind::Error);
        e.live = false;
        assert!(e.to_note(None).actions.is_empty());
    }
}
