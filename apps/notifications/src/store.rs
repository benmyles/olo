//! The notification history: what came, newest last, kept across a
//! restart in `history.json` under Tabula's state directory (pictures are
//! not kept; the icon name is). Transient notes and apps that asked not
//! to be kept never get here.

use std::{path::PathBuf, time::UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    markup::{self, Span},
    server::{Kind, Note},
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
    /// Not yet seen in the history.
    pub unread: bool,
    /// Still open at the app (its actions can still be run): false once it
    /// closed, expired from the app's side, or after a restart.
    pub live: bool,
    /// On screen as a card right now.
    pub shown: bool,
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
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first.
    pub entries: Vec<Entry>,
    path: Option<PathBuf>,
}

impl History {
    pub fn path() -> PathBuf {
        tabula_kit::state_dir().join("notifications").join("history.json")
    }

    /// The history from `path` (None: in memory only, for tests). After a
    /// restart nothing in it is still open at its app.
    pub fn load(path: Option<PathBuf>) -> History {
        let mut entries: Vec<Entry> =
            path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        for e in &mut entries {
            e.live = false;
            e.shown = false;
        }
        History { entries, path }
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
            self.entries.drain(..extra);
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| e.id == id && e.live)
    }

    pub fn unread(&self) -> u32 {
        self.entries.iter().filter(|e| e.unread).count() as u32
    }

    pub fn mark_read(&mut self) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            changed |= std::mem::take(&mut e.unread);
        }
        changed
    }

    /// Forget the entry at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
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
        let dir = std::env::temp_dir().join(format!("tabula-history-{}", std::process::id()));
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
}
