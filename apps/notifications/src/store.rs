//! The notification history: what came, newest last, kept across a
//! restart in `history.json` under Olo's state directory, with each
//! note's picture (a sender's face, a page's icon) beside it, small, in
//! `pictures/`. Transient notes and apps that asked not to be kept never
//! get here.
//!
//! A notification is **unread** until the person has seen to it
//! (clicked, opened, acted on, gone to what it's about); then **read**,
//! fading with the time since; then **cleared**: out of this list and
//! into the archive (the Cleared list), which says why each left
//! ([`Why`]). Nothing unread leaves by itself: what is tidied away
//! after a while is what was read (Ben, 2026-11-18: "what fades out vs
//! goes to history/archive vs goes away vs automatically gets marked
//! as read vs what doesn't etc. is all very confusing").

use std::{
    path::{Path, PathBuf},
    sync::Arc,
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

/// Why a notification left the list for the Cleared one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// The person cleared it (its ×, Clear, Clear All).
    Cleared,
    /// Read, and older than the time read ones are kept.
    Tidied,
    /// Its app took it back (seen to elsewhere, or no longer true).
    Withdrawn,
    /// The same again from its app took its place.
    Replaced,
    /// The list was full: the oldest go.
    Overflow,
}

impl Why {
    /// How a Cleared row says it, before when: "Cleared", "Tidied
    /// away", "Withdrawn by Telegram", "Replaced by a newer one",
    /// "Made room for newer ones".
    pub fn said(self, app: &str) -> String {
        match self {
            Why::Cleared => "Cleared".to_owned(),
            Why::Tidied => "Tidied away".to_owned(),
            Why::Withdrawn if app.is_empty() => "Withdrawn".to_owned(),
            Why::Withdrawn => format!("Withdrawn by {app}"),
            Why::Replaced => "Replaced by a newer one".to_owned(),
            Why::Overflow => "Made room for newer ones".to_owned(),
        }
    }
}

/// Now, in seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One notification as the history keeps it (and as `List` gives it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entry {
    /// Read from a list kept before 2026-11-18, where what an app said
    /// was waiting in it was an item among the notifications: such an
    /// item isn't a notification (`crate::waiting`), and is dropped as
    /// the list is read. Never written.
    #[serde(skip_serializing)]
    pub(crate) attention: Option<serde_json::Value>,
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
    /// Urgent (`Note::urgent`): how it came, said to agents and kept
    /// for when it shows again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub urgent: bool,
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
    /// Its card stays until it's dealt with (no time of its own: urgent,
    /// an error, or its sender asked): on screen again after a restart,
    /// while it's unread.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stays: bool,
    pub look: NoteLook,
    /// A website's: the browser it came through.
    pub via: Option<String>,
    /// What shows the thing it's about, once it's no longer open at its
    /// app: arguments for the app's own program (`x-olo-show`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<String>,
    /// Its picture, kept small in `pictures/` (a PNG; it may take a moment
    /// to appear).
    pub picture: Option<String>,
    /// Pinned: it stays put, never fading away nor going with Clear All
    /// or a banner group's Close All, until unpinned.
    pub pinned: bool,
    /// Snoozed until then (seconds since the epoch): out of sight and
    /// not counted, then back as new, as a banner and unread.
    pub snoozed_until: Option<u64>,
    /// In the archive: when it went there (seconds since the epoch).
    #[serde(skip_serializing_if = "is_zero")]
    pub archived: u64,
    /// In the archive: why it left the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
    /// When it was read (seconds since the epoch; 0: not known, or not
    /// read): a read one fades, and is tidied away, by the time since.
    #[serde(skip_serializing_if = "is_zero")]
    pub read_at: u64,
}

impl Entry {
    pub fn from_note(n: &Note, unread: bool) -> Entry {
        Entry {
            attention: None,
            id: n.id,
            app: n.app_name.clone(),
            desktop_entry: n.desktop_entry.clone(),
            icon: n.icon.clone(),
            summary: n.summary.clone(),
            text: markup::plain(&n.body),
            body: n.body.clone(),
            actions: n.actions.clone(),
            kind: n.kind.into(),
            urgent: n.urgent,
            progress: n.progress,
            time: n.time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            unread,
            live: true,
            shown: false,
            stays: n.timeout.is_none(),
            look: n.look.clone(),
            via: n.via.clone(),
            show: n.show.clone(),
            picture: None,
            pinned: false,
            snoozed_until: None,
            archived: 0,
            why: None,
            read_at: 0,
        }
    }

    /// What its fading counts from: when it was read, if that's known
    /// (one read long after it came isn't tidied away the moment it's
    /// read), else when it came.
    pub fn since(&self) -> u64 {
        self.time.max(self.read_at)
    }

    /// Read (`now`), or unread again.
    pub fn set_unread(&mut self, unread: bool, now: u64) {
        self.unread = unread;
        self.read_at = if unread { 0 } else { now };
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
            urgent: self.urgent,
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
            show: self.show.clone(),
        }
    }
}

/// Where a note's picture is kept.
fn picture_path(dir: &Path, id: u32) -> PathBuf {
    dir.join("pictures").join(format!("{id}.png"))
}

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first. Shared: the History window lays out what it holds
    /// without copying 200 notes at each step of a resize; a change made
    /// while it holds them copies them once (`entries_mut`).
    pub entries: Arc<Vec<Entry>>,
    path: Option<PathBuf>,
    /// What has left by itself since it was last asked for
    /// ([`History::take_left`]: read and tidied away, or pushed out by
    /// newer ones), each with why: for the archive.
    left: Vec<(Entry, Why)>,
}

/// How many the archive keeps, and for how many days.
pub const ARCHIVE_KEEP: usize = 1000;
pub const ARCHIVE_DAYS: u64 = 30;

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The names a window's app may go by (`WindowInfo::app_id`) for the app
/// a notification is from, by its desktop entry and its name: lower case.
pub fn app_ids(desktop_entry: Option<&str>, app_name: &str) -> Vec<String> {
    // (As it's written, and without a `.desktop` at its end: that may be
    // the file's ending, or the end of the app's own id, as Telegram's
    // "org.telegram.desktop" is.)
    let mut ids: Vec<String> = Vec::new();
    for e in desktop_entry.iter().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()) {
        if let Some(short) = e.strip_suffix(".desktop").filter(|s| !s.is_empty()) {
            ids.push(short.to_owned());
        }
        ids.push(e);
    }
    if !app_name.is_empty() {
        ids.push(app_name.to_lowercase());
        ids.push(app_name.to_lowercase().replace(' ', "-"));
        // Ours by name ("StyledEdit" is olo.styled-edit).
        for (bin, name, _) in olo_xdg::apps::OUR_APPS {
            if name.eq_ignore_ascii_case(app_name) {
                ids.push(bin.replacen("olo-", "olo.", 1));
            }
        }
    }
    ids
}

/// Whether a window of app `app_id` is one of the app's that goes by
/// `ids` ([`app_ids`]): the same, or the same after its last dot
/// ("org.telegram.desktop" and "desktop" aside, `olo.terminal` is
/// `olo-terminal`'s, `com.slack.Slack` is `slack`'s).
pub fn is_app(ids: &[String], app_id: &str) -> bool {
    let a = app_id.to_lowercase();
    let dashed = a.replace('.', "-");
    ids.iter().any(|i| *i == a || *i == dashed || a.rsplit('.').next() == Some(i.as_str()) || i.rsplit('.').next() == Some(a.as_str()))
}

/// The host a site's notification is from, as `visited_site` is told it:
/// lower case, without `www.`.
fn bare_host(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    h.strip_prefix("www.").map(str::to_owned).unwrap_or(h)
}

impl History {
    /// The person went to a window of app `app_id`, however they got
    /// there (a click on its Deskbar row, a key, a notification, the
    /// window itself): what it said is read (Ben, 2026-11-18: "what
    /// matters is i went to the app's window or tab that it was about";
    /// what waits in it is seen too: `WaitingApps::visited_app`). Not what is about one place in
    /// the app that may not be the one showing: a notification that says
    /// what shows it (`Entry::show`: a tab of Terminal's; its app says
    /// when that has been looked at), or a website's through its browser
    /// (`Entry::via`: [`History::visited_site`]). The ids read.
    pub fn visited_app(&mut self, app_id: &str) -> Vec<u32> {
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|e| e.unread && !e.snoozed() && e.via.is_none() && e.show.is_empty())
            .filter(|e| is_app(&app_ids(e.desktop_entry.as_deref(), &e.app), app_id))
            .map(|e| e.id)
            .collect();
        ids.into_iter().filter(|&id| self.set_read(id, true)).collect()
    }

    /// The person is looking at a page of `host` in a browser (its tab
    /// showing, in the window in front): that site's notifications are
    /// read. A site's own host or one under it ("mail.google.com" for
    /// Gmail's; "app.slack.com" for slack.com's). The ids read.
    pub fn visited_site(&mut self, host: &str) -> Vec<u32> {
        let host = bare_host(host);
        if host.is_empty() {
            return Vec::new();
        }
        let of = |e: &Entry| {
            // (A site we know is named for itself, its host beside it as
            // its source; another is named by its host.)
            let from = e.look.source.as_deref().map(bare_host).unwrap_or_else(|| bare_host(&e.app));
            !from.is_empty() && from.contains('.') && (host == from || host.ends_with(&format!(".{from}")))
        };
        let ids: Vec<u32> = self.entries.iter().filter(|e| e.unread && !e.snoozed() && e.via.is_some() && of(e)).map(|e| e.id).collect();
        ids.into_iter().filter(|&id| self.set_read(id, true)).collect()
    }

    /// What is unread and not snoozed, newest first, as Deskbar's Inbox
    /// widget shows it (`icons`: the names an entry's icon may have;
    /// `fade_minutes`: how long a card's dot takes to drain to its
    /// floor). Each a notification: blue.
    pub fn inbox(&self, icons: impl Fn(&Entry) -> Vec<String>, fade_minutes: Option<u32>) -> Vec<olo_protocol::notifications::InboxCard> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.unread && !e.snoozed()).collect();
        entries.sort_by_key(|e| std::cmp::Reverse((e.time, e.id)));
        entries
            .into_iter()
            .map(|e| olo_protocol::notifications::InboxCard {
                id: e.id,
                app: e.app.clone(),
                icons: icons(e),
                title: e.summary.clone(),
                // (One line: a card has no room for paragraphs.)
                detail: e.text.split_whitespace().collect::<Vec<_>>().join(" "),
                attention: false,
                time: e.time,
                fade_minutes: fade_minutes.filter(|_| !e.pinned),
            })
            .collect()
    }

    /// (The session's own: an agent screen's centre keeps its history
    /// in the screen's folder, never in the person's.)
    pub fn path() -> PathBuf {
        olo_xdg::session_state_dir().join("notifications").join("history.json")
    }

    /// Where the archive is kept: what has left the history (faded away,
    /// dismissed, cleared, forgotten), for looking back at (Ben,
    /// 2026-11-18: "after a notification has been faded out / dismissed
    /// / cleared let's put it in an archived section in the
    /// notifications app and have a way to see the archived ones when
    /// needed"). A list like the history, read back the same way.
    pub fn archive_path() -> PathBuf {
        olo_xdg::session_state_dir().join("notifications").join("archive.json")
    }

    /// The history from `path` (None: in memory only, for tests). After a
    /// restart nothing in it is still open at its app, and only what is
    /// pinned is still on screen. Pictures nothing names any more go, off
    /// the UI thread.
    pub fn load(path: Option<PathBuf>) -> History {
        let mut entries: Vec<Entry> = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // (What an app said was waiting in it, kept as an item before
        // 2026-11-18: not a notification, `Entry::attention`.)
        entries.retain(|e| e.attention.is_none());
        for e in &mut entries {
            e.live = false;
            // (Back on screen: what was there to stay. One with a time of
            // its own had it.)
            e.shown &= e.pinned || (e.stays && e.unread);
        }
        History { entries: Arc::new(entries), path, left: Vec::new() }
    }

    /// The entries, to change: copied first if anything else holds them.
    /// The pictures no entry of `lists` names any more (in the folder
    /// beside the first one's file) are removed, off this thread: at
    /// start, for what went while nothing ran.
    pub fn sweep_pictures(lists: &[&History]) {
        let Some(dir) = lists.first().and_then(|h| h.path.as_ref()).and_then(|p| p.parent()).map(|d| d.join("pictures")) else { return };
        let named: std::collections::HashSet<PathBuf> = lists.iter().flat_map(|h| h.entries.iter()).filter_map(|e| e.picture.as_ref().map(PathBuf::from)).collect();
        std::thread::spawn(move || {
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if !named.contains(&f.path()) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        });
    }

    /// What has left by itself since this was last asked (read and
    /// tidied away, pushed out by newer ones), each with why: for the
    /// archive.
    pub fn take_left(&mut self) -> Vec<(Entry, Why)> {
        std::mem::take(&mut self.left)
    }

    /// Into the archive (this list is one): `gone`, as they were when
    /// they left the history, each read, closed at its app and neither
    /// pinned nor snoozed any more, marked with when (`now`). Never what
    /// an app says is waiting in it (that isn't a notification). The
    /// same one archived again (after an Undo) takes its own place. Then
    /// the oldest go: those archived over `ARCHIVE_DAYS` ago, and those
    /// past `ARCHIVE_KEEP`. Whether anything changed.
    pub fn archive(&mut self, gone: impl IntoIterator<Item = (Entry, Why)>, now: u64) -> bool {
        let mut changed = false;
        for (mut e, why) in gone {
            e.why = Some(why);
            e.unread = false;
            e.live = false;
            e.shown = false;
            e.pinned = false;
            e.snoozed_until = None;
            e.archived = now;
            let entries = self.entries_mut();
            entries.retain(|x| (x.id, x.time) != (e.id, e.time));
            entries.push(e);
            changed = true;
        }
        self.prune_archive(now) || changed
    }

    /// The archive's oldest go (their pictures with them): those
    /// archived over `ARCHIVE_DAYS` ago, and those past `ARCHIVE_KEEP`.
    /// Whether any went.
    pub fn prune_archive(&mut self, now: u64) -> bool {
        let old = |e: &Entry| now.saturating_sub(e.archived) > ARCHIVE_DAYS * 86400;
        let over = self.entries.len().saturating_sub(ARCHIVE_KEEP);
        let aged = self.entries.iter().skip(over).any(old);
        if over == 0 && !aged {
            return false;
        }
        let gone = self.take_out(|i, e| i >= over && !old(e));
        gone.iter().for_each(|(_, e)| Self::drop_picture(e));
        true
    }

    /// Out of the archive again, as an Undo puts them back in the
    /// history (`which`: each one's id and time). Their pictures stay.
    pub fn unarchive(&mut self, which: &[(u32, u64)]) -> bool {
        !self.take_out(|_, e| !which.contains(&(e.id, e.time))).is_empty()
    }

    pub fn entries_mut(&mut self) -> &mut Vec<Entry> {
        Arc::make_mut(&mut self.entries)
    }

    /// Keep `picture` for the newest entry with `id`: scaled down and
    /// written as a PNG off the UI thread.
    pub fn keep_picture(&mut self, id: u32, picture: &Picture) {
        let Some(dir) = self.path.as_ref().and_then(|p| p.parent()).map(Path::to_owned) else { return };
        let Some(e) = self.entries_mut().iter_mut().rev().find(|e| e.id == id) else { return };
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
        let Ok(bytes) = serde_json::to_vec(&*self.entries) else { return };
        olo_kit::store::save_bytes(path, bytes);
    }

    /// Add `e`, or update the entry with its id still open (a replacement).
    pub fn put(&mut self, e: Entry) {
        let entries = self.entries_mut();
        match entries.iter_mut().rev().find(|x| x.id == e.id && x.live) {
            Some(old) => {
                // A replacement keeps being pinned, or snoozed.
                let (unread, pinned, snoozed_until) = (old.unread || e.unread, old.pinned || e.pinned, old.snoozed_until.or(e.snoozed_until));
                *old = Entry { unread, pinned, snoozed_until, ..e };
            }
            None => entries.push(e),
        }
        // Past what's kept, the oldest go (to the archive: `take_left`),
        // but those pinned or snoozed: the read ones first, and unread
        // ones only when there's nothing read left to go.
        let extra = entries.len().saturating_sub(KEEP);
        if extra > 0 {
            let may_go = |e: &Entry| !e.pinned && !e.snoozed();
            let read: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| may_go(e) && !e.unread).map(|(i, _)| i).take(extra).collect();
            let unread: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| may_go(e) && e.unread).map(|(i, _)| i).take(extra - read.len()).collect();
            let mut left = Vec::new();
            let mut i = 0;
            entries.retain(|e| {
                let goes = read.contains(&i) || unread.contains(&i);
                i += 1;
                if goes {
                    left.push((e.clone(), Why::Overflow));
                }
                !goes
            });
            self.left.append(&mut left);
        }
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Entry> {
        // (Looked for first: a note that isn't there copies nothing.)
        let at = self.entries.iter().rposition(|e| e.id == id && e.live)?;
        Some(&mut self.entries_mut()[at])
    }

    /// The newest entry with `id`, open at its app or not.
    pub fn newest_mut(&mut self, id: u32) -> Option<&mut Entry> {
        let at = self.entries.iter().rposition(|e| e.id == id)?;
        Some(&mut self.entries_mut()[at])
    }

    pub fn newest(&self, id: u32) -> Option<&Entry> {
        self.entries.iter().rev().find(|e| e.id == id)
    }

    /// Unread and in sight (not snoozed): what the bell counts.
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
        if !self.entries.iter().any(|e| e.unread) {
            return false;
        }
        let now = now_secs();
        for e in self.entries_mut().iter_mut().filter(|e| e.unread) {
            e.set_unread(false, now);
        }
        true
    }

    /// Mark the newest entry with `id` read (or unread); whether it changed.
    pub fn set_read(&mut self, id: u32, read: bool) -> bool {
        match self.entries.iter().rposition(|e| e.id == id) {
            Some(at) if self.entries[at].unread == read => {
                self.entries_mut()[at].set_unread(!read, now_secs());
                true
            }
            _ => false,
        }
    }

    /// Forget the entry at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            Self::drop_picture(&self.entries_mut().remove(index));
        }
    }

    /// Take out those `keep` doesn't keep, each with where it was, for
    /// Undo to put back (their pictures stay till then: what is left of
    /// them goes at the next start, nothing naming them).
    pub fn take_out(&mut self, keep: impl Fn(usize, &Entry) -> bool) -> Vec<(usize, Entry)> {
        let mut gone = Vec::new();
        let mut kept = Vec::with_capacity(self.entries.len());
        for (i, e) in Arc::unwrap_or_clone(std::mem::take(&mut self.entries)).into_iter().enumerate() {
            if keep(i, &e) { kept.push(e) } else { gone.push((i, e)) }
        }
        self.entries = Arc::new(kept);
        gone
    }

    /// Put back what `take_out` took (where each was, among what is
    /// there now), closed at its app by now and off screen.
    pub fn put_back(&mut self, gone: Vec<(usize, Entry)>) {
        for (i, mut e) in gone {
            e.live = false;
            e.shown = false;
            let i = i.min(self.entries.len());
            self.entries_mut().insert(i, e);
        }
    }

    /// Forget all but those `keep` says to.
    pub fn retain(&mut self, keep: impl Fn(&Entry) -> bool) {
        let (kept, gone): (Vec<Entry>, Vec<Entry>) = Arc::unwrap_or_clone(std::mem::take(&mut self.entries)).into_iter().partition(|e| keep(e));
        gone.iter().for_each(Self::drop_picture);
        self.entries = Arc::new(kept);
    }

    /// What was read long enough ago to be tidied away by `now` leaves,
    /// for the archive (`take_left`); the ones still open at their apps,
    /// to tell them. Never one that's unread (nothing the person hasn't
    /// seen goes by itself), on screen, pinned or snoozed.
    pub fn fade_out(&mut self, settings: &olo_protocol::NotificationSettings, now: u64) -> Option<Vec<u32>> {
        let faded = |e: &Entry| !e.unread && !e.shown && !e.pinned && !e.snoozed() && settings.faded(now.saturating_sub(e.since()));
        if !self.entries.iter().any(faded) {
            return None;
        }
        let live = self.entries.iter().filter(|e| faded(e) && e.live).map(|e| e.id).collect();
        // (Their pictures stay: they go on to the archive, `take_left`.)
        let gone = self.take_out(|_, e| !faded(e));
        self.left.extend(gone.into_iter().map(|(_, e)| (e, Why::Tidied)));
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
    fn what_the_window_holds_stays_as_it_was() {
        let mut h = History::load(None);
        h.put(entry(1, "One"));
        // The History window's (a step of a resize takes no copy).
        let held = h.entries.clone();
        assert!(h.set_read(1, true));
        assert!(held[0].unread && !h.entries[0].unread);
        // Held by nothing else: changed in place, and a note that isn't
        // there copies nothing.
        drop(held);
        let at = Arc::as_ptr(&h.entries);
        assert!(h.set_read(1, false));
        let held = h.entries.clone();
        assert!(h.get_mut(9).is_none() && !h.set_read(9, true));
        assert!(Arc::ptr_eq(&held, &h.entries));
        drop(held);
        assert_eq!(Arc::as_ptr(&h.entries), at);
    }

    /// The Inbox's cards of notifications: the unread ones, newest first,
    /// each blue (a notification is never red: what waits in an app is
    /// the app's own, `crate::waiting`), a pinned one's dot not draining;
    /// a snoozed or read one isn't among them. And a list kept from
    /// before, with an app's "waiting" item in it, is read without it.
    #[test]
    fn the_inbox_is_what_is_unread() {
        let mut h = History::load(None);
        h.put(Entry { time: 10, app: "Slack".into(), text: "Lunch\nat one?".into(), ..entry(1, "Dana") });
        h.put(Entry { time: 20, pinned: true, ..entry(2, "Build failed") });
        h.put(Entry { time: 30, unread: false, ..entry(3, "Read") });
        h.put(Entry { time: 40, snoozed_until: Some(u64::MAX), ..entry(4, "Later") });
        let cards = h.inbox(|e| vec![e.app.to_lowercase()], Some(60));
        assert_eq!(
            cards.iter().map(|c| (c.id, c.title.as_str(), c.detail.as_str(), c.attention, c.fade_minutes)).collect::<Vec<_>>(),
            [(2, "Build failed", "", false, None), (1, "Dana", "Lunch at one?", false, Some(60))]
        );
        assert_eq!((h.unread(), cards[1].icons.clone()), (2, vec!["slack".to_owned()]));
        // After a restart, what's on screen again: a pinned one, and one
        // that was there to stay and is still unread; not one that had
        // its time, nor one read since.
        let path = PathBuf::from(format!("/var/tmp/olo-notif-shown-{}.json", std::process::id()));
        let on = |id: u32, e: Entry| Entry { shown: true, ..Entry { id, ..e } };
        let was = vec![
            on(1, Entry { pinned: true, ..entry(1, "pinned") }),
            on(2, Entry { stays: true, ..entry(2, "a call") }),
            on(3, entry(3, "had its time")),
            on(4, Entry { stays: true, unread: false, ..entry(4, "seen to") }),
        ];
        std::fs::write(&path, serde_json::to_string(&was).unwrap()).unwrap();
        let back = History::load(Some(path.clone()));
        assert_eq!(back.entries.iter().filter(|e| e.shown).map(|e| e.id).collect::<Vec<_>>(), [1, 2]);
        assert!(back.entries.iter().all(|e| !e.live));
        let _ = std::fs::remove_file(path);
        // An old list's waiting item: dropped as it's read; the rest kept.
        let path = PathBuf::from(format!("/var/tmp/olo-notif-old-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"[{"id":7,"app":"Slack","summary":"3 waiting","unread":true,"attention":{"app":"olo-slack","count":3,"urgent":false},"seen":1},{"id":8,"app":"Slack","summary":"Dana","unread":true}]"#,
        )
        .unwrap();
        let old = History::load(Some(path.clone()));
        assert_eq!(old.entries.iter().map(|e| (e.id, e.summary.as_str())).collect::<Vec<_>>(), [(8, "Dana")]);
        assert!(!serde_json::to_string(&*old.entries).unwrap().contains("attention"));
        let _ = std::fs::remove_file(path);
    }

    /// A notification is read by going to what it is about, however one
    /// gets there (Ben, 2026-11-18: "doesn't matter if it was from
    /// clicking the notification or not what matters is i went to the
    /// app's window or tab that it was about"): an app's by its window
    /// coming to the front; one about a place within its app (a tab of
    /// Terminal's) only when the app says that was looked at; a website's
    /// by its page showing in the browser, not by the browser's window.
    #[test]
    fn going_to_what_it_is_about_reads_it() {
        let mut h = History::load(None);
        let from = |id: u32, app: &str, entry: Option<&str>| Entry { app: app.into(), desktop_entry: entry.map(str::to_owned), ..entry_of(id) };
        fn entry_of(id: u32) -> Entry {
            Entry { id, summary: "News".into(), unread: true, live: true, ..Default::default() }
        }
        h.put(from(1, "Telegram", Some("org.telegram.desktop")));
        h.put(from(2, "Slack", Some("olo-slack")));
        h.put(Entry { show: vec!["--show-shell".into(), "17".into()], ..from(3, "Terminal", Some("olo-terminal")) });
        h.put(from(4, "Terminal", Some("olo-terminal")));
        h.put(Entry { via: Some("Google Chrome".into()), look: NoteLook { source: Some("teams.microsoft.com".into()), ..Default::default() }, ..from(5, "Microsoft Teams", Some("google-chrome")) });
        h.put(Entry { via: Some("Google Chrome".into()), ..from(6, "news.example.org", Some("google-chrome")) });
        h.put(Entry { snoozed_until: Some(u64::MAX), ..from(7, "Telegram", Some("org.telegram.desktop")) });
        let unread = |h: &History| h.entries.iter().filter(|e| e.unread).map(|e| e.id).collect::<Vec<_>>();

        // Another app's window: nothing of these.
        assert!(h.visited_app("olo.tracker").is_empty());
        // Telegram's window: its notification, not the one put away till later.
        assert_eq!(h.visited_app("org.telegram.desktop"), vec![1]);
        assert_eq!(unread(&h), vec![2, 3, 4, 5, 6, 7]);
        // Terminal's window, whatever tab it shows: what it said of itself,
        // not what it said of a tab (Terminal says when that is looked at).
        assert_eq!(h.visited_app("olo.terminal"), vec![4]);
        // Chrome's window: no site's notifications, whatever page it shows.
        assert!(h.visited_app("google-chrome").is_empty());
        // The site's page showing: its own, by its host or one under it.
        assert!(h.visited_site("example.org").is_empty(), "a host above the site's isn't the site");
        assert_eq!(h.visited_site("www.teams.microsoft.com"), vec![5]);
        assert_eq!(h.visited_site("news.example.org"), vec![6]);
        assert!(h.visited_site("").is_empty() && h.visited_site("teams.microsoft.com").is_empty(), "read once");
        // Slack's own window, by the name its window goes by.
        assert_eq!(h.visited_app("olo-slack"), vec![2]);
        assert_eq!(unread(&h), vec![3, 7]);
    }

    /// A window's app is a notification's by its desktop entry or its
    /// name, as each side writes it.
    #[test]
    fn a_windows_app_and_a_notifications() {
        let ids = app_ids(Some("olo-terminal.desktop"), "Terminal");
        assert!(is_app(&ids, "olo.terminal") && is_app(&ids, "olo-terminal") && !is_app(&ids, "olo.tracker"));
        let ids = app_ids(Some("com.slack.Slack"), "Slack");
        assert!(is_app(&ids, "Slack") && is_app(&ids, "com.slack.Slack") && !is_app(&ids, "slackware"));
        let ids = app_ids(None, "Google Chrome");
        assert!(is_app(&ids, "google-chrome") && !is_app(&ids, "chrome-beta"));
        assert!(!is_app(&app_ids(None, ""), "anything") && !is_app(&app_ids(Some(" "), ""), ""));
    }

    /// What leaves the list goes to the Cleared one (the archive; Ben,
    /// 2026-11-18: "after a notification has been faded out / dismissed /
    /// cleared let's put it in an archived section"), each saying why:
    /// what was read and is tidied away, and what newer ones push out,
    /// are handed on (`take_left`); there each is read, closed, unpinned,
    /// and marked with when and why; an Undo takes it out again; and the
    /// archive itself lets its oldest go. Nothing unread is tidied away.
    #[test]
    fn what_leaves_the_history_is_archived() {
        let settings = olo_protocol::NotificationSettings { fade: true, fade_minutes: 60, ..Default::default() };
        let mut h = History::load(None);
        let mut a = History::load(None);
        let at = |id: u32, time: u64| Entry { time, ..entry(id, "One") };
        let read = |id: u32, time: u64| Entry { unread: false, ..at(id, time) };
        h.put(read(1, 100));
        h.put(Entry { pinned: true, ..read(2, 100) });
        h.put(at(3, 5000));
        // (As old as the first, and never seen: it stays, however old.)
        h.put(at(4, 100));
        // An hour on, the first, read, is tidied away: out of the list, handed on.
        assert_eq!(h.fade_out(&settings, 4000), Some(vec![1]));
        let left = h.take_left();
        assert_eq!(left.iter().map(|(e, why)| (e.id, *why)).collect::<Vec<_>>(), vec![(1, Why::Tidied)]);
        assert!(h.take_left().is_empty(), "handed on once");
        assert_eq!(h.fade_out(&settings, 4_000_000), None, "the unread one is there a month on");
        assert!(h.entries.iter().any(|e| e.id == 4 && e.unread));
        assert!(a.archive(left, 4000));
        let e = &a.entries[0];
        assert_eq!((e.id, e.unread, e.live, e.pinned, e.archived, e.why), (1, false, false, false, 4000, Some(Why::Tidied)));
        // Cleared by the person: taken out and archived; an Undo puts it
        // back and takes it out of the archive.
        let cleared = |gone: &[(usize, Entry)]| gone.iter().map(|(_, e)| (e.clone(), Why::Cleared)).collect::<Vec<_>>();
        let gone = h.take_out(|_, e| e.id != 3);
        assert!(a.archive(cleared(&gone), 4100));
        assert_eq!(a.entries.iter().map(|e| (e.id, e.why)).collect::<Vec<_>>(), vec![(1, Some(Why::Tidied)), (3, Some(Why::Cleared))]);
        h.put_back(gone);
        assert!(a.unarchive(&[(3, 5000)]) && !a.unarchive(&[(3, 5000)]));
        assert_eq!(a.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1]);
        assert!(h.entries.iter().any(|e| e.id == 3 && e.unread && e.why.is_none()), "back as it was");
        // Archived again (a Redo): once, in its own place.
        let again = h.take_out(|_, e| e.id != 3);
        assert!(a.archive(cleared(&again), 4200));
        assert!(a.archive(cleared(&again), 4300));
        assert_eq!(a.entries.iter().map(|e| (e.id, e.archived)).collect::<Vec<_>>(), vec![(1, 4000), (3, 4300)]);
        // Newer ones push the oldest out of the list, handed on too: the
        // read ones first, an unread one only when no read one is left;
        // never the pinned one.
        h.put(read(5, 6000));
        // (With the pinned one, the old unread one and the read one: two too many.)
        for id in 10..(10 + KEEP as u32 - 1) {
            h.put(at(id, 6000));
        }
        let pushed: Vec<(u32, Why)> = h.take_left().iter().map(|(e, why)| (e.id, *why)).collect();
        assert_eq!(pushed, vec![(5, Why::Overflow), (4, Why::Overflow)], "the read one first, though it's newer; then the oldest unread");
        assert!(h.entries.iter().any(|e| e.id == 2 && e.pinned) && h.entries.len() == KEEP);
        // The archive keeps so long and so many.
        assert!(!a.prune_archive(4000 + ARCHIVE_DAYS * 86400), "thirty days: still there");
        assert!(a.prune_archive(4000 + ARCHIVE_DAYS * 86400 + 1));
        assert_eq!(a.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![3], "the one archived over thirty days ago went");
        let many: Vec<(Entry, Why)> = (100..(100 + ARCHIVE_KEEP as u32 + 5)).map(|id| (at(id, 7000), Why::Cleared)).collect();
        assert!(a.archive(many, 4300 + 86400));
        assert_eq!(a.entries.len(), ARCHIVE_KEEP);
        assert_eq!((a.entries[0].id, a.entries.last().unwrap().id), (105, 100 + ARCHIVE_KEEP as u32 + 4), "the oldest went first (the one there before, and five of these)");
        // Written with when and why it was archived; one that isn't says nothing of either.
        let mut kept = History::load(None);
        kept.archive(vec![(at(1, 100), Why::Withdrawn)], 4000);
        let text = serde_json::to_string(&*kept.entries).unwrap();
        assert!(text.contains("\"archived\":4000") && text.contains("\"why\":\"withdrawn\""), "{text}");
        let plain = serde_json::to_string(&at(1, 100)).unwrap();
        assert!(!plain.contains("archived") && !plain.contains("why") && !plain.contains("read_at"), "nothing said of one that isn't: {plain}");
        // How each reason is said on a Cleared row.
        assert_eq!(
            [Why::Cleared, Why::Tidied, Why::Withdrawn, Why::Replaced, Why::Overflow].map(|w| w.said("Telegram")),
            ["Cleared", "Tidied away", "Withdrawn by Telegram", "Replaced by a newer one", "Made room for newer ones"]
        );
        assert_eq!(Why::Withdrawn.said(""), "Withdrawn");
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
        // (Read: when is kept; unread again: forgotten.)
        assert!(h.entries[1].read_at > 4000 && h.entries[1].since() == h.entries[1].read_at);
        assert!(h.set_read(2, false) && h.unread() == 3);
        assert_eq!((h.entries[1].read_at, h.entries[1].since()), (0, 4000));
        let settings = olo_protocol::NotificationSettings { fade_minutes: 60, ..Default::default() };
        // Unread, nothing goes, however old.
        assert_eq!(h.fade_out(&settings, 1_000_000), None, "nothing unread is tidied away");
        // Read at 3000 (long after it came), the first goes an hour after
        // that, not an hour after it came; it was open: its app hears.
        // Not the one still on screen, read or not.
        for e in h.entries_mut().iter_mut().filter(|e| e.id != 2) {
            e.set_unread(false, 3000);
        }
        assert_eq!(h.fade_out(&settings, 4700), None, "an hour after it came, not yet an hour after it was read");
        assert_eq!(h.fade_out(&settings, 6600), Some(vec![1]));
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
        // (All read, long ago.)
        for e in h.entries_mut() {
            e.unread = false;
        }
        let settings = olo_protocol::NotificationSettings { fade_minutes: 60, ..Default::default() };
        assert_eq!(h.fade_out(&settings, 100_000), Some(vec![3]), "only the one neither pinned nor snoozed is tidied away");
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

    /// Taken out (Clear All, Forget) and put back (Undo): each where it
    /// was, closed at its app, and what came since kept after.
    #[test]
    fn taken_out_then_put_back() {
        let mut h = History::load(None);
        for i in 1..=5 {
            h.put(Entry { live: true, ..entry(i, "") });
        }
        let gone = h.take_out(|i, _| i % 2 == 1);
        assert_eq!(h.entries.iter().map(|e| e.id).collect::<Vec<_>>(), [2, 4]);
        h.put(entry(6, "new"));
        h.put_back(gone);
        assert_eq!(h.entries.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
        assert!(h.entries.iter().filter(|e| e.id % 2 == 1).all(|e| !e.live && !e.shown));
    }
}
