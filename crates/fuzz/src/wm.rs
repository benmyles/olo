//! `olo-fuzz wm`: drive app_server with random combinations of every
//! arranging command, shortcut, switcher, workspace and display change,
//! on real apps (Terminal, StyledEdit, Tracker), checking after each step
//! that the desktop is still sane (fuzz.rs, `check_sane`) and that what
//! shouldn't move didn't. See docs/HACKING.md, "The fuzzers".

use std::collections::HashSet;

use olo_protocol::{Frame, OutputConfig, Reply, Request, SyntheticEvent, ZoomAxis};

use crate::fuzz::{Fuzz, R, Step, Wins, contains, fmt_p, fr, fr_of, inside, opt, overlap, tab_rect, tab_slot};

pub const APPS: [&str; 3] = ["olo-terminal", "olo-styled-edit", "olo-tracker"];
/// How to ask each for a window: StyledEdit and Tracker bring theirs
/// forward when launched bare (a folder already open, too), as BeOS apps do.
pub fn launch(app: &str) -> &'static [&'static [&'static str]] {
    match app {
        "olo-terminal" => &[&[]],
        "olo-styled-edit" => &[&["--new-window"], &[]],
        _ => &[&["/"], &["/usr"], &["/etc"], &["/tmp"], &["/usr/share"], &["/var"], &[]],
    }
}

pub const TILES: [&str; 13] = [
    "tile-left",
    "tile-right",
    "tile-top",
    "tile-bottom",
    "tile-top-left",
    "tile-top-right",
    "tile-bottom-left",
    "tile-bottom-right",
    "tile-left-third",
    "tile-center-third",
    "tile-right-third",
    "tile-left-two-thirds",
    "tile-right-two-thirds",
];
const WINDOW_EXTRA: [&str; 11] = ["zoom", "zoom-vertical", "zoom-horizontal", "restore", "center", "hide-window", "roll-up", "next-tab", "previous-tab", "next-display", "previous-display"];
const ARRANGE_CMDS: [&str; 8] = ["tile-all", "zoom-all", "cascade-all", "fan-out", "cascade-others", "fan-out-others", "undo-arrangement", "show-desktop"];
const WORKSPACE_CMDS: [&str; 10] = [
    "next-workspace",
    "previous-workspace",
    "window-to-next-workspace",
    "window-to-previous-workspace",
    "workspace-1",
    "workspace-2",
    "workspace-3",
    "window-to-workspace-1",
    "window-to-workspace-2",
    "window-to-workspace-3",
];

// Key chords, pressed as a person would (they act on the window with the
// keyboard, so focus bugs show).
pub const CTRL: u32 = 29;
pub const SHIFT: u32 = 42;
pub const ALT: u32 = 56;
pub const CMD: u32 = 125;
pub const BTN_LEFT: u32 = 0x110;
// The window decoration's measures (olo_theme::deco) and ⌘-dragging's
// (app_server's shell/tabdrag.rs).
pub const TAB_H: i32 = 21;
const BOX_INSET: i32 = 5;
const BOX_SIZE: i32 = 13;
const NEAR: i32 = 12;
/// A tab at least this wide has its middle clear of its boxes.
const TAB_GRIP: i32 = 2 * (BOX_INSET + BOX_SIZE + 2) + 8;
/// A Mac's keys (the default keyboard profile, olo_protocol::shortcuts).
const KEYS: [(&str, &[u32]); 23] = [
    ("cascade-all", &[CTRL, CMD, 43]),
    ("center", &[CTRL, ALT, 46]),
    ("fan-out", &[CTRL, CMD, 53]),
    ("hide-window", &[CMD, 50]),
    ("next-tab", &[CTRL, 15]),
    ("next-workspace", &[CTRL, 106]),
    ("previous-workspace", &[CTRL, 105]),
    ("restore", &[CTRL, ALT, 14]),
    ("roll-up", &[CTRL, CMD, 50]),
    ("tile-all", &[CTRL, CMD, 28]),
    ("tile-bottom", &[CTRL, ALT, 108]),
    ("tile-bottom-right", &[CTRL, ALT, 37]),
    ("tile-center-third", &[CTRL, ALT, 33]),
    ("tile-left", &[CTRL, ALT, 105]),
    ("tile-left-third", &[CTRL, ALT, 32]),
    ("tile-right", &[CTRL, ALT, 106]),
    ("tile-top", &[CTRL, ALT, 103]),
    ("tile-top-left", &[CTRL, ALT, 22]),
    ("undo-arrangement", &[CTRL, CMD, 44]),
    ("window-to-next-workspace", &[CTRL, SHIFT, 106]),
    ("zoom", &[CTRL, ALT, 28]),
    ("zoom-all", &[CTRL, ALT, CMD, 28]),
    ("zoom-horizontal", &[CTRL, ALT, SHIFT, 105]),
];

type Op = fn(&mut Fuzz, &Wins, &[u64]) -> Step;

/// Every step, by name (as `--only` takes them) and weight.
pub const OPS: [(&str, u32, Op); 26] = [
    ("shortcut", 12, op_shortcut),
    ("keys", 6, op_keys),
    ("arrange", 5, op_arrange),
    ("activate", 4, op_activate),
    ("twitch", 5, op_twitch),
    ("twitch_windows", 2, op_twitch_windows),
    ("send_to_back", 3, op_send_to_back),
    ("hide_show", 3, op_hide_show),
    ("open", 3, op_open),
    ("close", 1, op_close),
    ("workspace_trip", 3, op_workspace_trip),
    ("workspace_cmd", 2, op_workspace_cmd),
    ("zoom_trip", 4, op_zoom_trip),
    ("tile_trip", 5, op_tile_trip),
    ("arrange_trip", 3, op_arrange_trip),
    ("window_ws_trip", 2, op_window_ws_trip),
    ("display_trip", 2, op_display_trip),
    ("displays", 1, op_displays),
    ("place", 2, op_place),
    ("tab_along", 4, op_tab_along),
    ("tab_out", 3, op_tab_out),
    ("tab_in", 4, op_tab_in),
    ("group_window", 2, op_group_window),
    ("ungroup_window", 2, op_ungroup_window),
    ("move_tab", 2, op_move_tab),
    ("app_resize", 4, op_app_resize),
];

pub fn step(f: &mut Fuzz) -> Step {
    let wins = f.windows()?;
    let shown = f.shown(&wins)?;
    let mut table: Vec<(u32, Op)> = OPS.iter().filter(|(n, _, _)| f.only.is_empty() || f.only.contains(*n)).map(|(_, w, op)| (*w, *op)).collect();
    if wins.len() < 3 {
        table = vec![(1, op_open)];
    }
    let op = f.rng.weighted(&table);
    op(f, &wins, &shown)
}

fn set(ids: impl IntoIterator<Item = u64>) -> HashSet<u64> {
    ids.into_iter().collect()
}

fn op_shortcut(f: &mut Fuzz, _: &Wins, shown: &[u64]) -> Step {
    let all: Vec<&str> = TILES.iter().chain(WINDOW_EXTRA.iter()).copied().collect();
    let sid = *f.rng.choice(&all);
    let w = if f.rng.random() < 0.8 { f.pick(shown.iter().copied()) } else { None };
    f.log(&format!("shortcut {sid} on {w:?}"));
    f.shortcut(sid, w)?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    if let Some(w) = w
        && TILES.contains(&sid)
        && after.get(&w).is_some_and(|x| !x.minimized)
    {
        f.check_inside_area(sid, w, &after)?;
    }
    Ok(())
}

fn op_keys(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    let (sid, codes) = *f.rng.choice(&KEYS);
    f.log(&format!("keys {sid}"));
    f.keys(codes)?;
    let after = f.settle()?;
    f.check_sane(&after)
}

fn op_arrange(f: &mut Fuzz, _: &Wins, shown: &[u64]) -> Step {
    let sid = *f.rng.choice(&ARRANGE_CMDS);
    let w = f.pick(shown.iter().copied());
    f.log(&format!("arrange {sid} (with {w:?})"));
    if let Some(w) = w {
        f.req(Request::Activate { id: w })?;
    }
    f.shortcut(sid, w)?;
    let after = f.settle()?;
    f.check_sane(&after)
}

fn op_activate(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let Some(w) = f.pick(wins.keys().copied()) else { return Ok(()) };
    let x = &wins[&w];
    f.log(&format!("activate {w} ({}{}, ws {})", x.app_id, if x.minimized { ", hidden" } else { "" }, x.workspace));
    let cur = f.showing()?;
    f.req(Request::Activate { id: w })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    if Fuzz::here(x, &cur) {
        f.check_unchanged(&format!("activating {w}"), wins, &after, &set([w]))?;
    }
    if !after.get(&w).is_some_and(|a| a.focused) {
        f.fail(&format!("activated window {w} doesn't have the keyboard"))?;
    }
    Ok(())
}

pub fn twitch(f: &mut Fuzz, key: u32, n: i64, back: bool) -> Step {
    let mods: Vec<u32> = if back { vec![CMD, SHIFT] } else { vec![CMD] };
    let mut ev: Vec<SyntheticEvent> = mods.iter().map(|&k| SyntheticEvent::Key { key: k, pressed: true }).collect();
    for _ in 0..n {
        ev.push(SyntheticEvent::Key { key, pressed: true });
        ev.push(SyntheticEvent::Key { key, pressed: false });
        if f.rng.random() < 0.3 {
            // Wander with the arrows too.
            let arrow = *f.rng.choice(&[103, 105, 106, 108]);
            ev.push(SyntheticEvent::Key { key: arrow, pressed: true });
            ev.push(SyntheticEvent::Key { key: arrow, pressed: false });
        }
    }
    ev.extend(mods.iter().rev().map(|&k| SyntheticEvent::Key { key: k, pressed: false }));
    if f.rng.random() < 0.5 {
        // One key at a time, with the loop turning between them, as typed.
        for e in ev {
            f.event(e)?;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    } else {
        f.req(Request::Synthetic { events: ev })?;
    }
    Ok(())
}

fn check_twitch(f: &mut Fuzz, what: &str, wins: &Wins, after: &Wins) -> Step {
    f.check_sane(after)?;
    // The chosen window: the focused one now. Every other one is as it
    // was: where it was, hidden if it was, on its workspace.
    let chosen: Vec<u64> = after.iter().filter(|(_, w)| w.focused).map(|(i, _)| *i).collect();
    for (i, w) in wins {
        if chosen.contains(i) {
            continue;
        }
        let Some(a) = after.get(i) else { continue };
        if a.minimized != w.minimized {
            f.fail(&format!("{what}: window {i} ({}) was {} and is now {}", w.app_id, if w.minimized { "hidden" } else { "shown" }, if a.minimized { "hidden" } else { "shown" }))?;
        }
        if a.workspace != w.workspace {
            f.fail(&format!("{what}: window {i} moved from workspace {} to {}", w.workspace, a.workspace))?;
        }
    }
    // One brought back from hidden into its group makes the group as big
    // as it can be at the least, where it is.
    let mut grown = HashSet::new();
    for c in &chosen {
        if wins.get(c).is_some_and(|w| w.minimized || fr(w).is_none()) {
            grown.extend(Fuzz::grown(*c, &Fuzz::group_of(after, *c), wins, after));
        }
    }
    let moved: Vec<String> = wins
        .iter()
        .filter(|(i, _)| !chosen.contains(i) && !grown.contains(i))
        .filter_map(|(i, w)| {
            let (a, b) = (fr(w)?, fr_of(after, *i)?);
            (a != b).then(|| format!("{i} ({}, ws {}) {a} -> {b}", w.app_id, w.workspace))
        })
        .collect();
    if !moved.is_empty() {
        return f.fail(&format!("{what} (chose {chosen:?}) moved windows: {}\n  before: {}\n  after:  {}", moved.join(", "), crate::fuzz::state(wins), crate::fuzz::state(after)));
    }
    let skip: HashSet<u64> = chosen.into_iter().chain(grown).collect();
    f.check_steady(what, wins, &skip)
}

fn op_twitch(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let (n, back) = (f.rng.int(1, 5), f.rng.random() < 0.25);
    f.log(&format!("cmd-tab x{n}{}", if back { " back" } else { "" }));
    twitch(f, 15, n, back)?;
    let after = f.settle()?;
    check_twitch(f, &format!("Cmd+Tab x{n}"), wins, &after)
}

fn op_twitch_windows(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let n = f.rng.int(1, 3);
    f.log(&format!("cmd-` x{n}"));
    twitch(f, 41, n, false)?;
    let after = f.settle()?;
    check_twitch(f, &format!("Cmd+` x{n}"), wins, &after)
}

fn op_send_to_back(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("send {w} to back"));
    f.req(Request::SendToBack { id: w })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_unchanged(&format!("sending {w} to back"), wins, &after, &HashSet::new())
}

fn op_hide_show(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("hide {w} ({}) and show it again", wins[&w].app_id));
    f.req(Request::Minimize { id: w })?;
    let mid = f.settle()?;
    f.check_sane(&mid)?;
    let mates = Fuzz::mates(wins, w);
    f.check_unchanged(&format!("hiding {w}"), wins, &mid, &mates)?;
    f.req(Request::Activate { id: w })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_unchanged(&format!("hiding and showing {w}"), wins, &after, &mates)?;
    // It comes back where it was, unless it was in a group (which it goes
    // back to, wherever that is now).
    if wins[&w].group.is_none() {
        f.check_back(&format!("hiding and showing {w}"), w, wins, &after)?;
    }
    Ok(())
}

fn op_open(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let mut app = *f.rng.choice(&APPS);
    let mut args: &[&str] = f.rng.choice(launch(app));
    // Now and then with a button held down in a Terminal (selecting): the
    // pointer is grabbed meanwhile, and the new window must still take its
    // group's size, not the group the new window's.
    let terms: Vec<u64> = wins.values().filter(|w| w.focused && w.app_id == "olo.terminal" && w.content.is_some()).map(|w| w.id).collect();
    let mut wins = wins.clone();
    let mut held = !terms.is_empty() && f.rng.random() < 0.3;
    if held {
        // Another Terminal, into a group sized by hand.
        (app, args) = ("olo-terminal", &[]);
        let r = fr_of(&wins, terms[0]).expect("a frame");
        let (w, h) = if r.w > 500 && r.h > 300 { (r.w - 37, r.h - 23) } else { (r.w + 37, r.h + 23) };
        f.req(Request::Place { id: terms[0], frame: Frame { x: r.x, y: r.y, width: w, height: h } })?;
        wins = f.settle()?;
        held = wins.get(&terms[0]).is_some_and(|t| t.content.is_some());
    }
    f.log(&format!("open {}{}", std::iter::once(app).chain(args.iter().copied()).collect::<Vec<_>>().join(" "), if held { " (a button held down)" } else { "" }));
    if held {
        let c = crate::fuzz::rect(wins[&terms[0]].content.as_ref().expect("content"));
        f.req(Request::Synthetic {
            events: vec![SyntheticEvent::Move { x: c.x as f64 + c.w as f64 / 2.0, y: c.y as f64 + c.h as f64 / 2.0 }, SyntheticEvent::Button { button: BTN_LEFT, pressed: true }],
        })?;
    }
    f.spawn(app, args)?;
    let t0 = std::time::Instant::now();
    let mut seen: Vec<Wins> = Vec::new();
    while t0.elapsed().as_secs_f64() < 4.0 {
        let now = f.windows()?;
        if seen.last() != Some(&now) {
            seen.push(now.clone());
        }
        if now.keys().any(|k| !wins.contains_key(k)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    if held {
        f.settle()?;
        seen.extend(f.trail.clone());
        f.event(SyntheticEvent::Button { button: BTN_LEFT, pressed: false })?;
        f.last_press = Some(std::time::Instant::now());
    }
    let after = f.settle()?;
    let mut trail = seen;
    trail.append(&mut f.trail);
    f.trail = trail;
    f.check_sane(&after)?;
    let new: HashSet<u64> = after.keys().filter(|k| !wins.contains_key(k)).copied().collect();
    let app_id = format!("olo.{}", &app[4..]);
    if new.is_empty() {
        // Brought one of its windows forward instead.
        let front: Vec<u64> = after.iter().filter(|(_, w)| w.focused).map(|(i, _)| *i).collect();
        if front.first().is_none_or(|i| after[i].app_id != app_id) {
            f.fail(&format!("launching {app} {args:?} showed none of its windows (focused: {front:?})"))?;
        }
        let skip: HashSet<u64> = front.first().map(|&w| Fuzz::mates(&after, w)).unwrap_or_default().into_iter().chain(front.iter().copied()).collect();
        return f.check_unchanged(&format!("launching {app} again"), &wins, &after, &skip);
    }
    f.check_unchanged(&format!("opening {app}"), &wins, &after, &new)?;
    for n in &new {
        if !after[n].focused {
            f.fail(&format!("new window {n} ({}) doesn't have the keyboard", after[n].app_id))?;
        }
    }
    Ok(())
}

fn op_close(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if wins.len() <= 3 {
        return Ok(());
    }
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    f.log(&format!("close {w} ({})", wins[&w].app_id));
    f.req(Request::Close { id: w })?;
    let after = f.settle()?;
    if after.contains_key(&w) {
        // StyledEdit asks before closing a changed document; leave it.
        return Ok(());
    }
    f.check_sane(&after)?;
    let shrunk = Fuzz::shrunk(w, &Fuzz::group_of(wins, w), wins, &after);
    f.check_unchanged(&format!("closing {w}"), wins, &after, &shrunk)
}

fn op_workspace_trip(f: &mut Fuzz, wins: &Wins, _: &[u64]) -> Step {
    let ws = f.workspace()?;
    let to = *f.rng.choice(&(0..4).filter(|i| *i != ws).collect::<Vec<u32>>());
    f.log(&format!("workspace {ws} -> {to} -> {ws}"));
    f.req(Request::SwitchWorkspace { index: to, output: None })?;
    let mid = f.settle()?;
    f.check_sane(&mid)?;
    f.req(Request::SwitchWorkspace { index: ws, output: None })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_round_trip(&format!("going to workspace {to} and back"), wins, &after, &HashSet::new())
}

fn op_workspace_cmd(f: &mut Fuzz, _: &Wins, shown: &[u64]) -> Step {
    let sid = *f.rng.choice(&WORKSPACE_CMDS);
    let w = f.pick(shown.iter().copied());
    f.log(&format!("shortcut {sid} on {w:?}"));
    f.shortcut(sid, w)?;
    let after = f.settle()?;
    f.check_sane(&after)
}

fn op_zoom_trip(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    let how = *f.rng.choice(&["zoom", "zoom-vertical", "zoom-horizontal", "ipc-zoom"]);
    f.log(&format!("{how} {w} ({}) and back", wins[&w].app_id));
    let mut mid = Wins::new();
    for n in 0..2 {
        if how == "ipc-zoom" {
            f.req(Request::ToggleZoom { id: w })?;
        } else {
            f.shortcut(how, Some(w))?;
        }
        mid = f.settle()?;
        f.check_sane(&mid)?;
        // (Zoomed by the first toggle, unless it was zoomed already.)
        let plain = how == "zoom" || how == "ipc-zoom";
        if n == 0 && plain && !wins[&w].zoomed {
            f.check_inside_area(how, w, &mid)?;
        }
        // Each toggle flips it (`WindowInfo::zoomed`).
        let expected = wins[&w].zoomed != (n == 0);
        if plain && let Some(m) = mid.get(&w).filter(|m| m.zoomed != expected) {
            let z = m.zoomed;
            f.fail(&format!("{how} on {w}: zoomed is {z} after toggling {} times", n + 1))?;
        }
    }
    f.check_round_trip(&format!("{how} twice on {w}"), wins, &mid, &Fuzz::mates(wins, w))
}

fn op_tile_trip(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    let k = f.rng.int(1, 3) as usize;
    let mut tiles: Vec<&str> = f.rng.sample(&TILES, k);
    // (Not Center: where it centers is the window's own place then.)
    if let Some(extra) = *f.rng.choice(&[None, None, Some("zoom"), Some("zoom-vertical"), Some("zoom-horizontal")]) {
        tiles.push(extra);
    }
    f.log(&format!("restore {w} ({}), {}, then restore", wins[&w].app_id, tiles.join(" then ")));
    // From its own frame (Restore first), and back to it.
    f.shortcut("restore", Some(w))?;
    let own = f.settle()?;
    f.check_sane(&own)?;
    f.check_unchanged(&format!("restoring {w}"), wins, &own, &Fuzz::mates(wins, w))?;
    for t in &tiles {
        f.shortcut(t, Some(w))?;
        let mid = f.settle()?;
        f.check_sane(&mid)?;
        if TILES.contains(t) || *t == "zoom" {
            f.check_inside_area(t, w, &mid)?;
        }
    }
    f.shortcut("restore", Some(w))?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_round_trip(&format!("{} then restore on {w}", tiles.join("/")), &own, &after, &Fuzz::mates(&own, w))
}

fn op_arrange_trip(f: &mut Fuzz, _: &Wins, shown: &[u64]) -> Step {
    if shown.len() < 2 {
        return Ok(());
    }
    let sid = *f.rng.choice(&["tile-all", "cascade-all", "fan-out", "zoom-all", "cascade-others", "fan-out-others"]);
    let w = f.pick(shown.iter().copied()).expect("two shown");
    f.log(&format!("{sid} (with {w}) then undo"));
    f.req(Request::Activate { id: w })?;
    let before = f.settle()?;
    f.shortcut(sid, Some(w))?;
    let mid = f.settle()?;
    f.check_sane(&mid)?;
    if sid == "tile-all" {
        for i in f.shown(&mid)? {
            f.check_inside_area(sid, i, &mid)?;
        }
    }
    f.shortcut("undo-arrangement", Some(w))?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    let all: HashSet<u64> = before.keys().copied().collect();
    f.check_round_trip(&format!("{sid} then undo"), &before, &after, &all)
}

fn op_window_ws_trip(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    // Along its own display's workspaces.
    let (current, by) = f.showing()?;
    let out = wins[&w].output.clone();
    let ws = out.as_ref().and_then(|o| by.get(o)).copied().unwrap_or(current);
    let to = *f.rng.choice(&(0..4).filter(|i| *i != ws).collect::<Vec<u32>>());
    f.log(&format!("window {w} to workspace {to} and back"));
    f.req(Request::MoveToWorkspace { id: w, index: to, output: None })?;
    let mid = f.settle()?;
    f.check_sane(&mid)?;
    let mates = Fuzz::mates(wins, w);
    f.check_unchanged(&format!("moving {w} away"), wins, &mid, &mates)?;
    f.req(Request::SwitchWorkspace { index: to, output: out.clone() })?;
    f.settle()?;
    f.req(Request::MoveToWorkspace { id: w, index: ws, output: None })?;
    f.req(Request::SwitchWorkspace { index: ws, output: out })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_unchanged(&format!("moving {w} away and back"), wins, &after, &mates)?;
    if wins[&w].group.is_none() {
        f.check_back(&format!("moving {w} to workspace {to} and back"), w, wins, &after)?;
    }
    Ok(())
}

fn op_display_trip(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    if f.output_rects()?.len() < 2 {
        return Ok(());
    }
    f.log(&format!("{w} ({}) to the next display and back", wins[&w].app_id));
    let mates = Fuzz::mates(wins, w);
    f.shortcut("next-display", Some(w))?;
    let mid = f.settle()?;
    f.check_sane(&mid)?;
    f.check_unchanged(&format!("{w} to the next display"), wins, &mid, &mates)?;
    f.shortcut("previous-display", Some(w))?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    f.check_round_trip(&format!("{w} to the next display and back"), wins, &after, &mates)
}

fn op_displays(f: &mut Fuzz, _: &Wins, _: &[u64]) -> Step {
    if f.outputs.len() < 2 {
        return Ok(());
    }
    let others: Vec<String> = f.outputs[1..].to_vec();
    let name = f.rng.choice(&others).clone();
    let what = *f.rng.choice(&["move", "scale", "mirror", "home"]);
    let mut cfg = OutputConfig::default();
    match what {
        "move" => cfg.position = Some(*f.rng.choice(&[(1280, 0), (1280, 400), (-960, 0), (0, 800), (200, -540)])),
        "scale" => cfg.scale = Some(*f.rng.choice(&[1.0, 1.5, 2.0])),
        "mirror" => cfg.mirror = f.outputs_info()?.first().map(|o| o.key.clone()),
        _ => {
            let (pos, scale) = f.home[&name];
            cfg.position = Some(pos);
            cfg.scale = Some(scale);
            cfg.mirror = Some(String::new());
        }
    }
    f.log(&format!("display {name}: {what} {}", serde_json::to_string(&cfg).unwrap_or_default()));
    f.req(Request::ConfigureOutput { name, config: cfg })?;
    let after = f.settle()?;
    f.check_sane(&after)
}

fn op_place(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let Some(w) = f.pick(shown.iter().copied()) else { return Ok(()) };
    let outs = f.output_rects()?;
    let o = *f.rng.choice(&outs);
    let frame = Frame {
        x: o.x + f.rng.int(-200, o.w as i64) as i32,
        y: o.y + f.rng.int(-50, o.h as i64) as i32,
        width: f.rng.int(80, (o.w + 200) as i64) as i32,
        height: f.rng.int(60, (o.h + 200) as i64) as i32,
    };
    f.log(&format!("place {w} at ({}, {}, {}, {})", frame.x, frame.y, frame.width, frame.height));
    f.req(Request::Place { id: w, frame })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    // Its group goes with it: as it was, and as it is now.
    let skip: HashSet<u64> = Fuzz::mates(wins, w).union(&Fuzz::mates(&after, w)).copied().collect();
    f.check_unchanged(&format!("placing {w}"), wins, &after, &skip)
}

/// Terminal resizing its own window: a new tab (its tab strip comes),
/// closing it again (the strip goes), a bigger or smaller text size (every
/// Terminal window keeps its grid). First the window is left as it is,
/// zoomed, zoomed one way, tiled, or made as tall as its display.
fn op_app_resize(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let terms: Vec<u64> = shown.iter().copied().filter(|i| wins[i].app_id == "olo.terminal").collect();
    let Some(w) = f.pick(terms.iter().copied()) else { return Ok(()) };
    let how = *f.rng.choice(&["as it is", "zoom", "zoom-vertical", "tile-left", "tile-top", "tall"]);
    let what = if f.text_steps >= 2 {
        *f.rng.choice(&["new tab", "new tab, closed", "smaller text"])
    } else if f.text_steps <= -1 {
        *f.rng.choice(&["new tab", "new tab, closed", "bigger text"])
    } else {
        *f.rng.choice(&["new tab", "new tab, closed", "bigger text", "smaller text"])
    };
    f.log(&format!("Terminal {w}, {how}: {what}"));
    if how == "tall" {
        f.shortcut("restore", Some(w))?;
        let now = f.settle()?;
        let Some(nf) = fr_of(&now, w) else { return Ok(()) };
        let outs = f.output_rects()?;
        let home = *outs.iter().max_by_key(|o| overlap(nf, **o)).expect("a display");
        let frame = Frame { x: home.x + home.w / 4, y: home.y, width: (home.w / 2).min(1000), height: home.h };
        f.req(Request::Place { id: w, frame })?;
    } else if how != "as it is" && !(how == "zoom" && wins[&w].zoomed) {
        f.shortcut(how, Some(w))?;
    }
    f.req(Request::Activate { id: w })?;
    let before = f.settle()?;
    f.check_sane(&before)?;
    let Some(bw) = before.get(&w).filter(|b| b.focused && fr(b).is_some()) else { return Ok(()) };
    // Zoomed or tiled, it keeps its size; zoomed vertically, its height,
    // and horizontally its width.
    let held = matches!(how, "zoom" | "tile-left" | "tile-top") || bw.zoomed;
    let tall = bw.zoomed_along == Some(ZoomAxis::Vertical) && !held;
    let wide = bw.zoomed_along == Some(ZoomAxis::Horizontal) && !held;
    if what.starts_with("new tab") {
        f.keys(&[CMD, SHIFT, 20])?;
        let mid = f.settle()?;
        f.check_sane(&mid)?;
        f.check_steady(&format!("a new tab in {w}"), &before, &HashSet::new())?;
        f.check_unchanged(&format!("a new tab in {w}"), &before, &mid, &HashSet::new())?;
        if what.ends_with("closed") && mid.get(&w).is_some_and(|m| m.focused) {
            std::thread::sleep(std::time::Duration::from_millis(300));
            f.keys(&[CMD, SHIFT, 17])?;
            let after = f.settle()?;
            f.check_sane(&after)?;
            if !after.contains_key(&w) {
                f.fail(&format!("closing the new tab in {w} closed the window"))?;
            }
            f.check_steady(&format!("closing the new tab in {w}"), &mid, &HashSet::new())?;
            f.check_unchanged(&format!("closing the new tab in {w}"), &mid, &after, &HashSet::new())?;
        }
        return Ok(());
    }
    let bigger = what == "bigger text";
    f.text_steps += if bigger { 1 } else { -1 };
    f.keys(&[CMD, SHIFT, if bigger { 13 } else { 12 }])?;
    // Every Terminal window resizes itself to keep its grid, each when it
    // next draws: one on another display can take longer.
    let after = f.settle_for(5.0, 1.0)?;
    f.check_sane(&after)?;
    let terms: Vec<u64> = terms.into_iter().filter(|i| fr_of(&before, *i).is_some() && fr_of(&after, *i).is_some()).collect();
    let skip: HashSet<u64> = terms.iter().flat_map(|i| Fuzz::mates(&before, *i)).collect();
    f.check_unchanged(what, &before, &after, &skip)?;
    // Any may fit its grid again later, once, after what this step did.
    let step = f.step;
    f.grid_pending_since.extend(after.iter().filter(|(_, a)| a.app_id == "olo.terminal").map(|(i, _)| (*i, step)));
    let outs = f.output_rects()?;
    for i in &terms {
        let (a, b) = (fr_of(&before, *i).expect("filtered"), fr_of(&after, *i).expect("filtered"));
        if a == b {
            continue;
        }
        let home = *outs.iter().max_by_key(|o| overlap(b, **o)).expect("a display");
        if !contains(home, b) {
            f.fail(&format!("{what}: Terminal window {i} went from {a} to {b}, past its display {home}"))?;
        }
        if before[i].zoomed {
            f.fail(&format!("{what}: zoomed Terminal window {i} changed size: {a} -> {b}"))?;
        }
    }
    let (a, b) = (fr_of(&before, w).expect("checked"), fr_of(&after, w));
    if held && b != Some(a) {
        f.fail(&format!("{what}: Terminal window {w} ({how}) didn't keep its size: {a} -> {}", opt(b)))?;
    }
    if let Some(b) = b {
        if tall && (a.y, a.h) != (b.y, b.h) {
            f.fail(&format!("{what}: Terminal window {w} ({how}) didn't keep its height: {a} -> {b}"))?;
        }
        if wide && (a.x, a.w) != (b.x, b.w) {
            f.fail(&format!("{what}: Terminal window {w} ({how}, so zoomed horizontally) didn't keep its width: {a} -> {b}"))?;
        }
    }
    // A zoomed one keeps it all along (no flicker).
    for i in &terms {
        if !before[i].zoomed {
            continue;
        }
        let was = fr_of(&before, *i);
        if let Some(t) = f.trail.iter().find_map(|t| fr_of(t, *i).filter(|r| Some(*r) != was)) {
            f.fail(&format!("{what}: zoomed Terminal window {i} flickered to {t} (from {})", opt(was)))?;
        }
    }
    Ok(())
}

// ---- groups by hand ----

/// `w` in front, ready to be ⌘-dragged by its tab: the windows then, where
/// to take hold of its tab, and its display's tiling area; None if its tab
/// can't be taken hold of.
/// The windows, where the pointer is held and the window's frame.
type Held = (Wins, (f64, f64), R);

fn hold_ready(f: &mut Fuzz, w: u64) -> Result<Option<Held>, crate::fuzz::Failure> {
    f.req(Request::Activate { id: w })?;
    let wins = f.settle()?;
    let Some(x) = wins.get(&w).filter(|x| !x.minimized && x.focused) else { return Ok(None) };
    let Some(t) = tab_rect(x).filter(|t| t.w >= TAB_GRIP) else { return Ok(None) };
    let at = (t.x as f64 + t.w as f64 / 2.0, t.y as f64 + t.h as f64 / 2.0);
    let (_, area) = f.area()?;
    let Some(area) = area.filter(|a| inside(at, *a)) else { return Ok(None) };
    Ok(Some((wins, at, area)))
}

/// Each shown window's tab bar, and as near to it as a held tab goes in
/// there, but `but`'s.
fn bands(f: &mut Fuzz, wins: &Wins, but: &HashSet<u64>) -> Result<Vec<R>, crate::fuzz::Failure> {
    let cur = f.showing()?;
    Ok(wins.iter().filter(|(i, w)| !but.contains(i) && !w.minimized && Fuzz::here(w, &cur)).filter_map(|(_, w)| fr(w)).map(|r| R { x: r.x, y: r.y - NEAR, w: r.w, h: TAB_H + 2 * NEAR }).collect())
}

fn shown_groups(wins: &Wins, shown: &[u64]) -> Vec<Vec<u64>> {
    Fuzz::groups(wins).into_values().filter(|g| shown.contains(&g[0])).collect()
}

fn op_tab_along(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let groups = shown_groups(wins, shown);
    if groups.is_empty() {
        return Ok(());
    }
    let g = f.rng.choice(&groups).clone();
    let w = *f.rng.choice(&g);
    let Some((wins, at, _)) = hold_ready(f, w)? else { return Ok(()) };
    let g = Fuzz::group_of(&wins, w);
    if g.len() < 2 {
        return Ok(());
    }
    let r = fr_of(&wins, w).expect("in front");
    let others: Vec<u64> = g.iter().copied().filter(|i| *i != w).collect();
    let wanted: Vec<i32> = others.iter().map(|i| tab_rect(&wins[i]).map_or(0, |t| t.w)).collect();
    let to_place = f.rng.below(g.len());
    let (sx, sw) = tab_slot(&wanted, tab_rect(&wins[&w]).map_or(0, |t| t.w), r.w, to_place);
    // Its middle over that place's, a little up or down along the bar.
    let to = (r.x as f64 + sx as f64 + sw as f64 / 2.0, at.1 + f.rng.uniform(-8.0, 8.0));
    if !f.on_display(to)? {
        return Ok(());
    }
    let from = g.iter().position(|i| *i == w).unwrap_or(0);
    f.log(&format!("cmd-drag {w}'s tab from place {from} to {to_place} along group {} {g:?}", crate::fuzz::g(wins[&w].group)));
    f.cmd_drag(at, &[to], false)?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    let mut want = others.clone();
    want.insert(to_place.min(want.len()), w);
    if !after.contains_key(&w) || Fuzz::group_of(&after, w) != want {
        let got = if after.contains_key(&w) { format!("{:?}", Fuzz::group_of(&after, w)) } else { "?".into() };
        f.fail(&format!("cmd-dragging {w}'s tab to place {to_place}: the group is {got}, not {want:?}"))?;
    }
    f.check_unchanged(&format!("cmd-dragging {w}'s tab along its group"), &wins, &after, &HashSet::new())
}

fn op_tab_out(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let groups = shown_groups(wins, shown);
    if groups.is_empty() {
        return Ok(());
    }
    let g = f.rng.choice(&groups).clone();
    let w = *f.rng.choice(&g);
    let Some((wins, at, _)) = hold_ready(f, w)? else { return Ok(()) };
    let g = Fuzz::group_of(&wins, w);
    if g.len() < 2 {
        return Ok(());
    }
    let Some(away) = f.pull_away(at)? else { return Ok(()) };
    // Let go somewhere on a display clear of every tab bar (over one, it
    // would go in there), maybe by way of others.
    let bands = bands(f, &wins, &HashSet::from([w]))?;
    let outs = f.output_rects()?;
    let mut spots = Vec::new();
    for _ in 0..40 {
        let o = *f.rng.choice(&outs);
        let p = (o.x as f64 + f.rng.uniform(40.0, (o.w as f64 - 40.0).max(41.0)), o.y as f64 + f.rng.uniform(40.0, (o.h as f64 - 40.0).max(41.0)));
        if !bands.iter().any(|b| inside(p, *b)) {
            spots.push(p);
            if spots.len() > f.rng.below(3) {
                break;
            }
        }
    }
    let Some(&to) = spots.last() else { return Ok(()) };
    let by_way = if spots.len() > 1 { format!(" by way of {:?}", spots[..spots.len() - 1].iter().map(|p| fmt_p(*p)).collect::<Vec<_>>()) } else { String::new() };
    f.log(&format!("cmd-drag {w}'s tab out of group {} {g:?} to {}{by_way}", crate::fuzz::g(wins[&w].group), fmt_p(to)));
    let mut path = vec![away];
    path.extend(&spots);
    f.cmd_drag(at, &path, false)?;
    f.alone.insert(w);
    f.by_hand.remove(&w);
    let after = f.settle()?;
    f.check_sane(&after)?;
    let Some(aw) = after.get(&w) else { return Ok(()) };
    let rest: Vec<u64> = g.iter().copied().filter(|i| *i != w).collect();
    if let Some(gr) = aw.group {
        f.fail(&format!("{w}, cmd-dragged out of its group, is in group {gr}"))?;
    }
    if rest.len() > 1 && Fuzz::group_of(&after, rest[0]) != rest {
        f.fail(&format!("the rest of {w}'s group {rest:?} came apart: now {:?}", Fuzz::group_of(&after, rest[0])))?;
    }
    // The pointer still on its tab, which is the size it was.
    let t = tab_rect(&after[&w]);
    if !t.is_some_and(|t| inside(to, R { x: t.x - 1, y: t.y - 1, w: t.w + 2, h: t.h + 2 })) {
        f.fail(&format!("{w}, let go at {} after cmd-dragging it out, has its tab at {}", fmt_p(to), opt(t)))?;
    }
    if let (Some(a), Some(b)) = (fr_of(&after, w), fr_of(&wins, w))
        && (a.w, a.h) != (b.w, b.h)
    {
        f.fail(&format!("{w}, cmd-dragged out of its group, changed size: {b} -> {a}"))?;
    }
    let mut skip = Fuzz::shrunk(w, &Fuzz::group_of(&wins, w), &wins, &after);
    skip.insert(w);
    f.check_unchanged(&format!("cmd-dragging {w} out of its group"), &wins, &after, &skip)
}

fn op_tab_in(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if shown.len() < 2 {
        return Ok(());
    }
    let w = f.pick(shown.iter().copied()).expect("two shown");
    let mates = Fuzz::mates(wins, w);
    let Some(t) = f.pick(shown.iter().copied().filter(|i| !mates.contains(i))) else { return Ok(()) };
    // Its group in front, then w's in front of that: only w's can hide it.
    f.req(Request::Activate { id: t })?;
    f.settle()?;
    let (_, t_area) = f.area()?;
    let Some((wins, at, _)) = hold_ready(f, w)? else { return Ok(()) };
    let Some(t_area) = t_area else { return Ok(()) };
    if !wins.get(&t).is_some_and(|x| !x.minimized && fr(x).is_some()) || Fuzz::mates(&wins, w).contains(&t) {
        return Ok(());
    }
    let tg = Fuzz::group_of(&wins, t);
    let tr = fr_of(&wins, t).expect("checked");
    let cover: Vec<R> = Fuzz::mates(&wins, w).into_iter().filter(|i| *i != w).filter_map(|i| fr_of(&wins, i)).map(|c| R { x: c.x, y: c.y - NEAR, w: c.w, h: c.h + NEAR }).collect();
    // Over its tab bar; on its very tab if it is a dialog or panel.
    let mut bar = R { x: tr.x + 2, y: tr.y - NEAR + 2, w: tr.w - 4, h: TAB_H + 2 * NEAR - 4 };
    if !wins[&t].groupable
        && let Some(r) = tab_rect(&wins[&t])
    {
        bar = R { x: r.x + 1, y: r.y + 1, w: r.w - 2, h: r.h - 2 };
    }
    let mut to = None;
    for _ in 0..20 {
        let p = (bar.x as f64 + f.rng.uniform(0.0, bar.w.max(1) as f64), bar.y as f64 + f.rng.uniform(0.0, bar.h.max(1) as f64));
        if inside(p, t_area) && !cover.iter().any(|c| inside(p, *c)) {
            to = Some(p);
            break;
        }
    }
    let away = f.pull_away(at)?;
    let (Some(to), Some(away)) = (to, away) else { return Ok(()) };
    // (A dialog or panel is never grouped: ⌘-dragging one moves it.)
    let joins = wins[&w].groupable && wins[&t].groupable;
    let name = |i: u64| format!("{i} ({}{}, group {})", wins[&i].app_id, if wins[&i].groupable { "" } else { ", not groupable" }, crate::fuzz::g(wins[&i].group));
    f.log(&format!("cmd-drag {} onto {}'s tabs {tg:?} at {}", name(w), name(t), fmt_p(to)));
    let shown_as = f.cmd_drag(at, &[away, to], true)?.unwrap_or_default();
    if joins {
        f.by_hand.extend([w, t]);
        f.alone.remove(&w);
        f.alone.remove(&t);
    } else if wins[&w].group.is_some() {
        f.alone.insert(w);
    }
    let after = f.settle()?;
    f.check_sane(&after)?;
    if !after.contains_key(&w) || !after.contains_key(&t) {
        return Ok(());
    }
    if !joins {
        if after[&w].group.is_some() || Fuzz::group_of(&after, t) != tg {
            f.fail(&format!("{} let go over {}'s tabs went in: {:?}", name(w), name(t), Fuzz::group_of(&after, w)))?;
        }
        return Ok(());
    }
    // Held there, it showed as a tab among theirs, in its place.
    let held = shown_as.get(&w).and_then(tab_rect);
    let theirs: Vec<Option<R>> = tg.iter().filter_map(|i| shown_as.get(i)).map(tab_rect).collect();
    let ok = held.is_some_and(|h| h.y == tr.y && h.x >= tr.x && h.x + h.w <= tr.x + tr.w && !theirs.iter().flatten().any(|r| overlap(h, *r) > 0));
    if !ok {
        f.fail(&format!("{w} held over {t}'s tabs showed its tab at {}, not among theirs {:?} along ({}, {}, {})", opt(held), theirs.iter().map(|r| opt(*r)).collect::<Vec<_>>(), tr.x, tr.y, tr.w))?;
    }
    let got = Fuzz::group_of(&after, w);
    let mut want_set: HashSet<u64> = tg.iter().copied().collect();
    want_set.insert(w);
    if after[&w].group.is_none() || got.iter().copied().collect::<HashSet<_>>() != want_set || got.iter().copied().filter(|i| *i != w).collect::<Vec<_>>() != tg {
        f.fail(&format!("{w} let go over {t}'s tabs: its group is {got:?}, not {tg:?} with it"))?;
    } else if let Some(h) = held
        && tab_rect(&after[&w]) != Some(h)
    {
        // Where it showed it would go (if the group grew, its tabs wider).
        let grew = matches!((fr_of(&after, t), fr_of(&wins, t)), (Some(a), Some(b)) if a.w > b.w);
        let place = theirs.iter().flatten().filter(|r| r.x < h.x).count();
        if !grew || got.iter().position(|i| *i == w) != Some(place) {
            f.fail(&format!("{w} showed its tab at {h} over {t}'s, but went in at {}", opt(tab_rect(&after[&w]))))?;
        }
    }
    // (Its old group, tiled, gives back what it needed.)
    let mut skip = Fuzz::grown(w, &tg, &wins, &after);
    skip.extend(Fuzz::shrunk(w, &Fuzz::group_of(&wins, w), &wins, &after));
    skip.insert(w);
    f.check_unchanged(&format!("cmd-dragging {w} into {t}'s group"), &wins, &after, &skip)
}

fn op_group_window(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    if shown.len() < 2 {
        return Ok(());
    }
    let w = f.pick(shown.iter().copied()).expect("two shown");
    let t = f.pick(shown.iter().copied().filter(|i| *i != w)).expect("two shown");
    let joins = wins[&w].groupable && wins[&t].groupable;
    let tg: Vec<u64> = Fuzz::group_of(wins, t).into_iter().filter(|i| *i != w).collect();
    let choices: Vec<Option<u32>> = std::iter::once(None).chain((0..tg.len() as u32 + 2).map(Some)).collect();
    let index = *f.rng.choice(&choices);
    f.log(&format!("group_window {w} ({}) with {t} ({}, group {tg:?}) at {index:?}", wins[&w].app_id, wins[&t].app_id));
    let reply = f.try_req(Request::GroupWindow { id: w, with: t, index })?;
    let refused = matches!(reply, Reply::Error { .. });
    if !refused && joins {
        f.by_hand.extend([w, t]);
        f.alone.remove(&w);
        f.alone.remove(&t);
    }
    let after = f.settle()?;
    f.check_sane(&after)?;
    if refused == joins {
        let said = if let Reply::Error { message } = &reply { message.clone() } else { "done".into() };
        f.fail(&format!("group_window {w} with {t}: {said}, though {w} is {}groupable and {t} {}", if wins[&w].groupable { "" } else { "not " }, if wins[&t].groupable { "is" } else { "is not" }))?;
    }
    if refused {
        return f.check_unchanged(&format!("group_window {w} with {t}, refused"), wins, &after, &HashSet::new());
    }
    let at = index.map_or(tg.len(), |i| (i as usize).min(tg.len()));
    let mut want = tg.clone();
    want.insert(at, w);
    if after.contains_key(&w) && Fuzz::group_of(&after, w) != want {
        f.fail(&format!("group_window {w} with {t} at {index:?}: the group is {:?}, not {want:?}", Fuzz::group_of(&after, w)))?;
    }
    if after.get(&w).is_some_and(|a| !a.focused) {
        f.fail(&format!("group_window {w} with {t}: {w} doesn't have the keyboard"))?;
    }
    // The group may grow to what its new window can be at the least.
    let mut skip = Fuzz::grown(w, &tg, wins, &after);
    skip.extend(Fuzz::shrunk(w, &Fuzz::group_of(wins, w), wins, &after));
    skip.insert(w);
    f.check_unchanged(&format!("group_window {w} with {t}"), wins, &after, &skip)
}

fn op_ungroup_window(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let groups = shown_groups(wins, shown);
    if groups.is_empty() {
        return Ok(());
    }
    let g = f.rng.choice(&groups).clone();
    let w = *f.rng.choice(&g);
    let rest: Vec<u64> = g.iter().copied().filter(|i| *i != w).collect();
    f.log(&format!("ungroup_window {w} (group {} {g:?})", crate::fuzz::g(wins[&w].group)));
    f.req(Request::UngroupWindow { id: w })?;
    f.alone.insert(w);
    f.by_hand.remove(&w);
    let after = f.settle()?;
    f.check_sane(&after)?;
    let Some(aw) = after.get(&w) else { return Ok(()) };
    if let Some(gr) = aw.group {
        f.fail(&format!("ungroup_window {w}: it is in group {gr}"))?;
    }
    if !aw.focused {
        f.fail(&format!("ungroup_window {w}: it doesn't have the keyboard"))?;
    }
    // Stepped out from the group as a cascade, at its size.
    let (a, b) = (fr_of(&wins.clone(), w), fr_of(&after, w));
    if a == b || a.map(|r| (r.w, r.h)) != b.map(|r| (r.w, r.h)) {
        f.fail(&format!("ungroup_window {w}: {} -> {}", opt(a), opt(b)))?;
    }
    if rest.len() > 1 && Fuzz::group_of(&after, rest[0]) != rest {
        f.fail(&format!("ungroup_window {w}: the rest of its group {rest:?} came apart: now {:?}", Fuzz::group_of(&after, rest[0])))?;
    }
    let mut skip = Fuzz::shrunk(w, &Fuzz::group_of(wins, w), wins, &after);
    skip.insert(w);
    f.check_unchanged(&format!("ungroup_window {w}"), wins, &after, &skip)
}

fn op_move_tab(f: &mut Fuzz, wins: &Wins, shown: &[u64]) -> Step {
    let groups = shown_groups(wins, shown);
    if groups.is_empty() {
        return Ok(());
    }
    let g = f.rng.choice(&groups).clone();
    let w = *f.rng.choice(&g);
    let index = f.rng.below(g.len() + 1);
    f.log(&format!("move_tab {w} to {index} in group {} {g:?}", crate::fuzz::g(wins[&w].group)));
    f.req(Request::MoveTab { id: w, index: index as u32 })?;
    let after = f.settle()?;
    f.check_sane(&after)?;
    let mut want: Vec<u64> = g.iter().copied().filter(|i| *i != w).collect();
    want.insert(index.min(want.len()), w);
    if after.contains_key(&w) && Fuzz::group_of(&after, w) != want {
        f.fail(&format!("move_tab {w} to {index}: the group is {:?}, not {want:?}", Fuzz::group_of(&after, w)))?;
    }
    f.check_unchanged(&format!("move_tab {w}"), wins, &after, &HashSet::new())
}
