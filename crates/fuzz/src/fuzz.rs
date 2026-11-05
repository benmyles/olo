//! The fuzzers' core: talking to app_server, waiting for the windows to
//! settle, and the checks every step makes (wm.rs, auto.rs: the steps).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::Write,
    time::{Duration, Instant},
};

use olo_protocol::{Frame, Reply, Request, SyntheticEvent, WindowInfo};

use crate::{
    rng::Rng,
    session::{Ipc, Session},
};

pub type Wins = BTreeMap<u64, WindowInfo>;

/// A rectangle: x, y, width, height (logical pixels).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl std::fmt::Display for R {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {}, {})", self.x, self.y, self.w, self.h)
    }
}

pub fn rect(f: &Frame) -> R {
    R { x: f.x, y: f.y, w: f.width, h: f.height }
}

/// A window's frame; None while it is hidden or on another workspace.
pub fn fr(w: &WindowInfo) -> Option<R> {
    w.frame.as_ref().map(rect)
}

pub fn fr_of(wins: &Wins, id: u64) -> Option<R> {
    wins.get(&id).and_then(fr)
}

pub fn tab_rect(w: &WindowInfo) -> Option<R> {
    w.tab_frame.as_ref().map(rect)
}

/// A window's group, said ("none" for none).
pub fn g(group: Option<u64>) -> String {
    group.map_or_else(|| "none".into(), |n| n.to_string())
}

pub fn opt(r: Option<R>) -> String {
    r.map_or_else(|| "None".into(), |r| r.to_string())
}

pub fn overlap(a: R, b: R) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    w.max(0) as i64 * h.max(0) as i64
}

/// `f` all inside `r`.
pub fn contains(r: R, f: R) -> bool {
    r.x <= f.x && r.y <= f.y && f.x + f.w <= r.x + r.w && f.y + f.h <= r.y + r.h
}

pub fn inside(p: (f64, f64), r: R) -> bool {
    r.x as f64 <= p.0 && p.0 < (r.x + r.w) as f64 && r.y as f64 <= p.1 && p.1 < (r.y + r.h) as f64
}

pub fn fmt_p(p: (f64, f64)) -> String {
    format!("({:.0}, {:.0})", p.0, p.1)
}

/// `want`, a tile's part, as app_server gives way for a window that can't
/// be that small (tiling.rs `give_way`): its least size, kept inside
/// `area` (one side held, else moved in from the edge it would cross).
pub fn give_way(want: R, least: Option<[i32; 2]>, area: R) -> R {
    let Some([lw, lh]) = least else { return want };
    let (w, h) = (want.w.max(lw), want.h.max(lh));
    if (w, h) == (want.w, want.h) {
        return want;
    }
    let inside = |pos: i32, n: i32, lo: i32, span: i32| if n <= span { pos.max(lo).min(lo + span - n) } else { lo };
    R { x: inside(want.x, w, area.x, area.w), y: inside(want.y, h, area.y, area.h), w, h }
}

/// Each tab's offset and width along a group's top (groups.rs `tab_bar`).
pub fn tab_bar(wanted: &[i32], frame_w: i32) -> Vec<(i32, i32)> {
    let n = wanted.len() as i32;
    if n == 0 {
        return Vec::new();
    }
    let widths: Vec<i32> = if wanted.iter().sum::<i32>() <= frame_w {
        wanted.to_vec()
    } else {
        let each = (frame_w / n).max(1);
        let mut v = vec![each; (n - 1) as usize];
        v.push((frame_w - each * (n - 1)).max(1));
        v
    };
    let mut x = 0;
    widths
        .into_iter()
        .map(|w| {
            let at = (x, w);
            x += w;
            at
        })
        .collect()
}

/// The offset and width a tab `width` wide has at `place` among tabs
/// wanting `wanted`.
pub fn tab_slot(wanted: &[i32], width: i32, frame_w: i32, place: usize) -> (i32, i32) {
    let mut all = wanted.to_vec();
    all.insert(place.min(all.len()), width);
    tab_bar(&all, frame_w)[place]
}

pub fn state(wins: &Wins) -> String {
    wins.iter()
        .map(|(i, w)| format!("{i} {} ws{}{}{} {}", w.app_id.trim_start_matches("olo."), w.workspace, if w.minimized { " hidden" } else { "" }, if w.focused { " *" } else { "" }, opt(fr(w))))
        .collect::<Vec<_>>()
        .join("; ")
}

/// A failed check, the run stopping (unless it keeps going).
#[derive(Debug)]
pub struct Failure(pub String);

pub type Step = Result<(), Failure>;

/// Each display's workspace showing: the active display's, and by
/// connector.
pub type Showing = (u32, HashMap<String, u32>);

pub struct Fuzz {
    pub session: Session,
    pub ipc: Ipc,
    pub rng: Rng,
    pub seed: u64,
    pub quiet: bool,
    pub keep_going: bool,
    pub history: Vec<String>,
    pub failures: Vec<String>,
    pub step: u32,
    pub out: Box<dyn Write + Send>,
    /// Windows put in a group by hand (or joined by one), which may share
    /// it with other apps', and ones taken out by hand since.
    pub by_hand: BTreeSet<u64>,
    pub alone: BTreeSet<u64>,
    /// Steps Terminal's text size has been made bigger (less smaller).
    pub text_steps: i32,
    /// Terminal windows whose text size changed, and when (`grid_pending`).
    pub grid_pending_since: HashMap<u64, u32>,
    pub only: HashSet<String>,
    pub tile_seen: HashSet<(u64, String)>,
    pub tiles_fail: bool,
    /// `--trace-tiles`: each window's part, frame, workspace, group and
    /// hidden state, printed whenever they change (as last printed).
    pub trace_tiles: Option<HashMap<u64, String>>,
    pub last_press: Option<Instant>,
    pub outputs: Vec<String>,
    pub home: HashMap<String, ((i32, i32), f64)>,
    /// Every state seen while settling, a few milliseconds apart.
    pub trail: Vec<Wins>,
    pub step_trail: Vec<(f64, Wins)>,
    pub step_t0: Instant,
    log_at: Option<u64>,
    /// Lines logged with the time (UTC, as app_server's log has it).
    pub timestamps: bool,
    /// auto-fuzz: restarts app_server now and then.
    pub reloads: bool,
}

impl Fuzz {
    pub fn new(name: &str, seed: u64, quiet: bool, out: Box<dyn Write + Send>) -> Result<Fuzz, String> {
        let session = Session::open(name)?;
        let ipc = Ipc::new(&session.socket);
        let log_at = std::fs::metadata(session.log_path()).ok().map(|m| m.len());
        let mut f = Fuzz {
            session,
            ipc,
            rng: Rng::new(seed),
            seed,
            quiet,
            keep_going: false,
            history: Vec::new(),
            failures: Vec::new(),
            step: 0,
            out,
            by_hand: BTreeSet::new(),
            alone: BTreeSet::new(),
            text_steps: 0,
            grid_pending_since: HashMap::new(),
            only: HashSet::new(),
            tile_seen: HashSet::new(),
            tiles_fail: false,
            trace_tiles: None,
            last_press: None,
            outputs: Vec::new(),
            home: HashMap::new(),
            trail: Vec::new(),
            step_trail: Vec::new(),
            step_t0: Instant::now(),
            log_at,
            timestamps: false,
            reloads: false,
        };
        let outs = f.outputs_info().map_err(|e| e.0)?;
        f.outputs = outs.iter().map(|o| o.name.clone()).collect();
        f.home = outs.iter().map(|o| (o.name.clone(), (o.position, o.scale))).collect();
        // (Any window there already may have been put in a group by hand,
        // in a run before.)
        f.by_hand = f.windows().map_err(|e| e.0)?.keys().copied().collect();
        Ok(f)
    }

    pub fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    // ---- talking to app_server ----

    /// `req`'s reply; an error reply is a failure.
    pub fn req(&mut self, req: Request) -> Result<Reply, Failure> {
        match self.try_req(req.clone())? {
            Reply::Error { message } => Err(Failure(format!("{} -> error {message}", serde_json::to_string(&req).unwrap_or_default()))),
            r => Ok(r),
        }
    }

    /// `req`'s reply, an error one too.
    pub fn try_req(&mut self, req: Request) -> Result<Reply, Failure> {
        self.ipc.call(&req).map_err(Failure)
    }

    pub fn windows(&mut self) -> Result<Wins, Failure> {
        match self.req(Request::ListWindows)? {
            Reply::Windows { windows } => Ok(windows.into_iter().map(|w| (w.id, w)).collect()),
            r => Err(Failure(format!("list_windows answered {r:?}"))),
        }
    }

    pub fn showing(&mut self) -> Result<Showing, Failure> {
        match self.req(Request::Workspaces)? {
            Reply::Workspaces { current, displays, .. } => Ok((current, displays.into_iter().map(|d| (d.output, d.current)).collect())),
            r => Err(Failure(format!("workspaces answered {r:?}"))),
        }
    }

    pub fn workspace(&mut self) -> Result<u32, Failure> {
        Ok(self.showing()?.0)
    }

    pub fn outputs_info(&mut self) -> Result<Vec<olo_protocol::OutputInfo>, Failure> {
        match self.req(Request::Outputs)? {
            Reply::Outputs { outputs } => Ok(outputs),
            r => Err(Failure(format!("outputs answered {r:?}"))),
        }
    }

    /// `w` is on the workspace its display shows (hidden there or not).
    pub fn here(w: &WindowInfo, cur: &Showing) -> bool {
        let shows = w.output.as_ref().and_then(|o| cur.1.get(o)).copied().unwrap_or(cur.0);
        w.workspace == shows
    }

    pub fn shown(&mut self, wins: &Wins) -> Result<Vec<u64>, Failure> {
        let cur = self.showing()?;
        Ok(wins.iter().filter(|(_, w)| Self::here(w, &cur) && !w.minimized).map(|(i, _)| *i).collect())
    }

    pub fn keys(&mut self, codes: &[u32]) -> Step {
        let mut events: Vec<SyntheticEvent> = codes.iter().map(|&k| SyntheticEvent::Key { key: k, pressed: true }).collect();
        events.extend(codes.iter().rev().map(|&k| SyntheticEvent::Key { key: k, pressed: false }));
        self.req(Request::Synthetic { events })?;
        Ok(())
    }

    pub fn event(&mut self, e: SyntheticEvent) -> Step {
        self.req(Request::Synthetic { events: vec![e] })?;
        Ok(())
    }

    pub fn shortcut(&mut self, id: &str, window: Option<u64>) -> Step {
        self.req(Request::RunShortcut { id: id.into(), window })?;
        Ok(())
    }

    pub fn spawn(&mut self, app: &str, args: &[&str]) -> Step {
        self.session.spawn(app, args).map_err(Failure)
    }

    /// The windows once they stop changing: none settling (its program
    /// still to draw what it was asked) and nothing changing for `quiet`
    /// seconds. Every state seen on the way is kept in `trail`, so a
    /// window that jumps and comes back (a flicker) is caught too.
    pub fn settle_for(&mut self, timeout: f64, quiet: f64) -> Result<Wins, Failure> {
        let t0 = Instant::now();
        self.trail.clear();
        type Key = Vec<(u64, Option<R>, bool, u32, bool, bool)>;
        let mut last: Option<Key> = None;
        let mut since = Instant::now();
        loop {
            let now = self.windows()?;
            let key: Key = now.iter().map(|(i, w)| (*i, fr(w), w.minimized, w.workspace, w.focused, w.settling)).collect();
            if last.as_ref() != Some(&key) {
                self.trail.push(now.clone());
                self.step_trail.push((self.step_t0.elapsed().as_secs_f64(), now));
                last = Some(key);
                since = Instant::now();
            } else if since.elapsed().as_secs_f64() > quiet && !now.values().any(|w| w.settling && !w.behind) {
                // (A tab behind another isn't drawn: it settles when in front.)
                return Ok(now);
            } else if t0.elapsed().as_secs_f64() > timeout {
                let settling: Vec<u64> = now.iter().filter(|(_, w)| w.settling).map(|(i, _)| *i).collect();
                self.history.push(format!("{}: (still settling after {timeout} s: {settling:?})", self.step));
                return Ok(now);
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    pub fn settle(&mut self) -> Result<Wins, Failure> {
        self.settle_for(5.0, 0.25)
    }

    /// The step's states so far, each window as it changed: for a
    /// failure's report.
    pub fn trail_report(&self) -> String {
        let mut lines = Vec::new();
        let mut seen: HashMap<u64, String> = HashMap::new();
        for (at, state) in &self.step_trail {
            let mut changed = Vec::new();
            for (i, w) in state {
                let k = format!(
                    "{}{}{}{}{} ws{} {}",
                    opt(fr(w)),
                    if w.settling { " settling" } else { "" },
                    if w.zoomed { " zoomed" } else { "" },
                    if w.tiled { " tiled" } else { "" },
                    w.group.map(|g| format!(" g{g}")).unwrap_or_default(),
                    w.workspace,
                    w.output.as_deref().unwrap_or("None")
                );
                if seen.get(i) != Some(&k) {
                    changed.push(format!("{i} {k}"));
                    seen.insert(*i, k);
                }
            }
            if !changed.is_empty() {
                lines.push(format!("{:7.1} ms: {}", at * 1000.0, changed.join("; ")));
            }
        }
        let start = lines.len().saturating_sub(30);
        lines[start..].join("\n        ")
    }

    pub fn log(&mut self, what: &str) {
        let line = if self.timestamps {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let s = now.as_secs();
            format!("{:02}:{:02}:{:02}.{:03} {what}", (s / 3600) % 24, (s / 60) % 60, s % 60, now.subsec_millis())
        } else {
            what.to_owned()
        };
        self.history.push(format!("{}: {line}", self.step));
        if !self.quiet {
            let step = self.step;
            self.say(&format!("{step:4} {line}"));
        }
    }

    pub fn pick(&mut self, ids: impl IntoIterator<Item = u64>) -> Option<u64> {
        self.rng.pick(ids)
    }

    // ---- the desktop's shape ----

    pub fn output_rects(&mut self) -> Result<Vec<R>, Failure> {
        Ok(self
            .outputs_info()?
            .into_iter()
            .filter(|o| o.mirror_of.is_none())
            .filter_map(|o| {
                let m = o.current_mode?;
                let s = if o.scale > 0.0 { o.scale } else { 1.0 };
                let (w, h) = if matches!(o.rotation, 0 | 180) { (m.width, m.height) } else { (m.height, m.width) };
                Some(R { x: o.position.0, y: o.position.1, w: (w as f64 / s).round() as i32, h: (h as f64 / s).round() as i32 })
            })
            .collect())
    }

    /// The active display and its tiling area.
    pub fn area(&mut self) -> Result<(Option<String>, Option<R>), Failure> {
        match self.req(Request::Arrangement)? {
            Reply::Arrangement { output, area, .. } => Ok((Some(output), Some(rect(&area)))),
            _ => Ok((None, None)),
        }
    }

    // ---- checks ----

    pub fn fail(&mut self, what: &str) -> Step {
        let start = self.history.len().saturating_sub(12);
        let msg = format!("step {} (seed {}): {what}\n  last: {}\n  seen: {}", self.step, self.seed, self.history[start..].join("\n        "), self.trail_report());
        if self.keep_going {
            self.say(&format!("FAIL {msg}"));
            self.failures.push(msg);
            Ok(())
        } else {
            Err(Failure(msg))
        }
    }

    pub fn check_sane(&mut self, wins: &Wins) -> Step {
        let t = Instant::now();
        self.req(Request::Ping)?;
        if t.elapsed() > Duration::from_secs(1) {
            self.fail("ping took over a second")?;
        }
        let cur = self.showing()?;
        let outs = self.output_rects()?;
        let shown: Vec<&WindowInfo> = wins.values().filter(|w| Self::here(w, &cur) && !w.minimized).collect();
        let mut problems = Vec::new();
        for w in &shown {
            let Some(f) = fr(w) else {
                problems.push(format!("shown window {} ({}) has no frame", w.id, w.app_id));
                continue;
            };
            if f.w <= 0 || f.h <= 0 {
                problems.push(format!("window {} ({}) is empty: {f}", w.id, w.app_id));
            }
            // Within reach: enough of its top edge on some display to take
            // hold of (it may hang off an edge, as a person can leave it).
            let top = R { h: f.h.min(20), ..f };
            if !outs.iter().any(|o| overlap(top, *o) >= f.w.min(32) as i64 * f.h.min(20) as i64 && o.y <= f.y) {
                problems.push(format!("window {} ({}) is off the displays: {f}; displays {:?}", w.id, w.app_id, outs.iter().map(R::to_string).collect::<Vec<_>>()));
            }
        }
        let focused: Vec<&WindowInfo> = wins.values().filter(|w| w.focused).collect();
        // With several displays, none has it when the display in use shows
        // none (the keyboard doesn't go to another display's).
        if focused.len() > 1 || (!shown.is_empty() && focused.is_empty() && cur.1.len() < 2) {
            problems.push(format!("{} windows have the keyboard with {} shown: {:?}", focused.len(), shown.len(), focused.iter().map(|w| w.id).collect::<Vec<_>>()));
        }
        for w in &focused {
            if w.minimized || !Self::here(w, &cur) {
                problems.push(format!("window {} has the keyboard but is {}", w.id, if w.minimized { "hidden".to_owned() } else { format!("on workspace {}", w.workspace) }));
            }
        }
        for p in problems {
            self.fail(&p)?;
        }
        self.check_groups(wins, &cur)?;
        self.check_zoomed(wins, &cur)?;
        self.check_tiles(wins, &cur)?;
        self.check_log()
    }

    /// A tiled window on the active display is where its tile's part says,
    /// within a gap, given way where its app won't be that small. Reported,
    /// with `--tiles` failing on it.
    fn check_tiles(&mut self, wins: &Wins, cur: &Showing) -> Step {
        if let Some(mut trace) = self.trace_tiles.take() {
            for w in wins.values() {
                let part = w.part.map(|p| format!("({:.3}, {:.3}, {:.3}, {:.3})", p[0], p[1], p[2], p[3])).unwrap_or_else(|| "None".into());
                let now = format!("part {part} frame {} ws {} group {} hidden {}", opt(fr(w)), w.workspace, w.group.map_or_else(|| "None".to_owned(), |g| g.to_string()), w.minimized);
                if trace.get(&w.id) != Some(&now) {
                    let step = self.step;
                    self.say(&format!("  step {step}: window {} {now}", w.id));
                    trace.insert(w.id, now);
                }
            }
            self.trace_tiles = Some(trace);
        }
        let (out, area) = self.area()?;
        let Some(area) = area else { return Ok(()) };
        for w in wins.values() {
            let (Some(part), Some(f)) = (w.part, fr(w)) else { continue };
            if w.output != out || !Self::here(w, cur) || w.minimized || w.zoomed {
                continue;
            }
            let [px, py, pw, ph] = part.map(f64::from);
            let (ax, ay, aw, ah) = (area.x as f64, area.y as f64, area.w as f64, area.h as f64);
            let want = give_way(R { x: (ax + px * aw).round() as i32, y: (ay + py * ah).round() as i32, w: (pw * aw).round() as i32, h: (ph * ah).round() as i32 }, w.least, area);
            let off = (f.x - want.x).abs().max((f.y - want.y).abs()).max((f.x + f.w - want.x - want.w).abs()).max((f.y + f.h - want.y - want.h).abs());
            if off > 12 {
                let key = (w.id, format!("{part:?} {f}"));
                if self.tile_seen.insert(key) {
                    let msg = format!("window {} ({}) at {f} isn't its tile's part {part:?} = {want}", w.id, w.app_id);
                    if self.tiles_fail {
                        self.fail(&msg)?;
                    } else {
                        let step = self.step;
                        self.say(&format!("  step {step}: {msg}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// A zoomed window on the active display fills (at most) its tiling
    /// area, whatever happened since it was zoomed.
    fn check_zoomed(&mut self, wins: &Wins, cur: &Showing) -> Step {
        let zoomed: Vec<(u64, String, R)> = wins.values().filter(|w| w.zoomed && Self::here(w, cur) && !w.minimized).filter_map(|w| Some((w.id, w.app_id.clone(), fr(w)?))).collect();
        if zoomed.is_empty() {
            return Ok(());
        }
        let (out, area) = self.area()?;
        let Some(area) = area else { return Ok(()) };
        for (id, app, f) in zoomed {
            if overlap(f, area) > 0 && !contains(area, f) {
                self.fail(&format!("zoomed window {id} ({app}) {f} sticks out of the tiling area {area} on {}", out.as_deref().unwrap_or("?")))?;
            }
        }
        Ok(())
    }

    /// No panic in app_server's log since the last look (olo-session would
    /// start it again, and the windows come back, hiding it).
    fn check_log(&mut self) -> Step {
        let Some(at) = self.log_at else { return Ok(()) };
        let Ok(bytes) = std::fs::read(self.session.log_path()) else { return Ok(()) };
        if (bytes.len() as u64) < at {
            self.log_at = Some(0);
            return Ok(());
        }
        let new = String::from_utf8_lossy(&bytes[at as usize..]).into_owned();
        self.log_at = Some(bytes.len() as u64);
        for line in new.lines().filter(|l| l.contains("panicked")) {
            self.fail(&format!("app_server panicked: {}", line.trim()))?;
        }
        Ok(())
    }

    fn check_groups(&mut self, wins: &Wins, cur: &Showing) -> Step {
        let mut groups: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        let mut problems = Vec::new();
        for (i, w) in wins {
            let Some(g) = w.group else {
                if let Some(t) = w.tab {
                    problems.push(format!("window {i} has tab place {t} but no group"));
                }
                continue;
            };
            if w.minimized || !Self::here(w, cur) {
                problems.push(format!("window {i} is {} but in group {g}", if w.minimized { "hidden" } else { "away" }));
            }
            groups.entry(g).or_default().push(*i);
        }
        for (g, m) in &groups {
            if m.len() < 2 {
                problems.push(format!("group {g} is just window {}", m[0]));
                continue;
            }
            let mut order = m.clone();
            order.sort_by_key(|i| wins[i].tab.unwrap_or(0));
            if order.iter().map(|i| wins[i].tab).collect::<Vec<_>>() != (0..m.len() as u32).map(Some).collect::<Vec<_>>() {
                problems.push(format!("group {g}'s tab places are {:?}", order.iter().map(|i| (*i, wins[i].tab)).collect::<Vec<_>>()));
            }
            let frames: BTreeSet<Option<(i32, i32, i32, i32)>> = m.iter().map(|i| fr(&wins[i]).map(|r| (r.x, r.y, r.w, r.h))).collect();
            if frames.len() > 1 {
                problems.push(format!("group {g}'s windows don't share a frame: {}", order.iter().map(|i| format!("{i} {}", opt(fr(&wins[i])))).collect::<Vec<_>>().join(", ")));
                continue;
            }
            if let Some(Some((x, y, fw, _))) = frames.into_iter().next() {
                let mut end = x;
                for i in &order {
                    let t = tab_rect(&wins[i]);
                    let ok = t.is_some_and(|t| t.y == y && t.h == crate::wm::TAB_H && t.x >= end && t.x + t.w <= x + fw);
                    if !ok {
                        problems.push(format!(
                            "group {g}'s tabs aren't side by side along its top ({x}, {y}, {fw}) in order: {}",
                            order.iter().map(|o| format!("{o} {}", opt(tab_rect(&wins[o])))).collect::<Vec<_>>().join(", ")
                        ));
                        break;
                    }
                    end = t.map_or(end, |t| t.x + t.w);
                }
            }
            let apps: BTreeSet<&str> = m.iter().map(|i| wins[i].app_id.as_str()).collect();
            if apps.len() > 1 && !m.iter().any(|i| self.by_hand.contains(i)) {
                problems.push(format!("group {g} mixes {apps:?} though none of {order:?} was put in it by hand"));
            }
        }
        for i in self.alone.clone() {
            if let Some(g) = wins.get(&i).and_then(|w| w.group) {
                problems.push(format!("window {i}, taken out of its group by hand, is in group {g} again"));
            }
        }
        for p in problems {
            self.fail(&p)?;
        }
        Ok(())
    }

    /// Terminal windows still fitting their grid after a text-size change
    /// (within five steps of it).
    pub fn grid_pending(&self) -> HashSet<u64> {
        self.grid_pending_since.iter().filter(|(_, at)| self.step.saturating_sub(**at) <= 5).map(|(i, _)| *i).collect()
    }

    fn late(&self, before: &Wins, after: &Wins) -> HashSet<u64> {
        self.grid_pending().into_iter().filter(|i| before.contains_key(i) && after.contains_key(i) && fr_of(before, *i) != fr_of(after, *i)).collect()
    }

    /// Everything back as it was; only `moving` moved on the way.
    pub fn check_round_trip(&mut self, what: &str, before: &Wins, after: &Wins, moving: &HashSet<u64>) -> Step {
        let late = self.late(before, after);
        let moved: Vec<String> = before
            .iter()
            .filter(|(i, _)| after.contains_key(i) && !late.contains(i))
            .filter_map(|(i, w)| {
                let (a, b) = (fr(w)?, fr_of(after, *i)?);
                (a != b).then(|| format!("{i} ({}) {a} -> {b}", w.app_id))
            })
            .collect();
        if !moved.is_empty() {
            return self.fail(&format!("{what} didn't come back: {}", moved.join(", ")));
        }
        // (Nor on the way: one fitting its grid as it shows again.)
        let on_the_way: HashSet<u64> =
            self.grid_pending().into_iter().filter(|i| before.contains_key(i) && self.trail.iter().any(|t| t.contains_key(i) && fr_of(t, *i) != fr_of(before, *i))).collect();
        let skip: HashSet<u64> = moving.union(&on_the_way).copied().collect();
        self.check_steady(what, before, &skip)
    }

    pub fn check_unchanged(&mut self, what: &str, before: &Wins, after: &Wins, skip: &HashSet<u64>) -> Step {
        let mut skip: HashSet<u64> = skip.iter().copied().collect();
        skip.extend(self.late(before, after));
        // (One still to draw what it was asked, behind its group's front,
        // draws it when it can.)
        skip.extend(before.iter().filter(|(_, w)| w.settling).map(|(i, _)| *i));
        // A tab behind another, not drawn, takes its group's size where it
        // is when it next draws.
        skip.extend(before.iter().filter(|(i, w)| w.behind && matches!((fr(w), fr_of(after, **i)), (Some(a), Some(b)) if (a.x, a.y) == (b.x, b.y))).map(|(i, _)| *i));
        // (A group whose windows weren't one size yet becomes so.)
        for g in Self::groups(before).values() {
            let sizes: HashSet<Option<(i32, i32)>> = g.iter().map(|i| fr_of(before, *i).map(|r| (r.w, r.h))).collect();
            if sizes.len() > 1 {
                skip.extend(g.iter().copied());
            }
        }
        let moved: Vec<String> = before
            .iter()
            .filter(|(i, _)| !skip.contains(i))
            .filter_map(|(i, w)| {
                let a2 = after.get(i)?;
                let (a, b) = (fr(w)?, fr(a2)?);
                (a != b && w.workspace == a2.workspace).then(|| format!("{i} ({}) {a} -> {b}", w.app_id))
            })
            .collect();
        if !moved.is_empty() {
            return self.fail(&format!("{what} moved other windows: {}", moved.join(", ")));
        }
        self.check_steady(what, before, &skip)
    }

    /// No window outside `skip` changed on the way, even for a moment.
    pub fn check_steady(&mut self, what: &str, before: &Wins, skip: &HashSet<u64>) -> Step {
        for state in self.trail.clone() {
            let flick: Vec<String> = state
                .iter()
                .filter(|(i, _)| !skip.contains(i))
                .filter_map(|(i, w)| {
                    let (a, b) = (fr_of(before, *i)?, fr(w)?);
                    (a != b).then(|| format!("{i} ({}) {a} -> {b} and back", before[i].app_id))
                })
                .collect();
            if !flick.is_empty() {
                return self.fail(&format!("{what} flickered other windows: {}", flick.join(", ")));
            }
        }
        Ok(())
    }

    pub fn check_back(&mut self, what: &str, wid: u64, before: &Wins, after: &Wins) -> Step {
        let (a, b) = (fr_of(before, wid), fr_of(after, wid));
        if self.grid_pending().contains(&wid) && a != b {
            return Ok(());
        }
        if before.contains_key(&wid) && after.contains_key(&wid) && a.is_some() && a != b {
            return self.fail(&format!("{what} didn't come back: window {wid} ({}) {} -> {}", before[&wid].app_id, opt(a), opt(b)));
        }
        Ok(())
    }

    pub fn check_inside_area(&mut self, what: &str, wid: u64, wins: &Wins) -> Step {
        let (out, area) = self.area()?;
        let (Some(w), Some(area)) = (wins.get(&wid), area) else { return Ok(()) };
        let Some(f) = fr(w).filter(|_| !w.minimized) else { return Ok(()) };
        // Only on the active display (the one `arrangement` describes).
        if overlap(f, area) == 0 {
            return Ok(());
        }
        if !contains(area, f) {
            return self.fail(&format!("{what}: window {wid} ({}) {f} sticks out of the tiling area {area} on {}", w.app_id, out.as_deref().unwrap_or("?")));
        }
        Ok(())
    }

    // ---- groups ----

    /// `w` and the windows in its group, which follow it.
    pub fn mates(wins: &Wins, w: u64) -> HashSet<u64> {
        if !wins.contains_key(&w) {
            return HashSet::from([w]);
        }
        Self::group_of(wins, w).into_iter().collect()
    }

    /// The windows in `w`'s group in the order of their tabs (just `w` if
    /// it is in none).
    pub fn group_of(wins: &Wins, w: u64) -> Vec<u64> {
        let Some(g) = wins.get(&w).and_then(|x| x.group) else { return vec![w] };
        let mut v: Vec<u64> = wins.iter().filter(|(_, o)| o.group == Some(g)).map(|(i, _)| *i).collect();
        v.sort_by_key(|i| wins[i].tab.unwrap_or(0));
        v
    }

    /// The groups, each in tab order, by number.
    pub fn groups(wins: &Wins) -> BTreeMap<u64, Vec<u64>> {
        let mut out: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        let mut ids: Vec<u64> = wins.keys().copied().collect();
        ids.sort_by_key(|i| (wins[i].tab.unwrap_or(0), *i));
        for i in ids {
            if let Some(g) = wins[&i].group {
                out.entry(g).or_default().push(i);
            }
        }
        out
    }

    /// Those of `members` that only grew to what `w`, just come into their
    /// group, can be at the least: where they were, or, tiled, in their
    /// tile wherever it went.
    pub fn grown(w: u64, members: &[u64], before: &Wins, after: &Wins) -> HashSet<u64> {
        let least = after.get(&w).and_then(|x| x.least).or_else(|| before.get(&w).and_then(|x| x.least));
        let Some([lw, lh]) = least else { return HashSet::new() };
        members
            .iter()
            .copied()
            .filter(|&i| {
                let (Some(a), Some(b)) = (fr_of(before, i), fr_of(after, i)) else { return false };
                i != w && a != b && (b.w, b.h) == (a.w.max(lw), a.h.max(lh)) && ((b.x, b.y) == (a.x, a.y) || (before[&i].tiled && after[&i].tiled))
            })
            .collect()
    }

    /// Those of `members`, tiled, that only gave back what `w`, just gone
    /// from their group, needed.
    pub fn shrunk(w: u64, members: &[u64], before: &Wins, after: &Wins) -> HashSet<u64> {
        let Some([lw, lh]) = before.get(&w).and_then(|x| x.least) else { return HashSet::new() };
        members
            .iter()
            .copied()
            .filter(|&i| {
                let (Some(a), Some(b)) = (fr_of(before, i), fr_of(after, i)) else { return false };
                i != w && a != b && before[&i].tiled && after[&i].tiled && (a.w, a.h) == (b.w.max(lw), b.h.max(lh))
            })
            .collect()
    }

    // ---- dragging tabs ----

    /// ⌘-drag from `start` through the points of `path`, as a person
    /// would: ⌘ down, press, move a step at a time, let go, ⌘ up. Returns
    /// the windows just before letting go, if asked.
    pub fn cmd_drag(&mut self, start: (f64, f64), path: &[(f64, f64)], look_before_letting_go: bool) -> Result<Option<Wins>, Failure> {
        use crate::wm::{BTN_LEFT, CMD};
        // (Two presses on a tab in a moment are a double-click: rolling up.)
        if let Some(t) = self.last_press {
            std::thread::sleep(Duration::from_secs_f64(0.45).saturating_sub(t.elapsed()));
        }
        self.last_press = Some(Instant::now());
        self.event(SyntheticEvent::Move { x: start.0, y: start.1 })?;
        self.event(SyntheticEvent::Key { key: CMD, pressed: true })?;
        self.event(SyntheticEvent::Button { button: BTN_LEFT, pressed: true })?;
        // (The drag starts once the loop turns.)
        std::thread::sleep(Duration::from_millis(30));
        let (mut x, mut y) = start;
        for &(tx, ty) in path {
            let n = 2.max(((tx - x).abs().max((ty - y).abs()) / 40.0) as i32);
            for k in 1..=n {
                let f = k as f64 / n as f64;
                self.event(SyntheticEvent::Move { x: x + (tx - x) * f, y: y + (ty - y) * f })?;
            }
            (x, y) = (tx, ty);
            std::thread::sleep(Duration::from_millis(10));
        }
        let seen = if look_before_letting_go { Some(self.windows()?) } else { None };
        self.event(SyntheticEvent::Button { button: BTN_LEFT, pressed: false })?;
        self.event(SyntheticEvent::Key { key: CMD, pressed: false })?;
        self.last_press = Some(Instant::now());
        Ok(seen)
    }

    pub fn on_display(&mut self, p: (f64, f64)) -> Result<bool, Failure> {
        Ok(self.output_rects()?.iter().any(|o| inside(p, *o)))
    }

    /// A point straight above or below `at`, far enough for a tab to come
    /// out of its bar; on a display.
    pub fn pull_away(&mut self, at: (f64, f64)) -> Result<Option<(f64, f64)>, Failure> {
        for dy in self.rng.sample(&[70.0, -70.0], 2) {
            let p = (at.0, at.1 + dy);
            if self.on_display(p)? {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    /// The run: `steps` steps of `step`, stopping at the first failure
    /// unless it keeps going. Its exit code.
    pub fn run(&mut self, steps: u32, kind: &str, trails: &HashSet<u32>, step: fn(&mut Fuzz) -> Step) -> i32 {
        let (name, seed) = (self.session.name.clone(), self.seed);
        self.say(&format!("{kind} {name}: seed {seed}, {steps} steps"));
        for _ in 0..steps {
            if crate::stopping() {
                let at = self.step;
                self.say(&format!("interrupted after {at} steps (seed {seed})"));
                return 130;
            }
            crate::session::reap();
            self.step += 1;
            self.step_trail.clear();
            self.step_t0 = Instant::now();
            if let Err(Failure(msg)) = step(self) {
                self.say(&format!("FAIL {msg}"));
                return 1;
            }
            if trails.contains(&self.step) {
                let report = self.trail_report();
                let at = self.step;
                self.say(&format!("  seen in step {at}: {report}"));
            }
        }
        if !self.failures.is_empty() {
            let n = self.failures.len();
            self.say(&format!("\n{n} failures (seed {seed})"));
            return 1;
        }
        self.say(&format!("ok: {steps} steps (seed {seed})"));
        0
    }
}
