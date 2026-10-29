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
//! bus.

mod cli;
mod gtk;
mod history;
mod look;
mod markup;
mod server;
mod sound;
mod store;
mod web;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use server::{Bus, Control, In, Kind, Note, Reason};
use store::{Entry, History};
use olo_kit::{
    App, Cx, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Align, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::{Button, TextControl},
};
use olo_protocol::{HistoryView, NotificationSettings, NotificationState};

const APP_ID: &str = "olo.notifications";
/// A card's width (logical): the Deskbar's, so banners line up under
/// it at the screen's right edge, in a column that is usually free.
const WIDTH: f32 = m::DESKBAR_WIDTH;
const PAD: f32 = 8.0;
/// A picture the app sent (a contact, a site); without one the group's
/// header already shows whose it is, and the text takes the width.
const ICON: f32 = 24.0;
/// Between groups, and from the display's edges.
const GAP: f32 = 8.0;
/// A group's title row: the app's icon and name.
const HEADER: f32 = 22.0;
/// Room kept clear above and below the cards on screen.
const SCREEN_EDGE: f32 = 32.0;
/// At most this many at once; the rest wait their turn.
const SHOWN: usize = 4;
const BODY_ROWS: usize = 5;
/// How long a click waits for its activation token before the action
/// goes without one.
const TOKEN_WAIT: Duration = Duration::from_millis(500);

enum Msg {
    In(In),
    /// Something may have expired (or Do Not Disturb begun or ended).
    Tick,
}

struct Card {
    note: Note,
    /// When it goes (None: when dismissed).
    until: Option<Instant>,
    picture: Option<TextureHandle>,
    /// What is typed into its reply field.
    reply: String,
}

/// An action chosen, waiting for the activation token to send with it.
struct Pending {
    id: u32,
    key: String,
    /// Say it closed, after the action.
    close: bool,
    since: Instant,
}

struct Notifications {
    bus: Arc<Mutex<Option<Bus>>>,
    win: Option<WindowId>,
    /// Oldest first.
    cards: Vec<Card>,
    /// How many of them are on screen: up to SHOWN, as many as fit.
    fits: usize,
    /// The window's height last asked for.
    height: u32,
    /// Whether the window takes the keyboard (a reply field shows).
    keyboard: bool,
    pending: HashMap<u64, Pending>,
    next_tag: u64,
    proxy: olo_kit::Proxy,
    settings: NotificationSettings,
    /// When `notifications.json` was last read.
    settings_read: Option<SystemTime>,
    history: History,
    /// The History window, and what it shows.
    history_win: Option<WindowId>,
    view: history::HistoryState,
    /// `List`'s answer, shared with the bus thread.
    snapshot: Arc<Mutex<String>>,
    /// What Deskbar was last told.
    told: Option<NotificationState>,
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

fn now_secs() -> u64 {
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

    fn quiet(&self) -> bool {
        self.settings.quiet(now_secs(), local_minute())
    }

    /// Tell `List`, Deskbar and the disk what changed.
    fn publish(&mut self, history_changed: bool) {
        if history_changed {
            self.history.save();
        }
        let quiet = self.quiet();
        let state = NotificationState { unread: self.history.unread(), quiet };
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

    fn show(&mut self, cx: &mut Cx, mut note: Note) {
        self.refresh_settings();
        let app = self.settings.app_via(&note.app_name, note.via.as_deref());
        let quiet = self.quiet();
        let never = self.settings.seconds == 0;
        if note.timeout == Some(server::DEFAULT_TIMEOUT) {
            note.timeout = Some(self.banner_time());
        }
        // Do Not Disturb keeps all but errors off the screen and quiet; a
        // snoozed one's news waits with it.
        let snoozed = self.history.get_mut(note.id).is_some_and(|e| e.snoozed());
        let banner = app.banners && ((!quiet && !never) || note.kind == Kind::Error) && !snoozed;
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
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    fn add_card(&mut self, cx: &mut Cx, note: Note) {
        // Pinned: on screen until closed.
        let pinned = self.history.newest(note.id).is_some_and(|e| e.pinned);
        let until = note.timeout.filter(|_| !pinned).map(|t| Instant::now() + t);
        if let Some(t) = note.timeout.filter(|_| !pinned) {
            self.tick_after(t);
        }
        match self.cards.iter_mut().find(|c| c.note.id == note.id) {
            // In place: what was typed stays, the picture is made again.
            Some(old) => {
                old.note = note;
                old.until = until;
                old.picture = None;
            }
            None => self.cards.push(Card { note, until, picture: None, reply: String::new() }),
        }
        if self.win.is_none() {
            self.win = Some(cx.open_window(WindowOptions {
                title: "Notifications".into(),
                size: (WIDTH as u32, 1),
                kind: WindowKind::Layer(LayerOptions {
                    layer: Layer::Top,
                    anchor: (false, true, false, true),
                    exclusive_zone: 0,
                    margin: (0, 0, GAP as i32, 0),
                    keyboard: olo_kit::Keyboard::None,
                    namespace: "notifications".into(),
                    output: None,
                }),
                panel: false,
                ..Default::default()
            }));
            self.height = 1;
            self.keyboard = false;
        }
        self.redraw(cx);
    }

    fn tick_after(&self, after: Duration) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after + Duration::from_millis(20));
            proxy.send(Msg::Tick);
        });
    }

    /// A notification goes. Its card times out into the history (still
    /// open at its app, so its actions work from there); otherwise the app
    /// hears why it closed.
    fn remove(&mut self, cx: &mut Cx, id: u32, reason: Reason) {
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
    fn take(&mut self, cx: &mut Cx, id: u32) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.note.id != id);
        let was = self.cards.len() != before;
        // The history shows which are on screen.
        if let Some(w) = self.history_win.filter(|_| was) {
            cx.redraw(w);
        }
        self.wake(0..self.fits);
        if self.cards.is_empty() {
            if let Some(w) = self.win.take() {
                cx.close_window(w);
            }
        } else {
            self.redraw(cx);
        }
        was
    }

    /// Lay out the cards in `groups`, each under its app's title, one
    /// after another; with the height they take (and a GAP).
    fn lay_cards(&self, ui: &egui::Ui, p: &olo_kit::theme::Palette, room: &look::Room, groups: &[(String, Vec<usize>)]) -> (Vec<Frame>, f32) {
        let mut frames = Vec::new();
        let mut y = 0.0;
        for (app, members) in groups {
            let top = y;
            y += HEADER;
            let mut laid = Vec::new();
            for &i in members {
                let card = &self.cards[i];
                let ink = banner_ink(p, &card.note.look);
                let face = look::lay(ui, &card_face(card), &ink, room);
                let mut h = PAD + face.height + PAD;
                if card.note.reply.is_some() {
                    h += m::CONTROL_HEIGHT + PAD;
                }
                if card.note.actions.iter().any(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(card.note.via.as_deref(), k)) {
                    h += m::CONTROL_HEIGHT + PAD;
                }
                let rect = Rect::from_min_size(pos2(0.0, y), vec2(WIDTH, h.ceil()));
                y += rect.height();
                laid.push(Laid { i, rect, face, ink });
            }
            frames.push((app.clone(), Rect::from_min_max(pos2(0.0, top), pos2(WIDTH, y)), laid));
            y += GAP;
        }
        (frames, y)
    }

    /// The ones waiting have their time from when they appear (a little,
    /// if theirs ran out while they waited).
    fn wake(&mut self, cards: std::ops::Range<usize>) {
        let now = Instant::now();
        let mut later = Vec::new();
        for c in self.cards.iter_mut().take(cards.end).skip(cards.start) {
            if let (Some(t), Some(until)) = (c.note.timeout, c.until)
                && until < now + Duration::from_secs(1)
            {
                c.until = Some(now + t.min(Duration::from_secs(3)).max(Duration::from_secs(1)));
                later.push(t);
            }
        }
        for t in later {
            self.tick_after(t.min(Duration::from_secs(3)));
        }
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(w) = self.win {
            cx.redraw(w);
        }
    }

    fn expire(&mut self, cx: &mut Cx, hovered: Option<u32>) {
        let now = Instant::now();
        let gone: Vec<u32> = self
            .cards
            .iter()
            .take(self.fits)
            .filter(|c| c.until.is_some_and(|u| u <= now) && Some(c.note.id) != hovered)
            .map(|c| c.note.id)
            .collect();
        for id in gone {
            self.remove(cx, id, Reason::Expired);
        }
    }

    /// Run action `key` of notification `id` (from `window`, which asks
    /// for the activation token): the app gets the token first, so it may
    /// bring its window forward. The card goes at once; the app hears it
    /// closed after the action.
    fn invoke(&mut self, cx: &mut Cx, window: Option<WindowId>, id: u32, key: String) {
        let resident = self.cards.iter().any(|c| c.note.id == id && c.note.resident);
        let pending = Pending { id, key, close: !resident, since: Instant::now() };
        // The token first: taking the last card closes the window.
        match window {
            Some(win) => {
                let tag = self.next_tag;
                self.next_tag += 1;
                self.pending.insert(tag, pending);
                cx.activation_token(win, tag);
                self.tick_after(TOKEN_WAIT);
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

    fn open_history(&mut self, cx: &mut Cx) {
        match self.history_win {
            Some(w) => cx.activate(w),
            None => self.history_win = Some(cx.open_window(WindowOptions::new("Notifications", 400, 560).min_size(360, 240))),
        }
    }

    /// Mark notification `id` read (or unread again).
    fn set_read(&mut self, cx: &mut Cx, id: u32, read: bool) {
        if self.history.set_read(id, read) {
            self.publish(true);
            if let Some(w) = self.history_win {
                cx.redraw(w);
            }
        }
    }

    fn mark_all_read(&mut self, cx: &mut Cx) {
        if self.history.mark_read() {
            self.publish(true);
        }
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// Forget what has faded away; tell the apps of those still open.
    fn fade_out(&mut self, cx: &mut Cx) {
        let Some(live) = self.history.fade_out(&self.settings, now_secs()) else { return };
        for id in live {
            self.bus(|b| b.closed(id, Reason::Expired));
        }
        self.publish(true);
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// Do Not Disturb on for `minutes` (0: until turned off), or off.
    fn set_quiet(&mut self, on: bool, minutes: u32) {
        self.refresh_settings();
        self.settings.dnd = on;
        self.settings.dnd_until = (on && minutes > 0).then(|| now_secs() + minutes as u64 * 60);
        self.save_settings();
        if let Some(until) = self.settings.dnd_until {
            self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())));
        }
        self.publish(false);
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            tracing::warn!("notifications: can't save settings: {e}");
        }
        self.settings_read = std::fs::metadata(NotificationSettings::path()).and_then(|m| m.modified()).ok();
    }

    fn control(&mut self, cx: &mut Cx, c: Control) {
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
            Control::DoNotDisturb(on, minutes) => self.set_quiet(on, minutes),
            Control::SetRead(id, read) => self.set_read(cx, id, read),
            Control::MarkAllRead => self.mark_all_read(cx),
            Control::Pin(id, on) => self.set_pinned(cx, id, on),
            Control::Snooze(id, minutes) => self.snooze(cx, id, now_secs() + minutes as u64 * 60),
        }
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// Pin a note (it stays put: no fading, no going with Clear All, and
    /// on screen until closed), or unpin it.
    fn set_pinned(&mut self, cx: &mut Cx, id: u32, on: bool) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.pinned = on;
        let mut again = None;
        if let Some(c) = self.cards.iter_mut().find(|c| c.note.id == id) {
            c.until = if on { None } else { c.note.timeout.map(|t| Instant::now() + t) };
            again = c.note.timeout.filter(|_| !on);
        }
        if let Some(t) = again {
            self.tick_after(t);
        }
        self.publish(true);
        self.redraw(cx);
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// Put a note away until `until` (seconds since the epoch): out of
    /// sight (off the screen too) and not counted, then back as new
    /// (`wake_snoozed`).
    fn snooze(&mut self, cx: &mut Cx, id: u32, until: u64) {
        let Some(e) = self.history.newest_mut(id) else { return };
        e.snoozed_until = Some(until);
        e.shown = false;
        self.take(cx, id);
        if self.view.selected == Some(id) {
            self.view.selected = None;
        }
        self.tick_after(Duration::from_secs(until.saturating_sub(now_secs())));
        self.publish(true);
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// `note_menu`'s command for the note `id`.
    fn note_command(&mut self, cx: &mut Cx, id: u32, command: &str) {
        if command == "pin" {
            let on = !self.history.newest(id).is_some_and(|e| e.pinned);
            self.set_pinned(cx, id, on);
        } else if let Some((minutes, _)) = command.strip_prefix("snooze:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| SNOOZES.get(i)) {
            self.snooze(cx, id, minutes.map_or_else(tomorrow_morning, |m| now_secs() + m as u64 * 60));
        }
    }

    /// Snoozed notes whose time has come: back as new, unread and (unless
    /// banners are off or it is quiet) on screen again.
    fn wake_snoozed(&mut self, cx: &mut Cx) {
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
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }

    /// Forget the history (but what is pinned or snoozed); what is still
    /// open at its app closes.
    fn clear_history(&mut self, cx: &mut Cx) {
        let kept = |e: &Entry| e.shown || e.pinned || e.snoozed();
        let live: Vec<u32> = self.history.entries.iter().filter(|e| e.live && !kept(e)).map(|e| e.id).collect();
        for id in live {
            self.bus(|b| b.closed(id, Reason::Dismissed));
        }
        self.history.retain(kept);
        self.publish(true);
        if let Some(w) = self.history_win {
            cx.redraw(w);
        }
    }
}

/// Bring forward a window of the app with this desktop entry or name (a
/// click on a note with no default action), off the UI thread.
fn raise_app(desktop_entry: Option<&str>, app_name: &str) {
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
fn named_icons(ctx: &egui::Context, icon: Option<&str>, desktop_entry: Option<&str>, app: &str) -> Vec<String> {
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
    names.extend(app_icons(ctx, desktop_entry, app));
    names
}

/// The mark of who a note is from, filling `r`: a website's tile; else
/// the first of `names` there is; else, for an app that gave its colour
/// but has no icon, its initial on that colour; else `generic`.
fn paint_mark(painter: &egui::Painter, r: Rect, app: &str, accent: Option<[u8; 3]>, via: bool, names: &[String], generic: &str) {
    if !via && names.iter().any(|n| icons::paint(painter, r, n)) {
        return;
    }
    if via || (accent.is_some() && !app.trim().is_empty()) {
        let (letter, fill) = web::tile(app, accent);
        return style::monogram(painter, r, &letter, fill);
    }
    let _ = icons::paint(painter, r, generic);
}

/// An app as notes name it: (its desktop entry, its name).
type AppKey = (Option<String>, String);

/// Each app's own icon names, once looked up (None: being looked up).
static APP_ICONS: Mutex<Option<HashMap<AppKey, Option<Vec<String>>>>> = Mutex::new(None);

/// The sending app's own icon names. Finding them can read every desktop
/// entry on the system, milliseconds a time, so a worker looks each app
/// up once ([`warm_app_icons`]) and they are kept; until then there are
/// none, and `ctx` repaints when they come.
fn app_icons(ctx: &egui::Context, desktop_entry: Option<&str>, app: &str) -> Vec<String> {
    let key = (desktop_entry.map(str::to_owned), app.to_owned());
    if let Some(known) = APP_ICONS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return known.clone().unwrap_or_default();
    }
    let ctx = ctx.clone();
    warm_app_icons(vec![key], move || ctx.request_repaint());
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

/// Text laid out for a card.
fn galley(ui: &egui::Ui, text: &str, color: Color32, width: f32, rows: usize, font: egui::FontId) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The link under `pos` in a body drawn at `origin`.
fn link_at(body: &[markup::Span], g: &egui::Galley, origin: egui::Pos2, pos: egui::Pos2) -> Option<String> {
    let local = pos - origin;
    if !Rect::from_min_size(egui::Pos2::ZERO, g.size()).contains(local.to_pos2()) {
        return None;
    }
    let at = g.cursor_from_pos(local).index.0;
    let mut start = 0;
    for s in body {
        let n = s.text.chars().count();
        if at >= start && at < start + n {
            return s.link.clone();
        }
        start += n;
    }
    None
}

/// A card on screen, laid out: its index in the cards, where it goes,
/// its face and colours.
struct Laid {
    i: usize,
    rect: Rect,
    face: look::Laid,
    ink: look::Ink,
}

/// A group of cards on screen: its app, frame and members.
type Frame = (String, Rect, Vec<Laid>);

/// What a card on screen shows.
fn card_face(card: &Card) -> look::Face<'_> {
    look::Face {
        title: &card.note.summary,
        body: &card.note.body,
        look: &card.note.look,
        progress: card.note.progress,
        picture: card.picture.as_ref().map_or(look::Picture::None, look::Picture::Texture),
        failed: card.note.kind == Kind::Error,
    }
}

/// The colours of a card on screen (on the panel's grey).
fn banner_ink(p: &olo_kit::theme::Palette, note_look: &olo_protocol::NoteLook) -> look::Ink {
    look::Ink {
        text: c(p.panel_text),
        soft: c(p.panel_text),
        dim: style::disabled_text(p),
        link: c(p.keyboard_navigation),
        balloon: c(p.document_background),
        balloon_edge: style::darken(p.panel_background, 1.25),
        accent: note_look.accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)),
    }
}

/// A time in the history: "14:05" today, else "Oct 26, 14:05".
fn when(secs: u64) -> String {
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

/// What happened to a card this frame.
enum Did {
    Dismiss,
    Action(String),
    /// A click with no default action: the app comes forward.
    Raise,
    Reply(String),
    Link(String),
    /// Its menu's command (`note_menu`).
    Command(String),
}

impl App for Notifications {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        if Some(window) == self.history_win {
            return self.history_ui(cx, window, ui);
        }
        let p = style::palette(ui.ctx());
        let hover = ui.input(|i| i.pointer.hover_pos());
        // Pictures sent, made textures before they're measured.
        for card in self.cards.iter_mut().take(SHOWN) {
            if card.picture.is_none()
                && let Some(pic) = &card.note.picture
            {
                let img = ColorImage::from_rgba_unmultiplied([pic.width, pic.height], &pic.rgba);
                card.picture = Some(ui.ctx().load_texture(format!("note-{}", card.note.id), img, TextureOptions::LINEAR));
            }
        }
        let room = look::Room { width: WIDTH - 2.0 * PAD, title_rows: 2, body_rows: BODY_ROWS, title_reserve: 16.0, picture: ICON, image_max: 120.0, fields_max: 4 };
        // As many as fit on the screen (one, at least), grouped by app (in
        // the order the apps first show), oldest first in each; the rest
        // wait their turn.
        let most = cx.screen_size().map_or(f32::INFINITY, |(_, h)| h as f32 - 2.0 * SCREEN_EDGE);
        let mut n = self.cards.len().min(SHOWN);
        let (shown, frames, y) = loop {
            let shown: Vec<usize> = (0..n).collect();
            let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in &shown {
                let app = self.cards[i].note.app_name.clone();
                match groups.iter_mut().find(|(a, _)| *a == app) {
                    Some((_, members)) => members.push(i),
                    None => groups.push((app, vec![i])),
                }
            }
            let (frames, y) = self.lay_cards(ui, &p, &room, &groups);
            if n <= 1 || y - GAP <= most {
                break (shown, frames, y);
            }
            n -= 1;
        };
        if n > self.fits {
            self.wake(self.fits..n);
        }
        self.fits = n;
        let total = (y - GAP).max(1.0).ceil() as u32;
        if total != self.height {
            self.height = total;
            if let Some(w) = self.win {
                cx.resize(w, (WIDTH as u32, total));
            }
        }
        // The keyboard only while there is something to type into.
        let wants_keys = shown.iter().any(|&i| self.cards[i].note.reply.is_some());
        if wants_keys != self.keyboard
            && let Some(w) = self.win
        {
            self.keyboard = wants_keys;
            cx.set_keyboard(w, if wants_keys { olo_kit::Keyboard::OnDemand } else { olo_kit::Keyboard::None });
        }
        let mut hovered = None;
        let mut did = Vec::new();
        let painter = ui.painter().clone();
        for (app, frame, laid) in frames {
            painter.rect_filled(frame, 0.0, c(p.panel_background));
            style::frame_rect(&painter, frame, style::darken(p.panel_background, 1.6));
            style::raised(&painter, frame.shrink(1.0), &p);
            // The group's title: the app's icon and name (and, for a
            // website, its address) and the close box, for all of them.
            let head = Rect::from_min_size(frame.min, vec2(WIDTH, HEADER));
            // Its middle on a device pixel, so the icon (a whole number
            // of pixels) and the text centre on the same line.
            let ppp = ui.ctx().pixels_per_point();
            let mid = (head.center().y * ppp).round() / ppp;
            let first = &self.cards[laid[0].i].note;
            let small = Rect::from_min_size(pos2(head.left() + PAD, mid - 8.0), vec2(16.0, 16.0));
            let mut names = app_icons(ui.ctx(), first.desktop_entry.as_deref(), &first.app_name);
            names.extend(named_icons(ui.ctx(), first.icon.as_deref(), first.desktop_entry.as_deref(), &first.app_name));
            paint_mark(&painter, small, &first.app_name, first.look.accent, first.via.is_some(), &names, "application");
            let name = if app.is_empty() { "Notification" } else { app.as_str() };
            let mut close_room = 22.0;
            // Pinned (all of them): it says so, by the close box.
            if laid.iter().all(|l| self.history.newest(self.cards[l.i].note.id).is_some_and(|e| e.pinned)) {
                let r = style::label(&painter, head.right() - PAD - close_room, mid, Align::Max, "Pinned", style::small_font(), style::disabled_text(&p));
                close_room += r.width() + 8.0;
            }
            let name_x = small.right() + 6.0;
            let n = style::label_fit(&painter, name_x, mid, Align::Min, name, egui::FontId::new(style::small_font().size, style::bold()), c(p.panel_text), head.right() - PAD - close_room - name_x);
            if let Some(src) = &first.look.source {
                let x = n.right() + 5.0;
                let room = head.right() - PAD - close_room - x;
                if room > 24.0 {
                    style::label_fit(&painter, x, mid, Align::Min, src, style::small_font(), style::disabled_text(&p), room);
                }
            }
            let single = laid.len() == 1;
            let all = Rect::from_center_size(pos2(head.right() - PAD - 6.0, mid), vec2(12.0, 12.0));
            let r = ui.interact(all.expand(3.0), ui.make_persistent_id(("close-all", &app)), Sense::click());
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if single { "Close".to_owned() } else { format!("Close All from {name}") }));
            close_box(&painter, all, if r.hovered() { c(p.panel_text) } else { style::disabled_text(&p) });
            if r.clicked() {
                // (Not those pinned: they stay put.)
                for l in &laid {
                    let id = self.cards[l.i].note.id;
                    if single || !self.history.newest(id).is_some_and(|e| e.pinned) {
                        did.push((id, Did::Dismiss));
                    }
                }
            }
            style::hline_above(&painter, frame.left() + 2.0, frame.right() - 2.0, head.bottom(), style::darken(p.panel_background, 1.25));
            for (n, Laid { i, rect, face, ink }) in laid.into_iter().enumerate() {
                if n > 0 {
                    style::hline(&painter, rect.left() + PAD, rect.right() - PAD, rect.top(), style::darken(p.panel_background, 1.18));
                    style::hline(&painter, rect.left() + PAD, rect.right() - PAD, rect.top() + 1.0, c(p.shine));
                }
                let card = &mut self.cards[i];
                let id = card.note.id;
                if hover.is_some_and(|h| rect.contains(h)) {
                    hovered = Some(id);
                }
                // The whole card is the default action; its menu pins or
                // snoozes it (kept while that is open).
                let click = ui.interact(rect, ui.make_persistent_id(("card", id)), Sense::click());
                if let Some(pinned) = self.history.newest(id).map(|e| e.pinned)
                    && let Some(command) = olo_kit::widgets::context_menu(&click, || note_menu(olo_kit::Menu::new("Notification"), Some(pinned)))
                {
                    did.push((id, Did::Command(command)));
                }
                if olo_kit::menu::is_open(ui.ctx(), click.id.with("context")) {
                    hovered = Some(id);
                }
                // One of several has its own close box too, while the
                // pointer is over it (the header's closes them all).
                let close = Rect::from_center_size(pos2(rect.right() - PAD - 6.0, rect.top() + PAD + 7.0), vec2(12.0, 12.0));
                let close_resp = (!single).then(|| ui.interact(close.expand(3.0), ui.make_persistent_id(("close", id)), Sense::click()));
                if let Some(r) = &close_resp {
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close"));
                    if hover.is_some_and(|h| rect.contains(h)) {
                        close_box(&painter, close, if r.hovered() { c(p.panel_text) } else { style::disabled_text(&p) });
                    }
                }
                let painted = look::paint(ui, &painter, &card_face(card), &face, &ink, rect.min + vec2(PAD, PAD), room.width);
                let mut link = None;
                if let (Some(pos), Some((origin, g))) = (hover, &painted.body) {
                    link = link_at(&card.note.body, g, *origin, pos);
                    if link.is_some() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
                let mut by = rect.bottom() - PAD - m::CONTROL_HEIGHT;
                let mut pressed = None;
                // Actions along the bottom, right to left as in an alert
                // (a browser's own Settings stays in the history).
                let buttons: Vec<(String, String)> =
                    card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty() && !web::browser_action(card.note.via.as_deref(), k)).cloned().collect();
                if !buttons.is_empty() {
                    let mut bx = rect.right() - PAD;
                    // In the narrow column they share the width when they must.
                    let n = buttons.len() as f32;
                    let share = ((WIDTH - 2.0 * PAD - (n - 1.0) * m::SPACING) / n).floor();
                    for (key, label) in buttons.iter().rev() {
                        let w = (style::label_width(ui.ctx(), label, style::plain_font()) + 24.0).max(m::BUTTON_MIN_WIDTH).min(share);
                        let r = Rect::from_min_size(pos2(bx - w, by), vec2(w, m::CONTROL_HEIGHT));
                        if ui.put(r, Button::new(label.as_str()).min_width(w)).clicked() {
                            pressed = Some(key.clone());
                        }
                        bx -= w + m::SPACING;
                    }
                    by -= m::CONTROL_HEIGHT + PAD;
                }
                // The reply field and its Send button.
                if let Some(placeholder) = card.note.reply.clone() {
                    let x = rect.left() + PAD;
                    let send_w = m::BUTTON_MIN_WIDTH;
                    let field = Rect::from_min_size(pos2(x, by), vec2(rect.right() - PAD - x - send_w - m::SPACING, m::CONTROL_HEIGHT));
                    let r = ui.put(field, TextControl::new(&mut card.reply).hint(&placeholder));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let send = Rect::from_min_size(pos2(rect.right() - PAD - send_w, by), vec2(send_w, m::CONTROL_HEIGHT));
                    let clicked = ui.add_enabled_ui(!card.reply.trim().is_empty(), |ui| ui.put(send, Button::new("Send")).clicked()).inner;
                    if (enter || clicked) && !card.reply.trim().is_empty() {
                        did.push((id, Did::Reply(card.reply.trim().to_owned())));
                    }
                }
                if close_resp.is_some_and(|r| r.clicked()) {
                    did.push((id, Did::Dismiss));
                } else if let Some(key) = pressed {
                    did.push((id, Did::Action(key)));
                } else if click.clicked() {
                    if let Some(url) = link {
                        did.push((id, Did::Link(url)));
                    } else if card.note.actions.iter().any(|(k, _)| k == "default") {
                        did.push((id, Did::Action("default".into())));
                    } else {
                        did.push((id, Did::Raise));
                    }
                }
            }
        }
        for (id, d) in did {
            match d {
                Did::Dismiss => self.remove(cx, id, Reason::Dismissed),
                Did::Action(key) => self.invoke(cx, self.win, id, key),
                Did::Raise => {
                    if let Some(card) = self.cards.iter().find(|c| c.note.id == id) {
                        raise_app(card.note.desktop_entry.as_deref(), &card.note.app_name);
                    }
                    self.remove(cx, id, Reason::Dismissed);
                }
                Did::Reply(text) => {
                    self.bus(|b| b.replied(id, &text));
                    self.remove(cx, id, Reason::Dismissed);
                }
                Did::Command(command) => self.note_command(cx, id, &command),
                Did::Link(url) => {
                    self.set_read(cx, id, true);
                    std::thread::spawn(move || {
                        if let Err(e) = olo_kit::open::open(&olo_kit::open::Target::parse(&url)) {
                            tracing::warn!("notifications: can't open {url}: {e}");
                        }
                    });
                }
            }
        }
        // Hovering keeps a card; it goes a moment after the pointer leaves.
        if let Some(id) = hovered
            && let Some(card) = self.cards.iter_mut().find(|c| c.note.id == id)
            && card.until.is_some_and(|u| u < Instant::now() + Duration::from_secs(2))
        {
            card.until = Some(Instant::now() + Duration::from_secs(2));
            self.tick_after(Duration::from_secs(2));
        }
        self.expire(cx, hovered);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<olo_kit::ActivationToken>() {
            Ok(t) => {
                if let Some(p) = self.pending.remove(&t.tag) {
                    self.send(p, Some(&t.token));
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::In(In::Show(note)) => self.show(cx, *note),
            Msg::In(In::Close(id)) => {
                self.remove(cx, id, Reason::Closed);
                if let Some(w) = self.history_win {
                    cx.redraw(w);
                }
            }
            Msg::In(In::Control(c, done)) => {
                self.control(cx, c);
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Msg::Tick => {
                self.flush_pending();
                self.wake_snoozed(cx);
                // Do Not Disturb may have begun or ended.
                self.refresh_settings();
                self.fade_out(cx);
                self.publish(false);
                // The history fades on.
                if let Some(w) = self.history_win {
                    cx.redraw(w);
                }
                // Let the frame decide (it knows whether the pointer is
                // over a card).
                self.redraw(cx);
                if self.win.is_none() {
                    self.expire(cx, None);
                }
            }
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if Some(window) == self.history_win {
            self.history_win = None;
            self.view.held.clear();
            self.view.selected = None;
            cx.close_window(window);
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<olo_kit::Menu>> {
        (Some(window) == self.history_win).then(|| {
            let view = self.settings.view;
            let mut views = olo_kit::Menu::new("View");
            for (v, label, command, key) in VIEWS {
                views = views.item(label, command, Some(key)).checked(view == v);
            }
            let selected = self.selected_unread();
            let pinned = self.view.selected.and_then(|id| self.history.newest(id)).map(|e| e.pinned);
            let edit = olo_kit::Menu::new("Edit")
                .item("Find", "find", Some(egui::Key::F))
                .separator()
                .item(if selected == Some(false) { "Mark as Unread" } else { "Mark as Read" }, "toggle-read", Some(egui::Key::U))
                .enabled(selected.is_some())
                .item("Mark All as Read", "read-all", None)
                .enabled(self.history.unread() > 0)
                .separator();
            let edit = note_menu(edit, pinned).separator().item("Clear All", "clear", None).enabled(self.history.entries.iter().any(|e| !e.shown && !e.pinned && !e.snoozed()));
            vec![olo_kit::Menu::new("File").item("Close", "close", Some(egui::Key::W)), edit, views]
        })
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        match command {
            "close" => self.close_requested(cx, window),
            "clear" => self.clear_history(cx),
            "find" => self.view.focus_search = true,
            "toggle-read" => {
                self.toggle_read(cx);
            }
            "read-all" => {
                self.view.held.extend(self.history.entries.iter().filter(|e| e.unread).map(|e| e.id));
                self.mark_all_read(cx);
            }
            // The selected note's.
            c if c == "pin" || c.starts_with("snooze:") => {
                if let Some(id) = self.view.selected {
                    self.note_command(cx, id, c);
                }
            }
            _ => {
                if let Some((v, ..)) = VIEWS.iter().find(|(_, _, c, _)| *c == command) {
                    self.set_view(*v);
                }
            }
        }
        cx.redraw(window);
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        // The History window comes back, with its search and folds.
        self.history_win.map(|_| {
            serde_json::json!({ "history": true, "search": self.view.search, "folded": self.view.folded, "selected": self.view.selected }).to_string()
        })
    }
}

/// Snoozing for so many minutes (None: until nine tomorrow morning,
/// `tomorrow_morning`).
const SNOOZES: [(Option<u32>, &str); 4] = [(Some(15), "15 Minutes"), (Some(60), "1 Hour"), (Some(180), "3 Hours"), (None, "Tomorrow Morning")];

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
fn note_menu(menu: olo_kit::Menu, pinned: Option<bool>) -> olo_kit::Menu {
    let mut snooze = olo_kit::Menu::new("Snooze");
    for (i, (_, label)) in SNOOZES.iter().enumerate() {
        snooze = snooze.item(*label, format!("snooze:{i}"), None).enabled(pinned.is_some());
    }
    menu.item(if pinned == Some(true) { "Unpin" } else { "Pin" }, "pin", None).enabled(pinned.is_some()).submenu(snooze)
}

/// The History window's views: View menu label, command and key.
const VIEWS: [(HistoryView, &str, &str, egui::Key); 4] = [
    (HistoryView::List, "List", "view-list", egui::Key::Num1),
    (HistoryView::Apps, "By App", "view-apps", egui::Key::Num2),
    (HistoryView::Cards, "Cards", "view-cards", egui::Key::Num3),
    (HistoryView::Conversations, "Conversations", "view-conversations", egui::Key::Num4),
];

/// A small ×.
fn close_box(painter: &egui::Painter, r: Rect, color: Color32) {
    let k = r.shrink(3.0);
    painter.line_segment([k.left_top(), k.right_bottom()], egui::Stroke::new(1.2, color));
    painter.line_segment([k.right_top(), k.left_bottom()], egui::Stroke::new(1.2, color));
}

fn main() -> std::process::ExitCode {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
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
    olo_kit::run(APP_ID, move |cx| {
        cx.stay_resident();
        let proxy = cx.proxy();
        let bus = Arc::new(Mutex::new(None));
        let snapshot = Arc::new(Mutex::new(String::new()));
        // Another notification service may have the name (a desktop
        // started before ours): wait for it to go rather than take it.
        let history = History::load(Some(History::path()));
        let first = history.entries.iter().map(|e| e.id).max().unwrap_or(0).wrapping_add(1);
        warm_app_icons(history.entries.iter().map(|e| (e.desktop_entry.clone(), e.app.clone())).collect(), || {});
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
        // A scheduled Do Not Disturb begins and ends on the minute; the
        // settings file changes when Preferences or Deskbar write it.
        let ticker = proxy.clone();
        std::thread::Builder::new()
            .name("notifications-clock".into())
            .spawn(move || {
                let modified = || std::fs::metadata(NotificationSettings::path()).and_then(|m| m.modified()).ok();
                let (mut seen, mut minute) = (modified(), now_secs() / 60);
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let (m, now) = (modified(), now_secs() / 60);
                    if m != seen || now != minute {
                        (seen, minute) = (m, now);
                        ticker.send(Msg::Tick);
                    }
                }
            })
            .expect("spawn");
        let mut app = Notifications {
            bus,
            win: None,
            cards: Vec::new(),
            fits: SHOWN,
            height: 0,
            keyboard: false,
            pending: HashMap::new(),
            next_tag: 0,
            proxy,
            settings: NotificationSettings::default(),
            settings_read: None,
            history,
            history_win: None,
            view: history::HistoryState::default(),
            snapshot,
            told: None,
        };
        app.refresh_settings();
        app.fade_out(cx);
        // Snoozed ones back on time (the clock's minutes catch them after
        // a suspend).
        app.wake_snoozed(cx);
        // Pinned banners on screen before a restart (an upgrade) are again.
        let pinned: Vec<Note> = app.history.entries.iter().filter(|e| e.shown).map(|e| e.to_note(Some(app.banner_time()))).collect();
        for note in pinned {
            app.add_card(cx, note);
        }
        let now = now_secs();
        for until in app.history.entries.iter().filter_map(|e| e.snoozed_until) {
            app.tick_after(Duration::from_secs(until.saturating_sub(now)));
        }
        app.publish(false);
        // After an upgrade: the History window as it was.
        if let Some(state) = cx.restored_state().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            && state["history"] == true
        {
            app.view.search = state["search"].as_str().unwrap_or_default().to_owned();
            app.view.folded = serde_json::from_value(state["folded"].clone()).unwrap_or_default();
            app.view.selected = state["selected"].as_u64().map(|id| id as u32);
            app.open_history(cx);
        }
        app
    });
    std::process::ExitCode::SUCCESS
}
