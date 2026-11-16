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
//! Two layouts (`TilingSettings::layout`): Halves, as above, and Main and
//! Stack, as dwm's: one main window in its place (any part a window can be
//! tiled into, chosen for each display's workspace, `main_place_of`; the
//! middle two thirds unless chosen), the rest stacked in the rest, each as
//! big: a column beside it, one either side of a main window in the
//! middle (dwm's centred main), or the rest of a corner's column and the
//! other half. It keeps its place alone too; a part with no window yet is
//! room for the next. It's the same tree, built in that shape
//! ([`main_and_stack`]) when the layout or the place is chosen and
//! whenever the number of tiles with room changes, from the shape as it
//! was ([`main_and_stack_of`]: its main window, the stack's order, where
//! the person dragged its edges), or in reading order. A new window (not
//! one going back into a place kept for it) is the main one, the one that
//! was first in the stack; a main window that closes or hides gives its
//! place to the one before it. Make Main (⌃⌥⌘M) makes any tile the main
//! one, the one that was taking its place, and Swap with Previous Main
//! (⌃⌥⌘⌫) puts back the one before ([`Mains`]). Between those the tree is
//! the person's: edges dragged, tiles swapped, a tiling command, kept as
//! they are.
//!
//! Laying out sends every window changing size its size in one round
//! (`rounds.rs`, as an edge drag's): they draw it and land together, in the
//! same frame, not each as it gets there.
//!
//! The commands work on the tree: Tile Left (and the other halves,
//! quarters and thirds) gives the window that part with the rest tiled
//! around it; Zoom Vertically and Horizontally make it a full column or
//! row; Tile All balances the tree; dragging a tile onto another swaps
//! them, or onto one's edge puts it beside it; dragging an edge moves the
//! split to just where it is let go (no grid: the windows are as sized,
//! and the rest go on fitting around them as windows come and go), the
//! tiles beside it following each step and shown with it (`rounds`). Zoom
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
    /// A place kept for a window (or group) that took no room: one away
    /// (minimized, its group's tab behind), or one closed that may come
    /// back (its app re-executing). Brought back, it takes none either,
    /// till its window does: room held for it would be a hole where the
    /// tiles had met before the restart, closed a moment later.
    Away(Vec<u64>),
    /// A closed window's place, with nothing saved to name it by.
    Like {
        app_id: String,
        title: String,
        /// Whether it held room (one only just closed, or not back yet
        /// from an earlier restart, does). (From an app_server before
        /// this was saved: it did.)
        #[serde(default = "yes")]
        holds: bool,
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

fn yes() -> bool {
    true
}

impl SavedNode {
    /// `node` to keep: `slots` names each window's saved places, `like`
    /// a closed window's (None: its room is kept for one like it).
    pub fn of(node: &Node, slots: &dyn Fn(u64) -> Vec<u64>, like: &dyn Fn(&str, &str) -> Option<u64>) -> SavedNode {
        let kept = |s: Vec<u64>, holds: bool| if holds { SavedNode::Slots(s) } else { SavedNode::Away(s) };
        match node {
            Node::Window(id) => SavedNode::Slots(slots(*id)),
            Node::Kept(Kept { who: Who::Window(id), holds, .. }) => kept(slots(*id), *holds),
            Node::Kept(Kept { who: Who::Like { app_id, title }, holds, .. }) => match like(app_id, title) {
                Some(slot) => kept(vec![slot], *holds),
                None => SavedNode::Like { app_id: app_id.clone(), title: title.clone(), holds: *holds },
            },
            // A place still kept from the restart before, for a window not
            // back since: its slots were that save's numbering, which this
            // one has given to other windows (seed 16 of auto --reloads:
            // slot 4 kept twice, the second holding a fifth of the column
            // for nobody until it timed out, 20 s on, and the tiles beside
            // it then jumped). Its room goes to its neighbours.
            Node::Kept(Kept { who: Who::Returning(_), .. }) => SavedNode::Room { holds: false },
            Node::Kept(Kept { who: Who::Nobody, holds, .. }) => SavedNode::Room { holds: *holds },
            Node::Split { cut, ratio, a, b } => {
                SavedNode::Split { columns: *cut == Cut::Columns, ratio: *ratio, a: Box::new(SavedNode::of(a, slots, like)), b: Box::new(SavedNode::of(b, slots, like)) }
            }
        }
    }

    /// The tree to start with: every tile's room held, until `until`,
    /// for the windows coming back (those that took none, kept without).
    pub fn node(&self, until: Instant) -> Node {
        match self {
            SavedNode::Slots(s) | SavedNode::Away(s) if s.is_empty() => Node::gap(),
            SavedNode::Slots(s) => Node::Kept(Kept { who: Who::Returning(s.clone()), holds: true, until: Some(until) }),
            SavedNode::Away(s) => Node::Kept(Kept { who: Who::Returning(s.clone()), holds: false, until: Some(until) }),
            SavedNode::Like { app_id, title, holds } => Node::Kept(Kept { who: Who::Like { app_id: app_id.clone(), title: title.clone() }, holds: *holds, until: Some(until) }),
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
    // The edge between them on a grid of 2⁻²⁰ of the area, as every edge
    // is (the area's are 0 and 1): sums and differences of such are exact
    // in an f32, so a tile's far edge (its start plus its length) is
    // exactly its neighbour's start, however deep each is in the tree.
    // (A split nested in another gave one `w·r + w·(1−r)` and the other
    // `w`, an ulp apart, now and then rounding to pixels a gap apart:
    // auto-fuzz seed 38, two tiles 7 px apart where the gap is 6.)
    let q = |v: f32| (v * GRID).round() / GRID;
    match cut {
        Cut::Columns => {
            let e = q(x + w * ratio).clamp(x, x + w);
            ((x, y, e - x, h), (e, y, (x + w) - e, h))
        }
        Cut::Rows => {
            let e = q(y + h * ratio).clamp(y, y + h);
            ((x, y, w, e - y), (x, e, w, (y + h) - e))
        }
    }
}

/// `divide`'s grid: edges are whole multiples of `1 / GRID` of the area.
const GRID: f32 = 1_048_576.0;

/// Where along `cut` a part starts, and how long it is.
fn span((x, y, w, h): Part, cut: Cut) -> (f32, f32) {
    match cut {
        Cut::Columns => (x, w),
        Cut::Rows => (y, h),
    }
}

/// What of the gap between tiles each side of a tile gives up, in
/// fractions of the area (as `tiling::part` lays them): `lead` at its
/// left and top sides, `trail` at its right and bottom ones, and nothing
/// at the area's edges.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inset {
    pub lead: (f32, f32),
    pub trail: (f32, f32),
}

impl Inset {
    pub fn of(area: Rectangle<i32, Logical>, gap: i32) -> Inset {
        let half = gap / 2;
        let (w, h) = (area.size.w.max(1) as f32, area.size.h.max(1) as f32);
        Inset { lead: ((gap - half) as f32 / w, (gap - half) as f32 / h), trail: (half as f32 / w, half as f32 / h) }
    }
}

/// What's wrong with tiles' parts, if anything: one outside the area, not
/// a number, or two overlapping.
pub fn parts_wrong(parts: &[(u64, Part)]) -> Option<String> {
    const E: f32 = 1e-3;
    for (i, (a, p)) in parts.iter().enumerate() {
        if ![p.0, p.1, p.2, p.3].iter().all(|v| v.is_finite()) || p.0 < -E || p.1 < -E || p.0 + p.2 > 1.0 + E || p.1 + p.3 > 1.0 + E {
            return Some(format!("{a} at {p:?}, outside the area"));
        }
        for (b, q) in &parts[i + 1..] {
            let ox = (p.0 + p.2).min(q.0 + q.2) - p.0.max(q.0);
            let oy = (p.1 + p.3).min(q.1 + q.3) - p.1.max(q.1);
            if ox > E && oy > E {
                return Some(format!("{a} at {p:?} overlaps {b} at {q:?}"));
            }
        }
    }
    None
}

/// Where a new window goes in Main and Stack's order
/// (`main_and_stack_again`): first (the main window), right after a
/// window, or last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NewAt {
    First,
    After(u64),
    Last,
}

/// `id` into `order` where `at` says (after a window not in it: last).
fn put_new(order: &mut Vec<Node>, id: u64, at: NewAt) {
    let i = match at {
        NewAt::First => 0,
        NewAt::After(f) => order.iter().position(|n| *n == Node::Window(f)).map_or(order.len(), |i| i + 1),
        NewAt::Last => order.len(),
    };
    order.insert(i, Node::Window(id));
}

/// How the auto-tiled arrangement is reshaped (`reshape_tiles`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reshape {
    /// A quarter turn, clockwise or back.
    Rotate(bool),
    /// Mirrored left and right, or top and bottom.
    Flip(bool),
    EvenOut,
}

/// A step of Resize by direction: a twentieth of the display along that
/// way, at least `RESIZE_STEP_PX` points.
pub const RESIZE_STEP: f32 = 0.05;
pub const RESIZE_STEP_PX: f32 = 32.0;

/// Each leaf that takes room (the windows, and the places that hold
/// theirs) and its part of the area, from the top left. `min` is each
/// window's least size in fractions of the area, a whole gap with it: a
/// split gives each side at least that, where both fit, whatever its
/// ratio, less what of the gap a tile at the area's edge doesn't give up
/// (`inset`: there it has room to its least size and no more).
pub fn layout<'a>(node: &'a Node, min: &dyn Fn(u64) -> (f32, f32), inset: Inset) -> Vec<(&'a Node, Part)> {
    let mut m = Vec::new();
    measure(node, min, &mut m);
    let mut out = Vec::new();
    if m[0].shows {
        place(node, (0.0, 0.0, 1.0, 1.0), &m, 0, &mut out, inset);
    }
    out
}

fn place<'a>(node: &'a Node, r: Part, m: &[Measure], i: usize, out: &mut Vec<(&'a Node, Part)>, inset: Inset) {
    let Node::Split { cut, ratio, a, b } = node else {
        out.push((node, r));
        return;
    };
    let (ia, ib) = (i + 1, i + 1 + m[i + 1].len);
    match (m[ia].shows, m[ib].shows) {
        (true, false) => place(a, r, m, ia, out, inset),
        (false, true) => place(b, r, m, ib, out, inset),
        (false, false) => {}
        (true, true) => {
            let (ra, rb) = divide(*cut, laid_ratio(*cut, *ratio, r, m[ia], m[ib], inset), r);
            place(a, ra, m, ia, out, inset);
            place(b, rb, m, ib, out, inset);
        }
    }
}

/// The ratio a split of room `r` is laid out at: as it is, unless its two
/// sides' least sizes (`ma`, `mb`) hold it off that.
fn laid_ratio(cut: Cut, ratio: f32, r: Part, ma: Measure, mb: Measure, inset: Inset) -> f32 {
    let (start, len) = span(r, cut);
    let pick = |mm: Measure| if cut == Cut::Columns { mm.min.0 } else { mm.min.1 };
    // Each side's least, its gap as it is there: a tile gives up `lead`
    // of it at its start and `trail` at its end, none at the area's edge
    // (each min counts a whole gap, `lead` and `trail`).
    let (lead, trail) = if cut == Cut::Columns { (inset.lead.0, inset.trail.0) } else { (inset.lead.1, inset.trail.1) };
    let (at_start, at_end) = (start <= 1e-6, start + len >= 1.0 - 1e-6);
    let need_a = pick(ma) - if at_start { lead } else { 0.0 };
    let need_b = pick(mb) - if at_end { trail } else { 0.0 };
    let (lo, hi) = (need_a.max(0.0) / len.max(1e-6), 1.0 - need_b.max(0.0) / len.max(1e-6));
    if lo <= hi {
        ratio.clamp(lo, hi)
    } else {
        // They don't both fit: each its share of what they want.
        pick(ma) / (pick(ma) + pick(mb)).max(1e-6)
    }
}

/// Whether `tree`, its splits taken from where windows are (`moved`:
/// these windows' frames in `area`; the others' parts, `part_of`), lays
/// the `moved` ones out just there (to a pixel: parts round), their and
/// the others' least sizes (`min`) holding no edge off where it is.
pub fn lays_out_as(tree: &Node, moved: &[(u64, Rectangle<i32, Logical>)], part_of: &dyn Fn(u64) -> Option<Part>, min: &dyn Fn(u64) -> (f32, f32), area: Rectangle<i32, Logical>, gap: i32) -> bool {
    let frame = |i: u64| moved.iter().find(|(m, _)| *m == i).map(|(_, r)| *r);
    let mut tree = tree.clone();
    take_ratios(&mut tree, &|i| frame(i).map(|r| fraction_of(area, r, gap)).or_else(|| part_of(i)), still_in(area));
    let inset = Inset::of(area, gap);
    let laid = as_laid_out(&tree, min, inset);
    layout(&laid, min, inset).into_iter().all(|(n, p)| {
        let Node::Window(i) = n else { return true };
        let Some(r) = frame(*i) else { return true };
        let q = part(area, p, gap);
        let near = |a: i32, b: i32| (a - b).abs() <= 1;
        near(q.loc.x, r.loc.x) && near(q.loc.y, r.loc.y) && near(q.loc.x + q.size.w, r.loc.x + r.size.w) && near(q.loc.y + q.size.h, r.loc.y + r.size.h)
    })
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
pub fn as_laid_out(node: &Node, min: &dyn Fn(u64) -> (f32, f32), inset: Inset) -> Node {
    fn go(node: &Node, r: Part, m: &[Measure], i: usize, inset: Inset) -> Node {
        let Node::Split { cut, ratio, a, b } = node else { return node.clone() };
        let (ia, ib) = (i + 1, i + 1 + m[i + 1].len);
        let ratio = if m[ia].shows && m[ib].shows { laid_ratio(*cut, *ratio, r, m[ia], m[ib], inset) } else { *ratio };
        let (ra, rb) = divide(*cut, ratio, r);
        Node::split(*cut, ratio, go(a, ra, m, ia, inset), go(b, rb, m, ib, inset))
    }
    let mut m = Vec::new();
    measure(node, min, &mut m);
    go(node, (0.0, 0.0, 1.0, 1.0), &m, 0, inset)
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

/// Where Main and Stack's main window is, by its place's side: beside
/// one edge, in the middle across, or in a corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainSide {
    Left,
    Right,
    Top,
    Bottom,
    Center,
    Corner { left: bool, top: bool },
}

fn main_side(place: Tile) -> MainSide {
    match place {
        Tile::Left | Tile::LeftThird | Tile::LeftTwoThirds | Tile::Cell => MainSide::Left,
        Tile::Right | Tile::RightThird | Tile::RightTwoThirds => MainSide::Right,
        Tile::Top | Tile::TopTwoThirds => MainSide::Top,
        Tile::Bottom | Tile::BottomTwoThirds => MainSide::Bottom,
        Tile::CenterThird | Tile::CenterTwoThirds => MainSide::Center,
        Tile::TopLeft => MainSide::Corner { left: true, top: true },
        Tile::TopRight => MainSide::Corner { left: false, top: true },
        Tile::BottomLeft => MainSide::Corner { left: true, top: false },
        Tile::BottomRight => MainSide::Corner { left: false, top: false },
    }
}

/// `tiles` one above the other (`Rows`) or side by side, each as big;
/// none, room for the next window.
fn evenly(cut: Cut, mut tiles: Vec<Node>) -> Node {
    let n = tiles.len();
    if n == 0 {
        return Node::hole();
    }
    let first = tiles.remove(0);
    if tiles.is_empty() {
        return first;
    }
    Node::split(cut, 1.0 / n as f32, first, evenly(cut, tiles))
}

/// What [`evenly`] made of them: its tiles in order, none for room left
/// for the next window; None if `node` isn't that shape.
fn evenly_of(cut: Cut, node: &Node) -> Option<Vec<Node>> {
    match node {
        Node::Split { cut: c, a, b, .. } if *c == cut && !matches!(**a, Node::Split { .. }) => {
            let mut rest = evenly_of(cut, b)?;
            rest.insert(0, (**a).clone());
            Some(rest)
        }
        Node::Split { .. } => None,
        n if *n == Node::hole() => Some(Vec::new()),
        n => Some(vec![n.clone()]),
    }
}

/// Main and Stack's tree (as dwm's, and its centred main's): `main` at
/// `at` (its part of the area as fractions: its place's, or where its
/// edges were dragged to), by `place`'s side, and `stack`, in order, in
/// the rest, each as big as the others there. Beside an edge, the rest
/// is a column (a row, under or over it); in the middle, a column either
/// side, the right one taking the first half (and the odd one); in a
/// corner, the rest of its column takes one and the other half the rest.
/// It keeps its place alone too (Ben's default, the middle two thirds,
/// is a window in the middle of a wide display, not all of it): a part
/// with no window in it yet is room for the next one. `away`, places
/// kept for windows away that take no room, go at the stack's end.
pub fn main_and_stack(mains: Vec<Node>, mut stack: Vec<Node>, away: Vec<Node>, place: Tile, at: Part) -> Node {
    use Cut::{Columns, Rows};
    let (x, y, w, h) = at;
    // (More than one main window: they share the main place, each as big,
    // one above the other, or side by side across the top or bottom.)
    let main = evenly(if place.mains_side_by_side() { Columns } else { Rows }, mains);
    let ratio = |r: f32| r.clamp(0.05, 0.95);
    // (A place kept that takes no room, in with the last of the windows.)
    let with_away = |mut tiles: Vec<Node>| {
        tiles.extend(away.iter().cloned());
        tiles
    };
    match main_side(place) {
        MainSide::Left => Node::split(Columns, ratio(w), main, evenly(Rows, with_away(stack))),
        MainSide::Right => Node::split(Columns, ratio(x), evenly(Rows, with_away(stack)), main),
        MainSide::Top => Node::split(Rows, ratio(h), main, evenly(Columns, with_away(stack))),
        MainSide::Bottom => Node::split(Rows, ratio(y), evenly(Columns, with_away(stack)), main),
        MainSide::Center => {
            let left = stack.split_off(stack.len().div_ceil(2));
            let (right, left) = if left.is_empty() { (with_away(stack), left) } else { (stack, with_away(left)) };
            Node::split(Columns, ratio(x), evenly(Rows, left), Node::split(Columns, ratio(w / (1.0 - x).max(0.05)), main, evenly(Rows, right)))
        }
        MainSide::Corner { left, top } => {
            // (One in the rest of its column, once there are two.)
            let below = if stack.len() >= 2 { stack.pop() } else { None };
            let (other, below) = match below {
                Some(b) => (evenly(Rows, stack), evenly(Rows, with_away(vec![b]))),
                None => (evenly(Rows, with_away(stack)), Node::hole()),
            };
            let column = if top { Node::split(Rows, ratio(h), main, below) } else { Node::split(Rows, ratio(y), below, main) };
            if left { Node::split(Columns, ratio(w), column, other) } else { Node::split(Columns, ratio(x), other, column) }
        }
    }
}

/// Main and Stack's main part `at` (fractions of the area) for `place`
/// made `step` (a fraction of the area along the way it grows) bigger, or
/// smaller (negative), from its side: the edge into the rest moves (both,
/// for a place in the middle, which stays in the middle; a corner's
/// column grows across). Kept between a tenth and nine tenths.
pub fn main_part_grown(place: Tile, at: Part, step: f32) -> Part {
    let (x, y, w, h) = at;
    let size = |s: f32| (s + step).clamp(0.1, 0.9);
    match main_side(place) {
        MainSide::Left => (0.0, y, size(w), h),
        MainSide::Right => {
            let w = size(w);
            (1.0 - w, y, w, h)
        }
        MainSide::Top => (x, 0.0, w, size(h)),
        MainSide::Bottom => {
            let h = size(h);
            (x, 1.0 - h, w, h)
        }
        MainSide::Center => {
            let nw = size(w);
            ((x - (nw - w) / 2.0).clamp(0.0, 1.0 - nw), y, nw, h)
        }
        MainSide::Corner { left, .. } => {
            let nw = size(w);
            (if left { 0.0 } else { 1.0 - nw }, y, nw, h)
        }
    }
}

/// The main window, the stack in its order and the main window's part
/// (as fractions of the area), if `tree` is Main and Stack's shape for
/// `place`, as [`main_and_stack`] builds it (places away that take no
/// room in the stack where they are). None if it isn't: the person
/// rearranged it, or it was fitted to its windows' least sizes.
pub fn main_and_stack_of(tree: &Node, place: Tile) -> Option<(Vec<Node>, Vec<Node>, Part)> {
    use Cut::{Columns, Rows};
    // The main windows: one, or a run of them as `main_and_stack` puts
    // them (none is a hole: room for one).
    let along = if place.mains_side_by_side() { Columns } else { Rows };
    let leaf = |n: &Node| if *n == Node::hole() { Some(vec![n.clone()]) } else { evenly_of(along, n).filter(|m| !m.is_empty()) };
    let split = |n: &Node, want: Cut| match n {
        Node::Split { cut, ratio, a, b } if *cut == want => Some((*ratio, (**a).clone(), (**b).clone())),
        _ => None,
    };
    match main_side(place) {
        MainSide::Left => {
            let (r, a, b) = split(tree, Columns)?;
            Some((leaf(&a)?, evenly_of(Rows, &b)?, (0.0, 0.0, r, 1.0)))
        }
        MainSide::Right => {
            let (r, a, b) = split(tree, Columns)?;
            Some((leaf(&b)?, evenly_of(Rows, &a)?, (r, 0.0, 1.0 - r, 1.0)))
        }
        MainSide::Top => {
            let (r, a, b) = split(tree, Rows)?;
            Some((leaf(&a)?, evenly_of(Columns, &b)?, (0.0, 0.0, 1.0, r)))
        }
        MainSide::Bottom => {
            let (r, a, b) = split(tree, Rows)?;
            Some((leaf(&b)?, evenly_of(Columns, &a)?, (0.0, r, 1.0, 1.0 - r)))
        }
        MainSide::Center => {
            let (x, left, inner) = split(tree, Columns)?;
            let (r, main, right) = split(&inner, Columns)?;
            let mut stack = evenly_of(Rows, &right)?;
            stack.extend(evenly_of(Rows, &left)?);
            Some((leaf(&main)?, stack, (x, 0.0, r * (1.0 - x), 1.0)))
        }
        MainSide::Corner { left, top } => {
            let (rx, a, b) = split(tree, Columns)?;
            let (column, other) = if left { (a, b) } else { (b, a) };
            let (ry, ca, cb) = split(&column, Rows)?;
            let (main, below) = if top { (ca, cb) } else { (cb, ca) };
            let mut stack = evenly_of(Rows, &other)?;
            stack.extend(evenly_of(Rows, &below)?);
            let (x, w) = if left { (0.0, rx) } else { (rx, 1.0 - rx) };
            let (y, h) = if top { (0.0, ry) } else { (ry, 1.0 - ry) };
            Some((leaf(&main)?, stack, (x, y, w, h)))
        }
    }
}

/// A leaf of Main and Stack's that has room: a window, or a place kept
/// that holds its room (not a hole: room for the next window).
fn has_room(n: &Node) -> bool {
    match n {
        Node::Window(_) => true,
        Node::Kept(k) => k.holds && k.who != Who::Nobody,
        Node::Split { .. } => false,
    }
}

/// How many tiles with room `tree` has: Main and Stack is built again
/// when that changes (a window came, went, or went away for a while).
fn tiles_with_room(tree: &Node) -> usize {
    let mut n = 0;
    tree.each_leaf(&mut |l| n += has_room(l) as usize);
    n
}

/// Main and Stack's tree for `tree` again, its main window in `place`:
/// from its shape as built for `was` (the place it was built for), its
/// main window and stack's order and where its edges were dragged (in a
/// place changed, its new part); a tree of another shape, or one not
/// built as Main and Stack (`was` None: another layout's, which may look
/// like it, two halves), in its order. A `new` window (not in it yet) is the main one, the one that
/// was first in the stack. Else the main window is `first` if it has
/// room there (a window made main), the one that was main taking its
/// place in the stack; else the one in the main place, else `main` (the
/// main window as last built), else `before` (the one before it: the
/// main window closed or hid, the one it took the place of comes back),
/// else the first. None for a tree with nothing in it.
#[allow(clippy::too_many_arguments)]
pub fn main_and_stack_again(tree: &Node, place: Tile, was: Option<Tile>, count: usize, new: Option<(u64, NewAt)>, first: Option<u64>, main: Option<u64>, before: Option<u64>) -> Option<Node> {
    let shape = was.and_then(|was| Some((main_and_stack_of(tree, was)?, was)));
    let (mut order, at) = match shape.clone() {
        Some(((mut mains, mut stack, at), was)) => {
            mains.append(&mut stack);
            (mains, if was == place { at } else { place.fraction() })
        }
        None => (leaves(tree), place.fraction()),
    };
    // (Room for the next window is the shape's to leave.)
    order.retain(|n| *n != Node::hole() && !n.is_gap());
    let shaped = shape.and_then(|((m, ..), _)| m.into_iter().next());
    // (A new window first is the main one; anywhere else, the main one
    // stays.)
    let (first, shaped) = match new {
        Some((id, NewAt::First)) => {
            order.insert(0, Node::Window(id));
            (Some(id), None)
        }
        Some((id, at)) => {
            put_new(&mut order, id, at);
            (first, shaped)
        }
        None => (first, shaped),
    };
    let pick = |want: Option<Node>, order: &[Node]| want.and_then(|w| order.iter().position(|n| *n == w && has_room(n)));
    let i = pick(first.map(Node::Window), &order)
        .or_else(|| pick(shaped, &order))
        .or_else(|| pick(main.map(Node::Window), &order))
        .or_else(|| pick(before.map(Node::Window), &order))
        .or_else(|| order.iter().position(has_room));
    let Some(i) = i else {
        // (Only places away: kept as they are.)
        return (!order.is_empty()).then(|| tree.clone());
    };
    // The one in the main place goes where the new main window was.
    let chosen = order.remove(i);
    if i > 0 && !order.is_empty() && first.is_some() {
        let old = order.remove(0);
        order.insert(i - 1, old);
    }
    let (mut stack, away): (Vec<Node>, Vec<Node>) = order.into_iter().partition(has_room);
    // The next ones in order are main windows too, up to `count` (dwm's
    // nmaster): one fewer, the last of them is the first of the stack.
    let more = count.max(1).saturating_sub(1).min(stack.len());
    let mut mains = vec![chosen];
    mains.extend(stack.drain(..more));
    Some(main_and_stack(mains, stack, away, place, at))
}

/// Every leaf (windows and kept places), in reading order.
/// The layouts built in a shape of their own from the tiles in order
/// (tiling window managers' Columns, Rows, Grid and Spiral: awesome's
/// fair and spiral, xmonad's Grid, Amethyst's Column and Row): `tree`'s
/// tiles with room, in its order, laid out so; places kept with no room
/// go on at the end (they take none). None for a layout not built so, or
/// a tree with no tile with room. `aspect`: the area's width over its
/// height (a grid's rows and columns about square).
pub fn built_again(layout: olo_protocol::TilingLayout, tree: &Node, aspect: f32) -> Option<Node> {
    use olo_protocol::TilingLayout as L;
    // (In the layout's own order: Spiral's tree has a tile on the right
    // or below after the rest, and read as its tree has them, a window
    // opening or closing moved the others round.)
    let all = in_order(layout, Tile::Left, tree);
    let (room, rest): (Vec<Node>, Vec<Node>) = all.into_iter().partition(has_room);
    if room.is_empty() {
        return None;
    }
    let built = match layout {
        L::Columns => evenly(Cut::Columns, room),
        L::Rows => evenly(Cut::Rows, room),
        L::Grid => grid(room, aspect),
        L::Spiral => spiral(room)?,
        L::Halves | L::MainAndStack => return None,
    };
    // (Kept places with no room: beside it, taking none.)
    Some(rest.into_iter().fold(built, |t, k| Node::split(Cut::Columns, 1.0, t, k)))
}

/// `nodes` in a grid: as xmonad's, the square root of how many columns
/// (rounded up), filled in reading order, the last row's cells wider
/// where it has fewer; on a display more than twice as wide as tall, a
/// column more for each further width.
fn grid(nodes: Vec<Node>, aspect: f32) -> Node {
    let n = nodes.len().max(1);
    let wide = (aspect / 2.0).floor().max(0.0) as usize;
    let cols = ((n as f32).sqrt().ceil() as usize + wide).clamp(1, n);
    let rows: Vec<Node> = nodes.chunks(cols).map(|row| evenly(Cut::Columns, row.to_vec())).collect();
    evenly(Cut::Rows, rows)
}

/// `nodes` in a spiral: each takes half of the room left, going round
/// (left, top, right, bottom), the last the rest.
fn spiral(mut nodes: Vec<Node>) -> Option<Node> {
    let last = nodes.pop()?;
    // From the inside out: the `i`th node takes its half on the side the
    // spiral turns to at step `i` (0 left, 1 top, 2 right, 3 bottom).
    Some(nodes.into_iter().enumerate().rev().fold(last, |inner, (i, node)| match i % 4 {
        0 => Node::split(Cut::Columns, 0.5, node, inner),
        1 => Node::split(Cut::Rows, 0.5, node, inner),
        2 => Node::split(Cut::Columns, 0.5, inner, node),
        _ => Node::split(Cut::Rows, 0.5, inner, node),
    }))
}

/// `tree`'s leaves in the order of the windows it was built from, as
/// `layout` (with Main and Stack's main window in `place`): Main and
/// Stack's main window then its stack; Spiral's round from the outside in
/// (its tree has a tile on the right or below after the rest); the
/// others' (and a tree another shape than its layout's, refitted or
/// rearranged) as their tree has them.
pub fn in_order(layout: olo_protocol::TilingLayout, place: Tile, tree: &Node) -> Vec<Node> {
    use olo_protocol::TilingLayout as L;
    let all = leaves(tree);
    let read = match layout {
        L::MainAndStack => main_and_stack_of(tree, place).map(|(mut mains, mut stack, _)| {
            mains.append(&mut stack);
            mains
        }),
        L::Spiral => spiral_order(tree),
        _ => None,
    };
    let Some(mut order) = read else { return all };
    // (Any leaf the shape didn't name, on at the end.)
    for n in all {
        if !order.contains(&n) {
            order.push(n);
        }
    }
    order
}

/// The tiles of a tree shaped as [`spiral`] builds them, in the order
/// they were given; None for another shape.
fn spiral_order(tree: &Node) -> Option<Vec<Node>> {
    // Places kept with no room, put on at the end (`built_again`).
    let mut rest = Vec::new();
    let mut t = tree;
    while let Node::Split { cut: Cut::Columns, ratio, a, b } = t
        && *ratio >= 1.0 - 1e-6
        && !has_room(b)
        && !matches!(**b, Node::Split { .. })
    {
        rest.insert(0, (**b).clone());
        t = a;
    }
    let mut order = Vec::new();
    let mut depth = 0;
    while let Node::Split { cut, a, b, .. } = t {
        let (want, in_b) = match depth % 4 {
            0 => (Cut::Columns, false),
            1 => (Cut::Rows, false),
            2 => (Cut::Columns, true),
            _ => (Cut::Rows, true),
        };
        let (node, inner) = if in_b { (b, a) } else { (a, b) };
        if *cut != want || matches!(**node, Node::Split { .. }) {
            return None;
        }
        order.push((**node).clone());
        t = inner;
        depth += 1;
    }
    order.push(t.clone());
    order.extend(rest);
    Some(order)
}

/// `tree`'s tiles with room in order, each next one taking half of the
/// room left across its longer side (`aspect`: the area's width over its
/// height), places kept with no room on at the end: Halves' shape for
/// tiles that came one after another. None with no tile with room.
pub fn dwindle(tree: &Node, aspect: f32) -> Option<Node> {
    let (room, rest): (Vec<Node>, Vec<Node>) = leaves(tree).into_iter().partition(has_room);
    let mut cuts = Vec::new();
    let (mut w, mut h) = (aspect.max(0.01), 1.0f32);
    for _ in 1..room.len() {
        let cut = if w >= h { Cut::Columns } else { Cut::Rows };
        if cut == Cut::Columns {
            w /= 2.0;
        } else {
            h /= 2.0;
        }
        cuts.push(cut);
    }
    let mut nodes = room;
    let last = nodes.pop()?;
    let built = nodes.into_iter().zip(cuts).rev().fold(last, |rest, (node, cut)| Node::split(cut, 0.5, node, rest));
    Some(rest.into_iter().fold(built, |t, k| Node::split(Cut::Columns, 1.0, t, k)))
}

fn leaves(node: &Node) -> Vec<Node> {
    let mut out = Vec::new();
    node.each_leaf(&mut |n| out.push(n.clone()));
    out
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
        Tile::TopTwoThirds => Node::split(Rows, 2.0 * T, me, whole(rest)),
        Tile::BottomTwoThirds => Node::split(Rows, T, whole(rest), me),
        Tile::CenterThird | Tile::CenterTwoThirds => {
            let (x, _, w, _) = tile.fraction();
            let (left, right) = take_nearest(rest, (x / 2.0, 0.5), centre);
            Node::split(Columns, x, left, Node::split(Columns, w / (1.0 - x), me, right))
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
        Tile::CenterTwoThirds => (1.0 / 6.0, 1.0),
        Tile::TopTwoThirds | Tile::BottomTwoThirds => (1.0, 1.0 / 3.0),
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
/// their edges were dragged: where they were let go. An edge within
/// `still` of its split (in fractions of the area across and down: what
/// the parts' pixels round by) didn't move, and its split stays as it is.
pub fn take_ratios(node: &mut Node, part_of: &dyn Fn(u64) -> Option<Part>, still: (f32, f32)) {
    let mut boxes = Vec::new();
    bounds(node, part_of, &mut boxes);
    let mut i = 0;
    set_ratios(node, (0.0, 0.0, 1.0, 1.0), &boxes, &mut i, still);
}

/// `take_ratios`' `still` for `area`: a pixel and a half (an edge's part
/// rounds to a pixel, and its split is half way across the gap).
pub fn still_in(area: Rectangle<i32, Logical>) -> (f32, f32) {
    (1.5 / area.size.w.max(1) as f32, 1.5 / area.size.h.max(1) as f32)
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

fn set_ratios(node: &mut Node, r: Part, boxes: &[(Option<Part>, usize)], i: &mut usize, still: (f32, f32)) {
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
        if (e - was).abs() > if *cut == Cut::Columns { still.0 } else { still.1 } {
            *ratio = ((e - start) / len).clamp(0.05, 0.95);
        }
    }
    let (ra, rb) = divide(*cut, *ratio, r);
    set_ratios(a, ra, boxes, i, still);
    *i = ib;
    set_ratios(b, rb, boxes, i, still);
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

/// Main and Stack's main window of a tree, as last built, and the one
/// that was before it (still in the tree).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Mains {
    now: Option<u64>,
    before: Option<u64>,
}

/// app_server's auto-tiling: a tree per workspace and display (by
/// connector) that has it on.
#[derive(Debug)]

pub struct AutoTiler {
    trees: HashMap<(u32, String), Node>,
    /// The layout each tree was last built in, and Main and Stack's main
    /// window's place: chosen anew, it's built in that one (Main and
    /// Stack's shape), once.
    built_as: HashMap<(u32, String), (olo_protocol::TilingLayout, Tile, u32)>,
    /// Main and Stack: each tree's main window, and the one before it
    /// (Swap with Previous Main).
    mains: HashMap<(u32, String), Mains>,
    /// Each tree as last laid out with all its windows there
    /// ([`as_laid_out`]): what a restart keeps, though windows are
    /// closing as it comes (an upgrade re-executes their apps).
    laid: HashMap<(u32, String), Node>,
    /// The saved places (slots) each window that came back took a kept
    /// tile for: its group's, the others of which are its tabs to be
    /// (`auto_output` gives them no tile of their own as they come).
    kept_for: HashMap<u64, Vec<u64>>,
    /// Windows put back from a restart, waiting for their group to form
    /// (in a tile kept for it, or a mate's): no tile of their own given
    /// meanwhile. Let go once in a tile or a tiled group, once a hand
    /// groups or ungroups one (`auto_settled`), and as `RESTORING` ends.
    awaiting: std::collections::HashSet<u64>,
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
            built_as: HashMap::new(),
            mains: HashMap::new(),
            laid: HashMap::new(),
            kept_for: HashMap::new(),
            awaiting: std::collections::HashSet::new(),
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

pub(crate) fn fullscreen(w: &WindowElement) -> bool {
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

    /// Floats whatever is asked: picture in picture, a window that names
    /// its parent (a dialog, Quick Look), an app's alert. Each belongs
    /// over another window (or all its app's: `keep_children_above`), and
    /// a tile can't be over the floats: tiled by hand, an alert stayed
    /// over its app's floating windows as their child, a tile over floats.
    pub fn floats_always(w: &WindowElement) -> bool {
        if w.meta().pip.get() {
            return true;
        }
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        t.parent().is_some() || super::groups::is_parentless_alert(w)
    }

    /// Whether `w` floats over auto-tiled windows rather than being in a
    /// tile: floated by hand, or a dialog, an alert or other fixed-size
    /// window, one whose largest size is less than its display's tiles,
    /// one tagged `floating`, or a kind that floats (`floating_kind`).
    /// Tiled by hand, it doesn't.
    pub fn floats(&self, w: &WindowElement) -> bool {
        let WindowSurface::Wayland(t) = w.0.underlying_surface();
        // (On all workspaces, as i3's sticky: a tile belongs to one.)
        if w.meta().pip.get() || t.parent().is_some() || w.meta().all_workspaces.get() {
            return true;
        }
        if let Some(f) = Self::float_override(w) {
            // (An alert tiled by hand before that was refused, from a
            // restart's saved state: it floats. Asked only of those, not
            // every window every turn.)
            return f || super::groups::is_parentless_alert(w);
        }
        // (All it needs of the surface's state at one go: this is asked of
        // every window on every turn of the loop.)
        use smithay::wayland::{
            compositor::with_states,
            shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData, dialog::ToplevelDialogHint},
            xdg_toplevel_tag::XdgToplevelTagSurfaceData,
        };
        // Apps chosen to float always (Float Window ▸ Always): most
        // computers have none, and the app id is compared where it is.
        let apps = &self.settings.tiling.floating_apps;
        let (floats, min, max) = with_states(t.wl_surface(), |st| {
            let size = *st.cached_state.get::<SurfaceCachedState>().current();
            let tagged = st.data_map.get::<XdgToplevelTagSurfaceData>().is_some_and(|d| d.tag().is_some_and(|t| t.split_whitespace().any(|t| t == "floating")));
            let floats = tagged
                || st.data_map.get::<XdgToplevelSurfaceData>().is_some_and(|d| {
                    let d = d.lock().unwrap();
                    let app = d.app_id.as_deref().unwrap_or("");
                    // (A dialog is one "relative to its parent", and one with
                    // a parent floats already: GTK 4 gives every toplevel an
                    // xdg_dialog_v1, which makes its main windows "dialogs".)
                    d.dialog_hint == ToplevelDialogHint::Modal || floating_kind(app, d.title.as_deref().unwrap_or("")) || (!app.is_empty() && apps.iter().any(|a| a == app))
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
                        as_laid_out(t, &|id| mins.get(&id).copied().unwrap_or((0.0, 0.0)), Inset::of(area, self.tile_gap()))
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
    pub(super) fn auto_mins(&self, windows: &[WindowElement], area: Rectangle<i32, Logical>) -> HashMap<u64, (f32, f32)> {
        let gap = self.tile_gap();
        // (And a pixel either side: each of a tile's edges rounds to a
        // pixel, which took up to two from a tile at its least size. Laid
        // out 288 wide, StyledEdit drew its 290 over the gap beside it:
        // auto-fuzz seed 38.)
        let edges = 2;
        windows
            .iter()
            .map(|w| {
                let m = self.min_frame(w);
                (w.id(), ((m.w + gap + edges) as f32 / area.size.w.max(1) as f32, (m.h + gap + edges) as f32 / area.size.h.max(1) as f32))
            })
            .collect()
    }

    /// Lay out again what changed (`tick`): each display of this
    /// workspace with auto-tiling on, if its windows or room changed, a
    /// command changed its tree, or a kept place is due to go; trees of
    /// displays turned off are dropped.
    /// Whether auto-tiling is in use (anything tiled, or a mode for it
    /// set): then it looks closely every `LOOK`, and the loop wakes for it.
    pub fn auto_tiling(&self) -> bool {
        let t = &self.settings.tiling;
        let auto = olo_protocol::WindowingMode::AutoTiling;
        !self.auto.trees.is_empty() || t.default_mode == auto || t.modes.iter().any(|m| m.mode == auto)
    }

    /// How long the event loop may sleep with nothing coming: auto-tiling's
    /// `LOOK` while it's in use, else a second, a safety net for anything
    /// changed off the loop (other threads tell it through channels).
    pub fn auto_loop_timeout(&self) -> Duration {
        if self.auto_tiling() { LOOK } else { Duration::from_secs(1) }
    }

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
        // (Not one behind another in its group: its group's tile is the
        // front's. Put back in a place kept for it, it was a tile of its own
        // as well, seconds after a restart, once a hand had grouped it.)
        let back: Vec<(u64, u64)> = self.all_windows().iter().filter(|w| w.meta().placed.get() && w.meta().slot.get() > 0 && !self.behind_in_group(w)).map(|w| (w.meta().slot.get(), w.id())).collect();
        if back.is_empty() {
            return;
        }
        let mut changed = false;
        for t in self.auto.trees.values_mut() {
            for &(slot, id) in &back {
                if t.contains(id) {
                    continue;
                }
                let mut slots = None;
                t.each_leaf(&mut |n| {
                    if let Node::Kept(Kept { who: Who::Returning(s), .. }) = n
                        && slots.is_none()
                        && s.contains(&slot)
                    {
                        slots = Some(s.clone());
                    }
                });
                let took = t.replace(&|n| matches!(n, Node::Kept(Kept { who: Who::Returning(s), .. }) if s.contains(&slot)), Node::Window(id));
                if took {
                    info!(window = id, slot, "back in its tile");
                    self.auto.kept_for.insert(id, slots.unwrap_or_else(|| vec![slot]));
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
        let tiles_before = tree.as_ref().map_or(0, tiles_with_room);
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
            // Heirs given a tile here already (several of a group's
            // windows, each in a tile of its own, gone into it at once, as
            // their groups come back after a restart): the first keeps it,
            // the rest close (each given to the heir, it was laid out in
            // two tiles, its group's windows each sized for one).
            let mut heirs: Vec<u64> = Vec::new();
            for id in gone {
                let known = self.auto.known.get(&id).cloned();
                let alive = self.window_by_id(id);
                // Its group: the one it is behind in now, else the one it
                // was in (closed, minimized).
                let group = alive.as_ref().and_then(|w| self.groups.place(w)).map(|(n, _)| n).or(known.as_ref().and_then(|k| k.group));
                let heir = group.and_then(|g| units.iter().find(|u| self.groups.place(u).map(|(n, _)| n) == Some(g)).map(|u| u.id()));
                if let Some(h) = heir
                    && heirs.contains(&h)
                {
                    let rest = std::mem::replace(t, Node::gap());
                    *t = rest.without(&|n| *n == Node::Window(id)).unwrap_or_else(Node::gap);
                    continue;
                }
                heirs.extend(heir);
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
        // The windows new to it. (Not one just put back from a restart
        // that's about to rejoin its group, a tab of a window here: given
        // a tile of its own first, it split a neighbour's for a few
        // frames, till the group formed.)
        if self.auto.started.elapsed() >= RESTORING {
            self.auto.kept_for.clear();
            self.auto.awaiting.clear();
        }
        // Waiting no longer: in a tile, or in a group with one that is.
        let in_trees = |id: u64, s: &Self| s.auto.trees.values().any(|t| t.contains(id)) || tree.as_ref().is_some_and(|t| t.contains(id));
        let settled: Vec<u64> = self
            .auto
            .awaiting
            .iter()
            .copied()
            .filter(|&id| match self.window_by_id(id) {
                None => true,
                Some(w) => in_trees(id, self) || self.groups.mates(&w).iter().any(|m| m.id() != id && in_trees(m.id(), self)),
            })
            .collect();
        for id in settled {
            self.auto.awaiting.remove(&id);
        }
        // (Its group's tile taken by a mate already, shown or not yet: its
        // tabs may make a group of their own first, before the mate holding
        // the tile shows to join.)
        let mates_tile = |w: &WindowElement| {
            let slot = w.meta().slot.get();
            slot > 0 && self.auto.kept_for.iter().any(|(id, s)| *id != w.id() && s.contains(&slot) && tree.as_ref().is_some_and(|t| t.contains(*id)))
        };
        let fresh: Vec<WindowElement> = units
            .iter()
            .filter(|w| !tree.as_ref().is_some_and(|t| t.contains(w.id())))
            .filter(|w| !(self.auto.awaiting.contains(&w.id()) && (mates_tile(w) || (self.groups.number_of(w).is_none() && self.joins_a_group(w)))))
            .cloned()
            .collect();
        for w in &fresh {
            self.auto_insert(&mut tree, w, area, &key);
            // Gone from the place it left in another tree.
            for t in self.auto.trees.values_mut() {
                let id = w.id();
                if t.any_leaf(&|n| matches!(n, Node::Kept(Kept { who: Who::Window(i), .. }) if *i == id)) {
                    let rest = std::mem::replace(t, Node::gap());
                    *t = rest.without(&|n| matches!(n, Node::Kept(Kept { who: Who::Window(i), .. }) if *i == id)).unwrap_or_else(Node::gap);
                }
            }
        }
        // Main and Stack: built in its shape when it's just been chosen (or
        // its main window's place), or as the number of tiles with room
        // changes (a window came, went, or hid: the stack each as big
        // again, and a main window gone gives its place to the one before
        // it); otherwise as the person left it.
        let layout = self.layout_of(&key);
        let (place, was) = self.main_places(&key);
        // Another layout chosen: the tiles in the order the last one had
        // them (Spiral's and Main and Stack's isn't their tree's), so
        // going round the layouts keeps the windows' order, as tiling
        // window managers keep their window list.
        let prev = self.auto.built_as.get(&key).copied();
        let switched = prev.is_some_and(|(l, ..)| l != layout);
        let count = self.main_count_of(&key);
        if let Some((l, p, _)) = prev
            && switched
            && let Some(t) = tree.as_mut()
            && tiles_with_room(t) > 0
        {
            *t = evenly(Cut::Columns, in_order(l, p, t));
        }
        if let Some(t) = tree.as_mut()
            && layout == olo_protocol::TilingLayout::MainAndStack
            && (self.auto.built_as.get(&key) != Some(&(layout, place, count)) || tiles_with_room(t) != tiles_before)
        {
            let m = self.auto.mains.get(&key).copied().unwrap_or_default();
            if let Some(built) = main_and_stack_again(t, place, was, count as usize, None, None, m.now, m.before) {
                *t = built;
            }
        }
        // Columns, Rows, Grid and Spiral: built in their shape from the
        // tiles in order as the layout is chosen and as the number of
        // tiles with room changes; between those, as the person left it.
        // (Switched: built as another layout before. Not on the first look
        // after a restart, with nothing built yet: the tree kept is the
        // person's.)
        let aspect = area.size.w as f32 / area.size.h.max(1) as f32;
        if let Some(t) = tree.as_mut()
            && layout.built()
            && (switched || tiles_with_room(t) != tiles_before)
            && let Some(built) = built_again(layout, t, aspect)
        {
            *t = built;
        }
        // Halves, chosen: the tiles in order as it would have made them
        // (each the next one's half, across its longer side), as bspwm and
        // Hyprland's dwindle rearrange on a change of layout; from then on
        // it grows a split at a time.
        if let Some(t) = tree.as_mut()
            && layout == olo_protocol::TilingLayout::Halves
            && switched
            && let Some(built) = dwindle(t, aspect)
        {
            *t = built;
        }
        self.auto.built_as.insert(key.clone(), (layout, place, count));
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
        // Which is the main window now (as built, or as the person
        // swapped tiles), and the one before it. (A tree of another shape,
        // fitted to its windows' least sizes, keeps what it knew.)
        if layout == olo_protocol::TilingLayout::MainAndStack
            && let Some(t) = &tree
            && let Some((m, ..)) = main_and_stack_of(t, place)
        {
            let now = if let Some(Node::Window(id)) = m.first() { Some(*id) } else { None };
            let rec = self.auto.mains.entry(key.clone()).or_default();
            if rec.now != now {
                if rec.now.is_some_and(|o| t.contains(o)) {
                    rec.before = rec.now;
                } else if rec.before == now {
                    rec.before = None;
                }
                rec.now = now;
                info!(output = key.1, main = ?now, before = ?rec.before, "the main window");
            }
        }
        if let Some(t) = tree {
            let now = t.to_string();
            if before.as_ref() != Some(&now) {
                info!(output = key.1, tree = now, "tiles");
            }
            if !t.any_leaf(&|n| closing(n)) {
                let mins = self.auto_mins(&units, area);
                self.auto.laid.insert(key.clone(), as_laid_out(&t, &|id| mins.get(&id).copied().unwrap_or((0.0, 0.0)), Inset::of(area, self.tile_gap())));
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
    fn auto_insert(&self, tree: &mut Option<Node>, w: &WindowElement, area: Rectangle<i32, Logical>, key: &(u32, String)) {
        let id = w.id();
        let Some(t) = tree.as_mut() else {
            // (Main and Stack's main window keeps its place alone too.)
            *tree = Some(match self.layout_of(key) {
                olo_protocol::TilingLayout::MainAndStack => {
                    let place = self.main_places(key).0;
                    main_and_stack(vec![Node::Window(id)], Vec::new(), Vec::new(), place, place.fraction())
                }
                _ => Node::Window(id),
            });
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
        // Main and Stack: the new window is the main one, the one that was
        // first in the stack (the shape leaves its own room for the next
        // window, not a hole a tiling command left).
        // Where it goes in the order (`TilingSettings::new_windows`):
        // after the tile that had the focus last, if that's the choice.
        let at = match self.settings.tiling.new_windows {
            olo_protocol::NewWindows::First => NewAt::First,
            olo_protocol::NewWindows::Last => NewAt::Last,
            olo_protocol::NewWindows::AfterActive => {
                let in_tree = t.windows();
                self.recent.iter().find(|r| **r != id && in_tree.contains(r)).map_or(NewAt::First, |r| NewAt::After(*r))
            }
        };
        let chosen = self.layout_of(key);
        if chosen == olo_protocol::TilingLayout::MainAndStack {
            let (place, was) = self.main_places(key);
            let m = self.auto.mains.get(key).copied().unwrap_or_default();
            if let Some(built) = main_and_stack_again(t, place, was, self.main_count_of(key) as usize, Some((id, at)), None, m.now, m.before) {
                *t = built;
            }
            return;
        }
        // Columns, Rows, Grid, Spiral: into their order, built again in
        // their shape as the number of tiles changes (not a split of a
        // tile, which another shape has no place for).
        if chosen.built() {
            let mut order = in_order(chosen, Tile::Left, t);
            put_new(&mut order, id, at);
            // (Built here: claimed at its first commit, the tree has it
            // before the rebuild looks, which then sees no new tile.)
            let flat = evenly(Cut::Columns, order);
            *t = built_again(chosen, &flat, area.size.w as f32 / area.size.h.max(1) as f32).unwrap_or(flat);
            return;
        }
        let windows = self.all_windows();
        let mins = self.auto_mins(&windows, area);
        let min = |i: u64| mins.get(&i).copied().unwrap_or((0.0, 0.0));
        let laid: Vec<(Node, Part)> = layout(t, &min, Inset::of(area, self.tile_gap())).into_iter().map(|(n, p)| (n.clone(), p)).collect();
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
    /// drag, and so are the neighbours following its edges (the tree has
    /// their tiles where they were till the drag ends: laid out, they were
    /// asked back there each turn, against the drag).
    fn auto_apply(&mut self, output: &Output, tree: &Node, units: &[WindowElement]) {
        // Every window changing size in one round (unless a drag's last
        // round is being made, which they join): they land together.
        let own_round = self.round_collecting.is_none();
        if own_round {
            self.round_collecting = Some(Vec::new());
        }
        self.auto_apply_tiles(output, tree, units);
        if own_round {
            let windows = self.round_collecting.take().unwrap_or_default();
            self.round_last(windows);
        }
    }

    /// Where `tree`'s tiles go on `output`, and the gap between them: the
    /// tiling area, or, a tile alone with smart gaps on, the whole usable
    /// area and none.
    fn auto_area(&self, output: &Output, tree: &Node) -> Option<(Rectangle<i32, Logical>, i32)> {
        if self.settings.tiling.smart_gaps && tiles_with_room(tree) == 1 { Some((self.usable_area(output)?, 0)) } else { Some((self.tile_area(output)?, self.tile_gap())) }
    }

    /// The area auto-tiled `w`'s tile is in ([`Self::auto_area`]), if it
    /// is one on the display `w` is on. (Just sent to another display, it
    /// is still in the tree it left till the tiler looks again: held to
    /// that tree's area, its first commit there pulled it back across,
    /// where it stayed, over the tiles; tile-qa, seed 51.)
    pub(super) fn auto_area_of(&self, w: &WindowElement) -> Option<(Rectangle<i32, Logical>, i32)> {
        let key = self.tree_of(w)?;
        let output = self.space.outputs().find(|o| o.name() == key.1)?;
        if self.window_output(w).as_ref() != Some(output) {
            return None;
        }
        self.auto_area(output, self.auto.trees.get(&key)?)
    }

    fn auto_apply_tiles(&mut self, output: &Output, tree: &Node, units: &[WindowElement]) {
        let Some((area, gap)) = self.auto_area(output, tree) else { return };
        let mins = self.auto_mins(units, area);
        let min = |i: u64| mins.get(&i).copied().unwrap_or((0.0, 0.0));
        let moving_by_drag = |s: &Self, w: &WindowElement| s.tile_drag.as_ref().is_some_and(|d| d.moves(w));
        let placed: Vec<(u64, Part)> = layout(tree, &min, Inset::of(area, gap)).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect();
        // (Development builds, every test session's: the parts as laid
        // out hold to the tree's invariants, said in the log if not, where
        // tile-qa looks.)
        if cfg!(debug_assertions)
            && let Some(wrong) = parts_wrong(&placed)
        {
            tracing::error!(output = output.name(), tree = %tree, wrong, "tiles laid out wrong");
        }
        for (id, f) in placed {
            let Some(w) = units.iter().find(|w| w.id() == id).cloned() else { continue };
            let was_tiled = Self::tiled(&w).is_some();
            Self::set_tile_state(&w, Some(Tile::Cell), Some(f));
            if self.is_zoomed(&w) || fullscreen(&w) || self.auto.moving.as_ref() == Some(&w) || moving_by_drag(self, &w) {
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
        // (Where it is now is where it stays unless moved here: a move a
        // layout a moment ago queued is given up.)
        self.round_unmove(w);
        if geo.size == rect.size && super::groups::size_on_the_way(w).is_none() {
            // Drawn at its tile's size, and asked for no other: there now.
            // (Following, it is where it was going: it would move nowhere
            // else, and no commit may come to move it.)
            // (Laid out as a drag is let go: it moves with the drag's last
            // round, as the tiles changing size draw theirs, not before.)
            if geo != rect {
                let Some(loc) = self.space.element_location(w) else { return };
                if self.round_collecting.is_some() {
                    self.round_move(w, loc + (rect.loc - geo.loc));
                } else {
                    self.space.relocate_element(w, loc + (rect.loc - geo.loc));
                }
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
        if t.is_initial_configure_sent() || w.meta().placed.get() || self.floats(&w) {
            return;
        }
        if w.meta().returning.borrow().is_some() {
            return self.auto_claim_returning(&w);
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
        self.auto_insert(&mut tree, &w, area, &key);
        self.auto.trees.insert(key, tree.unwrap_or(Node::Window(w.id())));
        info!(window = w.id(), app_id = w.app_id(), "a new window into its tile");
        // At once, so its first configure has its tile's size.
        self.auto_output(&output);
    }

    /// A window coming back from a restart (claimed at its first commit,
    /// `claim_placement`) whose tile was kept for it on a display showing
    /// it: into that tile now, at the tile's size as it is now. Its saved
    /// frame is only where the tile was: the tiles may have moved since
    /// (a drag, others coming back), and shown there it was over its
    /// neighbour, out past the tiling area, till the next layout moved it.
    fn auto_claim_returning(&mut self, w: &WindowElement) {
        let Some(p) = w.meta().returning.borrow().clone() else { return };
        if p.tile.is_none() || p.floating.is_some() || p.minimized || p.slot == 0 {
            return;
        }
        let Some(output) = self.saved_display(&p) else { return };
        let ws = self.showing(&output);
        if p.workspace != ws || !self.auto_on(&output, ws) {
            return;
        }
        let (id, slot) = (w.id(), p.slot);
        let mine = |n: &Node| match n {
            Node::Kept(Kept { who: Who::Returning(s), .. }) => s.contains(&slot),
            Node::Kept(Kept { who: Who::Window(i), .. }) => *i == id,
            Node::Kept(Kept { who: Who::Like { app_id, title }, .. }) => *app_id == p.app_id && *title == p.title,
            _ => false,
        };
        let Some(tree) = self.auto.trees.get_mut(&(ws, output.name())) else { return };
        let mut slots = None;
        tree.each_leaf(&mut |n| {
            if let Node::Kept(Kept { who: Who::Returning(s), .. }) = n
                && slots.is_none()
                && s.contains(&slot)
            {
                slots = Some(s.clone());
            }
        });
        if !tree.replace(&mine, Node::Window(id)) {
            return;
        }
        self.auto.kept_for.insert(id, slots.unwrap_or_else(|| vec![slot]));
        // On its display, so it is one of its tiles.
        let at = self.saved_location(&p);
        self.space.relocate_element(w, at);
        info!(window = id, app_id = w.app_id(), slot, "back into the tile kept for it");
        // At once, so its first configure has its tile's size.
        self.auto_output(&output);
    }

    /// `w` put back from a restart: it waits for its group to form before
    /// it's given a tile of its own (`awaiting`).
    pub(crate) fn auto_note_returning(&mut self, w: &WindowElement) {
        if self.auto.started.elapsed() < RESTORING {
            self.auto.awaiting.insert(w.id());
        }
    }

    /// A hand grouped or ungrouped `w`: it's as it is now, not waiting for
    /// the group it came back in.
    pub(super) fn auto_settled(&mut self, w: &WindowElement) {
        self.auto.awaiting.remove(&w.id());
    }

    /// In an auto-tiling tree on a display showing it: laid out by it.
    pub(crate) fn in_auto_tile(&self, w: &WindowElement) -> bool {
        self.tree_of(w).is_some()
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

    /// Floating windows over the tiles of auto-tiled displays, and among
    /// themselves in the order they are (the one brought forward last on
    /// top). Run as the tiles are laid out and wherever a window is raised
    /// (`focus_window`: a tile activated comes over them only for a
    /// moment).
    pub(crate) fn keep_floating_above(&mut self) {
        if !self.auto.trees.keys().any(|(ws, o)| *ws == self.showing_named(o)) {
            return;
        }
        let leaves = self.auto_leaves();
        let order: Vec<WindowElement> = self.space.elements().cloned().collect();
        let Some(top_tile) = order.iter().rposition(|w| leaves.contains_key(&w.id())) else { return };
        let float = |w: &&WindowElement| !leaves.contains_key(&w.id()) && self.floats(w);
        if !order[..top_tile].iter().any(|w| float(&w)) {
            return;
        }
        // Every one raised, bottom first: over the tiles, in the order they
        // were. (Only those under a tile raised, one came over the float
        // brought forward last, and kept doing so: Ben's "one of them keeps
        // itself on top".)
        let floats: Vec<WindowElement> = order.iter().filter(float).cloned().collect();
        for w in floats {
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
        self.arrangement_step("Auto-Tiling", super::tiling::ArrangedKind::Auto);
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
        if Self::floats_always(w) {
            info!(window = w.id(), ?floating, "floating: it floats always (a dialog, an alert): left as it is");
            return;
        }
        let was = self.floats(w);
        self.set_floating_from(w, floating, was);
    }

    /// `set_floating`, with whether `w` floated before (`was`): asked
    /// before an app's rule changed, which changes it too.
    pub(crate) fn set_floating_from(&mut self, w: &WindowElement, floating: Option<bool>, was: bool) {
        for m in self.groups.mates(w).iter().chain(std::iter::once(w)) {
            Self::restore_float_override(m, floating);
            // (On purpose: what a restart saved isn't put back over it.
            // Floated by an agent, a window that came back as a tile was
            // put back in its old tile's place, floating, 11 s later.)
            m.meta().moved_on_purpose.set(true);
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

    /// Every window of app `app_id` floats from now on, or no longer does
    /// (`TilingSettings::floating_apps`, saved): the window menu's Float
    /// Window ▸ Always. Its windows open now let go of what a hand chose
    /// for each, so they all go by it at once; those that start floating
    /// leave their tiles as Float Window takes one out.
    pub fn set_app_floating(&mut self, app_id: &str, floating: bool) {
        if app_id.is_empty() {
            return;
        }
        let listed = self.settings.tiling.floating_apps.iter().any(|a| a == app_id);
        if floating == listed {
            // (Chosen again: each of its windows goes by it, any floated
            // or tiled by hand too.)
            let windows = self.apps_windows(&[app_id.to_owned()]);
            self.let_go(windows);
            return;
        }
        let windows = self.apps_windows(&[app_id.to_owned()]);
        let apps = &mut self.settings.tiling.floating_apps;
        if floating {
            apps.push(app_id.to_owned());
        } else {
            apps.retain(|a| a != app_id);
        }
        self.settings_changed();
        info!(app = app_id, floating, windows = windows.len(), "an app's windows float always, or not");
        self.let_go(windows);
    }

    /// The windows of `apps` (but those that float always), each with
    /// whether it floats now: asked before the apps' rule changes, which
    /// changes that too.
    pub(crate) fn apps_windows(&self, apps: &[String]) -> Vec<(WindowElement, bool)> {
        if apps.is_empty() {
            return Vec::new();
        }
        self.all_windows()
            .into_iter()
            .filter(|w| !Self::floats_always(w) && apps.contains(&w.app_id()))
            .map(|w| {
                let was = self.floats(&w);
                (w, was)
            })
            .collect()
    }

    /// Each of `windows` lets go of what a hand chose for it (floated, or
    /// tiled), to go by its app's rule; one that starts floating leaves
    /// its tile as Float Window takes it out.
    pub(crate) fn let_go(&mut self, windows: Vec<(WindowElement, bool)>) {
        for (w, was) in &windows {
            self.set_floating_from(w, None, *was);
        }
    }

    // ---- commands --------------------------------------------------------

    /// The tree a command on `w` works on, if `w` is a tile; kept for ⌃⌘Z,
    /// the step called `name`.
    fn auto_take(&mut self, w: &WindowElement, name: &str) -> Option<((u32, String), Node)> {
        let key = self.tree_of(w)?;
        self.auto_take_key(key, name)
    }

    /// [`auto_take`] of the tree `key` names (a workspace and display).
    fn auto_take_key(&mut self, key: (u32, String), name: &str) -> Option<((u32, String), Node)> {
        let tree = self.auto.trees.remove(&key)?;
        self.auto.undo = Some(Undo::Tree(key.clone(), Some(tree.clone())));
        self.arrangement_step(name, super::tiling::ArrangedKind::Auto);
        Some((key, tree))
    }

    /// Turning auto-tiling on is what ⌃⌘Z would take back: on which
    /// workspace and displays.
    pub(super) fn auto_turned_on_undo(&self) -> Option<(u32, Vec<String>)> {
        match &self.auto.undo {
            Some(Undo::TurnedOn(ws, outputs, _)) => Some((*ws, outputs.clone())),
            _ => None,
        }
    }

    fn auto_put(&mut self, key: (u32, String), tree: Node) {
        self.auto.trees.insert(key, tree);
        self.auto.dirty = true;
        self.auto_tick();
    }

    /// The middle of each tile of `tree`, in fractions of the area.
    fn auto_centres(&self, tree: &Node) -> HashMap<u64, (f32, f32)> {
        layout(tree, &|_| (0.0, 0.0), Inset::default()).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, (p.0 + p.2 / 2.0, p.1 + p.3 / 2.0))) } else { None }).collect()
    }

    /// A tiling command for a tile: its part, the rest tiled around it.
    /// False if `w` isn't a tile (the command does as it always does).
    pub fn auto_tile_window(&mut self, w: &WindowElement, tile: Tile) -> bool {
        if self.floats(w) && self.window_output(w).is_some_and(|o| self.auto_on(&o, self.showing(&o))) {
            // A floating window tiled by hand is a tile from now on.
            self.set_floating(w, Some(false));
            self.auto_tick();
        }
        let Some((key, tree)) = self.auto_take(w, "Tile") else { return false };
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

    /// Auto-tiling's layout in tree `key` (its display's workspace's own,
    /// else the default).
    fn layout_of(&self, key: &(u32, String)) -> olo_protocol::TilingLayout {
        let tiling = &self.settings.tiling;
        match self.space.outputs().find(|o| o.name() == key.1) {
            Some(o) => tiling.layout_of(key.0, &crate::desktop::display_key(o)),
            None => tiling.layout,
        }
    }

    /// Where Main and Stack's main window goes in tree `key` now (its
    /// display's workspace's place, else the default), and where it went
    /// as the tree was last built (the same, unless it was just chosen
    /// or the tree wasn't built in Main and Stack).
    fn main_places(&self, key: &(u32, String)) -> (Tile, Option<Tile>) {
        let tiling = &self.settings.tiling;
        let place = match self.space.outputs().find(|o| o.name() == key.1) {
            Some(o) => tiling.main_place_of(key.0, &crate::desktop::display_key(o)),
            None => tiling.main_place,
        };
        // (Nothing built yet, after a restart: the tree kept is Main and
        // Stack's as it was. Built as another layout: none to read.)
        let was = match self.auto.built_as.get(key) {
            Some((olo_protocol::TilingLayout::MainAndStack, p, _)) => Some(*p),
            Some(_) => None,
            None => Some(place),
        };
        (place, was)
    }

    /// Main and Stack's main window of `w`'s tiles, if `w` is in tiles
    /// laid out so.
    pub fn main_window_of(&self, key: &(u32, String)) -> Option<u64> {
        if self.layout_of(key) != olo_protocol::TilingLayout::MainAndStack {
            return None;
        }
        let tree = self.auto.trees.get(key)?;
        self.auto.mains.get(key)?.now.filter(|id| tree.contains(*id))
    }

    /// The main window before tree `key`'s (Main and Stack), still in it.
    pub fn previous_main_of(&self, key: &(u32, String)) -> Option<u64> {
        self.main_window_of(key)?;
        let tree = self.auto.trees.get(key)?;
        self.auto.mains.get(key)?.before.filter(|id| tree.contains(*id))
    }

    /// Whether `w` is its tiles' main window (Main and Stack).
    pub fn is_main(&self, w: &WindowElement) -> bool {
        self.tree_of(w).and_then(|k| self.main_window_of(&k)) == Some(w.id())
    }

    /// Every main window (Main and Stack), of every tree: for the IPC's
    /// window list, at once.
    pub fn main_windows(&self) -> Vec<u64> {
        self.auto.trees.keys().filter_map(|k| self.main_window_of(k)).collect()
    }

    /// Main and Stack's tree of `w` ready for a command on its main
    /// window (`name`, for Undo Window Arrangement): its key, the tree
    /// and the main window's place in it. None where `w` isn't a tile of
    /// Main and Stack's.
    fn main_take(&mut self, w: &WindowElement, name: &str) -> Option<((u32, String), Node, Tile)> {
        if self.floats(w) {
            return None;
        }
        let key = self.tree_of(w)?;
        if self.layout_of(&key) != olo_protocol::TilingLayout::MainAndStack {
            return None;
        }
        let place = self.main_places(&key).0;
        let (key, tree) = self.auto_take(w, name)?;
        Some((key, tree, place))
    }

    /// Make Main (⌃⌥⌘M, the window menu, `Request::MakeMain`): `w` is
    /// its tiles' main window, and the one that was takes its place in
    /// the stack. False where its tiles aren't Main and Stack's.
    pub fn make_main(&mut self, w: &WindowElement) -> bool {
        if self.is_main(w) {
            return self.tree_of(w).is_some();
        }
        let Some((key, tree, place)) = self.main_take(w, "Make Main") else { return false };
        let m = self.auto.mains.get(&key).copied().unwrap_or_default();
        let built = main_and_stack_again(&tree, place, Some(place), self.main_count_of(&key) as usize, None, Some(w.id()), m.now, m.before).unwrap_or(tree);
        info!(window = w.id(), "made the main window");
        self.auto_put(key, built);
        true
    }

    /// Swap with Previous Main (⌃⌥⌘⌫, `Request::SwapMain`): the main
    /// window before the one that is now (on `output`'s workspace
    /// showing, else the active display's) is main again, and the one
    /// that is goes back to the stack where it was. False if there was
    /// none there.
    pub fn swap_main(&mut self, output: Option<Output>) -> bool {
        let Some(output) = output.or_else(|| self.active_output()) else { return false };
        let key = (self.showing(&output), output.name());
        let Some(before) = self.auto.mains.get(&key).and_then(|m| m.before) else { return false };
        let Some(w) = self.window_by_id(before).filter(|_| self.auto.trees.get(&key).is_some_and(|t| t.contains(before))) else { return false };
        self.make_main(&w)
    }

    /// Choose where Main and Stack's main window goes on `output` (the
    /// active display unless given) on its workspace `workspace` (the one
    /// showing unless given): its tiles are built again so.
    /// Focus by direction (⌃⌥⌘ H J K L, `directional`): the window beside
    /// `from` (the focused one; else the active display's middle) that
    /// way, among those on screen on every display, takes the keyboard
    /// and comes to the front. Whether one did.
    pub fn focus_toward(&mut self, from: Option<WindowElement>, dir: olo_protocol::Dir) -> bool {
        let frame = |s: &Self, w: &WindowElement| s.space.element_geometry(w).map(|g| (g.loc.x as f32, g.loc.y as f32, g.size.w as f32, g.size.h as f32));
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let windows: Vec<WindowElement> = outputs.iter().flat_map(|o| self.windows_on(o)).collect();
        let others: Vec<(u64, super::tiling::Part)> = windows.iter().filter_map(|w| Some((w.id(), frame(self, w)?))).collect();
        let start = match from.as_ref().and_then(|w| frame(self, w)) {
            Some(f) => f,
            None => {
                let Some(g) = self.active_output().and_then(|o| self.space.output_geometry(&o)) else { return false };
                ((g.loc.x + g.size.w / 2) as f32, (g.loc.y + g.size.h / 2) as f32, 0.0, 0.0)
            }
        };
        let Some(id) = super::directional::nearest(&others, start, dir) else { return false };
        let Some(w) = windows.into_iter().find(|w| w.id() == id) else { return false };
        info!(window = id, dir = dir.id(), "focused by direction");
        self.focus_window(&w);
        true
    }

    /// Swap by direction (⌃⌥⇧⌘ H J K L): auto-tiled `w` trades tiles with
    /// the one beside it that way on its display. Whether it did.
    pub fn swap_toward(&mut self, w: &WindowElement, dir: olo_protocol::Dir) -> bool {
        if self.floats(w) {
            return false;
        }
        let Some(key) = self.tree_of(w) else { return false };
        let Some(tree) = self.auto.trees.get(&key) else { return false };
        let parts: Vec<(u64, super::tiling::Part)> =
            layout(tree, &|_| (0.0, 0.0), Inset::default()).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect();
        let Some(from) = parts.iter().find(|(i, _)| *i == w.id()).map(|(_, p)| *p) else { return false };
        let Some(other) = super::directional::nearest(&parts, from, dir) else { return false };
        let Some((key, mut tree)) = self.auto_take(w, "Swap Tiles") else { return false };
        tree.swap(w.id(), other);
        info!(window = w.id(), with = other, dir = dir.id(), "tiles swapped by direction");
        self.auto_put(key, tree);
        true
    }

    /// Resize by direction (⌃⌥⇧⌘ arrows): the edge beside auto-tiled `w`
    /// that way moves that way a step (`RESIZE_STEP` of the display, at
    /// least `RESIZE_STEP_PX`), its tree kept as laid out so least sizes
    /// hold it. Whether an edge moved.
    pub fn resize_toward(&mut self, w: &WindowElement, dir: olo_protocol::Dir) -> bool {
        if self.floats(w) {
            return false;
        }
        let Some(key) = self.tree_of(w) else { return false };
        let Some(output) = self.space.outputs().find(|o| o.name() == key.1).cloned() else { return false };
        let Some(area) = self.tile_area(&output) else { return false };
        let Some(tree) = self.auto.trees.get(&key) else { return false };
        let along = match dir {
            olo_protocol::Dir::Left | olo_protocol::Dir::Right => area.size.w,
            olo_protocol::Dir::Up | olo_protocol::Dir::Down => area.size.h,
        }
        .max(1) as f32;
        let step = RESIZE_STEP.max(RESIZE_STEP_PX / along);
        let windows = self.all_windows();
        let mins = self.auto_mins(&windows, area);
        let min = |id: u64| mins.get(&id).copied().unwrap_or((0.0, 0.0));
        let inset = Inset::of(area, self.tile_gap());
        let mut moved = tree.clone();
        if !super::directional::resize_split(&mut moved, w.id(), dir, step) {
            return false;
        }
        // As laid out: a ratio least sizes hold off isn't kept beyond them
        // (the next press the other way moves at once), and a press that
        // moves nothing on screen changes nothing (nor what Undo takes
        // back).
        let (now, was) = (as_laid_out(&moved, &min, inset), as_laid_out(tree, &min, inset));
        if now == was {
            return false;
        }
        let Some((key, _)) = self.auto_take(w, "Resize Tile") else { return false };
        info!(window = w.id(), dir = dir.id(), "tile resized by direction");
        self.auto_put(key, now);
        true
    }

    /// Rotate, flip or even out the auto-tiled arrangement on `output`'s
    /// workspace (the active display's; `reshape`): in Main and Stack its
    /// main window's place turned or mirrored, its stack made even and
    /// the main window its place's share again. Whether anything changed.
    pub fn reshape_tiles(&mut self, output: Option<Output>, how: Reshape) -> bool {
        use super::reshape::{even_out, flip, flipped_place, rotate, rotated_place};
        let Some(o) = output.or_else(|| self.active_output()) else { return false };
        let ws = self.showing(&o);
        if !self.auto_on(&o, ws) {
            return false;
        }
        let key = (ws, o.name());
        let Some(tree) = self.auto.trees.get(&key) else { return false };
        let name = match how {
            Reshape::Rotate(_) => "Rotate Tiles",
            Reshape::Flip(_) => "Flip Tiles",
            Reshape::EvenOut => "Even Out Tiles",
        };
        let new = if self.layout_of(&key) == olo_protocol::TilingLayout::MainAndStack {
            let place = self.main_places(&key).0;
            match how {
                Reshape::Rotate(cw) => return rotated_place(place, cw).is_some_and(|p| self.set_main_place(p, Some(ws), Some(o))),
                Reshape::Flip(across) => {
                    let p = flipped_place(place, across);
                    return p != place && self.set_main_place(p, Some(ws), Some(o));
                }
                Reshape::EvenOut => {
                    let m = self.auto.mains.get(&key).copied().unwrap_or_default();
                    let flat = evenly(Cut::Columns, in_order(olo_protocol::TilingLayout::MainAndStack, place, tree));
                    let Some(built) = main_and_stack_again(&flat, place, None, self.main_count_of(&key) as usize, None, None, m.now, m.before) else { return false };
                    built
                }
            }
        } else {
            match how {
                Reshape::Rotate(cw) => rotate(tree, cw),
                Reshape::Flip(across) => flip(tree, across),
                Reshape::EvenOut => even_out(tree),
            }
        };
        if new == *tree {
            return false;
        }
        let Some((key, _)) = self.auto_take_key(key, name) else { return false };
        info!(output = key.1, tree = %new, "{name}");
        self.auto_put(key, new);
        true
    }

    /// Main and Stack's main part a step bigger or smaller on the active
    /// display's workspace, whichever tile has the focus (dwm's and
    /// xmonad's Shrink / Expand, ⌃⌥⌘. and ⌃⌥⌘,), kept as dragged edges
    /// are. Whether it changed.
    pub fn grow_main(&mut self, grow: bool) -> bool {
        let Some(o) = self.active_output() else { return false };
        let ws = self.showing(&o);
        let key = (ws, o.name());
        if !self.auto_on(&o, ws) || self.layout_of(&key) != olo_protocol::TilingLayout::MainAndStack {
            return false;
        }
        let place = self.main_places(&key).0;
        let Some(tree) = self.auto.trees.get(&key) else { return false };
        let Some((mains, stack, at)) = main_and_stack_of(tree, place) else { return false };
        let Some(area) = self.tile_area(&o) else { return false };
        // A step of the display along the way it grows, at least
        // `RESIZE_STEP_PX` points.
        let along = if place.mains_side_by_side() { area.size.h } else { area.size.w }.max(1) as f32;
        let step = RESIZE_STEP.max(RESIZE_STEP_PX / along);
        let built = main_and_stack(mains, stack, Vec::new(), place, main_part_grown(place, at, if grow { step } else { -step }));
        let windows = self.all_windows();
        let mins = self.auto_mins(&windows, area);
        let min = |id: u64| mins.get(&id).copied().unwrap_or((0.0, 0.0));
        let inset = Inset::of(area, self.tile_gap());
        let (now, was) = (as_laid_out(&built, &min, inset), as_laid_out(tree, &min, inset));
        if now == was {
            return false;
        }
        let Some((key, _)) = self.auto_take_key(key, if grow { "Bigger Main" } else { "Smaller Main" }) else { return false };
        info!(output = key.1, grow, tree = %now, "main part");
        self.auto_put(key, now);
        true
    }

    /// Toggle split (⌃⌥⌘S): the split auto-tiled `w` shares with its
    /// neighbour turned the other way. Whether there was one.
    pub fn toggle_split(&mut self, w: &WindowElement) -> bool {
        if self.floats(w) {
            return false;
        }
        let Some(key) = self.tree_of(w) else { return false };
        let Some(mut tree) = self.auto.trees.get(&key).cloned() else { return false };
        if !super::reshape::toggle_split(&mut tree, w.id()) {
            return false;
        }
        let Some((key, _)) = self.auto_take_key(key, "Turn Split") else { return false };
        info!(window = w.id(), tree = %tree, "split turned");
        self.auto_put(key, tree);
        true
    }

    /// Auto-tiling's layout on one display's workspace (the active display
    /// and the workspace showing, unless given), remembered for it.
    pub fn set_tiling_layout(&mut self, layout: olo_protocol::TilingLayout, workspace: Option<u32>, output: Option<Output>) -> bool {
        let Some(output) = output.or_else(|| self.active_output()) else { return false };
        let ws = workspace.unwrap_or_else(|| self.showing(&output)).min(self.settings.workspace_count.max(1) - 1);
        let key = crate::desktop::display_key(&output);
        if self.settings.tiling.layout_of(ws, &key) == layout {
            return true;
        }
        self.settings.tiling.set_layout(ws, &key, layout);
        self.settings_changed();
        info!(workspace = ws, output = output.name(), layout = layout.id(), "tiling layout");
        self.auto.changed();
        self.auto_tick();
        true
    }

    /// How many main windows Main and Stack has on the tree `key` names.
    pub fn main_count_of(&self, key: &(u32, String)) -> u32 {
        let display = self.space.outputs().find(|o| o.name() == key.1).map(crate::desktop::display_key).unwrap_or_default();
        self.settings.tiling.main_count_of(key.0, &display)
    }

    /// How many main windows Main and Stack has on `output`'s workspace
    /// `workspace` (the active display's, the one showing): remembered
    /// for it and laid out so. Whether it changed.
    pub fn set_main_count(&mut self, count: u32, workspace: Option<u32>, output: Option<Output>) -> bool {
        let Some(output) = output.or_else(|| self.active_output()) else { return false };
        let ws = workspace.unwrap_or_else(|| self.showing(&output)).min(self.settings.workspace_count.max(1) - 1);
        let display = crate::desktop::display_key(&output);
        let count = count.clamp(1, olo_protocol::MAX_MAINS);
        if self.settings.tiling.main_count_of(ws, &display) == count {
            return false;
        }
        self.settings.tiling.set_main_count(ws, &display, count);
        self.settings_changed();
        info!(workspace = ws, output = output.name(), count, "main windows");
        self.auto.changed();
        self.auto_tick();
        true
    }

    pub fn set_main_place(&mut self, place: Tile, workspace: Option<u32>, output: Option<Output>) -> bool {
        if place == Tile::Cell {
            return false;
        }
        let Some(output) = output.or_else(|| self.active_output()) else { return false };
        let ws = workspace.unwrap_or_else(|| self.showing(&output)).min(self.settings.workspace_count.max(1) - 1);
        self.settings.tiling.set_main_place(ws, &crate::desktop::display_key(&output), place);
        self.settings_changed();
        info!(workspace = ws, output = output.name(), ?place, "the main window's place");
        self.auto.changed();
        self.auto_tick();
        true
    }

    /// Zoom Vertically or Horizontally for a tile: a full column (or row)
    /// beside the rest; again, the tiles as they were (or, if they have
    /// changed since, back among them).
    pub fn auto_zoom_along(&mut self, w: &WindowElement, axis: Axis) -> bool {
        let Some((key, tree)) = self.auto_take(w, "Zoom") else { return false };
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
                    self.auto_insert(&mut t, w, area, &key);
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
        self.arrangement_step("Balance Tiles", super::tiling::ArrangedKind::Auto);
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
            // (The arranger's step, its name kept: auto-tiling's to undo.)
            if let Some(a) = &mut self.arrangement {
                a.kind = super::tiling::ArrangedKind::Auto;
            }
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
                self.settings_changed();
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
        let target = layout(&self.auto.trees[&to], &min, Inset::of(area, self.tile_gap())).into_iter().find_map(|(n, r)| match n {
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
        self.arrangement_step("Move Tile", super::tiling::ArrangedKind::Auto);
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

    /// `w` and `other` side by side (the window menu's Tile With), the
    /// one further left now on the left: auto-tiled on one display's tree,
    /// `other` moves into a tile beside `w`'s (as a tile dropped on its
    /// edge); else they take the halves, as Tile Left and Tile Right.
    pub fn tile_with(&mut self, w: &WindowElement, other: &WindowElement) {
        let x = |s: &Self, e: &WindowElement| s.space.element_geometry(e).map_or(0, |g| g.loc.x);
        let w_left = x(self, w) <= x(self, other);
        if let (Some(a), Some(b)) = (self.tree_of(w), self.tree_of(other))
            && a == b
            && let Some(before) = self.auto.trees.remove(&a)
        {
            self.auto.undo = Some(Undo::Tree(a.clone(), Some(before.clone())));
            self.arrangement_step("Tile With", super::tiling::ArrangedKind::Auto);
            let mut t = before.without(&|n| *n == Node::Window(other.id())).unwrap_or(Node::Window(w.id()));
            t.split_leaf(w.id(), Node::Window(other.id()), Cut::Columns, !w_left);
            info!(window = w.id(), with = other.id(), "tiled with");
            self.auto.trees.insert(a, t);
            self.auto.dirty = true;
            self.auto_tick();
            return;
        }
        let (left, right) = if w_left { (w.clone(), other.clone()) } else { (other.clone(), w.clone()) };
        self.run_shortcut(olo_protocol::Shortcut::Tile(Tile::Left), Some(left));
        self.run_shortcut(olo_protocol::Shortcut::Tile(Tile::Right), Some(right));
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

    /// Whether `w`'s tree, its splits moved to where a drag has the
    /// windows it moves (`frames`; the other tiles' parts as they are),
    /// lays those out just there: what the drag shows is what stays as
    /// it's let go (`auto_drag_ended`), no least size (`mins`, its
    /// `auto_mins`) holding an edge off where it went.
    pub(super) fn auto_keeps(&self, w: &WindowElement, frames: &[(WindowElement, Rectangle<i32, Logical>)], mins: &HashMap<u64, (f32, f32)>) -> bool {
        let (Some(tree), Some(area)) = (self.tree_of(w).and_then(|k| self.auto.trees.get(&k)), self.window_output(w).and_then(|o| self.tile_area(&o))) else { return true };
        let moved: Vec<(u64, Rectangle<i32, Logical>)> = frames.iter().map(|(f, r)| (f.id(), *r)).collect();
        let others: HashMap<u64, Part> = tree.windows().into_iter().filter_map(|i| Some((i, Self::custom_part(&self.window_by_id(i)?)?))).collect();
        lays_out_as(tree, &moved, &|i| others.get(&i).copied(), &|i| mins.get(&i).copied().unwrap_or((0.0, 0.0)), area, self.tile_gap())
    }

    /// An edge drag of a tile ended (its neighbours have their parts):
    /// the splits moved to where they now are.
    pub fn auto_drag_ended(&mut self, w: &WindowElement) {
        let Some(key) = self.tree_of(w) else { return };
        let Some(mut tree) = self.auto.trees.remove(&key) else { return };
        self.auto.undo = Some(Undo::Tree(key.clone(), Some(tree.clone())));
        self.arrangement_step("Resize Tiles", super::tiling::ArrangedKind::Auto);
        let parts: HashMap<u64, Part> = tree.windows().into_iter().filter_map(|i| Some((i, Self::custom_part(&self.window_by_id(i)?)?))).collect();
        // Where it can be: not past a window's least size (else the split
        // would be one place and the windows another, until they are
        // laid out again; the drag stopped there, `auto_keeps`).
        if let Some(area) = self.window_output(w).and_then(|o| self.tile_area(&o)) {
            take_ratios(&mut tree, &|i| parts.get(&i).copied(), still_in(area));
            let mins = self.auto_mins(&self.all_windows(), area);
            tree = as_laid_out(&tree, &|i| mins.get(&i).copied().unwrap_or((0.0, 0.0)), Inset::of(area, self.tile_gap()));
        }
        self.auto_put(key, tree);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiles side by side share an edge exactly, however deep each is in
    /// the tree: one's start plus its length is the next one's start.
    #[test]
    fn neighbours_share_their_edges_exactly() {
        for k in 1..400 {
            let r0 = 0.2 + k as f32 * 0.0015;
            let r1 = 0.7 - k as f32 * 0.001;
            let (left, right) = divide(Cut::Columns, r0, (0.0, 0.0, 1.0, 1.0));
            let (a, b) = divide(Cut::Columns, r1, left);
            assert_eq!(a.0 + a.2, b.0, "{k}");
            assert_eq!(b.0 + b.2, right.0, "{k}: a nested split's far edge is its neighbour's start");
            assert_eq!(right.0 + right.2, 1.0);
            let (c, d) = divide(Cut::Rows, r1, right);
            assert_eq!(c.1 + c.3, d.1);
            assert_eq!(d.1 + d.3, 1.0);
            // In pixels, the gap the same everywhere.
            let area = Rectangle::<i32, Logical>::new((0, 0).into(), (2992, 1064).into());
            let (pb, pr) = (crate::shell::tiling::part(area, b, 6), crate::shell::tiling::part(area, right, 6));
            assert_eq!(pr.loc.x - (pb.loc.x + pb.size.w), 6, "{k}");
        }
    }

    /// Each window's part of the area as `t` lays out (no gaps, no least
    /// sizes).
    fn laid_parts(t: &Node) -> Vec<(Node, Part)> {
        layout(t, &|_| (0.0, 0.0), Inset::default()).into_iter().map(|(n, p)| (n.clone(), p)).collect()
    }

    fn part_of(t: &Node, id: u64) -> Part {
        laid_parts(t).into_iter().find(|(n, _)| *n == Node::Window(id)).map(|(_, p)| p).unwrap_or_else(|| panic!("{id} not in {t}"))
    }

    /// Main and Stack, its main window in each kind of place: beside an
    /// edge with the stack in a column (a row) beside it, in the middle
    /// with a column either side (the right one first, with the odd one),
    /// in a corner with the rest of its column taking one; alone, in its
    /// place, with room left for the next window. Read back as it was
    /// built: the main window, the stack in order and its part.
    #[test]
    fn built_layouts_shapes() {
        use olo_protocol::TilingLayout as L;
        let tree = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Rows, 0.3, w(2), Node::split(Cut::Columns, 0.5, w(3), w(4))));
        let parts_of = |t: &Node| -> Vec<(u64, Part)> { parts(t) };
        let near_all = |got: Vec<(u64, Part)>, want: &[(u64, Part)]| {
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (id, p) in want {
                let g = got.iter().find(|(i, _)| i == id).unwrap_or_else(|| panic!("{id} missing: {got:?}")).1;
                assert!(near(g, *p), "{id}: {g:?} not {p:?}");
            }
        };
        // Columns, Rows: each as big, in the tree's order.
        let c = built_again(L::Columns, &tree, 1.6).unwrap();
        near_all(parts_of(&c), &[(1, (0.0, 0.0, 0.25, 1.0)), (2, (0.25, 0.0, 0.25, 1.0)), (3, (0.5, 0.0, 0.25, 1.0)), (4, (0.75, 0.0, 0.25, 1.0))]);
        let r = built_again(L::Rows, &tree, 1.6).unwrap();
        near_all(parts_of(&r), &[(1, (0.0, 0.0, 1.0, 0.25)), (4, (0.0, 0.75, 1.0, 0.25))].iter().copied().chain([(2, (0.0, 0.25, 1.0, 0.25)), (3, (0.0, 0.5, 1.0, 0.25))]).collect::<Vec<_>>());
        // Grid: four on a square area, two by two.
        let g = built_again(L::Grid, &tree, 1.0).unwrap();
        near_all(parts_of(&g), &[(1, (0.0, 0.0, 0.5, 0.5)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.0, 0.5, 0.5, 0.5)), (4, (0.5, 0.5, 0.5, 0.5))]);
        let g5 = built_again(L::Grid, &Node::split(Cut::Columns, 0.5, tree.clone(), w(5)), 1.0).unwrap();
        assert_eq!(parts_of(&g5).len(), 5, "five in a square-ish grid");
        // Four on a 16:9 display: two by two still (xmonad's).
        let g169 = built_again(L::Grid, &tree, 16.0 / 9.0).unwrap();
        near_all(parts_of(&g169), &[(1, (0.0, 0.0, 0.5, 0.5)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.0, 0.5, 0.5, 0.5)), (4, (0.5, 0.5, 0.5, 0.5))]);
        // Dwindle (Halves chosen): each next one half of the rest across
        // its longer side, on a wide area: left half, then top of the
        // right, then the left of the bottom right.
        let d = dwindle(&tree, 16.0 / 9.0).unwrap();
        near_all(parts_of(&d), &[(1, (0.0, 0.0, 0.5, 1.0)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.5, 0.5, 0.25, 0.5)), (4, (0.75, 0.5, 0.25, 0.5))]);
        // Spiral: left half, then the top of the rest, then its right,
        // then the bottom.
        let sp = built_again(L::Spiral, &evenly(Cut::Columns, (1..=4).map(w).collect()), 1.6).unwrap();
        near_all(parts_of(&sp), &[(1, (0.0, 0.0, 0.5, 1.0)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.75, 0.5, 0.25, 0.5)), (4, (0.5, 0.5, 0.25, 0.5))]);
        // A place kept with no room goes on, taking none; Halves and
        // Main and Stack aren't built here.
        let kept = Node::Kept(Kept { who: Who::Window(9), holds: false, until: None });
        let with_kept = built_again(L::Columns, &Node::split(Cut::Rows, 0.5, tree.clone(), kept), 1.6).unwrap();
        assert!(with_kept.any_leaf(&|n| matches!(n, Node::Kept(k) if k.who == Who::Window(9))));
        assert_eq!(parts_of(&with_kept).len(), 4);
        assert!(built_again(L::Halves, &tree, 1.6).is_none() && built_again(L::MainAndStack, &tree, 1.6).is_none());
        // The order as built, whatever the tree's own: Spiral's (2 is
        // after 4 in its tree: on the right of it), with five and six too.
        let ids = |v: Vec<Node>| -> Vec<u64> { v.into_iter().filter_map(|n| if let Node::Window(i) = n { Some(i) } else { None }).collect() };
        assert_eq!(ids(in_order(L::Spiral, Tile::Left, &sp)), [1, 2, 3, 4]);
        let six = (1..=6).map(w).collect::<Vec<_>>();
        let sp6 = spiral(six.clone()).unwrap();
        assert_ne!(ids(leaves(&sp6)), [1, 2, 3, 4, 5, 6], "its tree's own order isn't");
        assert_eq!(ids(in_order(L::Spiral, Tile::Left, &sp6)), [1, 2, 3, 4, 5, 6]);
        assert_eq!(ids(in_order(L::Spiral, Tile::Left, &with_kept_spiral(&sp6))), [1, 2, 3, 4, 5, 6]);
        // Main and Stack's main window on the right: it first.
        let right = main_and_stack(vec![w(3)], vec![w(1), w(2)], vec![], Tile::Right, Tile::Right.fraction());
        assert_eq!(ids(in_order(L::MainAndStack, Tile::Right, &right)), [3, 1, 2]);
        // Not shaped as its layout (refitted, rows first): its tree's order.
        let other = Node::split(Cut::Rows, 0.5, w(1), Node::split(Cut::Columns, 0.5, w(2), w(3)));
        assert_eq!(ids(in_order(L::Spiral, Tile::Left, &other)), [1, 2, 3]);
        // Round every layout and back: the same order.
        let mut t = c.clone();
        let mut prev = L::Columns;
        for l in [L::Spiral, L::Grid, L::Spiral, L::Rows, L::MainAndStack, L::Spiral, L::Halves, L::Columns] {
            let flat = evenly(Cut::Columns, in_order(prev, Tile::Left, &t));
            t = match l {
                L::MainAndStack => main_and_stack_again(&flat, Tile::Left, None, 1, None, None, None, None).unwrap(),
                L::Halves => dwindle(&flat, 1.6).unwrap(),
                _ => built_again(l, &flat, 1.6).unwrap(),
            };
            prev = l;
            assert_eq!(ids(in_order(l, Tile::Left, &t)), [1, 2, 3, 4], "after {l:?}");
        }
        // Two halves aren't read as Main and Stack's (not built as it):
        // the main window gets its place's share.
        let halves = Node::split(Cut::Columns, 0.5, w(1), w(2));
        let m = main_and_stack_again(&halves, Tile::LeftTwoThirds, None, 1, None, None, None, None).unwrap();
        assert!(near(parts_of(&m)[0].1, (0.0, 0.0, Tile::LeftTwoThirds.fraction().2, 1.0)), "{:?}", parts_of(&m));
    }

    /// Built layouts keep their own order as windows come and go: a
    /// window closing in Spiral (its place kept, taking no room) leaves the
    /// others where they were in its order, and a new one goes into the
    /// order where it's put, not a split of a tile.
    #[test]
    fn built_layouts_keep_their_order() {
        use olo_protocol::TilingLayout as L;
        let ids = |v: Vec<Node>| -> Vec<u64> { v.into_iter().filter_map(|n| if let Node::Window(i) = n { Some(i) } else { None }).collect() };
        let sp = spiral((1..=5).map(w).collect()).unwrap();
        let mut closed = sp.clone();
        closed.replace(&|n| *n == w(2), Node::Kept(Kept { who: Who::Window(2), holds: false, until: None }));
        let again = built_again(L::Spiral, &closed, 1.6).unwrap();
        assert_eq!(ids(in_order(L::Spiral, Tile::Left, &again)), [1, 3, 4, 5], "{again}");
        // (Before: its tree's own order, 1 3 5 4, swapped two of them.)
        for (at, want) in [(NewAt::First, vec![9, 1, 2, 3, 4, 5]), (NewAt::After(3), vec![1, 2, 3, 9, 4, 5]), (NewAt::Last, vec![1, 2, 3, 4, 5, 9]), (NewAt::After(77), vec![1, 2, 3, 4, 5, 9])] {
            let mut order = in_order(L::Spiral, Tile::Left, &sp);
            put_new(&mut order, 9, at);
            let built = built_again(L::Spiral, &evenly(Cut::Columns, order), 1.6).unwrap();
            assert_eq!(ids(in_order(L::Spiral, Tile::Left, &built)), want, "{at:?}");
        }
        // Main and Stack: after a stack window, or last; the main window
        // stays.
        let place = Tile::LeftTwoThirds;
        let t = main_and_stack(vec![w(1)], vec![w(2), w(3)], Vec::new(), place, place.fraction());
        let order = |t: &Node| {
            let (m, s, _) = main_and_stack_of(t, place).unwrap();
            (ids(m), ids(s))
        };
        let after = main_and_stack_again(&t, place, Some(place), 1, Some((9, NewAt::After(2))), None, Some(1), None).unwrap();
        assert_eq!(order(&after), (vec![1], vec![2, 9, 3]));
        let last = main_and_stack_again(&t, place, Some(place), 1, Some((9, NewAt::Last)), None, Some(1), None).unwrap();
        assert_eq!(order(&last), (vec![1], vec![2, 3, 9]));
        let after_main = main_and_stack_again(&t, place, Some(place), 1, Some((9, NewAt::After(1))), None, Some(1), None).unwrap();
        assert_eq!(order(&after_main), (vec![1], vec![9, 2, 3]));
    }

    #[test]
    fn parts_wrong_says_what() {
        assert_eq!(parts_wrong(&[(1, (0.0, 0.0, 0.5, 1.0)), (2, (0.5, 0.0, 0.5, 1.0))]), None);
        assert!(parts_wrong(&[(1, (0.0, 0.0, 0.6, 1.0)), (2, (0.5, 0.0, 0.5, 1.0))]).is_some_and(|w| w.contains("overlaps")));
        assert!(parts_wrong(&[(1, (0.6, 0.0, 0.5, 1.0))]).is_some_and(|w| w.contains("outside")));
        assert!(parts_wrong(&[(1, (f32::NAN, 0.0, 0.5, 1.0))]).is_some());
    }

    fn with_kept_spiral(t: &Node) -> Node {
        Node::split(Cut::Columns, 1.0, t.clone(), Node::Kept(Kept { who: Who::Window(9), holds: false, until: None }))
    }

    /// Random tiling, many times over: windows come and go, tiles swap
    /// and resize by direction, least sizes change, layouts and places
    /// are chosen in any order, refits happen. Whatever the sequence the
    /// tree holds every window once, its tiles cover the area exactly
    /// without overlapping, nothing is NaN, and a layout chosen keeps the
    /// windows' order (Ben, 2026-11-15: "make sure the tiling logic is
    /// rock solid / bulletproof").
    #[test]
    fn tiling_survives_anything() {
        use olo_protocol::{Dir, TilingLayout as L};
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        let check = |t: &Node, want: &std::collections::BTreeSet<u64>, what: &str| {
            let mut ids = t.windows();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            assert_eq!(n, ids.len(), "{what}: a window twice in {t}");
            assert_eq!(ids.into_iter().collect::<std::collections::BTreeSet<_>>(), *want, "{what}: {t}");
            let parts = layout(t, &|_| (0.0, 0.0), Inset::default());
            let mut area = 0.0;
            for (i, (_, p)) in parts.iter().enumerate() {
                assert!(p.0.is_finite() && p.1.is_finite() && p.2.is_finite() && p.3.is_finite(), "{what}: {p:?} in {t}");
                assert!(p.0 >= -1e-4 && p.1 >= -1e-4 && p.0 + p.2 <= 1.0 + 1e-4 && p.1 + p.3 <= 1.0 + 1e-4, "{what}: {p:?} outside, in {t}");
                area += p.2 * p.3;
                for (_, q) in &parts[i + 1..] {
                    let ox = (p.0 + p.2).min(q.0 + q.2) - p.0.max(q.0);
                    let oy = (p.1 + p.3).min(q.1 + q.3) - p.1.max(q.1);
                    assert!(ox <= 1e-4 || oy <= 1e-4, "{what}: {p:?} overlaps {q:?} in {t}");
                }
            }
            if !want.is_empty() {
                assert!((area - 1.0).abs() < 1e-3, "{what}: tiles cover {area} of the area in {t}");
            }
        };
        let layouts = L::ALL;
        let places = Tile::CHOOSABLE;
        for round in 0..400 {
            let mut tree: Option<Node> = None;
            let mut want = std::collections::BTreeSet::new();
            let mut next = 1u64;
            let (mut lay, mut place) = (L::Halves, Tile::Left);
            let mut mins: HashMap<u64, (f32, f32)> = HashMap::new();
            for step in 0..60 {
                let what = format!("round {round} step {step}");
                match rnd(10) {
                    // A window comes, splitting one (or the first).
                    0 | 1 => {
                        let id = next;
                        next += 1;
                        tree = Some(match tree.take() {
                            None => w(id),
                            // (A hole first, as app_server fills one: Main and
                            // Stack's room for the next window.)
                            Some(mut t) if t.any_leaf(&|n| *n == Node::hole()) => {
                                t.replace(&|n| *n == Node::hole(), w(id));
                                t
                            }
                            Some(mut t) => {
                                let ids = t.windows();
                                let at = ids[rnd(ids.len())];
                                let cut = if rnd(2) == 0 { Cut::Columns } else { Cut::Rows };
                                assert!(t.split_leaf(at, w(id), cut, rnd(2) == 0), "{what}");
                                t
                            }
                        });
                        want.insert(id);
                        if rnd(3) == 0 {
                            mins.insert(id, (rnd(40) as f32 / 100.0, rnd(40) as f32 / 100.0));
                        }
                    }
                    // One goes.
                    2 if !want.is_empty() => {
                        let ids: Vec<u64> = want.iter().copied().collect();
                        let id = ids[rnd(ids.len())];
                        want.remove(&id);
                        tree = tree.take().and_then(|t| t.without(&|n| *n == w(id)));
                    }
                    // Swap, resize by direction.
                    3 if want.len() > 1 => {
                        let ids: Vec<u64> = want.iter().copied().collect();
                        let (x, y) = (ids[rnd(ids.len())], ids[rnd(ids.len())]);
                        if let Some(t) = tree.as_mut() {
                            t.swap(x, y);
                        }
                    }
                    4 if !want.is_empty() => {
                        let ids: Vec<u64> = want.iter().copied().collect();
                        if let Some(t) = tree.as_mut() {
                            super::super::directional::resize_split(t, ids[rnd(ids.len())], Dir::ALL[rnd(4)], rnd(30) as f32 / 100.0);
                        }
                    }
                    // Another layout or place chosen: in the order the
                    // last had them.
                    5 | 6 => {
                        let (l, p) = (layouts[rnd(layouts.len())], places[rnd(places.len())]);
                        if let Some(t) = tree.as_mut() {
                            let order: Vec<u64> = in_order(lay, place, t).into_iter().filter_map(|n| if let Node::Window(i) = n { Some(i) } else { None }).collect();
                            let flat = evenly(Cut::Columns, in_order(lay, place, t));
                            let built = match l {
                                L::MainAndStack => main_and_stack_again(&flat, p, None, 1 + rnd(4), None, None, None, None),
                                L::Halves => dwindle(&flat, 1.6),
                                _ => built_again(l, &flat, 1.6),
                            };
                            if let Some(b) = built {
                                *t = b;
                            }
                            check(t, &want, &format!("{what}: {l:?} {p:?}"));
                            let now: Vec<u64> = in_order(l, p, t).into_iter().filter_map(|n| if let Node::Window(i) = n { Some(i) } else { None }).collect();
                            assert_eq!(now, order, "{what}: {lay:?} → {l:?} reordered, {t}");
                        }
                        (lay, place) = (l, p);
                    }
                    // Rotated, flipped, evened out, a split turned.
                    8 => {
                        if let Some(t) = tree.as_mut() {
                            use super::super::reshape as r;
                            *t = match rnd(4) {
                                0 => r::rotate(t, rnd(2) == 0),
                                1 => r::flip(t, rnd(2) == 0),
                                2 => r::even_out(t),
                                _ => {
                                    let ids = t.windows();
                                    if !ids.is_empty() {
                                        r::toggle_split(t, ids[rnd(ids.len())]);
                                    }
                                    t.clone()
                                }
                            };
                        }
                    }
                    // Kept as laid out with least sizes; refitted to them.
                    7 => {
                        if let Some(t) = tree.as_mut() {
                            let min = |id: u64| mins.get(&id).copied().unwrap_or((0.0, 0.0));
                            *t = as_laid_out(t, &min, Inset::default());
                        }
                    }
                    _ => {
                        if let Some(t) = tree.as_mut() {
                            let min = |id: u64| mins.get(&id).copied().unwrap_or((0.0, 0.0));
                            if let Some(better) = super::super::autofit::refit(t, &min, 1.6) {
                                *t = better;
                            }
                        }
                    }
                }
                match &tree {
                    Some(t) => check(t, &want, &what),
                    None => assert!(want.is_empty(), "{what}: the tree went with windows left"),
                }
            }
        }
    }

    #[test]
    fn main_and_stack_shapes() {
        let ids = |v: Vec<Node>| v.into_iter().map(|n| if let Node::Window(i) = n { i } else { 0 }).collect::<Vec<u64>>();
        let cases: [(Tile, usize, Vec<(u64, Part)>); 6] = [
            (Tile::Left, 2, vec![(1, (0.0, 0.0, 0.5, 1.0)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.5, 0.5, 0.5, 0.5))]),
            (Tile::RightTwoThirds, 1, vec![(1, (1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0)), (2, (0.0, 0.0, 1.0 / 3.0, 1.0))]),
            (Tile::TopTwoThirds, 2, vec![(1, (0.0, 0.0, 1.0, 2.0 / 3.0)), (2, (0.0, 2.0 / 3.0, 0.5, 1.0 / 3.0)), (3, (0.5, 2.0 / 3.0, 0.5, 1.0 / 3.0))]),
            (Tile::CenterTwoThirds, 3, vec![(1, (1.0 / 6.0, 0.0, 2.0 / 3.0, 1.0)), (2, (5.0 / 6.0, 0.0, 1.0 / 6.0, 0.5)), (3, (5.0 / 6.0, 0.5, 1.0 / 6.0, 0.5)), (4, (0.0, 0.0, 1.0 / 6.0, 1.0))]),
            (Tile::BottomLeft, 3, vec![(1, (0.0, 0.5, 0.5, 0.5)), (2, (0.5, 0.0, 0.5, 0.5)), (3, (0.5, 0.5, 0.5, 0.5)), (4, (0.0, 0.0, 0.5, 0.5))]),
            (Tile::CenterThird, 0, vec![(1, (1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0))]),
        ];
        for (place, n, want) in cases {
            let stack: Vec<Node> = (2..2 + n as u64).map(w).collect();
            let t = main_and_stack(vec![w(1)], stack.clone(), Vec::new(), place, place.fraction());
            for (id, p) in want {
                assert!(near(part_of(&t, id), p), "{place:?}: {id} at {:?}, not {p:?}, in {t}", part_of(&t, id));
            }
            let (main, back, at) = main_and_stack_of(&t, place).unwrap_or_else(|| panic!("{place:?}: {t} isn't its shape"));
            assert_eq!((main, ids(back)), (vec![w(1)], ids(stack)), "{place:?}");
            assert!(near(at, place.fraction()), "{place:?}: {at:?}");
            // Alone in the middle, room either side for the next windows.
            if n == 0 {
                assert_eq!(laid_parts(&t).iter().filter(|(n, _)| *n == Node::hole()).count(), 2, "{t}");
            }
        }
        // Not its shape: another place's, or the person's own.
        let t = main_and_stack(vec![w(1)], vec![w(2), w(3)], Vec::new(), Tile::Left, Tile::Left.fraction());
        assert!(main_and_stack_of(&t, Tile::Top).is_none() && main_and_stack_of(&t, Tile::CenterTwoThirds).is_none());
        // (Two above each other on the left is two main windows' shape.)
        let two = Node::split(Cut::Columns, 0.5, Node::split(Cut::Rows, 0.5, w(1), w(2)), w(3));
        assert_eq!(main_and_stack_of(&two, Tile::Left).map(|(m, s, _)| (m, s)), Some((vec![w(1), w(2)], vec![w(3)])));
        let own = Node::split(Cut::Columns, 0.5, Node::split(Cut::Rows, 0.5, w(1), Node::split(Cut::Columns, 0.5, w(2), w(4))), w(3));
        assert!(main_and_stack_of(&own, Tile::Left).is_none());
    }

    /// Bigger and smaller main parts, from each kind of place: the edge
    /// into the rest moves, a middle one stays in the middle, a tenth to
    /// nine tenths.
    #[test]
    fn main_part_grows_from_its_side() {
        let t = 1.0 / 3.0;
        assert!(near(main_part_grown(Tile::LeftTwoThirds, Tile::LeftTwoThirds.fraction(), 0.05), (0.0, 0.0, 2.0 * t + 0.05, 1.0)));
        assert!(near(main_part_grown(Tile::Right, Tile::Right.fraction(), -0.1), (0.6, 0.0, 0.4, 1.0)));
        assert!(near(main_part_grown(Tile::Top, Tile::Top.fraction(), 0.1), (0.0, 0.0, 1.0, 0.6)));
        assert!(near(main_part_grown(Tile::BottomTwoThirds, Tile::BottomTwoThirds.fraction(), 0.1), (0.0, t - 0.1, 1.0, 2.0 * t + 0.1)));
        let c = main_part_grown(Tile::CenterTwoThirds, Tile::CenterTwoThirds.fraction(), 0.1);
        assert!(near(c, (t / 2.0 - 0.05, 0.0, 2.0 * t + 0.1, 1.0)), "{c:?}");
        assert!(near(main_part_grown(Tile::TopRight, Tile::TopRight.fraction(), 0.1), (0.4, 0.0, 0.6, 0.5)));
        assert!(near(main_part_grown(Tile::Left, (0.0, 0.0, 0.88, 1.0), 0.1), (0.0, 0.0, 0.9, 1.0)), "nine tenths at most");
        assert!(near(main_part_grown(Tile::Left, (0.0, 0.0, 0.12, 1.0), -0.1), (0.0, 0.0, 0.1, 1.0)), "a tenth at least");
        // Built at it: the main window there.
        let at = main_part_grown(Tile::LeftTwoThirds, Tile::LeftTwoThirds.fraction(), 0.05);
        let built = main_and_stack(vec![w(1)], vec![w(2)], Vec::new(), Tile::LeftTwoThirds, at);
        assert!(near(part_of(&built, 1), at), "{:?}", part_of(&built, 1));
    }

    /// Several main windows (dwm's nmaster): they share the main place,
    /// one above the other (side by side across the top), as the protocol
    /// says; a number changed takes the next ones from the stack or gives
    /// the last back to it, the order kept; more than there are windows,
    /// all of them.
    #[test]
    fn main_windows_more_than_one() {
        let ids = |v: Vec<Node>| -> Vec<u64> { v.into_iter().filter_map(|n| if let Node::Window(i) = n { Some(i) } else { None }).collect() };
        for place in [Tile::LeftTwoThirds, Tile::Top, Tile::CenterTwoThirds, Tile::BottomRight, Tile::Right] {
            for mains in 1..=3usize {
                for stack in 0..=3usize {
                    let ms: Vec<Node> = (1..=mains as u64).map(w).collect();
                    let st: Vec<Node> = (10..10 + stack as u64).map(w).collect();
                    let t = main_and_stack(ms.clone(), st.clone(), Vec::new(), place, place.fraction());
                    let want = place.main_and_stack_parts_with(mains, stack);
                    let ids_in_order: Vec<u64> = (1..=mains as u64).chain(10..10 + stack as u64).collect();
                    for (id, p) in ids_in_order.iter().zip(want) {
                        assert!(near(part_of(&t, *id), p), "{place:?} {mains}+{stack}: {id} at {:?}, not {p:?}, in {t}", part_of(&t, *id));
                    }
                    let (m, s, _) = main_and_stack_of(&t, place).unwrap_or_else(|| panic!("{place:?} {mains}+{stack}: {t}"));
                    assert_eq!((ids(m), ids(s)), ((1..=mains as u64).collect(), (10..10 + stack as u64).collect()), "{place:?} {mains}+{stack}");
                }
            }
        }
        let place = Tile::LeftTwoThirds;
        let t = main_and_stack(vec![w(1)], vec![w(2), w(3), w(4)], Vec::new(), place, place.fraction());
        let order = |t: &Node| -> (Vec<u64>, Vec<u64>) {
            let (m, s, _) = main_and_stack_of(t, place).unwrap();
            (ids(m), ids(s))
        };
        let two = main_and_stack_again(&t, place, Some(place), 2, None, None, Some(1), None).unwrap();
        assert_eq!(order(&two), (vec![1, 2], vec![3, 4]));
        let one = main_and_stack_again(&two, place, Some(place), 1, None, None, Some(1), None).unwrap();
        assert_eq!(order(&one), (vec![1], vec![2, 3, 4]));
        let all = main_and_stack_again(&t, place, Some(place), 9, None, None, Some(1), None).unwrap();
        assert_eq!(order(&all), (vec![1, 2, 3, 4], vec![]));
        // A new window is the first main window; Make Main of a stack window
        // puts it first, the one that was first where it was.
        let new = main_and_stack_again(&two, place, Some(place), 2, Some((9, NewAt::First)), None, Some(1), None).unwrap();
        assert_eq!(order(&new), (vec![9, 1], vec![2, 3, 4]));
        let made = main_and_stack_again(&two, place, Some(place), 2, None, Some(4), Some(1), None).unwrap();
        assert_eq!(order(&made), (vec![4, 2], vec![3, 1]));
    }

    /// Main and Stack laid out as the protocol says it is
    /// (`Tile::main_and_stack_parts`, what Preferences' miniature draws),
    /// in every place, with none to five more windows.
    #[test]
    fn main_and_stack_parts_are_the_protocols() {
        for place in Tile::CHOOSABLE {
            for n in 0..=5usize {
                let t = main_and_stack(vec![w(0)], (1..=n as u64).map(w).collect(), Vec::new(), place, place.fraction());
                for (i, want) in place.main_and_stack_parts(n).into_iter().enumerate() {
                    let got = part_of(&t, i as u64);
                    assert!(near(got, want), "{place:?} with {n}: {i} at {got:?}, not {want:?}, in {t}");
                }
            }
        }
    }

    /// Main and Stack built again: a new window is the main one, the one
    /// that was first in the stack; a window made main takes the main
    /// place and the one that was takes its place in the stack; a main
    /// window gone gives its place to the one before it, and its place
    /// kept (taking no room) goes to the stack's end; a place changed
    /// keeps the main window and the stack's order; an edge dragged stays
    /// where it was let go.
    #[test]
    fn main_and_stack_built_again() {
        let place = Tile::CenterTwoThirds;
        let order = |t: &Node| {
            let (m, stack, _) = main_and_stack_of(t, place).unwrap_or_else(|| panic!("{t}"));
            m.into_iter().chain(stack).map(|n| if let Node::Window(i) = n { i } else { 0 }).collect::<Vec<u64>>()
        };
        let t = main_and_stack(vec![w(1)], vec![w(2), w(3)], Vec::new(), place, place.fraction());
        let new = main_and_stack_again(&t, place, Some(place), 1, Some((5, NewAt::First)), None, Some(1), None).unwrap();
        assert_eq!(order(&new), [5, 1, 2, 3]);
        let made = main_and_stack_again(&new, place, Some(place), 1, None, Some(2), Some(5), Some(1)).unwrap();
        assert_eq!(order(&made), [2, 1, 5, 3], "2 the main window, 5 where 2 was");
        // The main window closed (its place kept a while, taking no room):
        // the one before it is main again.
        let closed = Node::Kept(Kept { who: Who::Like { app_id: "a".into(), title: "t".into() }, holds: false, until: None });
        let mut gone = made.clone();
        gone.replace(&|n| *n == w(2), closed.clone());
        let back = main_and_stack_again(&gone, place, Some(place), 1, None, None, Some(2), Some(5)).unwrap();
        assert_eq!(main_and_stack_of(&back, place).map(|(m, ..)| m), Some(vec![w(5)]));
        let (_, stack, _) = main_and_stack_of(&back, place).unwrap();
        assert_eq!(stack.last(), Some(&closed), "its place at the stack's end: {back}");
        assert!(laid_parts(&back).iter().all(|(n, _)| *n != closed), "taking no room: {back}");
        // Another place: the same main window and order, in its part.
        let left = main_and_stack_again(&made, Tile::Left, Some(place), 1, None, None, Some(2), Some(5)).unwrap();
        let (m, stack, at) = main_and_stack_of(&left, Tile::Left).unwrap();
        assert_eq!((m, stack), (vec![w(2)], vec![w(1), w(5), w(3)]));
        assert!(near(at, Tile::Left.fraction()), "{at:?}");
        // Its edge dragged (the main window wider), then a window more: as
        // dragged.
        let mut dragged = made.clone();
        if let Node::Split { ratio, .. } = &mut dragged {
            *ratio = 0.1;
        }
        let more = main_and_stack_again(&dragged, place, Some(place), 1, Some((9, NewAt::First)), None, Some(2), None).unwrap();
        assert!((part_of(&more, 9).0 - 0.1).abs() < 1e-4, "{:?}", part_of(&more, 9));
        // A tree of another shape: in reading order, its main window kept.
        let other = Node::split(Cut::Rows, 0.5, w(7), Node::split(Cut::Columns, 0.5, w(8), w(2)));
        let built = main_and_stack_again(&other, place, Some(place), 1, None, None, Some(2), None).unwrap();
        assert_eq!(order(&built), [2, 7, 8]);
    }

    /// The two-thirds tiles in auto-tiling: the window in its part, the
    /// rest around it (in the middle, the nearest on its side).
    #[test]
    fn two_thirds_tiles_tile_the_rest_around() {
        let rest = || Some(Node::split(Cut::Columns, 0.5, w(2), w(3)));
        let centre = |i: u64| if i == 2 { (0.25, 0.5) } else { (0.75, 0.5) };
        for tile in [Tile::CenterTwoThirds, Tile::TopTwoThirds, Tile::BottomTwoThirds] {
            let t = tiled_into(rest(), 1, tile, &centre);
            assert!(near(part_of(&t, 1), tile.fraction()), "{tile:?}: {:?} in {t}", part_of(&t, 1));
        }
        let t = tiled_into(rest(), 1, Tile::CenterTwoThirds, &centre);
        assert!(near(part_of(&t, 2), (0.0, 0.0, 1.0 / 6.0, 1.0)) && near(part_of(&t, 3), (5.0 / 6.0, 0.0, 1.0 / 6.0, 1.0)), "{t}");
    }

    /// A tree kept across a restart comes back the same shape and ratios,
    /// each tile held for the windows its saved places name (and a place
    /// that took no room, a closed or away window's, taking none); splits as
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
        node.each_leaf(&mut |n| match n {
            Node::Kept(Kept { who: Who::Returning(s), holds, until: Some(_) }) => assert_eq!(*holds, s != &[99], "{s:?}"),
            n => panic!("{n:?}"),
        });
        // Laid out, the closed window's place takes no room, as before.
        let parts = layout(&node, &|_| (0.0, 0.0), Inset::default());
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().any(|(_, p)| (p.3 - 1.0).abs() < 1e-6 && p.0 > 0.29));
        // One with nothing saved to name it by keeps whether it held room;
        // from an app_server before that was saved, it did.
        let closed = |holds| Node::Kept(Kept { who: Who::Like { app_id: "b".into(), title: "t".into() }, holds, until: None });
        for holds in [false, true] {
            let saved = SavedNode::of(&closed(holds), &|_| vec![], &|_, _| None);
            assert!(matches!(saved.node(until), Node::Kept(Kept { holds: h, .. }) if h == holds));
        }
        let old: SavedNode = serde_json::from_str(r#"{"Like":{"app_id":"b","title":"t"}}"#).unwrap();
        assert!(matches!(old.node(until), Node::Kept(Kept { holds: true, .. })));
        // A least size of half the room holds the edge at a half.
        let laid = as_laid_out(&Node::split(Cut::Columns, 0.3, w(1), w(2)), &|id| if id == 1 { (0.5, 0.0) } else { (0.0, 0.0) }, Inset::default());
        assert!(matches!(laid, Node::Split { ratio, .. } if (ratio - 0.5).abs() < 1e-6));
        // The shape's ratios from the tree as it was laid out.
        let now = Node::split(Cut::Columns, 0.5, Node::Kept(Kept { who: Who::Window(1), holds: false, until: None }), w(2));
        let then = Node::split(Cut::Columns, 0.25, w(1), w(2));
        assert!(matches!(with_ratios_of(&now, &then), Node::Split { ratio, .. } if ratio == 0.25));
    }

    fn w(id: u64) -> Node {
        Node::Window(id)
    }

    /// A tile at its least size beside the area's edge stays that size
    /// when the tree is laid out again (an edge dragged till it stopped
    /// there): its least counts a whole gap, but there it gives up only
    /// half of one. (It was laid out 4 px wider, and its neighbours moved
    /// as the drag was let go.)
    #[test]
    fn a_tile_at_its_least_beside_the_edge_stays_so() {
        let area = Rectangle::new((0, 0).into(), (1000, 800).into());
        let gap = 8;
        // Window 1's least frame 200 wide, with a whole gap; 2's small.
        let min = |id: u64| if id == 1 { (208.0 / 1000.0, 0.0) } else { (0.1, 0.0) };
        // Where the drag left it: its frame 200 wide (its part's right
        // edge half a gap past that).
        let ratio = 204.0 / 1000.0;
        let laid = as_laid_out(&Node::split(Cut::Columns, ratio, w(1), w(2)), &min, Inset::of(area, gap));
        let Node::Split { ratio: kept, .. } = laid else { panic!("{laid}") };
        assert!((kept - ratio).abs() < 1e-6, "{kept}");
        let p = layout(&laid, &min, Inset::of(area, gap));
        assert_eq!(crate::shell::tiling::part(area, p[0].1, gap).size.w, 200);
        // On the other side too, and in the middle a whole gap still holds.
        let at_end = as_laid_out(&Node::split(Cut::Columns, 1.0 - ratio, w(2), w(1)), &min, Inset::of(area, gap));
        let Node::Split { ratio: kept, .. } = at_end else { panic!() };
        assert!((kept - (1.0 - ratio)).abs() < 1e-6, "{kept}");
        let middle = Node::split(Cut::Columns, 0.1, w(3), Node::split(Cut::Columns, 0.1, w(1), w(2)));
        let p = layout(&middle, &|id| if id == 1 { (208.0 / 1000.0, 0.0) } else { (0.05, 0.0) }, Inset::of(area, gap));
        let one = p.iter().find(|(n, _)| **n == w(1)).unwrap().1;
        assert!(crate::shell::tiling::part(area, one, gap).size.w >= 200);
    }

    /// A drag is held where the tree would hold the tiles as it's let go:
    /// a split's side no smaller than the least of all it holds (a row of
    /// two beside a column, each of the row at its least), and nowhere
    /// at all where the least sizes don't fit (the split is at their
    /// shares then, whatever its ratio).
    #[test]
    fn a_drag_lays_out_as_the_tree_will() {
        let area = Rectangle::new((0, 0).into(), (1000, 800).into());
        let gap = 8;
        // [1 | [2 / [3 | 4]]]: 3 and 4 each at least 200 wide (a whole gap
        // with it), 2 small.
        let min = |id: u64| match id {
            3 | 4 => (208.0 / 1000.0, 0.0),
            _ => (0.05, 0.0),
        };
        let tree = Node::split(Cut::Columns, 0.4, w(1), Node::split(Cut::Rows, 0.5, w(2), Node::split(Cut::Columns, 0.5, w(3), w(4))));
        let laid: HashMap<u64, Rectangle<i32, Logical>> =
            layout(&tree, &min, Inset::of(area, gap)).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, crate::shell::tiling::part(area, p, gap))) } else { None }).collect();
        let part_of = |i: u64| laid.get(&i).map(|r| fraction_of(area, *r, gap));
        // 1's right edge dragged right by `dx`: 2's and 3's left edges with it.
        let drag = |dx: i32| {
            let grow = |r: Rectangle<i32, Logical>| Rectangle::new(r.loc, (r.size.w + dx, r.size.h).into());
            let shrink = |r: Rectangle<i32, Logical>| Rectangle::new((r.loc.x + dx, r.loc.y).into(), (r.size.w - dx, r.size.h).into());
            vec![(1, grow(laid[&1])), (2, shrink(laid[&2])), (3, shrink(laid[&3]))]
        };
        assert!(lays_out_as(&tree, &drag(0), &part_of, &min, area, gap));
        assert!(lays_out_as(&tree, &drag(50), &part_of, &min, area, gap));
        // 3 down to its least (200), and no further.
        let room = laid[&3].size.w - 200;
        assert!(lays_out_as(&tree, &drag(room), &part_of, &min, area, gap));
        assert!(!lays_out_as(&tree, &drag(room + 10), &part_of, &min, area, gap));
        // 1, 3 and 4 can't all be 600 wide: the split stays at the shares.
        let wide = |id: u64| if id == 2 { (0.05, 0.0) } else { (0.6, 0.0) };
        let shares: HashMap<u64, Rectangle<i32, Logical>> =
            layout(&tree, &wide, Inset::of(area, gap)).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, crate::shell::tiling::part(area, p, gap))) } else { None }).collect();
        let at = |i: u64| shares.get(&i).map(|r| fraction_of(area, *r, gap));
        let moved = |dx: i32| vec![(1, Rectangle::new(shares[&1].loc, (shares[&1].size.w + dx, shares[&1].size.h).into()))];
        assert!(lays_out_as(&tree, &moved(0), &at, &wide, area, gap));
        assert!(!lays_out_as(&tree, &moved(-20), &at, &wide, area, gap));
    }

    fn parts(t: &Node) -> Vec<(u64, Part)> {
        layout(t, &|_| (0.0, 0.0), Inset::default()).into_iter().filter_map(|(n, p)| if let Node::Window(i) = n { Some((*i, p)) } else { None }).collect()
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

    /// A place kept from an earlier restart isn't saved under its old
    /// slots, which this save numbers afresh: no slot is kept twice.
    #[test]
    fn an_old_kept_place_isnt_saved_by_its_old_slots() {
        let old = Node::Kept(Kept { who: Who::Returning(vec![4]), holds: true, until: None });
        let tree = Node::split(Cut::Rows, 0.8, Node::Window(9), old);
        let saved = SavedNode::of(&tree, &|id| if id == 9 { vec![4] } else { Vec::new() }, &|_, _| None);
        let restored = saved.node(Instant::now() + Duration::from_secs(20));
        let mut fours = 0;
        restored.each_leaf(&mut |n| {
            if matches!(n, Node::Kept(Kept { who: Who::Returning(s), .. }) if s.contains(&4)) {
                fours += 1;
            }
        });
        assert_eq!(fours, 1, "{restored}");
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
        let p = layout(&t, &|i| if i == 1 { (0.3, 0.1) } else { (0.2, 0.1) }, Inset::default());
        assert!((p[0].1.2 - 0.3).abs() < 1e-5, "{p:?}");
        // Both can't fit: each its share of what they want.
        let p = layout(&t, &|_| (0.6, 0.1), Inset::default());
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
        take_ratios(&mut t, &|i| Some(if i == 1 { (0.0, 0.0, 0.71, 1.0) } else { (0.71, 0.0, 0.29, 1.0) }), (0.003, 0.003));
        assert!(near(parts(&t)[0].1, (0.0, 0.0, 0.71, 1.0)), "{t:?}");
        // Within the right half: just there too, and the splits that
        // didn't move stay as they are.
        let mut t = Node::split(Cut::Columns, 0.5, w(1), Node::split(Cut::Columns, 0.5, w(2), w(3)));
        take_ratios(
            &mut t,
            &|i| {
                Some(match i {
                    1 => (0.0, 0.0, 0.5, 1.0),
                    2 => (0.5, 0.0, 0.08, 1.0),
                    _ => (0.58, 0.0, 0.42, 1.0),
                })
            },
            (0.003, 0.003),
        );
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
