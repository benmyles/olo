//! A row of document tabs under a window's menu bar (Terminal's shells):
//! folder tabs (Ben, 2026-11-14: "folder tabs -> labels"): BeOS's
//! slanted ones, or Platinum's rounded ones, on the panel; the chosen
//! one in the content's colours ([`TabStrip::content`]) and open onto
//! it, the others a shade off the panel behind the line along the foot,
//! lighter under the pointer; each with its × while chosen or under the
//! pointer (middle-click closes too), a + after the last, tabs dragged
//! along the row to reorder them and out of it to tear them off into a
//! window of their own. A group of tabs is led by a label of its colour
//! (its look's: `Palette::group_colors`) with its name in capitals; a
//! gap between tabs, more between groups. Titles too long for their tab
//! are cut short with "…". Or, [`TabStrip::side`], a column of them down
//! the content's left side, the chosen one a folder tab on its side.
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
//! it draws it again only when what it's over changes. Its shapes are
//! drawn on device pixels, their slants antialiased, as the window's
//! tab is.

use std::{
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use olo_interface::{
    Bounds, CursorStyle, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, Pixels, Point, Rgba, SharedString,
    Styled as _, WindowCx, agent, black, canvas, fill, point, px, size, white,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// Paints a mark into its rectangle (a tab's icon, its badge).
pub type PaintFn = Rc<dyn Fn(&mut Painter, Bounds<Pixels>)>;

thread_local! {
    static OVER: std::cell::Cell<Option<Hsla>> = const { std::cell::Cell::new(None) };
}

/// What a tab's icon or badge is being drawn over, while the strip has
/// its painter paint it: the tab's own fill (the content's on the chosen
/// tab, a shade off the panel behind, lighter under the pointer). For a
/// painter that sets a mark off from the icon under it with a rim of
/// that colour: a rim in the panel's own showed as a disc on the chosen
/// tab. None when it isn't the strip that calls the painter.
pub fn drawn_over() -> Option<Hsla> {
    OVER.with(std::cell::Cell::get)
}

/// `paint` called with [`drawn_over`] saying `fill`.
fn over(fill: Hsla, paint: impl FnOnce()) {
    let before = OVER.with(|o| o.replace(Some(fill)));
    paint();
    OVER.with(|o| o.set(before));
}

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
    /// Something unread in it, as the window's tab and Deskbar's red dot
    /// say (`crate::unread`): a red dot after its title; `Some(Some(t))`,
    /// looked at since `t`, fading out over [`STRIP_DOT_FADE`].
    pub unread: Option<Option<std::time::Instant>>,
}

/// How long a tab's red dot takes to fade once the tab is looked at (as
/// a window tab's does: app_server's `UNREAD_FADE`).
pub const STRIP_DOT_FADE: std::time::Duration = std::time::Duration::from_secs(5);

/// A tab's red dot's size, and its gap from the title.
const STRIP_DOT: f32 = 7.0;
const STRIP_DOT_GAP: f32 = 5.0;

/// How strong a tab's red dot is now (`StripTab::unread`): whole while
/// unread, easing out over [`STRIP_DOT_FADE`] once looked at; 0 after.
pub fn dot_strength(unread: Option<Option<std::time::Instant>>) -> f32 {
    match unread {
        None => 0.0,
        Some(None) => 1.0,
        Some(Some(since)) => {
            let t = since.elapsed().as_secs_f32() / STRIP_DOT_FADE.as_secs_f32();
            if t >= 1.0 {
                return 0.0;
            }
            let left = 1.0 - t;
            left * left * (3.0 - 2.0 * left)
        }
    }
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
/// How tall a strip is: its folder tabs and room over them, as a row of
/// documents is something you aim at all day.
pub const STRIP_HEIGHT: f32 = 34.0;
/// How tall a band of heads under a strip is (Terminal's panes' in Tile
/// All Tabs).
pub const STRIP_ROW: f32 = 26.0;
/// Where a tab's top is below the strip's: a tab is the rest of it, its
/// bottom open onto the content.
const TAB_TOP: f32 = 8.0;
/// The caret marking where a tab dragged would go: as wide, and as deep
/// (in the band above the tabs, or beside the rows).
const MARK_WIDE: f32 = 10.0;
const MARK_DEEP: f32 = 6.0;
/// The first tab's room from the strip's left, and between groups (and
/// a group and the tabs in none).
const TAB_LEFT: f32 = 10.0;
const GROUP_GAP: f32 = 14.0;
/// A tab's narrowest and widest: each as wide as its title needs between
/// these (the widest cut back when they don't all fit).
const TAB_MIN: f32 = 140.0;
const TAB_MAX: f32 = 250.0;
/// The close button's hit area, and the × in it.
const CLOSE: f32 = 16.0;
const CROSS: f32 = 7.0;
/// The + button, and its centre past the last tab.
const NEW_SIZE: f32 = 20.0;
const NEW_AFTER: f32 = 18.0;
/// A group's label (a pane head's, [`chip`]): its height, text size and
/// room either side of its name.
pub const CHIP_HEIGHT: f32 = 18.0;
const CHIP_SIZE: f32 = style::SMALL_SIZE;
/// A label's name is spaced out a little, as small bold capitals are.
const LABEL_SPACING: f32 = 0.6;
const CHIP_PAD: f32 = 8.0;
/// How tall a tab is in a side strip (less, to fit, when there are many),
/// and the room under it.
pub const STRIP_SIDE_TAB: f32 = 28.0;
const SIDE_TAB_GAP: f32 = 1.0;
/// How big a tab's badge is: as the text is.
pub const STRIP_BADGE: f32 = 12.0;
/// How strongly a [`StripTab::faded`] tab's icon and title are drawn.
pub const STRIP_FADED: f32 = 0.45;
/// How tall a band beside a side strip is (Terminal's pane heads); the
/// strip's first row is centred on it. As tall as a strip along the top.
pub const STRIP_SIDE_HEAD: f32 = STRIP_HEIGHT;
/// How tall a group's label is in a side strip, the room under it, and
/// the room between groups.
pub const STRIP_SIDE_HEADING: f32 = 22.0;
const SIDE_LABEL_GAP: f32 = 2.0;
const SIDE_GROUP_GAP: f32 = 8.0;
/// A side strip's rows' left, the tabs' faces inside them, and the
/// label's text in it.
const SIDE_LEFT: f32 = 8.0;
const SIDE_FACE: f32 = 10.0;
const SIDE_LABEL_TEXT: f32 = 12.0;
/// How wide a side strip starts, and the narrowest and widest its edge
/// can be dragged to.
pub const STRIP_SIDE_WIDTH: f32 = 204.0;
pub const STRIP_SIDE_MIN: f32 = 120.0;
pub const STRIP_SIDE_MAX: f32 = 400.0;

/// The look's tabs: BeOS's slanted folder tabs, or Platinum's rounded
/// ones; how far a title is from a tab's sides, the gap between tabs,
/// and a label's room either side of its name.
#[derive(Clone, Copy)]
pub(crate) struct Shape {
    rounded: bool,
    slant: f32,
    radius: f32,
    pad: f32,
    pub(crate) gap: f32,
    pub(crate) label_pad: f32,
}

impl Shape {
    pub(crate) fn now() -> Shape {
        if theme::palette().theme == "platinum" {
            Shape { rounded: true, slant: 0.0, radius: 5.0, pad: 12.0, gap: 3.0, label_pad: 10.0 }
        } else {
            Shape { rounded: false, slant: 8.0, radius: 0.0, pad: 17.0, gap: 2.0, label_pad: 16.0 }
        }
    }

    /// A text-only tab's room either side of its name (a tab view's):
    /// what it likes, and the least it can do with.
    pub(crate) fn name_pads(self) -> (f32, f32) {
        (self.label_pad + 4.0, if self.rounded { 6.0 } else { self.slant + 4.0 })
    }

    /// What a tab has besides its title: its pads, icon and the gap after
    /// it, the gap before the close button, the button and a little.
    fn chrome(self) -> f32 {
        2.0 * self.pad + 16.0 + 8.0 + 8.0 + 12.0 + 2.0
    }
}

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
    /// The content's fill and text: the chosen tab's, which opens onto it.
    content: Option<(Hsla, Hsla)>,
    /// The chosen tab doesn't open onto what's under the strip.
    closed: bool,
    /// The window it's in, as app_server knows it (its surface).
    window: Option<u32>,
    on_action: Option<ActionFn>,
}

impl TabStrip {
    pub fn new(id: impl Into<SharedString>, tabs: Vec<StripTab>, active: usize) -> TabStrip {
        TabStrip { id: id.into(), tabs, active, max_width: f32::INFINITY, new_tab: true, groups: Vec::new(), chosen: None, side: false, content: None, closed: false, window: None, on_action: None }
    }

    /// The colours of what the tabs show (Terminal's ground and text):
    /// the chosen tab is a folder tab of them, open onto it. (Else a
    /// document's.)
    pub fn content(mut self, fill: Hsla, ink: Hsla) -> Self {
        self.content = Some((fill, ink));
        self
    }

    /// What's under the strip isn't the chosen tab's (Terminal's Tile
    /// All Tabs: every tab's pane): the chosen tab stays closed, the line
    /// along the strip's foot (or down its edge) running under it.
    pub fn closed(mut self) -> Self {
        self.closed = true;
        self
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
            Some(c) => c.2.left() - px(GROUP_GAP / 2.0),
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

/// A strip's +, ten points across, its arms a point and a half thick
/// ([`style::plus_mark`]).
fn plus_mark(g: &mut Painter, center: Point<Pixels>, color: Hsla) {
    style::plus_mark(g, center, 10.0, 1.5, color);
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
    /// Where everything goes along the top of `full`: each group's label,
    /// then its tabs, a gap between tabs and more between groups; each
    /// tab as wide as its title needs (within [`TAB_MIN`]–[`TAB_MAX`]),
    /// the widest cut back when they don't all fit ([`fit_widths`]).
    fn layout_top(&self, g: &Painter, full: Bounds<Pixels>) -> Layout {
        let sh = Shape::now();
        let n = self.tabs.len();
        let groups_of: Vec<Option<usize>> = self.tabs.iter().map(|t| t.group.filter(|&g| g < self.groups.len())).collect();
        let leads = |i: usize| groups_of[i].is_some() && (i == 0 || groups_of[i - 1] != groups_of[i]);
        let top = f32::from(full.top()) + TAB_TOP;
        let cy = ((top + f32::from(full.bottom())) / 2.0).round();
        let label_w = |group: usize| label_width(&self.groups[group].name, sh);
        // (What isn't a tab: the labels, and the gaps between everything.)
        let gap = |prev: Option<Option<usize>>, now: Option<usize>| match prev {
            None => 0.0,
            Some(p) if p != now => GROUP_GAP,
            Some(_) => sh.gap,
        };
        let mut fixed = 0.0;
        let mut prev = None;
        for (i, &group) in groups_of.iter().enumerate() {
            if leads(i) {
                fixed += gap(prev, group) + label_w(group.unwrap_or(0));
                prev = Some(group);
            }
            fixed += gap(prev, group);
            prev = Some(group);
        }
        // (Room after the last tab for its + and the strip's end.)
        let new_w = if self.new_tab { NEW_AFTER + NEW_SIZE / 2.0 + 12.0 } else { m::TIGHT };
        let bold = style::ui_font(FontWeight::BOLD);
        let natural: Vec<f32> =
            self.tabs.iter().map(|t| (f32::from(style::text_width(t.label.clone(), style::PLAIN_SIZE, bold.clone())) + sh.chrome()).clamp(TAB_MIN, TAB_MAX.min(self.max_width).max(TAB_MIN))).collect();
        let room = f32::from(full.size.width) - TAB_LEFT - new_w - fixed;
        let widths = fit_widths(&natural, room, TAB_MIN.min(sh.chrome() + 40.0));
        let mut chips = Vec::new();
        let mut slots = Vec::with_capacity(n);
        let mut x = f32::from(full.left()) + TAB_LEFT;
        let mut prev = None;
        for i in 0..n {
            if let Some(group) = groups_of[i].filter(|_| leads(i)) {
                x += gap(prev, groups_of[i]);
                let w = label_w(group);
                chips.push((group, i, Bounds::from_corners(point(px(x), px(top)), point(px(x + w), full.bottom()))));
                x += w;
                prev = Some(groups_of[i]);
            }
            x += gap(prev, groups_of[i]);
            prev = Some(groups_of[i]);
            slots.push(Bounds::from_corners(point(px(x), px(top)), point(px(x + widths[i]), full.bottom())));
            x += widths[i];
        }
        let end = px(x);
        let closes = slots.iter().map(|r| style::snap_centred(g, point(r.right() - px(sh.pad + CLOSE / 2.0 - 2.0), px(cy)), CLOSE, CLOSE)).collect();
        let new = self.new_tab.then(|| style::snap_centred(g, point(end + px(NEW_AFTER), px(cy)), NEW_SIZE, NEW_SIZE));
        Layout { full, slots, chips, closes, new, edge: None, end, groups_of, cy: px(cy) }
    }

    /// Where everything goes down `full`: each group's label (as wide as
    /// its name), then its rows; a gap between groups.
    fn layout_side(&self, g: &Painter, full: Bounds<Pixels>) -> Layout {
        let sh = Shape::now();
        let n = self.tabs.len();
        let groups_of: Vec<Option<usize>> = self.tabs.iter().map(|t| t.group.filter(|&g| g < self.groups.len())).collect();
        let leads = |i: usize| groups_of[i].is_some() && (i == 0 || groups_of[i - 1] != groups_of[i]);
        let parts = |i: usize| i > 0 && groups_of[i] != groups_of[i - 1];
        let headings = (0..n).filter(|&i| leads(i)).count() as f32;
        let breaks = (0..n).filter(|&i| parts(i)).count() as f32;
        let (left, right) = (full.left(), full.right());
        let rows = (n + usize::from(self.new_tab)) as f32;
        let fixed = headings * (STRIP_SIDE_HEADING + SIDE_LABEL_GAP) + breaks * SIDE_GROUP_GAP;
        let height = f32::from(full.size.height);
        let natural = fixed + rows * (STRIP_SIDE_TAB + SIDE_TAB_GAP);
        let row_h = if natural <= height { STRIP_SIDE_TAB } else { ((((height - fixed) / rows.max(1.0)) - SIDE_TAB_GAP) / 2.0).floor().max(8.0) * 2.0 };
        let first_h = if n > 0 && leads(0) { STRIP_SIDE_HEADING } else { row_h };
        let mut y = f32::from(full.top()) + ((STRIP_SIDE_HEAD - first_h) / 2.0).max(0.0).floor();
        let mut chips = Vec::new();
        let mut slots = Vec::with_capacity(n);
        let lx = left + px(SIDE_LEFT);
        for (i, of) in groups_of.iter().enumerate() {
            if parts(i) {
                y += SIDE_GROUP_GAP;
            }
            if let Some(group) = of.filter(|_| leads(i)) {
                let w = side_label_width(&self.groups[group].name, sh).min(f32::from(right - lx));
                chips.push((group, i, style::snap(g, Bounds { origin: point(lx, px(y)), size: size(px(w), px(STRIP_SIDE_HEADING)) })));
                y += STRIP_SIDE_HEADING + SIDE_LABEL_GAP;
            }
            slots.push(style::snap(g, Bounds::from_corners(point(left, px(y)), point(right, px(y + row_h)))));
            y += row_h + SIDE_TAB_GAP;
        }
        let end_row = style::snap(g, Bounds::from_corners(point(left, px(y + 2.0)), point(right, px(y + 2.0 + row_h))));
        let closes = slots.iter().map(|r| style::snap_centred(g, point(r.right() - px(14.0 + CLOSE / 2.0 - 2.0), r.center().y.round()), CLOSE, CLOSE)).collect();
        let edge = Some(Bounds::from_corners(point(full.right() - px(4.0), full.top()), full.bottom_right()));
        Layout { full, slots, chips, closes, new: self.new_tab.then_some(end_row), edge, end: end_row.top(), groups_of, cy: full.top() }
    }
}

/// A side strip's group label as agents find it: where it's laid out.
fn side_chip(_g: &Painter, head: Bounds<Pixels>, _name: &SharedString) -> Bounds<Pixels> {
    head
}

/// A side strip's New Tab in its row `r`, as it shows: its + and its
/// name (the whole row takes a click).
fn side_new_tab(r: Bounds<Pixels>) -> Bounds<Pixels> {
    let text = style::text_width("New Tab", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL));
    let left = r.left() + px(SIDE_LEFT + 6.0);
    Bounds::from_corners(point(left, r.top()), point((left + px(22.0) + text).min(r.right()), r.bottom()))
}

/// A group's name as its label says it: in capitals.
fn label_text(name: &SharedString) -> SharedString {
    name.to_uppercase().into()
}

/// How wide a group's label is along the top: its name and its room,
/// on the 2-point grid.
fn label_width(name: &SharedString, sh: Shape) -> f32 {
    let w = f32::from(style::text_width_spaced(label_text(name), CHIP_SIZE, style::ui_font(FontWeight::BOLD), LABEL_SPACING));
    ((w + 2.0 * sh.label_pad) / 2.0).ceil() * 2.0
}

/// And down the side.
fn side_label_width(name: &SharedString, _sh: Shape) -> f32 {
    let w = f32::from(style::text_width_spaced(label_text(name), CHIP_SIZE, style::ui_font(FontWeight::BOLD), LABEL_SPACING));
    ((w + 2.0 * SIDE_LABEL_TEXT) / 2.0).ceil() * 2.0
}

/// A tab's face: its icon (if `icon` and it has one) at `face` and title
/// after it in `ink` (bold when chosen), its close × while chosen or
/// under the pointer (red under it; a badge in its place otherwise),
/// centred on `cy`; `back`, what it's on.
#[allow(clippy::too_many_arguments)]
fn tab_face(g: &mut Painter, tab: &StripTab, selected: bool, (pointed, on_close): (bool, bool), (face, cy, close): (Pixels, Pixels, Bounds<Pixels>), icon: bool, ink: Hsla, back: Hsla) {
    let badge = tab.badge.clone().filter(|_| icon);
    if pointed || selected && badge.is_none() {
        let color = if on_close { style::c(theme::palette().failure) } else { lerp(ink, back, 0.3) };
        style::close_mark(g, close.center(), CROSS, color);
    }
    let mut end = close.left() - px(m::TIGHT);
    // Its red dot, after the title (the title cut short for it), fading
    // once looked at: drawn again each frame while it does.
    let dot = dot_strength(tab.unread);
    if dot > 0.0 {
        let at = style::snap_centred(g, point(end - px(STRIP_DOT / 2.0), cy), STRIP_DOT, STRIP_DOT);
        let red = style::c(theme::palette().unread).opacity(dot);
        g.paint_quad(olo_interface::fill(at, red).corner_radii(at.size.width / 2.0));
        end -= px(STRIP_DOT + STRIP_DOT_GAP);
        if dot < 1.0 {
            g.paint_again_within(0.0);
        }
    }
    if let Some(paint) = badge.filter(|_| !pointed) {
        let b = style::snap_centred(g, close.center(), STRIP_BADGE, STRIP_BADGE);
        over(back, || paint(g, b));
    }
    // Its icon and title, faded if it's left out of something.
    g.with_element_opacity(tab.faded.as_ref().map(|_| STRIP_FADED), |g| {
        let mut left = face;
        if let Some(paint) = tab.icon.as_ref().filter(|_| icon) {
            let b = style::snap(g, Bounds { origin: point(left, cy - px(8.0)), size: size(px(16.0), px(16.0)) });
            over(back, || paint(g, b));
            left = b.right() + px(8.0);
        }
        // (No room for even a few letters: none, rather than "…" alone.)
        if end - left >= px(20.0) {
            let font = style::ui_font(if selected { FontWeight::BOLD } else { FontWeight::NORMAL });
            style::label_fit(g, left, cy, style::Align::Left, tab.label.clone(), style::PLAIN_SIZE, font, ink, end - left);
        }
    });
}

/// A group's label in `r` (a pane head's, Tile All Tabs'): its name in
/// capitals on a label of its colour (`filled`; else on the panel's,
/// its name in the colour), lighter under the pointer, shaped as the
/// look's tabs are.
pub fn chip(g: &mut Painter, r: Bounds<Pixels>, name: &SharedString, color: Hsla, filled: bool, hovered: bool) {
    let f = Folder::of(None);
    let (fill_c, ink) = if filled { (if hovered { lerp(color, white(), 0.12) } else { color }, label_ink(color)) } else { (if hovered { f.hover } else { f.behind }, group_ink(color, f.behind)) };
    let (light, shadow, outline) = if filled { label_edges(fill_c) } else { (f.light, f.shadow, f.soft) };
    side_folder(g, r, fill_c, light, shadow, outline, true, Shape::now());
    style::label_spaced(g, r.center().x, r.center().y, style::Align::Center, label_text(name), CHIP_SIZE, style::ui_font(FontWeight::BOLD), ink, LABEL_SPACING);
}

/// How wide a group's label is for `name` (a pane head's).
pub fn chip_width(name: &SharedString) -> f32 {
    let w = f32::from(style::text_width_spaced(label_text(name), CHIP_SIZE, style::ui_font(FontWeight::BOLD), LABEL_SPACING));
    ((w + 2.0 * CHIP_PAD).max(CHIP_HEIGHT) / 2.0).ceil() * 2.0
}

/// A colour's relative luminance (WCAG's).
fn luminance(c: Hsla) -> f32 {
    let c = Rgba::from(c);
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A label's name (bold capitals) on its colour: the look's ink for it
/// (`Palette::label_ink`: white by day, near-black at night, where it
/// reads).
fn label_ink(fill: Hsla) -> Hsla {
    let c = Rgba::from(fill);
    let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    style::c(theme::palette().label_ink([byte(c.r), byte(c.g), byte(c.b), 255]))
}

/// A group's colour as text on `back`: itself if it reads (4:1), else
/// toward the panel's ink until it does.
fn group_ink(color: Hsla, back: Hsla) -> Hsla {
    let dark = luminance(back) < 0.2;
    let toward = if dark { white() } else { black() };
    (0..=10).map(|k| lerp(color, toward, k as f32 * 0.08)).find(|c| contrast(*c, back) >= 4.0).unwrap_or(toward)
}

/// A label's bevel and outline, from its colour.
fn label_edges(fill: Hsla) -> (Hsla, Hsla, Hsla) {
    (lerp(fill, white(), 0.35), lerp(fill, black(), 0.18), lerp(fill, black(), 0.45))
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
            // The mark: a caret in the band above the tabs (beside the
            // rows down the side), pointing at the gap, where a tab
            // riding under the pointer never covers it.
            let mark = style::c(p.keyboard_navigation);
            let draw_mark = |g: &mut Painter, at: Pixels| {
                let (half, deep) = ((MARK_WIDE / 2.0 * g.scale_factor()).round(), (MARK_DEEP * g.scale_factor()).round());
                let pts = if s.side {
                    let y = dev(g, at.clamp(full.top() + px(MARK_WIDE / 2.0), full.bottom() - px(MARK_WIDE / 2.0)));
                    let x = dev(g, full.left() + px(1.0));
                    [(x, y - half), (x + deep, y), (x, y + half)]
                } else {
                    let x = dev(g, at.clamp(full.left() + px(MARK_WIDE / 2.0), full.right() - px(MARK_WIDE / 2.0)));
                    let y = dev(g, full.top() + px(1.0));
                    [(x - half, y), (x + half, y), (x, y + deep)]
                };
                poly(g, &pts, mark);
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
        // (Every move over it: which part is under the pointer, lit; a
        // drag from it, anywhere in the window.)
        element = element.hears_motion().on_drag_move(move |e: &MouseMoveEvent, cx| {
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
pub(crate) fn fit_widths(natural: &[f32], room: f32, min: f32) -> Vec<f32> {
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
    // The cap the widest are cut to (on the grid): those under it keep
    // their width, up to the grid (down, a short name a tab was sized to
    // lost a fraction of a point and was cut: "DNS" as "D…"); the rest
    // share what's left. (Where they'd fit but for that rounding, the
    // widest few go down to the grid. A cap of none had every tab two
    // points wide: 2026-11-14, four tabs needing 816.4 of 818.)
    let mut sorted = up.clone();
    sorted.sort_by(f32::total_cmp);
    let (mut left, mut cap) = (room, f32::INFINITY);
    for (k, &w) in sorted.iter().enumerate() {
        let share = left / (n - k) as f32;
        if w > share {
            cap = grid(share);
            break;
        }
        left -= w;
    }
    up.iter().map(|&w| w.min(cap)).collect()
}

/// The colours a strip's folder tabs are painted in: the tabs behind (a
/// shade off the panel), one under the pointer, their soft outline and
/// bevel; the chosen one in the content's colours, outlined as the
/// window's frame is.
pub(crate) struct Folder {
    dark: bool,
    ground: Hsla,
    pub(crate) behind: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) soft: Hsla,
    pub(crate) light: Hsla,
    pub(crate) shadow: Hsla,
    pub(crate) front: Hsla,
    pub(crate) front_light: Hsla,
    pub(crate) front_shadow: Hsla,
    pub(crate) outline: Hsla,
    pub(crate) ink: Hsla,
    pub(crate) front_ink: Hsla,
}

impl Folder {
    pub(crate) fn of(content: Option<(Hsla, Hsla)>) -> Folder {
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let panel = p.panel_background;
        let (w, b) = (olo_theme::WHITE, olo_theme::BLACK);
        let mix = |a, c, t| style::c(olo_theme::mix(a, c, t));
        let frame = p.frame_focused[0];
        let (front, front_ink) = content.unwrap_or((style::c(p.document_background), style::c(p.document_text)));
        Folder {
            dark,
            ground: style::c(panel),
            behind: if dark { mix(panel, w, 0.04) } else { mix(panel, b, 0.04) },
            hover: if dark { mix(panel, w, 0.09) } else { mix(panel, w, 0.3) },
            soft: mix(frame, panel, if dark { 0.25 } else { 0.4 }),
            light: if dark { mix(panel, w, 0.12) } else { mix(panel, w, 0.8) },
            shadow: if dark { mix(panel, b, 0.3) } else { mix(panel, b, 0.13) },
            front,
            front_light: lerp(front, white(), if dark { 0.08 } else { 0.6 }),
            front_shadow: lerp(front, black(), if dark { 0.25 } else { 0.1 }),
            outline: style::c(frame),
            ink: mix(p.panel_text, panel, 0.15),
            front_ink,
        }
    }
}

/// Device pixels, for shapes drawn on them.
pub(crate) fn dev(g: &Painter, v: Pixels) -> f32 {
    (f32::from(v) * g.scale_factor()).round()
}

/// The hairline and the bevel, in device pixels (as the window's frame
/// has them).
pub(crate) fn lines(g: &Painter) -> (f32, f32) {
    let s = g.scale_factor();
    (s.floor().max(1.0), s.round().max(1.0))
}

/// A polygon in device pixels, filled (antialiased along its slants).
fn poly(g: &mut Painter, pts: &[(f32, f32)], color: Hsla) {
    let s = g.scale_factor();
    let mut path = olo_interface::PathBuilder::fill();
    let pts: Vec<Point<Pixels>> = pts.iter().map(|&(x, y)| point(px(x / s), px(y / s))).collect();
    path.add_polygon(&pts, true);
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
    }
}

/// A rectangle in device pixels with its corners rounded (top-left,
/// top-right, bottom-right, bottom-left), filled.
fn rounded(g: &mut Painter, (l, t, r, b): (f32, f32, f32, f32), [tl, tr, br, bl]: [f32; 4], color: Hsla) {
    if r <= l || b <= t {
        return;
    }
    let s = g.scale_factor();
    let at = |x: f32, y: f32| point(px(x / s), px(y / s));
    let k = |q: f32| point(px(q / s), px(q / s));
    let mut path = olo_interface::PathBuilder::fill();
    path.move_to(at(l + tl, t));
    path.line_to(at(r - tr, t));
    if tr > 0.0 {
        path.arc_to(k(tr), px(0.0), false, true, at(r, t + tr));
    }
    path.line_to(at(r, b - br));
    if br > 0.0 {
        path.arc_to(k(br), px(0.0), false, true, at(r - br, b));
    }
    path.line_to(at(l + bl, b));
    if bl > 0.0 {
        path.arc_to(k(bl), px(0.0), false, true, at(l, b - bl));
    }
    path.line_to(at(l, t + tl));
    if tl > 0.0 {
        path.arc_to(k(tl), px(0.0), false, true, at(l + tl, t));
    }
    path.close();
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
    }
}

/// A convex polygon (device pixels, clockwise on screen) moved in by a
/// distance per edge (edge `i` from point `i` to point `i + 1`).
fn inset(pts: &[(f32, f32)], ds: &[f32]) -> Vec<(f32, f32)> {
    let n = pts.len();
    let lines: Vec<(f32, f32, f32, f32)> = (0..n)
        .map(|i| {
            let ((x0, y0), (x1, y1)) = (pts[i], pts[(i + 1) % n]);
            let (dx, dy) = (x1 - x0, y1 - y0);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (nx, ny) = (-dy / len, dx / len);
            (x0 + nx * ds[i], y0 + ny * ds[i], dx, dy)
        })
        .collect();
    (0..n)
        .map(|i| {
            let (a, b) = (lines[(i + n - 1) % n], lines[i]);
            let den = a.2 * b.3 - a.3 * b.2;
            if den.abs() < 1e-9 {
                return (b.0, b.1);
            }
            let t = ((b.0 - a.0) * b.3 - (b.1 - a.1) * b.2) / den;
            (a.0 + a.2 * t, a.1 + a.3 * t)
        })
        .collect()
}

/// A folder tab over `r` (its top to the strip's bottom), open along its
/// bottom: its outline, the bevel's light along its left and top and
/// shadow down its right, then its fill. BeOS's slanted, or Platinum's
/// rounded at the top. It ends where `r` does, on a device pixel: it
/// ended a pixel past it, which the tab view allowed for and the strip
/// didn't, so a strip's tabs' outlines ran a pixel below the line along
/// its foot (a tick at each lower corner, at every scale; found by a
/// reviewer's look, 2026-11-21).
#[allow(clippy::too_many_arguments)]
pub(crate) fn folder(g: &mut Painter, r: Bounds<Pixels>, fill_c: Hsla, light: Hsla, shadow: Hsla, outline: Hsla, sh: Shape) {
    let (hair, bev) = lines(g);
    let s = g.scale_factor();
    let (l, t, rr, b) = (dev(g, r.left()), dev(g, r.top()), dev(g, r.right()), dev(g, r.bottom()));
    if !sh.rounded {
        let k = (sh.slant * s).round();
        let outer = [(l, b), (l + k, t), (rr - k, t), (rr, b)];
        poly(g, &outer, outline);
        poly(g, &inset(&outer, &[hair, hair, hair, 0.0]), light);
        poly(g, &inset(&outer, &[hair + bev, hair + bev, hair, 0.0]), shadow);
        poly(g, &inset(&outer, &[hair + bev, hair + bev, hair + bev, 0.0]), fill_c);
    } else {
        let q = (sh.radius * s).round();
        let top = |q: f32| [q.max(0.0), q.max(0.0), 0.0, 0.0];
        rounded(g, (l, t, rr, b), top(q), outline);
        rounded(g, (l + hair, t + hair, rr - hair, b), top(q - hair), light);
        rounded(g, (l + hair + bev, t + hair + bev, rr - hair, b), top(q - hair - bev), shadow);
        rounded(g, (l + hair + bev, t + hair + bev, rr - hair - bev, b), top(q - hair - bev), fill_c);
    }
}

/// The same turned on its side (a side strip's): its left closed, its
/// right open onto the content, or closed too (`closed`: a label), lit
/// along its left and top.
#[allow(clippy::too_many_arguments)]
fn side_folder(g: &mut Painter, r: Bounds<Pixels>, fill_c: Hsla, light: Hsla, shadow: Hsla, outline: Hsla, closed: bool, sh: Shape) {
    let (hair, bev) = lines(g);
    let s = g.scale_factor();
    let (l, t, rr, b) = (dev(g, r.left()), dev(g, r.top()), dev(g, r.right()), dev(g, r.bottom()));
    let (rc, rb) = if closed { (hair, hair + bev) } else { (0.0, 0.0) };
    if !sh.rounded {
        let ch = (5.0 * s).round();
        let outer = [(rr, t), (rr, b), (l + ch, b), (l, b - ch), (l, t + ch), (l + ch, t)];
        poly(g, &outer, outline);
        poly(g, &inset(&outer, &[rc, hair, hair, hair, hair, hair]), shadow);
        poly(g, &inset(&outer, &[rb, hair + bev, hair, hair, hair, hair]), light);
        poly(g, &inset(&outer, &[rb, hair + bev, hair + bev, hair + bev, hair + bev, hair + bev]), fill_c);
        return;
    }
    let q = (5.0 * s).round();
    let qr = if closed { q } else { 0.0 };
    let corners = |a: f32, c: f32| [a.max(0.0), c.max(0.0), c.max(0.0), a.max(0.0)];
    rounded(g, (l, t, rr, b), corners(q, qr), outline);
    rounded(g, (l + hair, t + hair, rr - rc, b - hair), corners(q - hair, qr - hair), shadow);
    rounded(g, (l + hair, t + hair, rr - rb, b - hair - bev), corners(q - hair, qr - hair), light);
    rounded(g, (l + hair + bev, t + hair + bev, rr - rb, b - hair - bev), corners(q - hair - bev, qr - hair - bev), fill_c);
}

/// The group a label stands for, drawn: its colour, or (not chosen, with
/// [`TabStrip::group_selection`]) the panel's with its name in the
/// colour; lighter under the pointer.
fn label_colours(f: &Folder, color: Hsla, filled: bool, hovered: bool) -> (Hsla, Hsla, Hsla, Hsla, Hsla) {
    if filled {
        let fill_c = if hovered { lerp(color, white(), 0.12) } else { color };
        let (light, shadow, outline) = label_edges(fill_c);
        (fill_c, light, shadow, outline, label_ink(color))
    } else {
        let fill_c = if hovered { f.hover } else { f.behind };
        (fill_c, f.light, f.shadow, f.soft, group_ink(color, fill_c))
    }
}

/// The tabs in the group that tab `first` leads, to the last.
#[allow(dead_code)]
fn last_of(l: &Layout, g: usize, first: usize) -> usize {
    (first..l.slots.len()).take_while(|&i| l.groups_of[i] == Some(g)).last().unwrap_or(first)
}

/// Along the top (Ben, 2026-11-14, "folder tabs -> labels"): folder tabs
/// on the panel, the chosen one in the content's colours and open onto
/// it, the others a shade off the panel behind the line along the
/// strip's foot (lighter under the pointer); a group led by a label of
/// its colour with its name in capitals; a gap between tabs, more
/// between groups. The × only on the chosen tab and the one under the
/// pointer. A tab being dragged (`lift`: it, and how far) rides as a
/// chosen one under the pointer, its place left empty.
#[allow(clippy::too_many_arguments)]
fn paint_top(g: &mut Painter, s: &TabStrip, full: Bounds<Pixels>, l: &Layout, under: Option<Part>, drag: Option<Drag>, lift: Option<(usize, Pixels)>) {
    let sh = Shape::now();
    let f = Folder::of(s.content);
    let n = s.tabs.len();
    style::fill_rect(g, full, f.ground);
    let lifted = |i: usize| lift.is_some_and(|(t, _)| t == i);
    let pointed = |i: usize| matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i);
    // (Its icon only with room for it, a few letters and the ×.)
    let icons = |r: Bounds<Pixels>| r.size.width >= px(2.0 * sh.pad + 16.0 + 8.0 + 28.0 + CLOSE);
    // The labels.
    for &(group, _, r) in &l.chips {
        let StripGroup { name, color } = s.groups[group].clone();
        let chosen = s.chosen.as_ref().map(|c| c.get(group).copied().unwrap_or(false));
        let hovered = under == Some(Part::Chip(group)) || drag == Some(Drag::Group(group));
        let (fill_c, light, shadow, outline, ink) = label_colours(&f, color, chosen != Some(false), hovered);
        folder(g, r, fill_c, light, shadow, outline, sh);
        style::label_spaced(g, r.center().x, l.cy, style::Align::Center, label_text(&name), CHIP_SIZE, style::ui_font(FontWeight::BOLD), ink, LABEL_SPACING);
    }
    // The tabs behind.
    for (i, tab) in s.tabs.iter().enumerate().filter(|&(i, _)| i != s.active && !lifted(i)) {
        let r = l.slots[i];
        let back = if pointed(i) { f.hover } else { f.behind };
        folder(g, r, back, f.light, f.shadow, f.soft, sh);
        crate::review::own_grid(g, r);
        tab_face(g, tab, false, (pointed(i), under == Some(Part::Close(i))), (r.left() + px(sh.pad), l.cy, l.closes[i]), icons(r), f.ink, back);
    }
    // The line along the foot, but under the chosen tab: it opens onto
    // the content.
    let (hair, _) = lines(g);
    let sc = g.scale_factor();
    let foot = |g: &mut Painter, x0: f32, x1: f32| {
        if x1 > x0 {
            let b = dev(g, full.bottom());
            style::fill_rect(g, Bounds::from_corners(point(px(x0 / sc), px((b - hair) / sc)), point(px(x1 / sc), px(b / sc))), f.outline);
        }
    };
    let chosen = (s.active < n && !lifted(s.active)).then(|| l.slots[s.active]);
    match chosen.filter(|_| !s.closed) {
        Some(r) => {
            foot(g, dev(g, full.left()), dev(g, r.left()) + hair);
            foot(g, dev(g, r.right()) - hair, dev(g, full.right()));
        }
        None => foot(g, dev(g, full.left()), dev(g, full.right())),
    }
    if let Some(r) = chosen {
        folder(g, r, f.front, f.front_light, f.front_shadow, f.outline, sh);
        if s.closed {
            foot(g, dev(g, r.left()), dev(g, r.right()));
        }
        crate::review::own_grid(g, r);
        tab_face(g, &s.tabs[s.active], true, (pointed(s.active), under == Some(Part::Close(s.active))), (r.left() + px(sh.pad), l.cy, l.closes[s.active]), icons(r), f.front_ink, f.front);
    }
    if let Some(plus) = l.new {
        let hov = under == Some(Part::New);
        if hov {
            g.paint_quad(fill(plus, lerp(f.ground, if f.dark { black() } else { white() }, if f.dark { 0.2 } else { 0.5 })).corner_radii(px(NEW_SIZE / 2.0)));
        }
        plus_mark(g, plus.center(), lerp(style::c(theme::palette().panel_text), f.ground, if hov { 0.0 } else { 0.25 }));
    }
    if let Some((i, d)) = lift.filter(|&(i, _)| i < n) {
        // Kept within the strip, on device pixels.
        let r = l.slots[i];
        let d = style::snap_coord(g, d.clamp(full.left() - r.left(), full.right() - r.right()));
        let r = Bounds { origin: point(r.left() + d, r.top()), ..r };
        let close = Bounds { origin: point(l.closes[i].left() + d, l.closes[i].top()), ..l.closes[i] };
        folder(g, r, f.front, f.front_light, f.front_shadow, f.outline, sh);
        tab_face(g, &s.tabs[i], true, (false, false), (r.left() + px(sh.pad), l.cy, close), icons(r), f.front_ink, f.front);
    }
}

/// Down the side: rows on the panel; the chosen one a folder tab turned
/// on its side, open onto the content at its right; one under the
/// pointer on a shade of the panel; a group's label of its colour above
/// its rows. A row being dragged rides as the chosen one under the
/// pointer, as along the top.
#[allow(clippy::too_many_arguments)]
fn paint_side(g: &mut Painter, s: &TabStrip, full: Bounds<Pixels>, l: &Layout, under: Option<Part>, drag: Option<Drag>, lift: Option<(usize, Pixels)>) {
    let sh = Shape::now();
    let f = Folder::of(s.content);
    let lifted = |i: usize| lift.is_some_and(|(t, _)| t == i);
    let (hair, _) = lines(g);
    let sc = g.scale_factor();
    style::fill_rect(g, full, f.ground);
    let n = s.tabs.len();
    let lx = full.left() + px(SIDE_LEFT);
    let rx = full.right();
    let mid = |g: &Painter, r: Bounds<Pixels>| style::snap_coord(g, r.center().y.round());
    let icons = |r: Bounds<Pixels>| r.size.width >= px(72.0);
    // The edge along its right, but beside the chosen row.
    let chosen = (s.active < n && !lifted(s.active)).then(|| l.slots[s.active]);
    let edge = |g: &mut Painter, y0: f32, y1: f32| {
        if y1 > y0 {
            let x = dev(g, rx);
            style::fill_rect(g, Bounds::from_corners(point(px((x - hair) / sc), px(y0 / sc)), point(px(x / sc), px(y1 / sc))), f.outline);
        }
    };
    match chosen.filter(|_| !s.closed) {
        Some(r) => {
            edge(g, dev(g, full.top()), dev(g, r.top()));
            edge(g, dev(g, r.bottom()), dev(g, full.bottom()));
        }
        None => edge(g, dev(g, full.top()), dev(g, full.bottom())),
    }
    for &(group, _, r) in &l.chips {
        let StripGroup { name, color } = s.groups[group].clone();
        let chosen = s.chosen.as_ref().map(|c| c.get(group).copied().unwrap_or(false));
        let hovered = under == Some(Part::Chip(group)) || drag == Some(Drag::Group(group));
        let (fill_c, light, shadow, outline, ink) = label_colours(&f, color, chosen != Some(false), hovered);
        side_folder(g, r, fill_c, light, shadow, outline, true, sh);
        let (text, font, room) = (label_text(&name), style::ui_font(FontWeight::BOLD), r.size.width - px(SIDE_LABEL_TEXT));
        // (Spaced out where it fits; cut short, not spaced, where not.)
        if style::text_width_spaced(text.clone(), CHIP_SIZE, font.clone(), LABEL_SPACING) <= room {
            style::label_spaced(g, r.left() + px(SIDE_LABEL_TEXT), mid(g, r), style::Align::Left, text, CHIP_SIZE, font, ink, LABEL_SPACING);
        } else {
            style::label_fit(g, r.left() + px(SIDE_LABEL_TEXT), mid(g, r), style::Align::Left, text, CHIP_SIZE, font, ink, room);
        }
    }
    for (i, tab) in s.tabs.iter().enumerate().filter(|&(i, _)| !lifted(i)) {
        let r = l.slots[i];
        let row = Bounds::from_corners(point(lx, r.top()), point(rx, r.bottom()));
        let pointed = matches!(under, Some(Part::Tab(t) | Part::Close(t)) if t == i);
        let (back, ink) = if i == s.active {
            // (To the edge's outer side, its outlines meeting the edge's
            // ends: it opens onto the content.)
            let open = Bounds::from_corners(row.origin, point(px(dev(g, rx) / sc), row.bottom()));
            side_folder(g, open, f.front, f.front_light, f.front_shadow, f.outline, false, sh);
            if s.closed {
                edge(g, dev(g, row.top()), dev(g, row.bottom()));
            }
            (f.front, f.front_ink)
        } else if pointed {
            let b = (dev(g, lx), dev(g, row.top()) + hair, dev(g, rx - px(8.0)), dev(g, row.bottom()) - hair);
            let q = if sh.rounded { (5.0 * sc).round() } else { 0.0 };
            rounded(g, b, [q; 4], f.hover);
            (f.hover, f.ink)
        } else {
            (f.ground, f.ink)
        };
        tab_face(g, tab, i == s.active, (pointed, under == Some(Part::Close(i))), (lx + px(SIDE_FACE), mid(g, r), l.closes[i]), icons(r), ink, back);
    }
    if let Some(r) = l.new {
        let hov = under == Some(Part::New);
        let color = lerp(style::c(theme::palette().panel_text), f.ground, if hov { 0.0 } else { 0.3 });
        let y = mid(g, r);
        let x = r.left() + px(SIDE_LEFT + 6.0);
        plus_mark(g, point(x + px(8.0), y), color);
        style::label_fit(g, x + px(22.0), y, style::Align::Left, "New Tab", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, r.right() - px(m::TIGHT) - x - px(22.0));
    }
    if let Some((i, d)) = lift.filter(|&(i, _)| i < n) {
        let r = l.slots[i];
        let d = style::snap_coord(g, d.clamp(full.top() - r.top(), full.bottom() - r.bottom()));
        let r = Bounds { origin: point(r.left(), r.top() + d), ..r };
        let close = Bounds { origin: point(l.closes[i].left(), l.closes[i].top() + d), ..l.closes[i] };
        let row = Bounds::from_corners(point(lx, r.top()), point(px(dev(g, rx) / sc), r.bottom()));
        side_folder(g, row, f.front, f.front_light, f.front_shadow, f.outline, false, sh);
        tab_face(g, &s.tabs[i], true, (false, false), (lx + px(SIDE_FACE), mid(g, r), close), icons(r), f.front_ink, f.front);
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
        // They fit, but not each rounded up to the grid: the widest down.
        assert_eq!(fit_widths(&[226.388, 201.164, 189.008, 199.796], 818.0, 120.0), vec![226.0, 202.0, 190.0, 200.0]);
        // One under the cap keeps what its name needs, up to the grid.
        assert_eq!(fit_widths(&[37.3, 120.0, 120.0], 250.0, 40.0), vec![38.0, 106.0, 106.0]);
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
