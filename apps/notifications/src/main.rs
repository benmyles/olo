//! olo-notifications: the session's notification service. Apps'
//! notifications (org.freedesktop.Notifications: notify-send, browsers,
//! chat and mail) appear in the main display's lower right corner,
//! grouped by app under its name as on Haiku: the app's icon or picture,
//! the summary in bold, the body (bold, italics and links kept), a bar
//! for progress, its actions as buttons and, when it can be answered, a
//! reply field. An error's title starts with a stop sign, and it stays.
//! A click on a card is its default action (the app gets an
//! activation token first, so its window comes forward) or brings the
//! app's window forward; the close box dismisses it; it goes by itself
//! after a while, not while the pointer is over it, into the history.
//!
//! The history (Deskbar's bell, or `--history`) keeps what came, across a
//! restart; what is still open at its app can be acted on from there. Do
//! Not Disturb (by hand, for a while, or on a schedule) keeps everything
//! but errors off the screen and quiet. Per-app choices and Do Not
//! Disturb live in `notifications.json` (Preferences ▸ Notifications).
//! `com.olodesktop.Notifications1` and the command line (`--list`, `--invoke`,
//! `--dismiss`, `--read`, `--clear`, `--dnd`) give agents the same.
//!
//! Only the real session runs the service (a test or nested session must
//! not take the user's notifications); `--bus ADDRESS` serves a private
//! bus. On olo-kit: what it keeps (the cards on screen, the history, the
//! settings) is one model the app's looper and its two windows (the
//! banners, the History window, each a view on a looper of its own) share
//! behind a lock; whoever changes it tells the windows.

mod banners;
mod cli;
mod glance;
mod gtk;
mod history;
mod look;
mod markup;
mod pictures;
mod server;
mod sound;
mod startup;
mod store;
mod waiting;
mod web;

use olo_sync::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_kit::{Bounds, LayerKeyboard, LayerShellLayer, Menu, Painter, Pixels, WindowCx, WindowHandle, WindowOptions};
use olo_kit_base::{metrics as m, watch::Watch};
use olo_protocol::{NotificationSettings, NotificationState};
use server::{Bus, Control, In, Note, Reason};
use store::{Entry, History};

const APP_ID: &str = "olo.notifications";
/// A card's width (logical): the Deskbar's, so banners line up under
/// it at the screen's right edge, in a column that is usually free.
pub const WIDTH: f32 = m::DESKBAR_WIDTH;
/// Between groups, and from the display's edges.
pub const GAP: f32 = 8.0;
/// At most this many at once; the rest wait their turn.
pub const SHOWN: usize = 4;
/// How long a click waits for its activation token before the action
/// goes without one.
const TOKEN_WAIT: Duration = Duration::from_millis(500);

enum Msg {
    In(In),
    Attention(Vec<olo_protocol::notifications::Attention>),
    /// The person has been at a window of this app for a moment
    /// (`follow_visits`).
    Visited(String),
    /// The window in front changed: the person is in no app for sure
    /// until they've stayed in one (`Visited`).
    Left,
    /// Something may have expired (or Do Not Disturb begun or ended, or
    /// the settings or a share changed).
    Tick,
    /// An action's activation token came (or none could be made).
    Token(u64, Option<String>),
    /// Icons were found: drawn again.
    Refresh,
    /// Look for cards whose time is up (asked for then, `expire_at`).
    Expire(Instant),
}

pub struct Card {
    pub note: Note,
    /// When it goes (None: when dismissed).
    pub until: Option<Instant>,
    /// The picture it was sent with.
    pub picture: Option<pictures::Source>,
    /// A reply has been begun in its field (the banners' view keeps the
    /// field): it stays while one has.
    pub replying: bool,
}

/// An action chosen, waiting for the activation token to send with it.
struct Pending {
    id: u32,
    key: String,
    /// Say it closed, after the action.
    close: bool,
    since: Instant,
}

pub struct Notifications {
    next_id: Arc<std::sync::atomic::AtomicU32>,
    bus: Arc<Mutex<Option<Bus>>>,
    /// The model itself, for the windows it opens.
    me: std::sync::Weak<Mutex<Notifications>>,
    /// Something the windows show changed: they're told once whoever
    /// holds the lock is done ([`with`]).
    dirty: bool,
    /// The banners' layer surface.
    win: Option<WindowHandle<banners::Banners>>,
    /// Oldest first.
    pub cards: Vec<Card>,
    /// How many of them are on screen: up to SHOWN, as many as fit.
    pub fits: usize,
    /// The card under the pointer: it stays while it is.
    pub hovered: Option<u32>,
    /// When the cards are next looked at for one whose time is up.
    expiry: Option<Instant>,
    pending: HashMap<u64, Pending>,
    next_tag: u64,
    proxy: olo_kit::Proxy,
    pub settings: NotificationSettings,
    /// When `notifications.json` was last read.
    settings_read: Option<SystemTime>,
    pub history: History,
    /// What apps say is waiting in them (red): not notifications.
    pub waiting: waiting::WaitingApps,
    /// Who sent each notification still open at its app (its id: the
    /// sender's unique name on the bus), to know when that app has gone
    /// ([`Notifications::sender_gone`]). Not kept: after a restart none is
    /// open at its app.
    senders: HashMap<u32, String>,
    /// What has left the history (faded away, dismissed, cleared,
    /// forgotten): the History window's Archived view.
    pub archive: History,
    /// Edit ▸ Undo and Redo in the History window: entries forgotten
    /// (Clear All, Forget) put back.
    pub steps: olo_kit::undo::History<Gone>,
    /// Per-instance 128-bit random prefix for `ClearMany` batch ids so no
    /// batch id is ever issued twice across service restarts.
    batch_prefix: String,
    /// Monotonic per-instance batch counter paired with `batch_prefix`.
    next_batch_seq: u64,
    /// Bounded ring of recent `ClearMany` batches, independent of History's
    /// Undo/Redo stack so unrelated History edits never cancel Deskbar's Undo.
    clear_batches: VecDeque<ClearBatch>,
    /// `NotificationClosed` signals queued while holding the model lock and
    /// emitted only after `Mutex<Notifications>` is released ([`with`]).
    pending_closes: Vec<(u32, Reason)>,
    /// The History window, and what it shows.
    history_win: Option<WindowHandle<history::HistoryWindow>>,
    pub view: history::HistoryState,
    /// `List`'s answer, shared with the bus thread.
    snapshot: Arc<Mutex<String>>,
    /// `ListArchived`'s.
    archived_snapshot: Arc<Mutex<String>>,
    /// What Deskbar was last told.
    told: Option<NotificationState>,
    /// The screen is being shared (`SharingState`).
    sharing: bool,
    /// The app whose window the person is in (its app id), once they've
    /// stayed there a moment (`follow_visits`); None between windows.
    in_front: Option<String>,
    /// The settings file and the screen shares, watched.
    _watches: Vec<Watch>,
}

/// Maximum number of recent `ClearMany` batches retained for receipt Undo.
const MAX_CLEAR_BATCHES: usize = 32;

/// One `ClearMany` batch kept by the service independently of History's Undo
/// stack so unrelated History edits never invalidate Deskbar's receipt Undo.
#[derive(Debug, Clone)]
pub struct ClearBatch {
    pub id: String,
    pub entries: Vec<(usize, store::Entry)>,
}

/// Generate a 128-bit random hex prefix once per service instance so batch ids
/// never repeat across restarts, crashes, or clock jumps.
fn generate_batch_prefix() -> String {
    let mut bytes = [0u8; 16];
    // SAFETY: `bytes` is a valid writable buffer of 16 bytes.
    let n = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if n != bytes.len() as isize {
        static FALLBACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let seq = FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let a = (nanos as u64) ^ (u64::from(std::process::id()) << 32) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let b = ((nanos >> 64) as u64).wrapping_add(seq.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        bytes[..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..].copy_from_slice(&b.to_le_bytes());
    }
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// What Undo takes back in the History window (and Deskbar's receipt).
#[derive(Debug, Clone)]
pub enum Gone {
    /// Entries forgotten, each with where it was; `batch` links a ClearMany
    /// step when it is still on top of History's stack; `only_eligible` keeps
    /// Redo from clearing items pinned or snoozed since.
    Forgot { entries: Vec<(usize, store::Entry)>, batch: Option<String>, only_eligible: bool },
    /// Put back (Redo forgets them again): each by its entry key.
    Back { keys: Vec<store::EntryKey>, only_eligible: bool },
    /// Entries deleted from the archive, each with where it was.
    Deleted(Vec<(usize, store::Entry)>),
    /// Put back in the archive (Redo deletes them again).
    Undeleted(Vec<store::EntryKey>),
}

/// The local time of day, in minutes.
fn local_minute() -> u16 {
    // SAFETY: localtime_r writes into the zeroed struct we own.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour * 60 + tm.tm_min) as u16
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Notifications {
    fn bus(&self, f: impl FnOnce(&Bus)) {
        if let Some(b) = self.bus.lock().unwrap().as_ref() {
            f(b);
        }
    }

    /// Queue a `NotificationClosed` signal to be emitted after releasing the
    /// model lock ([`with`]) on the bus signal worker.
    pub fn queue_closed(&mut self, id: u32, reason: Reason) {
        self.pending_closes.push((id, reason));
    }

    /// Read `notifications.json` again if it changed.
    fn refresh_settings(&mut self) {
        let modified = std::fs::metadata(NotificationSettings::path()).and_then(|m| m.modified()).ok();
        if modified != self.settings_read || modified.is_none() && self.settings_read.is_none() {
            self.settings = NotificationSettings::load();
            self.settings_read = modified;
        }
    }

    /// How long a banner stays (set to Never, 0 seconds, errors still
    /// show, for a while).
    fn banner_time(&self) -> Duration {
        let seconds = if self.settings.seconds == 0 { 10 } else { self.settings.seconds.clamp(2, 60) };
        Duration::from_secs(seconds as u64)
    }

    pub fn quiet(&self) -> bool {
        self.settings.quiet(now_secs(), local_minute()) || self.sharing_quiet()
    }

    /// Quiet because the screen is shared: errors wait too.
    fn sharing_quiet(&self) -> bool {
        self.settings.while_sharing && self.sharing
    }

    /// Whether the screen is shared now, or recorded (a recording going
    /// on this display: a banner would be in the video, as CleanShot X
    /// turns on Do Not Disturb while recording); when that starts (and it
    /// is to be quiet then), the banners on screen go into the history, as
    /// if they had timed out.
    fn refresh_sharing(&mut self) {
        let recording = olo_protocol::recording::going_on(&olo_protocol::recording::this_display()).is_some_and(|g| g.recording);
        let sharing = !olo_protocol::sharing::SharingState::load().shares.is_empty() || recording;
        if sharing == self.sharing {
            return;
        }
        self.sharing = sharing;
        if self.sharing_quiet() {
            let ids: Vec<u32> = self.cards.iter().map(|c| c.note.id).collect();
            for id in ids {
                self.remove(id, Reason::Expired);
            }
        }
    }

    /// Show the archive in the History window (View ▸ Archived), or the
    /// history again.
    pub fn show_archived(&mut self, on: bool) {
        if self.view.archived != on {
            self.view.archived = on;
            self.view.selected = None;
            self.view.held.clear();
            self.changed();
        }
    }

    /// Delete from the archive, for good but for Undo: those at `at`
    /// (None: all of it, Empty Archive).
    pub fn delete_archived(&mut self, at: Option<Vec<usize>>) {
        let gone = self.archive.take_out(|i, _| at.as_ref().is_some_and(|at| !at.contains(&i)));
        if gone.is_empty() {
            return;
        }
        if gone.iter().any(|(_, e)| Some(e.id) == self.view.selected) {
            self.view.selected = None;
        }
        self.steps.done(if at.is_some() { "Delete" } else { "Delete All" }, Gone::Deleted(gone));
        self.archive.save();
        self.archived_changed();
        self.changed();
    }

    /// One of the Cleared list (at `at`) put back in the list, read, as
    /// it was but for its app (closed there by now): where its time
    /// puts it.
    pub fn put_back(&mut self, at: usize) {
        let gone = self.archive.take_out(|i, _| i != at);
        if gone.is_empty() {
            return;
        }
        for (_, mut e) in gone {
            (e.archived, e.why, e.unread, e.live, e.shown) = (0, None, false, false, false);
            // (Read now: it has its time in the list again before it's tidied away.)
            e.read_at = now_secs();
            if self.view.selected == Some(e.id) {
                self.view.selected = None;
            }
            let entries = self.history.entries_mut();
            let pos = entries.iter().position(|x| x.time > e.time).unwrap_or(entries.len());
            entries.insert(pos, e);
        }
        // Crash-safe order: put back in history FIRST, shorten cleared archive SECOND.
        store::History::save_pair(&self.history, &self.archive, None);
        self.archived_changed();
        self.publish(false);
        self.changed();
    }

    /// What the History window lists: the history, or the archive
    /// (its Archived view).
    pub fn listed(&self) -> &History {
        if self.view.archived { &self.archive } else { &self.history }
    }

    /// These have left the history, each for its reason: into the
    /// archive (the Cleared list), and kept.
    pub fn to_archive_each(&mut self, gone: Vec<(Entry, store::Why)>) {
        if self.archive.archive(gone, now_secs()) {
            self.archive.save();
            self.archived_changed();
        }
    }

    /// These have left the history for `why`: into the archive.
    pub fn to_archive(&mut self, gone: Vec<Entry>, why: store::Why) {
        if self.archive.archive(gone.into_iter().map(|e| (e, why)), now_secs()) {
            self.archive.save();
            self.archived_changed();
        }
    }

    /// The archive changed: `ListArchived`'s answer with it.
    fn archived_changed(&mut self) {
        *self.archived_snapshot.lock().unwrap() = serde_json::json!({ "archived": &*self.archive.entries }).to_string();
    }

    /// Tell `List`, Deskbar and the disk what changed.
    fn publish(&mut self, history_changed: bool) {
        // (What left by itself, pushed out by newer ones or faded away.)
        let left = self.history.take_left();
        if !left.is_empty() {
            if self.archive.archive(left, now_secs()) {
                self.archived_changed();
            }
            // Archive first, then history, so no crash loses overflowed/tidied entries.
            store::History::save_pair(&self.archive, &self.history, None);
        } else if history_changed {
            self.history.save();
        }
        let quiet = self.quiet();
        let state = NotificationState {
            unread: self.history.unread(),
            quiet,
            apps: self.history.unread_apps(),
            // (What waits in apps first, red; then what's unread, blue.)
            cards: self
                .waiting
                .cards(|w| named_icons(None, Some(&w.app), &w.name))
                .into_iter()
                .chain(self.history.inbox(|e| named_icons(e.icon.as_deref(), e.desktop_entry.as_deref(), &e.app), self.settings.fade.then_some(self.settings.fade_minutes)))
                .collect(),
            fade_minutes: self.settings.fade.then_some(self.settings.fade_minutes),
            badge: self.settings.badge,
        };
        if self.told.as_ref() != Some(&state) {
            olo_kit::store::save_bytes(NotificationState::path(), serde_json::to_vec(&state).unwrap_or_default());
            self.told = Some(state.clone());
        }
        // Each with how faded it is.
        let now = now_secs();
        let notes: Vec<serde_json::Value> = self
            .history
            .entries
            .iter()
            .map(|e| {
                let mut v = serde_json::to_value(e).unwrap_or_default();
                // (How strongly its row shows: an unread one whole, a read
                // one fading by the time since it was read.)
                let strength = if e.pinned || e.unread { 1.0 } else { self.settings.strength(now.saturating_sub(e.since())) };
                v["strength"] = serde_json::json!((strength * 100.0).round() / 100.0);
                // (Where it stands, in a word: what a person sees as a
                // blue dot, or none. One that's cleared is in `ListArchived`.)
                v["state"] = serde_json::json!(if e.unread { "unread" } else { "read" });
                v
            })
            .collect();
        let list = serde_json::json!({
            "quiet": quiet,
            "unread": state.unread,
            "notifications": notes,
            // What apps say is waiting in them: not notifications.
            "waiting": &self.waiting.apps,
        });
        *self.snapshot.lock().unwrap() = list.to_string();
    }

    fn show(&mut self, mut note: Note) {
        self.refresh_settings();
        match &note.sender {
            Some(s) => {
                self.senders.insert(note.id, s.clone());
            }
            None => {
                self.senders.remove(&note.id);
            }
        }
        // One of each (Preferences ▸ Notifications): the same again from
        // the same app takes the earlier one's place.
        if self.settings.one_of_each && !note.replaces {
            let text = markup::plain(&note.body);
            let earlier = store::same_as(&self.history, &self.cards.iter().map(|c| &c.note).collect::<Vec<_>>(), &note.app_name, &note.summary, &text, note.id);
            for id in earlier {
                self.take(id);
                // (To the Cleared list, saying why: not gone without a trace.)
                let gone: Vec<Entry> = self.history.take_out(|_, e| e.id != id).into_iter().map(|(_, e)| e).collect();
                self.to_archive(gone, store::Why::Replaced);
                self.queue_closed(id, Reason::Expired);
            }
        }
        let app = self.settings.app_via(&note.app_name, note.via.as_deref());
        let quiet = self.quiet();
        let never = self.settings.seconds == 0;
        if note.timeout == Some(server::DEFAULT_TIMEOUT) {
            note.timeout = Some(self.banner_time());
        }
        // Do Not Disturb keeps all but what's urgent off the screen and
        // quiet (not while the screen is shared); a snoozed one's news
        // waits with it.
        let snoozed = self.history.get_mut(note.id).is_some_and(|e| e.snoozed());
        // One from the app the person is in, about the app as a whole
        // (not a tab of it, nor a site through a browser): they're
        // there, so it's read as it comes, with no banner over what
        // they're looking at (it's in the list, read). An urgent one
        // shows all the same.
        let there = !note.urgent
            && note.via.is_none()
            && note.show.is_empty()
            && self.in_front.as_deref().is_some_and(|front| store::is_app(&store::app_ids(note.desktop_entry.as_deref(), &note.app_name), front));
        let banner = app.banners && ((!quiet && !never) || (note.urgent && !self.sharing_quiet())) && !snoozed && !there;
        if app.sounds
            && !quiet
            && !snoozed
            && !note.replaces
            && let Some(path) = note.sound.as_ref().and_then(|s| sound::for_note(s, note.kind))
        {
            sound::play(&path);
        }
        let kept = app.history && !note.transient;
        if kept {
            let mut e = Entry::from_note(&note, !there);
            if there {
                e.read_at = now_secs();
            }
            e.shown = banner;
            self.history.put(e);
            if let Some(p) = &note.picture {
                self.history.keep_picture(note.id, p);
            }
        }
        if banner {
            self.add_card(note);
        } else if self.cards.iter().any(|c| c.note.id == note.id) {
            // A replacement for one on screen that may no longer show.
            self.take(note.id);
        } else if !kept {
            // Neither shown nor kept: gone as far as the app is concerned.
            self.queue_closed(note.id, Reason::Dismissed);
        }
        self.publish(kept);
        self.changed();
    }

    fn add_card(&mut self, note: Note) {
        // Pinned: on screen until closed.
        let pinned = self.history.newest(note.id).is_some_and(|e| e.pinned);
        let until = note.timeout.filter(|_| !pinned).map(|t| Instant::now() + t);
        if let Some(at) = until {
            self.expire_at(at);
        }
        let picture = note.picture.as_ref().map(|p| pictures::Source::Note(note.id, Arc::new(pictures::Raw::from_unmultiplied(p.width, p.height, &p.rgba))));
        match self.cards.iter_mut().find(|c| c.note.id == note.id) {
            // In place: what was typed stays, the picture is made again.
            Some(old) => {
                old.note = note;
                old.until = until;
                old.picture = picture;
            }
            None => self.cards.push(Card { note, until, picture, replying: false }),
        }
        self.open_banners();
        self.changed();
    }

    /// Something the windows show changed.
    pub fn changed(&mut self) {
        self.dirty = true;
    }

    /// The windows told that what they show changed (once the lock is
    /// about to be let go: [`with`]).
    fn tell(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        if let Some(w) = &self.win {
            w.messenger().send(banners::Told::Changed);
        }
        if let Some(w) = &self.history_win {
            w.messenger().send(history::Told::Changed);
        }
    }

    /// The banners' surface (its first frame reads the cards).
    fn open_banners(&mut self) {
        if self.win.as_ref().is_some_and(|w| w.is_open()) {
            return;
        }
        let Some(shared) = self.me.upgrade() else { return };
        let options = WindowOptions::new("Notifications", WIDTH, 1.0).layer("notifications", LayerShellLayer::Top, (false, true, false, true)).layer_options(|l| {
            l.margin = (0, 0, GAP as i32, 0);
            l.keyboard = LayerKeyboard::None;
        });
        self.win = Some(olo_kit::open_window(options.no_panel(), move |cx| banners::Banners::new(shared, cx)));
    }

    fn close_banners(&mut self) {
        if let Some(w) = self.win.take() {
            w.close();
        }
    }

    /// Check again after `after` (a card's time, a snooze, a token).
    fn tick_after(&self, after: Duration) {
        later(&self.proxy, after + Duration::from_millis(20), Msg::Tick);
    }

    /// The connection `sender` (a unique name on the bus) has gone: the app
    /// quit, or started again (an upgrade re-executes Olo's apps), and
    /// knows nothing of what it sent. Those notes are no longer open at
    /// their app: a click on one shows what it's about the way a closed
    /// one's does (`x-olo-show`, else the app), instead of an action
    /// nobody hears (Ben, 2026-11-18: a click on a waiting tab's note
    /// "only took me to the last active window/tab in terminal").
    fn sender_gone(&mut self, sender: &str) {
        let ids: Vec<u32> = self.senders.iter().filter(|(_, s)| s.as_str() == sender).map(|(id, _)| *id).collect();
        if ids.is_empty() {
            return;
        }
        let mut kept = false;
        for id in &ids {
            self.senders.remove(id);
            if let Some(e) = self.history.get_mut(*id)
                && e.live
            {
                e.live = false;
                kept = true;
            }
        }
        // (On screen still: its buttons can't do anything now; a click on
        // it shows what it's about.)
        for c in self.cards.iter_mut().filter(|c| ids.contains(&c.note.id)) {
            c.note.actions.clear();
            c.note.reply = None;
            c.replying = false;
        }
        tracing::info!(sender, notes = ids.len(), "notifications: their sender left the bus: no longer open at their app");
        self.publish(kept);
        self.changed();
    }

    /// What waits in an app (`id`: its `Waiting`) gone to: the thing
    /// that waits, where the app said what it is in a notification of
    /// its own (that window, that tab: `waiting::about`), as a click on
    /// that notification; else its app comes forward. Either puts it
    /// off, as going there by any way does.
    pub fn open_waiting(&mut self, id: u32) {
        let Some(w) = self.waiting.get(id).cloned() else { return };
        // Where the app said what waits where: that one (an app's own id,
        // the first it names), shown as the app shows it (Terminal's
        // window and tab, `--show-shell ID`).
        let wait = w.waits.iter().find(|x| x.id == id).or(w.waits.first());
        match wait {
            Some(x) => {
                if !show_what(Some(&w.app), &x.waits.shows) {
                    raise_app(Some(&w.app), &w.name);
                }
            }
            None => match waiting::about(&self.history.entries, &w.app).cloned() {
                Some(e) if e.live => self.invoke(None, e.id, "default".into()),
                Some(e) => {
                    self.set_read(e.id, true);
                    if !show_what(e.desktop_entry.as_deref(), &e.show) {
                        raise_app(Some(&w.app), &w.name);
                    }
                }
                None => raise_app(Some(&w.app), &w.name),
            },
        }
        if self.waiting.later(wait.map_or(w.id, |x| x.id)) {
            self.publish(false);
            self.changed();
        }
    }

    /// A notification cleared by the person (its ×, on a banner, in the
    /// Inbox or in the list; `Dismiss` to an agent): off the screen, out
    /// of the list and into the Cleared one, and Edit ▸ Undo brings it
    /// back. × means this everywhere. A pinned one stays put, as it does
    /// through everything (its banner goes; it's read).
    pub fn clear(&mut self, id: u32) {
        self.remove(id, Reason::Dismissed);
        self.set_read(id, true);
        let gone = self.history.take_out(|_, e| e.id != id || e.pinned);
        if !gone.is_empty() {
            if self.archive.archive(gone.iter().map(|(_, e)| (e.clone(), store::Why::Cleared)), now_secs()) {
                self.archived_changed();
            }
            store::History::save_pair(&self.archive, &self.history, None);
            self.steps.done("Clear", Gone::Forgot { entries: gone, batch: None, only_eligible: true });
            if self.view.selected == Some(id) {
                self.view.selected = None;
            }
            self.publish(false);
        }
    }

    /// A notification goes. Its card times out into the history (still
    /// open at its app, so its actions work from there, and unread);
    /// one its app takes back while it's still open there is cleared
    /// (to the Cleared list, "Withdrawn by APP": the app's way of saying
    /// it was seen to elsewhere, or is no longer true); otherwise the
    /// app hears why it closed.
    pub fn remove(&mut self, id: u32, reason: Reason) {
        self.senders.remove(&id);
        let on_screen = self.take(id);
        let kept = self.history.get_mut(id).is_some();
        if reason == Reason::Closed && self.history.newest(id).is_some_and(|e| e.live && !e.pinned) {
            let gone: Vec<Entry> = self.history.take_out(|_, e| e.id != id || !e.live).into_iter().map(|(_, e)| e).collect();
            if self.archive.archive(gone.into_iter().map(|e| (e, store::Why::Withdrawn)), now_secs()) {
                self.archived_changed();
            }
            store::History::save_pair(&self.archive, &self.history, None);
            if self.view.selected == Some(id) {
                self.view.selected = None;
            }
            self.queue_closed(id, reason);
            self.publish(false);
            return;
        }
        if reason == Reason::Expired && kept {
            if let Some(e) = self.history.get_mut(id) {
                e.shown = false;
            }
            self.publish(true);
            return;
        }
        if let Some(e) = self.history.get_mut(id) {
            e.live = false;
            e.shown = false;
            if reason == Reason::Dismissed && e.unread {
                e.set_unread(false, now_secs());
            }
        }
        if on_screen || kept {
            self.queue_closed(id, reason);
        }
        self.publish(kept);
    }

    /// Take a card away (saying nothing); whether it was there.
    fn take(&mut self, id: u32) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.note.id != id);
        let was = self.cards.len() != before;
        if self.hovered == Some(id) {
            self.hovered = None;
        }
        self.wake(0..self.fits);
        if self.cards.is_empty() {
            self.close_banners();
        }
        self.changed();
        was
    }

    /// The ones waiting have their time from when they appear (a little,
    /// if theirs ran out while they waited).
    pub fn wake(&mut self, cards: std::ops::Range<usize>) {
        let now = Instant::now();
        for c in self.cards.iter_mut().take(cards.end).skip(cards.start) {
            if let (Some(t), Some(until)) = (c.note.timeout, c.until)
                && until < now + Duration::from_secs(1)
            {
                c.until = Some(now + t.min(Duration::from_secs(3)).max(Duration::from_secs(1)));
            }
        }
    }

    /// A card being used: under the pointer, or a reply begun in it. It
    /// stays, and has its time again once it isn't (`hover`, `typed`).
    fn held(&self, c: &Card) -> bool {
        Some(c.note.id) == self.hovered || c.replying
    }

    /// Cards whose time is up go (not one being used).
    fn expire(&mut self) {
        let now = Instant::now();
        let gone: Vec<u32> = self.cards.iter().take(self.fits).filter(|c| c.until.is_some_and(|u| u <= now) && !self.held(c)).map(|c| c.note.id).collect();
        for id in gone {
            self.remove(id, Reason::Expired);
        }
        // The next to go, checked for then. (Not one being used: its time
        // past, it was looked for again every 20 ms while the pointer
        // rested on it.)
        if let Some(next) = self.cards.iter().take(self.fits).filter(|c| !self.held(c)).filter_map(|c| c.until).min() {
            self.expire_at(next);
        }
    }

    /// A reply typed in note `id`'s card changed: emptied, the card has
    /// its time again (a moment, as when the pointer leaves it).
    pub fn typed(&mut self, id: u32, empty: bool) {
        let Some(card) = self.cards.iter_mut().find(|c| c.note.id == id) else { return };
        card.replying = !empty;
        if empty && card.until.is_some_and(|u| u < Instant::now() + Duration::from_secs(2)) {
            let at = Instant::now() + Duration::from_secs(2);
            card.until = Some(at);
            self.expire_at(at);
        }
    }

    /// Look for cards whose time is up at `at` (one timer for them all:
    /// a later one waits for the earlier, which asks again).
    fn expire_at(&mut self, at: Instant) {
        if self.expiry.is_some_and(|e| e <= at && e > Instant::now()) {
            return;
        }
        self.expiry = Some(at);
        later(&self.proxy, at.saturating_duration_since(Instant::now()) + Duration::from_millis(20), Msg::Expire(at));
    }

    /// The pointer went over a card (or off them): one under it stays,
    /// and goes a moment after it leaves.
    pub fn hover(&mut self, id: Option<u32>) {
        if self.hovered == id {
            return;
        }
        if let Some(left) = self.hovered
            && let Some(card) = self.cards.iter_mut().find(|c| c.note.id == left)
            && card.until.is_some_and(|u| u < Instant::now() + Duration::from_secs(2))
        {
            let at = Instant::now() + Duration::from_secs(2);
            card.until = Some(at);
            self.expire_at(at);
        }
        self.hovered = id;
        self.changed();
    }

    /// Run action `key` of notification `id` (`from`: the window clicked
    /// in, which makes the activation token): the app gets the token
    /// first, so it may bring its window forward. The card goes at once;
    /// the app hears it closed after the action.
    pub fn invoke(&mut self, from: Option<&WindowCx>, id: u32, key: String) {
        let resident = self.cards.iter().any(|c| c.note.id == id && c.note.resident);
        let pending = Pending { id, key, close: !resident, since: Instant::now() };
        match from {
            Some(window) => {
                let tag = self.next_tag;
                self.next_tag += 1;
                self.pending.insert(tag, pending);
                let proxy = self.proxy.clone();
                window.activation_token(move |token| proxy.send(Msg::Token(tag, token)));
                self.tick_after(TOKEN_WAIT);
            }
            None => self.send(pending, None),
        }
        if let Some(e) = self.history.get_mut(id) {
            if e.unread {
                e.set_unread(false, now_secs());
            }
            if !resident {
                e.live = false;
                e.shown = false;
            }
        }
        if !resident {
            self.take(id);
        }
        self.publish(true);
        self.changed();
    }

    /// The reply typed into note `id`'s card: sent, and the card goes.
    pub fn reply(&mut self, id: u32, text: &str) {
        self.bus(|b| b.replied(id, text));
        self.remove(id, Reason::Dismissed);
    }

    /// Token (when there is one), action, then closed, in the spec's order.
    fn send(&self, p: Pending, token: Option<&str>) {
        self.bus(|b| {
            if let Some(t) = token {
                b.activation_token(p.id, t);
            }
            b.action(p.id, &p.key);
            if p.close {
                b.closed(p.id, Reason::Dismissed);
            }
        });
    }

    /// Actions whose token never came go without one.
    fn flush_pending(&mut self) {
        let late: Vec<u64> = self.pending.iter().filter(|(_, p)| p.since.elapsed() >= TOKEN_WAIT).map(|(t, _)| *t).collect();
        for tag in late {
            if let Some(p) = self.pending.remove(&tag) {
                self.send(p, None);
            }
        }
    }

    /// The History window, opened (or brought forward).
    pub fn open_history(&mut self) {
        let Some(shared) = self.me.upgrade() else { return };
        let options = WindowOptions::new("Notifications", 400.0, 560.0).min_size(360.0, 240.0).keep_size("history");
        olo_kit::open_or_raise(&mut self.history_win, options, move |cx| history::HistoryWindow::new(shared, cx));
    }

    /// The History window closed: what was read while it was open moves.
    pub fn history_closed(&mut self) {
        self.history_win = None;
        self.view.held.clear();
        self.view.selected = None;
        self.changed();
    }

    pub fn close_history(&mut self) {
        if let Some(w) = self.history_win.take() {
            self.history_closed();
            w.close();
        }
    }

    /// Mark notification `id` read (or unread again).
    pub fn set_read(&mut self, id: u32, read: bool) {
        if self.history.set_read(id, read) {
            self.publish(true);
            self.changed();
        }
    }

    /// These were read by going to what they are about: their cards
    /// leave the screen (saying nothing to their apps: they are still
    /// open there), and everyone showing counts hears.
    fn seen(&mut self, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        for &id in ids {
            if self.take(id)
                && let Some(e) = self.history.get_mut(id)
            {
                e.shown = false;
            }
        }
        self.publish(true);
        self.changed();
    }

    /// The person is looking at a page of `host` in a browser: that
    /// site's notifications are read.
    pub fn visited_site(&mut self, host: &str) {
        let read = self.history.visited_site(host);
        self.seen(&read);
    }

    pub fn mark_all_read(&mut self) {
        if self.history.mark_read() {
            self.publish(true);
        }
        self.changed();
    }

    /// Forget what has faded away; tell the apps of those still open.
    fn fade_out(&mut self) {
        let Some(live) = self.history.fade_out(&self.settings, now_secs()) else { return };
        for id in live {
            self.queue_closed(id, Reason::Expired);
        }
        self.publish(true);
        self.changed();
    }

    /// Do Not Disturb on for `minutes` (0: until turned off), or off.
    pub fn set_quiet(&mut self, on: bool, minutes: u32) {
        self.refresh_settings();
        self.settings.dnd = on;
        self.settings.dnd_until = (on && minutes > 0).then(|| now_secs() + minutes as u64 * 60);
        self.save_settings();
        if let Some(until) = self.settings.dnd_until {
            self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())));
        }
        self.publish(false);
        self.changed();
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            tracing::warn!("notifications: can't save settings: {e}");
        }
        self.settings_read = std::fs::metadata(NotificationSettings::path()).and_then(|m| m.modified()).ok();
    }

    fn control(&mut self, c: Control) {
        match c {
            // What waits in an app (a card of the Inbox's): opening it
            // goes to the app; Later (the card's clock, `Snooze`, or
            // `Dismiss` from an agent) puts it off until more waits.
            Control::Invoke(id, _) if self.waiting.get(id).is_some() => self.open_waiting(id),
            Control::Dismiss(id) | Control::Snooze(id, _) if self.waiting.get(id).is_some() => {
                if self.waiting.later(id) {
                    self.publish(false);
                }
            }
            Control::Invoke(id, key) => {
                // From a program, not a click (Deskbar's Inbox, the command
                // line, an agent): no token to hand on. One in the history
                // as a click on it there (open at its app or not), by its
                // id, whichever list the window last showed (by its place
                // in that list it opened another's, 2026-11-18); one only
                // on screen, its action; one in the Cleared list, what
                // it's about, else its app.
                if let Some(e) = self.history.newest(id).cloned() {
                    self.open_note(None, e, Some(key));
                } else if self.cards.iter().any(|c| c.note.id == id) {
                    self.invoke(None, id, key);
                } else if let Some(e) = self.archive.newest(id).cloned()
                    && !show_what(e.desktop_entry.as_deref(), &e.show)
                {
                    raise_app(e.desktop_entry.as_deref(), &e.app);
                }
            }
            Control::Dismiss(id) => self.clear(id),
            Control::ClearHistory => self.clear_history(),
            Control::ShowHistory => self.open_history(),
            Control::DoNotDisturb(on, minutes) => self.set_quiet(on, minutes),
            Control::SetRead(id, read) => self.set_read(id, read),
            Control::MarkAllRead => self.mark_all_read(),
            Control::Pin(id, on) => self.set_pinned(id, on),
            Control::Snooze(id, minutes) => self.snooze(id, now_secs() + minutes as u64 * 60),
            Control::VisitedSite(host) => return self.visited_site(&host),
        }
        self.changed();
    }

    /// Pin a note (it stays put: no fading, no going with Clear All, and
    /// on screen until closed), or unpin it.
    pub fn set_pinned(&mut self, id: u32, on: bool) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.pinned = on;
        let mut again = None;
        if let Some(c) = self.cards.iter_mut().find(|c| c.note.id == id) {
            c.until = if on { None } else { c.note.timeout.map(|t| Instant::now() + t) };
            again = c.until;
        }
        if let Some(at) = again {
            self.expire_at(at);
        }
        self.publish(true);
        self.changed();
    }

    /// Put a note away until `until` (seconds since the epoch): out of
    /// sight (off the screen too) and not counted, then back as new
    /// (`wake_snoozed`).
    pub fn snooze(&mut self, id: u32, until: u64) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.snoozed_until = Some(until);
        e.shown = false;
        self.take(id);
        if self.view.selected == Some(id) {
            self.view.selected = None;
        }
        self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())));
        self.publish(true);
        self.changed();
    }

    /// `note_menu`'s command for the note `id`.
    pub fn note_command(&mut self, id: u32, command: &str) {
        if command == "pin" {
            let on = !self.history.newest(id).is_some_and(|e| e.pinned);
            self.set_pinned(id, on);
        } else if let Some((minutes, _)) = command.strip_prefix("snooze:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| SNOOZES.get(i)) {
            self.snooze(id, minutes.map_or_else(tomorrow_morning, |m| now_secs() + m as u64 * 60));
        }
    }

    /// Snoozed notes whose time has come: back as new, unread and (unless
    /// banners are off or it is quiet) on screen again.
    fn wake_snoozed(&mut self) {
        let now = now_secs();
        let due: Vec<u32> = self.history.entries.iter().filter(|e| e.snoozed_until.is_some_and(|t| t <= now)).map(|e| e.id).collect();
        if due.is_empty() {
            return;
        }
        self.refresh_settings();
        let quiet = self.quiet();
        let (never, time) = (self.settings.seconds == 0, self.banner_time());
        for id in due {
            let Some(e) = self.history.newest_mut(id) else { continue };
            e.snoozed_until = None;
            e.set_unread(true, now);
            e.time = now;
            let banner = self.settings.app_via(&e.app, e.via.as_deref()).banners && !quiet && !never;
            e.shown = banner;
            let note = e.to_note(Some(time));
            if banner {
                self.add_card(note);
            }
        }
        self.publish(true);
        self.changed();
    }

    /// Forget the history (but what is pinned or snoozed); what is still
    /// open at its app closes.
    pub fn clear_history(&mut self) {
        let kept = |e: &Entry| e.shown || e.pinned || e.snoozed();
        let live: Vec<u32> = self.history.entries.iter().filter(|e| e.live && !kept(e)).map(|e| e.id).collect();
        for id in live {
            self.queue_closed(id, Reason::Dismissed);
        }
        let gone = self.history.take_out(|_, e| kept(e));
        if !gone.is_empty() {
            if self.archive.archive(gone.iter().map(|(_, e)| (e.clone(), store::Why::Cleared)), now_secs()) {
                self.archived_changed();
            }
            store::History::save_pair(&self.archive, &self.history, None);
            self.steps.done("Clear All", Gone::Forgot { entries: gone, batch: None, only_eligible: true });
        }
        self.publish(false);
        self.changed();
    }

    /// Clear matching `(id, instance)` targets in one batch and one Undo step
    /// (Deskbar's Clear All with `unread_only = true`, and
    /// `olo-notifications --clear ID1 ID2…` with `unread_only = false`). Never
    /// touches pinned notifications (their banner and unread state stay),
    /// snoozed notifications, or waiting cards (`self.waiting`). Saves archive
    /// strictly before history, queues close signals to be emitted outside the
    /// model lock, and returns `(cleared_ids, batch_id, persisted)`.
    pub fn clear_many(&mut self, targets: &[(u32, u64)], unread_only: bool) -> (Vec<u32>, String, Option<futures_channel::oneshot::Receiver<()>>) {
        let Some((cleared, batch, live_ids, archive_changed)) =
            clear_many_entries(&mut self.history, &mut self.archive, &mut self.steps, &mut self.clear_batches, &self.batch_prefix, &mut self.next_batch_seq, targets, unread_only, now_secs())
        else {
            return (Vec::new(), String::new(), None);
        };
        for &id in &cleared {
            self.senders.remove(&id);
            let on_screen = self.take(id);
            if on_screen || live_ids.contains(&id) {
                self.queue_closed(id, Reason::Dismissed);
            }
            if self.view.selected == Some(id) {
                self.view.selected = None;
            }
        }
        if archive_changed {
            self.archived_changed();
        }
        // Crash-safe order: write cleared archive FIRST, then shortened history SECOND.
        let (tx, rx) = futures_channel::oneshot::channel();
        store::History::save_pair(&self.archive, &self.history, Some(tx));
        self.publish(false);
        self.changed();
        (cleared, batch, Some(rx))
    }

    /// Undo the ClearMany batch identified by `batch`: restores exactly those
    /// entries of that batch that are still in the cleared archive and not
    /// already back in history (independent of History's Undo stack). Writes
    /// history strictly before shortening the archive, returning `(restored_ids,
    /// persisted)`.
    pub fn undo_clear(&mut self, batch: &str) -> (Vec<u32>, Option<futures_channel::oneshot::Receiver<()>>) {
        let (restored, archive_changed) = undo_clear_batch(&mut self.history, &mut self.archive, &mut self.steps, &mut self.clear_batches, batch);
        if restored.is_empty() {
            return (Vec::new(), None);
        }
        if archive_changed {
            self.archived_changed();
        }
        // Crash-safe order: put back in history FIRST, shorten cleared archive SECOND.
        let (tx, rx) = futures_channel::oneshot::channel();
        store::History::save_pair(&self.history, &self.archive, Some(tx));
        self.publish(false);
        self.changed();
        (restored, Some(rx))
    }

    /// Edit ▸ Undo (`redo` false) or Redo in the History window: entries
    /// forgotten put back (closed at their apps by now), or forgotten again.
    pub fn undo_redo(&mut self, redo: bool) {
        use olo_kit::undo::Purpose;
        let Some(step) = (if redo { self.steps.take_redo() } else { self.steps.take_undo() }) else { return };
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        let order = apply_undo_step(&mut self.history, &mut self.archive, &mut self.steps, step, purpose, now_secs());
        match order {
            SaveOrder::HistoryThenArchive => {
                store::History::save_pair(&self.history, &self.archive, None);
                self.archived_changed();
            }
            SaveOrder::ArchiveThenHistory => {
                store::History::save_pair(&self.archive, &self.history, None);
                self.archived_changed();
            }
            SaveOrder::ArchiveOnly => {
                self.archive.save();
                self.archived_changed();
            }
            SaveOrder::None => {}
        }
        self.publish(false);
        self.changed();
    }

    /// Time passed, or something it depends on changed.
    fn tick(&mut self) {
        self.flush_pending();
        self.wake_snoozed();
        // Do Not Disturb may have begun or ended (a share too).
        self.refresh_settings();
        self.refresh_sharing();
        self.fade_out();
        self.publish(false);
        self.expire();
        // The history fades on, its times say so.
        self.changed();
    }

    fn message(&mut self, msg: Msg) {
        match msg {
            Msg::Attention(badges) => {
                // (An app's name: Olo's own by its program, another's by its desktop entry.)
                let name = |id: &str| olo_xdg::apps::OUR_APPS.iter().find(|(bin, ..)| *bin == id).map_or_else(|| gtk::app_name(id), |(_, name, _)| (*name).to_owned());
                if self.waiting.update(&badges, now_secs(), name, || self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    self.publish(false);
                    self.changed();
                }
            }
            Msg::Left => self.in_front = None,
            Msg::Visited(app_id) => {
                self.in_front = Some(app_id.clone());
                let read = self.history.visited_app(&app_id);
                // (And what waits in it has been seen, as much as there is.)
                if self.waiting.visited_app(&app_id) && read.is_empty() {
                    self.publish(false);
                    self.changed();
                }
                self.seen(&read);
            }
            Msg::In(In::Show(note)) => self.show(*note),
            Msg::In(In::Close(id)) => self.remove(id, Reason::Closed),
            Msg::In(In::SenderGone(sender)) => self.sender_gone(&sender),
            Msg::In(In::Control(c, done)) => {
                self.control(c);
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Msg::In(In::ClearMany { targets, unread_only, reply }) => {
                let out = self.clear_many(&targets, unread_only);
                let _ = reply.send(out);
            }
            Msg::In(In::UndoClear { batch, reply }) => {
                let out = self.undo_clear(&batch);
                let _ = reply.send(out);
            }
            Msg::Tick => self.tick(),
            Msg::Token(tag, token) => {
                if let Some(p) = self.pending.remove(&tag) {
                    self.send(p, token.as_deref());
                }
            }
            // (What each note's mark is drawn from is in its key: `icons_made`.)
            Msg::Refresh => {
                ICONS_MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // (The inbox's cards name their icons: told again with them.)
                self.publish(false);
            }
            Msg::Expire(at) => {
                if self.expiry == Some(at) {
                    self.expiry = None;
                }
                self.expire();
            }
        }
        self.changed();
    }
}

/// The model, as the app's looper and the windows hold it.
pub type Shared = Arc<Mutex<Notifications>>;

/// Does `f` with the model, tells the windows if what they show changed, and
/// emits any queued `NotificationClosed` signals strictly AFTER releasing the
/// model lock (so a backpressured bus never blocks banners or History).
pub fn with<R>(shared: &Shared, f: impl FnOnce(&mut Notifications) -> R) -> R {
    let (out, bus, closes) = {
        let mut n = shared.lock().unwrap();
        let out = f(&mut n);
        n.tell();
        let closes = std::mem::take(&mut n.pending_closes);
        let bus = (!closes.is_empty()).then(|| n.bus.clone());
        (out, bus, closes)
    };
    if let Some(bus) = bus
        && let Some(b) = bus.lock().unwrap().as_ref()
    {
        for (id, reason) in closes {
            b.closed(id, reason);
        }
    }
    out
}

/// Sends the app's looper `msg` after `after` (a sleeping thread: nothing
/// runs till then).
fn later(proxy: &olo_kit::Proxy, after: Duration, msg: Msg) {
    let proxy = proxy.clone();
    let _ = std::thread::Builder::new().name("notifications-timer".into()).stack_size(64 * 1024).spawn(move || {
        std::thread::sleep(after);
        proxy.send(msg);
    });
}

/// How many times icons have been found since the program began: in the
/// key of whatever draws a note's mark, so it's drawn again when its
/// app's icon comes.
static ICONS_MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn icons_made() -> u64 {
    ICONS_MADE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The program a notification's own app is, if it's one of Olo's: by its
/// desktop entry ("olo-terminal", "olo-terminal.desktop").
pub fn own_program(desktop_entry: Option<&str>) -> Option<&'static str> {
    let entry = desktop_entry?.trim_end_matches(".desktop");
    olo_xdg::apps::OUR_APPS.iter().map(|(bin, ..)| *bin).find(|bin| *bin == entry)
}

/// Shows what a notification is about (`x-olo-show`): its app's own
/// program started with those arguments, which hands them to the copy
/// that runs. Only for Olo's own programs (anything else could have the
/// centre run what it liked), and only arguments: never a shell's words.
/// False if it didn't say how, or isn't ours.
pub fn show_what(desktop_entry: Option<&str>, show: &[String]) -> bool {
    let Some(program) = own_program(desktop_entry).filter(|_| !show.is_empty()) else { return false };
    let program = olo_xdg::find_program(program).map_or_else(|| program.into(), std::path::PathBuf::into_os_string);
    let argv: Vec<std::ffi::OsString> = std::iter::once(program).chain(show.iter().map(Into::into)).collect();
    match olo_xdg::open::spawn(&argv) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("notifications: can't show what a note is about ({argv:?}): {e}");
            false
        }
    }
}

/// How long a window is in front before the person is taken to have
/// gone to it: passing over it on the way to another (the window
/// switcher, a pointer crossing it where focus follows the pointer)
/// reads nothing.
const VISIT_DWELL: Duration = Duration::from_millis(900);

/// Tell the UI of each window the person goes to (`Msg::Visited`, its
/// app): from app_server's focus events, one a change of focus, asleep
/// between them. A notification is read by going to what it is about,
/// however one gets there.
fn follow_visits(proxy: olo_kit::Proxy) {
    use olo_protocol::{Client, Event, EventKind};
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    let _ = std::thread::Builder::new().name("notes-focus".into()).spawn(move || {
        loop {
            // Where the person is as this starts (or app_server did
            // again): events only say where they go from here, and one
            // who stays in an app is there all the same.
            if let Ok(mut client) = Client::connect()
                && let Ok(olo_protocol::Reply::Windows { windows }) = client.request(&olo_protocol::Request::ListWindows)
                && tx.send(windows.iter().find(|w| w.focused).map(|w| w.app_id.clone())).is_err()
            {
                return;
            }
            if let Ok(client) = Client::connect()
                && let Ok(events) = client.into_events_of(&[EventKind::Focus])
            {
                for e in events {
                    let Ok(e) = e else { break };
                    if let Event::WindowsChanged { windows } = e
                        && tx.send(windows.iter().find(|w| w.focused).map(|w| w.app_id.clone())).is_err()
                    {
                        return;
                    }
                }
            }
            // app_server restarting: again shortly.
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let _ = std::thread::Builder::new().name("notes-visits".into()).spawn(move || {
        while let Ok(mut at) = rx.recv() {
            // (Out of where they were, as of now.)
            proxy.send(Msg::Left);
            // Still there after a moment: gone to. (Another window in
            // front meanwhile starts the moment again, for that one.)
            loop {
                match rx.recv_timeout(VISIT_DWELL) {
                    Ok(next) => at = next,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            if let Some(app_id) = at.filter(|a| !a.is_empty()) {
                proxy.send(Msg::Visited(app_id));
            }
        }
    });
}

/// Bring forward a window of the app with this desktop entry or name (a
/// click on a note with no default action), off the UI thread.
pub fn raise_app(desktop_entry: Option<&str>, app_name: &str) {
    let ids = store::app_ids(desktop_entry, app_name);
    std::thread::spawn(move || {
        use olo_protocol::{Client, Reply, Request};
        let Ok(mut client) = Client::connect() else { return };
        let Ok(Reply::Windows { windows }) = client.request(&Request::ListWindows) else { return };
        if let Some(w) = windows.iter().find(|w| store::is_app(&ids, &w.app_id)) {
            let _ = client.request(&Request::Activate { id: w.id });
        }
    });
}

/// Icons to try for what a note names, best first: its icon, ours for
/// the common theme names, then its app's (from its desktop entry and
/// name).
pub fn named_icons(icon: Option<&str>, desktop_entry: Option<&str>, app: &str) -> Vec<String> {
    let mut names: Vec<String> = icon.iter().map(|i| i.to_string()).collect();
    if let Some(icon) = icon {
        let ours = [
            ("mail", "mail"),
            ("utilities-terminal", "terminal"),
            ("dialog-warning", "warning"),
            ("dialog-error", "stop"),
            ("dialog-information", "info"),
            ("battery", "power"),
            ("network", "network"),
            ("bluetooth", "bluetooth"),
            ("audio", "sound"),
            ("software-update", "software-update"),
            ("system-software-install", "installer"),
            ("camera", "camera"),
            ("printer", "printer"),
            ("drive", "disk"),
            ("media-removable", "usb"),
        ];
        names.extend(ours.iter().filter(|(theme, _)| icon.starts_with(theme)).map(|(_, o)| o.to_string()));
    }
    names.extend(app_icons(desktop_entry, app));
    names
}

/// The mark of who a note is from, filling `r`: a website's tile; else
/// the first of `names` there is; else, for an app that gave its colour
/// but has no icon, its initial on that colour; else the generic app's.
pub fn paint_mark(g: &mut Painter, r: Bounds<Pixels>, app: &str, accent: Option<[u8; 3]>, via: bool, names: &[String]) {
    if !via && names.iter().any(|n| olo_kit::icons::paint(g, r, n)) {
        return;
    }
    if via || (accent.is_some() && !app.trim().is_empty()) {
        let (letter, fill) = web::tile(app, accent);
        return olo_kit::style::monogram(g, r, &letter, fill);
    }
    let _ = olo_kit::icons::paint(g, r, "application");
}

/// An app as notes name it: (its desktop entry, its name).
type AppKey = (Option<String>, String);

/// Each app's own icon names, once looked up (None: being looked up).
static APP_ICONS: Mutex<Option<HashMap<AppKey, Option<Vec<String>>>>> = Mutex::new(None);

/// Where to say that icons were found (the windows are drawn again).
static REFRESH: std::sync::OnceLock<olo_kit::Proxy> = std::sync::OnceLock::new();

/// The sending app's own icon names. Finding them can read every desktop
/// entry on the system, milliseconds a time, so a worker looks each app
/// up once ([`warm_app_icons`]) and they are kept; until then there are
/// none, and the windows are drawn again when they come.
fn app_icons(desktop_entry: Option<&str>, app: &str) -> Vec<String> {
    let key = (desktop_entry.map(str::to_owned), app.to_owned());
    if let Some(known) = APP_ICONS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return known.clone().unwrap_or_default();
    }
    warm_app_icons(vec![key], || {
        if let Some(p) = REFRESH.get() {
            p.send(Msg::Refresh);
        }
    });
    Vec::new()
}

/// Look up the icons of `apps` not yet known, on a worker; then `done`.
fn warm_app_icons(apps: Vec<AppKey>, done: impl FnOnce() + Send + 'static) {
    let apps: Vec<AppKey> = {
        let mut known = APP_ICONS.lock().unwrap();
        let known = known.get_or_insert_with(HashMap::new);
        apps.into_iter().filter(|k| known.insert(k.clone(), None).is_none()).collect()
    };
    if apps.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for key in apps {
            let names = look_up_app_icons(key.0.as_deref(), &key.1);
            APP_ICONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, Some(names));
        }
        done();
    });
}

fn look_up_app_icons(desktop_entry: Option<&str>, app: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(e) = desktop_entry {
        let e = e.trim_end_matches(".desktop");
        names.extend(olo_xdg::icon_for_app(e));
        // (Olo's own by its program's name, entry or none.)
        names.extend(olo_xdg::apps::OUR_APPS.iter().find(|(bin, ..)| *bin == e).map(|(_, _, icon)| (*icon).to_owned()));
    }
    // (One of Olo's own by its name as people see it, "StyledEdit":
    // sent with no desktop entry, it had the generic app's cube.)
    names.extend(olo_xdg::apps::OUR_APPS.iter().find(|(_, name, _)| name.eq_ignore_ascii_case(app.trim())).map(|(_, _, icon)| (*icon).to_owned()));
    let app = app.to_lowercase().replace(' ', "-");
    if !app.is_empty() {
        names.extend(olo_xdg::icon_for_app(&app));
        names.extend(olo_xdg::apps::our_app(&app).map(|(_, icon)| icon.to_owned()));
    }
    names
}

/// A time in the history: "14:05" today, else "Oct 26, 14:05".
pub fn when(secs: u64) -> String {
    let local = |t: i64| {
        // SAFETY: localtime_r writes into the zeroed struct we own.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&(t as libc::time_t), &mut tm);
            tm
        }
    };
    let tm = local(secs as i64);
    let today = local(now_secs() as i64);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        format!("{} {}, {:02}:{:02}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

/// Snoozing for so many minutes (None: until nine tomorrow morning,
/// `tomorrow_morning`).
pub const SNOOZES: [(Option<u32>, &str); 4] = [(Some(15), "15 Minutes"), (Some(60), "1 Hour"), (Some(180), "3 Hours"), (None, "Tomorrow Morning")];

/// Nine tomorrow morning, local time (seconds since the epoch).
fn tomorrow_morning() -> u64 {
    // SAFETY: localtime_r and mktime use only the struct we own.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (tm.tm_mday + 1, 9, 0, 0, -1);
        libc::mktime(&mut tm).max(0) as u64
    }
}

/// A note's Pin (or Unpin) and Snooze ▸ items, after `menu`'s (`pinned`:
/// None with no note to act on).
pub fn note_menu(menu: Menu, pinned: Option<bool>) -> Menu {
    let mut snooze = Menu::new("Snooze");
    for (i, (_, label)) in SNOOZES.iter().enumerate() {
        snooze = snooze.item(*label, format!("snooze:{i}")).enabled(pinned.is_some());
    }
    menu.item(if pinned == Some(true) { "Unpin" } else { "Pin" }, "pin").enabled(pinned.is_some()).submenu(snooze)
}

/// The next minute's start, from now.
fn to_next_minute() -> Duration {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    Duration::from_secs(60) - Duration::from_millis((now.as_millis() % 60_000) as u64) + Duration::from_millis(50)
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let address = match args.iter().position(|a| a == "--bus") {
        Some(i) if i + 1 < args.len() => {
            let a = args.remove(i + 1);
            args.remove(i);
            Some(a)
        }
        Some(_) => return cli::usage(),
        None => None,
    };
    // The start-up sound, played now (Preferences ▸ Sound's Play).
    if args.first().map(String::as_str) == Some("--startup-sound") {
        let chime = args.get(1).map_or(Some(olo_protocol::StartupChime::default()), |c| olo_protocol::StartupChime::parse(c));
        let Some(chime) = chime else {
            eprintln!("olo-notifications: --startup-sound takes olo or swell");
            return std::process::ExitCode::FAILURE;
        };
        return std::process::ExitCode::from(startup::play_now(chime) as u8);
    }
    // How the start-up sound would play now, said (nothing played).
    if args.first().map(String::as_str) == Some("--startup-level") {
        return std::process::ExitCode::from(startup::say_level() as u8);
    }
    // Anything else is a command for the running service.
    if args.first().is_some_and(|arg| arg == "--glance") {
        return glance::serve();
    }
    if !args.is_empty() {
        return cli::run(address.as_deref(), &args);
    }
    if address.is_none() && !olo_xdg::session::real() {
        eprintln!("olo-notifications: not in a real Olo session (use --bus ADDRESS to try it)");
        return std::process::ExitCode::FAILURE;
    }
    olo_preview::audio::set_identity("Notifications", "olo-notifications");
    olo_kit::run(APP_ID, move |app| {
        olo_kit::stay_resident();
        // Olo's start-up sound, once a session.
        startup::at_start();
        let proxy = olo_kit::proxy();
        let _ = REFRESH.set(proxy.clone());
        let bus = Arc::new(Mutex::new(None));
        let snapshot = Arc::new(Mutex::new(String::new()));
        let (history, mut archive) = History::load_pair(Some(History::path()), Some(History::archive_path()));
        if archive.prune_archive(now_secs()) {
            archive.save();
        }
        History::sweep_pictures(&[&history, &archive]);
        // (Past the archive's too: an id names one notification, there or here.)
        let first = history.entries.iter().chain(archive.entries.iter()).map(|e| e.id).max().unwrap_or(0).wrapping_add(1).max(1);
        let archived_snapshot = Arc::new(Mutex::new(serde_json::json!({ "archived": &*archive.entries }).to_string()));
        let next_id = Arc::new(std::sync::atomic::AtomicU32::new(first));
        let bus_ids = next_id.clone();
        warm_app_icons(history.entries.iter().map(|e| (e.desktop_entry.clone(), e.app.clone())).collect(), || {});
        // Another notification service may have the name (a desktop
        // started before ours): wait for it to go rather than take it.
        let (slot, to_ui, shared) = (bus.clone(), proxy.clone(), (snapshot.clone(), archived_snapshot.clone()));
        std::thread::Builder::new()
            .name("notifications-bus".into())
            .spawn(move || {
                let mut said = false;
                loop {
                    let to_ui = to_ui.clone();
                    match Bus::serve(address.as_deref(), bus_ids.clone(), shared.clone(), move |m| {
                        // Its app's icons, by the time it shows.
                        if let In::Show(n) = &m {
                            warm_app_icons(vec![(n.desktop_entry.clone(), n.app_name.clone())], || {});
                        }
                        to_ui.send(Msg::In(m))
                    }) {
                        Ok(b) => {
                            *slot.lock().unwrap() = Some(b);
                            return;
                        }
                        Err(e) if !said => {
                            eprintln!("olo-notifications: can't serve {} yet ({e}); trying every 30 s", server::NAME);
                            said = true;
                        }
                        Err(_) => {}
                    }
                    std::thread::sleep(Duration::from_secs(30));
                }
            })
            .expect("spawn");
        // The settings change when Preferences or Deskbar write them, and
        // a share or a recording starting or ending is news too: all
        // watched (nothing read while nothing changes).
        let mut watches = Vec::new();
        let path = olo_protocol::notifications::Attention::path();
        let attention = {
            let proxy = proxy.clone();
            let path = path.clone();
            move || {
                if let Some(badges) = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()) {
                    proxy.send(Msg::Attention(badges));
                }
            }
        };
        let initial = attention.clone();
        watches.extend(olo_kit_base::watch::watch([path], move |_| attention()));
        initial();
        follow_visits(proxy.clone());
        for path in [NotificationSettings::path(), olo_protocol::sharing::SharingState::path(), olo_protocol::recording::path_for(&olo_protocol::recording::this_display())] {
            let ticker = proxy.clone();
            watches.extend(olo_kit_base::watch::watch([path], move |_| ticker.send(Msg::Tick)));
        }
        let shared: Shared = Arc::new_cyclic(|me| {
            Mutex::new(Notifications {
                next_id,
                bus,
                me: me.clone(),
                dirty: false,
                win: None,
                cards: Vec::new(),
                fits: SHOWN,
                hovered: None,
                expiry: None,
                pending: HashMap::new(),
                next_tag: 0,
                proxy: proxy.clone(),
                settings: NotificationSettings::default(),
                settings_read: None,
                history,
                waiting: waiting::WaitingApps::load(Some(waiting::WaitingApps::path())),
                senders: HashMap::new(),
                archive,
                history_win: None,
                view: history::HistoryState::default(),
                snapshot,
                archived_snapshot,
                told: None,
                sharing: false,
                in_front: None,
                _watches: watches,
                steps: Default::default(),
                batch_prefix: generate_batch_prefix(),
                next_batch_seq: 0,
                clear_batches: VecDeque::new(),
                pending_closes: Vec::new(),
            })
        });
        with(&shared, |n| {
            n.refresh_sharing();
            n.refresh_settings();
            n.fade_out();
            // Snoozed ones back on time.
            n.wake_snoozed();
            // The banners that were on screen to stay before a restart (an
            // upgrade) are again: the pinned ones, and those with no time
            // of their own (urgent, an error, sticky) not yet dealt with.
            let pinned: Vec<Note> = n.history.entries.iter().filter(|e| e.shown).map(|e| e.to_note((!e.stays).then(|| n.banner_time()))).collect();
            for note in pinned {
                n.add_card(note);
            }
            let now = now_secs();
            for until in n.history.entries.iter().filter_map(|e| e.snoozed_until).collect::<Vec<_>>() {
                n.tick_after(Duration::from_secs(until.saturating_sub(now)));
            }
            n.publish(false);
        });
        // A scheduled Do Not Disturb begins and ends on the minute, and
        // the history's times and fading move on: a tick each minute.
        let minutes = proxy.clone();
        let _ = std::thread::Builder::new().name("notifications-minute".into()).stack_size(64 * 1024).spawn(move || {
            loop {
                std::thread::sleep(to_next_minute());
                minutes.send(Msg::Tick);
            }
        });
        let messages = shared.clone();
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(msg) = message.downcast::<Msg>() {
                with(&messages, |n| n.message(*msg));
            }
        });
        // After an upgrade: flush pending ordered writes and keep the History window as it was.
        let saving = shared.clone();
        olo_kit::on_upgrade(move |_| {
            store::flush_writes();
            let n = saving.lock().unwrap();
            n.history_win.as_ref().filter(|w| w.is_open()).map(|_| serde_json::json!({ "history": true, "search": n.view.search, "folded": n.view.folded, "selected": n.view.selected }).to_string())
        });
        if let Some(state) = olo_kit::restored_state().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            && state["history"] == true
        {
            with(&shared, |n| {
                n.view.search = state["search"].as_str().unwrap_or_default().to_owned();
                n.view.folded = serde_json::from_value(state["folded"].clone()).unwrap_or_default();
                n.view.selected = state["selected"].as_u64().map(|id| id as u32);
                n.open_history();
            });
        }
    });
    store::flush_writes();
    std::process::ExitCode::SUCCESS
}

/// Which crash-safe file-write order an Undo/Redo step requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveOrder {
    None,
    HistoryThenArchive,
    ArchiveThenHistory,
    ArchiveOnly,
}

/// Pure core of [`Notifications::clear_many`]: removes matching unpinned,
/// unsnoozed history entries while preserving their original `unread`/`read_at`
/// state for one `"Clear All"` Undo step and an independent [`ClearBatch`]
/// record. When `unread_only` is true (Deskbar's Inbox), entries must still be
/// unread and match any non-zero `instance` (`Entry::uid`). Returns
/// `(cleared_ids, batch_id, live_ids, archive_changed)`.
#[allow(clippy::too_many_arguments)]
fn clear_many_entries(
    history: &mut History,
    archive: &mut History,
    steps: &mut olo_kit::undo::History<Gone>,
    clear_batches: &mut VecDeque<ClearBatch>,
    batch_prefix: &str,
    next_seq: &mut u64,
    targets: &[(u32, u64)],
    unread_only: bool,
    now: u64,
) -> Option<(Vec<u32>, String, Vec<u32>, bool)> {
    if targets.is_empty() {
        return None;
    }
    let mut any_instance = std::collections::HashSet::new();
    let mut exact = std::collections::HashSet::new();
    for &(id, instance) in targets {
        if instance == 0 {
            any_instance.insert(id);
        } else {
            exact.insert((id, instance));
        }
    }
    let matches_target = |e: &Entry| exact.contains(&(e.id, e.uid)) || (!unread_only && any_instance.contains(&e.id)) || (unread_only && any_instance.contains(&e.id) && e.uid == 0);
    let gone = history.take_out(|_, e| !(matches_target(e) && !e.pinned && !e.snoozed() && (!unread_only || e.unread)));
    if gone.is_empty() {
        return None;
    }
    let cleared: Vec<u32> = gone.iter().map(|(_, e)| e.id).collect();
    let live: Vec<u32> = gone.iter().filter(|(_, e)| e.live).map(|(_, e)| e.id).collect();
    let archive_changed = archive.archive(gone.iter().map(|(_, e)| (e.clone(), store::Why::Cleared)), now);
    *next_seq = next_seq.wrapping_add(1).max(1);
    let batch_id = format!("{batch_prefix}-{}", *next_seq);
    clear_batches.push_back(ClearBatch { id: batch_id.clone(), entries: gone.clone() });
    while clear_batches.len() > MAX_CLEAR_BATCHES {
        clear_batches.pop_front();
    }
    steps.done("Clear All", Gone::Forgot { entries: gone, batch: Some(batch_id.clone()), only_eligible: true });
    Some((cleared, batch_id, live, archive_changed))
}

/// Reconcile saved `entries` against current `archive` and `history`: restores
/// ONLY those entries still present in `archive` and NOT already present in
/// `history` (preventing duplicates when History's Put Back already restored
/// one of them, Astra 6). Returns `(restored_ids, restored_keys, archive_changed)`.
fn reconcile_restore(history: &mut History, archive: &mut History, entries: Vec<(usize, store::Entry)>) -> (Vec<u32>, Vec<store::EntryKey>, bool) {
    let mut to_restore = Vec::new();
    let mut restored_ids = Vec::new();
    let mut restored_keys = Vec::new();
    for (idx, e) in entries {
        let key = store::EntryKey::of(&e);
        let in_archive = archive.entries.iter().any(|a| key.matches(a));
        let in_history = history.entries.iter().any(|h| key.matches(h) || h.id == e.id);
        if in_archive && !in_history {
            restored_ids.push(e.id);
            restored_keys.push(key);
            to_restore.push((idx, e));
        }
    }
    if to_restore.is_empty() {
        return (Vec::new(), Vec::new(), false);
    }
    history.put_back(to_restore);
    let archive_changed = archive.unarchive(&restored_keys);
    (restored_ids, restored_keys, archive_changed)
}

/// Pure core of [`Notifications::undo_clear`]: looks up `batch` in the
/// service's independent `clear_batches` registry (so unrelated History Undo
/// steps never cancel a live Deskbar receipt's Undo, Astra 11), consumes it
/// once, and puts back only entries still in `archive` and absent from
/// `history` (Astra 6). If History's top undo step is this exact batch, also
/// advances it to Redo with only the actually-restored entries.
fn undo_clear_batch(history: &mut History, archive: &mut History, steps: &mut olo_kit::undo::History<Gone>, clear_batches: &mut VecDeque<ClearBatch>, batch: &str) -> (Vec<u32>, bool) {
    use olo_kit::undo::Purpose;
    if batch.is_empty() {
        return (Vec::new(), false);
    }
    let Some(pos) = clear_batches.iter().position(|b| b.id == batch) else {
        return (Vec::new(), false);
    };
    let found = clear_batches.remove(pos).unwrap();
    let (restored_ids, restored_keys, archive_changed) = reconcile_restore(history, archive, found.entries);
    // Keep History's Undo/Redo stack in sync when this batch is still its top step;
    // leave unrelated History edits completely undisturbed.
    if let Some(top) = steps.take_undo() {
        let is_this = matches!(&top.change, Gone::Forgot { batch: Some(b), .. } if b == batch);
        if is_this {
            if !restored_keys.is_empty() {
                steps.record(Purpose::Undo, top.name, Gone::Back { keys: restored_keys, only_eligible: true });
            }
        } else {
            steps.record(Purpose::Redo, top.name, top.change);
        }
    }
    (restored_ids, archive_changed)
}

/// Applies one History Undo or Redo step to `history` and `archive`, recording
/// the inverse step on `steps` (with only the entries actually moved) and
/// returning the crash-safe save order.
fn apply_undo_step(history: &mut History, archive: &mut History, steps: &mut olo_kit::undo::History<Gone>, step: olo_kit::undo::Step<Gone>, purpose: olo_kit::undo::Purpose, now: u64) -> SaveOrder {
    match step.change {
        Gone::Forgot { entries: gone, only_eligible, .. } => {
            let (restored_ids, restored_keys, _) = reconcile_restore(history, archive, gone);
            if restored_ids.is_empty() {
                return SaveOrder::None;
            }
            steps.record(purpose, step.name, Gone::Back { keys: restored_keys, only_eligible });
            SaveOrder::HistoryThenArchive
        }
        Gone::Back { keys, only_eligible } => {
            // Recheck eligibility (pinned/snoozed) on Redo so a notification
            // pinned after Undo is never cleared or stripped of its pin (Astra 7).
            let gone = history.take_out(|_, e| !(keys.iter().any(|k| k.matches(e)) && (!only_eligible || (!e.pinned && !e.snoozed()))));
            if gone.is_empty() {
                return SaveOrder::None;
            }
            archive.archive(gone.iter().map(|(_, e)| (e.clone(), store::Why::Cleared)), now);
            steps.record(purpose, step.name, Gone::Forgot { entries: gone, batch: None, only_eligible });
            SaveOrder::ArchiveThenHistory
        }
        Gone::Deleted(gone) => {
            let back = gone.iter().map(|(_, e)| store::EntryKey::of(e)).collect();
            archive.put_back(gone);
            steps.record(purpose, step.name, Gone::Undeleted(back));
            SaveOrder::ArchiveOnly
        }
        Gone::Undeleted(which) => {
            let gone = archive.take_out(|_, e| !which.iter().any(|k| k.matches(e)));
            steps.record(purpose, step.name, Gone::Deleted(gone));
            SaveOrder::ArchiveOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unread(id: u32, time: u64) -> Entry {
        Entry { id, app: "Rex".into(), summary: format!("Note {id}"), unread: true, live: true, shown: true, time, ..Default::default() }
    }

    /// ClearMany removes matching unpinned notifications in one atomic Undo
    /// step, leaves pinned/snoozed/unlisted/missing entries alone, scales past
    /// the 100-step undo limit, and Undo puts every cleared notification back
    /// unread in the Inbox.
    #[test]
    fn clear_many_one_undo_step_and_pins_untouched() {
        let (mut history, mut archive) = History::load_pair(None, None);
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        // 105 unread notifications (101+ to prove Undo isn't exhausted),
        // plus one pinned (id 200) and one snoozed (id 201) and one newcomer (id 202).
        for id in 1..=105u32 {
            history.put(unread(id, 1000 + u64::from(id)));
        }
        history.put(Entry { pinned: true, ..unread(200, 2000) });
        history.put(Entry { snoozed_until: Some(u64::MAX), ..unread(201, 2001) });
        history.put(unread(202, 2002));
        let mut want: Vec<(u32, u64)> = (1..=105u32).map(|id| (id, history.newest(id).unwrap().uid)).collect();
        // Include pinned, snoozed, duplicate, and nonexistent IDs in the request.
        want.extend([(200, history.newest(200).unwrap().uid), (201, history.newest(201).unwrap().uid), (999, 999), (1, history.newest(1).unwrap().uid)]);

        let (cleared, batch, live, archive_changed) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &want, true, 5000).unwrap();
        assert_eq!(cleared.len(), 105);
        assert_eq!(live.len(), 105);
        assert!(!batch.is_empty() && archive_changed);
        // Pinned, snoozed and unlisted newcomer all stay untouched (unread & shown intact).
        let pinned = history.newest(200).unwrap();
        assert!(pinned.pinned && pinned.unread && pinned.shown && pinned.live);
        assert!(history.newest(201).unwrap().snoozed());
        assert!(history.newest(202).unwrap().unread);
        assert_eq!(history.entries.len(), 3);
        assert_eq!(archive.entries.len(), 105);
        assert_eq!(steps.undo_label(), "Undo Clear All");

        // Undo by batch id restores ALL 105 unread entries in one step and unarchives them.
        let (restored, unarchived) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert_eq!(restored.len(), 105);
        assert!(unarchived);
        assert_eq!(history.entries.len(), 108);
        assert!(archive.entries.is_empty());
        assert_eq!(history.inbox(|_| vec![], Some(60)).len(), 107, "all 105 restored + pinned 200 + newcomer 202 unread in Inbox");
        let one = history.newest(1).unwrap();
        assert!(one.unread && !one.live && !one.shown);

        // Second Undo with the same batch id is idempotent and restores nothing.
        let (again, _) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert!(again.is_empty());
    }

    /// Astra 2: batch ids never repeat across service restarts; a restarted
    /// instance rejects an earlier instance's batch id even on its first batch.
    #[test]
    fn batch_ids_never_repeat_across_service_restarts() {
        let (mut h1, mut a1) = History::load_pair(None, None);
        let (mut s1, mut b1, p1, mut seq1) = (olo_kit::undo::History::default(), VecDeque::new(), generate_batch_prefix(), 0u64);
        h1.put(unread(1, 100));
        let (_, batch_inst1, _, _) = clear_many_entries(&mut h1, &mut a1, &mut s1, &mut b1, &p1, &mut seq1, &[(1, 0)], false, 200).unwrap();

        // Simulated restart: fresh instance with fresh prefix and seq = 0, clearing a read entry.
        let (mut h2, mut a2) = History::load_pair(None, None);
        let (mut s2, mut b2, p2, mut seq2) = (olo_kit::undo::History::default(), VecDeque::new(), generate_batch_prefix(), 0u64);
        h2.put(Entry { unread: false, ..unread(2, 101) });
        let (_, batch_inst2, _, _) = clear_many_entries(&mut h2, &mut a2, &mut s2, &mut b2, &p2, &mut seq2, &[(2, 0)], false, 201).unwrap();

        assert_ne!(batch_inst1, batch_inst2, "batch ids must never collide across restarts");
        let (restored, _) = undo_clear_batch(&mut h2, &mut a2, &mut s2, &mut b2, &batch_inst1);
        assert!(restored.is_empty(), "old instance's batch id must not undo new instance's batch");
        assert!(h2.newest(2).is_none() && a2.newest(2).is_some());
    }

    /// Astra 3: ordered crash-safe save writes history before archive on Undo
    /// (archive before history on Clear), and `History::load_pair` deduplicates
    /// an interrupted pair so neither crash window loses or duplicates entries.
    #[test]
    fn crash_safe_save_order_and_recovery_never_loses_or_duplicates() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-notif-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let h_path = dir.join("history.json");
        let a_path = dir.join("archive.json");

        let (mut history, mut archive) = History::load_pair(Some(h_path.clone()), Some(a_path.clone()));
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        history.put(unread(10, 1000));
        history.put(unread(11, 1001));
        let u10 = history.newest(10).unwrap().uid;
        let u11 = history.newest(11).unwrap().uid;
        let (_, batch, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &[(10, u10), (11, u11)], true, 2000).unwrap();

        // Clear writes archive FIRST, then history SECOND.
        History::save_pair(&archive, &history, None);
        store::flush_writes();
        let (h_after_clear, a_after_clear) = History::load_pair(Some(h_path.clone()), Some(a_path.clone()));
        assert!(h_after_clear.entries.is_empty() && a_after_clear.entries.len() == 2);

        // Undo puts back into history FIRST, then shortens archive SECOND.
        let (restored, _) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert_eq!(restored, vec![10, 11]);
        // Simulate crash strictly between first rename (history saved) and second rename (archive still has 10, 11).
        history.save();
        store::flush_writes();
        // On disk right now: BOTH history.json and archive.json contain 10 and 11 (zero loss!).
        let raw_h: Vec<Entry> = serde_json::from_str(&std::fs::read_to_string(&h_path).unwrap()).unwrap();
        let raw_a: Vec<Entry> = serde_json::from_str(&std::fs::read_to_string(&a_path).unwrap()).unwrap();
        assert_eq!((raw_h.len(), raw_a.len()), (2, 2));

        // Startup recovery deduplicates archive against history: both restored entries kept, zero duplicates!
        let (rec_h, rec_a) = History::load_pair(Some(h_path.clone()), Some(a_path.clone()));
        store::flush_writes();
        assert_eq!(rec_h.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![10, 11]);
        assert!(rec_a.entries.is_empty(), "interrupted undo duplicate must be swept from archive on load");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Astra 6: Put Back on A in History followed by receipt Undo puts back
    /// ONLY B and never creates two history entries with A's ID.
    #[test]
    fn put_back_then_undo_clear_never_duplicates() {
        let (mut history, mut archive) = History::load_pair(None, None);
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        history.put(unread(10, 100));
        history.put(unread(11, 101));
        let u10 = history.newest(10).unwrap().uid;
        let u11 = history.newest(11).unwrap().uid;
        let (_, batch, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &[(10, u10), (11, u11)], true, 200).unwrap();

        // Simulate History's "Put Back" on A (id 10): moves 10 out of archive into history as read.
        let put = archive.take_out(|_, e| e.id != 10);
        for (_, mut e) in put {
            (e.archived, e.why, e.unread, e.live, e.shown) = (0, None, false, false, false);
            history.entries_mut().push(e);
        }
        assert_eq!(history.entries.len(), 1);
        assert_eq!(archive.entries.len(), 1);

        // Receipt Undo puts back ONLY B (id 11), never duplicating A (id 10).
        let (restored, _) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert_eq!(restored, vec![11]);
        assert_eq!(history.entries.iter().filter(|e| e.id == 10).count(), 1, "A must not be duplicated");
        assert!(!history.newest(10).unwrap().unread, "A keeps its Put Back read state");
        assert!(history.newest(11).unwrap().unread, "B is restored unread");
        assert!(archive.entries.is_empty());
    }

    /// Astra 7: pinning a restored notification before Edit ▸ Redo Clear All
    /// protects the newly pinned entry and keeps its pin intact.
    #[test]
    fn redo_clear_all_respects_newly_pinned_and_snoozed() {
        let (mut history, mut archive) = History::load_pair(None, None);
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        history.put(unread(10, 100));
        history.put(unread(11, 101));
        history.put(unread(12, 102));
        let targets: Vec<_> = [10, 11, 12].into_iter().map(|id| (id, history.newest(id).unwrap().uid)).collect();
        let (_, batch, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &targets, true, 200).unwrap();

        // Receipt Undo restores 10, 11, 12 and puts Clear All onto History's Redo stack.
        let (restored, _) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert_eq!(restored, vec![10, 11, 12]);
        assert!(steps.can_redo());

        // Pin 10 and snooze 11 before choosing Redo Clear All.
        history.newest_mut(10).unwrap().pinned = true;
        history.newest_mut(11).unwrap().snoozed_until = Some(u64::MAX);

        let step = steps.take_redo().unwrap();
        let order = apply_undo_step(&mut history, &mut archive, &mut steps, step, olo_kit::undo::Purpose::Redo, 300);
        assert_eq!(order, SaveOrder::ArchiveThenHistory);
        // Only 12 moved to archive; 10 stays pinned in history, 11 stays snoozed in history.
        assert!(history.newest(10).is_some_and(|e| e.pinned && e.unread));
        assert!(history.newest(11).is_some_and(|e| e.snoozed()));
        assert!(history.newest(12).is_none());
        assert_eq!(archive.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![12]);
    }

    /// Astra 8: Inbox `ClearMany(unread_only = true)` skips entries marked read
    /// meanwhile and rejects a reposted notification that reused an id after
    /// withdrawal (changed `uid`), while in-place updates preserve `uid` and
    /// CLI `unread_only = false` still clears read entries.
    #[test]
    fn inbox_clear_requires_unread_and_matching_instance_while_cli_clears_read() {
        let (mut history, mut archive) = History::load_pair(None, None);
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        history.put(unread(17, 100));
        history.put(unread(18, 101));
        history.put(unread(19, 102));
        let u17 = history.newest(17).unwrap().uid;
        let u18 = history.newest(18).unwrap().uid;
        let u19 = history.newest(19).unwrap().uid;

        // 1. Ordinary in-place replacement of 19 preserves its `uid`.
        history.put(Entry { summary: "Updated 19".into(), ..unread(19, 105) });
        assert_eq!(history.newest(19).unwrap().uid, u19, "in-place update must preserve uid");

        // 2. Entry 17 is marked read in History before Deskbar's request is handled.
        history.set_read(17, true);

        // 3. Entry 18 is withdrawn and a brand new notification reuses id 18 via tag.
        history.take_out(|_, e| e.id != 18);
        history.put(Entry { summary: "Brand new 18".into(), ..unread(18, 110) });
        let u18_new = history.newest(18).unwrap().uid;
        assert_ne!(u18_new, u18, "withdrawal + repost must mint a new uid");

        // Deskbar's Inbox ClearMany with captured (17, u17), (18, u18), (19, u19) and unread_only = true:
        // ONLY 19 (updated in place, still unread) is cleared! 17 (read) and 18 (reposted) survive!
        let (cleared, _, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &[(17, u17), (18, u18), (19, u19)], true, 200).unwrap();
        assert_eq!(cleared, vec![19]);
        assert!(history.newest(17).is_some(), "read entry 17 must not be archived by Inbox clear");
        assert!(history.newest(18).is_some_and(|e| e.summary == "Brand new 18"), "reposted 18 must survive");

        // CLI explicit history clear (`unread_only = false, instance = 0`) CAN still clear read entry 17.
        let (cli_cleared, _, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &[(17, 0)], false, 201).unwrap();
        assert_eq!(cli_cleared, vec![17]);
    }

    /// Astra 11: unrelated History edits (e.g. deleting an archived row) do
    /// NOT invalidate Deskbar's receipt Undo batch, and UndoClear leaves the
    /// unrelated History Undo step intact.
    #[test]
    fn unrelated_history_edit_does_not_cancel_receipt_undo() {
        let (mut history, mut archive) = History::load_pair(None, None);
        let mut steps = olo_kit::undo::History::default();
        let mut batches = VecDeque::new();
        let prefix = generate_batch_prefix();
        let mut seq = 0u64;

        // Pre-existing archived row X (id 5).
        archive.archive([(unread(5, 50), store::Why::Cleared)], 60);
        history.put(unread(10, 100));
        history.put(unread(11, 101));
        let u10 = history.newest(10).unwrap().uid;
        let u11 = history.newest(11).unwrap().uid;

        let (_, batch, _, _) = clear_many_entries(&mut history, &mut archive, &mut steps, &mut batches, &prefix, &mut seq, &[(10, u10), (11, u11)], true, 200).unwrap();

        // User deletes unrelated archived row 5 in History -> becomes top History Undo step.
        let deleted = archive.take_out(|_, e| e.id != 5);
        steps.done("Delete", Gone::Deleted(deleted));
        assert_eq!(steps.undo_label(), "Undo Delete");

        // Deskbar receipt Undo STILL succeeds and restores 10 and 11, leaving History's "Undo Delete" on top!
        let (restored, _) = undo_clear_batch(&mut history, &mut archive, &mut steps, &mut batches, &batch);
        assert_eq!(restored, vec![10, 11]);
        assert!(history.newest(10).is_some() && history.newest(11).is_some());
        assert_eq!(steps.undo_label(), "Undo Delete", "unrelated History undo step must stay intact");
    }

    /// Astra 16 & 23: service-door test over a real peer-to-peer D-Bus
    /// connection exercising the actual `Controls` interface, CLI dispatcher,
    /// Deskbar-style `ClearMany` + `UndoClear`, and `MAX_CLEAR_MANY` ingress cap.
    #[test]
    fn service_door_clear_many_undo_cli_and_ingress_bound() {
        let (server_sock, client_sock) = std::os::unix::net::UnixStream::pair().unwrap();
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (mut history, mut archive) = History::load_pair(None, None);
        history.put(unread(17, 100));
        history.put(unread(18, 101));
        history.put(unread(19, 102));
        history.put(Entry { pinned: true, ..unread(20, 103) });
        let u17 = history.newest(17).unwrap().uid;
        let u18 = history.newest(18).unwrap().uid;

        let state = Arc::new(Mutex::new((history, archive, olo_kit::undo::History::default(), VecDeque::new(), generate_batch_prefix(), 0u64)));
        let calls_srv = calls.clone();
        let state_srv = state.clone();
        let controls = server::Controls::new(
            Arc::new(Mutex::new("{}".into())),
            Arc::new(Mutex::new("{}".into())),
            Arc::new(move |msg| match msg {
                In::Control(Control::Dismiss(id), done) => {
                    calls_srv.lock().unwrap().push(format!("Dismiss:{id}"));
                    if let Some(d) = done {
                        let _ = d.send(());
                    }
                }
                In::ClearMany { targets, unread_only, reply } => {
                    calls_srv.lock().unwrap().push(format!("ClearMany:{unread_only}:{}", targets.len()));
                    let mut g = state_srv.lock().unwrap();
                    let (ref mut h, ref mut a, ref mut st, ref mut batches, ref prefix, ref mut seq) = *g;
                    let out = clear_many_entries(h, a, st, batches, prefix, seq, &targets, unread_only, 500).map_or_else(|| (Vec::new(), String::new(), None), |(c, b, _, _)| (c, b, None));
                    let _ = reply.send(out);
                }
                In::UndoClear { batch, reply } => {
                    calls_srv.lock().unwrap().push(format!("UndoClear:{batch}"));
                    let mut g = state_srv.lock().unwrap();
                    let (ref mut h, ref mut a, ref mut st, ref mut batches, ..) = *g;
                    let (restored, _) = undo_clear_batch(h, a, st, batches, &batch);
                    let _ = reply.send((restored, None));
                }
                In::Control(_, Some(d)) => {
                    let _ = d.send(());
                }
                _ => {}
            }),
        );

        let guid = zbus::Guid::generate();
        let server_thread = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_sock).server(guid).unwrap().p2p().serve_at(server::CONTROL_PATH, controls).unwrap().build().unwrap()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_sock).p2p().method_timeout(Duration::from_secs(5)).build().unwrap();
        let _server_conn = server_thread.join().unwrap();

        // 1. Oversized ClearMany (> MAX_CLEAR_MANY) is rejected at bus ingress before reaching UI!
        let oversized: Vec<(u32, u64)> = (0..=server::MAX_CLEAR_MANY as u32).map(|i| (i, 0)).collect();
        let err = server::call_on(&client, None, "ClearMany", &(oversized, false)).unwrap_err();
        assert!(err.to_string().contains("InvalidArgs"), "expected InvalidArgs, got {err}");
        assert!(calls.lock().unwrap().is_empty(), "oversized request must not reach UI");

        // 2. Deskbar Inbox ClearAll sends ONE ClearMany([(17, u17), (18, u18)], true), ZERO Dismiss.
        let reply = server::call_on(&client, None, "ClearMany", &(vec![(17u32, u17), (18u32, u18)], true)).unwrap();
        let (cleared, batch): (Vec<u32>, String) = reply.body().deserialize().unwrap();
        assert_eq!(cleared, vec![17, 18]);
        assert!(!batch.is_empty());
        assert_eq!(*calls.lock().unwrap(), vec!["ClearMany:true:2"]);
        // Untargeted 19 and pinned 20 survived!
        assert!(state.lock().unwrap().0.newest(19).is_some() && state.lock().unwrap().0.newest(20).is_some());

        // 3. One UndoClear(batch) restores both 17 and 18 while keeping 19 and 20 intact.
        let undo_reply = server::call_on(&client, None, "UndoClear", &(batch.clone(),)).unwrap();
        let restored: Vec<u32> = undo_reply.body().deserialize().unwrap();
        assert_eq!(restored, vec![17, 18]);
        assert!(state.lock().unwrap().0.newest(17).is_some() && state.lock().unwrap().0.newest(18).is_some() && state.lock().unwrap().0.newest(19).is_some());

        // 4. CLI `--clear 17 18` sends ONE ClearMany (never two Dismiss) and exits 0.
        calls.lock().unwrap().clear();
        let code = cli::run_on(&client, None, &["--clear".into(), "17".into(), "18".into()]);
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert_eq!(*calls.lock().unwrap(), vec!["ClearMany:false:2"]);

        // 5. CLI `--clear 20 999` (pinned + missing) clears 0 and exits non-zero (Astra 22).
        let fail_code = cli::run_on(&client, None, &["--clear".into(), "20".into(), "999".into()]);
        assert_eq!(fail_code, std::process::ExitCode::FAILURE);
    }
}
