//! The notification history: what came, newest last, kept across a
//! restart in `history.json` under Olo's state directory, with each
//! note's picture (a sender's face, a page's icon) beside it, small, in
//! `pictures/`. Transient notes and apps that asked not to be kept never
//! get here. Each is unread until it's clicked, opened or acted on; with
//! fading on, each goes once it has faded away.

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

/// One notification as the history keeps it (and as `List` gives it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entry {
    /// Not a notification: what its app says is waiting in it (its count
    /// and urgency), one such item an app ([`History::update_attention`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<olo_protocol::notifications::Attention>,
    /// Of an app's item, how much has been seen (read, dismissed, or one
    /// of its notifications was): unread again past that.
    #[serde(skip_serializing_if = "is_zero")]
    pub seen: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub seen_urgent: bool,
    /// Red, for drawing ([`History::mark_waiting`]); not kept.
    #[serde(skip)]
    pub waiting: bool,
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
}

impl Entry {
    pub fn from_note(n: &Note, unread: bool) -> Entry {
        Entry {
            attention: None,
            seen: 0,
            seen_urgent: false,
            waiting: false,
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
            show: n.show.clone(),
            picture: None,
            pinned: false,
            snoozed_until: None,
            archived: 0,
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
    /// ([`History::take_left`]: faded away, or pushed out by newer ones):
    /// for the archive.
    left: Vec<Entry>,
}

/// How many the archive keeps, and for how many days.
pub const ARCHIVE_KEEP: usize = 1000;
pub const ARCHIVE_DAYS: u64 = 30;

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Whether `e` is of the app that `item` is the waiting item of (`item`'s
/// attention names its desktop entry, as launcher badges do; its `app`
/// is the app's name): by the entry `e` names, else by its name.
fn of_app(e: &Entry, item: &Entry) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(".desktop").to_ascii_lowercase();
    let Some(a) = &item.attention else { return false };
    let app = norm(&a.app);
    e.id == item.id || e.desktop_entry.as_deref().is_some_and(|d| norm(d) == app) || norm(&e.app) == app || (!item.app.is_empty() && norm(&e.app) == norm(&item.app))
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

/// What waits in an app, in words: "3 waiting", "2 more waiting" (beside
/// what was seen or said already), or, with no count, that it wants
/// attention.
fn waiting_words(count: u64, more: bool) -> String {
    match (count, more) {
        (0, _) => "Wants your attention".to_owned(),
        (n, false) => format!("{n} waiting"),
        (n, true) => format!("{n} more waiting"),
    }
}

impl History {
    /// What apps say is waiting in them (`badges`: each app's count and
    /// urgency on its Deskbar row), kept as one item an app, beside its
    /// notifications: there while the app says so, gone when it stops.
    /// What of it has been seen stays seen (`Entry::seen`): the item is
    /// unread again only for more than that, or for turning urgent.
    /// Whether anything changed.
    pub fn update_attention(&mut self, badges: &[olo_protocol::notifications::Attention], now: u64, name: impl Fn(&str) -> String, mut id: impl FnMut() -> u32) -> bool {
        let mut changed = false;
        if self.entries.iter().any(|e| e.attention.as_ref().is_some_and(|a| !badges.iter().any(|b| b.app == a.app))) {
            self.entries_mut().retain(|e| e.attention.as_ref().is_none_or(|a| badges.iter().any(|b| b.app == a.app)));
            changed = true;
        }
        for badge in badges {
            let at = self.entries.iter().position(|e| e.attention.as_ref().is_some_and(|a| a.app == badge.app));
            if at.is_some_and(|i| self.entries[i].attention.as_ref() == Some(badge)) {
                continue;
            }
            let title = waiting_words(badge.count, false);
            if let Some(i) = at {
                let e = &mut self.entries_mut()[i];
                e.seen = e.seen.min(badge.count);
                e.seen_urgent &= badge.urgent;
                let unread = badge.count > e.seen || (badge.urgent && !e.seen_urgent);
                if unread && !e.unread {
                    e.time = now;
                }
                e.unread = unread;
                e.summary = title;
                e.attention = Some(badge.clone());
            } else {
                self.put(Entry {
                    id: id(),
                    app: name(&badge.app),
                    desktop_entry: Some(badge.app.clone()),
                    summary: title,
                    time: now,
                    unread: true,
                    attention: Some(badge.clone()),
                    ..Default::default()
                });
            }
            changed = true;
        }
        self.mark_waiting();
        changed
    }

    /// Whether `e` is red: its app says something is waiting in it (the
    /// app's own item, and its notifications while it does). Blue
    /// otherwise: news.
    pub fn waiting(&self, e: &Entry) -> bool {
        e.attention.is_some() || self.entries.iter().any(|s| of_app(e, s))
    }

    /// Every entry marked red or not ([`History::waiting`], kept in
    /// `Entry::waiting` for drawing): as the apps that wait change, and
    /// as a notification comes. (Nothing written where nothing differs:
    /// the entries are shared with the windows drawing them.)
    pub fn mark_waiting(&mut self) {
        let items: Vec<Entry> = self.entries.iter().filter(|e| e.attention.is_some()).cloned().collect();
        let red = |e: &Entry| e.attention.is_some() || items.iter().any(|s| of_app(e, s));
        if self.entries.iter().any(|e| e.waiting != red(e)) {
            for e in self.entries_mut() {
                e.waiting = red(e);
            }
        }
    }

    /// How much of what waits in an app (its item `s`) its unread
    /// notifications don't already say: the count past what was seen,
    /// less those notifications (each is one of the things waiting).
    /// None: nothing more, the item is covered by them.
    fn more(&self, s: &Entry) -> Option<(u64, bool)> {
        let a = s.attention.as_ref()?;
        let notes = self.entries.iter().filter(|e| e.attention.is_none() && e.unread && !e.snoozed() && of_app(e, s)).count() as u64;
        let fresh = a.count.saturating_sub(s.seen);
        if fresh > notes {
            return Some((fresh - notes, notes > 0 || s.seen > 0));
        }
        // (Waiting without a count: said unless a notification says it.)
        (a.count == 0 && a.urgent && !s.seen_urgent && notes == 0).then_some((0, false))
    }

    /// Whether `e` counts as unread and in sight: not snoozed, and not an
    /// app's item its notifications cover.
    fn in_inbox(&self, e: &Entry) -> bool {
        e.unread && !e.snoozed() && (e.attention.is_none() || self.more(e).is_some())
    }

    /// `id` was read: what waits in its app has been seen, as much as
    /// there is now (it is unread again only for more).
    pub fn acknowledge(&mut self, id: u32) {
        let Some(e) = self.entries.iter().find(|e| e.id == id) else { return };
        let Some(at) = self.entries.iter().position(|s| if e.attention.is_some() { s.id == id } else { of_app(e, s) }) else { return };
        let s = &mut self.entries_mut()[at];
        if let Some(a) = &s.attention {
            (s.seen, s.seen_urgent, s.unread) = (a.count, a.urgent, false);
        }
    }

    /// The person went to a window of app `app_id`, however they got
    /// there (a click on its Deskbar row, a key, a notification, the
    /// window itself): what it said is read, and what waits in it has
    /// been seen (Ben, 2026-11-18: "what matters is i went to the app's
    /// window or tab that it was about"). Not what is about one place in
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
    /// `fade_minutes`: how long a notification takes to fade away).
    pub fn inbox(&self, icons: impl Fn(&Entry) -> Vec<String>, fade_minutes: Option<u32>) -> Vec<olo_protocol::notifications::InboxCard> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| self.in_inbox(e)).collect();
        entries.sort_by_key(|e| std::cmp::Reverse((e.time, e.id)));
        entries
            .into_iter()
            .map(|e| {
                let (title, detail) = match self.more(e) {
                    Some((n, more)) => (waiting_words(n, more), format!("Open {} to see {}.", e.app, if n == 1 { "it" } else { "them" })),
                    // (One line: a card has no room for paragraphs.)
                    None => (e.summary.clone(), e.text.split_whitespace().collect::<Vec<_>>().join(" ")),
                };
                olo_protocol::notifications::InboxCard {
                    id: e.id,
                    app: e.app.clone(),
                    icons: icons(e),
                    title,
                    detail,
                    attention: self.waiting(e),
                    time: e.time,
                    fade_minutes: fade_minutes.filter(|_| e.attention.is_none() && !e.pinned),
                }
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
        for e in &mut entries {
            e.live = false;
            e.shown &= e.pinned;
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

    /// What has left by itself since this was last asked (faded away,
    /// pushed out by newer ones): for the archive.
    pub fn take_left(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.left)
    }

    /// Into the archive (this list is one): `gone`, as they were when
    /// they left the history, each read, closed at its app and neither
    /// pinned nor snoozed any more, marked with when (`now`). Never what
    /// an app says is waiting in it (that isn't a notification). The
    /// same one archived again (after an Undo) takes its own place. Then
    /// the oldest go: those archived over `ARCHIVE_DAYS` ago, and those
    /// past `ARCHIVE_KEEP`. Whether anything changed.
    pub fn archive(&mut self, gone: impl IntoIterator<Item = Entry>, now: u64) -> bool {
        let mut changed = false;
        for mut e in gone.into_iter().filter(|e| e.attention.is_none()) {
            e.unread = false;
            e.live = false;
            e.shown = false;
            e.pinned = false;
            e.snoozed_until = None;
            e.waiting = false;
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
        // The oldest go (to the archive: `take_left`), but those pinned
        // or snoozed.
        let mut extra = entries.len().saturating_sub(KEEP);
        if extra > 0 {
            let mut left = Vec::new();
            entries.retain(|e| {
                let goes = extra > 0 && !e.pinned && !e.snoozed();
                if goes {
                    extra -= 1;
                    left.push(e.clone());
                }
                !goes
            });
            self.left.append(&mut left);
        }
        self.mark_waiting();
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

    /// Unread and in sight (not snoozed).
    pub fn unread(&self) -> u32 {
        self.entries.iter().filter(|e| self.in_inbox(e)).count() as u32
    }

    /// The apps with notes unread and in sight, each with its newest
    /// (a website's are its browser's), in the order they last had one.
    pub fn unread_apps(&self) -> Vec<olo_protocol::NoteApp> {
        let mut apps: Vec<olo_protocol::NoteApp> = Vec::new();
        for e in self.entries.iter().filter(|e| e.unread && !e.snoozed() && e.attention.is_none()) {
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
        for e in self.entries_mut() {
            e.unread = false;
        }
        true
    }

    /// Mark the newest entry with `id` read (or unread); whether it changed.
    pub fn set_read(&mut self, id: u32, read: bool) -> bool {
        match self.entries.iter().rposition(|e| e.id == id) {
            Some(at) if self.entries[at].unread == read => {
                let e = &mut self.entries_mut()[at];
                e.unread = !read;
                // (An app's item made unread again: all of it.)
                if !read {
                    (e.seen, e.seen_urgent) = (0, false);
                }
                if read {
                    self.acknowledge(id);
                }
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

    /// What has faded away by `now` (not what is on screen) leaves, for
    /// the archive (`take_left`); the ones still open at their apps, to
    /// tell them.
    pub fn fade_out(&mut self, settings: &olo_protocol::NotificationSettings, now: u64) -> Option<Vec<u32>> {
        let faded = |e: &Entry| e.attention.is_none() && !e.shown && !e.pinned && !e.snoozed() && settings.faded(now.saturating_sub(e.time));
        if !self.entries.iter().any(faded) {
            return None;
        }
        let live = self.entries.iter().filter(|e| faded(e) && e.live).map(|e| e.id).collect();
        // (Their pictures stay: they go on to the archive, `take_left`.)
        let gone = self.take_out(|_, e| !faded(e));
        self.left.extend(gone.into_iter().map(|(_, e)| e));
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

    /// Red and blue are one list (Ben, 2026-11-17: "we also need to unify
    /// the red/activity type notifications and the info/blue type ones"):
    /// an app that says something is waiting has its items red; what its
    /// notifications already say isn't said twice; what was seen stays
    /// seen until there is more.
    #[test]
    fn what_waits_in_an_app_and_its_notifications_are_one_list() {
        use olo_protocol::notifications::Attention;
        let mut h = History::load(None);
        let mut next = 100;
        let mut wait = |h: &mut History, badges: &[(&str, u64, bool)]| {
            let badges: Vec<Attention> = badges.iter().map(|(app, count, urgent)| Attention { app: (*app).into(), count: *count, urgent: *urgent }).collect();
            h.update_attention(
                &badges,
                50,
                |app| if app == "olo-slack" { "Slack".into() } else { app.to_owned() },
                || {
                    next += 1;
                    next
                },
            )
        };
        let cards = |h: &History| h.inbox(|_| Vec::new(), Some(30)).into_iter().map(|c| (c.app, c.title, c.attention, c.fade_minutes.is_some())).collect::<Vec<_>>();
        let note = |id: u32, app: &str, entry: &str, summary: &str| Entry {
            id,
            app: app.into(),
            desktop_entry: Some(entry.into()),
            summary: summary.into(),
            unread: true,
            live: true,
            time: 60 + id as u64,
            ..Default::default()
        };
        let (red, blue) = (true, false);

        // News alone: blue, and it fades.
        h.put(note(1, "Mail", "olo-mail", "Invoice"));
        assert_eq!(cards(&h), [("Mail".into(), "Invoice".into(), blue, true)]);
        assert!(!h.entries[0].waiting);

        // Slack says two wait, with no notification of its own: one red
        // item says so, and doesn't fade.
        assert!(wait(&mut h, &[("olo-slack", 2, false)]));
        assert_eq!(cards(&h), [("Mail".into(), "Invoice".into(), blue, true), ("Slack".into(), "2 waiting".into(), red, false)]);
        assert_eq!(h.unread(), 2);

        // A notification of one of them: red too (its app is waiting),
        // and the item says only what it doesn't.
        h.put(note(2, "Slack", "olo-slack", "Dana: lunch?"));
        assert_eq!(cards(&h)[0], ("Slack".into(), "Dana: lunch?".into(), red, true));
        assert!(cards(&h).contains(&("Slack".into(), "1 more waiting".into(), red, false)));
        assert!(h.entries.iter().find(|e| e.id == 2).unwrap().waiting);
        // And of the other: both are said, the item not at all.
        h.put(note(3, "Slack", "olo-slack", "Sam: ok"));
        assert_eq!(cards(&h).iter().filter(|c| c.0 == "Slack").map(|c| c.1.as_str()).collect::<Vec<_>>(), ["Sam: ok", "Dana: lunch?"]);
        assert_eq!(h.unread(), 3, "Mail's and Slack's two");

        // One dismissed: seen, and Slack's item doesn't come back for it,
        // though Slack still counts two.
        assert!(h.set_read(3, true));
        assert_eq!(cards(&h).iter().filter(|c| c.0 == "Slack").map(|c| c.1.as_str()).collect::<Vec<_>>(), ["Dana: lunch?"]);
        assert!(h.set_read(2, true));
        assert_eq!(cards(&h).iter().filter(|c| c.0 == "Slack").count(), 0);
        // (The same count said again changes nothing.)
        assert!(!wait(&mut h, &[("olo-slack", 2, false)]));
        // A third, with no notification: one more.
        assert!(wait(&mut h, &[("olo-slack", 3, false)]));
        assert!(cards(&h).contains(&("Slack".into(), "1 more waiting".into(), red, false)));
        // Fewer, then as many again: that one is new.
        let item = h.entries.iter().find(|e| e.attention.is_some()).unwrap().id;
        assert!(h.set_read(item, true));
        assert!(wait(&mut h, &[("olo-slack", 1, false)]));
        assert_eq!(cards(&h).iter().filter(|c| c.0 == "Slack").count(), 0);
        assert!(wait(&mut h, &[("olo-slack", 2, false)]));
        assert!(cards(&h).contains(&("Slack".into(), "1 more waiting".into(), red, false)));

        // Slack dealt with in Slack: its item goes, and news of it is blue.
        h.put(note(4, "Slack", "olo-slack", "Reminder"));
        assert!(h.entries.iter().find(|e| e.id == 4).unwrap().waiting);
        assert!(wait(&mut h, &[]));
        assert_eq!(cards(&h), [("Slack".into(), "Reminder".into(), blue, true), ("Mail".into(), "Invoice".into(), blue, true)]);
        assert!(h.entries.iter().all(|e| !e.waiting));

        // An app that only wants attention (no count): said once, unless
        // a notification says it.
        assert!(wait(&mut h, &[("olo-terminal", 0, true)]));
        assert!(cards(&h).iter().any(|c| c.1 == "Wants your attention" && c.2));
        h.put(note(5, "Terminal", "olo-terminal", "Claude needs you"));
        assert_eq!(cards(&h).iter().filter(|c| c.2).map(|c| c.1.as_str()).collect::<Vec<_>>(), ["Claude needs you"]);
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
        use olo_protocol::notifications::Attention;
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

        // What waits in an app has been seen by going to it, as much as
        // there was: unread again only for more.
        let wait = |h: &mut History, count: u64| {
            let mut next = 200;
            h.update_attention(
                &[Attention { app: "org.telegram.desktop".into(), count, urgent: false }],
                50,
                |_| "Telegram".into(),
                || {
                    next += 1;
                    next
                },
            );
        };
        wait(&mut h, 3);
        let item = |h: &History| h.entries.iter().find(|e| e.attention.is_some()).cloned().unwrap();
        assert!(item(&h).unread);
        assert_eq!(h.visited_app("org.telegram.desktop"), vec![item(&h).id]);
        assert_eq!((item(&h).unread, item(&h).seen), (false, 3));
        wait(&mut h, 3);
        assert!(!item(&h).unread, "nothing new");
        wait(&mut h, 5);
        assert!(item(&h).unread, "two more");
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

    /// What leaves the history goes to the archive (Ben, 2026-11-18:
    /// "after a notification has been faded out / dismissed / cleared
    /// let's put it in an archived section"): what fades away and what
    /// newer ones push out are handed on (`take_left`); in the archive
    /// each is read, closed, unpinned, and marked with when; an Undo
    /// takes it out again; and the archive itself lets its oldest go.
    #[test]
    fn what_leaves_the_history_is_archived() {
        let settings = olo_protocol::NotificationSettings { fade: true, fade_minutes: 60, ..Default::default() };
        let mut h = History::load(None);
        let mut a = History::load(None);
        let at = |id: u32, time: u64| Entry { time, ..entry(id, "One") };
        h.put(at(1, 100));
        h.put(Entry { pinned: true, ..at(2, 100) });
        h.put(at(3, 5000));
        // An hour on, the first has faded away: out of the history, handed on.
        assert_eq!(h.fade_out(&settings, 4000), Some(vec![1]));
        let left = h.take_left();
        assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1]);
        assert!(h.take_left().is_empty(), "handed on once");
        assert!(a.archive(left, 4000));
        let e = &a.entries[0];
        assert_eq!((e.id, e.unread, e.live, e.pinned, e.archived), (1, false, false, false, 4000));
        // Cleared (or forgotten, or dismissed): taken out and archived;
        // an Undo puts it back and takes it out of the archive.
        let gone = h.take_out(|_, e| e.id != 3);
        assert!(a.archive(gone.iter().map(|(_, e)| e.clone()), 4100));
        assert_eq!(a.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1, 3]);
        h.put_back(gone);
        assert!(a.unarchive(&[(3, 5000)]) && !a.unarchive(&[(3, 5000)]));
        assert_eq!(a.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1]);
        // Archived again (a Redo): once, in its own place.
        let again = h.take_out(|_, e| e.id != 3);
        assert!(a.archive(again.iter().map(|(_, e)| e.clone()), 4200));
        assert!(a.archive(again.into_iter().map(|(_, e)| e), 4300));
        assert_eq!(a.entries.iter().map(|e| (e.id, e.archived)).collect::<Vec<_>>(), vec![(1, 4000), (3, 4300)]);
        // What an app says is waiting in it is not a notification: never archived.
        let item = Entry { attention: Some(olo_protocol::notifications::Attention { app: "olo-slack".into(), count: 2, urgent: false }), ..at(9, 4300) };
        assert!(!a.archive(vec![item], 4400));
        // Newer ones push the oldest out of the history: handed on too.
        for id in 10..(10 + KEEP as u32 + 1) {
            h.put(at(id, 6000));
        }
        let pushed: Vec<u32> = h.take_left().iter().map(|e| e.id).collect();
        assert_eq!(pushed, vec![10, 11], "the oldest two, not the pinned one before them");
        assert!(h.entries.iter().any(|e| e.id == 2 && e.pinned));
        // The archive keeps so long and so many.
        assert!(!a.prune_archive(4000 + ARCHIVE_DAYS * 86400), "thirty days: still there");
        assert!(a.prune_archive(4000 + ARCHIVE_DAYS * 86400 + 1));
        assert_eq!(a.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![3], "the one archived over thirty days ago went");
        let many: Vec<Entry> = (100..(100 + ARCHIVE_KEEP as u32 + 5)).map(|id| at(id, 7000)).collect();
        assert!(a.archive(many, 4300 + 86400));
        assert_eq!(a.entries.len(), ARCHIVE_KEEP);
        assert_eq!((a.entries[0].id, a.entries.last().unwrap().id), (105, 100 + ARCHIVE_KEEP as u32 + 4), "the oldest went first (the one there before, and five of these)");
        // Written with when it was archived; one that isn't says nothing of it.
        let mut kept = History::load(None);
        kept.archive(vec![at(1, 100)], 4000);
        let text = serde_json::to_string(&*kept.entries).unwrap();
        assert!(text.contains("\"archived\":4000"), "{text}");
        assert!(!serde_json::to_string(&at(1, 100)).unwrap().contains("archived"), "nothing said of one that isn't");
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
