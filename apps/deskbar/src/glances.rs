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
    deskbar::{Measure, Size, Widget},
    glance::{self as g, Body, Glance, Request, Requests, Timeline},
};

/// The least time between two starts of one app's provider.
const RESTART: Duration = Duration::from_secs(60);

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

    /// How tall widget `w`'s row is on its own.
    pub fn height(&self, w: &Widget) -> f32 {
        olo_kit::glance::height(self.now(w.id), w.size)
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

/// A beside-row cell's height for `w` (its icon and value, its title
/// under): as the meters' by size.
pub fn cell_height(size: Size) -> f32 {
    match size {
        Size::Small => 32.0,
        Size::Medium => 40.0,
        Size::Large => 52.0,
    }
}
