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
//! bus. On Olo's GPUI (the GPUI kit, `olo-ui`).

mod banners;
mod cli;
mod gtk;
mod history;
mod look;
mod markup;
mod pictures;
mod server;
mod sound;
mod store;
mod web;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_kit_base::{metrics as m, watch::Watch};
use olo_protocol::{NotificationSettings, NotificationState};
use olo_ui::{
    Frame, WindowOptions,
    gpui::{self, App, AppContext as _, Context, Entity, LayerKeyboard, LayerShellLayer, Window, WindowHandle},
    widgets::{TextField, text_field},
};
use server::{Bus, Control, In, Kind, Note, Reason};
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
    /// Something may have expired (or Do Not Disturb begun or ended, or
    /// the settings or a share changed).
    Tick,
    /// An action's activation token came (or none could be made).
    Token(u64, Option<String>),
    /// Icons were found: drawn again.
    Refresh,
}

pub struct Card {
    pub note: Note,
    /// When it goes (None: when dismissed).
    pub until: Option<Instant>,
    /// The picture it was sent with.
    pub picture: Option<pictures::Source>,
    /// Its reply field, when it can be answered.
    pub reply: Option<Entity<TextField>>,
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
    bus: Arc<Mutex<Option<Bus>>>,
    /// The banners' layer surface, and whether one is being opened.
    win: Option<WindowHandle<Frame>>,
    opening: bool,
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
    proxy: olo_ui::Proxy,
    pub settings: NotificationSettings,
    /// When `notifications.json` was last read.
    settings_read: Option<SystemTime>,
    pub history: History,
    /// The History window, and what it shows.
    history_win: Option<WindowHandle<Frame>>,
    history_opening: bool,
    pub view: history::HistoryState,
    /// `List`'s answer, shared with the bus thread.
    snapshot: Arc<Mutex<String>>,
    /// What Deskbar was last told.
    told: Option<NotificationState>,
    /// The screen is being shared (`SharingState`).
    sharing: bool,
    /// The settings file and the screen shares, watched.
    _watches: Vec<Watch>,
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

    /// Whether the screen is shared now; when sharing starts (and it is to
    /// be quiet then), the banners on screen go into the history, as if
    /// they had timed out.
    fn refresh_sharing(&mut self, cx: &mut Context<Self>) {
        let sharing = !olo_protocol::sharing::SharingState::load().shares.is_empty();
        if sharing == self.sharing {
            return;
        }
        self.sharing = sharing;
        if self.sharing_quiet() {
            let ids: Vec<u32> = self.cards.iter().map(|c| c.note.id).collect();
            for id in ids {
                self.remove(cx, id, Reason::Expired);
            }
        }
    }

    /// Tell `List`, Deskbar and the disk what changed.
    fn publish(&mut self, history_changed: bool) {
        if history_changed {
            self.history.save();
        }
        let quiet = self.quiet();
        let state = NotificationState {
            unread: self.history.unread(),
            quiet,
            apps: self.history.unread_apps(),
            fade_minutes: self.settings.fade.then_some(self.settings.fade_minutes),
            show_count: self.settings.deskbar_count,
        };
        if self.told.as_ref() != Some(&state) {
            let path = NotificationState::path();
            let tmp = path.with_extension("json.new");
            if std::fs::write(&tmp, serde_json::to_string(&state).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
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
                let strength = if e.pinned { 1.0 } else { self.settings.strength(now.saturating_sub(e.time)) };
                v["strength"] = serde_json::json!((strength * 100.0).round() / 100.0);
                v
            })
            .collect();
        let list = serde_json::json!({
            "quiet": quiet,
            "unread": state.unread,
            "notifications": notes,
        });
        *self.snapshot.lock().unwrap() = list.to_string();
    }

    fn show(&mut self, cx: &mut Context<Self>, mut note: Note) {
        self.refresh_settings();
        // One of each (Preferences ▸ Notifications): the same again from
        // the same app takes the earlier one's place.
        if self.settings.one_of_each && !note.replaces {
            let text = markup::plain(&note.body);
            let earlier = store::same_as(&self.history, &self.cards.iter().map(|c| &c.note).collect::<Vec<_>>(), &note.app_name, &note.summary, &text, note.id);
            for id in earlier {
                self.take(cx, id);
                self.history.retain(|e| e.id != id);
                self.bus(|b| b.closed(id, Reason::Expired));
            }
        }
        let app = self.settings.app_via(&note.app_name, note.via.as_deref());
        let quiet = self.quiet();
        let never = self.settings.seconds == 0;
        if note.timeout == Some(server::DEFAULT_TIMEOUT) {
            note.timeout = Some(self.banner_time());
        }
        // Do Not Disturb keeps all but errors off the screen and quiet; a
        // snoozed one's news waits with it.
        let snoozed = self.history.get_mut(note.id).is_some_and(|e| e.snoozed());
        let banner = app.banners && ((!quiet && !never) || (note.kind == Kind::Error && !self.sharing_quiet())) && !snoozed;
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
            let mut e = Entry::from_note(&note, true);
            e.shown = banner;
            self.history.put(e);
            if let Some(p) = &note.picture {
                self.history.keep_picture(note.id, p);
            }
        }
        if banner {
            self.add_card(cx, note);
        } else if self.cards.iter().any(|c| c.note.id == note.id) {
            // A replacement for one on screen that may no longer show.
            self.take(cx, note.id);
        } else if !kept {
            // Neither shown nor kept: gone as far as the app is concerned.
            self.bus(|b| b.closed(note.id, Reason::Dismissed));
        }
        self.publish(kept);
        cx.notify();
    }

    fn add_card(&mut self, cx: &mut Context<Self>, note: Note) {
        // Pinned: on screen until closed.
        let pinned = self.history.newest(note.id).is_some_and(|e| e.pinned);
        let until = note.timeout.filter(|_| !pinned).map(|t| Instant::now() + t);
        if let Some(at) = until {
            self.expire_at(at, cx);
        }
        let picture = note.picture.as_ref().map(|p| pictures::Source::Note(note.id, Arc::new(pictures::Raw::from_unmultiplied(p.width, p.height, &p.rgba))));
        match self.cards.iter_mut().find(|c| c.note.id == note.id) {
            // In place: what was typed stays, the picture is made again.
            Some(old) => {
                if note.reply.is_some() && old.reply.is_none() {
                    old.reply = Some(reply_field(&note, cx));
                }
                old.note = note;
                old.until = until;
                old.picture = picture;
            }
            None => {
                let reply = note.reply.is_some().then(|| reply_field(&note, cx));
                self.cards.push(Card { note, until, picture, reply });
            }
        }
        self.open_banners(cx);
        cx.notify();
    }

    /// The banners' surface, opened once the update that asks for it is
    /// over (its first frame reads the cards).
    fn open_banners(&mut self, cx: &mut Context<Self>) {
        if self.win.is_some() || self.opening {
            return;
        }
        self.opening = true;
        let me = cx.entity();
        cx.defer(move |cx| {
            let view = me.clone();
            let options = WindowOptions::new("Notifications", WIDTH, 1.0).layer("notifications", LayerShellLayer::Top, (false, true, false, true)).layer_options(|l| {
                l.margin = (0, 0, GAP as i32, 0);
                l.keyboard = LayerKeyboard::None;
            });
            let win = olo_ui::open_window(cx, options.no_panel(), move |_, cx| cx.new(|cx| banners::Banners::new(view, cx)));
            me.update(cx, |n, cx| {
                n.opening = false;
                n.win = win;
                // Closed meanwhile (the last card went).
                if n.cards.is_empty() {
                    n.close_banners(cx);
                }
            });
        });
    }

    fn close_banners(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.win.take() {
            cx.defer(move |cx| cx.close_window(w));
        }
    }

    /// Check again after `after` (a card's time, a snooze, a token).
    fn tick_after(&self, after: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |me, cx| {
            cx.background_executor().timer(after + Duration::from_millis(20)).await;
            let _ = me.update(cx, |n, cx| n.tick(cx));
        })
        .detach();
    }

    /// A notification goes. Its card times out into the history (still
    /// open at its app, so its actions work from there); otherwise the app
    /// hears why it closed.
    pub fn remove(&mut self, cx: &mut Context<Self>, id: u32, reason: Reason) {
        let on_screen = self.take(cx, id);
        let kept = self.history.get_mut(id).is_some();
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
            if reason == Reason::Dismissed {
                e.unread = false;
            }
        }
        if on_screen || kept {
            self.bus(|b| b.closed(id, reason));
        }
        self.publish(kept);
    }

    /// Take a card away (saying nothing); whether it was there.
    fn take(&mut self, cx: &mut Context<Self>, id: u32) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.note.id != id);
        let was = self.cards.len() != before;
        if self.hovered == Some(id) {
            self.hovered = None;
        }
        self.wake(0..self.fits);
        if self.cards.is_empty() {
            self.close_banners(cx);
        }
        cx.notify();
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

    /// Cards whose time is up go (not the one under the pointer).
    fn expire(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let gone: Vec<u32> = self.cards.iter().take(self.fits).filter(|c| c.until.is_some_and(|u| u <= now) && Some(c.note.id) != self.hovered).map(|c| c.note.id).collect();
        for id in gone {
            self.remove(cx, id, Reason::Expired);
        }
        // The next to go, checked for then.
        if let Some(next) = self.cards.iter().take(self.fits).filter_map(|c| c.until).min() {
            self.expire_at(next, cx);
        }
    }

    /// Look for cards whose time is up at `at` (one timer for them all:
    /// a later one waits for the earlier, which asks again).
    fn expire_at(&mut self, at: Instant, cx: &mut Context<Self>) {
        if self.expiry.is_some_and(|e| e <= at && e > Instant::now()) {
            return;
        }
        self.expiry = Some(at);
        cx.spawn(async move |me, cx| {
            cx.background_executor().timer(at.saturating_duration_since(Instant::now()) + Duration::from_millis(20)).await;
            let _ = me.update(cx, |n, cx| {
                if n.expiry == Some(at) {
                    n.expiry = None;
                }
                n.expire(cx);
            });
        })
        .detach();
    }

    /// The pointer went over a card (or off them): one under it stays,
    /// and goes a moment after it leaves.
    pub fn hover(&mut self, id: Option<u32>, cx: &mut Context<Self>) {
        if self.hovered == id {
            return;
        }
        if let Some(left) = self.hovered
            && let Some(card) = self.cards.iter_mut().find(|c| c.note.id == left)
            && card.until.is_some_and(|u| u < Instant::now() + Duration::from_secs(2))
        {
            let at = Instant::now() + Duration::from_secs(2);
            card.until = Some(at);
            self.expire_at(at, cx);
        }
        self.hovered = id;
        cx.notify();
    }

    /// Run action `key` of notification `id` (`from`: the window clicked
    /// in, which makes the activation token): the app gets the token
    /// first, so it may bring its window forward. The card goes at once;
    /// the app hears it closed after the action.
    pub fn invoke(&mut self, cx: &mut Context<Self>, from: Option<&Window>, id: u32, key: String) {
        let resident = self.cards.iter().any(|c| c.note.id == id && c.note.resident);
        let pending = Pending { id, key, close: !resident, since: Instant::now() };
        match from {
            Some(window) => {
                let tag = self.next_tag;
                self.next_tag += 1;
                self.pending.insert(tag, pending);
                let proxy = self.proxy.clone();
                window.activation_token(move |token| proxy.send(Msg::Token(tag, token)));
                self.tick_after(TOKEN_WAIT, cx);
            }
            None => self.send(pending, None),
        }
        if let Some(e) = self.history.get_mut(id) {
            e.unread = false;
            if !resident {
                e.live = false;
                e.shown = false;
            }
        }
        if !resident {
            self.take(cx, id);
        }
        self.publish(true);
        cx.notify();
    }

    /// The reply typed into note `id`'s card: sent, and the card goes.
    pub fn reply(&mut self, cx: &mut Context<Self>, id: u32, text: &str) {
        self.bus(|b| b.replied(id, text));
        self.remove(cx, id, Reason::Dismissed);
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
    pub fn open_history(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.history_win {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.activate_window());
            });
            return;
        }
        if self.history_opening {
            return;
        }
        self.history_opening = true;
        let me = cx.entity();
        cx.defer(move |cx| {
            let view = me.clone();
            let win = olo_ui::open_window(cx, WindowOptions::new("Notifications", 400.0, 560.0).min_size(360.0, 240.0), move |window, cx| cx.new(|cx| history::HistoryWindow::new(view, window, cx)));
            if let Some(w) = win {
                let closing = me.clone();
                let _ = w.update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, move |_, cx| {
                        closing.update(cx, |n, cx| n.history_closed(cx));
                        true
                    });
                });
            }
            me.update(cx, |n, _| {
                n.history_opening = false;
                n.history_win = win;
            });
        });
    }

    /// The History window closed: what was read while it was open moves.
    fn history_closed(&mut self, cx: &mut Context<Self>) {
        self.history_win = None;
        self.view.held.clear();
        self.view.selected = None;
        cx.notify();
    }

    pub fn close_history(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.history_win {
            self.history_closed(cx);
            cx.defer(move |cx| cx.close_window(w));
        }
    }

    /// Mark notification `id` read (or unread again).
    pub fn set_read(&mut self, cx: &mut Context<Self>, id: u32, read: bool) {
        if self.history.set_read(id, read) {
            self.publish(true);
            cx.notify();
        }
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        if self.history.mark_read() {
            self.publish(true);
        }
        cx.notify();
    }

    /// Forget what has faded away; tell the apps of those still open.
    fn fade_out(&mut self, cx: &mut Context<Self>) {
        let Some(live) = self.history.fade_out(&self.settings, now_secs()) else { return };
        for id in live {
            self.bus(|b| b.closed(id, Reason::Expired));
        }
        self.publish(true);
        cx.notify();
    }

    /// Do Not Disturb on for `minutes` (0: until turned off), or off.
    pub fn set_quiet(&mut self, on: bool, minutes: u32, cx: &mut Context<Self>) {
        self.refresh_settings();
        self.settings.dnd = on;
        self.settings.dnd_until = (on && minutes > 0).then(|| now_secs() + minutes as u64 * 60);
        self.save_settings();
        if let Some(until) = self.settings.dnd_until {
            self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())), cx);
        }
        self.publish(false);
        cx.notify();
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            tracing::warn!("notifications: can't save settings: {e}");
        }
        self.settings_read = std::fs::metadata(NotificationSettings::path()).and_then(|m| m.modified()).ok();
    }

    fn control(&mut self, cx: &mut Context<Self>, c: Control) {
        match c {
            Control::Invoke(id, key) => {
                if self.history.get_mut(id).is_some() || self.cards.iter().any(|c| c.note.id == id) {
                    // From a program, not a click: no token to hand on.
                    self.invoke(cx, None, id, key);
                }
            }
            Control::Dismiss(id) => self.remove(cx, id, Reason::Dismissed),
            Control::ClearHistory => self.clear_history(cx),
            Control::ShowHistory => self.open_history(cx),
            Control::DoNotDisturb(on, minutes) => self.set_quiet(on, minutes, cx),
            Control::SetRead(id, read) => self.set_read(cx, id, read),
            Control::MarkAllRead => self.mark_all_read(cx),
            Control::Pin(id, on) => self.set_pinned(cx, id, on),
            Control::Snooze(id, minutes) => self.snooze(cx, id, now_secs() + minutes as u64 * 60),
        }
        cx.notify();
    }

    /// Pin a note (it stays put: no fading, no going with Clear All, and
    /// on screen until closed), or unpin it.
    pub fn set_pinned(&mut self, cx: &mut Context<Self>, id: u32, on: bool) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.pinned = on;
        let mut again = None;
        if let Some(c) = self.cards.iter_mut().find(|c| c.note.id == id) {
            c.until = if on { None } else { c.note.timeout.map(|t| Instant::now() + t) };
            again = c.until;
        }
        if let Some(at) = again {
            self.expire_at(at, cx);
        }
        self.publish(true);
        cx.notify();
    }

    /// Put a note away until `until` (seconds since the epoch): out of
    /// sight (off the screen too) and not counted, then back as new
    /// (`wake_snoozed`).
    pub fn snooze(&mut self, cx: &mut Context<Self>, id: u32, until: u64) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.snoozed_until = Some(until);
        e.shown = false;
        self.take(cx, id);
        if self.view.selected == Some(id) {
            self.view.selected = None;
        }
        self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())), cx);
        self.publish(true);
        cx.notify();
    }

    /// `note_menu`'s command for the note `id`.
    pub fn note_command(&mut self, cx: &mut Context<Self>, id: u32, command: &str) {
        if command == "pin" {
            let on = !self.history.newest(id).is_some_and(|e| e.pinned);
            self.set_pinned(cx, id, on);
        } else if let Some((minutes, _)) = command.strip_prefix("snooze:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| SNOOZES.get(i)) {
            self.snooze(cx, id, minutes.map_or_else(tomorrow_morning, |m| now_secs() + m as u64 * 60));
        }
    }

    /// Snoozed notes whose time has come: back as new, unread and (unless
    /// banners are off or it is quiet) on screen again.
    fn wake_snoozed(&mut self, cx: &mut Context<Self>) {
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
            e.unread = true;
            e.time = now;
            let banner = self.settings.app_via(&e.app, e.via.as_deref()).banners && !quiet && !never;
            e.shown = banner;
            let note = e.to_note(Some(time));
            if banner {
                self.add_card(cx, note);
            }
        }
        self.publish(true);
        cx.notify();
    }

    /// Forget the history (but what is pinned or snoozed); what is still
    /// open at its app closes.
    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        let kept = |e: &Entry| e.shown || e.pinned || e.snoozed();
        let live: Vec<u32> = self.history.entries.iter().filter(|e| e.live && !kept(e)).map(|e| e.id).collect();
        for id in live {
            self.bus(|b| b.closed(id, Reason::Dismissed));
        }
        self.history.retain(kept);
        self.publish(true);
        cx.notify();
    }

    /// Time passed, or something it depends on changed.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.flush_pending();
        self.wake_snoozed(cx);
        // Do Not Disturb may have begun or ended (a share too).
        self.refresh_settings();
        self.refresh_sharing(cx);
        self.fade_out(cx);
        self.publish(false);
        self.expire(cx);
        // The history fades on, its times say so.
        cx.notify();
    }

    fn message(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::In(In::Show(note)) => self.show(cx, *note),
            Msg::In(In::Close(id)) => self.remove(cx, id, Reason::Closed),
            Msg::In(In::Control(c, done)) => {
                self.control(cx, c);
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Msg::Tick => self.tick(cx),
            Msg::Token(tag, token) => {
                if let Some(p) = self.pending.remove(&tag) {
                    self.send(p, token.as_deref());
                }
            }
            Msg::Refresh => {}
        }
        cx.notify();
    }
}

/// A card's reply field (Return sends, as Send does).
fn reply_field(note: &Note, cx: &mut Context<Notifications>) -> Entity<TextField> {
    let hint = note.reply.clone().unwrap_or_default();
    text_field(cx, move |f| f.name("Reply").hint(hint))
}

/// Bring forward a window of the app with this desktop entry or name (a
/// click on a note with no default action), off the UI thread.
pub fn raise_app(desktop_entry: Option<&str>, app_name: &str) {
    let mut ids: Vec<String> = desktop_entry.iter().map(|e| e.trim_end_matches(".desktop").to_lowercase()).collect();
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
    std::thread::spawn(move || {
        use olo_protocol::{Client, Reply, Request};
        let Ok(mut client) = Client::connect() else { return };
        let Ok(Reply::Windows { windows }) = client.request(&Request::ListWindows) else { return };
        let matches = |app_id: &str| {
            let a = app_id.to_lowercase();
            ids.iter().any(|i| *i == a || a.rsplit('.').next() == Some(i.as_str()) || i.rsplit('.').next() == Some(a.as_str()))
        };
        if let Some(w) = windows.iter().find(|w| matches(&w.app_id)) {
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
pub fn paint_mark(window: &mut Window, cx: &mut App, r: gpui::Bounds<gpui::Pixels>, app: &str, accent: Option<[u8; 3]>, via: bool, names: &[String]) {
    if !via && names.iter().any(|n| olo_ui::icons::paint(window, r, n)) {
        return;
    }
    if via || (accent.is_some() && !app.trim().is_empty()) {
        let (letter, fill) = web::tile(app, accent);
        return olo_ui::style::monogram(window, cx, r, &letter, fill);
    }
    let _ = olo_ui::icons::paint(window, r, "application");
}

/// An app as notes name it: (its desktop entry, its name).
type AppKey = (Option<String>, String);

/// Each app's own icon names, once looked up (None: being looked up).
static APP_ICONS: Mutex<Option<HashMap<AppKey, Option<Vec<String>>>>> = Mutex::new(None);

/// Where to say that icons were found (the windows are drawn again).
static REFRESH: std::sync::OnceLock<olo_ui::Proxy> = std::sync::OnceLock::new();

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
        names.extend(olo_xdg::icon_for_app(e.trim_end_matches(".desktop")));
    }
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
pub fn note_menu(menu: olo_ui::Menu, pinned: Option<bool>) -> olo_ui::Menu {
    let mut snooze = olo_ui::Menu::new("Snooze");
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
    // Anything else is a command for the running service.
    if !args.is_empty() {
        return cli::run(address.as_deref(), &args);
    }
    if address.is_none() && std::env::var_os("OLO_REAL_SESSION").is_none() {
        eprintln!("olo-notifications: not in a real Olo session (use --bus ADDRESS to try it)");
        return std::process::ExitCode::FAILURE;
    }
    olo_preview::audio::set_identity("Notifications", "olo-notifications");
    olo_ui::run(APP_ID, move |cx| {
        olo_ui::stay_resident(cx);
        let proxy = olo_ui::proxy(cx);
        let _ = REFRESH.set(proxy.clone());
        let bus = Arc::new(Mutex::new(None));
        let snapshot = Arc::new(Mutex::new(String::new()));
        let history = History::load(Some(History::path()));
        let first = history.entries.iter().map(|e| e.id).max().unwrap_or(0).wrapping_add(1);
        warm_app_icons(history.entries.iter().map(|e| (e.desktop_entry.clone(), e.app.clone())).collect(), || {});
        // Another notification service may have the name (a desktop
        // started before ours): wait for it to go rather than take it.
        let (slot, to_ui, shared) = (bus.clone(), proxy.clone(), snapshot.clone());
        std::thread::Builder::new()
            .name("notifications-bus".into())
            .spawn(move || {
                let mut said = false;
                loop {
                    let to_ui = to_ui.clone();
                    match Bus::serve(address.as_deref(), first, shared.clone(), move |m| {
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
        // a share starting or ending is news too: both watched (nothing
        // read while nothing changes).
        let mut watches = Vec::new();
        for path in [NotificationSettings::path(), olo_protocol::sharing::SharingState::path()] {
            let ticker = proxy.clone();
            watches.extend(olo_kit_base::watch::watch([path], move |_| ticker.send(Msg::Tick)));
        }
        let app = cx.new(|cx: &mut Context<Notifications>| {
            let mut app = Notifications {
                bus,
                win: None,
                opening: false,
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
                history_win: None,
                history_opening: false,
                view: history::HistoryState::default(),
                snapshot,
                told: None,
                sharing: false,
                _watches: watches,
            };
            app.refresh_sharing(cx);
            app.refresh_settings();
            app.fade_out(cx);
            // Snoozed ones back on time.
            app.wake_snoozed(cx);
            // Pinned banners on screen before a restart (an upgrade) are again.
            let pinned: Vec<Note> = app.history.entries.iter().filter(|e| e.shown).map(|e| e.to_note(Some(app.banner_time()))).collect();
            for note in pinned {
                app.add_card(cx, note);
            }
            let now = now_secs();
            for until in app.history.entries.iter().filter_map(|e| e.snoozed_until).collect::<Vec<_>>() {
                app.tick_after(Duration::from_secs(until.saturating_sub(now)), cx);
            }
            app.publish(false);
            // A scheduled Do Not Disturb begins and ends on the minute, and
            // the history's times and fading move on: a tick each minute.
            cx.spawn(async move |me, cx| {
                loop {
                    cx.background_executor().timer(to_next_minute()).await;
                    if me.update(cx, |n, cx| n.tick(cx)).is_err() {
                        return;
                    }
                }
            })
            .detach();
            app
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            if let Ok(msg) = message.downcast::<Msg>() {
                messages.update(cx, |n, cx| n.message(*msg, cx));
            }
        });
        // After an upgrade: the History window as it was.
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            let n = saving.read(cx);
            n.history_win.map(|_| serde_json::json!({ "history": true, "search": n.view.search, "folded": n.view.folded, "selected": n.view.selected }).to_string())
        });
        if let Some(state) = olo_ui::restored_state().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            && state["history"] == true
        {
            app.update(cx, |n, cx| {
                n.view.search = state["search"].as_str().unwrap_or_default().to_owned();
                n.view.folded = serde_json::from_value(state["folded"].clone()).unwrap_or_default();
                n.view.selected = state["selected"].as_u64().map(|id| id as u32);
                n.open_history(cx);
            });
        }
    });
    std::process::ExitCode::SUCCESS
}
