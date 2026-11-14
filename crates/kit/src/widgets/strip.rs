//! A row of document tabs under a window's menu bar (Terminal's shells):
//! each a title on a quiet band, the selected one on a raised plate lit
//! from above (as BeOS's controls are), all inside the band; each with a
//! close box (middle-click closes too), a + after the last, tabs dragged
//! along the row to reorder them and out of it to tear them off into a
//! window of their own. A group of tabs sits in a capsule of its colour,
//! led by a chip with its name. Titles too long for their tab are cut
//! short with "…". Or, [`TabStrip::side`], a column of them down the
//! content's left side.
//! Each tab is a `Tab` to agents, its whole title and its status in its
//! name.
//!
//! What the person did comes to [`TabStrip::on_action`] as a
//! [`StripAction`]. A window of the app being moved over this one marks
//! where its tabs would go, and let go there, [`on_window_dropped`]'s
//! handler gets the place ([`strip_place`]).
//!
//! It is one drawing: where everything goes is worked out as it's drawn
//! and kept with the press, the drag and what's under the pointer in the
//! element's own state, which its handlers go by; a pointer moving over
//! it draws it again only when what it's over changes.

use std::{
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use olo_interface::{
    BorderStyle, Bounds, BoxShadow, Corners, CursorStyle, Edges, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter,
    Pixels, Point, Rgba, SharedString, Styled as _, WindowCx, agent, black, canvas, fill, linear_color_stop, linear_gradient, point, px, quad, size, transparent_black, white,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// Paints a mark into its rectangle (a tab's icon, its badge).
pub type PaintFn = Rc<dyn Fn(&mut Painter, Bounds<Pixels>)>;

/// One tab in a [`TabStrip`].
#[derive(Clone, Default)]
pub struct StripTab {
    pub label: SharedString,
    /// Paints the 16×16 icon left of the label.
    pub icon: Option<PaintFn>,
    /// What the tab is doing, for agents and screen readers ("waiting
    /// for an answer"); the icon shows it.
    pub status: Option<SharedString>,
    /// Paints a [`STRIP_BADGE`]-square mark at the tab's right, where its
    /// close box goes (Terminal's pull request state), which takes its
    /// place under the pointer. Like the icon, only on a tab wide enough.
    pub badge: Option<PaintFn>,
    /// The group it's in (into [`TabStrip::groups`]). A group's tabs are
    /// side by side.
    pub group: Option<usize>,
    /// Left out of something the window shows (Terminal's tab hidden
    /// when tiled), and what: its icon and title drawn faded (at
    /// [`STRIP_FADED`]), the rest as any tab's; agents read the reason
    /// in its name ("hidden when tiled").
    pub faded: Option<SharedString>,
}

/// A group of a [`TabStrip`]'s tabs: the name on its chip, and its colour.
#[derive(Clone)]
pub struct StripGroup {
    pub name: SharedString,
    pub color: Hsla,
}

/// What the person did to a [`TabStrip`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StripAction {
    Select(usize),
    Close(usize),
    /// The + after the last tab.
    New,
    /// A right-click on a tab, at the pointer (in the window).
    Menu(usize, Point<Pixels>),
    /// A tab dragged along the row to another place: `to` among the
    /// others, and the group it's in there.
    Move {
        from: usize,
        to: usize,
        group: Option<usize>,
    },
    /// A group's chip dragged: its tabs go to `to` among the others.
    MoveGroup {
        group: usize,
        to: usize,
    },
    /// A click on a group's chip, at the pointer (with
    /// [`TabStrip::group_selection`], a right-click: its menu).
    Group(usize, Point<Pixels>),
    /// A click on a group's chip with [`TabStrip::group_selection`].
    ToggleGroup(usize),
    /// A tab dragged out of the strip (`at`, the pointer, in the window's
    /// coordinates, maybe outside it), the button still held: a window of
    /// its own, which the app opens under the pointer and moves with it.
    TearOff {
        tab: usize,
        at: Point<Pixels>,
    },
    /// Likewise a group, by its chip.
    TearOffGroup {
        group: usize,
        at: Point<Pixels>,
    },
    /// A [`TabStrip::side`] strip's edge dragged: its width now, and
    /// whether it was let go (time to keep it).
    Resize {
        width: f32,
        done: bool,
    },
}

/// How far out of a strip a tab or group is dragged to tear it off.
const TEAR: f32 = 40.0;
/// How far the pointer goes before a press on a tab is a drag.
const DRAG: f32 = 4.0;
/// Widest a tab grows, for a strip that keeps them narrow
/// ([`TabStrip::max_tab_width`]; else up to `TAB_MAX`).
pub const STRIP_TAB_MAX: f32 = 220.0;
/// How tall a strip is: roomier than a window tab, as a row of documents
/// is something you aim at all day.
pub const STRIP_HEIGHT: f32 = 32.0;
/// How tall a band of heads under a strip is (Terminal's panes' in Tile
/// All Tabs): a tab's plate and a little.
pub const STRIP_ROW: f32 = 26.0;
/// A tab's plate (the selected one's, one under the pointer's): its
/// height, centred in the strip, its rounding, and its room from its
/// slot's sides.
const PLATE: f32 = 24.0;
const TAB_RADIUS: f32 = 7.0;
const PLATE_GAP: f32 = 2.0;
/// The room round a group's tabs inside its capsule, which is rounded
/// to match (so the corners are concentric).
const HALO: f32 = 2.0;
const CAPSULE_RADIUS: f32 = TAB_RADIUS + HALO;
/// A tab's narrowest and widest: each as wide as its title needs between
/// these (the widest cut back when they don't all fit).
const TAB_MIN: f32 = 140.0;
const TAB_MAX: f32 = 320.0;
/// What a tab has besides its title: room either side, its icon and the
/// gap after it, its close button and the gap before it.
const TAB_CHROME: f32 = m::SPACING + 16.0 + m::MARK_GAP + m::TIGHT + CLOSE + CLOSE_INSET;
/// The close button, and its room from the tab's right.
const CLOSE: f32 = 16.0;
const CLOSE_INSET: f32 = 6.0;
/// A group's chip: its height, text size and room either side of its
/// name, its room from its capsule's left (as from its top), and its gap
/// before its first tab (with the inset, even: the tabs start on the
/// 2-point grid, so on device pixels at 1.5×).
pub const CHIP_HEIGHT: f32 = 18.0;
const CHIP_SIZE: f32 = style::SMALL_SIZE;
const CHIP_PAD: f32 = 8.0;
const CHIP_INSET: f32 = (PLATE + 2.0 * HALO - CHIP_HEIGHT) / 2.0;
const CHIP_GAP: f32 = 10.0 - CHIP_INSET;
/// The + button.
const NEW_SIZE: f32 = 22.0;
/// How tall a tab is in a side strip (less, to fit, when there are many).
pub const STRIP_SIDE_TAB: f32 = 24.0;
/// How big a tab's badge is: as the text is.
pub const STRIP_BADGE: f32 = 12.0;
/// How strongly a [`StripTab::faded`] tab's icon and title are drawn.
pub const STRIP_FADED: f32 = 0.45;
/// How tall a band beside a side strip is (Terminal's pane heads); the
/// strip's first row is centred on it. As tall as a strip along the top,
/// so a group's chip heading the first row is where it is there, well
/// under the menu bar's titles.
pub const STRIP_SIDE_HEAD: f32 = STRIP_HEIGHT;
/// How tall a group's heading is in a side strip: as Deskbar's
/// workspace headings.
pub const STRIP_SIDE_HEADING: f32 = 20.0;
/// How wide a side strip starts, and the narrowest and widest its edge
/// can be dragged to.
pub const STRIP_SIDE_WIDTH: f32 = 180.0;
pub const STRIP_SIDE_MIN: f32 = 120.0;
pub const STRIP_SIDE_MAX: f32 = 400.0;

type ActionFn = Rc<dyn Fn(&StripAction, &mut WindowCx)>;

pub struct TabStrip {
    id: SharedString,
    tabs: Vec<StripTab>,
    active: usize,
    max_width: f32,
    new_tab: bool,
    groups: Vec<StripGroup>,
    chosen: Option<Vec<bool>>,
    side: bool,
    /// The window it's in, as app_server knows it (its surface).
    window: Option<u32>,
    on_action: Option<ActionFn>,
}

impl TabStrip {
    pub fn new(id: impl Into<SharedString>, tabs: Vec<StripTab>, active: usize) -> TabStrip {
        TabStrip { id: id.into(), tabs, active, max_width: f32::INFINITY, new_tab: true, groups: Vec::new(), chosen: None, side: false, window: None, on_action: None }
    }

    /// Down the content's left side rather than along its top: a column
    /// of rows, the selected one raised, each group under its chip in a
    /// capsule; New Tab a row after the last; tabs drag up and down to
    /// reorder, and
    /// the strip's right edge drags to make it wider or narrower
    /// ([`StripAction::Resize`]). It fills the element's bounds: the app
    /// gives it its width ([`STRIP_SIDE_WIDTH`] to start).
    pub fn side(mut self) -> Self {
        self.side = true;
        self
    }

    /// Tabs no wider than `w` (else up to `TAB_MAX`).
    pub fn max_tab_width(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }

    /// Without the + button.
    pub fn no_new_tab(mut self) -> Self {
        self.new_tab = false;
        self
    }

    /// The groups its tabs are in ([`StripTab::group`]).
    pub fn groups(mut self, groups: Vec<StripGroup>) -> Self {
        self.groups = groups;
        self
    }

    /// Groups can be chosen (`chosen[g]`): a chosen one's chip is lit;
    /// clicking a chip gives [`StripAction::ToggleGroup`], the right
    /// button [`StripAction::Group`] (its menu).
    pub fn group_selection(mut self, chosen: Vec<bool>) -> Self {
        self.chosen = Some(chosen);
        self
    }

    /// The window it's in (`WindowCx::surface_id`), for one of the app's
    /// windows moved over it to mark where its tabs would go
    /// ([`on_window_dropped`]).
    pub fn in_window(mut self, surface: Option<u32>) -> Self {
        self.window = surface;
        self
    }

    pub fn on_action(mut self, f: impl Fn(&StripAction, &mut WindowCx) + 'static) -> Self {
        self.on_action = Some(Rc::new(f));
        self
    }
}

/// What's under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Tab(usize),
    Close(usize),
    Chip(usize),
    New,
    Edge,
}

/// Between drawings: the press and what it became, what the pointer is
/// over, and where everything was as last drawn (what its handlers go
/// by).
#[derive(Default)]
struct StripState {
    press: Option<(Part, MouseButton, Point<Pixels>)>,
    drag: Option<Drag>,
    pointer: Option<Point<Pixels>>,
    under: Option<Part>,
    /// Where the pointer was last, in the window.
    at: Option<Point<Pixels>>,
    laid: Option<Rc<Layout>>,
}

impl StripState {
    /// The press is over (released, or what it held was torn off).
    fn let_go(&mut self) {
        (self.press, self.drag, self.pointer) = (None, None, None);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Tab(usize),
    Group(usize),
    Edge,
}

/// Where everything goes, worked out as it's drawn.
struct Layout {
    full: Bounds<Pixels>,
    slots: Vec<Bounds<Pixels>>,
    /// Each group's chip (heading, down the side): the group, its first
    /// tab, where.
    chips: Vec<(usize, usize, Bounds<Pixels>)>,
    closes: Vec<Bounds<Pixels>>,
    new: Option<Bounds<Pixels>>,
    edge: Option<Bounds<Pixels>>,
    /// Past the last tab (x along the top, y down the side).
    end: Pixels,
    groups_of: Vec<Option<usize>>,
    /// The tabs' centre line (along the top).
    cy: Pixels,
}

impl Layout {
    /// Where something dragged would go before tab `i`: before its chip,
    /// if it leads a group.
    fn before(&self, i: usize, side: bool) -> Pixels {
        match self.chips.iter().find(|c| c.1 == i) {
            Some(c) if side => c.2.top(),
            Some(c) => c.2.left() - px(CHIP_INSET + m::TIGHT / 2.0),
            None if side => self.slots[i].top(),
            None => self.slots[i].left(),
        }
    }

    fn along(b: Bounds<Pixels>, side: bool) -> Pixels {
        if side { b.center().y } else { b.center().x }
    }

    /// What of it `at` is over: a close box before its tab, a tab before
    /// what else is there.
    fn part_at(&self, at: Point<Pixels>) -> Option<Part> {
        if !self.full.contains(&at) {
            return None;
        }
        if let Some(i) = self.closes.iter().position(|r| r.contains(&at)) {
            return Some(Part::Close(i));
        }
        if let Some(i) = self.slots.iter().position(|r| r.contains(&at)) {
            return Some(Part::Tab(i));
        }
        if let Some((g, _, _)) = self.chips.iter().find(|(_, _, r)| r.contains(&at)) {
            return Some(Part::Chip(*g));
        }
        if self.new.is_some_and(|r| r.contains(&at)) {
            return Some(Part::New);
        }
        self.edge.is_some_and(|r| r.contains(&at)).then_some(Part::Edge)
    }
}

/// The strip each window drew last (by its surface), for a window let go
/// over it.
#[derive(Default)]
struct Strips {
    placed: HashMap<u32, Placed>,
    /// Where a window of the app being moved is over each window.
    over: HashMap<u32, Point<Pixels>>,
    dropped: Option<Dropped>,
}

type Dropped = Arc<dyn Fn(u32, u32, usize) + Send + Sync>;

fn strips() -> std::sync::MutexGuard<'static, Strips> {
    static STRIPS: OnceLock<Mutex<Strips>> = OnceLock::new();
    STRIPS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
struct Placed {
    area: Bounds<Pixels>,
    mids: Vec<Pixels>,
    side: bool,
}

impl Placed {
    fn place(&self, at: Point<Pixels>) -> Option<usize> {
        let near = self.area.dilate(px(m::SPACING));
        let along = if self.side { at.y } else { at.x };
        near.contains(&at).then(|| self.mids.iter().filter(|&&m| m < along).count())
    }
}

/// Where among the tabs of the strip the window with surface `window`
/// drew last something let go at `at` (in the window) goes, if `at` is on
/// the strip.
pub fn strip_place(window: u32, at: Point<Pixels>) -> Option<usize> {
    strips().placed.get(&window)?.place(at)
}

/// What to do with one of the app's windows let go on another's strip
/// (the window's surface, the one it's on, the place among that one's
/// tabs): Terminal's tabs go in there. Called on the thread that heard
/// (the commands socket's).
pub fn on_window_dropped(f: impl Fn(u32, u32, usize) + Send + Sync + 'static) {
    strips().dropped = Some(Arc::new(f));
}

/// app_server says one of the app's windows being moved is over the
/// window with surface `onto` at `at` (None: not any more): its strip
/// marks the place.
pub(crate) fn window_over(onto: Option<u32>, at: Point<Pixels>) {
    let before: Vec<u32> = {
        let mut strips = strips();
        let before = strips.over.drain().map(|(id, _)| id).collect();
        if let Some(o) = onto {
            strips.over.insert(o, at);
        }
        before
    };
    olo_interface::all_windows(move |cx| {
        if cx.surface_id().is_some_and(|s| before.contains(&s) || Some(s) == onto) {
            cx.refresh();
        }
    });
}

/// app_server says the window with surface `window` was let go over
/// `onto` at `at`.
pub(crate) fn window_dropped(window: u32, onto: u32, at: Point<Pixels>) {
    window_over(None, at);
    let place = strip_place(onto, at);
    let f = strips().dropped.clone();
    if let (Some(place), Some(f)) = (place, f) {
        f(window, onto, place);
    }
}

/// The colour `t` of the way from `a` to `b`.
fn lerp(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    Hsla::from(Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t })
}

/// A strip's +, ten points across: arms a point and a half thick in
/// whole device pixels, as long either side of the other at any scale.
fn plus_mark(g: &mut Painter, center: Point<Pixels>, color: Hsla) {
    let s = g.scale_factor();
    let thick = (1.5 * s).round();
    let len = (10.0 * s).round();
    let len = len + (len - thick).rem_euclid(2.0);
    let k = style::snap_centred(g, center, len / s, len / s);
    let arm = px((len - thick) / 2.0 / s);
    style::fill_rect(g, Bounds::from_corners(point(k.left(), k.top() + arm), point(k.right(), k.bottom() - arm)), color);
    style::fill_rect(g, Bounds::from_corners(point(k.left() + arm, k.top()), point(k.right() - arm, k.bottom())), color);
}

/// A tab's name to agents: its title, what it's doing, its group.
fn tab_name(tab: &StripTab, group: Option<&StripGroup>) -> SharedString {
    let mut name = tab.label.to_string();
    if let Some(s) = &tab.status {
        name = format!("{name}, {s}");
    }
    if let Some(f) = &tab.faded {
        name = format!("{name}, {f}");
    }
    if let Some(g) = group {
        name = format!("{name}, in {}", g.name);
    }
    name.into()
}

/// The group a tab is in once dropped: `groups` each tab's, tab `from`
/// put at `to` among the others, the pointer `before` that place or
/// after. Among a group's tabs it is in that group; at a group's edge,
/// the group of the tab the pointer is over; a group's only tab keeps its
/// group unless dropped on another.
fn dropped_into(groups: &[Option<usize>], from: usize, to: usize, before: bool) -> Option<usize> {
    let others: Vec<Option<usize>> = groups.iter().enumerate().filter(|&(i, _)| i != from).map(|(_, g)| *g).collect();
    let left = to.checked_sub(1).and_then(|i| others.get(i)).copied().flatten();
    let right = others.get(to).copied().flatten();
    let own = groups.get(from).copied().flatten();
    if left.is_some() && left == right {
        return left;
    }
    let over = if before { left } else { right };
    if own.is_some() && !others.contains(&own) { over.or(own) } else { over }
}

/// The place among the tabs outside group `g` nearest `to` that isn't
/// inside another group: where a dragged group goes.
fn group_place(groups: &[Option<usize>], g: usize, to: usize) -> usize {
    let others: Vec<Option<usize>> = groups.iter().copied().filter(|x| *x != Some(g)).collect();
    let between = |k: usize| k == 0 || k == others.len() || others[k - 1].is_none() || others[k - 1] != others[k];
    (0..=others.len()).filter(|&k| between(k)).min_by_key(|&k| k.abs_diff(to)).unwrap_or(0)
}

impl TabStrip {
    /// Where everything goes along the top of `full`: each group's chip,
    /// then its tabs; each tab as wide as its title needs (within
    /// [`TAB_MIN`]–[`TAB_MAX`]), the widest cut back when they don't all
    /// fit ([`fit_widths`]).
    fn layout_top(&self, g: &Painter, full: Bounds<Pixels>) -> Layout {
        let n = self.tabs.len();
        let groups_of: Vec<Option<usize>> = self.tabs.iter().map(|t| t.group.filter(|&g| g < self.groups.len())).collect();
        let leads = |i: usize| groups_of[i].is_some() && (i == 0 || groups_of[i - 1] != groups_of[i]);
        // A capsule has a gap before it (unless first) and after it
        // (unless a capsule follows, which has its own).
        let ends = |i: usize| groups_of[i].is_some() && i + 1 < n && groups_of[i + 1].is_none();
        // The tabs' faces and the chips are centred in the band.
        let cy = ((f32::from(full.top()) + f32::from(full.bottom())) / 2.0).round();
        let chip_w = |group: usize| chip_width(&self.groups[group].name);
        let mut extra = 0.0;
        for i in 0..n {
            if leads(i) {
                extra += if i > 0 { m::TIGHT } else { 0.0 } + CHIP_INSET + chip_w(groups_of[i].unwrap_or(0)) + CHIP_GAP;
            }
            if ends(i) {
                extra += m::TIGHT;
            }
        }
        let new_w = if self.new_tab { NEW_SIZE + m::TIGHT } else { 0.0 };
        let medium = style::ui_font(FontWeight::MEDIUM);
        let natural: Vec<f32> = self
            .tabs
            .iter()
            .map(|t| (f32::from(style::text_width(t.label.clone(), style::PLAIN_SIZE, medium.clone())) + TAB_CHROME).clamp(TAB_MIN, TAB_MAX.min(self.max_width).max(TAB_MIN)))
            .collect();
        let room = f32::from(full.size.width) - 2.0 * m::TIGHT - new_w - extra;
        let widths = fit_widths(&natural, room, TAB_MIN);
        let mut chips = Vec::new();
        let mut slots = Vec::with_capacity(n);
        let mut x = f32::from(full.left()) + m::TIGHT;
        for i in 0..n {
            if let Some(group) = groups_of[i].filter(|_| leads(i)) {
                x += if i > 0 { m::TIGHT } else { 0.0 } + CHIP_INSET;
                let w = chip_w(group);
                chips.push((group, i, style::snap(g, Bounds { origin: point(px(x), px(cy - CHIP_HEIGHT / 2.0)), size: size(px(w), px(CHIP_HEIGHT)) })));
                x += w + CHIP_GAP;
            }
            slots.push(Bounds::from_corners(point(px(x), full.top()), point(px(x + widths[i]), full.bottom())));
            x += widths[i];
            if ends(i) {
                x += m::TIGHT;
            }
        }
        let end = px(x);
        let closes = slots.iter().map(|r| style::snap_centred(g, point(r.right() - px(CLOSE_INSET + CLOSE / 2.0), px(cy)), CLOSE, CLOSE)).collect();
        let new = self.new_tab.then(|| style::snap_centred(g, point(end + px(m::TIGHT + NEW_SIZE / 2.0), px(cy)), NEW_SIZE, NEW_SIZE));
        Layout { full, slots, chips, closes, new, edge: None, end, groups_of, cy: px(cy) }
    }

    /// Where everything goes down `full`.
    fn layout_side(&self, g: &Painter, full: Bounds<Pixels>) -> Layout {
        let n = self.tabs.len();
        let groups_of: Vec<Option<usize>> = self.tabs.iter().map(|t| t.group.filter(|&g| g < self.groups.len())).collect();
        let leads = |i: usize| groups_of[i].is_some() && (i == 0 || groups_of[i - 1] != groups_of[i]);
        let headings = (0..n).filter(|&i| leads(i)).count();
        let line = style::hairline(g);
        let (left, right) = (full.left(), full.right() - line);
        let rows = n + usize::from(self.new_tab);
        let natural = rows as f32 * STRIP_SIDE_TAB + headings as f32 * STRIP_SIDE_HEADING;
        let height = f32::from(full.size.height);
        let row_h = if natural <= height { STRIP_SIDE_TAB } else { (((height - headings as f32 * STRIP_SIDE_HEADING) / rows.max(1) as f32) / 2.0).floor().max(8.0) * 2.0 };
        let row = |g: &Painter, y: f32, h: f32| style::snap(g, Bounds::from_corners(point(left, px(y)), point(right, px(y + h))));
        let first_h = if n > 0 && leads(0) { STRIP_SIDE_HEADING } else { row_h };
        let mut y = f32::from(full.top()) + ((STRIP_SIDE_HEAD - first_h) / 2.0).max(0.0).floor();
        let mut chips = Vec::new();
        let mut slots = Vec::with_capacity(n);
        for i in 0..n {
            if let Some(group) = groups_of[i].filter(|_| leads(i)) {
                chips.push((group, i, row(g, y, STRIP_SIDE_HEADING)));
                y += STRIP_SIDE_HEADING;
            }
            slots.push(row(g, y, row_h));
            y += row_h;
        }
        let end_row = row(g, y, row_h);
        let closes = slots.iter().map(|r| style::snap_centred(g, point(r.right() - px(m::TIGHT + CLOSE_INSET + CLOSE / 2.0), r.center().y.round()), CLOSE, CLOSE)).collect();
        let edge = Some(Bounds::from_corners(point(full.right() - px(4.0), full.top()), full.bottom_right()));
        Layout { full, slots, chips, closes, new: self.new_tab.then_some(end_row), edge, end: end_row.top(), groups_of, cy: full.top() }
    }
}

/// Where a side strip's tab faces start in their rows: their icons at
/// the window's inset, under the menu bar's first title.
const SIDE_FACE: f32 = m::WINDOW_INSET - m::SPACING;

/// A side strip's group chip in its heading `head`: its name at the
/// window's inset (under the tabs' icons and the menu bar's first title),
/// centred on the heading. (As wide as its name needs: nothing follows it
/// across, so no grid to keep, and every chip starts at one place.)
fn side_chip(g: &Painter, head: Bounds<Pixels>, name: &SharedString) -> Bounds<Pixels> {
    let text = f32::from(style::text_width(name.clone(), CHIP_SIZE, style::ui_font(FontWeight::BOLD)));
    let w = (text + 2.0 * CHIP_PAD).max(CHIP_HEIGHT).min(f32::from(head.size.width) - m::WINDOW_INSET);
    let left = head.left() + px(m::WINDOW_INSET - CHIP_PAD);
    let cy = style::snap_coord(g, head.center().y.round());
    style::snap(g, Bounds { origin: point(left, cy - px(CHIP_HEIGHT / 2.0)), size: size(px(w), px(CHIP_HEIGHT)) })
}

/// A side strip's New Tab in its row `r`, as it shows: its + and its
/// name (the whole row takes a click).
fn side_new_tab(r: Bounds<Pixels>) -> Bounds<Pixels> {
    let text = style::text_width("New Tab", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL));
    let left = r.left() + px(SIDE_FACE + m::SPACING);
    Bounds::from_corners(point(left, r.top()), point((left + px(16.0 + m::MARK_GAP) + text).min(r.right()), r.bottom()))
}

/// The colours a tab's face is drawn in.
#[derive(Clone, Copy)]
struct FaceInks {
    /// The selected tab's fill and text.
    fill: Hsla,
    ink: Hsla,
    /// The others' text.
    dim: Hsla,
    /// Under the pointer.
    hover: Hsla,
}

/// A tab's face: its icon (if `icon` and it has one) and title, its close
/// button (while selected or under the pointer; a badge in its place
/// otherwise), centred on `cy`.
#[allow(clippy::too_many_arguments)]
fn tab_face(g: &mut Painter, tab: &StripTab, selected: bool, (pointed, on_close): (bool, bool), (r, cy, close): (Bounds<Pixels>, Pixels, Bounds<Pixels>), icon: bool, inks: FaceInks) {
    // (Under the pointer, as the selected one's: it's what a click
    // brings forward.)
    let text = if selected || pointed { inks.ink } else { inks.dim };
    // A badge sits where the close button goes, and gives way to it under
    // the pointer (the selected tab's too, so its state shows).
    let badge = tab.badge.clone().filter(|_| icon);
    if pointed || selected && badge.is_none() {
        let mut color = lerp(text, if selected { inks.fill } else { inks.hover }, 0.35);
        if on_close {
            let back = if selected { inks.fill } else { inks.hover };
            g.paint_quad(fill(close, lerp(back, text, 0.14)).corner_radii(px(CLOSE / 2.0)));
            color = text;
        }
        let k = style::inset(g, close, 5.0);
        g.paint_segment(k.origin, k.bottom_right(), px(1.25), true, color);
        g.paint_segment(point(k.right(), k.top()), point(k.left(), k.bottom()), px(1.25), true, color);
    }
    let end = close.left() - px(m::TIGHT);
    if let Some(paint) = badge.filter(|_| !pointed) {
        let b = style::snap_centred(g, close.center(), STRIP_BADGE, STRIP_BADGE);
        paint(g, b);
    }
    // Its icon and title, faded if it's left out of something.
    g.with_element_opacity(tab.faded.as_ref().map(|_| STRIP_FADED), |g| {
        // The first tab's icon is under the menu bar's first title.
        let mut left = r.left() + px(m::SPACING);
        if let Some(paint) = tab.icon.as_ref().filter(|_| icon) {
            let b = style::snap(g, Bounds { origin: point(left, cy - px(8.0)), size: size(px(16.0), px(16.0)) });
            paint(g, b);
            left = b.right() + px(m::MARK_GAP);
        }
        let font = style::ui_font(if selected { FontWeight::MEDIUM } else { FontWeight::NORMAL });
        style::label_fit(g, left, cy, style::Align::Left, tab.label.clone(), style::PLAIN_SIZE, font, text, end - left);
    });
}

/// A group's chip in `r`: its name on a pill of its colour (`filled`, as
/// a chosen one in Tile All Tabs is, or when not choosing; else its
/// colour's outline and name), a little lighter under the pointer.
pub fn chip(g: &mut Painter, r: Bounds<Pixels>, name: &SharedString, color: Hsla, filled: bool, hovered: bool) {
    let radius = px(f32::from(r.size.height) / 2.0);
    let line = style::hairline(g);
    let color = if hovered { lerp(color, white(), 0.12) } else { color };
    let ink = if filled {
        g.paint_quad(fill(r, color).corner_radii(radius));
        ink_on(color)
    } else {
        g.paint_quad(quad(r, radius, color.opacity(0.12), line, color, BorderStyle::Solid));
        color
    };
    style::label(g, r.center().x, r.center().y, style::Align::Center, name.clone(), CHIP_SIZE, style::ui_font(FontWeight::BOLD), ink);
}

/// Black or white, whichever reads on `fill`.
fn ink_on(fill: Hsla) -> Hsla {
    let c = Rgba::from(fill);
    let lum = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    if lum > 0.62 { black().opacity(0.82) } else { white() }
}

/// How wide a group's chip is for `name`.
pub fn chip_width(name: &SharedString) -> f32 {
    let w = f32::from(style::text_width(name.clone(), CHIP_SIZE, style::ui_font(FontWeight::BOLD)));
    ((w + 2.0 * CHIP_PAD).max(CHIP_HEIGHT) / 2.0).ceil() * 2.0
}

impl IntoElement for TabStrip {
    fn into_element(self) -> Element {
        let me = Rc::new(self);
        let id = SharedString::from(format!("olo-strip-{}", me.id));
        let s = me.clone();
        let mut element = canvas(move |g, full| {
            let l = Rc::new(if s.side { s.layout_side(g, full) } else { s.layout_top(g, full) });
            let (drag, pointer, press, under) = {
                let st = g.node_state::<StripState>();
                st.laid = Some(l.clone());
                // (What the pointer is over as things are laid out now: a
                // tab may have come or gone under it.)
                st.under = st.at.and_then(|at| l.part_at(at));
                (st.drag, st.pointer, st.press, st.under)
            };
            // A tab being dragged: lifted off its slot, as far along
            // as the pointer has gone since the press.
            let lift = match (drag, pointer, press) {
                (Some(Drag::Tab(i)), Some(at), Some((_, _, from))) => Some((i, if s.side { at.y - from.y } else { at.x - from.x })),
                _ => None,
            };
            let under = under.filter(|_| drag.is_none());
            if s.side {
                paint_side(g, &s, full, &l, under, drag, lift);
            } else {
                paint_top(g, &s, full, &l, under, drag, lift);
            }
            let p = theme::palette();
            let n = s.tabs.len();
            // Where a window of the app let go here would go, and,
            // while one is over it, that place marked.
            let mids: Vec<Pixels> = l.slots.iter().map(|r| Layout::along(*r, s.side)).collect();
            let placed = Placed { area: full, mids, side: s.side };
            let over = s.window.and_then(|w| {
                let mut strips = strips();
                strips.placed.insert(w, placed.clone());
                strips.over.get(&w).copied()
            });
            let mark = style::c(p.keyboard_navigation);
            let draw_mark = |g: &mut Painter, at: Pixels| {
                let r = if s.side {
                    let y = at.clamp(full.top() + px(1.0), full.bottom() - px(1.0));
                    Bounds::from_corners(point(full.left(), y - px(1.0)), point(full.right(), y + px(1.0)))
                } else {
                    let x = at.clamp(full.left() + px(1.0), full.right() - px(2.0));
                    Bounds::from_corners(point(x - px(1.0), l.cy - px(PLATE / 2.0)), point(x + px(1.0), l.cy + px(PLATE / 2.0)))
                };
                let r = style::snap(g, r);
                style::fill_rect(g, r, mark);
            };
            let end_of =
                |others: &[usize], to: usize| others.get(to).map_or_else(|| others.last().map_or(l.end, |&i| if s.side { l.slots[i].bottom() } else { l.slots[i].right() }), |&i| l.before(i, s.side));
            if let Some(to) = over.and_then(|at| placed.place(at)) {
                draw_mark(g, if to < n { l.before(to, s.side) } else { l.end });
            }
            // A tab or group being dragged: a mark where it'd go.
            match (drag, pointer) {
                (Some(Drag::Tab(from)), Some(pos)) if from < n => {
                    let others: Vec<usize> = (0..n).filter(|&i| i != from).collect();
                    let along = if s.side { pos.y } else { pos.x };
                    let to = others.iter().filter(|&&i| Layout::along(l.slots[i], s.side) < along).count();
                    draw_mark(g, end_of(&others, to));
                }
                (Some(Drag::Group(group)), Some(pos)) => {
                    let others: Vec<usize> = (0..n).filter(|&i| l.groups_of[i] != Some(group)).collect();
                    let along = if s.side { pos.y } else { pos.x };
                    let near = others.iter().filter(|&&i| Layout::along(l.slots[i], s.side) < along).count();
                    draw_mark(g, end_of(&others, group_place(&l.groups_of, group, near)));
                }
                _ => {}
            }
            if let Some(edge) = l.edge.filter(|_| matches!(under, Some(Part::Edge)) || drag == Some(Drag::Edge)) {
                g.set_cursor_style(CursorStyle::ResizeLeftRight, if drag == Some(Drag::Edge) { full.dilate(px(4096.0)) } else { edge });
            }
            // Agents: each tab, its close box, the chips, New Tab.
            for (i, tab) in s.tabs.iter().enumerate() {
                let group = l.groups_of[i].and_then(|group| s.groups.get(group));
                g.agent(agent::Node { role: "Tab", name: Some(tab_name(tab, group)), bounds: l.slots[i], enabled: true, clickable: true, selected: Some(i == s.active), ..Default::default() });
                g.agent(agent::Node::button(format!("Close {}", tab.label), l.closes[i]));
            }
            for &(group, _, r) in &l.chips {
                // (Down the side, the chip as it shows: its heading takes a click.)
                let r = if s.side { side_chip(g, r, &s.groups[group].name) } else { r };
                let chosen = s.chosen.as_ref().map(|c| c.get(group).copied().unwrap_or(false));
                g.agent(agent::Node {
                    role: "Button",
                    name: Some(format!("Group {}", s.groups[group].name).into()),
                    bounds: r,
                    enabled: true,
                    clickable: true,
                    selected: chosen,
                    ..Default::default()
                });
            }
            if let Some(r) = l.new {
                let r = if s.side { side_new_tab(r) } else { r };
                g.agent(agent::Node::button("New Tab", r));
            }
            if let Some(r) = l.edge {
                g.agent(agent::Node { role: "Splitter", name: Some("Tab Width".into()), bounds: r, enabled: true, ..Default::default() });
            }
        })
        .id(id)
        .size_full();

        // The pointer coming to a tab (its close box, a chip, +) or
        // leaving it: drawn again, lit or not. And, a press held, what it
        // has become: a tab or a group dragged along the strip, or out of
        // it; the side strip's edge.
        let (s, act) = (me.clone(), me.on_action.clone());
        element = element.on_window_mouse_move(move |e: &MouseMoveEvent, cx| {
            let (l, press, drag, changed) = {
                let st = cx.node_state::<StripState>();
                let Some(l) = st.laid.clone() else { return };
                st.at = Some(e.position);
                let under = l.part_at(e.position);
                let changed = st.drag.is_none() && under != st.under;
                st.under = under;
                (l, st.press, st.drag, changed)
            };
            if changed {
                cx.repaint();
            }
            let Some(act) = &act else { return };
            let Some((part, MouseButton::Left, from)) = press else { return };
            if e.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let n = s.tabs.len();
            let drag = drag.or_else(|| {
                let d = e.position - from;
                let far = (f32::from(d.x).powi(2) + f32::from(d.y).powi(2)).sqrt() > DRAG;
                match part {
                    Part::Tab(i) if far && n > 1 => Some(Drag::Tab(i)),
                    Part::Chip(group) if far && l.groups_of.iter().any(|x| *x != Some(group)) => Some(Drag::Group(group)),
                    _ => None,
                }
            });
            let Some(drag) = drag else { return };
            // Out of the strip, well beyond it or out of the window: torn
            // off, to be a window of its own.
            let full = l.full;
            let screen = Bounds { origin: point(px(0.0), px(0.0)), size: cx.viewport_size() };
            let pos = e.position;
            let out = !screen.contains(&pos) || if s.side { pos.x > full.right() + px(TEAR) } else { pos.y < full.top() - px(TEAR) || pos.y > full.bottom() + px(TEAR) };
            match drag {
                Drag::Edge => {
                    // On the 2-point grid: the edge on a device pixel at 1.5×.
                    let width = ((f32::from(pos.x - full.left()) / 2.0).round() * 2.0).clamp(STRIP_SIDE_MIN, STRIP_SIDE_MAX);
                    act(&StripAction::Resize { width, done: false }, cx);
                }
                Drag::Tab(tab) if out => {
                    cx.node_state::<StripState>().let_go();
                    act(&StripAction::TearOff { tab, at: pos }, cx);
                }
                Drag::Group(group) if out => {
                    cx.node_state::<StripState>().let_go();
                    act(&StripAction::TearOffGroup { group, at: pos }, cx);
                }
                _ => {}
            }
            if !out {
                let st = cx.node_state::<StripState>();
                st.drag = Some(drag);
                st.pointer = Some(pos);
                cx.repaint();
            }
        });
        // The pointer gone from it (off the window, onto a menu over it):
        // nothing of it is under the pointer any more.
        element = element.on_hover(|over: &bool, cx| {
            if !*over {
                let st = cx.node_state::<StripState>();
                st.at = None;
                if st.under.take().is_some() && st.drag.is_none() {
                    cx.repaint();
                }
            }
        });
        let Some(act) = me.on_action.clone() else { return element };
        let s = me;
        element
            // A press: what it's on, for its release (or a drag).
            .on_any_mouse_down(|e: &MouseDownEvent, cx| {
                let st = cx.node_state::<StripState>();
                let Some(part) = st.laid.as_ref().and_then(|l| l.part_at(e.position)) else { return };
                st.press = Some((part, e.button, e.position));
                st.drag = (part == Part::Edge && e.button == MouseButton::Left).then_some(Drag::Edge);
                cx.stop_propagation();
                cx.repaint();
            })
            .on_window_mouse_up(move |e: &MouseUpEvent, cx| {
                let (l, part, drag) = {
                    let st = cx.node_state::<StripState>();
                    let (Some(l), Some((part, button, _))) = (st.laid.clone(), st.press) else { return };
                    if button != e.button {
                        return;
                    }
                    let drag = st.drag;
                    st.let_go();
                    (l, part, drag)
                };
                cx.repaint();
                let n = s.tabs.len();
                let pos = e.position;
                let along = if s.side { pos.y } else { pos.x };
                let action = match drag {
                    Some(Drag::Edge) => {
                        let width = ((f32::from(pos.x - l.full.left()) / 2.0).round() * 2.0).clamp(STRIP_SIDE_MIN, STRIP_SIDE_MAX);
                        Some(StripAction::Resize { width, done: true })
                    }
                    Some(Drag::Tab(from)) if from < n => {
                        let others: Vec<usize> = (0..n).filter(|&i| i != from).collect();
                        let to = others.iter().filter(|&&i| Layout::along(l.slots[i], s.side) < along).count();
                        let edge = others.get(to).map_or(l.end, |&i| l.before(i, s.side));
                        let group = dropped_into(&l.groups_of, from, to, along < edge);
                        (to != from || group != l.groups_of[from]).then_some(StripAction::Move { from, to, group })
                    }
                    Some(Drag::Group(group)) => {
                        let others: Vec<usize> = (0..n).filter(|&i| l.groups_of[i] != Some(group)).collect();
                        let near = others.iter().filter(|&&i| Layout::along(l.slots[i], s.side) < along).count();
                        let to = group_place(&l.groups_of, group, near);
                        let first = l.groups_of.iter().position(|x| *x == Some(group)).unwrap_or(0);
                        (to != others.iter().filter(|&&i| i < first).count()).then_some(StripAction::MoveGroup { group, to })
                    }
                    Some(_) => None,
                    // A click: on what it was pressed on, still.
                    None if l.part_at(pos) != Some(part) => None,
                    None => match (part, e.button) {
                        (Part::Close(i), MouseButton::Left) | (Part::Tab(i), MouseButton::Middle) => Some(StripAction::Close(i)),
                        (Part::Tab(i), MouseButton::Right) => Some(StripAction::Menu(i, pos)),
                        (Part::Tab(i), MouseButton::Left) => Some(StripAction::Select(i)),
                        (Part::Chip(group), MouseButton::Left) if s.chosen.is_some() => Some(StripAction::ToggleGroup(group)),
                        (Part::Chip(group), MouseButton::Left | MouseButton::Right) => Some(StripAction::Group(group, pos)),
                        (Part::New, MouseButton::Left) => Some(StripAction::New),
                        _ => None,
                    },
                };
                if let Some(a) = action {
                    act(&a, cx);
                }
            })
    }
}

/// How wide each tab is: as its title needs (`natural`) if they all fit
/// in `room`; else the widest cut back alike till they do, none below
/// `min` (all alike, then, narrower still if they must). On the 2-point
/// grid, so every tab's edges are on device pixels at 1.5× too.
fn fit_widths(natural: &[f32], room: f32, min: f32) -> Vec<f32> {
    let grid = |w: f32| (w / 2.0).floor().max(1.0) * 2.0;
    let n = natural.len();
    // (Up to the grid, where they fit: down, a title would lose a
    // fraction of a point and be cut short.)
    let up: Vec<f32> = natural.iter().map(|&w| (w / 2.0).ceil() * 2.0).collect();
    if up.iter().sum::<f32>() <= room {
        return up;
    }
    if n == 0 || min * n as f32 >= room {
        return vec![grid(room / n.max(1) as f32); n];
    }
    // The cap the widest are cut to: those under it keep their width,
    // the rest share what's left.
    let mut sorted = natural.to_vec();
    sorted.sort_by(f32::total_cmp);
    let (mut left, mut cap) = (room, 0.0);
    for (k, &w) in sorted.iter().enumerate() {
        let share = left / (n - k) as f32;
        if w > share {
            cap = share;
            break;
        }
        left -= w;
    }
    natural.iter().map(|&w| grid(w.min(cap))).collect()
}

/// The colours a strip is painted in, top or side.
struct Look {
    dark: bool,
    band: Hsla,
    /// The line between the strip and the content.
    edge: Hsla,
    /// The selected tab's plate, its rim and its lit top.
    raised: Hsla,
    rim: Hsla,
    shine: Hsla,
    shadow: Hsla,
    /// A plate under the pointer.
    hover: Hsla,
    /// The faint lines between tabs.
    divider: Hsla,
    ink: Hsla,
    dim: Hsla,
}

impl Look {
    fn of() -> Look {
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let bg = style::c(p.panel_background);
        let band = style::shade(&p, p.panel_background, 1.06);
        let white = white();
        // (In the dark a lighter edge: a darker one would be lost.)
        let edge = if dark { lerp(bg, white, 0.14) } else { style::shade(&p, p.panel_background, 1.22) };
        let ink = style::c(p.panel_text);
        Look {
            dark,
            band,
            edge,
            raised: if dark { lerp(bg, white, 0.09) } else { lerp(bg, white, 0.82) },
            rim: if dark { white.opacity(0.07) } else { black().opacity(0.09) },
            shine: if dark { white.opacity(0.08) } else { white.opacity(0.9) },
            shadow: black().opacity(if dark { 0.42 } else { 0.13 }),
            hover: if dark { lerp(band, white, 0.06) } else { lerp(band, white, 0.42) },
            divider: lerp(edge, band, 0.35),
            ink,
            dim: lerp(ink, band, 0.32),
        }
    }

    /// The fill of a group's capsule.
    fn capsule(&self, color: Hsla) -> Hsla {
        color.opacity(if self.dark { 0.22 } else { 0.16 })
    }

    /// A raised plate: shadowed, rimmed, lit along its top.
    fn raise(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let line = style::hairline(g);
        let radius = px(TAB_RADIUS);
        let shadows = [
            BoxShadow { color: self.shadow, offset: point(px(0.0), px(1.0)), blur_radius: px(3.0), spread_radius: px(0.0) },
            BoxShadow { color: self.shadow.opacity(self.shadow.a * 0.4), offset: point(px(0.0), px(0.0)), blur_radius: px(1.0), spread_radius: px(0.0) },
        ];
        g.paint_shadows(b, Corners::all(radius), &shadows);
        // A touch lighter at the top, the rim lit along it (tapering
        // into the corners, as a top border does), as BeOS's controls
        // are lit from above.
        let fill = linear_gradient(180.0, linear_color_stop(lerp(self.raised, white(), if self.dark { 0.035 } else { 0.6 }), 0.0), linear_color_stop(self.raised, 1.0));
        g.paint_quad(quad(b, radius, fill, line, self.rim, BorderStyle::Solid));
        let top = Edges { top: line, ..Default::default() };
        g.paint_quad(quad(b, radius, transparent_black(), top, self.shine, BorderStyle::Solid));
    }

    fn inks(&self) -> FaceInks {
        FaceInks { fill: self.raised, ink: self.ink, dim: self.dim, hover: self.hover }
    }
}

/// Tab `r`'s plate, centred on `cy`.
fn plate(g: &Painter, r: Bounds<Pixels>, cy: Pixels) -> Bounds<Pixels> {
    style::snap(g, Bounds::from_corners(point(r.left() + px(PLATE_GAP), cy - px(PLATE / 2.0)), point(r.right() - px(PLATE_GAP), cy + px(PLATE / 2.0))))
}

/// The tabs in the group that tab `first` leads, to the last.
fn last_of(l: &Layout, g: usize, first: usize) -> usize {
    (first..l.slots.len()).take_while(|&i| l.groups_of[i] == Some(g)).last().unwrap_or(first)
}

/// Along the top: the tabs on a quiet band, each its icon and title; the
/// selected one on a raised plate, lit along its top with a soft shadow
/// under it; one under the pointer on a faint plate; a faint line between
/// the others. A group is a capsule of its colour round its chip (its
/// name on a pill of the colour) and its tabs. Everything sits inside the
/// band, a line under it. A tab being dragged (`lift`: it, and how far)
/// rides on a raised plate under the pointer, its slot left empty.
#[allow(clippy::too_many_arguments)]
fn paint_top(g: &mut Painter, s: &TabStrip, full: Bounds<Pixels>, l: &Layout, under: Option<Part>, drag: Option<Drag>, lift: Option<(usize, Pixels)>) {
    let look = Look::of();
    let n = s.tabs.len();
    style::fill_rect(g, full, look.band);
    style::hline_above(g, full.left(), full.right(), full.bottom(), look.edge);
    let lifted = |i: usize| lift.is_some_and(|(t, _)| t == i);
    let lit = |i: usize| i == s.active || lifted(i) || matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i);
    for &(group, first, chip_r) in &l.chips {
        let last = last_of(l, group, first);
        let reach = px(PLATE / 2.0 + HALO);
        let cap = Bounds::from_corners(point(chip_r.left() - px(CHIP_INSET), l.cy - reach), point(l.slots[last].right(), l.cy + reach));
        g.paint_quad(fill(style::snap(g, cap), look.capsule(s.groups[group].color)).corner_radii(px(CAPSULE_RADIUS)));
    }
    // Faint lines between tabs that aren't lit (none where a capsule
    // begins or ends: its edge parts them).
    for i in 0..n.saturating_sub(1) {
        if !lit(i) && !lit(i + 1) && l.groups_of[i] == l.groups_of[i + 1] {
            style::vline_left_of(g, l.slots[i].right() + style::hairline(g) / 2.0, l.cy - px(7.0), l.cy + px(7.0), look.divider);
        }
    }
    for i in (0..n).filter(|&i| i != s.active && !lifted(i) && lit(i)) {
        g.paint_quad(fill(plate(g, l.slots[i], l.cy), look.hover).corner_radii(px(TAB_RADIUS)));
    }
    for &(group, _, chip_r) in &l.chips {
        let StripGroup { name, color } = s.groups[group].clone();
        let chosen = s.chosen.as_ref().map(|c| c.get(group).copied().unwrap_or(false));
        let hovered = under == Some(Part::Chip(group)) || drag == Some(Drag::Group(group));
        chip(g, chip_r, &name, color, chosen != Some(false), hovered);
    }
    if s.active < n && !lifted(s.active) {
        look.raise(g, plate(g, l.slots[s.active], l.cy));
    }
    let inks = look.inks();
    for (i, tab) in s.tabs.iter().enumerate().filter(|&(i, _)| !lifted(i)) {
        let r = l.slots[i];
        let pointed = matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i);
        {
            let on = plate(g, r, l.cy);
            crate::review::own_grid(g, on);
        }
        tab_face(g, tab, i == s.active, (pointed, under == Some(Part::Close(i))), (r, l.cy, l.closes[i]), r.size.width >= px(72.0), inks);
    }
    if let Some(plus) = l.new {
        let color = if under == Some(Part::New) {
            g.paint_quad(fill(plus, look.hover).corner_radii(px(NEW_SIZE / 2.0)));
            look.ink
        } else {
            look.dim
        };
        plus_mark(g, plus.center(), color);
    }
    if let Some((i, d)) = lift.filter(|&(i, _)| i < n) {
        // Kept within the strip, on device pixels.
        let r = l.slots[i];
        let d = style::snap_coord(g, d.clamp(full.left() - r.left(), full.right() - r.right()));
        let r = Bounds { origin: point(r.left() + d, r.top()), ..r };
        let close = Bounds { origin: point(l.closes[i].left() + d, l.closes[i].top()), ..l.closes[i] };
        look.raise(g, plate(g, r, l.cy));
        tab_face(g, &s.tabs[i], true, (false, false), (r, l.cy, close), r.size.width >= px(72.0), inks);
    }
}

/// Down the side: rows on the band, the one showing on a raised plate,
/// one under the pointer on a faint one; a group's chip heads its rows,
/// a capsule of its colour round them. A row being dragged rides on a
/// raised plate under the pointer, as along the top.
#[allow(clippy::too_many_arguments)]
fn paint_side(g: &mut Painter, s: &TabStrip, full: Bounds<Pixels>, l: &Layout, under: Option<Part>, drag: Option<Drag>, lift: Option<(usize, Pixels)>) {
    let look = Look::of();
    let lifted = |i: usize| lift.is_some_and(|(t, _)| t == i);
    let line = style::hairline(g);
    style::fill_rect(g, full, look.band);
    let n = s.tabs.len();
    // (Its icons, and New Tab's +, under the menu bar's first title.)
    let face = |r: Bounds<Pixels>| Bounds::from_corners(point(r.left() + px(SIDE_FACE), r.top()), point(r.right() - px(m::TIGHT), r.bottom()));
    let plate = |g: &Painter, r: Bounds<Pixels>| style::snap(g, Bounds::from_corners(point(r.left() + px(m::TIGHT), r.top() + px(1.0)), point(r.right() - px(m::TIGHT), r.bottom() - px(1.0))));
    let mid = |g: &Painter, r: Bounds<Pixels>| style::snap_coord(g, r.center().y.round());
    for &(group, first, head) in &l.chips {
        let last = last_of(l, group, first);
        let cap = Bounds::from_corners(point(full.left() + px(m::TIGHT - HALO), head.top() + px(1.0)), point(full.right() - line - px(m::TIGHT - HALO), l.slots[last].bottom()));
        g.paint_quad(fill(style::snap(g, cap), look.capsule(s.groups[group].color)).corner_radii(px(CAPSULE_RADIUS)));
    }
    for i in (0..n).filter(|&i| !lifted(i)) {
        let r = l.slots[i];
        if i == s.active {
            look.raise(g, plate(g, r));
        } else if matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i) {
            g.paint_quad(fill(plate(g, r), look.hover).corner_radii(px(TAB_RADIUS)));
        }
    }
    style::vline_left_of(g, full.right(), full.top(), full.bottom(), look.edge);
    for &(group, _, head) in &l.chips {
        let StripGroup { name, color } = s.groups[group].clone();
        let chosen = s.chosen.as_ref().map(|c| c.get(group).copied().unwrap_or(false));
        let hovered = under == Some(Part::Chip(group)) || drag == Some(Drag::Group(group));
        let c = side_chip(g, head, &name);
        chip(g, c, &name, color, chosen != Some(false), hovered);
    }
    let inks = look.inks();
    for (i, tab) in s.tabs.iter().enumerate().filter(|&(i, _)| !lifted(i)) {
        let r = l.slots[i];
        let pointed = matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i);
        let y = mid(g, r);
        tab_face(g, tab, i == s.active, (pointed, under == Some(Part::Close(i))), (face(r), y, l.closes[i]), r.size.width >= px(72.0), inks);
    }
    if let Some(r) = l.new {
        let color = if under == Some(Part::New) {
            g.paint_quad(fill(plate(g, r), look.hover).corner_radii(px(TAB_RADIUS)));
            look.ink
        } else {
            look.dim
        };
        // The + where the tabs' icons are, then what it does where their
        // titles are.
        let f = face(r);
        let y = mid(g, r);
        plus_mark(g, point(f.left() + px(m::SPACING + 8.0), y), color);
        let text = f.left() + px(m::SPACING + 16.0 + m::MARK_GAP);
        style::label_fit(g, text, y, style::Align::Left, "New Tab", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, f.right() - px(m::TIGHT) - text);
    }
    if let Some((i, d)) = lift.filter(|&(i, _)| i < n) {
        let r = l.slots[i];
        let d = style::snap_coord(g, d.clamp(full.top() - r.top(), full.bottom() - r.bottom()));
        let r = Bounds { origin: point(r.left(), r.top() + d), ..r };
        let close = Bounds { origin: point(l.closes[i].left(), l.closes[i].top() + d), ..l.closes[i] };
        look.raise(g, plate(g, r));
        tab_face(g, &s.tabs[i], true, (false, false), (face(r), mid(g, r), close), r.size.width >= px(72.0), inks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_take_what_their_titles_need_and_the_widest_give_way() {
        // Room for all: each as it needs.
        assert_eq!(fit_widths(&[200.0, 140.0, 140.0], 600.0, 140.0), vec![200.0, 140.0, 140.0]);
        // Up to the grid, so a title isn't a fraction too wide for its tab.
        assert_eq!(fit_widths(&[238.06, 158.18], 600.0, 140.0), vec![240.0, 160.0]);
        // Not: the widest cut back, the others as they were.
        assert_eq!(fit_widths(&[300.0, 140.0, 140.0], 500.0, 140.0), vec![220.0, 140.0, 140.0]);
        assert_eq!(fit_widths(&[300.0, 260.0, 140.0], 600.0, 140.0), vec![230.0, 230.0, 140.0]);
        // Not even at the narrowest: all alike.
        assert_eq!(fit_widths(&[300.0, 140.0, 140.0], 300.0, 140.0), vec![100.0, 100.0, 100.0]);
    }

    #[test]
    fn a_tab_dropped_among_a_groups_joins_it() {
        let g = [None, Some(0), Some(0), None];
        // Tab 3 put between tabs 1 and 2: in group 0.
        assert_eq!(dropped_into(&g, 3, 2, true), Some(0));
        // Tab 1 dragged past the group's end, over the tab after: out.
        assert_eq!(dropped_into(&g, 1, 3, false), None);
    }

    #[test]
    fn a_group_goes_between_groups() {
        let g = [Some(0), Some(0), Some(1), Some(1), None];
        assert_eq!(group_place(&g, 1, 1), 0);
        assert_eq!(group_place(&g, 0, 2), 2);
    }
}
