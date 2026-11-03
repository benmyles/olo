//! Auto-tiling (a windowing mode, `modes`: Deskbar's tray, ⌃⌥⌘T,
//! `Request::SetAutoTiling`): while it is a display's mode on a workspace,
//! every window there is in a tile. A new one takes half of the window
//! that had the focus (split along its longer side, as bspwm and
//! Hyprland's dwindle do); a closed one's room goes to what was beside
//! it. Dialogs, alerts, fixed-size windows and windows tagged `floating`
//! (xdg-toplevel-tag, the kit's `WindowOptions::floating`) float over the
//! tiles instead, and anyone can float or tile a window by hand (the
//! window menu, ⌃⌥⌘F, `Request::SetFloating`).
//!
//! Each display of each workspace has a tree of splits ([`Node`]): every
//! split cuts its room in two, side by side or one above the other, at a
//! ratio kept as windows come and go. A group of windows is one tile (its
//! front window is the leaf). The tree is laid out (`layout`, O(windows),
//! minimum sizes kept) only when something changes: a window comes or
//! goes, the room changes (Deskbar, the gap and margin), or a command
//! changes the tree. Laying out moves each window as tiling commands do
//! (`set_frame`), or, for a size change of a window already drawn, as a
//! neighbour follows an edge drag (`follow`), so nothing jumps ahead of
//! its picture and nothing waits on a client.
//!
//! Where the tiles are too small for their windows' least sizes (each
//! new window halves a tile, whatever is in it), the tree is arranged
//! again so they all fit, if any arrangement does (`autofit`): splits
//! turned the other way, else every way of cutting the room searched;
//! else the one that overlaps least.
//!
//! The commands work on the tree: Tile Left (and the other halves,
//! quarters and thirds) gives the window that part with the rest tiled
//! around it; Zoom Vertically and Horizontally make it a full column or
//! row; Tile All balances the tree; dragging a tile onto another swaps
//! them, or onto one's edge puts it beside it; dragging an edge moves the
//! split to just where it is let go (no grid: the windows are as sized,
//! and the rest go on fitting around them as windows come and go). Zoom
//! still zooms a window over the tiles, and unzooming puts it back. ⌃⌘Z
//! undoes the last change to a tree (and turning auto-tiling on).
//!
//! A window that leaves for a while (hidden, on another workspace) leaves
//! a [`Kept`] place behind, which takes no room; back, it goes there. A
//! closed one does too, for a moment: an app re-executing itself after
//! an upgrade gets its windows back where they were. An in-place restart
//! keeps the trees themselves ([`SavedTree`], beside the window layout):
//! each tile's leaf names the saved places of its window (a group's,
//! all of them), so the next app_server starts with the same trees, the
//! same ratios and room held for every tile, and each window takes its
//! own tile back as it returns, in whatever order the apps come back
//! (an upgrade re-executes them all as app_server restarts). Without
//! saved trees (from an app_server before them), it makes them again from
//! where the windows are (their tiles' parts, saved with them), keeping
//! room for the ones not back yet.

use std::{
    cell::Cell,
    collections::HashMap,
    time::{Duration, Instant},
};

use olo_protocol::Tile;
use smithay::{
    desktop::WindowSurface,
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle},
};
use tracing::info;

use super::{
    WindowElement,
    tiling::{Axis, Part, Side, fraction_of, part, resizable},
    wm::PreZoom,
};
use crate::state::{AnvilState, Backend};

/// How near two edges are to count as one, in fractions of the area.
const TOL: f32 = 0.01;
/// How long room is kept for windows not back yet after a restart.
const RESTORING: Duration = Duration::from_secs(20);
/// How often everything is looked at closely while the windows stay put
/// (for what changes without moving one: a window's least or largest
/// size, a dialog hint set late).
const LOOK: Duration = Duration::from_millis(500);
/// How long a closed window's place waits for it to come back (an app
/// re-executing itself after an upgrade).
const CLOSED_KEEP: Duration = Duration::from_secs(20);

/// How a split divides its room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// Side by side: `a` on the left, `b` on the right.
    Columns,
    /// One above the other: `a` above, `b` below.
    Rows,
}

/// Whose place a [`Kept`] is.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    /// Nobody's yet: room a tiling command left, for the next window.
    Nobody,
    /// A window away for now (hidden, rolled away to another workspace).
    Window(u64),
    /// A window that closed, or isn't back yet after a restart: the next
    /// one like it that is put back where it was (`WindowMeta::put_back`).
    Like { app_id: String, title: String },
    /// A window not back yet after a restart, by its saved places
    /// (`Placement::slot`; a group's windows each have one): the one put
    /// back into any of them.
    Returning(Vec<u64>),
}

/// A place in the tree kept for a window.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub who: Who,
    /// It keeps its room; otherwise what is beside it has it meanwhile.
    pub holds: bool,
    /// Forgotten after this.
    pub until: Option<Instant>,
}

/// A display's tiles: a window, a kept place, or a split of the room
/// between two more (`ratio` of it to `a`).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Window(u64),
    Kept(Kept),
    Split { cut: Cut, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
    pub fn split(cut: Cut, ratio: f32, a: Node, b: Node) -> Node {
        Node::Split { cut, ratio, a: Box::new(a), b: Box::new(b) }
    }

    /// Room for the next window.
    pub fn hole() -> Node {
        Node::Kept(Kept { who: Who::Nobody, holds: true, until: None })
    }

    /// Room that is nobody's and goes to what is beside it.
    fn gap() -> Node {
        Node::Kept(Kept { who: Who::Nobody, holds: false, until: None })
    }

    fn is_gap(&self) -> bool {
        matches!(self, Node::Kept(Kept { who: Who::Nobody, holds: false, .. }))
    }

    /// Its windows, from the top left.
    pub fn windows(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.each_leaf(&mut |n| {
            if let Node::Window(id) = n {
                out.push(*id);
            }
        });
        out
    }

    pub(super) fn each_leaf(&self, f: &mut dyn FnMut(&Node)) {
        match self {
            Node::Split { a, b, .. } => {
                a.each_leaf(f);
                b.each_leaf(f);
            }
            leaf => f(leaf),
        }
    }

    fn each_leaf_mut(&mut self, f: &mut dyn FnMut(&mut Node)) {
        match self {
            Node::Split { a, b, .. } => {
                a.each_leaf_mut(f);
                b.each_leaf_mut(f);
            }
            leaf => f(leaf),
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        let mut found = false;
        self.each_leaf(&mut |n| found |= *n == Node::Window(id));
        found
    }

    pub(super) fn any_leaf(&self, f: &dyn Fn(&Node) -> bool) -> bool {
        let mut found = false;
        self.each_leaf(&mut |n| found |= f(n));
        found
    }

    /// Without the leaves `gone` picks: the room each had goes to what
    /// was beside it. Nothing, if that was all of them.
    pub fn without(self, gone: &dyn Fn(&Node) -> bool) -> Option<Node> {
        match self {
            Node::Split { cut, ratio, a, b } => match (a.without(gone), b.without(gone)) {
                (Some(a), Some(b)) => Some(Node::split(cut, ratio, a, b)),
                (a, b) => a.or(b),
            },
            leaf if gone(&leaf) => None,
            leaf => Some(leaf),
        }
    }

    /// The first leaf `pick` picks made `with`. Whether there was one.
    pub fn replace(&mut self, pick: &dyn Fn(&Node) -> bool, with: Node) -> bool {
        match self {
            Node::Split { a, b, .. } => a.replace(pick, with.clone()) || b.replace(pick, with),
            leaf if pick(leaf) => {
                *leaf = with;
                true
            }
            _ => false,
        }
    }

    /// The `n`th leaf (from 0) of those `pick` picks made `with`.
    pub fn replace_nth(&mut self, pick: &dyn Fn(&Node) -> bool, n: usize, with: Node) -> bool {
        let mut seen = 0;
        let mut done = false;
        self.each_leaf_mut(&mut |leaf| {
            if !done && pick(leaf) {
                if seen == n {
                    *leaf = with.clone();
                    done = true;
                }
                seen += 1;
            }
        });
        done
    }

    /// Window `id`'s tile split in two, `new` taking the second half (to
    /// the right or below), or the first.
    pub fn split_leaf(&mut self, id: u64, new: Node, cut: Cut, first: bool) -> bool {
        let pick = |n: &Node| *n == Node::Window(id);
        let (a, b) = if first { (new, Node::Window(id)) } else { (Node::Window(id), new) };
        self.replace(&pick, Node::split(cut, 0.5, a, b))
    }

    /// Windows `x` and `y` in each other's tiles.
    pub fn swap(&mut self, x: u64, y: u64) {
        self.each_leaf_mut(&mut |n| match n {
            Node::Window(id) if *id == x => *id = y,
            Node::Window(id) if *id == y => *id = x,
            _ => {}
        });
    }
}

/// The tree in brief, for the log: `[a | b]` side by side, `[a / b]`
/// one above the other, `·` kept places.
impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Node::Window(id) => write!(f, "{id}"),
            Node::Kept(k) => match &k.who {
                Who::Nobody => write!(f, "·"),
                Who::Window(id) => write!(f, "·{id}"),
                Who::Like { app_id, .. } => write!(f, "·{app_id}"),
                Who::Returning(slots) => write!(f, "·#{}", slots.first().copied().unwrap_or(0)),
            },
            Node::Split { cut, ratio, a, b } => write!(f, "[{a} {} {b}]{}", if *cut == Cut::Columns { "|" } else { "/" }, (ratio * 100.0).round()),
        }
    }
}

/// A tree as an in-place restart keeps it (`layout.rs` writes it beside
/// the window layout): its workspace and display, and its nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedTree {
    pub workspace: u32,
    pub output: String,
    pub tree: SavedNode,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedNode {
    /// A tile: the saved places of its window (or group).
    Slots(Vec<u64>),
    /// A closed window's place, with nothing saved to name it by.
    Like {
        app_id: String,
        title: String,
    },
    /// Room nobody has: kept (`holds`), or going to what is beside it.
    Room {
        holds: bool,
    },
    Split {
        columns: bool,
        ratio: f32,
        a: Box<SavedNode>,
        b: Box<SavedNode>,
    },
}

impl SavedNode {
    /// `node` to keep: `slots` names each window's saved places, `like`
    /// a closed window's (None: its room is kept for one like it).
    pub fn of(node: &Node, slots: &dyn Fn(u64) -> Vec<u64>, like: &dyn Fn(&str, &str) -> Option<u64>) -> SavedNode {
        match node {
            Node::Window(id) | Node::Kept(Kept { who: Who::Window(id), .. }) => SavedNode::Slots(slots(*id)),
            Node::Kept(Kept { who: Who::Like { app_id, title }, .. }) => match like(app_id, title) {
                Some(slot) => SavedNode::Slots(vec![slot]),
                None => SavedNode::Like { app_id: app_id.clone(), title: title.clone() },
            },
            Node::Kept(Kept { who: Who::Returning(s), .. }) => SavedNode::Slots(s.clone()),
            Node::Kept(Kept { who: Who::Nobody, holds, .. }) => SavedNode::Room { holds: *holds },
            Node::Split { cut, ratio, a, b } => {
                SavedNode::Split { columns: *cut == Cut::Columns, ratio: *ratio, a: Box::new(SavedNode::of(a, slots, like)), b: Box::new(SavedNode::of(b, slots, like)) }
            }
        }
    }

    /// The tree to start with: every tile room held, until `until`, for
    /// the windows coming back.
    pub fn node(&self, until: Instant) -> Node {
        match self {
            SavedNode::Slots(s) if s.is_empty() => Node::gap(),
            SavedNode::Slots(s) => Node::Kept(Kept { who: Who::Returning(s.clone()), holds: true, until: Some(until) }),
            SavedNode::Like { app_id, title } => Node::Kept(Kept { who: Who::Like { app_id: app_id.clone(), title: title.clone() }, holds: true, until: Some(until) }),
            SavedNode::Room { holds } => Node::Kept(Kept { who: Who::Nobody, holds: *holds, until: None }),
            SavedNode::Split { columns, ratio, a, b } => Node::split(if *columns { Cut::Columns } else { Cut::Rows }, ratio.clamp(0.0, 1.0), a.node(until), b.node(until)),
        }
    }
}

/// What laying out a node needs from below: the least room it takes,
/// whether it takes any, and how many nodes it is.
#[derive(Debug, Clone, Copy)]
struct Measure {
    min: (f32, f32),
    shows: bool,
    len: usize,
}

/// Every node's [`Measure`], in pre-order.
fn measure(node: &Node, min: &dyn Fn(u64) -> (f32, f32), out: &mut Vec<Measure>) -> usize {
    let i = out.len();
    out.push(Measure { min: (0.0, 0.0), shows: false, len: 1 });
    out[i] = match node {
        Node::Window(id) => Measure { min: min(*id), shows: true, len: 1 },
        Node::Kept(k) => Measure { min: (0.0, 0.0), shows: k.holds, len: 1 },
        Node::Split { cut, a, b, .. } => {
            let ia = measure(a, min, out);
            let ib = measure(b, min, out);
            let (ma, mb) = (out[ia], out[ib]);
            let min = match (ma.shows, mb.shows) {
                (true, true) => match cut {
                    Cut::Columns => (ma.min.0 + mb.min.0, ma.min.1.max(mb.min.1)),
                    Cut::Rows => (ma.min.0.max(mb.min.0), ma.min.1 + mb.min.1),
                },
                (true, false) => ma.min,
                (false, true) => mb.min,
                (false, false) => (0.0, 0.0),
            };
            Measure { min, shows: ma.shows || mb.shows, len: 1 + ma.len + mb.len }
        }
    };
    i
}

/// `r` cut at `ratio` of it.
fn divide(cut: Cut, ratio: f32, (x, y, w, h): Part) -> (Part, Part) {
    match cut {
        Cut::Columns => ((x, y, w * ratio, h), (x + w * ratio, y, w * (1.0 - ratio), h)),
        Cut::Rows => ((x, y, w, h * ratio), (x, y + h * ratio, w, h * (1.0 - ratio))),
    }
}

/// Where along `cut` a part starts, and how long it is.
fn span((x, y, w, h): Part, cut: Cut) -> (f32, f32) {
    match cut {
        Cut::Columns => (x, w),
        Cut::Rows => (y, h),
    }
}

/// Each leaf that takes room (the windows, and the places that hold
/// theirs) and its part of the area, from the top left. `min` is each
/// window's least size in fractions of the area (gap included): a split
/// gives each side at least that, where both fit, whatever its ratio.
pub fn layout<'a>(node: &'a Node, min: &dyn Fn(u64) -> (f32, f32)) -> Vec<(&'a Node, Part)> {
    let mut m = Vec::new();
    measure(node, min, &mut m);
    let mut out = Vec::new();
    if m[0].shows {
        place(node, (0.0, 0.0, 1.0, 1.0), &m, 0, &mut out);
    }
    out
}

fn place<'a>(node: &'a Node, r: Part, m: &[Measure], i: usize, out: &mut Vec<(&'a Node, Part)>) {
    let Node::Split { cut, ratio, a, b } = node else {
        out.push((node, r));
        return;
    };
    let (ia, ib) = (i + 1, i + 1 + m[i + 1].len);
    match (m[ia].shows, m[ib].shows) {
        (true, false) => place(a, r, m, ia, out),
        (false, true) => place(b, r, m, ib, out),
        (false, false) => {}
        (true, true) => {
            let (ra, rb) = divide(*cut, laid_ratio(*cut, *ratio, r, m[ia], m[ib]), r);
            place(a, ra, m, ia, out);
            place(b, rb, m, ib, out);
        }
    }
}

/// The ratio a split of room `r` is laid out at: as it is, unless its two
/// sides' least sizes (`ma`, `mb`) hold it off that.
fn laid_ratio(cut: Cut, ratio: f32, r: Part, ma: Measure, mb: Measure) -> f32 {
    let (_, len) = span(r, cut);
    let pick = |mm: Measure| if cut == Cut::Columns { mm.min.0 } else { mm.min.1 };
    let (lo, hi) = (pick(ma) / len.max(1e-6), 1.0 - pick(mb) / len.max(1e-6));
    if lo <= hi {
        ratio.clamp(lo, hi)
    } else {
        // They don't both fit: each its share of what they want.
        pick(ma) / (pick(ma) + pick(mb)).max(1e-6)
    }
}

/// A place kept for a window that closed or isn't back yet (not one away
/// for now: minimized, or on another workspace).
fn closing(n: &Node) -> bool {
    matches!(n, Node::Kept(Kept { who: Who::Like { .. } | Who::Returning(_), .. }))
}

/// `node` with the ratios of `laid`, the same tree as it was laid out
/// when all its windows were there, split by split while their shapes
/// agree (its windows' places kept for them since don't change it).
pub fn with_ratios_of(node: &Node, laid: &Node) -> Node {
    match (node, laid) {
        (Node::Split { cut, a, b, .. }, Node::Split { cut: c, ratio, a: la, b: lb }) if cut == c => Node::split(*cut, *ratio, with_ratios_of(a, la), with_ratios_of(b, lb)),
        (n, _) => n.clone(),
    }
}

/// `node` with each split at the ratio it is laid out at (its windows'
/// least sizes may hold an edge off where it was dragged to): a tree kept
/// across a restart lays out the same before they are known again.
pub fn as_laid_out(node: &Node, min: &dyn Fn(u64) -> (f32, f32)) -> Node {
    fn go(node: &Node, r: Part, m: &[Measure], i: usize) -> Node {
        let Node::Split { cut, ratio, a, b } = node else { return node.clone() };
        let (ia, ib) = (i + 1, i + 1 + m[i + 1].len);
        let ratio = if m[ia].shows && m[ib].shows { laid_ratio(*cut, *ratio, r, m[ia], m[ib]) } else { *ratio };
        let (ra, rb) = divide(*cut, ratio, r);
        Node::split(*cut, ratio, go(a, ra, m, ia), go(b, rb, m, ib))
    }
    let mut m = Vec::new();
    measure(node, min, &mut m);
    go(node, (0.0, 0.0, 1.0, 1.0), &m, 0)
}

/// `ids` in a tree as even as it can be: halves (by count) side by side
/// across the longer side of the room (`room`, its width and height),
/// and so on down; across the other way where only that way leaves each
/// window its least size (`min`, in the room's units).
pub fn balanced(ids: &[u64], room: (f32, f32), min: &dyn Fn(u64) -> (f32, f32)) -> Option<Node> {
    match ids {
        [] => None,
        [id] => Some(Node::Window(*id)),
        _ => {
            let half = ids.len() / 2;
            let ratio = half as f32 / ids.len() as f32;
            let try_cut = |cut: Cut| {
                let ((aw, ah), (bw, bh)) = match cut {
                    Cut::Columns => ((room.0 * ratio, room.1), (room.0 * (1.0 - ratio), room.1)),
                    Cut::Rows => ((room.0, room.1 * ratio), (room.0, room.1 * (1.0 - ratio))),
                };
                let t = Node::split(cut, ratio, balanced(&ids[..half], (aw, ah), min)?, balanced(&ids[half..], (bw, bh), min)?);
                let over = overflow(&t, room, min);
                Some((t, over))
            };
            let (first, second) = if room.0 >= room.1 { (Cut::Columns, Cut::Rows) } else { (Cut::Rows, Cut::Columns) };
            let (t, over) = try_cut(first)?;
            if over <= 0.0 {
                return Some(t);
            }
            match try_cut(second) {
                Some((u, o)) if o < over => Some(u),
                _ => Some(t),
            }
        }
    }
}

/// How far `node`'s least size goes past `room` (0 if it fits), in the
/// room's units.
pub(super) fn overflow(node: &Node, room: (f32, f32), min: &dyn Fn(u64) -> (f32, f32)) -> f32 {
    let mut m = Vec::new();
    measure(node, min, &mut m);
    (m[0].min.0 - room.0).max(0.0) + (m[0].min.1 - room.1).max(0.0)
}

/// The way to cut a tile of `size` in two for a window with `adding` as
/// its least size beside one with `having`: across its longer side,
/// unless only the other way leaves them both room.
pub fn cut_for(size: (f32, f32), having: (f32, f32), adding: (f32, f32)) -> Cut {
    let columns = having.0 + adding.0 <= size.0;
    let rows = having.1 + adding.1 <= size.1;
    match (size.0 >= size.1, columns, rows) {
        (true, false, true) => Cut::Rows,
        (false, true, false) => Cut::Columns,
        (true, ..) => Cut::Columns,
        (false, ..) => Cut::Rows,
    }
}

fn clamp_part((x, y, w, h): Part) -> Part {
    let (l, t) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    let (r, b) = ((x + w).clamp(0.0, 1.0), (y + h).clamp(0.0, 1.0));
    (l, t, r - l, b - t)
}

/// `p` and `q` share some room (more than a sliver).
fn overlap(p: Part, q: Part) -> bool {
    let w = (p.0 + p.2).min(q.0 + q.2) - p.0.max(q.0);
    let h = (p.1 + p.3).min(q.1 + q.3) - p.1.max(q.1);
    w > TOL && h > TOL
}

fn covers(p: Part, r: Part) -> bool {
    (p.0 - r.0).abs() <= TOL && (p.1 - r.1).abs() <= TOL && (p.0 + p.2 - r.0 - r.2).abs() <= TOL && (p.1 + p.3 - r.1 - r.3).abs() <= TOL
}

/// The tree that has `items` (leaves and their parts of the area) where
/// they are: tiles that cut the area in two, and so on down, as tiling
/// left them. Room no item has goes to its neighbours. None if they
/// overlap, or aren't in such tiles.
pub fn from_parts(items: Vec<(Node, Part)>) -> Option<Node> {
    let items: Vec<(Node, Part)> = items.into_iter().map(|(n, p)| (n, clamp_part(p))).filter(|(_, p)| p.2 > TOL && p.3 > TOL).collect();
    guillotine((0.0, 0.0, 1.0, 1.0), items)?.without(&Node::is_gap)
}

fn guillotine(r: Part, items: Vec<(Node, Part)>) -> Option<Node> {
    if items.is_empty() {
        return Some(Node::gap());
    }
    if items.len() == 1 && covers(items[0].1, r) {
        return items.into_iter().next().map(|(n, _)| n);
    }
    let centre = |p: Part, cut: Cut| {
        let (s, l) = span(p, cut);
        s + l / 2.0
    };
    // Every line no item crosses; best one with items either side, then
    // the one nearest the middle.
    let mut best: Option<(bool, f32, Cut, f32)> = None;
    for cut in [Cut::Columns, Cut::Rows] {
        let (start, len) = span(r, cut);
        for (_, p) in &items {
            let (s, l) = span(*p, cut);
            for e in [s, s + l] {
                if e <= start + TOL || e >= start + len - TOL {
                    continue;
                }
                let crossed = items.iter().any(|(_, q)| {
                    let (s, l) = span(*q, cut);
                    s < e - TOL && s + l > e + TOL
                });
                if crossed {
                    continue;
                }
                let before = items.iter().filter(|(_, q)| centre(*q, cut) < e).count();
                let both = before > 0 && before < items.len();
                let off = (e - (start + len / 2.0)).abs() / len;
                if best.is_none_or(|(b, o, ..)| (both, -off) > (b, -o)) {
                    best = Some((both, off, cut, e));
                }
            }
        }
    }
    let (_, _, cut, e) = best?;
    let (start, len) = span(r, cut);
    let ratio = (e - start) / len;
    let (ra, rb) = divide(cut, ratio, r);
    let (before, after): (Vec<_>, Vec<_>) = items.into_iter().partition(|(_, q)| centre(*q, cut) < e);
    Some(Node::split(cut, ratio, guillotine(ra, before)?, guillotine(rb, after)?))
}

/// The tree with window `w` given `tile`'s part of the area and the rest
/// (`rest`, the tree without it) tiled around it. Where the rest is more
/// than one part (a quarter leaves three quarters as a half and a
/// quarter), the window nearest the smaller one (`centre`) goes there;
/// a part with nobody to fill it is room for the next window.
pub fn tiled_into(rest: Option<Node>, w: u64, tile: Tile, centre: &dyn Fn(u64) -> (f32, f32)) -> Node {
    use Cut::{Columns, Rows};
    const T: f32 = 1.0 / 3.0;
    let me = Node::Window(w);
    let whole = |rest: Option<Node>| rest.unwrap_or_else(Node::hole);
    match tile {
        Tile::Left => Node::split(Columns, 0.5, me, whole(rest)),
        Tile::Right => Node::split(Columns, 0.5, whole(rest), me),
        Tile::Top => Node::split(Rows, 0.5, me, whole(rest)),
        Tile::Bottom => Node::split(Rows, 0.5, whole(rest), me),
        Tile::LeftThird => Node::split(Columns, T, me, whole(rest)),
        Tile::RightThird => Node::split(Columns, 2.0 * T, whole(rest), me),
        Tile::LeftTwoThirds => Node::split(Columns, 2.0 * T, me, whole(rest)),
        Tile::RightTwoThirds => Node::split(Columns, T, whole(rest), me),
        Tile::CenterThird => {
            let (left, right) = take_nearest(rest, (T / 2.0, 0.5), centre);
            Node::split(Columns, T, left, Node::split(Columns, 0.5, me, right))
        }
        Tile::TopLeft | Tile::TopRight | Tile::BottomLeft | Tile::BottomRight => {
            let left = matches!(tile, Tile::TopLeft | Tile::BottomLeft);
            let top = matches!(tile, Tile::TopLeft | Tile::TopRight);
            let small_at = (if left { 0.25 } else { 0.75 }, if top { 0.75 } else { 0.25 });
            let (small, big) = take_nearest(rest, small_at, centre);
            let column = if top { Node::split(Rows, 0.5, me, small) } else { Node::split(Rows, 0.5, small, me) };
            if left { Node::split(Columns, 0.5, column, big) } else { Node::split(Columns, 0.5, big, column) }
        }
        Tile::Cell => match rest {
            Some(rest) => Node::split(Columns, 0.5, rest, me),
            None => me,
        },
    }
}

/// The room (fractions of the area's width and height) the rest have
/// beside a window tiled into `tile`: the most of it in one piece.
fn rest_room(tile: Tile) -> (f32, f32) {
    match tile {
        Tile::Left | Tile::Right | Tile::TopLeft | Tile::TopRight | Tile::BottomLeft | Tile::BottomRight => (0.5, 1.0),
        Tile::Top | Tile::Bottom => (1.0, 0.5),
        Tile::LeftThird | Tile::RightThird => (2.0 / 3.0, 1.0),
        Tile::CenterThird | Tile::LeftTwoThirds | Tile::RightTwoThirds => (1.0 / 3.0, 1.0),
        Tile::Cell => (1.0, 1.0),
    }
}

/// The window of `rest` nearest `to` on its own and the rest of `rest`,
/// if it has two or more; else room for the next window, and `rest`.
fn take_nearest(rest: Option<Node>, to: (f32, f32), centre: &dyn Fn(u64) -> (f32, f32)) -> (Node, Node) {
    let Some(rest) = rest else { return (Node::hole(), Node::hole()) };
    let ids = rest.windows();
    if ids.len() < 2 {
        return (Node::hole(), rest);
    }
    let far = |id: &u64| {
        let (x, y) = centre(*id);
        (x - to.0).powi(2) + (y - to.1).powi(2)
    };
    let near = *ids.iter().min_by(|a, b| far(a).total_cmp(&far(b))).unwrap();
    let big = rest.without(&|n| *n == Node::Window(near)).unwrap_or_else(Node::hole);
    (Node::Window(near), big)
}

/// Each split's ratio as the windows' parts (`part_of`) have it, after
/// their edges were dragged: where they were let go.
pub fn take_ratios(node: &mut Node, part_of: &dyn Fn(u64) -> Option<Part>) {
    let mut boxes = Vec::new();
    bounds(node, part_of, &mut boxes);
    let mut i = 0;
    set_ratios(node, (0.0, 0.0, 1.0, 1.0), &boxes, &mut i);
}

/// Each node's windows' bounds (pre-order), and how many nodes it is.
fn bounds(node: &Node, part_of: &dyn Fn(u64) -> Option<Part>, out: &mut Vec<(Option<Part>, usize)>) -> usize {
    let i = out.len();
    out.push((None, 1));
    out[i] = match node {
        Node::Window(id) => (part_of(*id), 1),
        Node::Kept(_) => (None, 1),
        Node::Split { a, b, .. } => {
            let ia = bounds(a, part_of, out);
            let ib = bounds(b, part_of, out);
            let (ba, bb) = (out[ia], out[ib]);
            let both = match (ba.0, bb.0) {
                (Some(p), Some(q)) => {
                    let (l, t) = (p.0.min(q.0), p.1.min(q.1));
                    let (r, b) = ((p.0 + p.2).max(q.0 + q.2), (p.1 + p.3).max(q.1 + q.3));
                    Some((l, t, r - l, b - t))
                }
                (p, q) => p.or(q),
            };
            (both, 1 + ba.1 + bb.1)
        }
    };
    i
}

fn set_ratios(node: &mut Node, r: Part, boxes: &[(Option<Part>, usize)], i: &mut usize) {
    let me = *i;
    *i += 1;
    let Node::Split { cut, ratio, a, b } = node else { return };
    let (ia, ib) = (me + 1, me + 1 + boxes[me + 1].1);
    let (start, len) = span(r, *cut);
    let edge = |p: Option<Part>, end: bool| {
        p.map(|p| {
            let (s, l) = span(p, *cut);
            if end { s + l } else { s }
        })
    };
    let e = match (edge(boxes[ia].0, true), edge(boxes[ib].0, false)) {
        (Some(x), Some(y)) => Some((x + y) / 2.0),
        (x, y) => x.or(y),
    };
    if let Some(e) = e {
        let was = start + len * *ratio;
        if (e - was).abs() > 0.003 {
            *ratio = ((e - start) / len).clamp(0.05, 0.95);
        }
    }
    let (ra, rb) = divide(*cut, *ratio, r);
    set_ratios(a, ra, boxes, i);
    *i = ib;
    set_ratios(b, rb, boxes, i);
}

/// The tree with window `w` a full column (`Axis::Vertical`) or row
/// beside the rest, on the side its middle (`at`, fractions) is on, as
/// wide (or tall) as it is (`size`), within reason: one that already
/// spans the screen that way takes half.
pub fn along(rest: Option<Node>, w: u64, axis: Axis, at: (f32, f32), size: (f32, f32)) -> Node {
    let me = Node::Window(w);
    let Some(rest) = rest else { return me };
    let (cut, centre, len) = match axis {
        Axis::Vertical => (Cut::Columns, at.0, size.0),
        Axis::Horizontal => (Cut::Rows, at.1, size.1),
    };
    let len = if len > 0.8 { 0.5 } else { len.max(0.2) };
    if centre < 0.5 { Node::split(cut, len, me, rest) } else { Node::split(cut, 1.0 - len, rest, me) }
}

/// Where in a tile a dropped window goes: into its place (the middle),
/// or beside it on a side (its outer quarters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    Swap,
    Beside(Side),
}

/// Where `p` (fractions of the area) is in tile `r`.
pub fn drop_at(r: Part, p: (f32, f32)) -> Drop {
    let (fx, fy) = ((p.0 - r.0) / r.2.max(1e-6), (p.1 - r.1) / r.3.max(1e-6));
    let off = (fx - 0.5).abs().max((fy - 0.5).abs());
    if off < 0.25 {
        return Drop::Swap;
    }
    Drop::Beside(if (fx - 0.5).abs() >= (fy - 0.5).abs() {
        if fx < 0.5 { Side::Left } else { Side::Right }
    } else if fy < 0.5 {
        Side::Top
    } else {
        Side::Bottom
    })
}

/// Floated or tiled by hand (`Request::SetFloating`, ⌃⌥⌘F), over what
/// app_server would judge.
#[derive(Default)]
struct Float(Cell<Option<bool>>);

/// What app_server knew of a tile's window, for when it is gone: its app
/// and title (a closed window's place waits for one like it) and group.
#[derive(Debug, Clone)]
struct Known {
    app_id: String,
    title: String,
    group: Option<u64>,
}

/// What ⌃⌘Z puts back: a tree as it was, or the windowing mode there was
/// before auto-tiling.
#[derive(Debug, Clone)]
enum Undo {
    Tree((u32, String), Option<Node>),
    TurnedOn(u32, Vec<String>, olo_protocol::WindowingMode),
}

/// app_server's auto-tiling: a tree per workspace and display (by
/// connector) that has it on.
#[derive(Debug)]
pub struct AutoTiler {
    trees: HashMap<(u32, String), Node>,
    /// Each tree as last laid out with all its windows there
    /// ([`as_laid_out`]): what a restart keeps, though windows are
    /// closing as it comes (an upgrade re-executes their apps).
    laid: HashMap<(u32, String), Node>,
    known: HashMap<u64, Known>,
    /// What each display's tiles were laid out for: its room, and its
    /// windows with their least frames (most learnt only as one first
    /// draws, after its tile was given it).
    seen: HashMap<String, (Rectangle<i32, Logical>, Vec<(u64, smithay::utils::Size<i32, Logical>)>)>,
    /// Lay out again on the next tick, whatever changed.
    pub dirty: bool,
    undo: Option<Undo>,
    /// Windows zoomed along one axis, and the tree before (to go back to
    /// while it has the same windows).
    along: HashMap<u64, (Axis, Node)>,
    /// A tile being dragged by its tab.
    moving: Option<WindowElement>,
    started: Instant,
    /// When a kept place is next forgotten (a timer is set for it).
    next: Option<Instant>,
    /// The windows on screen at the last look (which, stacked how, where,
    /// how big) and the displays' room, and when everything was last
    /// looked at closely: while these stay the same, a turn of the loop
    /// costs next to nothing.
    glance: (Vec<olo_protocol::DisplayWorkspace>, Vec<(u64, Rectangle<i32, Logical>)>, Vec<Rectangle<i32, Logical>>),
    looked: Option<Instant>,
    /// The trees of displays unplugged, by their monitor's key, with
    /// their workspaces: back as they were when it's plugged in again
    /// (`leave`, `come_back`), its windows into their own tiles.
    away: HashMap<String, Vec<(u32, Node)>>,
}

impl Default for AutoTiler {
    fn default() -> Self {
        AutoTiler {
            trees: HashMap::new(),
            laid: HashMap::new(),
            known: HashMap::new(),
            seen: HashMap::new(),
            dirty: false,
            undo: None,
            along: HashMap::new(),
            moving: None,
            started: Instant::now(),
            next: None,
            glance: Default::default(),
            looked: None,
            away: HashMap::new(),
        }
    }
}

impl AutoTiler {
    /// The trees an in-place restart kept, to start with (before any
    /// window is back).
    pub fn restore(&mut self, saved: Vec<SavedTree>) {
        let until = self.started + RESTORING;
        for t in saved {
            let node = t.tree.node(until);
            info!(output = t.output, workspace = t.workspace, tree = %node, "tiles kept from before the restart");
            self.trees.insert((t.workspace, t.output), node);
        }
    }

    /// Every tree, to keep across a restart.
    pub fn trees(&self) -> impl Iterator<Item = (&(u32, String), &Node)> {
        self.trees.iter()
    }

    /// The display on connector `name` (the monitor `key`) is being
    /// unplugged: its trees are kept aside, as they are, before its
    /// windows leave them.
    pub fn leave(&mut self, name: &str, key: &str) {
        let trees: Vec<(u32, String)> = self.trees.keys().filter(|(_, o)| o == name).cloned().collect();
        if trees.is_empty() {
            return;
        }
        let kept = self.away.entry(key.to_owned()).or_default();
        kept.clear();
        for k in trees {
            if let Some(t) = self.trees.remove(&k) {
                kept.push((k.0, t));
            }
            self.laid.remove(&k);
        }
    }

    /// The monitor `key` is plugged in again, on connector `name`: its
    /// trees back (those it has no tree for now), for its windows to
    /// come back into.
    pub fn come_back(&mut self, key: &str, name: &str) {
        for (ws, t) in self.away.remove(key).unwrap_or_default() {
            self.trees.entry((ws, name.to_owned())).or_insert(t);
        }
        self.dirty = true;
    }

    /// Turned off for these: their trees go (the windows stay where they
    /// are, still tiled, so their edges still move together).
    fn forget(&mut self, workspace: u32, outputs: &[String]) {
        self.trees.retain(|(ws, o), _| !(*ws == workspace && outputs.contains(o)));
        self.laid.retain(|(ws, o), _| !(*ws == workspace && outputs.contains(o)));
        for o in outputs {
            self.seen.remove(o);
        }
    }

    /// Something changed that a tree depends on: lay out again.
    pub fn changed(&mut self) {
        self.dirty = true;
    }
}

/// Good guesses for programs not built on the kit: portal dialogs
/// (file choosers and the like) and picture-in-picture windows.
/// One size only across or down (an alert, a calculator): a smallest
/// size the same as the largest.
fn one_size(min: i32, max: i32) -> bool {
    max > 0 && min == max
}

/// A largest size a display could be bigger than (0 is none, and those
/// past any display's mean none too).
fn bounded(max: i32) -> bool {
    max > 0 && max < 1 << 15
}

fn floating_kind(app_id: &str, title: &str) -> bool {
    app_id.starts_with("xdg-desktop-portal") || app_id.ends_with(".portal") || super::pip::pip_title(title)
}

/// `r` grown to `min`, and moved only as far as it must to be in `area`.
fn fit(r: Rectangle<i32, Logical>, min: smithay::utils::Size<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let (w, h) = (r.size.w.max(min.w).min(area.size.w.max(min.w)), r.size.h.max(min.h).min(area.size.h.max(min.h)));
    let x = r.loc.x.min(area.loc.x + area.size.w - w).max(area.loc.x);
    let y = r.loc.y.min(area.loc.y + area.size.h - h).max(area.loc.y);
    Rectangle::new(Point::from((x, y)), smithay::utils::Size::from((w, h)))
}

pub(super) fn fullscreen(w: &WindowElement) -> bool {
    let WindowSurface::Wayland(t) = w.0.underlying_surface();
    t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen))
}

fn area_part(area: Rectangle<i32, Logical>, p: Point<f64, Logical>) -> (f32, f32) {
    (((p.x - area.loc.x as f64) / area.size.w.max(1) as f64) as f32, ((p.y - area.loc.y as f64) / area.size.h.max(1) as f64) as f32)
}

impl<B: Backend + 'static> AnvilState<B> {
    /// Whether auto-tiling is on for `output` on `workspace`.
    pub fn auto_on(&self, output: &Output, workspace: u32) -> bool {
        self.mode_of(output, workspace) == olo_protocol::WindowingMode::AutoTiling
    }

    /// Floated or tiled by hand, if it was.
    pub fn float_override(w: &WindowElement) -> Option<bool> {
        w.user_data().get::<Float>().and_then(|f| f.0.get())
    }

    /// Whether `w` floats over auto-tiled windows rather than being in a
    /// tile: floated by hand, or a dialog, an alert or other fixed-size
    /// window, one whose largest size is less than its display's tiles,
    /// one tagged `floating`, or a kind that floats (`floating_kind`).
    /// Tiled by hand, it doesn't.
    pub fn floats(&self, w: &WindowElement) -> bool {
        if w.meta().pip.get() {
            return true;
        }
        if let Some(f) = Self::float_override(w) {
            return f;
        }
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        if t.parent().is_some() {
            return true;
        }
        // (All it needs of the surface's state at one go: this is asked of
        // every window on every turn of the loop.)
        use smithay::wayland::{
            compositor::with_states,
            shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData, dialog::ToplevelDialogHint},
            xdg_toplevel_tag::XdgToplevelTagSurfaceData,
        };
        let (floats, min, max) = with_states(t.wl_surface(), |st| {
            let size = *st.cached_state.get::<SurfaceCachedState>().current();
            let tagged = st.data_map.get::<XdgToplevelTagSurfaceData>().is_some_and(|d| d.tag().is_some_and(|t| t.split_whitespace().any(|t| t == "floating")));
            let floats = tagged
                || st.data_map.get::<XdgToplevelSurfaceData>().is_some_and(|d| {
                    let d = d.lock().unwrap();
                    // (A dialog is one "relative to its parent", and one with
                    // a parent floats already: GTK 4 gives every toplevel an
                    // xdg_dialog_v1, which makes its main windows "dialogs".)
                    d.dialog_hint == ToplevelDialogHint::Modal || floating_kind(d.app_id.as_deref().unwrap_or(""), d.title.as_deref().unwrap_or(""))
                });
            (floats, size.min_size, size.max_size)
        });
        if floats || one_size(min.w, max.w) || one_size(min.h, max.h) {
            return true;
        }
        // A largest size it can't fill the whole of its display's tiles
        // with. (Most have none: 0, or for Electron i32::MAX.)
        if !(bounded(max.w) || bounded(max.h)) {
            return false;
        }
        let Some(area) = self.window_output(w).and_then(|o| self.tile_area(&o)) else { return false };
        let room = w.inner_size(area.size);
        (bounded(max.w) && max.w < room.w) || (bounded(max.h) && max.h < room.h)
    }

    /// Every tree with its splits as laid out on its display now (see
    /// [`as_laid_out`]), to keep across a restart.
    pub fn auto_trees_as_laid_out(&self) -> Vec<((u32, String), Node)> {
        let windows = self.all_windows();
        self.auto
            .trees()
            .map(|(key, t)| {
                // Some of its windows closed (re-executing for an upgrade):
                // as it was laid out with them all.
                let gone = |n: &Node| closing(n) || matches!(n, Node::Window(id) if self.window_by_id(*id).is_none());
                if t.any_leaf(&gone)
                    && let Some(laid) = self.auto.laid.get(key)
                {
                    return (key.clone(), with_ratios_of(t, laid));
                }
                let area = self.space.outputs().find(|o| o.name() == key.1).and_then(|o| self.tile_area(o));
                let laid = match area {
                    Some(area) => {
                        let mins = self.auto_mins(&windows, area);
                        as_laid_out(t, &|id| mins.get(&id).copied().unwrap_or((0.0, 0.0)))
                    }
                    None => t.clone(),
                };
                (key.clone(), laid)
            })
            .collect()
    }

    /// The app and title a tile's window had, as auto-tiling last knew
    /// it (it may have closed since).
    pub fn auto_known(&self, id: u64) -> Option<(String, String)> {
        self.auto.known.get(&id).map(|k| (k.app_id.clone(), k.title.clone()))
    }

    /// In an auto-tiled display's tiles.
    pub fn auto_tiled(&self, w: &WindowElement) -> bool {
        self.auto.trees.iter().any(|((ws, o), t)| *ws == self.showing_named(o) && t.contains(w.id()))
    }

    /// Every window in the trees of the workspaces showing, and its
    /// tree's display.
    fn auto_leaves(&self) -> HashMap<u64, String> {
        self.auto.trees.iter().filter(|((ws, o), _)| *ws == self.showing_named(o)).flat_map(|((_, o), t)| t.windows().into_iter().map(move |id| (id, o.clone()))).collect()
    }

    /// The tree `w` is in on the workspace its display shows, by key.
    pub(super) fn tree_of(&self, w: &WindowElement) -> Option<(u32, String)> {
        self.auto.trees.iter().find(|((ws, o), t)| *ws == self.showing_named(o) && t.contains(w.id())).map(|(k, _)| k.clone())
    }

    /// The windows on `output` that are tiles: not floating, and shown
    /// already (or given a tile on their first commit). The window being
    /// dragged by its tab is still in the tile it left.
    fn auto_units(&self, output: &Output, tree: Option<&Node>) -> Vec<WindowElement> {
        let leaves: HashMap<u64, String> = tree.map(|t| t.windows().into_iter().map(|id| (id, output.name())).collect()).unwrap_or_default();
        self.auto_units_all(&leaves).remove(&output.name()).unwrap_or_default()
    }

    /// `auto_units` for every display at once, in one pass over the
    /// windows (front first; a group counts as its front window).
    /// `leaves`: the windows in the trees, and the trees' displays.
    fn auto_units_all(&self, leaves: &HashMap<u64, String>) -> HashMap<String, Vec<WindowElement>> {
        let moving = self.auto.moving.as_ref().filter(|m| m.alive());
        let mut fronts = Vec::new();
        let mut out: HashMap<String, Vec<WindowElement>> = HashMap::new();
        for w in self.space.elements().rev() {
            if let Some(g) = self.groups.number_of(w) {
                if fronts.contains(&g) {
                    continue;
                }
                fronts.push(g);
            }
            let id = w.id();
            if Some(w) == moving {
                // In the tile it left, wherever it is now.
                if let Some(o) = leaves.get(&id) {
                    out.entry(o.clone()).or_default().push(w.clone());
                }
                continue;
            }
            if !(w.meta().placed.get() || leaves.contains_key(&id)) || self.floats(w) {
                continue;
            }
            if let Some(o) = self.window_output(w) {
                out.entry(o.name()).or_default().push(w.clone());
            }
        }
        out
    }

    /// Each window's least size, in fractions of `area` (gap included).
    fn auto_mins(&self, windows: &[WindowElement], area: Rectangle<i32, Logical>) -> HashMap<u64, (f32, f32)> {
        let gap = self.tile_gap();
        windows
            .iter()
            .map(|w| {
                let m = self.min_frame(w);
                (w.id(), ((m.w + gap) as f32 / area.size.w.max(1) as f32, (m.h + gap) as f32 / area.size.h.max(1) as f32))
            })
            .collect()
    }

    /// Lay out again what changed (`tick`): each display of this
    /// workspace with auto-tiling on, if its windows or room changed, a
    /// command changed its tree, or a kept place is due to go; trees of
    /// displays turned off are dropped.
    pub fn auto_tick(&mut self) {
        let t = &self.settings.tiling;
        let auto = olo_protocol::WindowingMode::AutoTiling;
        if self.auto.trees.is_empty() && t.default_mode != auto && !t.modes.iter().any(|m| m.mode == auto) {
            return;
        }
        let now = Instant::now();
        let due = self.auto.next.is_some_and(|t| now >= t);
        // Nothing moved, came, went or restacked, and nothing asked: no
        // more to do this turn (the common case, as the pointer moves).
        let glance = (
            self.display_workspaces(),
            self.space.elements().filter_map(|w| Some((w.id(), self.space.element_geometry(w)?))).collect::<Vec<_>>(),
            self.space.outputs().filter_map(|o| self.tile_area(o)).collect::<Vec<_>>(),
        );
        let still = glance == self.auto.glance;
        if still && !due && !self.auto.dirty && self.auto.looked.is_some_and(|t| now.duration_since(t) < LOOK) {
            return;
        }
        self.auto.glance = glance;
        self.auto.looked = Some(now);
        let dirty = std::mem::take(&mut self.auto.dirty) || due;
        if due {
            self.auto.next = None;
        }
        self.auto_returned();
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let on: Vec<&Output> = outputs.iter().filter(|o| self.auto_on(o, self.showing(o))).collect();
        for o in outputs.iter().filter(|o| !on.contains(o)) {
            if self.auto.trees.remove(&(self.showing(o), o.name())).is_some() {
                self.auto.seen.remove(&o.name());
            }
        }
        if !on.is_empty() {
            // What each display's tiles were laid out for, against what
            // there is now: only a change lays them out again.
            let mut units = self.auto_units_all(&self.auto_leaves());
            for o in on {
                let key = (self.showing(o), o.name());
                let Some(area) = self.tile_area(o) else { continue };
                let ids: Vec<(u64, smithay::utils::Size<i32, Logical>)> = units.remove(&o.name()).unwrap_or_default().iter().map(|w| (w.id(), self.min_frame(w))).collect();
                if dirty || self.auto.seen.get(&o.name()).is_none_or(|(a, i)| *a != area || *i != ids) || !self.auto.trees.contains_key(&key) {
                    self.auto_output(o);
                    let leaves: HashMap<u64, String> = self.auto.trees.get(&key).map(|t| t.windows().into_iter().map(|id| (id, o.name())).collect()).unwrap_or_default();
                    let now = self.auto_units_all(&leaves).remove(&o.name()).unwrap_or_default().iter().map(|w| (w.id(), self.min_frame(w))).collect();
                    self.auto.seen.insert(o.name(), (area, now));
                }
            }
        }
        // Gone displays' trees go too, and what was known of windows in
        // none of them; but not just after a restart, while displays are
        // still being found (DRM finds them after the clients are back):
        // theirs are kept for their windows to come back into.
        let names: Vec<String> = outputs.iter().map(|o| o.name()).collect();
        let finding = self.display_wait_until.is_some_and(|t| Instant::now() < t);
        self.auto.trees.retain(|(_, o), _| finding || names.contains(o));
        let trees: Vec<(u32, String)> = self.auto.trees.keys().cloned().collect();
        self.auto.laid.retain(|k, _| trees.contains(k));
        let mut kept: Vec<u64> = self.all_windows().iter().map(|w| w.id()).collect();
        kept.extend(self.auto.trees.values().flat_map(|t| t.windows()));
        self.auto.known.retain(|id, _| kept.contains(id));
        self.auto.along.retain(|id, _| kept.contains(id));
        self.keep_floating_above();
        self.auto_timer();
    }

    /// Windows back from a restart, in the tiles kept for them: each
    /// takes the leaf naming the place it was put back from, on any
    /// workspace (one not showing has it when it shows).
    fn auto_returned(&mut self) {
        if !self.auto.trees.values().any(|t| t.any_leaf(&|n| matches!(n, Node::Kept(Kept { who: Who::Returning(_), .. })))) {
            return;
        }
        let back: Vec<(u64, u64)> = self.all_windows().iter().filter(|w| w.meta().placed.get() && w.meta().slot.get() > 0).map(|w| (w.meta().slot.get(), w.id())).collect();
        if back.is_empty() {
            return;
        }
        let mut changed = false;
        for t in self.auto.trees.values_mut() {
            for &(slot, id) in &back {
                if t.contains(id) {
                    continue;
                }
                let took = t.replace(&|n| matches!(n, Node::Kept(Kept { who: Who::Returning(s), .. }) if s.contains(&slot)), Node::Window(id));
                if took {
                    info!(window = id, slot, "back in its tile");
                }
                changed |= took;
            }
        }
        if changed {
            self.auto.dirty = true;
        }
    }

    /// A timer for the next kept place to be forgotten.
    fn auto_timer(&mut self) {
        let mut next: Option<Instant> = None;
        for t in self.auto.trees.values() {
            t.each_leaf(&mut |n| {
                if let Node::Kept(Kept { until: Some(u), .. }) = n {
                    next = Some(next.map_or(*u, |x: Instant| x.min(*u)));
                }
            });
        }
        let Some(at) = next else { return };
        if self.auto.next.is_some_and(|n| n <= at) {
            return;
        }
        self.auto.next = Some(at);
        let wait = at.saturating_duration_since(Instant::now()) + Duration::from_millis(5);
        let _ = self.handle.insert_source(Timer::from_duration(wait), |_, _, state| {
            state.auto.dirty = true;
            TimeoutAction::Drop
        });
    }

    /// One display's tree brought up to date with its windows, and laid
    /// out.
    fn auto_output(&mut self, output: &Output) {
        // Windows back from a restart into the tiles kept for them first
        // (laid out from elsewhere, as when their display is found, they
        // would otherwise be new to it, and fill those tiles in the order
        // they came).
        self.auto_returned();
        let key = (self.showing(&output), output.name());
        let Some(area) = self.tile_area(output) else { return };
        let now = Instant::now();
        let mut tree = self.auto.trees.remove(&key);
        let before = tree.as_ref().map(|t| t.to_string());
        let units = self.auto_units(output, tree.as_ref());
        if tree.is_none() && !units.is_empty() {
            tree = self.auto_adopt(output, area, &units);
        }
        let ids: Vec<u64> = units.iter().map(|w| w.id()).collect();
        // Leaves whose window isn't a tile here now: a group's window
        // that's no longer its front hands its tile to the one that is;
        // the rest leave their place kept, while that's any use.
        if let Some(t) = tree.as_mut() {
            let mut gone = Vec::new();
            t.each_leaf(&mut |n| {
                if let Node::Window(id) = n
                    && !ids.contains(id)
                {
                    gone.push(*id);
                }
            });
            let in_tree: Vec<u64> = t.windows();
            for id in gone {
                let known = self.auto.known.get(&id).cloned();
                let alive = self.window_by_id(id);
                // Its group: the one it is behind in now, else the one it
                // was in (closed, minimized).
                let group = alive.as_ref().and_then(|w| self.groups.place(w)).map(|(n, _)| n).or(known.as_ref().and_then(|k| k.group));
                let heir = group.and_then(|g| units.iter().find(|u| self.groups.place(u).map(|(n, _)| n) == Some(g)).map(|u| u.id()));
                // The front came from a tile of its own here (it opened
                // beside this one, or was put in the group by its tab):
                // the group stays here, and that tile closes.
                if let Some(h) = heir
                    && in_tree.contains(&h)
                {
                    let rest = std::mem::replace(t, Node::gap());
                    *t = rest.without(&|n| *n == Node::Window(h)).unwrap_or_else(Node::gap);
                }
                let with = match (heir, alive, known) {
                    (Some(h), ..) => Node::Window(h),
                    (None, Some(_), _) => Node::Kept(Kept { who: Who::Window(id), holds: false, until: None }),
                    (None, None, Some(k)) => Node::Kept(Kept { who: Who::Like { app_id: k.app_id, title: k.title }, holds: false, until: Some(now + CLOSED_KEEP) }),
                    (None, None, None) => Node::gap(),
                };
                t.replace(&|n| *n == Node::Window(id), with);
            }
        }
        // Kept places that are no use any more.
        let elsewhere: Vec<u64> = self.auto.trees.values().flat_map(|t| t.windows()).collect();
        let alive = |id: u64| self.window_by_id(id).is_some();
        tree = tree.and_then(|t| {
            t.without(&|n| match n {
                Node::Kept(k) => k.until.is_some_and(|u| now >= u) || (k.who == Who::Nobody && !k.holds) || matches!(k.who, Who::Window(id) if !alive(id) || elsewhere.contains(&id)),
                _ => false,
            })
        });
        // The windows new to it.
        let fresh: Vec<WindowElement> = units.iter().filter(|w| !tree.as_ref().is_some_and(|t| t.contains(w.id()))).cloned().collect();
        for w in &fresh {
            self.auto_insert(&mut tree, w, area);
            // Gone from the place it left in another tree.
            for t in self.auto.trees.values_mut() {
                let id = w.id();
                if t.any_leaf(&|n| matches!(n, Node::Kept(Kept { who: Who::Window(i), .. }) if *i == id)) {
                    let rest = std::mem::replace(t, Node::gap());
                    *t = rest.without(&|n| matches!(n, Node::Kept(Kept { who: Who::Window(i), .. }) if *i == id)).unwrap_or_else(Node::gap);
                }
            }
        }
        // Tiles too small for their windows' least sizes, where another
        // arrangement has room for them all (or overlaps less): that one.
        if let Some(t) = tree.as_mut() {
            let mins = self.auto_mins(&units, area);
            let aspect = area.size.w as f32 / area.size.h.max(1) as f32;
            if let Some(better) = super::autofit::refit(t, &|id| mins.get(&id).copied().unwrap_or((0.0, 0.0)), aspect) {
                info!(output = key.1, from = %t, to = %better, "tiles refitted to their windows' least sizes");
                *t = better;
            }
        }
        for w in &units {
            let group = self.groups.place(w).map(|(n, _)| n);
            self.auto.known.insert(w.id(), Known { app_id: w.app_id(), title: w.title(), group });
        }
        if let Some(t) = tree {
            let now = t.to_string();
            if before.as_ref() != Some(&now) {
                info!(output = key.1, tree = now, "tiles");
            }
            if !t.any_leaf(&|n| closing(n)) {
                let mins = self.auto_mins(&units, area);
                self.auto.laid.insert(key.clone(), as_laid_out(&t, &|id| mins.get(&id).copied().unwrap_or((0.0, 0.0))));
            }
            self.auto_apply(output, &t, &units);
            self.auto.trees.insert(key, t);
        }
    }

    /// A tree for a display auto-tiling has just started on (turned on,
    /// or after a restart): from where its windows are, if they are in
    /// tiles (with room kept for windows not back yet from a restart),
    /// else as even as it can be, in reading order.
    fn auto_adopt(&self, output: &Output, area: Rectangle<i32, Logical>, units: &[WindowElement]) -> Option<Node> {
        let gap = self.tile_gap();
        let restoring = self.auto.started.elapsed() < RESTORING;
        let mut items: Vec<(Node, Part)> = units
            .iter()
            .filter_map(|w| {
                // Just back from a restart: the part it had, as saved (what
                // put it back may have tiled it afresh since).
                let saved = w.meta().restored_part.get().filter(|_| restoring);
                let tiled = saved.or_else(|| Self::tiled(w).map(|t| Self::custom_part(w).unwrap_or(t.fraction())));
                let f = tiled.or_else(|| self.space.element_geometry(w).map(|g| fraction_of(area, g, gap)))?;
                Some((Node::Window(w.id()), f))
            })
            .collect();
        if restoring {
            let until = self.auto.started + RESTORING;
            let out_geo = self.space.output_geometry(output);
            let here = |p: &crate::layout::Placement| {
                let middle = Point::from((p.x + p.width / 2, p.y + p.height / 2));
                p.workspace == self.showing(output) && !p.minimized && out_geo.is_some_and(|g| g.contains(middle))
            };
            // Back already, on their way to their places (claimed, not
            // shown yet): room for each, by window.
            for w in self.space.elements().filter(|w| !w.meta().placed.get() && !units.contains(w)) {
                let Some(p) = w.meta().returning.borrow().clone() else { continue };
                let Some(tile) = p.tile.filter(|_| here(&p)) else { continue };
                let kept = Kept { who: Who::Window(w.id()), holds: true, until: Some(until) };
                items.push((Node::Kept(kept), p.part.unwrap_or(tile.fraction())));
            }
            // Not back yet: room for one like each, where that is no one
            // else's (a window closed before the restart is remembered
            // too, and others may have its room now).
            for p in &self.layout {
                let Some(tile) = p.tile.filter(|_| here(p)) else { continue };
                let part = p.part.unwrap_or(tile.fraction());
                if items.iter().any(|(_, q)| overlap(*q, part)) {
                    continue;
                }
                let who = Who::Like { app_id: p.app_id.clone(), title: p.title.clone() };
                items.push((Node::Kept(Kept { who, holds: true, until: Some(until) }), part));
            }
        }
        // From where they are, with room for those to come; else without.
        let windows = |t: &Node| units.iter().all(|w| t.contains(w.id()));
        let mut tries = vec![items.clone()];
        tries.push(items.iter().filter(|(n, _)| matches!(n, Node::Window(_))).cloned().collect());
        for items in tries {
            if let Some(t) = from_parts(items).filter(windows) {
                return Some(t);
            }
        }
        let mut order: Vec<&WindowElement> = units.iter().collect();
        order.sort_by_key(|w| {
            let g = self.space.element_geometry(w).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        });
        let ids: Vec<u64> = order.iter().map(|w| w.id()).collect();
        let mins = self.auto_mins(units, area);
        let (aw, ah) = (area.size.w as f32, area.size.h as f32);
        balanced(&ids, (aw, ah), &|i| mins.get(&i).map_or((0.0, 0.0), |m| (m.0 * aw, m.1 * ah)))
    }

    /// `w` into `tree`: back in its kept place, if it has one; else in
    /// room a tiling command left; else taking half of the tile that had
    /// the focus last (or the biggest), across its longer side.
    fn auto_insert(&self, tree: &mut Option<Node>, w: &WindowElement, area: Rectangle<i32, Logical>) {
        let id = w.id();
        let Some(t) = tree.as_mut() else {
            *tree = Some(Node::Window(id));
            return;
        };
        if t.replace(&|n| matches!(n, Node::Kept(Kept { who: Who::Window(i), .. }) if *i == id), Node::Window(id)) {
            return;
        }
        if w.meta().put_back.get() {
            let (app, title) = (w.app_id(), w.title());
            let same = |n: &Node, titled: bool| matches!(n, Node::Kept(Kept { who: Who::Like { app_id, title: t }, .. }) if *app_id == app && (!titled || *t == title));
            if t.replace(&|n| same(n, true), Node::Window(id)) || t.replace(&|n| same(n, false), Node::Window(id)) {
                return;
            }
        }
        let windows = self.all_windows();
        let mins = self.auto_mins(&windows, area);
        let min = |i: u64| mins.get(&i).copied().unwrap_or((0.0, 0.0));
        let laid: Vec<(Node, Part)> = layout(t, &min).into_iter().map(|(n, p)| (n.clone(), p)).collect();
        let size = |p: &Part| p.2 * area.size.w as f32 * p.3 * area.size.h as f32;
        let biggest_hole = laid.iter().filter(|(n, _)| *n == Node::hole()).enumerate().max_by(|a, b| size(&a.1.1).total_cmp(&size(&b.1.1)));
        if let Some((nth, _)) = biggest_hole {
            t.replace_nth(&|n| *n == Node::hole(), nth, Node::Window(id));
            return;
        }
        let tiles: Vec<(u64, Part)> = laid.iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, *p)) } else { None }).collect();
        // (In pixels: a tile's longer side is across the screen's shape.)
        let px = |(w, h): (f32, f32)| (w * area.size.w as f32, h * area.size.h as f32);
        let mut own = self.auto_mins(std::slice::from_ref(w), area);
        let adding = own.remove(&id).unwrap_or((0.0, 0.0));
        // Whether the tile has room for both, cut the way it would be.
        let room = |(i, p): &(u64, Part)| {
            let (size, having, adding) = (px((p.2, p.3)), px(min(*i)), px(adding));
            match cut_for(size, having, adding) {
                Cut::Columns => having.0 + adding.0 <= size.0,
                Cut::Rows => having.1 + adding.1 <= size.1,
            }
        };
        // The tile that had the focus last, if it has room; else the
        // biggest that has; else the biggest.
        let last = self.recent.iter().find_map(|r| tiles.iter().find(|(i, _)| i == r && *i != id)).copied().filter(room);
        let biggest = |tiles: &mut dyn Iterator<Item = (u64, Part)>| tiles.max_by(|a, b| size(&a.1).total_cmp(&size(&b.1)));
        let target = last.or_else(|| biggest(&mut tiles.iter().copied().filter(room))).or_else(|| biggest(&mut tiles.iter().copied()));
        let Some((target, p)) = target else {
            // Only kept places: beside them.
            let rest = std::mem::replace(t, Node::gap());
            *t = Node::split(Cut::Columns, 0.5, rest, Node::Window(id));
            return;
        };
        let cut = cut_for(px((p.2, p.3)), px(min(target)), px(adding));
        t.split_leaf(target, Node::Window(id), cut, false);
    }

    /// Every window of `tree` (`units`, on `output`) moved into its tile.
    /// Zoomed and full-screen ones only learn their tile (unzoomed, they
    /// go there); the one being dragged by its tab or edges is left to the
    /// drag.
    fn auto_apply(&mut self, output: &Output, tree: &Node, units: &[WindowElement]) {
        let Some(area) = self.tile_area(output) else { return };
        let gap = self.tile_gap();
        let mins = self.auto_mins(units, area);
        let min = |i: u64| mins.get(&i).copied().unwrap_or((0.0, 0.0));
        let dragged = self.tile_drag.as_ref().map(|d| d.window());
        let placed: Vec<(u64, Part)> = layout(tree, &min).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect();
        for (id, f) in placed {
            let Some(w) = units.iter().find(|w| w.id() == id).cloned() else { continue };
            let was_tiled = Self::tiled(&w).is_some();
            Self::set_tile_state(&w, Some(Tile::Cell), Some(f));
            if self.is_zoomed(&w) || fullscreen(&w) || self.auto.moving.as_ref() == Some(&w) || dragged.as_ref() == Some(&w) {
                continue;
            }
            // No smaller than it can be, and on the display even so (where
            // the windows' least sizes don't all fit, they overlap).
            let rect = fit(part(area, f, gap), self.min_frame(&w), area);
            let Some(geo) = self.space.element_geometry(&w) else { continue };
            if !was_tiled && w.meta().placed.get() && geo.size.w > 1 {
                // Its own frame, for floating it or Restore. (One that
                // opened in its tile has none: floated, it takes the size
                // it would have opened at.)
                w.user_data().insert_if_missing(PreZoom::default);
                w.user_data().get::<PreZoom>().unwrap().0.borrow_mut().get_or_insert(geo);
            }
            self.auto_place(&w, geo, rect);
        }
        // Group mates take their front window's tile when they come forward.
        for w in units {
            for mate in self.groups.mates(w) {
                if mate != *w {
                    self.share_arrangement(w, &mate);
                }
            }
        }
    }

    /// `w` (now at `geo`) to `rect`: moved at once if that's all; sized
    /// now if it has yet to show; else asked for the size and moved when
    /// it draws it, as a neighbour follows an edge drag.
    ///
    /// What it has drawn is only all there is to it once nothing else is
    /// on its way: a size asked for a moment ago (another layout, an edge
    /// drag's last step) would land after this, wherever it put the
    /// window, and stay there (a tile too wide, out past the display's
    /// edge). So it is asked for this one instead, from where it began.
    fn auto_place(&mut self, w: &WindowElement, geo: Rectangle<i32, Logical>, rect: Rectangle<i32, Logical>) {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        if geo.size == rect.size && super::groups::size_on_the_way(w).is_none() {
            // Drawn at its tile's size, and asked for no other: there now.
            // (Following, it is where it was going: it would move nowhere
            // else, and no commit may come to move it.)
            if geo != rect {
                let Some(loc) = self.space.element_location(w) else { return };
                self.space.relocate_element(w, loc + (rect.loc - geo.loc));
            }
            self.unfollow(w);
        } else if !t.is_initial_configure_sent() {
            // Its frame comes with the answer to this first configure: its
            // content is the tile less the frame it will have.
            let content = w.inner_size(rect.size);
            t.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.size = Some(content);
            });
            self.space.relocate_element(w, rect.loc);
        } else if !w.meta().placed.get() {
            self.set_frame(w, rect, false);
        } else {
            let began = self.following_from(w);
            self.follow(w, began.unwrap_or(geo), rect);
        }
    }

    /// A window's first commit (before its first configure goes): if it
    /// will be a tile, give it its tile now, so it opens at that size
    /// rather than drawing another first.
    pub fn auto_claim(&mut self, surface: &WlSurface) {
        let Some(w) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return };
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        if t.is_initial_configure_sent() || w.meta().placed.get() || w.meta().returning.borrow().is_some() || self.floats(&w) {
            return;
        }
        let Some(output) = self.window_output(&w) else { return };
        if !self.auto_on(&output, self.showing(&output)) || self.joins_a_group(&w) {
            return;
        }
        let key = (self.showing(&output), output.name());
        if self.auto.trees.get(&key).is_some_and(|t| t.contains(w.id())) {
            return;
        }
        let Some(area) = self.tile_area(&output) else { return };
        let mut tree = self.auto.trees.remove(&key);
        if tree.is_none() {
            let units = self.auto_units(&output, None);
            tree = self.auto_adopt(&output, area, &units);
        }
        self.auto_insert(&mut tree, &w, area);
        self.auto.trees.insert(key, tree.unwrap_or(Node::Window(w.id())));
        info!(window = w.id(), app_id = w.app_id(), "a new window into its tile");
        // At once, so its first configure has its tile's size.
        self.auto_output(&output);
    }

    /// `w` (not shown yet) will join a group of its app's on screen, on
    /// its display.
    fn joins_a_group(&self, w: &WindowElement) -> bool {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        let Some(key) = self.group_key(w) else { return false };
        let display = self.window_output(w);
        // (Not `decorated`: that is false until it answers its first
        // configure, and this is before it.)
        w.will_be_decorated()
            && !super::groups::is_dialog(t)
            && resizable(w)
            && self.space.elements().any(|o| o != w && self.groupable(o) && self.group_key(o).as_ref() == Some(&key) && self.window_output(o) == display)
    }

    /// Floating windows over the tiles of auto-tiled displays, in the
    /// order they were.
    fn keep_floating_above(&mut self) {
        if !self.auto.trees.keys().any(|(ws, o)| *ws == self.showing_named(o)) {
            return;
        }
        let leaves = self.auto_leaves();
        let order: Vec<WindowElement> = self.space.elements().cloned().collect();
        let Some(top_tile) = order.iter().rposition(|w| leaves.contains_key(&w.id())) else { return };
        let under: Vec<WindowElement> = order[..top_tile].iter().filter(|w| !leaves.contains_key(&w.id()) && self.floats(w)).cloned().collect();
        for w in under {
            self.space.raise_element(&w, false);
        }
    }

    // ---- turning it on and off -------------------------------------------

    /// Auto-tiling on, off (Standard) or (`on` left out) the other way, as
    /// ⌃⌥⌘T: for `output` (the active display unless given) on its
    /// workspace `workspace` (the one showing unless given)
    /// (`set_windowing_mode`). Whether it is on now.
    pub fn set_auto_tiling(&mut self, on: Option<bool>, workspace: Option<u32>, output: Option<Output>) -> bool {
        use olo_protocol::WindowingMode as M;
        let Some(output) = output.or_else(|| self.active_output()) else { return false };
        let ws = workspace.unwrap_or_else(|| self.showing(&output));
        let on = on.unwrap_or(!self.auto_on(&output, ws));
        self.set_windowing_mode(if on { M::AutoTiling } else { M::Standard }, Some(ws), Some(output)) == M::AutoTiling
    }

    /// Auto-tiling just turned on for these displays of `workspace`, from
    /// `was`: ⌃⌘Z goes back to that.
    pub(super) fn auto_turned_on(&mut self, workspace: u32, outputs: Vec<String>, was: olo_protocol::WindowingMode) {
        self.auto.undo = Some(Undo::TurnedOn(workspace, outputs, was));
    }

    /// Auto-tiling turned off for these: their trees go (the windows stay
    /// where they are, still tiled, so their edges still move together).
    pub(super) fn auto_forget(&mut self, workspace: u32, outputs: &[String]) {
        self.auto.forget(workspace, outputs);
    }

    /// Each display's auto-tiling on its workspace `workspace` (the one
    /// it shows unless given), as the IPC tells it.
    pub fn auto_tiling_infos(&self, workspace: Option<u32>) -> Vec<olo_protocol::AutoTilingInfo> {
        self.space
            .outputs()
            .map(|o| {
                let ws = workspace.unwrap_or_else(|| self.showing(o));
                olo_protocol::AutoTilingInfo { output: o.name(), on: self.auto_on(o, ws), workspace: ws }
            })
            .collect()
    }

    /// Floated or tiled by hand, as it was before a restart (nothing
    /// else: its tile, or its frame, comes with it).
    pub fn restore_float_override(w: &WindowElement, floating: Option<bool>) {
        w.user_data().insert_if_missing(Float::default);
        w.user_data().get::<Float>().unwrap().0.set(floating);
    }

    /// Float `w` over the tiles (`Some(true)`), tile it (`Some(false)`), or
    /// leave it to app_server (`None`). Floated out of a tile, it has its
    /// own frame again, in front. Its group's windows with it: a group is
    /// one tile or floats as its front window does, and whichever is in
    /// front (after a restart, another may be) must say the same.
    pub fn set_floating(&mut self, w: &WindowElement, floating: Option<bool>) {
        let was = self.floats(w);
        for m in self.groups.mates(w).iter().chain(std::iter::once(w)) {
            Self::restore_float_override(m, floating);
        }
        if self.floats(w) && !was && self.auto_tiled(w) {
            let own = w.user_data().get::<PreZoom>().and_then(|p| *p.0.borrow());
            self.restore_frame(w);
            // Its own size not known (it opened in its tile, or came back
            // from a restart in it): two thirds of the room, in the middle.
            if own.is_none()
                && let Some(area) = self.window_output(w).and_then(|o| self.tile_area(&o))
            {
                let min = self.min_frame(w);
                let size = smithay::utils::Size::from(((area.size.w * 2 / 3).max(min.w), (area.size.h * 2 / 3).max(min.h)));
                let at = area.loc + Point::from(((area.size.w - size.w) / 2, (area.size.h - size.h) / 2));
                self.set_frame(w, fit(Rectangle::new(at, size), min, area), false);
            }
            self.space.raise_element(w, true);
        }
        self.auto.dirty = true;
        info!(window = w.id(), ?floating, floats = self.floats(w), "floating");
    }

    // ---- commands --------------------------------------------------------

    /// The tree a command on `w` works on, if `w` is a tile; kept for ⌃⌘Z.
    fn auto_take(&mut self, w: &WindowElement) -> Option<((u32, String), Node)> {
        let key = self.tree_of(w)?;
        let tree = self.auto.trees.remove(&key)?;
        self.auto.undo = Some(Undo::Tree(key.clone(), Some(tree.clone())));
        Some((key, tree))
    }

    fn auto_put(&mut self, key: (u32, String), tree: Node) {
        self.auto.trees.insert(key, tree);
        self.auto.dirty = true;
        self.auto_tick();
    }

    /// The middle of each tile of `tree`, in fractions of the area.
    fn auto_centres(&self, tree: &Node) -> HashMap<u64, (f32, f32)> {
        layout(tree, &|_| (0.0, 0.0)).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, (p.0 + p.2 / 2.0, p.1 + p.3 / 2.0))) } else { None }).collect()
    }

    /// A tiling command for a tile: its part, the rest tiled around it.
    /// False if `w` isn't a tile (the command does as it always does).
    pub fn auto_tile_window(&mut self, w: &WindowElement, tile: Tile) -> bool {
        if self.floats(w) && self.window_output(w).is_some_and(|o| self.auto_on(&o, self.showing(&o))) {
            // A floating window tiled by hand is a tile from now on.
            self.set_floating(w, Some(false));
            self.auto_tick();
        }
        let Some((key, tree)) = self.auto_take(w) else { return false };
        let centres = self.auto_centres(&tree);
        let rest = tree.without(&|n| *n == Node::Window(w.id()));
        // The rest as they were, if that fits the room they have now;
        // else tiled afresh there.
        let output = self.space.outputs().find(|o| o.name() == key.1).cloned();
        let rest = match (rest, output.and_then(|o| self.tile_area(&o))) {
            (Some(r), Some(area)) => {
                let (fw, fh) = rest_room(tile);
                let room = (fw * area.size.w as f32, fh * area.size.h as f32);
                let windows = self.all_windows();
                let mins = self.auto_mins(&windows, area);
                let (aw, ah) = (area.size.w as f32, area.size.h as f32);
                let min = |i: u64| mins.get(&i).map_or((0.0, 0.0), |m| (m.0 * aw, m.1 * ah));
                if overflow(&r, room, &min) > 0.0 { balanced(&r.windows(), room, &min).or(Some(r)) } else { Some(r) }
            }
            (r, _) => r,
        };
        let tree = tiled_into(rest, w.id(), tile, &|i| centres.get(&i).copied().unwrap_or((0.5, 0.5)));
        if self.is_zoomed(w) {
            self.unzoom_window(w);
        }
        info!(window = w.id(), ?tile, "tiled in auto-tiling");
        self.auto_put(key, tree);
        true
    }

    /// Zoom Vertically or Horizontally for a tile: a full column (or row)
    /// beside the rest; again, the tiles as they were (or, if they have
    /// changed since, back among them).
    pub fn auto_zoom_along(&mut self, w: &WindowElement, axis: Axis) -> bool {
        let Some((key, tree)) = self.auto_take(w) else { return false };
        let output = self.space.outputs().find(|o| o.name() == key.1).cloned();
        let Some(area) = output.and_then(|o| self.tile_area(&o)) else {
            self.auto.trees.insert(key, tree);
            return false;
        };
        let id = w.id();
        let tree = match self.auto.along.remove(&id) {
            Some((a, before)) if a == axis => {
                let (mut x, mut y) = (before.windows(), tree.windows());
                x.sort();
                y.sort();
                if x == y {
                    before
                } else {
                    let rest = tree.without(&|n| *n == Node::Window(id));
                    let mut t = rest;
                    self.auto_insert(&mut t, w, area);
                    t.unwrap_or(Node::Window(id))
                }
            }
            _ => {
                let geo = self.space.element_geometry(w).unwrap_or_default();
                let f = fraction_of(area, geo, self.tile_gap());
                let rest = tree.clone().without(&|n| *n == Node::Window(id));
                self.auto.along.insert(id, (axis, tree));
                along(rest, id, axis, (f.0 + f.2 / 2.0, f.1 + f.3 / 2.0), (f.2, f.3))
            }
        };
        info!(window = id, ?axis, "zoomed along in auto-tiling");
        self.auto_put(key, tree);
        true
    }

    /// Tile All on an auto-tiled display: its tiles as even as can be.
    pub fn auto_balance(&mut self) -> bool {
        let Some(output) = self.active_output() else { return false };
        let key = (self.showing(&output), output.name());
        let Some(tree) = self.auto.trees.remove(&key) else { return false };
        let Some(area) = self.tile_area(&output) else {
            self.auto.trees.insert(key, tree);
            return false;
        };
        self.auto.undo = Some(Undo::Tree(key.clone(), Some(tree.clone())));
        let mut ids = tree.windows();
        let order = |s: &Self, id: u64| {
            let g = s.window_by_id(id).and_then(|w| s.space.element_geometry(&w)).unwrap_or_default();
            (g.loc.y + g.size.h / 2, g.loc.x + g.size.w / 2)
        };
        ids.sort_by_key(|id| order(self, *id));
        let windows = self.all_windows();
        let mins = self.auto_mins(&windows, area);
        let (aw, ah) = (area.size.w as f32, area.size.h as f32);
        let Some(tree) = balanced(&ids, (aw, ah), &|i| mins.get(&i).map_or((0.0, 0.0), |m| (m.0 * aw, m.1 * ah))) else {
            self.auto.trees.insert(key, tree);
            return true;
        };
        self.auto_put(key, tree);
        true
    }

    /// Smart Tiling, Tile App and the like put windows where they chose
    /// on an auto-tiled display: the tree made again from where they are.
    pub fn auto_readopt(&mut self, output: &Output) {
        let key = (self.showing(&output), output.name());
        if let Some(tree) = self.auto.trees.remove(&key) {
            self.auto.undo = Some(Undo::Tree(key, Some(tree)));
        }
        self.auto.dirty = true;
    }

    /// After an arranger (Smart Tiling, Tile App) put the active
    /// display's windows where it chose: if that display is auto-tiled,
    /// its tree from there.
    pub fn readopt_active(&mut self) {
        if let Some(o) = self.active_output().filter(|o| self.auto_on(o, self.showing(o))) {
            self.auto_readopt(&o);
        }
    }

    /// ⌃⌘Z after a change to the tiles: the tree as it was (again, as it
    /// is now); after turning auto-tiling on, the mode there was, with the
    /// windows as they were. False if the last arrangement wasn't auto-tiling's.
    pub fn auto_undo(&mut self) -> bool {
        match self.auto.undo.take() {
            None => false,
            Some(Undo::Tree(key, before)) => {
                let now = self.auto.trees.remove(&key);
                if let Some(t) = before {
                    self.auto.trees.insert(key.clone(), t);
                }
                self.auto.undo = Some(Undo::Tree(key, now));
                self.auto.dirty = true;
                self.auto_tick();
                info!("undid an auto-tiling change");
                true
            }
            Some(Undo::TurnedOn(ws, outputs, was)) => {
                for o in self.space.outputs().filter(|o| outputs.contains(&o.name())).cloned().collect::<Vec<_>>() {
                    self.settings.tiling.set_mode(ws, &crate::desktop::display_key(&o), was);
                }
                self.auto.forget(ws, &outputs);
                crate::desktop::save_settings(&self.settings);
                self.ipc.broadcast(olo_protocol::Event::SettingsChanged { settings: self.settings.clone() });
                self.undo_arrangement();
                self.show_indicator(crate::osd::Indicator::Mode(was));
                info!(?was, "undid turning auto-tiling on");
                true
            }
        }
    }

    /// An arrangement not auto-tiling's was made: ⌃⌘Z undoes that now.
    pub fn auto_forget_undo(&mut self) {
        self.auto.undo = None;
    }

    // ---- dragging --------------------------------------------------------

    /// A tile is being dragged by its tab: it stays in its tile till let
    /// go. False if `w` isn't a tile.
    pub fn auto_moving(&mut self, w: &WindowElement) -> bool {
        if self.auto.moving.as_ref() == Some(w) {
            return true;
        }
        if !self.auto_tiled(w) {
            return false;
        }
        self.auto.moving = Some(w.clone());
        true
    }

    /// The dragged tile let go: on the middle of another tile, they swap;
    /// on its edge, it goes beside it there (on another display too);
    /// anywhere else, back to its tile.
    pub fn auto_dropped(&mut self, w: &WindowElement, pointer: Point<f64, Logical>) {
        if self.auto.moving.as_ref() != Some(w) {
            return;
        }
        self.auto.moving = None;
        let Some(from) = self.tree_of(w) else { return };
        let Some(output) = self.space.output_under(pointer).next().cloned() else {
            self.auto.dirty = true;
            return;
        };
        let to = (self.showing(&output), output.name());
        let (Some(area), true) = (self.tile_area(&output), self.auto.trees.contains_key(&to)) else {
            self.auto.dirty = true;
            return;
        };
        let p = area_part(area, pointer);
        let units = self.all_windows();
        let mins = self.auto_mins(&units, area);
        let min = |i: u64| mins.get(&i).copied().unwrap_or((0.0, 0.0));
        let target = layout(&self.auto.trees[&to], &min).into_iter().find_map(|(n, r)| match n {
            Node::Window(i) if *i != w.id() && p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3 => Some((*i, r)),
            _ => None,
        });
        let id = w.id();
        let Some((target, r)) = target else {
            self.auto.dirty = true;
            return;
        };
        let mut before = self.auto.trees.remove(&from);
        self.auto.undo = Some(Undo::Tree(from.clone(), before.clone()));
        let drop = drop_at(r, p);
        if from == to && drop == Drop::Swap {
            if let Some(t) = before.as_mut() {
                t.swap(id, target);
            }
            if let Some(t) = before {
                self.auto.trees.insert(from, t);
            }
        } else {
            let rest = before.and_then(|t| t.without(&|n| *n == Node::Window(id)));
            if let Some(t) = rest {
                self.auto.trees.insert(from.clone(), t);
            }
            let side = match drop {
                Drop::Beside(s) => s,
                Drop::Swap => Side::Right,
            };
            let (cut, first) = match side {
                Side::Left => (Cut::Columns, true),
                Side::Right => (Cut::Columns, false),
                Side::Top => (Cut::Rows, true),
                Side::Bottom => (Cut::Rows, false),
            };
            if let Some(t) = self.auto.trees.get_mut(&to) {
                t.split_leaf(target, Node::Window(id), cut, first);
            }
        }
        info!(window = id, target, ?drop, "dropped a tile");
        self.auto.dirty = true;
        self.auto_tick();
    }

    /// Whether an edge drag of tile `w` may move `side`: not the edge of
    /// the area.
    pub fn auto_side_moves(&self, w: &WindowElement, side: Side) -> bool {
        let (Some(area), Some(geo)) = (self.window_output(w).and_then(|o| self.tile_area(&o)), self.space.element_geometry(w)) else { return true };
        let near = self.tile_gap() / 2 + 2;
        match side {
            Side::Left => geo.loc.x - area.loc.x > near,
            Side::Right => area.loc.x + area.size.w - (geo.loc.x + geo.size.w) > near,
            Side::Top => geo.loc.y - area.loc.y > near,
            Side::Bottom => area.loc.y + area.size.h - (geo.loc.y + geo.size.h) > near,
        }
    }

    /// An edge drag of a tile ended (its neighbours have their parts):
    /// the splits moved to where they now are.
    pub fn auto_drag_ended(&mut self, w: &WindowElement) {
        let Some(key) = self.tree_of(w) else { return };
        let Some(mut tree) = self.auto.trees.remove(&key) else { return };
        self.auto.undo = Some(Undo::Tree(key.clone(), Some(tree.clone())));
        let parts: HashMap<u64, Part> = tree.windows().into_iter().filter_map(|i| Some((i, Self::custom_part(&self.window_by_id(i)?)?))).collect();
        take_ratios(&mut tree, &|i| parts.get(&i).copied());
        // Where it can be: not past a window's least size (else the split
        // would be one place and the windows another, until they are
        // laid out again).
        if let Some(area) = self.window_output(w).and_then(|o| self.tile_area(&o)) {
            let mins = self.auto_mins(&self.all_windows(), area);
            tree = as_laid_out(&tree, &|i| mins.get(&i).copied().unwrap_or((0.0, 0.0)));
        }
        self.auto_put(key, tree);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree kept across a restart comes back the same shape and ratios,
    /// each tile held for the windows its saved places name; splits as
    /// laid out (a least size holding an edge off where it was dropped);
    /// ratios taken over from the tree as it was with all its windows.
    #[test]
    fn trees_kept_across_a_restart() {
        let tree = Node::split(Cut::Columns, 0.3, w(1), Node::split(Cut::Rows, 0.6, w(2), Node::Kept(Kept { who: Who::Like { app_id: "a".into(), title: "t".into() }, holds: false, until: None })));
        let saved = SavedNode::of(&tree, &|id| vec![id * 10], &|app, _| (app == "a").then_some(99));
        let json = serde_json::to_string(&saved).unwrap();
        let back: SavedNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
        let until = Instant::now();
        let node = back.node(until);
        assert_eq!(node.to_string(), "[·#10 | [·#20 / ·#99]60]30");
        node.each_leaf(&mut |n| assert!(matches!(n, Node::Kept(Kept { holds: true, until: Some(_), .. }))));
        // A least size of half the room holds the edge at a half.
        let laid = as_laid_out(&Node::split(Cut::Columns, 0.3, w(1), w(2)), &|id| if id == 1 { (0.5, 0.0) } else { (0.0, 0.0) });
        assert!(matches!(laid, Node::Split { ratio, .. } if (ratio - 0.5).abs() < 1e-6));
        // The shape's ratios from the tree as it was laid out.
        let now = Node::split(Cut::Columns, 0.5, Node::Kept(Kept { who: Who::Window(1), holds: false, until: None }), w(2));
        let then = Node::split(Cut::Columns, 0.25, w(1), w(2));
        assert!(matches!(with_ratios_of(&now, &then), Node::Split { ratio, .. } if ratio == 0.25));
    }

    fn w(id: u64) -> Node {
        Node::Window(id)
    }

    fn parts(t: &Node) -> Vec<(u64, Part)> {
        layout(t, &|_| (0.0, 0.0)).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect()
    }

    fn near(p: Part, q: Part) -> bool {
        (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4 && (p.2 - q.2).abs() < 1e-4 && (p.3 - q.3).abs() < 1e-4
    }

    #[test]
    fn sizes_that_float_a_window() {
        // Electron's "no largest size" is i32::MAX (others' 0): they tile.
        assert!(!bounded(i32::MAX) && !bounded(0) && !one_size(300, i32::MAX));
        assert!(!one_size(0, 0) && !one_size(300, 0));
        // One size only, across or down: an alert, a calculator.
        assert!(one_size(420, 420));
        // A real largest size goes by the display's tiles.
        assert!(bounded(800));
    }

    #[test]
    fn a_closed_window_gives_its_room_to_its_neighbour() {
        let t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Rows, 0.5, w(2), w(3)));
        let t = t.without(&|n| *n == w(2)).unwrap();
        assert_eq!(t, Node::split(Cut::Columns, 0.5, w(1), w(3)));
        assert!(near(parts(&t)[1].1, (0.5, 0.0, 0.5, 1.0)));
    }

    #[test]
    fn kept_places_take_room_only_when_they_hold_it() {
        let away = Node::Kept(Kept { who: Who::Window(9), holds: false, until: None });
        let t = Node::split(Cut::Columns, 0.5, w(1), away);
        assert_eq!(parts(&t), vec![(1, (0.0, 0.0, 1.0, 1.0))]);
        let t = Node::split(Cut::Columns, 0.5, w(1), Node::hole());
        assert!(near(parts(&t)[0].1, (0.0, 0.0, 0.5, 1.0)));
    }

    #[test]
    fn splits_keep_minimum_sizes() {
        let t = Node::split(Cut::Columns, 0.1, w(1), w(2));
        let p = layout(&t, &|i| if i == 1 { (0.3, 0.1) } else { (0.2, 0.1) });
        assert!((p[0].1.2 - 0.3).abs() < 1e-5, "{p:?}");
        // Both can't fit: each its share of what they want.
        let p = layout(&t, &|_| (0.6, 0.1));
        assert!((p[0].1.2 - 0.5).abs() < 1e-5, "{p:?}");
        // A ratio is kept, though the layout gave it more for now.
        assert_eq!(t, Node::split(Cut::Columns, 0.1, w(1), w(2)));
    }

    #[test]
    fn even_trees_split_across_the_longer_side() {
        let none = |_| (0.0, 0.0);
        let t = balanced(&[1, 2, 3], (16.0, 10.0), &none).unwrap();
        let p = parts(&t);
        assert!(near(p[0].1, (0.0, 0.0, 1.0 / 3.0, 1.0)), "{p:?}");
        for (_, q) in &p {
            assert!((q.2 * q.3 - 1.0 / 3.0).abs() < 1e-4, "{p:?}");
        }
        let t = balanced(&[1, 2, 3, 4], (16.0, 10.0), &none).unwrap();
        assert!(parts(&t).iter().all(|(_, q)| (q.2 - 0.5).abs() < 1e-4 && (q.3 - 0.5).abs() < 1e-4));
        // Too wide to go side by side: one above the other.
        let t = balanced(&[1, 2], (16.0, 10.0), &|_| (9.0, 3.0)).unwrap();
        assert!(matches!(t, Node::Split { cut: Cut::Rows, .. }), "{t:?}");
        assert_eq!(cut_for((16.0, 10.0), (9.0, 3.0), (9.0, 3.0)), Cut::Rows);
        assert_eq!(cut_for((16.0, 10.0), (4.0, 3.0), (4.0, 3.0)), Cut::Columns);
    }

    #[test]
    fn tiles_on_screen_make_the_tree_they_came_from() {
        let t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Rows, 0.25, w(2), w(3)));
        let again = from_parts(parts(&t).into_iter().map(|(i, p)| (w(i), p)).collect()).unwrap();
        assert_eq!(again, t);
        // Room nobody has goes to the neighbour.
        let t = from_parts(vec![(w(1), (0.0, 0.0, 0.5, 1.0)), (w(2), (0.5, 0.0, 0.5, 0.5))]).unwrap();
        assert_eq!(parts(&t)[1], (2, (0.5, 0.0, 0.5, 1.0)));
        // Overlapping windows aren't tiles.
        assert_eq!(from_parts(vec![(w(1), (0.0, 0.0, 0.6, 1.0)), (w(2), (0.4, 0.0, 0.6, 1.0))]), None);
    }

    #[test]
    fn tiling_commands_tile_the_rest_around() {
        let c = |_| (0.5, 0.5);
        let t = tiled_into(Some(Node::split(Cut::Columns, 0.5, w(2), w(3))), 1, Tile::Left, &c);
        assert!(near(parts(&t)[0].1, (0.0, 0.0, 0.5, 1.0)));
        assert!(near(parts(&t)[1].1, (0.5, 0.0, 0.25, 1.0)));
        // A quarter: the nearest other window takes the quarter under it.
        let centres = |i: u64| if i == 2 { (0.2, 0.8) } else { (0.8, 0.5) };
        let t = tiled_into(Some(Node::split(Cut::Columns, 0.5, w(2), w(3))), 1, Tile::TopLeft, &centres);
        let p = parts(&t);
        assert_eq!(p.iter().find(|(i, _)| *i == 1).map(|x| x.1), Some((0.0, 0.0, 0.5, 0.5)));
        assert_eq!(p.iter().find(|(i, _)| *i == 2).map(|x| x.1), Some((0.0, 0.5, 0.5, 0.5)));
        assert_eq!(p.iter().find(|(i, _)| *i == 3).map(|x| x.1), Some((0.5, 0.0, 0.5, 1.0)));
        // Alone, it has its part, and the rest is room for the next.
        let t = tiled_into(None, 1, Tile::RightThird, &c);
        assert!(near(parts(&t)[0].1, (2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)));
        let t = tiled_into(Some(Node::split(Cut::Columns, 0.5, w(2), w(3))), 1, Tile::CenterThird, &|i| if i == 2 { (0.2, 0.5) } else { (0.8, 0.5) });
        let p = parts(&t);
        assert!(near(p[0].1, (0.0, 0.0, 1.0 / 3.0, 1.0)) && p[0].0 == 2, "{p:?}");
        assert!(near(p[1].1, (1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)) && p[1].0 == 1, "{p:?}");
    }

    #[test]
    fn dragged_edges_stay_where_let_go() {
        // Dragged to 0.71: there, not on a quarter or an eighth.
        let mut t = Node::split(Cut::Columns, 0.5, w(1), w(2));
        take_ratios(&mut t, &|i| Some(if i == 1 { (0.0, 0.0, 0.71, 1.0) } else { (0.71, 0.0, 0.29, 1.0) }));
        assert!(near(parts(&t)[0].1, (0.0, 0.0, 0.71, 1.0)), "{t:?}");
        // Within the right half: just there too, and the splits that
        // didn't move stay as they are.
        let mut t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Columns, 0.5, w(2), w(3)));
        take_ratios(&mut t, &|i| {
            Some(match i {
                1 => (0.0, 0.0, 0.5, 1.0),
                2 => (0.5, 0.0, 0.08, 1.0),
                _ => (0.58, 0.0, 0.42, 1.0),
            })
        });
        let p = parts(&t);
        assert!(near(p[1].1, (0.5, 0.0, 0.08, 1.0)), "{p:?}");
        assert!(near(p[0].1, (0.0, 0.0, 0.5, 1.0)));
        // A window coming in halves the one it goes beside; the others keep
        // the sizes they were given.
        assert!(t.split_leaf(3, w(4), Cut::Rows, false));
        let p = parts(&t);
        assert!(near(p[0].1, (0.0, 0.0, 0.5, 1.0)) && near(p[1].1, (0.5, 0.0, 0.08, 1.0)), "{p:?}");
        assert!(near(p[2].1, (0.58, 0.0, 0.42, 0.5)) && near(p[3].1, (0.58, 0.5, 0.42, 0.5)), "{p:?}");
    }

    #[test]
    fn zooming_along_makes_a_column_beside_the_rest() {
        let rest = Node::split(Cut::Rows, 0.5, w(2), w(3));
        let t = along(Some(rest), 1, Axis::Vertical, (0.8, 0.3), (0.4, 0.5));
        let p = parts(&t);
        assert!(near(p.iter().find(|(i, _)| *i == 1).unwrap().1, (0.6, 0.0, 0.4, 1.0)), "{p:?}");
        let t = along(Some(w(2)), 1, Axis::Horizontal, (0.25, 0.5), (0.5, 1.0));
        assert!(near(parts(&t).iter().find(|(i, _)| *i == 1).unwrap().1, (0.0, 0.5, 1.0, 0.5)), "{t}");
    }

    #[test]
    fn drops_swap_in_the_middle_and_go_beside_at_the_edges() {
        let r = (0.5, 0.0, 0.5, 1.0);
        assert_eq!(drop_at(r, (0.75, 0.5)), Drop::Swap);
        assert_eq!(drop_at(r, (0.52, 0.5)), Drop::Beside(Side::Left));
        assert_eq!(drop_at(r, (0.75, 0.95)), Drop::Beside(Side::Bottom));
        let mut t = Node::split(Cut::Columns, 0.3, w(1), w(2));
        t.swap(1, 2);
        assert_eq!(t, Node::split(Cut::Columns, 0.3, w(2), w(1)));
    }
}
