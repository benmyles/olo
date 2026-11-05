//! `olo-fuzz mode`: the windowing modes (Standard, Auto-Tiling, Focus:
//! each display's, on each workspace) with random mode changes,
//! activations (Deskbar's, an app brought forward by launching it again,
//! ⌘Tab), windows opening and closing, workspaces and displays, checking
//! after each step that each mode's rules hold:
//!
//! - the core's sanity checks (`check_sane`);
//! - on a display in Focus: of the windows that would be tiles, exactly
//!   one (with its group) shows, every other covered; the one showing is
//!   zoomed; the window with the keyboard, if it's one of them, is it;
//! - on a display in Standard or Auto-Tiling: nothing is covered;
//! - an activated window has the keyboard and shows, on its display and
//!   workspace, in front of its group, and activating moves nothing else;
//! - a Focus round trip puts every window back in its own frame.
//!
//! `--matrix` runs the same checks deterministically instead: in each
//! mode, a Tracker window behind the Terminal with the keyboard, hidden,
//! on the other display and on another workspace, brought forward each
//! way (as Deskbar does; by launching Tracker on its folder again; and
//! with a token in its environment that isn't for it, a link's opener's).
//! `--chrome` does it with Olo's Chrome's window too, on a profile of its
//! own with no network, in a session with a home of its own (`tsession
//! start … --home DIR`: the matrix copies Chrome there).

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

use olo_protocol::{Reply, Request, WindowingMode};

use crate::fuzz::{Failure, Fuzz, Showing, Step, Wins, fr, state};

const MODES: [WindowingMode; 3] = [WindowingMode::Standard, WindowingMode::AutoTiling, WindowingMode::Focus];
const TAB: u32 = 15;

fn name(m: WindowingMode) -> &'static str {
    match m {
        WindowingMode::Standard => "standard",
        WindowingMode::AutoTiling => "auto_tiling",
        WindowingMode::Focus => "focus",
    }
}

type Op = fn(&mut Fuzz, &Wins, &[u64]) -> Step;

pub const OPS: [(&str, u32, Op); 10] = [
    ("mode", 5, op_mode),
    ("focus_trip", 3, op_focus_trip),
    ("activate", 6, op_activate),
    ("relaunch", 3, op_relaunch),
    ("open", 3, op_open),
    ("close", 2, op_close),
    ("twitch", 4, op_twitch),
    ("workspace_trip", 2, op_workspace_trip),
    ("hide_show", 2, op_hide_show),
    ("to_display", 2, op_to_display),
];

pub fn step(f: &mut Fuzz) -> Step {
    let wins = f.windows()?;
    let shown = f.shown(&wins)?;
    let mut table: Vec<(u32, Op)> = OPS.iter().filter(|(n, _, _)| f.only.is_empty() || f.only.contains(*n)).map(|(_, w, op)| (*w, *op)).collect();
    if wins.len() < 4 {
        table = vec![(1, op_open)];
    }
    let op = f.rng.weighted(&table);
    op(f, &wins, &shown)
}

// ---- the modes as app_server has them ----

/// Each display's mode on the workspace it shows.
fn modes(f: &mut Fuzz) -> Result<BTreeMap<String, WindowingMode>, Failure> {
    match f.req(Request::WindowingMode { workspace: None })? {
        Reply::WindowingMode { displays, .. } => Ok(displays.into_iter().map(|d| (d.output, d.mode)).collect()),
        r => Err(Failure(format!("windowing_mode answered {r:?}"))),
    }
}

fn set_mode(f: &mut Fuzz, mode: WindowingMode, output: &str) -> Step {
    f.req(Request::SetWindowingMode { mode, workspace: None, output: Some(output.to_owned()) })?;
    Ok(())
}

/// What Focus would show one of on `output`: each window that would be a
/// tile there (a group's windows together).
fn units(wins: &Wins, output: &str, cur: &Showing) -> BTreeMap<(bool, u64), Vec<u64>> {
    let mut out: BTreeMap<(bool, u64), Vec<u64>> = BTreeMap::new();
    for (i, w) in wins {
        if w.output.as_deref() != Some(output) || w.minimized || !Fuzz::here(w, cur) || fr(w).is_none() || w.floating || w.rolled {
            continue;
        }
        let key = w.group.map_or((false, *i), |g| (true, g));
        out.entry(key).or_default().push(*i);
    }
    out
}

fn focus_state(wins: &Wins, units: &BTreeMap<(bool, u64), Vec<u64>>) -> String {
    units
        .iter()
        .map(|((g, n), ids)| {
            let each: Vec<String> = ids
                .iter()
                .map(|i| {
                    let w = &wins[i];
                    format!("{i}{}{}{}{}", if w.covered { " covered" } else { "" }, if w.behind { " behind" } else { "" }, if w.zoomed { " zoomed" } else { "" }, if w.focused { " *" } else { "" })
                })
                .collect();
            format!("{}{n}: {}", if *g { "g" } else { "w" }, each.join(","))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

// ---- checks ----

fn check_modes(f: &mut Fuzz, what: &str, wins: &Wins) -> Step {
    let cur = f.showing()?;
    for (output, mode) in modes(f)? {
        let on: Vec<u64> = wins.iter().filter(|(_, w)| w.output.as_deref() == Some(output.as_str()) && Fuzz::here(w, &cur) && !w.minimized).map(|(i, _)| *i).collect();
        if mode != WindowingMode::Focus {
            let covered: Vec<u64> = on.iter().copied().filter(|i| wins[i].covered).collect();
            if !covered.is_empty() {
                f.fail(&format!("{what}: {output} is in {}, yet {covered:?} are covered\n  {}", name(mode), state(wins)))?;
            }
            continue;
        }
        let units = units(wins, &output, &cur);
        if units.is_empty() {
            continue;
        }
        let showing: Vec<&(bool, u64)> = units.iter().filter(|(_, ids)| ids.iter().any(|i| !wins[i].covered)).map(|(k, _)| k).collect();
        if showing.len() != 1 {
            f.fail(&format!("{what}: {output} is in Focus and shows {} of its windows ({showing:?}), not one\n  {}", showing.len(), focus_state(wins, &units)))?;
            continue;
        }
        let ids = &units[showing[0]];
        let front: Vec<u64> = ids.iter().copied().filter(|i| !wins[i].behind).collect();
        if !front.iter().any(|i| wins[i].zoomed) {
            f.fail(&format!("{what}: {output} is in Focus and shows {front:?}, not zoomed\n  {}", focus_state(wins, &units)))?;
        }
        for i in on.iter().filter(|i| wins[i].focused && !wins[i].floating) {
            if !ids.contains(i) && units.values().any(|u| u.contains(i)) {
                f.fail(&format!("{what}: on {output} in Focus, {i} has the keyboard but {ids:?} shows\n  {}", focus_state(wins, &units)))?;
            }
        }
    }
    Ok(())
}

/// `w` was just brought forward: it has the keyboard, and shows.
fn check_shows(f: &mut Fuzz, what: &str, w: u64, after: &Wins) -> Step {
    let Some(a) = after.get(&w) else { return Ok(()) };
    let app = a.app_id.clone();
    if !a.focused {
        f.fail(&format!("{what}: {w} ({app}) doesn't have the keyboard\n  {}", state(after)))?;
    }
    if a.covered {
        f.fail(&format!("{what}: {w} ({app}) has been brought forward, but Focus covers it\n  {}", state(after)))?;
    }
    if a.behind {
        f.fail(&format!("{what}: {w} ({app}) is still behind in its group"))?;
    }
    let cur = f.showing()?;
    if a.minimized || !Fuzz::here(a, &cur) {
        f.fail(&format!("{what}: {w} ({app}) isn't on the workspace its display shows"))?;
    }
    Ok(())
}

fn after(f: &mut Fuzz, what: &str) -> Result<Wins, Failure> {
    let wins = f.settle()?;
    f.check_sane(&wins)?;
    check_modes(f, what, &wins)?;
    Ok(wins)
}

fn wait_for(f: &mut Fuzz, timeout: f64, cond: impl Fn(&Wins) -> bool) -> Result<bool, Failure> {
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < timeout {
        if cond(&f.windows()?) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(false)
}

// ---- steps ----

fn op_mode(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    let outputs = f.outputs.clone();
    let output = f.rng.choice(&outputs).clone();
    let mode = *f.rng.choice(&MODES);
    f.log(&format!("{output} to {}", name(mode)));
    set_mode(f, mode, &output)?;
    after(f, &format!("{output} to {}", name(mode))).map(|_| ())
}

/// Standard, Focus with a few activations, Standard again: every window
/// back in its own frame.
fn op_focus_trip(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    let outputs = f.outputs.clone();
    let output = f.rng.choice(&outputs).clone();
    f.log(&format!("a Focus round trip on {output}"));
    set_mode(f, WindowingMode::Standard, &output)?;
    let before = after(f, &format!("{output} to standard first"))?;
    let cur = f.showing()?;
    let mine: Vec<u64> = before.iter().filter(|(_, w)| w.output.as_deref() == Some(output.as_str()) && Fuzz::here(w, &cur) && !w.minimized && fr(w).is_some()).map(|(i, _)| *i).collect();
    set_mode(f, WindowingMode::Focus, &output)?;
    let mut now = after(f, &format!("{output} to focus"))?;
    for _ in 0..f.rng.int(0, 3) {
        let Some(w) = f.pick(mine.iter().copied()).filter(|w| now.contains_key(w)) else { break };
        f.log(&format!("  activate {w} ({}) in Focus", now[&w].app_id));
        f.req(Request::Activate { id: w })?;
        now = after(f, &format!("activating {w} in Focus"))?;
        check_shows(f, &format!("activating {w} in Focus"), w, &now)?;
    }
    set_mode(f, WindowingMode::Standard, &output)?;
    let back = after(f, &format!("{output} back to standard"))?;
    for i in mine {
        let Some(a) = back.get(&i) else { continue };
        if !before[&i].zoomed && fr(a) != fr(&before[&i]) && a.workspace == before[&i].workspace {
            f.fail(&format!("Focus round trip on {output}: {i} ({}) was {}, now {}", before[&i].app_id, crate::fuzz::opt(fr(&before[&i])), crate::fuzz::opt(fr(a))))?;
        }
    }
    Ok(())
}

fn op_activate(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let Some(w) = f.pick(wins.keys().copied()) else { return Ok(()) };
    let x = &wins[&w];
    f.log(&format!("activate {w} ({}{}, ws {}, {})", x.app_id, if x.minimized { ", hidden" } else { "" }, x.workspace, x.output.as_deref().unwrap_or("None")));
    let cur = f.showing()?;
    f.req(Request::Activate { id: w })?;
    let now = after(f, &format!("activating {w}"))?;
    if Fuzz::here(x, &cur) {
        let mut skip = Fuzz::mates(&now, w);
        skip.insert(w);
        f.check_unchanged(&format!("activating {w}"), wins, &now, &skip)?;
    }
    check_shows(f, &format!("activating {w}"), w, &now)
}

/// An app launched again with its window open brings that window forward
/// itself (xdg-activation, from the app). Tracker shows a folder in a
/// window on the workspace in use: one it has there comes forward, else it
/// opens one there.
fn op_relaunch(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let known = [("/", "/"), ("usr", "/usr"), ("etc", "/etc"), ("tmp", "/tmp"), ("share", "/usr/share"), ("var", "/var")];
    let trackers: Vec<u64> = wins.iter().filter(|(_, w)| w.app_id == "olo.tracker" && !w.title.is_empty()).map(|(i, _)| *i).collect();
    let pick: Vec<(u64, &str)> = trackers.iter().filter_map(|i| known.iter().find(|(t, _)| *t == wins[i].title).map(|(_, folder)| (*i, *folder))).collect();
    if pick.is_empty() {
        return op_open(f, wins, shown);
    }
    let (w, folder) = *f.rng.choice(&pick);
    let title = wins[&w].title.clone();
    let active = wins.values().find(|x| x.focused).and_then(|x| x.output.clone());
    let cur = f.showing()?;
    let here: Vec<u64> = trackers.iter().copied().filter(|i| wins[i].title == title && wins[i].output == active && Fuzz::here(&wins[i], &cur)).collect();
    f.log(&format!("olo-tracker {folder} again ({w} has it{})", if here.is_empty() && active.is_some() { ", on another workspace" } else { "" }));
    f.spawn("olo-tracker", &[folder])?;
    let what = format!("launching Tracker on {folder} again");
    let t = title.clone();
    wait_for(f, 4.0, |now| now.values().any(|x| x.focused && x.title == t))?;
    let now = after(f, &what)?;
    let mine: Vec<u64> = now.iter().filter(|(_, x)| x.focused && x.app_id == "olo.tracker" && x.title == title).map(|(i, _)| *i).collect();
    let Some(&w) = mine.first() else {
        return check_shows(f, &what, *here.first().unwrap_or(&w), &now);
    };
    if !here.is_empty() && !here.contains(&w) {
        f.fail(&format!("{what}: Tracker opened {w} for it, though {here:?} shows it on the workspace in use\n  {}", state(&now)))?;
    }
    if here.is_empty() && active.is_some() && now[&w].output != active {
        f.fail(&format!("{what}: Tracker's window for it, {w}, is on {:?}, not {active:?}, the display in use", now[&w].output))?;
    }
    check_shows(f, &what, w, &now)
}

fn op_twitch(f: &mut Fuzz, _: &Wins, shown: &[u64]) -> Step {
    if shown.len() < 2 {
        return Ok(());
    }
    let n = f.rng.int(1, 3);
    f.log(&format!("⌘Tab ×{n}"));
    crate::wm::twitch(f, TAB, n, false)?;
    let now = after(f, &format!("⌘Tab ×{n}"))?;
    if let Some(w) = now.iter().find(|(_, w)| w.focused).map(|(i, _)| *i) {
        check_shows(f, &format!("⌘Tab ×{n}"), w, &now)?;
    }
    Ok(())
}

fn op_open(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let app = *f.rng.choice(&crate::wm::APPS);
    let args: &[&str] = f.rng.choice(crate::wm::launch(app));
    f.log(&format!("open {}", std::iter::once(app).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")));
    f.spawn(app, args)?;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < 4.0 && f.windows()?.keys().all(|k| wins.contains_key(k)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let now = after(f, &format!("opening {app}"))?;
    let new: Vec<u64> = now.keys().copied().filter(|k| !wins.contains_key(k)).collect();
    for n in &new {
        check_shows(f, &format!("opening {app}"), *n, &now)?;
    }
    if new.is_empty()
        && let Some(w) = now.iter().find(|(_, w)| w.focused).map(|(i, _)| *i)
    {
        check_shows(f, &format!("launching {app} again"), w, &now)?;
    }
    Ok(())
}

fn op_close(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if wins.len() <= 4 {
        return Ok(());
    }
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("close {w} ({})", wins[&w].app_id));
    f.req(Request::Close { id: w })?;
    after(f, &format!("closing {w}")).map(|_| ())
}

fn op_workspace_trip(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    f.log("to workspace 2 and back");
    f.shortcut("workspace-2", None)?;
    after(f, "to workspace 2")?;
    f.shortcut("workspace-1", None)?;
    after(f, "back to workspace 1").map(|_| ())
}

fn op_hide_show(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("hide {w} ({}), then bring it back", wins[&w].app_id));
    f.shortcut("hide-window", Some(w))?;
    after(f, &format!("hiding {w}"))?;
    f.req(Request::Activate { id: w })?;
    let now = after(f, &format!("bringing {w} back"))?;
    check_shows(f, &format!("bringing {w} back"), w, &now)
}

fn op_to_display(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if f.outputs.len() < 2 {
        return Ok(());
    }
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("{w} ({}) to the next display", wins[&w].app_id));
    f.shortcut("next-display", Some(w))?;
    after(f, &format!("{w} to the next display")).map(|_| ())
}

// ---- the matrix: each mode, each way, each place ----

/// Where the window brought forward is, seen from the one with the
/// keyboard (a Terminal on the first display).
const PLACES: [&str; 4] = ["behind", "hidden", "other display", "other workspace"];
/// How: as Deskbar does (no token), its app launched again (a token it
/// asks for), and with a token in its environment that isn't for it.
const WAYS: [&str; 3] = ["deskbar", "relaunch", "opener's token"];

/// What brings a window forward in a case: Tracker on a folder, or
/// Olo's Chrome on a profile of its own.
#[derive(Clone)]
enum Target {
    Tracker(String),
    Chrome { args: Vec<String>, app_id: String },
}

fn open_one(f: &mut Fuzz, app: &str, args: &[&str], timeout: f64) -> Result<u64, Failure> {
    let before: HashSet<u64> = f.windows()?.keys().copied().collect();
    f.spawn(app, args)?;
    wait_for(f, timeout, |w| w.keys().any(|k| !before.contains(k)))?;
    let now = after(f, &format!("opening {app}"))?;
    now.keys().copied().filter(|k| !before.contains(k)).max().ok_or_else(|| Failure(format!("{app} {} opened no window", args.join(" "))))
}

/// `w` where `place` says.
fn put(f: &mut Fuzz, w: u64, place: &str, a: &str, b: Option<&str>) -> Step {
    let Some(mut i) = f.windows()?.remove(&w) else { return Err(Failure(format!("{w} has gone"))) };
    let (out, ws) = match place {
        "other display" => (b.unwrap_or(a).to_owned(), 0),
        "other workspace" => (a.to_owned(), 1),
        _ => (a.to_owned(), 0),
    };
    if i.minimized && place != "hidden" {
        f.req(Request::Activate { id: w })?;
        i = after(f, &format!("bringing {w} back"))?.remove(&w).ok_or_else(|| Failure(format!("{w} has gone")))?;
    }
    if i.output.as_deref() != Some(out.as_str()) || i.workspace != ws {
        f.req(Request::MoveToWorkspace { id: w, index: ws, output: Some(out.clone()) })?;
        i = after(f, &format!("{w} to workspace {} of {out}", ws + 1))?.remove(&w).ok_or_else(|| Failure(format!("{w} has gone")))?;
    }
    if place == "hidden" && !i.minimized {
        f.shortcut("hide-window", Some(w))?;
        after(f, &format!("hiding {w}"))?;
    }
    Ok(())
}

/// Nothing on `w`'s display is over it (but what floats over everything,
/// its own dialogs, and its group, which it leads).
fn check_in_front(f: &mut Fuzz, what: &str, w: u64, now: &Wins) -> Step {
    let Some(a) = now.get(&w) else { return Ok(()) };
    let Some(stack) = a.stack else { return Ok(()) };
    let mates = Fuzz::mates(now, w);
    let over: Vec<u64> =
        now.iter().filter(|(i, x)| **i != w && x.output == a.output && x.stack.is_some_and(|s| s < stack) && !x.floating && !x.covered && !x.behind && !mates.contains(i)).map(|(i, _)| *i).collect();
    if !over.is_empty() {
        return f.fail(&format!("{what}: {w} ({}) has the keyboard, but {over:?} are over it\n  {}", a.app_id, state(now)));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn case(f: &mut Fuzz, what: &str, mode: WindowingMode, place: &str, way: &str, w: u64, front: u64, target: &Target, a: &str, b: Option<&str>) -> Step {
    put(f, w, place, a, b)?;
    f.req(Request::Activate { id: front })?;
    let before = after(f, &format!("{what}: the Terminal forward"))?;
    let env: Vec<(&str, &str)> = if way == "opener's token" { vec![("XDG_ACTIVATION_TOKEN", "from-the-opener")] } else { vec![] };
    if way == "deskbar" {
        f.req(Request::Activate { id: w })?;
    } else {
        match target {
            Target::Chrome { args, .. } => {
                let page = format!("data:text/html,{}", f.step);
                let mut all: Vec<&str> = args.iter().map(String::as_str).collect();
                all.push(&page);
                f.session.spawn_with("olo-chrome", &all, &env).map_err(Failure)?;
            }
            Target::Tracker(folder) => f.session.spawn_with("olo-tracker", &[folder], &env).map_err(Failure)?,
        }
    }
    let title = match target {
        Target::Tracker(folder) => Path::new(folder).file_name().map(|n| n.to_string_lossy().into_owned()),
        Target::Chrome { .. } => None,
    };
    let chrome_id = match target {
        Target::Chrome { app_id, .. } => Some(app_id.clone()),
        Target::Tracker(_) => None,
    };
    let (t, c) = (title.clone(), chrome_id.clone());
    wait_for(f, 4.0, move |wins| wins.values().any(|x| x.focused && if let Some(c) = &c { x.app_id == *c } else { Some(&x.title) == t.as_ref() }))?;
    let now = after(f, what)?;
    // Tracker brings forward its window for the folder on the workspace in
    // use, else opens one there.
    let new = chrome_id.is_none() && matches!(place, "other display" | "other workspace") && way != "deskbar";
    let mut w = w;
    if new {
        let opened: Vec<u64> = now.iter().filter(|(i, x)| !before.contains_key(i) && Some(&x.title) == title.as_ref()).map(|(i, _)| *i).collect();
        let Some(&o) = opened.first() else {
            return f.fail(&format!("{what}: Tracker brought {w} forward, on {place}, not a window here for it\n  {}", state(&now)));
        };
        if now[&o].output.as_deref() != Some(a) {
            f.fail(&format!("{what}: Tracker's new window opened on {:?}, not {a}, the display in use", now[&o].output))?;
        }
        w = o;
    }
    check_shows(f, what, w, &now)?;
    check_in_front(f, what, w, &now)?;
    // Nothing else moves (but auto-tiling's tiles, for a window back in
    // one, or a new one).
    if !new && !(mode == WindowingMode::AutoTiling && place == "hidden") {
        let mut skip = Fuzz::mates(&now, w);
        skip.insert(w);
        f.check_unchanged(what, &before, &now, &skip)?;
    }
    Ok(())
}

/// Olo's Chrome for the matrix: in the session's own home (a copy of the
/// person's install, kept from updating itself), on a profile of its own,
/// with no network. Why not, if it can't be: a session on the person's
/// own home would have the matrix touch their Chrome's install.
fn set_up_chrome(f: &Fuzz, root: &Path) -> Result<Vec<String>, String> {
    let real = std::env::var("HOME").map_err(|_| "no HOME")?;
    let home = f.session.env.get("HOME").filter(|h| Path::new(h) != Path::new(&real)).ok_or("the session is on your own home: start it with --home DIR")?;
    let dest = Path::new(home).join(".local/share/olo/chrome");
    if !dest.join("current/google-chrome").exists() {
        let src = std::fs::canonicalize(Path::new(&real).join(".local/share/olo/chrome/current")).map_err(|_| "no Chrome installed")?;
        if !src.join("google-chrome").exists() {
            return Err("no Chrome installed".into());
        }
        let name = src.file_name().ok_or("no Chrome installed")?.to_owned();
        copy_tree(&src, &dest.join(&name)).map_err(|e| format!("copying Chrome: {e}"))?;
        let _ = std::os::unix::fs::symlink(&name, dest.join("current"));
    }
    let _ = std::fs::write(dest.join(".checked"), "");
    let profile = root.join("chrome");
    Ok([
        format!("--user-data-dir={}", profile.display()),
        "--proxy-server=127.0.0.1:9".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--password-store=basic".into(),
        "--disable-sync".into(),
    ]
    .to_vec())
}

/// What the matrix leaves, cleared however it ends: its Chrome stopped,
/// its folders removed.
struct Leftovers {
    root: std::path::PathBuf,
    chrome: Option<String>,
}

impl Drop for Leftovers {
    fn drop(&mut self) {
        if let Some(profile) = self.chrome.take() {
            stop_chrome(&profile);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `src` copied to `dest`, files reflinked where the filesystem can.
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(src)?, dest);
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
        return std::fs::set_permissions(dest, meta.permissions());
    }
    let from = std::fs::File::open(src)?;
    let to = std::fs::File::create(dest)?;
    // SAFETY: FICLONE between two files of ours; on failure, copied.
    if unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) } != 0 {
        std::io::copy(&mut &from, &mut &to)?;
    }
    std::fs::set_permissions(dest, meta.permissions())
}

/// Chrome's processes on `profile` (the matrix's own, never another
/// Chrome) gone: a moment to quit after its window closed, then told to,
/// then killed.
fn stop_chrome(profile_arg: &str) {
    for n in 0..70 {
        let pids: Vec<i32> = std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|b| *b == 0).any(|a| a == profile_arg.as_bytes())))
            .collect();
        if pids.is_empty() {
            return;
        }
        if n == 20 || n == 50 {
            let sig = if n == 20 { libc::SIGTERM } else { libc::SIGKILL };
            for pid in pids {
                // SAFETY: a signal to a Chrome of the matrix's own profile.
                unsafe { libc::kill(pid, sig) };
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every mode, on every display, by every way, a window in every place
/// brought forward: it has the keyboard, shows, is in front, and nothing
/// else moves. Prints the table; failures as the fuzz's.
pub fn matrix(f: &mut Fuzz, with_chrome: bool) -> i32 {
    f.keep_going = true;
    match matrix_run(f, with_chrome) {
        Ok(code) => code,
        Err(Failure(e)) => {
            f.say(&format!("FAIL {e}"));
            1
        }
    }
}

fn matrix_run(f: &mut Fuzz, with_chrome: bool) -> Result<i32, Failure> {
    let root = std::path::PathBuf::from(format!("/var/tmp/olo-mode-matrix-{}", f.session.name));
    let mut leftovers = Leftovers { root: root.clone(), chrome: None };
    let folders: BTreeMap<&str, String> = PLACES.iter().map(|p| (*p, root.join(p.replace(' ', "-")).display().to_string())).collect();
    for dir in folders.values() {
        let _ = std::fs::create_dir_all(dir);
    }
    let name_ = f.session.name.clone();
    f.say(&format!("mode-fuzz {name_}: the matrix"));
    for o in f.outputs.clone() {
        set_mode(f, WindowingMode::Standard, &o)?;
    }
    after(f, "every display in Standard")?;
    // (A Terminal left by a run before, if one was: it may not close.)
    let had: Vec<u64> = f.windows()?.iter().filter(|(_, x)| x.app_id == "olo.terminal").map(|(i, _)| *i).collect();
    let front = match had.first() {
        Some(&t) => t,
        None => open_one(f, "olo-terminal", &[], 8.0)?,
    };
    let a = f.windows()?.get(&front).and_then(|x| x.output.clone()).unwrap_or_default();
    let b = f.outputs.iter().find(|o| **o != a).cloned();
    let places: Vec<&str> = PLACES.iter().copied().filter(|p| b.is_some() || *p != "other display").collect();
    let mut targets: BTreeMap<(&str, &str), (u64, Target)> = BTreeMap::new();
    for p in &places {
        f.req(Request::Activate { id: front })?;
        after(f, "the Terminal forward")?;
        let folder = folders[p].clone();
        let title = Path::new(&folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let had: Vec<u64> = f.windows()?.iter().filter(|(_, x)| x.app_id == "olo.tracker" && x.title == title).map(|(i, _)| *i).collect();
        let w = match had.first() {
            Some(&w) => w,
            None => open_one(f, "olo-tracker", &[&folder], 8.0)?,
        };
        targets.insert(("tracker", p), (w, Target::Tracker(folder)));
    }
    let chrome = match with_chrome.then(|| set_up_chrome(f, &root)) {
        Some(Ok(args)) => Some(args),
        Some(Err(why)) => {
            f.say(&format!("(no Chrome: {why}; the matrix goes on without it)"));
            None
        }
        None => None,
    };
    leftovers.chrome = chrome.as_ref().map(|a| a[0].clone());
    if let Some(args) = &chrome {
        f.req(Request::Activate { id: front })?;
        after(f, "the Terminal forward")?;
        // (A new profile's first window takes Chrome most of a minute.)
        let had: Vec<u64> = f.windows()?.iter().filter(|(_, x)| x.app_id.starts_with("google-chrome")).map(|(i, _)| *i).collect();
        let mut all: Vec<&str> = args.iter().map(String::as_str).collect();
        all.push("about:blank");
        let w = match had.first() {
            Some(&w) => w,
            None => open_one(f, "olo-chrome", &all, 120.0)?,
        };
        let app_id = f.windows()?.get(&w).map(|x| x.app_id.clone()).unwrap_or_default();
        for p in &places {
            targets.insert(("chrome", p), (w, Target::Chrome { args: args.clone(), app_id: app_id.clone() }));
        }
    }
    let apps: Vec<&str> = if chrome.is_some() { vec!["tracker", "chrome"] } else { vec!["tracker"] };
    let keep: HashSet<u64> = f.windows()?.keys().copied().collect();
    let mut results: BTreeMap<(String, String, String, String), bool> = BTreeMap::new();
    for mode in MODES {
        for o in f.outputs.clone() {
            set_mode(f, mode, &o)?;
        }
        after(f, &format!("every display in {}", name(mode)))?;
        for app in &apps {
            for way in WAYS {
                for p in &places {
                    f.step += 1;
                    let what = format!("{app}, {}, {p}, {way}", name(mode));
                    if !f.only.iter().all(|o| what.contains(o.as_str())) {
                        continue;
                    }
                    f.log(&what);
                    let n = f.failures.len();
                    let (w, target) = targets[&(*app, *p)].clone();
                    if let Err(Failure(e)) = case(f, &what, mode, p, way, w, front, &target, &a, b.as_deref()) {
                        f.failures.push(e);
                    }
                    results.insert((app.to_string(), name(mode).to_owned(), way.to_owned(), p.to_string()), f.failures.len() == n);
                    // (Tracker's new windows closed: each case starts alike.)
                    for i in f.windows()?.into_keys().filter(|i| !keep.contains(i)) {
                        let _ = f.try_req(Request::Close { id: i });
                    }
                    let k = keep.clone();
                    wait_for(f, 4.0, move |wins| wins.keys().all(|i| k.contains(i)))?;
                }
            }
        }
    }
    for o in f.outputs.clone() {
        set_mode(f, WindowingMode::Standard, &o)?;
    }
    let close: HashSet<u64> = targets.values().map(|(w, _)| *w).chain(std::iter::once(front)).collect();
    for i in close {
        let _ = f.try_req(Request::Close { id: i });
    }
    let width = places.iter().map(|p| p.len()).max().unwrap_or(8) + 2;
    let mut table = format!("\n{:38}{}", "", places.iter().map(|p| format!("{p:width$}")).collect::<String>());
    for app in &apps {
        for mode in MODES {
            for way in WAYS {
                let cells: Vec<String> = places
                    .iter()
                    .map(|p| match results.get(&(app.to_string(), name(mode).to_owned(), way.to_owned(), p.to_string())) {
                        Some(true) => "ok".into(),
                        Some(false) => "FAIL".into(),
                        None => "-".into(),
                    })
                    .collect();
                if cells.iter().any(|c| c != "-") {
                    table += &format!("\n{app:8}{:14}{way:16}{}", name(mode), cells.iter().map(|c| format!("{c:width$}")).collect::<String>());
                }
            }
        }
    }
    f.say(&table);
    drop(leftovers);
    for fail in f.failures.clone() {
        f.say(&format!("\nFAIL {fail}"));
    }
    let n = f.failures.len();
    f.say(&if n > 0 { format!("\n{n} failures") } else { format!("\nok: {} cases", results.len()) });
    Ok(i32::from(n > 0))
}
