//! The widgets apps give Deskbar (`olo_protocol::glance`): each app's
//! provider told what's wanted of it (a file each app, written only as
//! that changes), started while it's wanted (again if it ends, once a
//! minute at most; afresh as Deskbar starts, so an upgrade's is the new
//! one), and its widgets' timelines read as their files change (an
//! inotify watch: nothing between). Each shows its entry for the time,
//! switched as the clock's minute comes; drawn by the kit
//! (`olo_kit::glance`), the same as anywhere.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use olo_kit::card::{Card, Part, Row};
use olo_protocol::{
    deskbar::{Measure, Widget},
    glance::{self as g, Body, Glance, Request, Requests, Timeline},
    notifications::InboxCard,
};
use serde::{Deserialize, Serialize};

/// The least time between two starts of one app's provider.
const RESTART: Duration = Duration::from_secs(60);
/// How long a single-card Dismiss/Snooze hide (or an undoing receipt waiting on
/// provider confirmation) waits with no word from its app.
pub const GONE: Duration = Duration::from_secs(3);
/// How long a post-clear receipt stays on top of an inbox stack before
/// collapsing (once the pointer is off the widget).
pub const RECEIPT: Duration = Duration::from_secs(8);
/// How long a failure/no-op receipt notice ("Nothing was cleared" / "Nothing
/// to undo") stays before collapsing if not clicked away sooner.
pub const NOTICE: Duration = Duration::from_secs(3);

/// Whether `w` is Notifications' Inbox widget.
pub fn is_inbox_widget(w: &Widget) -> bool {
    matches!(&w.measure, Measure::App { app, glance, .. } if app == "notifications" && glance == "inbox")
}

/// Identifies an inbox card without colliding between waiting cards (red,
/// `true`) and notifications (blue, `false`) that happen to share an id.
type CardKey = (bool, u32);

/// When a locally hidden inbox card stops being hidden if the provider
/// never reports it gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoneAt {
    /// Waiting on a bounded `ClearMany` reply for `generation`.
    InFlight(u64),
    /// Confirmed cleared by the service: stays hidden until the provider's
    /// timeline no longer contains this card (or `UndoClear` restores it),
    /// never reappearing on a 3-second timeout (Astra 10).
    Confirmed,
    /// Single-card Dismiss or Snooze optimistic hide (expires after `GONE`).
    Until(Instant),
}

/// Tracks an in-flight `ClearMany` independently of the visible receipt so an
/// Undo clicked before `ClearMany` replies is never lost if a newcomer, expiry,
/// replacement, or widget removal closes the visible receipt first (Astra 4).
#[derive(Debug, Clone)]
struct PendingClear {
    sent: HashSet<u32>,
    undo_requested: bool,
    was_selected: Option<(CardKey, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReceiptKind {
    Cleared {
        count: usize,
        /// Unique D-Bus sender (`:1.xx`) of the service instance that answered
        /// `ClearMany`; `None` while the call is in flight.
        owner: Option<String>,
        /// Globally unique batch id returned by `ClearMany`.
        batch: Option<String>,
        /// Person clicked Undo; receipt stays quiet until the provider
        /// republishes the authoritative restored cards (no local stack).
        undoing: bool,
    },
    NothingCleared,
    NothingToUndo,
}

/// Post-clear receipt shown on top of an inbox stack.
#[derive(Debug, Clone)]
struct Receipt {
    generation: u64,
    kind: ReceiptKind,
    expires_at: Option<Instant>,
    /// The receipt's deadline elapsed while the pointer was on the widget.
    due: bool,
    /// Selected card before Clear All (restored on Undo or failure).
    was_selected: Option<(CardKey, usize)>,
}

/// Acknowledged receipt state kept across an in-place Deskbar reload (Astra 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedReceipt {
    pub widget: u32,
    pub count: usize,
    pub owner: String,
    pub batch: String,
    pub remaining_ms: u64,
}

/// Information for Deskbar's receipt menu and click handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptInfo {
    pub generation: u64,
    pub can_undo: bool,
    pub dismissible_notice: bool,
}

/// Request to send `UndoClear(batch)` to the exact `owner` bus connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRequest {
    pub widget: u32,
    pub generation: u64,
    pub owner: String,
    pub batch: String,
}

/// What Deskbar should do after `Glances::finish_clear_all`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearOutcome {
    /// Visible cards or receipt changed: redraw both hits and widget layer.
    pub redraw: bool,
    /// Person clicked Undo while `ClearMany` was still in flight: enqueue
    /// `UndoClear` now on the serialized worker.
    pub undo_now: Option<UndoRequest>,
}

/// Zero-copy summary of an Inbox widget for one Deskbar build pass (Astra 24).
pub struct InboxSnapshot<'a> {
    pub top: Option<&'a InboxCard>,
    pub count: usize,
    pub snooze_minutes: u32,
    pub preview: bool,
    pub clear_plan: Option<(Vec<(u32, u64)>, String)>,
}

#[derive(Default)]
pub struct Glances {
    dir: PathBuf,
    /// What each app's provider was last asked.
    asked: HashMap<&'static str, Requests>,
    /// Each app's provider while it runs, and when it was last started.
    running: HashSet<&'static str>,
    started: HashMap<&'static str, Instant>,
    /// Last known `Measure` for each widget id so in-place kind changes drop
    /// stale timelines and Inbox state immediately (Astra 21).
    measures: HashMap<u32, Measure>,
    /// Each widget's timeline, and the entry it showed last (its time).
    timelines: HashMap<u32, Timeline>,
    showing: HashMap<u32, i64>,
    /// An inbox's card on top, by its `(attention, id)` key (None: the first),
    /// and the part of an inbox the pointer is on / pressing.
    selected: HashMap<u32, (CardKey, usize)>,
    hot: Option<(u32, Option<olo_kit::glance::InboxPart>)>,
    pressed: Option<(u32, olo_kit::glance::InboxPart)>,
    /// Inbox cards dismissed, snoozed or cleared here that their app hasn't
    /// taken away yet, keyed by `(attention, id)` for O(1) lookup and no
    /// red/blue ID collision.
    gone: HashMap<u32, HashMap<CardKey, GoneAt>>,
    /// The cards each inbox had when last read: one that comes after goes
    /// on top (Ben, 2026-11-17: "if newer ones arrive they should be
    /// added on top of the card stack too, not the bottom").
    had: HashMap<u32, Vec<CardKey>>,
    /// Post-clear receipt on top of each inbox stack (no local copy of cleared
    /// cards: the notifications service is the single authority).
    receipts: HashMap<u32, Receipt>,
    /// In-flight `ClearMany` operations keyed by `generation`, independent of
    /// visible receipts (Astra 4).
    pending_clears: HashMap<u64, PendingClear>,
    next_gen: u64,
    watch: Option<olo_kit::watch::Watch>,
}

/// Now, in seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Glances {
    pub fn new() -> Glances {
        Glances { dir: g::dir(), ..Glances::default() }
    }

    /// Snapshot active acknowledged receipts for an in-place Deskbar reload.
    pub fn save_receipts(&self) -> Vec<SavedReceipt> {
        let now = Instant::now();
        self.receipts
            .iter()
            .filter_map(|(&widget, r)| {
                let ReceiptKind::Cleared { count, owner: Some(owner), batch: Some(batch), undoing: false } = &r.kind else {
                    return None;
                };
                let remaining_ms = r.expires_at.map_or(0, |at| at.saturating_duration_since(now).as_millis() as u64);
                (remaining_ms > 0).then(|| SavedReceipt { widget, count: *count, owner: owner.clone(), batch: batch.clone(), remaining_ms })
            })
            .collect()
    }

    /// Restore acknowledged receipts from an in-place Deskbar reload.
    pub fn restore_receipts(&mut self, saved: Vec<SavedReceipt>) {
        let now = Instant::now();
        let cap_ms = RECEIPT.as_millis() as u64;
        for s in saved {
            if s.remaining_ms == 0 || s.count == 0 || s.owner.is_empty() || s.batch.is_empty() {
                continue;
            }
            self.next_gen = self.next_gen.wrapping_add(1).max(1);
            let generation = self.next_gen;
            let rem = Duration::from_millis(s.remaining_ms.min(cap_ms));
            self.receipts.insert(
                s.widget,
                Receipt {
                    generation,
                    kind: ReceiptKind::Cleared { count: s.count, owner: Some(s.owner), batch: Some(s.batch), undoing: false },
                    expires_at: Some(now + rem),
                    due: false,
                    was_selected: None,
                },
            );
        }
    }

    /// The widgets as the settings now say: each app asked for its own,
    /// its provider started if it isn't running; none, the folder
    /// unwatched and the providers told so (they end). Drops Inbox state
    /// whenever a widget is removed or changes kind in place (Astra 21).
    pub fn set(&mut self, widgets: &[Widget], proxy: &olo_kit::Proxy) {
        let inbox_ids: HashSet<u32> = widgets.iter().filter(|w| is_inbox_widget(w)).map(|w| w.id).collect();
        self.selected.retain(|id, _| inbox_ids.contains(id));
        self.gone.retain(|id, _| inbox_ids.contains(id));
        self.had.retain(|id, _| inbox_ids.contains(id));
        self.receipts.retain(|id, _| inbox_ids.contains(id));
        if self.hot.is_some_and(|(id, _)| !inbox_ids.contains(&id)) {
            self.hot = None;
        }
        if self.pressed.is_some_and(|(id, _)| !inbox_ids.contains(&id)) {
            self.pressed = None;
        }
        // Drop cached timeline whenever a widget is removed or changes its Measure.
        let prev_measures = std::mem::take(&mut self.measures);
        for w in widgets {
            self.measures.insert(w.id, w.measure.clone());
        }
        self.timelines.retain(|id, _| self.measures.get(id).is_some_and(|m| prev_measures.get(id) == Some(m)));
        self.showing.retain(|id, _| self.timelines.contains_key(id));

        let mut wanted: HashMap<&'static str, Requests> = HashMap::new();
        for w in widgets {
            if let Measure::App { app, glance, options } = &w.measure
                && let Some(p) = g::provider(app)
            {
                wanted.entry(p.app).or_default().widgets.push(Request { widget: w.id, kind: glance.clone(), options: options.clone(), size: w.size });
            }
        }
        if !wanted.is_empty() && std::fs::create_dir_all(&self.dir).is_err() {
            tracing::warn!("glances: no folder {}", self.dir.display());
            return;
        }
        // Apps no longer wanted: asked for nothing.
        for p in g::PROVIDERS {
            let now = wanted.remove(p.app).unwrap_or_default();
            if self.asked.get(p.app) == Some(&now) || (now.widgets.is_empty() && !self.asked.contains_key(p.app)) {
                continue;
            }
            if let Err(e) = g::write_json(&g::requests_path(&self.dir, p.app), &now) {
                tracing::warn!("glances: {}: {e}", p.app);
            }
            if now.widgets.is_empty() {
                self.asked.remove(p.app);
            } else {
                self.asked.insert(p.app, now);
                self.start(p.app, proxy);
            }
        }
        if self.asked.is_empty() {
            self.watch = None;
        } else if self.watch.is_none() {
            let proxy = proxy.clone();
            self.watch = olo_kit::watch::watch([self.dir.clone()], move |_| proxy.send(crate::Msg::Glances));
        }
        self.reread();
    }

    /// App `app`'s provider started (not if it runs; not twice in a
    /// minute), watched until it ends: on a worker, as one from before
    /// (a Deskbar before this one: an upgrade's old program) is ended
    /// first, so this one's is the program now installed.
    fn start(&mut self, app: &'static str, proxy: &olo_kit::Proxy) {
        if self.running.contains(app) || self.started.get(app).is_some_and(|t| t.elapsed() < RESTART) {
            return;
        }
        let Some(p) = g::provider(app) else { return };
        self.running.insert(app);
        self.started.insert(app, Instant::now());
        let (dir, proxy) = (self.dir.clone(), proxy.clone());
        let started = std::thread::Builder::new().name("deskbar-glance".into()).spawn(move || {
            if let Some(pid) = std::fs::read_to_string(g::lock_path(&dir, app)).ok().and_then(|s| s.trim().parse::<i32>().ok())
                && is_provider(pid, p.program)
            {
                // SAFETY: a signal to the process the lock names, checked
                // to be that provider.
                unsafe { libc::kill(pid, libc::SIGTERM) };
                for _ in 0..200 {
                    // (One of this process's own, from before it re-ran
                    // itself, reaped; another's, waited out.)
                    // SAFETY: reaping a child, if it is one; nothing otherwise.
                    unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
                    if !is_provider(pid, p.program) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let mut cmd = std::process::Command::new(p.program);
            cmd.arg(p.arg).env(g::DIR_ENV, &dir).stdin(std::process::Stdio::null());
            let deskbar = std::process::id();
            // SAFETY: prctl and getppid are async-signal-safe; nothing
            // else runs between fork and exec.
            unsafe {
                use std::os::unix::process::CommandExt as _;
                cmd.pre_exec(move || {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0);
                    if libc::getppid() as u32 != deskbar {
                        return Err(std::io::Error::other("Deskbar has gone"));
                    }
                    Ok(())
                });
            }
            match cmd.spawn() {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(e) => tracing::warn!("glances: {}: {e}", p.program),
            }
            proxy.send(crate::Msg::GlanceEnded(app));
        });
        if let Err(e) = started {
            tracing::warn!("glances: {e}");
            self.running.remove(app);
        }
    }

    /// App `app`'s provider ended: started again if it's still wanted (a
    /// minute after it last started, at the soonest).
    pub fn ended(&mut self, app: &'static str, proxy: &olo_kit::Proxy) {
        self.running.remove(app);
        if !self.asked.contains_key(app) {
            return;
        }
        let wait = self.started.get(app).map_or(Duration::ZERO, |t| RESTART.saturating_sub(t.elapsed()));
        if wait.is_zero() {
            self.start(app, proxy);
        } else {
            let proxy = proxy.clone();
            let _ = std::thread::Builder::new().name("deskbar-glance".into()).spawn(move || {
                std::thread::sleep(wait);
                proxy.send(crate::Msg::GlanceEnded(app));
            });
        }
    }

    /// Every asked-for widget's timeline read again: whether any changed.
    pub fn reread(&mut self) -> bool {
        let mut changed = false;
        let ids: Vec<u32> = self.asked.values().flat_map(|r| r.widgets.iter().map(|w| w.widget)).collect();
        for id in ids {
            let read: Option<Timeline> = std::fs::read(g::content_path(&self.dir, id)).ok().and_then(|b| serde_json::from_slice(&b).ok());
            if let Some(t) = read
                && self.timelines.get(&id).map(|had| &had.entries) != Some(&t.entries)
            {
                self.timelines.insert(id, t);
                changed = true;
            }
        }
        if changed {
            self.turned();
            self.reconcile_inboxes();
        }
        changed
    }

    /// Reconcile inbox selection, receipts and pending hides against the
    /// authoritative timelines now loaded.
    fn reconcile_inboxes(&mut self) {
        for (id, t) in &self.timelines {
            let Some(Body::Inbox { cards, .. }) = t.at(now()).and_then(|gl| gl.body.as_ref()) else { continue };
            let now_keys: Vec<CardKey> = cards.iter().map(|c| (c.attention, c.id)).collect();
            let undoing = matches!(self.receipts.get(id).map(|r| &r.kind), Some(ReceiptKind::Cleared { undoing: true, .. }));
            if let Some(had) = self.had.get(id)
                && let Some((at, new)) = cards.iter().enumerate().filter(|(_, c)| !had.contains(&(c.attention, c.id))).max_by_key(|(_, c)| (c.time, c.id))
            {
                if undoing {
                    // Provider republished the restored cards in authoritative
                    // order: close the quiet receipt and restore prior selection
                    // if it still exists in the list.
                    if let Some(r) = self.receipts.remove(id) {
                        match r.was_selected {
                            Some(sel) => {
                                self.selected.insert(*id, sel);
                            }
                            None => {
                                self.selected.remove(id);
                            }
                        }
                    }
                } else {
                    self.selected.insert(*id, ((new.attention, new.id), at));
                    self.receipts.remove(id);
                }
            }
            self.had.insert(*id, now_keys);
        }
        // Drop pending/confirmed hides once the provider's timeline no longer contains them.
        let timelines = &self.timelines;
        self.gone.retain(|id, items| {
            let there = |key: CardKey| {
                timelines.get(id).is_some_and(|t| t.entries.iter().any(|e| matches!(&e.glance.body, Some(Body::Inbox { cards, .. }) if cards.iter().any(|c| (c.attention, c.id) == key))))
            };
            items.retain(|&key, _| there(key));
            !items.is_empty()
        });
    }

    /// Whether a widget's entry for the time is another than it showed
    /// (the clock's minute came).
    pub fn turned(&mut self) -> bool {
        let t = now();
        let mut any = false;
        for (id, timeline) in &self.timelines {
            let from = timeline.entries.iter().rev().find(|e| e.from <= t).map_or(i64::MIN, |e| e.from);
            if self.showing.insert(*id, from) != Some(from) {
                any = true;
            }
        }
        any
    }

    /// What widget `id` shows now.
    pub fn now(&self, id: u32) -> Option<&Glance> {
        self.timelines.get(&id).and_then(|t| t.at(now()))
    }

    fn is_hidden(&self, id: u32, key: CardKey, at_now: Instant) -> bool {
        self.gone.get(&id).and_then(|m| m.get(&key)).is_some_and(|g| match g {
            GoneAt::InFlight(_) | GoneAt::Confirmed => true,
            GoneAt::Until(until) => at_now < *until,
        })
    }

    /// Zero-copy summary of widget `id`'s inbox: its top visible card, visible
    /// count, snooze minutes, preview flag, and clear plan — computed in one
    /// pass without cloning any `InboxCard` (Astra 24).
    pub fn inbox_snapshot(&self, id: u32) -> Option<InboxSnapshot<'_>> {
        let Body::Inbox { cards, snooze_minutes, preview } = self.now(id)?.body.as_ref()? else { return None };
        let at_now = Instant::now();
        let sel = self.selected.get(&id).copied();
        let mut count = 0usize;
        let mut found_selected: Option<&InboxCard> = None;
        let mut fallback_at_was: Option<&InboxCard> = None;
        let mut first: Option<&InboxCard> = None;
        let mut all_pin_known = true;
        let mut clearable: Vec<(u32, u64)> = Vec::new();

        for c in cards {
            let key = (c.attention, c.id);
            if self.is_hidden(id, key, at_now) {
                continue;
            }
            if count == 0 {
                first = Some(c);
            }
            if let Some((sel_key, was)) = sel {
                if key == sel_key {
                    found_selected = Some(c);
                }
                if count == was {
                    fallback_at_was = Some(c);
                }
            }
            if !c.attention {
                if c.instance == 0 {
                    all_pin_known = false;
                } else if !c.pinned {
                    clearable.push((c.id, c.instance));
                }
            }
            count += 1;
        }

        let top = if count == 0 {
            None
        } else if let Some(c) = found_selected {
            Some(c)
        } else if let Some((_, was)) = sel {
            if was >= count {
                // Clamp to last visible card.
                cards.iter().rev().find(|c| !self.is_hidden(id, (c.attention, c.id), at_now))
            } else {
                fallback_at_was.or(first)
            }
        } else {
            first
        };

        let clear_plan = if all_pin_known && clearable.len() >= 2 {
            let some_stay = clearable.len() < count;
            let label = olo_kit::glance::inbox_clear_all_label(clearable.len(), some_stay);
            Some((clearable, label))
        } else {
            None
        };

        Some(InboxSnapshot { top, count, snooze_minutes: *snooze_minutes, preview: *preview, clear_plan })
    }

    /// Zero-copy selected index in widget `id`'s visible inbox cards.
    fn inbox_selected_index(&self, id: u32) -> usize {
        let Some(Body::Inbox { cards, .. }) = self.now(id).and_then(|gl| gl.body.as_ref()) else { return 0 };
        let Some((sel_key, was)) = self.selected.get(&id).copied() else { return 0 };
        let at_now = Instant::now();
        let mut count = 0usize;
        for c in cards {
            let key = (c.attention, c.id);
            if self.is_hidden(id, key, at_now) {
                continue;
            }
            if key == sel_key {
                return count;
            }
            count += 1;
        }
        was.min(count.saturating_sub(1))
    }

    /// Widget `id`'s inbox, if it shows one: its visible cards, which is on
    /// top, how long Snooze snoozes, and whether cards show their text.
    /// Used only when a filtered owned list is needed (painting with pending
    /// hides, turning cards, card tip, tests).
    pub fn inbox(&self, id: u32) -> Option<(Vec<InboxCard>, usize, u32, bool)> {
        let Body::Inbox { cards, snooze_minutes, preview } = self.now(id)?.body.as_ref()? else { return None };
        let at_now = Instant::now();
        let cards: Vec<_> = cards.iter().filter(|c| !self.is_hidden(id, (c.attention, c.id), at_now)).cloned().collect();
        let at = match self.selected.get(&id) {
            Some((key, was)) => cards.iter().position(|c| (c.attention, c.id) == *key).unwrap_or((*was).min(cards.len().saturating_sub(1))),
            None => 0,
        };
        Some((cards, at, *snooze_minutes, *preview))
    }

    /// What widget `id` shows now, as it's drawn: an inbox less the cards
    /// dealt with here (borrowed zero-copy whenever nothing is hidden).
    pub fn shown(&self, id: u32) -> Option<std::borrow::Cow<'_, Glance>> {
        let gl = self.now(id)?;
        if !self.gone.contains_key(&id) {
            return Some(std::borrow::Cow::Borrowed(gl));
        }
        let (cards, _, snooze_minutes, preview) = self.inbox(id)?;
        Some(std::borrow::Cow::Owned(Glance { body: Some(Body::Inbox { cards, snooze_minutes, preview }), ..gl.clone() }))
    }

    /// Whether the dot of a card on top of an inbox is still draining
    /// (zero-copy: inspects the top visible card by reference).
    pub fn fading(&self) -> bool {
        let secs = now().max(0) as u64;
        self.timelines.keys().any(|id| !self.receipts.contains_key(id) && self.inbox_snapshot(*id).and_then(|s| s.top).is_some_and(|c| c.draining(secs)))
    }

    /// Card `(attention, item)` of widget `id` was dismissed or snoozed
    /// here: gone at once, the card after it in its place.
    pub fn dealt_with(&mut self, id: u32, attention: bool, item: u32) {
        if let Some((cards, at, ..)) = self.inbox(id) {
            match cards.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| cards.get(i))) {
                Some(next) => self.selected.insert(id, ((next.attention, next.id), at.min(cards.len().saturating_sub(2)))),
                None => self.selected.remove(&id),
            };
        }
        self.gone.entry(id).or_default().insert((attention, item), GoneAt::Until(Instant::now() + GONE));
    }

    /// Begin clearing the captured notification targets `(id, instance)` of
    /// widget `id`: intersects `captured` (in O(N) via a `HashSet`, Astra 24)
    /// with currently visible unpinned notifications (only when the producer is
    /// pin-aware, Astra 17), hides them at once, preserves any surviving
    /// selected card, and puts an 8-second receipt on top of the stack.
    pub fn begin_clear_all(&mut self, id: u32, captured: &[(u32, u64)]) -> Option<(u64, Vec<(u32, u64)>)> {
        let Body::Inbox { cards, .. } = self.now(id)?.body.as_ref()? else { return None };
        let at_now = Instant::now();
        // Gate on pin-aware producer: refuse if any visible notification card lacks pin metadata.
        if cards.iter().any(|c| !self.is_hidden(id, (c.attention, c.id), at_now) && !c.attention && c.instance == 0) {
            return None;
        }
        let captured_set: HashSet<(u32, u64)> = captured.iter().copied().collect();
        if captured_set.is_empty() {
            return None;
        }
        let mut targets: Vec<(u32, u64)> = Vec::new();
        let mut target_ids: HashSet<u32> = HashSet::new();
        for c in cards {
            if self.is_hidden(id, (c.attention, c.id), at_now) {
                continue;
            }
            if !c.attention && !c.pinned && captured_set.contains(&(c.id, c.instance)) && target_ids.insert(c.id) {
                targets.push((c.id, c.instance));
            }
        }
        if targets.is_empty() {
            return None;
        }
        let was_selected = self.selected.get(&id).copied();
        if let Some((key @ (att, item), _)) = was_selected {
            if att || !target_ids.contains(&item) {
                let remaining_pos =
                    cards.iter().filter(|c| !self.is_hidden(id, (c.attention, c.id), at_now) && (c.attention || !target_ids.contains(&c.id))).position(|c| (c.attention, c.id) == key).unwrap_or(0);
                self.selected.insert(id, (key, remaining_pos));
            } else {
                self.selected.remove(&id);
            }
        }
        self.next_gen = self.next_gen.wrapping_add(1).max(1);
        let generation = self.next_gen;
        let gone = self.gone.entry(id).or_default();
        for &item in &target_ids {
            gone.insert((false, item), GoneAt::InFlight(generation));
        }
        self.pending_clears.insert(generation, PendingClear { sent: target_ids, undo_requested: false, was_selected });
        self.receipts.insert(
            id,
            Receipt { generation, kind: ReceiptKind::Cleared { count: targets.len(), owner: None, batch: None, undoing: false }, expires_at: Some(at_now + RECEIPT), due: false, was_selected },
        );
        self.pressed = None;
        Some((generation, targets))
    }

    /// Reconcile the `ClearMany` worker's reply for `(id, generation)`:
    /// validates reply IDs against the sent set (Astra 16), transitions
    /// confirmed clears to `GoneAt::Confirmed` (Astra 10), honors any queued
    /// Undo intent even if the visible receipt already closed (Astra 4), and on
    /// failure/empty restores every hidden card and shows `"Nothing was
    /// cleared"` (Item 3 / Astra 9).
    pub fn finish_clear_all(&mut self, id: u32, generation: u64, result: Result<(Vec<u32>, String, String), ()>) -> ClearOutcome {
        let pending = self.pending_clears.remove(&generation);
        let sent = pending.as_ref().map(|p| &p.sent);
        let validated = match result {
            Ok((cleared, owner, batch)) if !batch.is_empty() && !owner.is_empty() => {
                // Validate reply against exact target IDs sent (Astra 16).
                let mut valid = Vec::new();
                let mut seen = HashSet::new();
                for item in cleared {
                    if sent.is_none_or(|s| s.contains(&item)) && seen.insert(item) {
                        valid.push(item);
                    }
                }
                if valid.is_empty() { None } else { Some((valid, seen, owner, batch)) }
            }
            _ => None,
        };

        if let Some((cleared, cleared_set, owner, batch)) = validated {
            let mut redraw = false;
            if let Some(map) = self.gone.get_mut(&id) {
                let mut remove = Vec::new();
                for (&(att, item), state) in map.iter_mut() {
                    if *state == GoneAt::InFlight(generation) {
                        if !att && cleared_set.contains(&item) {
                            *state = GoneAt::Confirmed;
                        } else {
                            remove.push((att, item));
                        }
                    }
                }
                if !remove.is_empty() {
                    redraw = true;
                    for key in remove {
                        map.remove(&key);
                    }
                }
                if map.is_empty() {
                    self.gone.remove(&id);
                }
            }
            // If the person already clicked Undo while `ClearMany` was in
            // flight, send `UndoClear` now — even if a newcomer, expiry or
            // widget removal closed the visible receipt in the meantime (Astra 4).
            if pending.as_ref().is_some_and(|p| p.undo_requested) {
                if let Some(r) = self.receipts.get_mut(&id).filter(|r| r.generation == generation) {
                    r.kind = ReceiptKind::Cleared { count: cleared.len(), owner: Some(owner.clone()), batch: Some(batch.clone()), undoing: true };
                    r.expires_at = Some(Instant::now() + GONE);
                    r.due = false;
                    redraw = true;
                }
                return ClearOutcome { redraw, undo_now: Some(UndoRequest { widget: id, generation, owner, batch }) };
            }
            if let Some(r) = self.receipts.get_mut(&id).filter(|r| r.generation == generation) {
                let changed_count = match &r.kind {
                    ReceiptKind::Cleared { count, .. } => *count != cleared.len(),
                    _ => true,
                };
                if changed_count {
                    redraw = true;
                }
                r.kind = ReceiptKind::Cleared { count: cleared.len(), owner: Some(owner), batch: Some(batch), undoing: false };
            }
            ClearOutcome { redraw, undo_now: None }
        } else {
            // Failure, timeout, spawn failure, older service, or 0 cleared:
            // bring all in-flight cards back at once and show "Nothing was cleared".
            if let Some(map) = self.gone.get_mut(&id) {
                map.retain(|_, state| *state != GoneAt::InFlight(generation));
                if map.is_empty() {
                    self.gone.remove(&id);
                }
            }
            let was_selected = pending.and_then(|p| p.was_selected).or_else(|| self.receipts.get(&id).filter(|r| r.generation == generation).and_then(|r| r.was_selected));
            if let Some(r) = self.receipts.get_mut(&id).filter(|r| r.generation == generation) {
                r.kind = ReceiptKind::NothingCleared;
                r.expires_at = Some(Instant::now() + NOTICE);
                r.due = false;
            }
            match was_selected {
                Some(sel) => {
                    self.selected.insert(id, sel);
                }
                None => {
                    self.selected.remove(&id);
                }
            }
            ClearOutcome { redraw: true, undo_now: None }
        }
    }

    /// Click on Undo for widget `id`'s receipt `generation`: returns `None` if
    /// no matching interactive receipt, `Some(None)` if `ClearMany` is still in
    /// flight (Undo intent recorded on `pending_clears` so it survives receipt
    /// closure, Astra 4), or `Some(Some(UndoRequest))` to enqueue `UndoClear`
    /// on the serialized worker while keeping the quiet receipt until the
    /// provider republishes (Astra 18).
    pub fn begin_undo(&mut self, id: u32, generation: u64) -> Option<Option<UndoRequest>> {
        let r = self.receipts.get_mut(&id)?;
        if r.generation != generation {
            return None;
        }
        let ReceiptKind::Cleared { owner, batch, undoing, .. } = &mut r.kind else {
            return None;
        };
        if *undoing {
            return None;
        }
        *undoing = true;
        self.pressed = None;
        match (owner.clone(), batch.clone()) {
            (Some(owner), Some(batch)) => {
                r.expires_at = Some(Instant::now() + GONE);
                r.due = false;
                Some(Some(UndoRequest { widget: id, generation, owner, batch }))
            }
            _ => {
                if let Some(p) = self.pending_clears.get_mut(&generation) {
                    p.undo_requested = true;
                }
                Some(None)
            }
        }
    }

    /// Reconcile `UndoClear`'s reply from the service (the single authority):
    /// removes restored IDs from `self.gone`, closes the quiet receipt right
    /// away if the provider's timeline still holds those cards (otherwise keeps
    /// the quiet receipt until `reconcile_inboxes` sees them republished), and
    /// shows `"Nothing to undo"` on failure or empty restore (Astra 11, 18).
    pub fn finish_undo(&mut self, id: u32, generation: u64, result: Result<Vec<u32>, ()>) -> bool {
        match result {
            Ok(restored) if !restored.is_empty() => {
                if let Some(map) = self.gone.get_mut(&id) {
                    for item in &restored {
                        map.remove(&(false, *item));
                    }
                    if map.is_empty() {
                        self.gone.remove(&id);
                    }
                }
                // If the provider's current timeline already contains any of
                // the restored cards (provider hadn't updated yet), reveal them
                // immediately in their authoritative order and end the receipt.
                let already_present = self.now(id).is_some_and(|gl| matches!(&gl.body, Some(Body::Inbox { cards, .. }) if cards.iter().any(|c| !c.attention && restored.contains(&c.id))));
                if already_present {
                    if let Some(r) = self.receipts.get(&id).filter(|r| r.generation == generation).cloned() {
                        self.receipts.remove(&id);
                        match r.was_selected {
                            Some(sel) => {
                                self.selected.insert(id, sel);
                            }
                            None => {
                                self.selected.remove(&id);
                            }
                        }
                    }
                } else if let Some(r) = self.receipts.get_mut(&id).filter(|r| r.generation == generation) {
                    r.expires_at = Some(Instant::now() + GONE);
                    r.due = false;
                }
                true
            }
            _ => {
                if let Some(r) = self.receipts.get_mut(&id).filter(|r| r.generation == generation) {
                    r.kind = ReceiptKind::NothingToUndo;
                    r.expires_at = Some(Instant::now() + NOTICE);
                    r.due = false;
                    return true;
                }
                false
            }
        }
    }

    /// Clicking a non-Undo failure/no-op notice receipt dismisses it right away
    /// so the restored cards underneath are immediately clickable.
    pub fn dismiss_notice_receipt(&mut self, id: u32, generation: u64) -> bool {
        let dismissible = self.receipts.get(&id).is_some_and(|r| r.generation == generation && matches!(r.kind, ReceiptKind::NothingCleared | ReceiptKind::NothingToUndo));
        if dismissible {
            self.receipts.remove(&id);
            true
        } else {
            false
        }
    }

    /// Soonest live inbox timer deadline across all active receipts and
    /// single-card optimistic hides — drives Deskbar's single cancellable
    /// looper timer (Astra 12).
    pub fn next_deadline(&self) -> Option<Instant> {
        let receipts = self.receipts.values().filter(|r| !r.due).filter_map(|r| r.expires_at);
        let hides = self.gone.values().flat_map(|m| m.values()).filter_map(|g| match g {
            GoneAt::Until(until) => Some(*until),
            GoneAt::InFlight(_) | GoneAt::Confirmed => None,
        });
        receipts.chain(hides).min()
    }

    /// Fire all inbox deadlines due by `at_now`: collapses due receipts when
    /// the pointer is off the widget (holding interactive receipts while
    /// hovered), and prunes expired single-card `Until` hides (never pruning
    /// `Confirmed` clears, Astra 10). Returns whether visible state changed.
    pub fn tick_timers(&mut self, at_now: Instant) -> bool {
        let mut changed = false;
        let hot_widget = self.hot.map(|(w, _)| w);
        let mut remove = Vec::new();
        for (&widget, r) in &mut self.receipts {
            if !r.due && r.expires_at.is_some_and(|at| at <= at_now) {
                let interactive = matches!(r.kind, ReceiptKind::Cleared { undoing: false, .. });
                if interactive && hot_widget == Some(widget) {
                    r.due = true;
                    r.expires_at = None;
                } else {
                    remove.push(widget);
                }
            }
        }
        for widget in remove {
            self.receipts.remove(&widget);
            changed = true;
        }
        self.gone.retain(|_, map| {
            let before = map.len();
            map.retain(|_, g| match g {
                GoneAt::InFlight(_) | GoneAt::Confirmed => true,
                GoneAt::Until(until) => at_now < *until,
            });
            if map.len() != before {
                changed = true;
            }
            !map.is_empty()
        });
        changed
    }

    /// Dismiss any receipt whose 8-second timer already elapsed if the
    /// pointer is no longer on its widget. Returns whether any closed.
    pub fn dismiss_due_receipts(&mut self) -> bool {
        let hot_widget = self.hot.map(|(w, _)| w);
        let before = self.receipts.len();
        self.receipts.retain(|&w, r| !r.due || hot_widget == Some(w));
        self.receipts.len() != before
    }

    /// Whether any inbox has a receipt whose 8-second timer has elapsed.
    pub fn has_due_receipt(&self) -> bool {
        self.receipts.values().any(|r| r.due)
    }

    /// Active post-clear receipt metadata for widget `id`, if any.
    pub fn inbox_receipt(&self, id: u32) -> Option<ReceiptInfo> {
        self.receipts.get(&id).map(|r| match &r.kind {
            ReceiptKind::Cleared { undoing, .. } => ReceiptInfo { generation: r.generation, can_undo: !undoing, dismissible_notice: false },
            ReceiptKind::NothingCleared | ReceiptKind::NothingToUndo => ReceiptInfo { generation: r.generation, can_undo: false, dismissible_notice: true },
        })
    }

    /// The next card on top (`previous`: the one before), round and
    /// round. Whether it changed.
    pub fn turn_card(&mut self, id: u32, previous: bool) -> bool {
        let Some((cards, at, ..)) = self.inbox(id).filter(|(cards, ..)| cards.len() > 1) else { return false };
        let at = (at + if previous { cards.len() - 1 } else { 1 }) % cards.len();
        self.selected.insert(id, ((cards[at].attention, cards[at].id), at));
        true
    }

    /// How widget `id`'s inbox is drawn now (zero-copy).
    pub fn inbox_shown(&self, id: u32) -> olo_kit::glance::Inbox {
        let hot = self.hot.filter(|(w, _)| *w == id);
        let pressed = self.pressed.filter(|(w, _)| *w == id).map(|(_, p)| p);
        let receipt = self.receipts.get(&id).map(|r| match r.kind {
            ReceiptKind::Cleared { count, undoing, .. } => olo_kit::glance::InboxReceipt::Cleared { count, undoing },
            ReceiptKind::NothingCleared => olo_kit::glance::InboxReceipt::NothingCleared,
            ReceiptKind::NothingToUndo => olo_kit::glance::InboxReceipt::NothingToUndo,
        });
        olo_kit::glance::Inbox { selected: self.inbox_selected_index(id), over: hot.is_some(), hot: hot.and_then(|(_, part)| part), pressed, receipt }
    }

    /// The pointer came onto an inbox (`part`: one of its buttons) or
    /// left them all. Whether anything changed.
    pub fn inbox_hot(&mut self, hot: Option<(u32, Option<olo_kit::glance::InboxPart>)>) -> bool {
        std::mem::replace(&mut self.hot, hot) != hot
    }

    /// A button on an inbox is held down (or released). Whether anything changed.
    pub fn inbox_pressed(&mut self, pressed: Option<(u32, olo_kit::glance::InboxPart)>) -> bool {
        std::mem::replace(&mut self.pressed, pressed) != pressed
    }

    /// How tall widget `w`'s row is on its own (zero-copy for Inbox, Astra 24).
    pub fn height(&self, w: &Widget) -> f32 {
        let gl = self.now(w.id);
        if let Some(Body::Inbox { cards, preview, .. }) = gl.and_then(|gl| gl.body.as_ref()) {
            let has_card_or_receipt = self.receipts.contains_key(&w.id) || {
                let at_now = Instant::now();
                cards.iter().any(|c| !self.is_hidden(w.id, (c.attention, c.id), at_now))
            };
            return olo_kit::glance::inbox_height_for(has_card_or_receipt, *preview, w.size);
        }
        olo_kit::glance::height(gl, w.size)
    }

    /// Its tip in words.
    pub fn tip(&self, w: &Widget) -> String {
        self.now(w.id).map_or_else(|| olo_kit::tr!("{}: not here yet", w.label()), |g| g.tip.clone())
    }

    /// Its tip as a card: its name and value, the headline's words, what
    /// its body shows as rows, its note.
    pub fn card(&self, w: &Widget) -> Card {
        let Some(gl) = self.now(w.id) else { return Card::new(w.label()).part(Part::Notes(vec![olo_kit::words::tr("Waiting for it…").into()])) };
        let caption = [gl.title.as_str(), gl.detail.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let mut card = Card::new(w.label()).caption(caption).value(gl.value.clone(), None);
        match &gl.body {
            Some(Body::Inbox { .. }) => {
                if let Some(item) = self.inbox_snapshot(w.id).and_then(|s| s.top) {
                    card = card.part(Part::Notes(vec![format!("{} · {}", item.app, item.title).into()]));
                }
            }
            Some(Body::Slots { slots, .. }) => {
                let rows = slots.iter().map(|s| Row { level: s.level, ..Row::new(s.label.clone(), s.value.clone()) }).collect();
                card = card.part(Part::Rows { heading: None, rows });
            }
            Some(Body::Spans { spans, .. }) => {
                let rows = spans.iter().map(|s| Row::new(s.label.clone(), format!("{} to {}", s.low_text, s.high_text))).collect();
                card = card.part(Part::Rows { heading: None, rows });
            }
            Some(Body::Lines { lines }) => {
                let rows = lines.iter().map(|l| Row::new(l.label.clone(), l.text.clone())).collect();
                card = card.part(Part::Rows { heading: None, rows });
            }
            None => {}
        }
        if let Some(note) = &gl.note {
            card = card.part(Part::Notes(vec![note.clone().into()]));
        }
        card
    }

    /// What a click on app widget `w` runs: its app, on what the widget
    /// shows now where it says what that is (`Glance::open`).
    pub fn opens(&self, w: &Widget) -> Option<(String, &'static str)> {
        let Measure::App { app, .. } = &w.measure else { return None };
        let p = g::provider(app)?;
        let on: String = self.now(w.id).map(|gl| gl.open.iter().map(|a| format!(" {}", quoted(a))).collect()).unwrap_or_default();
        Some((format!("run:{}{on}", p.program), p.name))
    }
}

/// Whether `pid` runs `program` (the lock's pid may be another's by now).
fn is_provider(pid: i32, program: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| program.starts_with(c.trim()))
}

/// One word of a command line, quoted for the shell that runs it.
pub(crate) fn quoted(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_protocol::glance::Entry;

    fn card(id: u32, attention: bool, pinned: bool, time: u64) -> InboxCard {
        InboxCard { id, app: "Rex".into(), title: format!("Card {id}"), attention, pinned, time, instance: if attention { 0 } else { u64::from(id) * 10 }, ..Default::default() }
    }

    fn set_cards(gl: &mut Glances, widget: u32, cards: Vec<InboxCard>) {
        let body = Body::Inbox { cards, snooze_minutes: 15, preview: true };
        let t = Timeline { made: 0, entries: vec![Entry { from: 0, glance: Glance { body: Some(body), ..Default::default() } }] };
        gl.timelines.insert(widget, t);
        gl.reconcile_inboxes();
    }

    /// Clear All targets only the intersection of captured `(id, instance)` and
    /// currently eligible unpinned notifications: never pinned cards, never
    /// waiting cards, never a newcomer, never a repost with a new instance,
    /// and withheld if the producer is not pin-aware (Astra 17).
    #[test]
    fn clear_all_targets_captured_unpinned_only_and_gates_legacy_producer() {
        let mut gl = Glances::default();
        set_cards(&mut gl, 12, vec![card(1, true, false, 100), card(2, false, true, 101), card(3, false, false, 102), card(4, false, false, 103)]);
        // Newcomer 5 arrives before activation while hit captured (3, 30), (4, 40).
        set_cards(&mut gl, 12, vec![card(1, true, false, 100), card(2, false, true, 101), card(3, false, false, 102), card(4, false, false, 103), card(5, false, false, 104)]);
        let (generation, targets) = gl.begin_clear_all(12, &[(1, 0), (2, 20), (3, 30), (4, 40)]).unwrap();
        assert_eq!(targets, vec![(3, 30), (4, 40)]);
        assert_eq!(gl.inbox_receipt(12), Some(ReceiptInfo { generation, can_undo: true, dismissible_notice: false }));
        let (remaining, ..) = gl.inbox(12).unwrap();
        assert_eq!(remaining.iter().map(|c| (c.attention, c.id)).collect::<Vec<_>>(), [(true, 1), (false, 2), (false, 5)]);

        // Legacy producer (`instance == 0`) withholds Clear All plan and refuses begin_clear_all (Astra 17).
        let mut legacy = Glances::default();
        let old = |id: u32| InboxCard { id, app: "Rex".into(), title: "Old".into(), attention: false, pinned: false, instance: 0, ..Default::default() };
        set_cards(&mut legacy, 12, vec![old(1), old(2)]);
        assert!(legacy.inbox_snapshot(12).unwrap().clear_plan.is_none());
        assert!(legacy.begin_clear_all(12, &[(1, 0), (2, 0)]).is_none());
    }

    /// Waiting card ID collision and selection preservation.
    #[test]
    fn clear_all_keeps_waiting_card_on_id_collision_and_preserves_selection() {
        let mut gl = Glances::default();
        set_cards(&mut gl, 12, vec![card(10, true, false, 100), card(41, true, false, 101), card(41, false, false, 102), card(42, false, false, 103)]);
        assert!(gl.turn_card(12, false));
        let (cards, at, ..) = gl.inbox(12).unwrap();
        assert_eq!((cards[at].attention, cards[at].id), (true, 41));

        let (generation, targets) = gl.begin_clear_all(12, &[(41, 410), (42, 420)]).unwrap();
        assert_eq!(targets, vec![(41, 410), (42, 420)]);
        let (remaining, at, ..) = gl.inbox(12).unwrap();
        assert_eq!(remaining.iter().map(|c| (c.attention, c.id)).collect::<Vec<_>>(), [(true, 10), (true, 41)]);
        assert_eq!((remaining[at].attention, remaining[at].id), (true, 41), "waiting card B stays selected");

        let out = gl.finish_clear_all(12, generation, Ok((vec![41, 42], ":1.55".into(), "batch-1".into())));
        assert_eq!(out.undo_now, None);
        assert!(gl.tick_timers(Instant::now() + RECEIPT + Duration::from_secs(1)));
        assert_eq!(gl.inbox_receipt(12), None);
        let (remaining, at, ..) = gl.inbox(12).unwrap();
        assert_eq!((remaining[at].attention, remaining[at].id), (true, 41));
    }

    /// Astra 4: clicking Undo while `ClearMany` is still in flight is preserved
    /// even when a new card arrives and closes the visible receipt before the
    /// `ClearMany` reply arrives.
    #[test]
    fn queued_undo_survives_new_arrival_before_clear_reply() {
        let mut gl = Glances::default();
        set_cards(&mut gl, 12, vec![card(1, false, false, 10), card(2, false, false, 20)]);
        let (generation, _) = gl.begin_clear_all(12, &[(1, 10), (2, 20)]).unwrap();
        // User clicks Undo while ClearMany is still in flight.
        assert_eq!(gl.begin_undo(12, generation), Some(None));
        // New card 9 arrives BEFORE ClearMany replies -> closes visible receipt!
        set_cards(&mut gl, 12, vec![card(9, false, false, 99), card(1, false, false, 10), card(2, false, false, 20)]);
        assert_eq!(gl.inbox_receipt(12), None);
        // ClearMany finally replies -> queued Undo intent STILL fires!
        let out = gl.finish_clear_all(12, generation, Ok((vec![1, 2], ":1.42".into(), "b-1".into())));
        assert_eq!(out.undo_now, Some(UndoRequest { widget: 12, generation, owner: ":1.42".into(), batch: "b-1".into() }));
    }

    /// Astra 10 & 18: confirmed clears NEVER reappear after 3 s if the provider
    /// is slow; Undo keeps no local restored stack and displays cards in the
    /// provider's authoritative order once republished.
    #[test]
    fn confirmed_clears_stay_hidden_and_undo_uses_authoritative_provider_order() {
        let mut gl = Glances::default();
        // Order: A (id 1), Pinned P (id 2), B (id 3).
        set_cards(&mut gl, 12, vec![card(1, false, false, 30), card(2, false, true, 20), card(3, false, false, 10)]);
        let (generation, _) = gl.begin_clear_all(12, &[(1, 10), (3, 30)]).unwrap();
        gl.finish_clear_all(12, generation, Ok((vec![1, 3], ":1.42".into(), "b-1".into())));

        // Even 10 seconds later with no provider update yet, 1 and 3 stay hidden (Astra 10).
        gl.tick_timers(Instant::now() + Duration::from_secs(10));
        assert_eq!(gl.inbox(12).unwrap().0.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2]);

        // Fresh clear + Undo when provider already removed 1 and 3:
        let mut gl = Glances::default();
        set_cards(&mut gl, 12, vec![card(1, false, false, 30), card(2, false, true, 20), card(3, false, false, 10)]);
        let (gen2, _) = gl.begin_clear_all(12, &[(1, 10), (3, 30)]).unwrap();
        gl.finish_clear_all(12, gen2, Ok((vec![1, 3], ":1.42".into(), "b-2".into())));
        set_cards(&mut gl, 12, vec![card(2, false, true, 20)]); // provider confirms only P remains

        // Click Undo: no fabricated stack; receipt stays quiet (undoing) until provider republishes.
        let req = gl.begin_undo(12, gen2).unwrap().unwrap();
        assert_eq!((req.owner.as_str(), req.batch.as_str()), (":1.42", "b-2"));
        assert_eq!(gl.inbox_receipt(12), Some(ReceiptInfo { generation: gen2, can_undo: false, dismissible_notice: false }));
        assert!(gl.finish_undo(12, gen2, Ok(vec![1, 3])));
        // Provider still hasn't republished yet -> quiet receipt still up, P never promoted by local append.
        assert!(gl.inbox_receipt(12).is_some());
        // Provider republishes authoritative order [1, 2, 3]: receipt ends and A (1) is on top!
        set_cards(&mut gl, 12, vec![card(1, false, false, 30), card(2, false, true, 20), card(3, false, false, 10)]);
        assert_eq!(gl.inbox_receipt(12), None);
        let (cards, at, ..) = gl.inbox(12).unwrap();
        assert_eq!(cards.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(cards[at].id, 1, "original top card A stays on top in authoritative order");
    }

    /// Astra 9, 12, 13, 16, 21: failure restores cards with "Nothing was
    /// cleared" notice; timer deadline clears completely once receipt ends;
    /// reload round-trips active receipt; stray reply IDs are ignored; kind
    /// change drops Inbox receipt.
    #[test]
    fn failure_notice_timer_cancellation_reload_reply_validation_and_kind_change() {
        let mut gl = Glances::default();
        set_cards(&mut gl, 12, vec![card(1, false, false, 10), card(2, false, false, 20)]);
        let (g1, _) = gl.begin_clear_all(12, &[(1, 10), (2, 20)]).unwrap();
        assert!(gl.next_deadline().is_some(), "single timer deadline armed");

        // Stray ID 999 in reply is filtered out (Astra 16); only 1 and 2 count.
        gl.finish_clear_all(12, g1, Ok((vec![1, 2, 999, 1], ":1.7".into(), "b-ok".into())));
        assert_eq!(gl.inbox_shown(12).receipt, Some(olo_kit::glance::InboxReceipt::Cleared { count: 2, undoing: false }));

        // Save & restore across in-place Deskbar reload (Astra 13).
        let saved = gl.save_receipts();
        assert_eq!(saved.len(), 1);
        assert_eq!((saved[0].widget, saved[0].count, saved[0].owner.as_str(), saved[0].batch.as_str()), (12, 2, ":1.7", "b-ok"));
        let mut reloaded = Glances::default();
        reloaded.restore_receipts(saved);
        assert!(reloaded.inbox_receipt(12).is_some() && reloaded.next_deadline().is_some());

        // Changing widget 12 from Inbox to CPU clears its receipt & timer deadline (Astra 12, 21).
        let cpu = Widget::new(12, Measure::Cpu { cores: false });
        let proxy = olo_kit::Proxy::new(|_| {});
        reloaded.set(&[cpu], &proxy);
        assert_eq!(reloaded.inbox_receipt(12), None);
        assert_eq!(reloaded.next_deadline(), None, "no timer deadline left after widget kind change");

        // Service failure / timeout brings cards back and shows "Nothing was cleared" (Item 3 / Astra 9).
        let mut gl2 = Glances::default();
        set_cards(&mut gl2, 12, vec![card(1, false, false, 10), card(2, false, false, 20)]);
        let (g2, _) = gl2.begin_clear_all(12, &[(1, 10), (2, 20)]).unwrap();
        let out = gl2.finish_clear_all(12, g2, Err(()));
        assert!(out.redraw && out.undo_now.is_none());
        assert_eq!(gl2.inbox(12).unwrap().0.len(), 2, "cards restored at once");
        assert_eq!(gl2.inbox_shown(12).receipt, Some(olo_kit::glance::InboxReceipt::NothingCleared));
        assert!(gl2.dismiss_notice_receipt(12, g2));
        assert_eq!(gl2.inbox_receipt(12), None);
        assert_eq!(gl2.next_deadline(), None, "zero pending deadlines once dismissed");
    }
}
