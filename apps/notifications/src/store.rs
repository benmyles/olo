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
        olo_kit::state_dir().join("notifications").join("history.json")
    }

    /// The history from `path` (None: in memory only, for tests). After a
    /// restart nothing in it is still open at its app. Pictures nothing
    /// names any more go, off the UI thread.
    pub fn load(path: Option<PathBuf>) -> History {
        let mut entries: Vec<Entry> =
            path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        for e in &mut entries {
            e.live = false;
            e.shown = false;
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

    /// Write it out (atomically), off the UI thread.
    pub fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        let Ok(text) = serde_json::to_string(&self.entries) else { return };
        std::thread::spawn(move || {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.new");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        });
    }

    /// Add `e`, or update the entry with its id still open (a replacement).
    pub fn put(&mut self, e: Entry) {
        match self.entries.iter_mut().rev().find(|x| x.id == e.id && x.live) {
            Some(old) => {
                let unread = old.unread || e.unread;
                *old = Entry { unread, ..e };
            }
            None => self.entries.push(e),
        }
        if self.entries.len() > KEEP {
            let extra = self.entries.len() - KEEP;
            for e in self.entries.drain(..extra) {
                Self::drop_picture(&e);
            }
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| e.id == id && e.live)
    }

    pub fn unread(&self) -> u32 {
        self.entries.iter().filter(|e| e.unread).count() as u32
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
        let faded = |e: &Entry| !e.shown && settings.faded(now.saturating_sub(e.time));
        if !self.entries.iter().any(faded) {
            return None;
        }
        let live = self.entries.iter().filter(|e| faded(e) && e.live).map(|e| e.id).collect();
        self.retain(|e| !faded(e));
        Some(live)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, summary: &str) -> Entry {
        Entry { id, app: "App".into(), summary: summary.into(), unread: true, live: true, ..Default::default() }
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
}
