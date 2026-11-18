//! The widgets apps give Deskbar (`olo_protocol::glance`): each app's
//! provider told what's wanted of it (a file each app, written only as
//! that changes), started while it's wanted (again if it ends, once a
//! minute at most; afresh as Deskbar starts, so an upgrade's is the new
//! one), and its widgets' timelines read as their files change (an
//! inotify watch: nothing between). Each shows its entry for the time,
//! switched as the clock's minute comes; drawn by the kit
//! (`olo_kit::glance`), the same as anywhere.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use olo_kit::card::{Card, Part, Row};
use olo_protocol::{
    deskbar::{Measure, Widget},
    glance::{self as g, Body, Glance, Request, Requests, Timeline},
};

/// The least time between two starts of one app's provider.
const RESTART: Duration = Duration::from_secs(60);
/// How long a card dealt with here stays out of sight with no word from
/// its app (the notification service not running: it shows again).
const GONE: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Glances {
    dir: PathBuf,
    /// What each app's provider was last asked.
    asked: HashMap<&'static str, Requests>,
    /// Each app's provider while it runs, and when it was last started.
    running: std::collections::HashSet<&'static str>,
    started: HashMap<&'static str, Instant>,
    /// Each widget's timeline, and the entry it showed last (its time).
    timelines: HashMap<u32, Timeline>,
    showing: HashMap<u32, i64>,
    /// An inbox's card on top, by its item (None: the first), and the
    /// part of an inbox the pointer is on.
    selected: HashMap<u32, (u32, usize)>,
    hot: Option<(u32, Option<olo_kit::glance::InboxPart>)>,
    /// Inbox cards dismissed or snoozed here that their app hasn't taken
    /// away yet (the item, when): not shown meanwhile, so a click answers
    /// at once (the service's answer comes a quarter of a second later).
    gone: HashMap<u32, Vec<(u32, Instant)>>,
    /// The cards each inbox had when last read: one that comes after goes
    /// on top (Ben, 2026-11-17: "if newer ones arrive they should be
    /// added on top of the card stack too, not the bottom").
    had: HashMap<u32, Vec<u32>>,
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

    /// The widgets as the settings now say: each app asked for its own,
    /// its provider started if it isn't running; none, the folder
    /// unwatched and the providers told so (they end).
    pub fn set(&mut self, widgets: &[Widget], proxy: &olo_kit::Proxy) {
        self.selected.retain(|id, _| widgets.iter().any(|w| w.id == *id));
        self.gone.retain(|id, _| widgets.iter().any(|w| w.id == *id));
        self.had.retain(|id, _| widgets.iter().any(|w| w.id == *id));
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
        self.timelines.retain(|id, _| widgets.iter().any(|w| w.id == *id));
        self.showing.retain(|id, _| self.timelines.contains_key(id));
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
                && self.timelines.get(&id) != Some(&t)
            {
                self.timelines.insert(id, t);
                changed = true;
            }
        }
        if changed {
            self.turned();
            // A card that has just come goes on top, whichever was chosen
            // (the newest of them, in whichever order they're shown).
            for (id, t) in &self.timelines {
                let Some(Body::Inbox { cards, .. }) = t.at(now()).and_then(|gl| gl.body.as_ref()) else { continue };
                let now_ids: Vec<u32> = cards.iter().map(|c| c.id).collect();
                if let Some(had) = self.had.get(id)
                    && let Some((at, new)) = cards.iter().enumerate().filter(|(_, c)| !had.contains(&c.id)).max_by_key(|(_, c)| (c.time, c.id))
                {
                    self.selected.insert(*id, (new.id, at));
                }
                self.had.insert(*id, now_ids);
            }
            // (What was dealt with here and is gone there too.)
            let timelines = &self.timelines;
            self.gone.retain(|id, items| {
                let there = |item: u32| timelines.get(id).is_some_and(|t| t.entries.iter().any(|e| matches!(&e.glance.body, Some(Body::Inbox { cards, .. }) if cards.iter().any(|c| c.id == item))));
                items.retain(|(item, _)| there(*item));
                !items.is_empty()
            });
        }
        changed
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

    /// Widget `id`'s inbox, if it shows one: its cards, which is on top
    /// (the one chosen while it is still there, else the first), how
    /// long Snooze snoozes, and whether cards show their text.
    pub fn inbox(&self, id: u32) -> Option<(Vec<olo_protocol::notifications::InboxCard>, usize, u32, bool)> {
        let Body::Inbox { cards, snooze_minutes, preview } = self.now(id)?.body.as_ref()? else { return None };
        let gone = self.gone.get(&id).map(|items| items.iter().filter(|(_, at)| at.elapsed() < GONE).map(|(item, _)| *item).collect::<Vec<_>>()).unwrap_or_default();
        // (And not what has faded away, should its app be late to say so.)
        let secs = now().max(0) as u64;
        let cards: Vec<_> = cards.iter().filter(|c| !gone.contains(&c.id) && c.strength(secs) > 0.0).cloned().collect();
        // The card chosen while it is there; gone, the one that took its
        // place (the next comes up under the pointer).
        let at = match self.selected.get(&id) {
            Some((item, was)) => cards.iter().position(|c| c.id == *item).unwrap_or((*was).min(cards.len().saturating_sub(1))),
            None => 0,
        };
        Some((cards, at, *snooze_minutes, *preview))
    }

    /// What widget `id` shows now, as it's drawn: an inbox less the cards
    /// dealt with here.
    pub fn shown(&self, id: u32) -> Option<std::borrow::Cow<'_, Glance>> {
        let gl = self.now(id)?;
        let secs = now().max(0) as u64;
        let whole = |gl: &Glance| !matches!(&gl.body, Some(Body::Inbox { cards, .. }) if cards.iter().any(|c| c.strength(secs) <= 0.0));
        if !self.gone.contains_key(&id) && whole(gl) {
            return Some(std::borrow::Cow::Borrowed(gl));
        }
        let (cards, _, snooze_minutes, preview) = self.inbox(id)?;
        Some(std::borrow::Cow::Owned(Glance { body: Some(Body::Inbox { cards, snooze_minutes, preview }), ..gl.clone() }))
    }

    /// Whether a card on top of an inbox is fading away (its dot drains:
    /// drawn again as it does).
    pub fn fading(&self) -> bool {
        let secs = now().max(0) as u64;
        self.timelines.keys().any(|id| self.inbox(*id).is_some_and(|(cards, at, ..)| cards.get(at).is_some_and(|c| c.strength(secs) < 1.0 || c.fade_minutes.is_some())))
    }

    /// Card `item` of widget `id` was dismissed or snoozed here: gone at
    /// once, the card after it in its place.
    pub fn dealt_with(&mut self, id: u32, item: u32) {
        if let Some((cards, at, ..)) = self.inbox(id) {
            match cards.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| cards.get(i))) {
                // (Where the one after it will be once this one is gone.)
                Some(next) => self.selected.insert(id, (next.id, at.min(cards.len().saturating_sub(2)))),
                None => self.selected.remove(&id),
            };
        }
        self.gone.entry(id).or_default().push((item, Instant::now()));
    }

    /// The next card on top (`previous`: the one before), round and
    /// round. Whether it changed.
    pub fn turn_card(&mut self, id: u32, previous: bool) -> bool {
        let Some((cards, at, ..)) = self.inbox(id).filter(|(cards, ..)| cards.len() > 1) else { return false };
        let at = (at + if previous { cards.len() - 1 } else { 1 }) % cards.len();
        self.selected.insert(id, (cards[at].id, at));
        true
    }

    /// How widget `id`'s inbox is drawn now.
    pub fn inbox_shown(&self, id: u32) -> olo_kit::glance::Inbox {
        let hot = self.hot.filter(|(w, _)| *w == id);
        olo_kit::glance::Inbox { selected: self.inbox(id).map_or(0, |(_, at, ..)| at), over: hot.is_some(), hot: hot.and_then(|(_, part)| part) }
    }

    /// The pointer came onto an inbox (`part`: one of its buttons) or
    /// left them all. Whether anything changed.
    pub fn inbox_hot(&mut self, hot: Option<(u32, Option<olo_kit::glance::InboxPart>)>) -> bool {
        std::mem::replace(&mut self.hot, hot) != hot
    }

    /// How tall widget `w`'s row is on its own.
    pub fn height(&self, w: &Widget) -> f32 {
        olo_kit::glance::height(self.shown(w.id).as_deref(), w.size)
    }

    /// Its tip in words.
    pub fn tip(&self, w: &Widget) -> String {
        self.now(w.id).map_or_else(|| format!("{}: not here yet", w.label()), |g| g.tip.clone())
    }

    /// Its tip as a card: its name and value, the headline's words, what
    /// its body shows as rows, its note.
    pub fn card(&self, w: &Widget) -> Card {
        let Some(gl) = self.now(w.id) else { return Card::new(w.label()).part(Part::Notes(vec!["Waiting for it…".into()])) };
        let caption = [gl.title.as_str(), gl.detail.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let mut card = Card::new(w.label()).caption(caption).value(gl.value.clone(), None);
        match &gl.body {
            Some(Body::Inbox { .. }) => {
                if let Some((cards, at, ..)) = self.inbox(w.id)
                    && let Some(item) = cards.get(at)
                {
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
            None => {}
        }
        if let Some(note) = &gl.note {
            card = card.part(Part::Notes(vec![note.clone().into()]));
        }
        card
    }
}

/// Whether `pid` runs `program` (the lock's pid may be another's by now).
fn is_provider(pid: i32, program: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| program.starts_with(c.trim()))
}

/// What a click on app widget `w` runs: its app.
pub fn opens(w: &Widget) -> Option<(String, &'static str)> {
    let Measure::App { app, .. } = &w.measure else { return None };
    let p = g::provider(app)?;
    Some((format!("run:{}", p.program), p.name))
}
