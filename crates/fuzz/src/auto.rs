//! `olo-fuzz auto`: auto-tiling, adversarially. Auto-Tiling on for the
//! first four workspaces; mostly drags of the seams between tiles the way
//! people do (fast and slow, a few steps or many, past where it stops and
//! back, held still before letting go), with windows opening, closing,
//! grouping, ungrouping, changing tabs, floating and coming forward,
//! workspace trips, and (`--reloads`) app_server restarting in place.
//! After each step, once settled: every tile inside the tiling area, none
//! over another, no float under a tile, and nothing still moving a moment
//! later; during each drag, sampled every few milliseconds on a thread of
//! its own: no tile over another, and tiles side by side as it began still
//! the tile gap apart.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_protocol::{Reply, Request, SyntheticEvent};

use crate::{
    fuzz::{Failure, Fuzz, R, Step, Wins, contains, fmt_p, fr, overlap, state},
    session::Ipc,
    wm::{APPS, BTN_LEFT, launch},
};

/// How long after settling a tile must still be where it settled.
const STILL: f64 = 0.4;
/// More windows than this on a workspace and their least sizes may not fit.
const MOST: usize = 5;

type Op = fn(&mut Fuzz, &Wins, &[u64]) -> Step;

pub fn start(f: &mut Fuzz) -> Step {
    f.timestamps = true;
    for ws in 0..4 {
        f.req(Request::SetAutoTiling { on: Some(true), workspace: Some(ws), output: None })?;
    }
    Ok(())
}

pub fn step(f: &mut Fuzz) -> Step {
    let wins = f.windows()?;
    let shown = f.shown(&wins)?;
    let mut table: Vec<(&str, u32, Op)> = vec![
        ("drag", 10, op_drag),
        ("open", 3, op_open),
        ("close", 2, op_close),
        ("workspace_trip", 2, op_workspace_trip),
        ("group", 2, op_group),
        ("ungroup", 2, op_ungroup),
        ("tab", 2, op_tab),
        ("float", 2, op_float),
        ("activate_float", 3, op_activate_float),
    ];
    if f.reloads {
        table.push(("reload", 1, op_reload));
    }
    let mut ops: Vec<(u32, Op)> = table.iter().filter(|(n, _, _)| f.only.is_empty() || f.only.contains(*n)).map(|(_, w, op)| (*w, *op)).collect();
    if tiles(f, &wins)?.len() < 2 {
        ops = vec![(1, op_open)];
    }
    let op = f.rng.weighted(&ops);
    op(f, &wins, &shown)
}

pub const OPS: [&str; 10] = ["drag", "open", "close", "workspace_trip", "group", "ungroup", "tab", "float", "activate_float", "reload"];

// ---- what's on screen ----

/// The tiles shown now: one window for each (a group's front).
fn tiles_on(wins: &Wins, ws: u32) -> Vec<u64> {
    let mut out = Vec::new();
    let mut groups = HashSet::new();
    for (i, w) in wins {
        if w.workspace != ws || w.minimized || fr(w).is_none() || !w.tiled || w.zoomed || w.floating || w.rolled {
            continue;
        }
        if let Some(g) = w.group
            && !groups.insert(g)
        {
            continue;
        }
        out.push(*i);
    }
    out
}

fn tiles(f: &mut Fuzz, wins: &Wins) -> Result<Vec<u64>, Failure> {
    let ws = f.workspace()?;
    Ok(tiles_on(wins, ws))
}

/// The floating windows shown now (floated by hand, dialogs, fixed-size
/// ones).
fn floating(f: &mut Fuzz, wins: &Wins) -> Result<Vec<u64>, Failure> {
    let ws = f.workspace()?;
    Ok(wins.iter().filter(|(_, w)| w.floating && w.workspace == ws && !w.minimized && fr(w).is_some()).map(|(i, _)| *i).collect())
}

// ---- checks ----

fn check_auto_tiles(f: &mut Fuzz, what: &str, wins: &Wins) -> Step {
    let (_, area) = f.area()?;
    let Some(area) = area else { return Ok(()) };
    let tiles = tiles(f, wins)?;
    for i in &tiles {
        let r = fr(&wins[i]).expect("a tile");
        if !contains(area, r) {
            f.fail(&format!("{what}: tile {i} ({}) {r} sticks out of the tiling area {area}\n  {}", wins[i].app_id, state(wins)))?;
        }
    }
    for (n, a) in tiles.iter().enumerate() {
        for b in &tiles[n + 1..] {
            let (ra, rb) = (fr(&wins[a]).expect("a tile"), fr(&wins[b]).expect("a tile"));
            if overlap(ra, rb) > 0 {
                f.fail(&format!("{what}: tiles {a} ({}) {ra} and {b} ({}) {rb} overlap\n  {}", wins[a].app_id, wins[b].app_id, state(wins)))?;
            }
        }
    }
    Ok(())
}

/// Floating windows over every tile (`stack`: 0 is the front).
fn check_floats_over(f: &mut Fuzz, what: &str, wins: &Wins) -> Step {
    let (tiles, floats) = (tiles(f, wins)?, floating(f, wins)?);
    let Some(top_tile) = tiles.iter().filter_map(|i| wins[i].stack).min() else { return Ok(()) };
    let under: Vec<u64> = floats.into_iter().filter(|i| wins[i].stack.is_some_and(|s| s > top_tile)).collect();
    if !under.is_empty() {
        return f.fail(&format!("{what}: floating {under:?} under a tile\n  {}", state(wins)));
    }
    Ok(())
}

/// Nothing moves after settling: a late size landing would.
fn check_still(f: &mut Fuzz, what: &str, wins: &Wins) -> Step {
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < STILL {
        let now = f.windows()?;
        let moved: Vec<String> = now
            .iter()
            .filter_map(|(i, w)| {
                let b = wins.get(i)?;
                (fr(b) != fr(w) && b.workspace == w.workspace).then(|| format!("{i} ({}) {} -> {}", w.app_id, crate::fuzz::opt(fr(b)), crate::fuzz::opt(fr(w))))
            })
            .collect();
        if !moved.is_empty() {
            return f.fail(&format!("{what}: settled, then moved: {}", moved.join(", ")));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Settle, and check everything.
fn after(f: &mut Fuzz, what: &str, timeout: f64) -> Result<Wins, Failure> {
    let wins = f.settle_for(timeout, 0.25)?;
    f.check_sane(&wins)?;
    check_auto_tiles(f, what, &wins)?;
    check_floats_over(f, what, &wins)?;
    check_still(f, what, &wins)?;
    Ok(wins)
}

// ---- drags ----

/// Each edge between two tiles side by side (with the gap between them):
/// the tile whose edge it is, its side, where along it they meet (below
/// both windows' tabs: beside a tab is whatever is behind it).
/// A seam: the window whose edge it is, along x or not, and where it is.
type Seam = (u64, bool, (i32, i32));

fn seams(f: &mut Fuzz, wins: &Wins) -> Result<Vec<Seam>, Failure> {
    let tiles = tiles(f, wins)?;
    let body_top = |i: u64| {
        let y = fr(&wins[&i]).expect("a tile").y;
        crate::fuzz::tab_rect(&wins[&i]).map_or(y, |t| y.max(t.y + t.h))
    };
    let mut out = Vec::new();
    for &a in &tiles {
        let ra = fr(&wins[&a]).expect("a tile");
        for &b in &tiles {
            if a == b {
                continue;
            }
            let rb = fr(&wins[&b]).expect("a tile");
            let along_y = (body_top(a).max(body_top(b)), (ra.y + ra.h).min(rb.y + rb.h));
            let along_x = (ra.x.max(rb.x), (ra.x + ra.w).min(rb.x + rb.w));
            if (0..=16).contains(&(rb.x - (ra.x + ra.w))) && along_y.1 - along_y.0 > 40 {
                out.push((a, true, along_y));
            }
            if (0..=16).contains(&(rb.y - (ra.y + ra.h))) && along_x.1 - along_x.0 > 40 {
                out.push((a, false, along_x));
            }
        }
    }
    Ok(out)
}

/// Each tile's frame, by its window (a group's: the front one's, what
/// shows; a tab behind it isn't drawn, and follows it).
fn fronts(wins: &Wins, ws: u32) -> BTreeMap<u64, R> {
    tiles_on(wins, ws)
        .into_iter()
        .map(|i| {
            let g = wins[&i].group;
            let front = wins.iter().find(|(_, w)| g.is_some() && w.group == g && !w.behind && fr(w).is_some()).map_or(i, |(j, _)| *j);
            (i, fr(&wins[&front]).expect("a frame"))
        })
        .collect()
}

/// The tiles' frames, sampled on a thread of its own every few
/// milliseconds till stopped, and which were side by side (a gap apart)
/// as it began.
/// The tiles' looks while a drag goes on: when, and each tile's frame.
type Samples = Vec<(f64, BTreeMap<u64, R>)>;

/// Two tiles side by side (a, b, along x or not, the gap between).
type Pair = (u64, u64, bool, i32);

struct MidDrag {
    start: BTreeMap<u64, R>,
    pairs: Vec<Pair>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Samples>>,
}

/// (A step that failed before the drag ended: the sampler stops with it.)
impl Drop for MidDrag {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl MidDrag {
    fn begin(f: &Fuzz, wins: &Wins, ws: u32) -> MidDrag {
        let start = fronts(wins, ws);
        let mut pairs = Vec::new();
        for (&a, ra) in &start {
            for (&b, rb) in &start {
                if a != b && (0..=16).contains(&(rb.x - (ra.x + ra.w))) && (ra.y + ra.h).min(rb.y + rb.h) - ra.y.max(rb.y) > 40 {
                    pairs.push((a, b, true, rb.x - (ra.x + ra.w)));
                }
                if a != b && (0..=16).contains(&(rb.y - (ra.y + ra.h))) && (ra.x + ra.w).min(rb.x + rb.w) - ra.x.max(rb.x) > 40 {
                    pairs.push((a, b, false, rb.y - (ra.y + ra.h)));
                }
            }
        }
        // The ones a gap apart, as app_server links them to follow an edge
        // (within 2 px of it); the gap the most of them are apart is the
        // tile gap.
        let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
        for p in &pairs {
            *counts.entry(p.3).or_default() += 1;
        }
        let gap = counts.iter().max_by_key(|(_, n)| **n).map_or(0, |(g, _)| *g);
        pairs.retain(|p| (p.3 - gap).abs() <= 2);
        let stop = Arc::new(AtomicBool::new(false));
        let (done, socket) = (stop.clone(), f.session.socket.clone());
        let thread = std::thread::spawn(move || {
            let mut ipc = Ipc::new(&socket);
            let t0 = Instant::now();
            let mut samples = Vec::new();
            while !done.load(Ordering::Relaxed) {
                let Ok(Reply::Windows { windows }) = ipc.call(&Request::ListWindows) else { break };
                let wins: Wins = windows.into_iter().map(|w| (w.id, w)).collect();
                samples.push((t0.elapsed().as_secs_f64(), fronts(&wins, ws)));
                std::thread::sleep(Duration::from_millis(4));
            }
            samples
        });
        MidDrag { start, pairs, stop, thread: Some(thread) }
    }

    fn end(mut self) -> (BTreeMap<u64, R>, Vec<Pair>, Samples) {
        self.stop.store(true, Ordering::Relaxed);
        let samples = self.thread.take().map(|t| t.join().unwrap_or_default()).unwrap_or_default();
        (std::mem::take(&mut self.start), std::mem::take(&mut self.pairs), samples)
    }
}

/// While the edge moved, every sample: no tile over another, and tiles
/// side by side when it began still a gap apart (they move together:
/// app_server shows each step of a drag at once, `rounds`).
fn check_mid_drag(f: &mut Fuzz, what: &str, watch: MidDrag) -> Step {
    let (start, pairs, samples) = watch.end();
    let n = samples.len();
    // (Its score, when, tiles over others, pairs out of step, the tiles.)
    type Worst = (i64, f64, Vec<(u64, u64)>, Vec<(u64, u64, i32)>, BTreeMap<u64, R>);
    let mut worst: Option<Worst> = None;
    let mut bad = 0;
    for (t, tiles) in samples {
        let ids: Vec<u64> = tiles.keys().copied().collect();
        let over: Vec<(u64, u64)> = ids.iter().enumerate().flat_map(|(k, a)| ids[k + 1..].iter().filter(|b| overlap(tiles[a], tiles[b]) > 0).map(|b| (*a, *b)).collect::<Vec<_>>()).collect();
        let mut apart = Vec::new();
        for &(a, b, x_axis, gap) in &pairs {
            let (Some(ra), Some(rb)) = (tiles.get(&a), tiles.get(&b)) else { continue };
            let d = if x_axis { rb.x - (ra.x + ra.w) } else { rb.y - (ra.y + ra.h) };
            if (d - gap).abs() > 1 {
                apart.push((a, b, d - gap));
            }
        }
        if !over.is_empty() || !apart.is_empty() {
            bad += 1;
            let score = apart.iter().map(|x| x.2.abs() as i64).sum::<i64>() + 1000 * over.len() as i64;
            if worst.as_ref().is_none_or(|w| score > w.0) {
                worst = Some((score, t, over, apart, tiles));
            }
        }
    }
    let Some((_, t, over, apart, tiles)) = worst else { return Ok(()) };
    let list = |m: &BTreeMap<u64, R>| m.iter().map(|(i, r)| format!("{i} {r}")).collect::<Vec<_>>().join("; ");
    f.fail(&format!(
        "{what}: mid-drag, {bad} of {n} looks had tiles out of step (worst at {:.0} ms: {}{})\n  {}\n  as it began: {}",
        t * 1000.0,
        if over.is_empty() { String::new() } else { format!("{over:?} overlapping") },
        if apart.is_empty() { String::new() } else { format!(" {:?} apart by more than the gap", apart.iter().map(|(a, b, d)| (a, b, format!("{d:+} px"))).collect::<Vec<_>>()) },
        list(&tiles),
        list(&start)
    ))
}

fn op_drag(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let seams = seams(f, wins)?;
    if seams.is_empty() {
        return Ok(());
    }
    let (_, area) = f.area()?;
    let Some(area) = area else { return Ok(()) };
    // An edge's point no other window is over (a float over the tiles:
    // pressed there, the press was the float's, a folder dragged out of a
    // Tracker into StyledEdit, a window and an alert opening mid-drag:
    // seed 16's step 54). A few tries, else no drag this step.
    let ws_now = f.workspace()?;
    let tiles_here = tiles(f, wins)?;
    let over = |x: f64, y: f64, a: u64| {
        wins.iter().any(|(&id, w)| {
            id != a
                && w.workspace == ws_now
                && !w.minimized
                && !w.behind
                && !tiles_here.contains(&id)
                && fr(w).is_some_and(|r| x >= r.x as f64 && x < (r.x + r.w) as f64 && y >= r.y as f64 && y < (r.y + r.h) as f64)
        })
    };
    let mut pick = None;
    for _ in 0..12 {
        let (a, right, (lo, hi)) = *f.rng.choice(&seams);
        let r = fr(&wins[&a]).expect("a tile");
        let along = f.rng.uniform(lo as f64 + 20.0, hi as f64 - 20.0);
        let start = if right { (r.x as f64 + r.w as f64 - 1.0, along) } else { (along, r.y as f64 + r.h as f64 - 1.0) };
        if !over(start.0, start.1, a) {
            pick = Some((a, right, along, r, start));
            break;
        }
    }
    let Some((a, right, along, _r, start)) = pick else { return Ok(()) };
    let end = if right { (f.rng.uniform(area.x as f64 + 60.0, (area.x + area.w) as f64 - 60.0), along) } else { (along, f.rng.uniform(area.y as f64 + 60.0, (area.y + area.h) as f64 - 60.0)) };
    // Past where it is let go, and back, now and then.
    let over = if f.rng.random() < 0.35 {
        let d = f.rng.uniform(-160.0, 160.0);
        Some(if right { (end.0 + d, end.1) } else { (end.0, end.1 + d) })
    } else {
        None
    };
    let steps = *f.rng.choice(&[3, 8, 20, 40, 80]);
    let step_s = *f.rng.choice(&[0.001, 0.004, 0.008, 0.016]);
    let hold = *f.rng.choice(&[0.0, 0.0, 0.03, 0.15, 0.4]);
    let side = if right { "right" } else { "bottom" };
    f.log(&format!(
        "drag {a}'s {side} edge from {} to {}{} in {steps} steps {:.0} ms apart, held {hold:.2} s",
        fmt_p(start),
        fmt_p(end),
        over.map(|o| format!(" by way of {}", fmt_p(o))).unwrap_or_default(),
        step_s * 1000.0
    ));
    let ws = f.workspace()?;
    let watch = MidDrag::begin(f, wins, ws);
    f.event(SyntheticEvent::Move { x: start.0, y: start.1 })?;
    std::thread::sleep(Duration::from_millis(30));
    f.event(SyntheticEvent::Button { button: BTN_LEFT, pressed: true })?;
    let mut at = start;
    for p in over.into_iter().chain(std::iter::once(end)) {
        for i in 1..=steps {
            std::thread::sleep(Duration::from_secs_f64(step_s));
            let k = i as f64 / steps as f64;
            f.event(SyntheticEvent::Move { x: at.0 + (p.0 - at.0) * k, y: at.1 + (p.1 - at.1) * k })?;
        }
        at = p;
    }
    std::thread::sleep(Duration::from_secs_f64(hold));
    f.event(SyntheticEvent::Button { button: BTN_LEFT, pressed: false })?;
    f.last_press = Some(Instant::now());
    let what = format!("dragging {a}'s {side} edge");
    after(f, &what, 3.0)?;
    check_mid_drag(f, &what, watch)
}

fn op_open(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if tiles(f, wins)?.len() >= MOST {
        return op_close(f, wins, shown);
    }
    let app = *f.rng.choice(&APPS);
    let args: &[&str] = f.rng.choice(launch(app));
    f.log(&format!("open {}", std::iter::once(app).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")));
    f.spawn(app, args)?;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < 4.0 && f.windows()?.keys().all(|k| wins.contains_key(k)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    after(f, &format!("opening {app}"), 3.0).map(|_| ())
}

fn op_close(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if shown.len() <= 2 {
        return Ok(());
    }
    let w = f.pick(shown.iter().copied()).expect("shown");
    f.log(&format!("close {w} ({})", wins[&w].app_id));
    f.req(Request::Close { id: w })?;
    after(f, &format!("closing {w}"), 3.0).map(|_| ())
}

fn op_workspace_trip(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    let ws = f.workspace()?;
    let to = *f.rng.choice(&(0..4).filter(|i| *i != ws).collect::<Vec<u32>>());
    f.log(&format!("workspace {ws} -> {to} -> {ws}"));
    f.req(Request::SwitchWorkspace { index: to, output: None })?;
    after(f, &format!("going to workspace {to}"), 3.0)?;
    f.req(Request::SwitchWorkspace { index: ws, output: None })?;
    after(f, &format!("coming back from workspace {to}"), 3.0).map(|_| ())
}

fn op_group(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let tiles = tiles(f, wins)?;
    if tiles.len() < 2 {
        return Ok(());
    }
    let w = f.pick(tiles.iter().copied()).expect("tiles");
    let t = f.pick(tiles.iter().copied().filter(|i| *i != w)).expect("two tiles");
    if !(wins[&w].groupable && wins[&t].groupable) {
        return Ok(());
    }
    f.log(&format!("group_window {w} ({}) with {t} ({})", wins[&w].app_id, wins[&t].app_id));
    if !matches!(f.try_req(Request::GroupWindow { id: w, with: t, index: None })?, Reply::Error { .. }) {
        f.by_hand.extend([w, t]);
        f.alone.remove(&w);
        f.alone.remove(&t);
    }
    after(f, &format!("grouping {w} with {t}"), 3.0).map(|_| ())
}

fn op_ungroup(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let groups: Vec<Vec<u64>> = Fuzz::groups(wins).into_values().filter(|g| shown.contains(&g[0])).collect();
    if groups.is_empty() {
        return Ok(());
    }
    let g = f.rng.choice(&groups).clone();
    let w = *f.rng.choice(&g);
    f.log(&format!("ungroup_window {w} (group {} {g:?})", crate::fuzz::g(wins[&w].group)));
    f.req(Request::UngroupWindow { id: w })?;
    f.alone.insert(w);
    f.by_hand.remove(&w);
    after(f, &format!("ungrouping {w}"), 3.0).map(|_| ())
}

/// Bring a window behind in its group to the front (its tab).
fn op_tab(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let behind: Vec<u64> = Fuzz::groups(wins).into_values().filter(|g| shown.contains(&g[0])).flatten().filter(|i| !wins[i].focused).collect();
    let Some(w) = f.pick(behind) else { return Ok(()) };
    f.log(&format!("activate {w} (behind in group {})", crate::fuzz::g(wins[&w].group)));
    f.req(Request::Activate { id: w })?;
    after(f, &format!("bringing {w} to the front of its group"), 3.0).map(|_| ())
}

/// Float a tile over the others, or tile a floating window again (the
/// window menu's Float Over the Tiles).
fn op_float(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let tiles: Vec<u64> = tiles(f, wins)?.into_iter().filter(|i| wins[i].group.is_none()).collect();
    // (A dialog or fixed-size window stays floating: asked, it says so.)
    let floats = floating(f, wins)?;
    let (w, on) = if !floats.is_empty() && (tiles.len() < 3 || f.rng.random() < 0.4) {
        (f.pick(floats).expect("floats"), false)
    } else if tiles.len() >= 3 {
        (f.pick(tiles).expect("tiles"), true)
    } else {
        return Ok(());
    };
    let always = wins[&w].floats_always;
    f.log(&format!("{} {w} ({}{})", if on { "float" } else { "tile" }, wins[&w].app_id, if always { ", which floats always" } else { "" }));
    f.req(Request::SetFloating { id: w, floating: Some(on) })?;
    let what = format!("{} {w}", if on { "floating" } else { "tiling" });
    let now = after(f, &what, 3.0)?;
    // A dialog or an alert stays floating, over the window it's for.
    if always && now.get(&w).is_some_and(|x| !x.floating || x.part.is_some()) {
        f.fail(&format!("{what}: it floats always (a dialog, an alert), and became a tile\n  {}", state(&now)))?;
    }
    Ok(())
}

/// Bring a floating window forward (it's the front one then), or a tile
/// (it takes the keyboard, the floats staying over it).
fn op_activate_float(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let floats = floating(f, wins)?;
    if floats.is_empty() {
        return Ok(());
    }
    let all: Vec<u64> = floats.iter().copied().chain(tiles(f, wins)?).collect();
    let w = f.pick(all).expect("some");
    let kind = if floats.contains(&w) { "floating" } else { "a tile" };
    f.log(&format!("activate {w} ({kind})"));
    f.req(Request::Activate { id: w })?;
    let now = after(f, &format!("activating {w} ({kind})"), 3.0)?;
    if !now.get(&w).is_some_and(|x| x.focused) {
        f.fail(&format!("activated window {w} ({kind}) doesn't have the keyboard"))?;
    }
    let floats_now = floating(f, &now)?;
    if floats_now.contains(&w)
        && let Some(front) = floats_now.iter().copied().min_by_key(|i| now[i].stack.unwrap_or(u32::MAX))
        && front != w
    {
        f.fail(&format!("activated floating window {w} is under floating {front}\n  {}", state(&now)))?;
    }
    Ok(())
}

fn op_reload(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    f.log("reload app_server in place");
    // (The old app_server answers a moment longer: a connection of its
    // own, read till it closes, says when it's gone.)
    let watch = std::os::unix::net::UnixStream::connect(&f.session.socket).ok();
    f.session.reload().map_err(Failure)?;
    if let Some(mut s) = watch {
        use std::io::Read as _;
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let mut buf = [0u8; 256];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
    }
    f.ipc.reset();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < 15.0 {
        if f.windows().is_ok_and(|w| w.len() >= wins.len()) {
            break;
        }
        f.ipc.reset();
        std::thread::sleep(Duration::from_millis(100));
    }
    // The windows come back with new ids: which were put in groups or
    // taken out by hand is no longer known.
    f.by_hand = f.windows()?.keys().copied().collect();
    f.alone.clear();
    after(f, "reloading", 6.0).map(|_| ())
}
