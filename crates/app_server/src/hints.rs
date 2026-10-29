//! Keyboard hints (Preferences ▸ Keyboard): pressing Control twice
//! quickly puts a letter on everything that can be picked: the focused
//! window's controls, the other windows (on their tabs) and Deskbar.
//! Typing a letter clicks what it marks or, for a part with more inside
//! (a box of controls, Deskbar, another window), letters what is inside,
//! and so on down; Backspace goes back up and Esc puts them away. When a
//! click opens a menu, its items are lettered next.
//!
//! What can be picked comes from the kit's widget trees (the same ones
//! agents read, `olo-menus-PID.sock`), asked for on worker threads; the
//! letters showing are [`Request::Hints`](olo_protocol::Request::Hints)
//! for agents.
//!
//! A level never has more than 26 letters: everything is lettered at once
//! when it fits, else the window's groups (as its boxes nest) are opened
//! smallest first while they fit, and what is left is lettered by group.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use crate::render::Exact;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    desktop::{layer_map_for_output, utils::surface_primary_scanout_output},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::{compositor::with_states, seat::WaylandFocus},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform},
};
use tiny_skia::Pixmap;

use crate::{
    shell::{WindowElement, fill},
    state::{AnvilState, Backend},
    text, theme,
};

/// Home row first.
pub const LETTERS: &str = "asdfghjklqwertyuiopzxcvbnm";

/// What picking gives the keyboard to rather than clicks.
const FOCUSED: [&str; 6] = ["Slider", "SpinButton", "ScrollBar", "TextInput", "MultilineTextInput", "SearchInput"];

/// How long the apps have to say what they show.
const WAIT: Duration = Duration::from_millis(500);
/// After a click, how long before looking for a menu it opened (and
/// putting the pointer back).
const SETTLE: Duration = Duration::from_millis(150);
const POLL: Duration = Duration::from_millis(8);

/// Badge sizes, logical.
const BADGE_H: i32 = 16;
const BADGE_PAD: i32 = 4;
const FONT: f32 = 11.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect { x, y, w: (self.x + self.w).max(o.x + o.w) - x, h: (self.y + self.h).max(o.y + o.h) - y }
    }

    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains_point(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    fn offset(self, dx: f64, dy: f64) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..self }
    }
}

/// Something a letter can pick.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A control, menu item, icon…: picking it clicks its middle, or gives
    /// it the keyboard (a slider or text field, which a click would
    /// change). `node` is its id in process `pid`'s tree.
    Leaf { rect: Rect, name: Option<String>, role: String, pid: u32, node: u64 },
    /// A part of a window (a box, Deskbar): picking it letters what's inside.
    /// Outlined, unless it is a layer surface's (windows may cover it).
    Group { rect: Rect, name: Option<String>, kids: Vec<Node>, outline: bool },
    /// Another window (its tab): picking it brings it forward and letters
    /// its content.
    Window { id: u64, rect: Rect, name: String },
}

impl Node {
    pub fn rect(&self) -> Rect {
        match self {
            Node::Leaf { rect, .. } | Node::Group { rect, .. } | Node::Window { rect, .. } => *rect,
        }
    }

    fn group(name: Option<String>, kids: Vec<Node>) -> Node {
        let rect = kids.iter().map(Node::rect).reduce(Rect::union).unwrap_or_default();
        Node::Group { rect, name, kids, outline: true }
    }

    /// A layer surface's group: not outlined.
    fn layer(name: String, kids: Vec<Node>) -> Node {
        let rect = kids.iter().map(Node::rect).reduce(Rect::union).unwrap_or_default();
        Node::Group { rect, name: Some(name), kids, outline: false }
    }

    /// How many letters it takes with everything inside lettered.
    fn leaves(&self) -> usize {
        match self {
            Node::Group { kids, .. } => kids.iter().map(Node::leaves).sum(),
            _ => 1,
        }
    }

    fn flatten(self, out: &mut Vec<Node>) {
        match self {
            Node::Group { kids, .. } => kids.into_iter().for_each(|k| k.flatten(out)),
            n => out.push(n),
        }
    }

    fn info(&self, label: char) -> olo_protocol::HintInfo {
        let r = self.rect();
        let (name, role) = match self {
            Node::Leaf { name, role, .. } => (name.clone(), role.clone()),
            Node::Group { name, .. } => (name.clone(), "Group".to_owned()),
            Node::Window { name, .. } => (Some(name.clone()), "Window".to_owned()),
        };
        olo_protocol::HintInfo { label: label.to_string(), x: r.x, y: r.y, width: r.w, height: r.h, name, role }
    }
}

/// The parts of the kit's tree (`olo_kit::commands`) hints need.
#[derive(Debug, Clone, Deserialize)]
pub struct UiNode {
    pub id: u64,
    #[serde(default)]
    pub parent: Option<u64>,
    pub role: String,
    #[serde(default)]
    pub name: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub enabled: bool,
    #[serde(default)]
    pub clickable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WindowTree {
    pub title: String,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub layer: Option<String>,
    /// Its `wl_surface`'s protocol id, if the app said.
    #[serde(default)]
    pub surface: Option<u32>,
    pub nodes: Vec<UiNode>,
}

/// What a window (or layer surface) at `origin` (global logical) shows,
/// as hint nodes in tree order. With a menu open, only its items.
pub fn build(tree: &WindowTree, origin: (f64, f64), pid: u32) -> Vec<Node> {
    let nodes = &tree.nodes;
    let content = Rect { x: 0.0, y: 0.0, w: tree.width, h: tree.height };
    let rect = |n: &UiNode| Rect { x: n.x, y: n.y, w: n.width, h: n.height };
    // Menu items hang outside the window; everything else must show in it.
    let target = |n: &UiNode| {
        n.clickable && n.enabled && n.width > 0.0 && n.height > 0.0 && (n.role == "MenuItem" || content.contains_point(rect(n).centre()))
    };
    let mut targets: Vec<bool> = nodes.iter().map(target).collect();
    // A nameless area behind others (a list's background) is not one, nor
    // is the outside of a control around its own inside (a menu field
    // and its label, around the field). The targets by their middles'
    // x, so each looks only at those between its sides.
    let mut ids: Vec<(f64, usize)> = (0..nodes.len()).filter(|&i| targets[i]).map(|i| (rect(&nodes[i]).centre().0, i)).collect();
    ids.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(_, i) in &ids {
        let (a, r) = (&nodes[i], rect(&nodes[i]));
        let (from, to) = (ids.partition_point(|&(x, _)| x < r.x), ids.partition_point(|&(x, _)| x < r.x + r.w));
        let covers = ids[from..to].iter().any(|&(_, j)| {
            let b = &nodes[j];
            let inside = r.contains_point(rect(b).centre()) && b.width * b.height < r.w * r.h;
            j != i && inside && (a.name.is_none() || a.name == b.name || a.role == b.role)
        });
        if covers {
            targets[i] = false;
        }
    }
    let menu = nodes.iter().zip(&targets).any(|(n, &t)| t && n.role == "MenuItem");
    let index: HashMap<u64, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut roots = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        match n.parent.and_then(|p| index.get(&p)) {
            Some(&p) if p != i => kids[p].push(i),
            _ => roots.push(i),
        }
    }
    struct Walk<'a> {
        nodes: &'a [UiNode],
        kids: Vec<Vec<usize>>,
        targets: Vec<bool>,
        origin: (f64, f64),
        pid: u32,
    }
    impl Walk<'_> {
        fn walk(&self, i: usize, depth: usize) -> Vec<Node> {
            let n = &self.nodes[i];
            // Deep enough for any real window; stops a malformed tree.
            let inner: Vec<Node> = if depth > 64 { Vec::new() } else { self.kids[i].iter().flat_map(|&k| self.walk(k, depth + 1)).collect() };
            if self.targets[i] {
                let rect = Rect { x: n.x, y: n.y, w: n.width, h: n.height }.offset(self.origin.0, self.origin.1);
                let leaf = Node::Leaf { rect, name: n.name.clone(), role: n.role.clone(), pid: self.pid, node: n.id };
                if inner.is_empty() {
                    return vec![leaf];
                }
                // A control with controls inside (a row with a check box): both.
                return vec![Node::group(n.name.clone(), std::iter::once(leaf).chain(inner).collect())];
            }
            if inner.len() > 1 { vec![Node::group(n.name.clone(), inner)] } else { inner }
        }
    }
    let walk = Walk { nodes, kids, targets, origin, pid };
    let mut out: Vec<Node> = roots.iter().flat_map(|&r| walk.walk(r, 0)).collect();
    if menu {
        // A menu's items, however they nest.
        let mut flat = Vec::new();
        out.into_iter().for_each(|n| n.flatten(&mut flat));
        flat.retain(|n| matches!(n, Node::Leaf { role, .. } if role == "MenuItem"));
        // Menus come in the order they opened, each a column of items: the
        // last opened (a submenu) first.
        let column = |n: &Node| (n.rect().x.round() as i64, n.rect().w.round() as i64);
        let mut columns: Vec<(i64, i64)> = Vec::new();
        for n in &flat {
            if !columns.contains(&column(n)) {
                columns.push(column(n));
            }
        }
        out = columns.iter().rev().flat_map(|c| flat.iter().filter(|n| column(n) == *c).cloned().collect::<Vec<_>>()).collect();
    }
    // One group around everything says nothing.
    while out.len() == 1 && matches!(out[0], Node::Group { .. }) {
        let Some(Node::Group { kids, .. }) = out.pop() else { unreachable!() };
        out = kids;
    }
    out
}

/// `nodes` with only the leaves `keep` lets through (those that can be
/// clicked: nothing else in front of them), groups made again around them.
pub fn prune(nodes: Vec<Node>, keep: &dyn Fn(&Node) -> bool) -> Vec<Node> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            Node::Group { name, kids, outline, .. } => {
                let mut kids = prune(kids, keep);
                if kids.len() > 1 {
                    let rect = kids.iter().map(Node::rect).reduce(Rect::union).unwrap_or_default();
                    out.push(Node::Group { rect, name, kids, outline });
                } else {
                    out.append(&mut kids);
                }
            }
            n if keep(&n) => out.push(n),
            _ => {}
        }
    }
    out
}

/// At most `budget` nodes to letter for `nodes`: all of them, flattened,
/// when they fit; else groups opened smallest first while they fit; and
/// if there are still too many, the last ones put together in runs.
pub fn expand(nodes: Vec<Node>, budget: usize) -> Vec<Node> {
    let budget = budget.max(1);
    if nodes.iter().map(Node::leaves).sum::<usize>() <= budget {
        let mut flat = Vec::new();
        nodes.into_iter().for_each(|n| n.flatten(&mut flat));
        return flat;
    }
    let mut items = nodes;
    loop {
        // A lone group opens whatever its size.
        if items.len() == 1 && matches!(items[0], Node::Group { .. }) {
            let Some(Node::Group { kids, .. }) = items.pop() else { unreachable!() };
            items = kids;
            continue;
        }
        let smallest = items
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                Node::Group { kids, .. } => Some((i, kids.len())),
                _ => None,
            })
            .filter(|&(_, k)| items.len() - 1 + k <= budget)
            .min_by_key(|&(_, k)| k);
        let Some((i, _)) = smallest else { break };
        let Node::Group { kids, .. } = items.remove(i) else { unreachable!() };
        items.splice(i..i, kids);
    }
    if items.len() > budget && budget > 1 {
        // As few runs as will do, of `per` at most, after letters of their own.
        let per = items.len().div_ceil(budget).max(2);
        let runs = (items.len() - budget).div_ceil(per - 1);
        let mut it = items.into_iter();
        let mut level: Vec<Node> = it.by_ref().take(budget - runs).collect();
        let rest: Vec<Node> = it.collect();
        let size = rest.len().div_ceil(runs);
        for run in rest.chunks(size) {
            level.push(if run.len() == 1 { run[0].clone() } else { Node::group(None, run.to_vec()) });
        }
        items = level;
    }
    items
}

/// The first level: the focused window's content (on the home row), then
/// the other windows, Deskbar and the desktop.
pub fn first_level(content: Vec<Node>, mut others: Vec<Node>, budget: usize) -> Vec<Node> {
    // Everything at once, if it fits.
    if content.iter().chain(&others).map(Node::leaves).sum::<usize>() <= budget {
        let mut level = Vec::new();
        content.into_iter().chain(others).for_each(|n| n.flatten(&mut level));
        return level;
    }
    // Many windows share one letter rather than crowd out the content.
    let windows = others.iter().filter(|n| matches!(n, Node::Window { .. })).count();
    if windows > 1 && others.len() > budget / 2 {
        let (w, rest): (Vec<Node>, Vec<Node>) = others.into_iter().partition(|n| matches!(n, Node::Window { .. }));
        others = rest;
        others.insert(0, Node::group(Some("Windows".into()), w));
    }
    let room = budget.saturating_sub(others.len()).max(1);
    let mut level = if content.is_empty() { Vec::new() } else { expand(content, room) };
    level.extend(others);
    level
}

/// The letter typed, from its evdev code (as on a US keyboard).
pub fn letter(key: u32) -> Option<char> {
    const ROWS: [(u32, &str); 3] = [(16, "qwertyuiop"), (30, "asdfghjkl"), (44, "zxcvbnm")];
    ROWS.iter().find_map(|&(first, row)| key.checked_sub(first).and_then(|i| row.chars().nth(i as usize)))
}

pub fn label(i: usize) -> char {
    LETTERS.chars().nth(i).unwrap_or('?')
}

// ---------------------------------------------------------------------------
// Asking the apps.

fn socket_path(pid: u32) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt.join(format!("olo-menus-{pid}.sock"))
}

/// What process `pid` shows on `surfaces` (`wl_surface` protocol ids),
/// only what can be picked (None if it isn't a kit app or is too slow).
/// Blocks: call it off the event loop.
fn ask(pid: u32, surfaces: &[u32]) -> Option<Vec<WindowTree>> {
    // The answer; empty if the app hung up without one.
    let question = |q: &str| -> Option<String> {
        let s = UnixStream::connect(socket_path(pid)).ok()?;
        s.set_read_timeout(Some(WAIT * 2)).ok()?;
        s.set_write_timeout(Some(WAIT)).ok()?;
        (&s).write_all(q.as_bytes()).ok()?;
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).ok()?;
        Some(line)
    };
    let hints = format!("{{\"hints\":{}}}\n", serde_json::to_string(surfaces).ok()?);
    // An app from before `hints` (not restarted since an upgrade) hangs
    // up on it: then all its windows.
    let line = match question(&hints)? {
        line if line.is_empty() => question("{\"tree\":true}\n")?,
        line => line,
    };
    serde_json::from_str(&line).ok()
}

/// Give the keyboard to widget `node` of process `pid`. Blocks.
fn focus(pid: u32, node: u64) {
    let Ok(s) = UnixStream::connect(socket_path(pid)) else { return };
    let _ = s.set_write_timeout(Some(WAIT));
    let _ = s.set_read_timeout(Some(WAIT));
    if (&s).write_all(format!("{{\"focus\":{node}}}\n").as_bytes()).is_ok() {
        let _ = BufReader::new(&s).read_line(&mut String::new());
    }
}

/// Where a tree is wanted from: a window or layer surface of process
/// `pid`, whose `wl_surface` has protocol id `id` (the app's own name
/// for it).
#[derive(Debug, Clone)]
struct Place {
    pid: u32,
    id: Option<u32>,
    what: What,
}

#[derive(Debug, Clone)]
enum What {
    Window { id: u64, title: String, content: Rect },
    Layer { namespace: String, rect: Rect },
}

/// What the answers are for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Purpose {
    /// The first level.
    Start,
    /// The first level, asked for ahead: Control was let go once on its
    /// own, so a second time (and the letters) is likely.
    Ahead,
    /// The content of a window just brought forward.
    Window(u64),
    /// After a click: a menu it opened, if any.
    Menu,
}

/// The tree in `trees` for `place`: the one for its surface or, from a
/// program that can't tell its surfaces apart (Chrome), matched as
/// `olo-screen tree` does.
fn find<'a>(trees: &'a [WindowTree], place: &Place) -> Option<&'a WindowTree> {
    if let Some(t) = trees.iter().find(|t| t.surface.is_some() && t.surface == place.id) {
        return Some(t);
    }
    let trees = || trees.iter().filter(|t| t.surface.is_none());
    match &place.what {
        What::Layer { namespace, .. } => trees().find(|t| t.layer.as_deref() == Some(namespace)),
        What::Window { title, content, .. } => {
            let windows = || trees().filter(|t| t.layer.is_none());
            let sized = |t: &&WindowTree| t.width as i32 == content.w as i32 && t.height as i32 == content.h as i32;
            windows()
                .find(|t| t.title == *title && sized(t))
                .or_else(|| windows().find(|t| t.title == *title))
                .or_else(|| windows().find(|t| !t.title.is_empty() && title.starts_with(&t.title) && t.width as i32 == content.w as i32))
                .or_else(|| windows().find(sized))
        }
    }
}

fn origin(place: &Place) -> (f64, f64) {
    match &place.what {
        What::Window { content, .. } | What::Layer { rect: content, .. } => (content.x, content.y),
    }
}

/// What one place shows, as hint nodes, and its tree's title.
type Built = (Vec<Node>, String);

/// Ask process `pid` about its `places` (indices into all of them) and
/// make their nodes, all on a worker thread: reading and walking a big
/// window's tree is no work for the event loop.
fn ask_and_build(pid: u32, places: Vec<(usize, Place)>) -> HashMap<usize, Built> {
    let ids: Vec<u32> = places.iter().filter_map(|(_, p)| p.id).collect();
    let trees = ask(pid, &ids).unwrap_or_default();
    places
        .iter()
        .filter_map(|(i, place)| {
            let tree = find(&trees, place)?;
            Some((*i, (build(tree, origin(place), pid), tree.title.clone())))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// State.

#[derive(Default)]
pub struct Hints {
    /// The levels picked through, the last one showing; empty when off.
    levels: Vec<Vec<Node>>,
    /// Asking the apps for a level, keys held back meanwhile (answers
    /// with another generation are stale).
    fetching: bool,
    generation: u64,
    /// Keys pressed while hints showed, whose releases are ours too.
    swallowed: Vec<u32>,
    /// The first level asked for ahead ([`Purpose::Ahead`]): when, and
    /// the level once the answers are in.
    ahead: Option<(Instant, Option<Vec<Node>>)>,
    /// When Control was pressed twice (how long the letters take).
    asked: Option<Instant>,
    overlay: Overlay,
}

impl std::fmt::Debug for Hints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hints").field("levels", &self.levels.len()).field("fetching", &self.fetching).finish()
    }
}

impl Hints {
    pub fn active(&self) -> bool {
        !self.levels.is_empty() || self.fetching
    }

    /// The level showing (none while asking the apps).
    pub fn showing(&self) -> &[Node] {
        if self.fetching { &[] } else { self.levels.last().map(Vec::as_slice).unwrap_or(&[]) }
    }

    pub fn infos(&self) -> Vec<olo_protocol::HintInfo> {
        self.showing().iter().enumerate().map(|(i, n)| n.info(label(i))).collect()
    }

    /// The letters showing, as elements for the output at `output`
    /// (global logical) drawn at `scale`.
    pub fn elements<R>(&mut self, renderer: &mut R, output: Rectangle<i32, Logical>, scale: f64) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let showing = if self.fetching { &[][..] } else { self.levels.last().map(Vec::as_slice).unwrap_or(&[]) };
        self.overlay.elements(renderer, showing, output, scale)
    }

    pub fn close(&mut self) {
        self.levels.clear();
        self.fetching = false;
        self.ahead = None;
        self.generation += 1;
    }

    /// Letters showing: say how long they took.
    fn shown(&mut self) {
        if let Some(at) = self.asked.take() {
            tracing::debug!(ms = at.elapsed().as_secs_f64() * 1000.0, letters = self.showing().len(), "keyboard hints shown");
        }
    }
}

/// How long a level asked for ahead is good for: the second press comes
/// within the double press time of the first, at most a second.
const AHEAD: Duration = Duration::from_secs(1);

impl<B: Backend + 'static> AnvilState<B> {
    /// Control pressed twice: show the hints, or put them away.
    pub fn toggle_hints(&mut self) {
        if !self.settings.keyboard.hints || self.lock.locked() {
            return;
        }
        if self.hints.active() {
            self.hints.close();
            return;
        }
        self.hints.asked = Some(Instant::now());
        match self.hints.ahead.take() {
            // Asked for when Control was let go the first time: shown now
            // if it's in, else as soon as it is.
            Some((at, level)) if at.elapsed() < AHEAD => match level {
                Some(level) if level.is_empty() => tracing::debug!("nothing to hint"),
                Some(level) => {
                    self.hints.levels = vec![level];
                    self.hints.shown();
                }
                None => {
                    self.hints.ahead = Some((at, None));
                    self.hints.fetching = true;
                }
            },
            _ => self.fetch_hints(Purpose::Start, None),
        }
    }

    /// Control let go once on its own: ask the apps now, so the letters
    /// are ready if it is pressed again (the apps' answers take a frame
    /// or two, and the second press takes longer).
    pub fn hints_ahead(&mut self) {
        if !self.settings.keyboard.hints || self.lock.locked() || self.hints.active() {
            return;
        }
        self.hints.ahead = Some((Instant::now(), None));
        self.fetch_hints(Purpose::Ahead, None);
    }

    /// A key, while hints may be showing: true if it was theirs.
    pub fn hints_key(&mut self, key: u32, pressed: bool) -> bool {
        const ESC: u32 = 1;
        const BACKSPACE: u32 = 14;
        // Ctrl, Shift, Alt, Super and Caps Lock pass (⌃⌃ again hides them).
        const MODIFIERS: [u32; 9] = [29, 97, 42, 54, 56, 100, 125, 126, 58];
        if !pressed {
            let Some(i) = self.hints.swallowed.iter().position(|k| *k == key) else { return false };
            self.hints.swallowed.swap_remove(i);
            return true;
        }
        if !self.hints.active() || MODIFIERS.contains(&key) {
            return false;
        }
        if self.lock.locked() || !self.settings.keyboard.hints {
            self.hints.close();
            return false;
        }
        let Some(pick) = (match key {
            ESC => {
                self.hints.close();
                None
            }
            BACKSPACE => {
                if self.hints.levels.len() > 1 && !self.hints.fetching {
                    self.hints.levels.pop();
                } else {
                    self.hints.close();
                }
                None
            }
            k => match letter(k) {
                Some(c) => Some(LETTERS.find(c).unwrap_or(usize::MAX)),
                // Anything else puts them away and does what it does.
                None => {
                    self.hints.close();
                    return false;
                }
            },
        }) else {
            self.hints.swallowed.push(key);
            return true;
        };
        self.hints.swallowed.push(key);
        if let Some(node) = self.hints.showing().get(pick).cloned() {
            self.pick_hint(node);
        }
        true
    }

    fn pick_hint(&mut self, node: Node) {
        match node {
            Node::Group { kids, .. } => {
                let level = expand(kids, LETTERS.len());
                self.hints.levels.push(level);
            }
            Node::Window { id, .. } => {
                let Some(w) = self.window_by_id(id) else { return self.hints.close() };
                self.restore(&w);
                self.fetch_hints(Purpose::Window(id), Some(w));
            }
            Node::Leaf { role, pid, node, .. } if FOCUSED.contains(&role.as_str()) => {
                self.hints.close();
                std::thread::spawn(move || focus(pid, node));
            }
            Node::Leaf { rect, .. } => {
                self.hints.close();
                use olo_protocol::SyntheticEvent as S;
                let back = self.pointer.current_location();
                let (x, y) = rect.centre();
                self.synthetic_input(S::Move { x, y });
                self.synthetic_input(S::Button { button: 0x110, pressed: true });
                self.synthetic_input(S::Button { button: 0x110, pressed: false });
                // The pointer goes back once the app has seen the click
                // there (a menu acts on the item under it), if it hasn't
                // been moved since.
                let _ = self.handle.insert_source(Timer::from_duration(SETTLE), move |_, _, state| {
                    let at = state.pointer.current_location();
                    if (at.x - x).abs() < 0.5 && (at.y - y).abs() < 0.5 {
                        state.synthetic_input(S::Move { x: back.x, y: back.y });
                    }
                    TimeoutAction::Drop
                });
                // A menu it opened is lettered next (it may have the
                // keyboard, so the window is the one clicked).
                let window = self.space.element_under((x, y)).map(|(w, _)| w.clone());
                let generation = self.hints.generation;
                let _ = self.handle.insert_source(Timer::from_duration(SETTLE), move |_, _, state| {
                    if state.hints.generation == generation && !state.hints.active() {
                        state.fetch_hints(Purpose::Menu, window.clone());
                    }
                    TimeoutAction::Drop
                });
            }
        }
    }

    /// The places to ask about, and their surfaces: the focused window
    /// (or `window`) and the layer surfaces showing (Deskbar, the
    /// desktop; not one hidden behind windows, whose frames, and so its
    /// answer, come once a second).
    fn hint_places(&self, window: Option<&WindowElement>) -> Vec<(Place, Option<WlSurface>)> {
        let mut places = Vec::new();
        let infos = self.window_infos_with_frames();
        // The focused window, or (a menu has the keyboard) the front one.
        let want = window
            .map(|w| w.id())
            .or_else(|| infos.iter().find(|i| i.focused).map(|i| i.id))
            .or_else(|| self.space.elements().next_back().map(|w| w.id()));
        for i in infos.iter().filter(|i| Some(i.id) == want) {
            if let (Some(pid), Some(c)) = (i.pid, i.content) {
                let content = Rect { x: c.x as f64, y: c.y as f64, w: c.width as f64, h: c.height as f64 };
                let surface = self.window_by_id(i.id).and_then(|w| w.wl_surface().map(|s| s.into_owned()));
                let what = What::Window { id: i.id, title: i.title.clone(), content };
                places.push((Place { pid: pid as u32, id: surface.as_ref().map(|s| s.id().protocol_id()), what }, surface));
            }
        }
        for output in self.space.outputs() {
            let Some(geo) = self.space.output_geometry(output) else { continue };
            let map = layer_map_for_output(output);
            // Deskbar, the desktop's icons, notifications…
            for l in map.layers() {
                let Some(g) = map.layer_geometry(l) else { continue };
                let shown = with_states(l.wl_surface(), |states| surface_primary_scanout_output(l.wl_surface(), states)).is_some();
                if !shown {
                    continue;
                }
                let Some(client) = self.display_handle.get_client(l.wl_surface().id()).ok() else { continue };
                let Some(pid) = crate::state::client_pid(&client, &self.display_handle) else { continue };
                let rect = Rect { x: (geo.loc.x + g.loc.x) as f64, y: (geo.loc.y + g.loc.y) as f64, w: g.size.w as f64, h: g.size.h as f64 };
                let what = What::Layer { namespace: l.namespace().to_owned(), rect };
                places.push((Place { pid: pid as u32, id: Some(l.wl_surface().id().protocol_id()), what }, Some(l.wl_surface().clone())));
            }
        }
        places
    }

    /// Would a click on `node` reach `owner` (and not a window in front of
    /// it)? An open menu is in front of everything.
    fn clicks_reach(&self, owner: Option<&WlSurface>, node: &Node) -> bool {
        if matches!(node, Node::Leaf { role, .. } if role == "MenuItem") {
            return true;
        }
        let (x, y) = node.rect().centre();
        let Some((under, _)) = self.surface_under((x, y).into()) else { return false };
        let Some(mut s) = under.wl_surface().map(|s| s.into_owned()) else { return false };
        // Up from subsurfaces and popups to the window or layer surface.
        for _ in 0..16 {
            if let Some(parent) = smithay::wayland::compositor::get_parent(&s) {
                s = parent;
            } else if let Some(root) = self.popups.find_popup(&s).and_then(|p| smithay::desktop::find_popup_root_surface(&p).ok()) {
                s = root;
            } else {
                break;
            }
        }
        owner == Some(&s)
    }

    /// The other windows showing on this workspace, by their tabs.
    fn hint_windows(&self, except: Option<u64>) -> Vec<Node> {
        let mut out = Vec::new();
        // Front first.
        for w in self.space.elements().rev() {
            let meta = w.meta();
            if Some(meta.id) == except || meta.minimized.get() || meta.workspace.get().is_some_and(|ws| ws != self.workspace) {
                continue;
            }
            let Some(at) = self.space.element_location(w) else { continue };
            let rect = if w.decorated() {
                let tab = w.decoration().tab_layout(&w.title(), w.content_size()).rect();
                Rect { x: (at.x + tab.loc.x) as f64, y: (at.y + tab.loc.y) as f64, w: tab.size.w as f64, h: tab.size.h as f64 }
            } else {
                let geo = smithay::desktop::space::SpaceElement::geometry(w);
                Rect { x: at.x as f64, y: at.y as f64, w: geo.size.w as f64, h: geo.size.h.min(24) as f64 }
            };
            out.push(Node::Window { id: meta.id, rect, name: w.title() });
        }
        out
    }

    /// Ask the apps what they show and make their nodes (on worker
    /// threads, one an app), then letter it.
    fn fetch_hints(&mut self, purpose: Purpose, window: Option<WindowElement>) {
        self.hints.generation += 1;
        let generation = self.hints.generation;
        // Looking for a menu after a click, or ahead of a second press,
        // leaves the keys alone meanwhile (they may be for what was
        // clicked, or anything).
        self.hints.fetching = !matches!(purpose, Purpose::Menu | Purpose::Ahead);
        let (places, surfaces): (Vec<Place>, Vec<Option<WlSurface>>) = self.hint_places(window.as_ref()).into_iter().unzip();
        let mut by_pid: HashMap<u32, Vec<(usize, Place)>> = HashMap::new();
        for (i, p) in places.iter().enumerate() {
            by_pid.entry(p.pid).or_default().push((i, p.clone()));
        }
        let asked = by_pid.len();
        let (tx, rx) = mpsc::channel();
        for (pid, mine) in by_pid {
            let tx = tx.clone();
            // One that can't start is waited for until the deadline.
            let _ = std::thread::Builder::new().name("hints".into()).spawn(move || {
                let _ = tx.send(ask_and_build(pid, mine));
            });
        }
        let start = Instant::now();
        let deadline = start + WAIT;
        let mut answers: HashMap<usize, Built> = HashMap::new();
        let mut answered = 0;
        let _ = self.handle.insert_source(Timer::immediate(), move |_, _, state| {
            if state.hints.generation != generation {
                return TimeoutAction::Drop;
            }
            while let Ok(built) = rx.try_recv() {
                answers.extend(built);
                answered += 1;
            }
            if answered < asked && Instant::now() < deadline {
                return TimeoutAction::ToDuration(POLL);
            }
            tracing::debug!(?purpose, ms = start.elapsed().as_secs_f64() * 1000.0, answered, asked, "keyboard hints: the apps answered");
            state.hints_arrived(purpose, &places, &surfaces, std::mem::take(&mut answers));
            TimeoutAction::Drop
        });
    }

    fn hints_arrived(&mut self, purpose: Purpose, places: &[Place], surfaces: &[Option<WlSurface>], mut answers: HashMap<usize, Built>) {
        let start = Instant::now();
        let mut content = Vec::new();
        let mut layers = Vec::new();
        for (i, place) in places.iter().enumerate() {
            let Some((nodes, title)) = answers.remove(&i) else { continue };
            let nodes = prune(nodes, &|n| self.clicks_reach(surfaces[i].as_ref(), n));
            match &place.what {
                What::Window { .. } => content.extend(nodes),
                What::Layer { namespace, .. } if !nodes.is_empty() => {
                    let name = Some(title).filter(|t| !t.is_empty()).unwrap_or_else(|| namespace.clone());
                    layers.push(Node::layer(name, nodes));
                }
                What::Layer { .. } => {}
            }
        }
        let budget = LETTERS.len();
        let level = match purpose {
            Purpose::Start | Purpose::Ahead => {
                let focused = places.iter().find_map(|p| match p.what {
                    What::Window { id, .. } => Some(id),
                    _ => None,
                });
                let mut others = self.hint_windows(focused);
                others.extend(layers);
                first_level(content, others, budget)
            }
            Purpose::Window(_) => expand(content, budget),
            Purpose::Menu => {
                let menu = |n: &Node| matches!(n, Node::Leaf { role, .. } if role == "MenuItem");
                let mut items: Vec<Node> = Vec::new();
                content.into_iter().chain(layers).for_each(|n| n.flatten(&mut items));
                items.retain(menu);
                expand(items, budget)
            }
        };
        tracing::debug!(?purpose, ms = start.elapsed().as_secs_f64() * 1000.0, letters = level.len(), "keyboard hints: lettered");
        if purpose == Purpose::Ahead {
            match &mut self.hints.ahead {
                // Control pressed again meanwhile: they're wanted now.
                Some(_) if self.hints.fetching => {
                    self.hints.ahead = None;
                    self.hints.fetching = false;
                }
                // Kept for a second press.
                Some((_, ready)) => {
                    *ready = Some(level);
                    return;
                }
                None => return,
            }
        } else {
            self.hints.fetching = false;
        }
        if level.is_empty() {
            // (A click that opened no menu is the usual case.)
            if purpose != Purpose::Menu {
                tracing::debug!(?purpose, "nothing to hint");
                self.hints.close();
            }
            return;
        }
        // A window's content comes after the level that picked it.
        if !matches!(purpose, Purpose::Window(_)) {
            self.hints.levels.clear();
        }
        self.hints.levels.push(level);
        self.hints.shown();
    }
}

// ---------------------------------------------------------------------------
// Drawing.

/// The badges and group outlines, each its own buffer (an element's id is
/// its buffer's).
#[derive(Default)]
pub struct Overlay {
    badges: HashMap<char, (MemoryRenderBuffer, Size<i32, Physical>)>,
    lines: Vec<MemoryRenderBuffer>,
    /// The scale and theme generation the buffers were drawn for.
    key: (i64, u64),
}

impl Overlay {
    /// Badges in front of group outlines.
    fn elements<R>(&mut self, renderer: &mut R, showing: &[Node], output: Rectangle<i32, Logical>, scale: f64) -> Vec<Exact<MemoryRenderBufferRenderElement<R>>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if showing.is_empty() {
            return Vec::new();
        }
        let key = ((scale * 1000.0).round() as i64, theme::generation());
        if self.key != key {
            self.badges.clear();
            self.lines.clear();
            self.key = key;
        }
        let palette = theme::palette();
        let out = Rect { x: output.loc.x as f64, y: output.loc.y as f64, w: output.size.w as f64, h: output.size.h as f64 };
        let phys = |x: f64, y: f64| -> Point<f64, Physical> { (((x - out.x) * scale).round(), ((y - out.y) * scale).round()).into() };
        let mut badges = Vec::new();
        let mut lines = Vec::new();
        let mut line = 0;
        for (i, node) in showing.iter().enumerate() {
            let r = node.rect();
            let on = |x: f64, y: f64| out.contains_point((x, y));
            let c = label(i);
            let (buffer, size) = self.badges.entry(c).or_insert_with(|| badge(&palette, c, scale));
            let (bw, bh) = (size.w as f64 / scale, size.h as f64 / scale);
            // Its left edge, middle height; a group's and window's top left.
            let (mut bx, mut by) = match node {
                Node::Leaf { .. } => (r.x, r.y + (r.h - bh) / 2.0),
                // Over the close box, clear of the title.
                Node::Window { .. } => (r.x + 3.0, r.y + (r.h - bh) / 2.0),
                Node::Group { .. } => (r.x - 2.0, r.y - 2.0),
            };
            if !on(bx + bw / 2.0, by + bh / 2.0) && !on(r.centre().0, r.centre().1) {
                continue;
            }
            bx = bx.clamp(out.x, out.x + out.w - bw);
            by = by.clamp(out.y, out.y + out.h - bh);
            badges.extend(crate::render::picture(renderer, phys(bx, by), buffer, (size.w, size.h), None, Kind::Unspecified));
            if let Node::Group { outline: true, .. } = node {
                // Corners to the nearest device pixel, and hairlines
                // between them (whole pixels, as thick on every side).
                let a = phys(r.x - 2.0, r.y - 2.0).to_i32_round::<i32>();
                let z = phys(r.x + r.w + 2.0, r.y + r.h + 2.0).to_i32_round::<i32>();
                let t = (scale.floor() as i32).max(1);
                let (w, h) = (z.x - a.x, z.y - a.y);
                // Top, bottom, left, right.
                for (x, y, w, h) in [(a.x, a.y, w, t), (a.x, z.y - t, w, t), (a.x, a.y, t, h), (z.x - t, a.y, t, h)] {
                    if self.lines.len() <= line {
                        self.lines.push(solid(palette.keyboard_navigation));
                    }
                    let rect = Rectangle::new((x, y).into(), (w.max(1), h.max(1)).into());
                    lines.extend(crate::render::exact(renderer, rect, &self.lines[line], (1, 1), None, Kind::Unspecified));
                    line += 1;
                }
            }
        }
        badges.extend(lines);
        badges
    }
}

fn solid(c: [u8; 4]) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(&c, Fourcc::Abgr8888, (1, 1), 1, Transform::Normal, Some(vec![Rectangle::from_size((1, 1).into())]))
}

/// A BeOS-yellow key cap with the letter on it, at `s` device pixels a
/// logical one.
fn badge(p: &olo_theme::Palette, c: char, s: f64) -> (MemoryRenderBuffer, Size<i32, Physical>) {
    let pm = badge_pixmap(p, c, s);
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let bsize: Size<i32, Buffer> = (w, h).into();
    let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, bsize, 1, Transform::Normal, Some(vec![Rectangle::from_size(bsize)]));
    (buffer, (w, h).into())
}

/// A hint's yellow badge with `c` on it, at scale `s` (Twitcher letters
/// its cells with the same).
pub fn badge_pixmap(p: &olo_theme::Palette, c: char, s: f64) -> Pixmap {
    let px = |l: f64| (l * s).round() as i32;
    let u = px(1.0).max(1);
    let label = c.to_ascii_uppercase().to_string();
    let size = FONT * s as f32;
    let tw = text::measure(&label, size, true).ceil() as i32;
    let h = px(BADGE_H as f64);
    let w = (tw + 2 * px(BADGE_PAD as f64)).max(h);
    let mut pm = Pixmap::new(w as u32, h as u32).expect("hint pixmap");
    fill(&mut pm, 0, 0, w, h, p.frame_focused[0]);
    fill(&mut pm, u, u, w - 2 * u, h - 2 * u, p.tab_focused);
    fill(&mut pm, u, u, w - 2 * u, u, p.tab_focused_light);
    fill(&mut pm, u, u, u, h - 2 * u, p.tab_focused_light);
    fill(&mut pm, u, h - 2 * u, w - 2 * u, u, p.tab_focused_shadow);
    fill(&mut pm, w - 2 * u, u, u, h - 2 * u, p.tab_focused_shadow);
    let y = text::top_for_centre(h as f32 / 2.0, size, true);
    text::draw(&mut pm, &label, ((w - tw) as f32 / 2.0).round(), y, size, true, p.tab_text_focused);
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u64, parent: Option<u64>, role: &str, name: Option<&str>, (x, y, w, h): (f64, f64, f64, f64), clickable: bool) -> UiNode {
        UiNode { id, parent, role: role.into(), name: name.map(Into::into), x, y, width: w, height: h, enabled: true, clickable }
    }

    fn tree(nodes: Vec<UiNode>) -> WindowTree {
        WindowTree { title: "Prefs".into(), width: 400.0, height: 300.0, layer: None, surface: None, nodes }
    }

    fn names(level: &[Node]) -> Vec<String> {
        level
            .iter()
            .map(|n| match n {
                Node::Leaf { name, .. } => name.clone().unwrap_or_default(),
                Node::Group { name, kids, .. } => format!("{}[{}]", name.clone().unwrap_or_default(), kids.len()),
                Node::Window { name, .. } => format!("<{name}>"),
            })
            .collect()
    }

    fn leaf(name: &str) -> Node {
        Node::Leaf { rect: Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }, name: Some(name.into()), role: "Button".into(), pid: 1, node: 0 }
    }

    #[test]
    fn controls_in_their_boxes() {
        let t = tree(vec![
            n(1, None, "Window", None, (0.0, 0.0, 400.0, 300.0), false),
            n(2, Some(1), "GenericContainer", Some("Key Repeat"), (0.0, 0.0, 0.0, 0.0), false),
            n(3, Some(2), "Slider", Some("Delay"), (10.0, 20.0, 100.0, 20.0), true),
            n(4, Some(2), "Slider", Some("Rate"), (10.0, 50.0, 100.0, 20.0), true),
            n(5, Some(1), "GenericContainer", Some("Mac Keys"), (0.0, 0.0, 0.0, 0.0), false),
            n(6, Some(5), "CheckBox", Some("Command"), (10.0, 120.0, 100.0, 20.0), true),
            n(7, Some(6), "Label", Some("Command"), (30.0, 120.0, 80.0, 20.0), false),
            // Disabled, and scrolled out of the window: not lettered.
            n(8, Some(5), "CheckBox", Some("Off"), (10.0, 150.0, 100.0, 20.0), true),
            n(9, Some(5), "Button", Some("Below"), (10.0, 900.0, 100.0, 20.0), true),
        ]);
        let mut t = t;
        t.nodes[7].enabled = false;
        let built = build(&t, (100.0, 50.0), 1);
        assert_eq!(names(&built), ["Key Repeat[2]", "Command"]);
        assert_eq!(built[1].rect(), Rect { x: 110.0, y: 170.0, w: 100.0, h: 20.0 });
        // Few enough: everything at once.
        assert_eq!(names(&expand(built.clone(), 26)), ["Delay", "Rate", "Command"]);
        // Too many: by box.
        assert_eq!(names(&expand(built, 2)), ["Key Repeat[2]", "Command"]);
    }

    #[test]
    fn a_background_that_takes_clicks_is_not_a_target() {
        let t = tree(vec![
            n(1, None, "Window", None, (0.0, 0.0, 400.0, 300.0), false),
            n(2, Some(1), "Clickable", None, (0.0, 0.0, 400.0, 300.0), true),
            n(3, Some(1), "Clickable", Some("a.txt"), (10.0, 10.0, 60.0, 60.0), true),
            n(4, Some(1), "Clickable", Some("b.txt"), (80.0, 10.0, 60.0, 60.0), true),
        ]);
        assert_eq!(names(&build(&t, (0.0, 0.0), 1)), ["a.txt", "b.txt"]);
    }

    #[test]
    fn a_control_around_its_own_field_is_one() {
        let t = tree(vec![
            n(1, None, "Window", None, (0.0, 0.0, 400.0, 300.0), false),
            n(2, Some(1), "ComboBox", Some("Match"), (10.0, 10.0, 175.0, 24.0), true),
            n(3, Some(1), "ComboBox", Some("Match"), (54.0, 10.0, 131.0, 24.0), true),
            // A row with a check box in it: both.
            n(4, Some(1), "Row", Some("a.txt"), (0.0, 100.0, 400.0, 20.0), true),
            n(5, Some(1), "CheckBox", Some("Done"), (4.0, 102.0, 16.0, 16.0), true),
        ]);
        let built = build(&t, (0.0, 0.0), 1);
        assert_eq!(names(&built), ["Match", "a.txt", "Done"]);
        assert_eq!(built[0].rect().x, 54.0);
    }

    #[test]
    fn an_open_menu_is_all_there_is() {
        let t = tree(vec![
            n(1, None, "Window", None, (0.0, 0.0, 400.0, 300.0), false),
            n(2, Some(1), "Clickable", Some("File"), (0.0, 0.0, 40.0, 20.0), true),
            n(3, Some(1), "Button", Some("OK"), (300.0, 250.0, 60.0, 20.0), true),
            // Hanging below the window.
            n(4, None, "MenuItem", Some("New"), (0.0, 20.0, 120.0, 20.0), true),
            n(5, None, "MenuItem", Some("Quit"), (0.0, 290.0, 120.0, 20.0), true),
        ]);
        assert_eq!(names(&build(&t, (0.0, 0.0), 1)), ["New", "Quit"]);
        // A submenu open from it: its items first.
        let mut t = t;
        t.nodes.push(n(6, None, "MenuItem", Some("Recent"), (120.0, 40.0, 90.0, 20.0), true));
        assert_eq!(names(&build(&t, (0.0, 0.0), 1)), ["Recent", "New", "Quit"]);
    }

    #[test]
    fn groups_open_smallest_first_and_runs_share_a_letter() {
        let g = |name: &str, k: usize| Node::group(Some(name.into()), (0..k).map(|i| leaf(&format!("{name}{i}"))).collect());
        let level = expand(vec![g("a", 3), g("b", 20), g("c", 5)], 10);
        assert_eq!(names(&level), ["a0", "a1", "a2", "b[20]", "c0", "c1", "c2", "c3", "c4"]);
        // A lone group opens whatever its size; 60 is 9 letters, then runs of 3.
        let level = expand(vec![g("x", 60)], 26);
        assert_eq!(level.len(), 26);
        assert_eq!(names(&level[8..10]), ["x8", "[3]"]);
        assert_eq!(names(&level[25..]), ["[3]"]);
        assert_eq!(level.iter().map(Node::leaves).sum::<usize>(), 60);
        // 30 in 23: 16 letters and 7 pairs.
        let level = expand((0..30).map(|i| leaf(&i.to_string())).collect(), 23);
        assert_eq!(level.len(), 23);
        assert_eq!(level.iter().filter(|n| matches!(n, Node::Group { .. })).count(), 7);
    }

    #[test]
    fn content_first_then_windows_and_deskbar() {
        let content: Vec<Node> = (0..30).map(|i| leaf(&format!("c{i}"))).collect();
        let w = |id: u64| Node::Window { id, rect: Rect::default(), name: format!("w{id}") };
        let level = first_level(content.clone(), vec![w(1), w(2), Node::group(Some("Deskbar".into()), vec![leaf("d")])], 26);
        assert_eq!(level.len(), 26);
        assert_eq!(names(&level[23..]), ["<w1>", "<w2>", "Deskbar[1]"]);
        // Many windows share one letter.
        let many: Vec<Node> = (0..20).map(w).collect();
        let level = first_level(content, many, 26);
        assert_eq!(names(&level[25..]), ["Windows[20]"]);
        // Few enough: all of it lettered.
        let level = first_level(vec![leaf("c")], vec![w(1), Node::group(Some("Deskbar".into()), vec![leaf("d"), leaf("e")])], 26);
        assert_eq!(names(&level), ["c", "<w1>", "d", "e"]);
    }

    #[test]
    fn what_cannot_be_clicked_goes() {
        let g = Node::group(Some("box".into()), vec![leaf("a"), leaf("b")]);
        let level = prune(vec![g.clone(), leaf("c")], &|n| !matches!(n, Node::Leaf { name: Some(n), .. } if n == "b"));
        assert_eq!(names(&level), ["a", "c"]);
        assert_eq!(prune(vec![g], &|_| true).len(), 1);
    }

    #[test]
    fn letters_from_keys() {
        assert_eq!(letter(30), Some('a'));
        assert_eq!(letter(16), Some('q'));
        assert_eq!(letter(50), Some('m'));
        assert_eq!(letter(28), None);
        assert_eq!(LETTERS.len(), 26);
        assert!(('a'..='z').all(|c| LETTERS.contains(c)));
    }

    #[test]
    fn trees_from_the_kit_parse() {
        let json = r#"[{"title":"Preferences","width":480,"height":560,"active":true,"layer":"deskbar","nodes":[
            {"id":1,"role":"Button","name":"OK","x":1.5,"y":2,"width":30,"height":20,"enabled":true,"clickable":true,"checked":false}]}]"#;
        let trees: Vec<WindowTree> = serde_json::from_str(json).unwrap();
        assert_eq!(trees[0].layer.as_deref(), Some("deskbar"));
        let place = Place { pid: 1, id: Some(7), what: What::Layer { namespace: "deskbar".into(), rect: Rect::default() } };
        assert!(find(&trees, &place).is_some());
        assert_eq!(names(&build(&trees[0], (0.0, 0.0), 1)), ["OK"]);
        // A surface the app named is found by it, not by its name.
        let json = r#"[{"title":"Deskbar","width":480,"height":40,"layer":"deskbar","surface":9,"nodes":[]},
            {"title":"Deskbar","width":480,"height":40,"layer":"deskbar","surface":7,"nodes":[]}]"#;
        let trees: Vec<WindowTree> = serde_json::from_str(json).unwrap();
        assert_eq!(find(&trees, &place).and_then(|t| t.surface), Some(7));
    }
}
