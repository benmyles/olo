//! A segmented control: a few choices side by side in one sunken
//! trough, one of them chosen, for switching what a pane shows between
//! two or three lists of the same standing (Rex's Sessions and Activity,
//! Phone's Messages and Contacts) or choosing one of a few modes
//! (Screenshot's shapes, Viewer's tools). Each segment is its name, with
//! a mark before it and a count after it if it has them. The chosen one
//! is a raised plate set in the trough, a button's face with a button's
//! bevel, its name in semibold; the others are their names on the trough's
//! ground, a short line between two neighbours neither of which is
//! chosen.
//!
//! It is made of what Olo's windows are made of already, a sunken well
//! and a bevelled button, which is why it was chosen of five looks drawn
//! and pictured (Ben, 2026-11-22: "this sessions/activity segmented
//! control looks weird. i think something more like this would be much
//! nicer", then a second drawing and two more, and "take a poll from the
//! other agents"; both reviewers ranked this one first twice: it can't
//! be taken for a second tab strip beside the real one, and it leaves
//! the window tab's yellow and the selection's blue meaning what they
//! mean).
//!
//! BeOS had none: it chose among a few with radio buttons or a pop-up
//! menu, and switched pages with a tab view. Tabs still own a panel they
//! stand on ([`TabView`](super::TabView)), and a window's documents are a
//! tab strip; this is for a choice that sits *in* a bar or a header, no
//! panel hanging from it (Ben: "this shouldn't replace tabs, the tabs are
//! good now this is different -- a segmented control -- vs a tab bar").
//! More than four choices, or names that don't fit: a pop-up menu
//! ([`MenuField`](super::MenuField)).
//!
//! A click on a segment chooses it, at the press. With the keyboard on it
//! (it is one stop in the window's tab order) the arrows move the choice,
//! Home and End take the first and the last; the chosen name is
//! underlined then, as a button's is. Agents find each segment as a
//! `Segment`, selected or not, with its count as its value.
//!
//! ```ignore
//! SegmentedControl::new("side", vec![Segment::new("Sessions").mark(Mark::Terminal), Segment::new("Activity").mark(Mark::Pulse).count(waiting)], chosen)
//!     .width(side_width)
//!     .on_choose(cx.msg_with(|i: &usize| Msg::Side(*i)))
//! ```

use std::rc::Rc;

use olo_interface::{
    BorderStyle, Bounds, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, Painter, PathBuilder, Pixels, SharedString, Styled as _, WindowCx, agent,
    control_canvas, fill, linear_color_stop, linear_gradient, point, px, quad,
};
use olo_kit_base::metrics as m;

use super::buttons::focus_underline;
use crate::{style, theme};

/// The least room between a plate's edge and what it holds, in a
/// control narrower than it would be by itself: four points of the
/// plate's face, inside its outline and bevel.
const LEAST_PAD: f32 = 6.0;
/// And the least before a name is cut short: where even its mark's going
/// leaves a name and its count too little (a two-figure count in a
/// sidebar at its narrowest), they take this much of the room first.
const TIGHT_PAD: f32 = 4.0;
/// Between a segment's name and its count.
const GAP: f32 = 5.0;
/// The chosen segment's name: semibold. Bold, as a chosen tab's name
/// is, was a touch too strong here (Ben, 2026-11-22: "yeah let's just
/// go with semibold"): the plate says which is chosen, and the name's
/// weight need only agree with it.
const CHOSEN: FontWeight = FontWeight::SEMIBOLD;
/// Between a segment's mark and its name.
const MARK_GAP: f32 = 4.0;

/// A mark before a segment's name: a small drawing in the name's own
/// colour, in whole device lines at every scale (one of Olo's colour
/// icons at this size is a smear at 1×, as both reviewers found of the
/// first pictures; a mark is what a control's own face carries).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// A terminal: sessions, shells.
    Terminal,
    /// A pulse: activity, what is going on.
    Pulse,
    /// A keyboard: typing.
    Keyboard,
    /// A bridge hung from two towers: what joins two places.
    Bridge,
    /// An open book: what is known, skills.
    Book,
}

type ChooseFn = Rc<dyn Fn(&usize, &mut WindowCx)>;

/// One choice of a [`SegmentedControl`].
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    name: SharedString,
    mark: Option<Mark>,
    /// How many wait in it, if it is one that counts.
    count: Option<usize>,
    /// What it counts waits for an answer ([`Segment::waiting`]).
    waiting: bool,
}

impl Segment {
    pub fn new(name: impl Into<SharedString>) -> Segment {
        Segment { name: crate::words::of(name), mark: None, count: None, waiting: false }
    }

    /// A mark before its name. Marks show where every segment has room
    /// for its own beside its whole name and its count; in a narrower
    /// control none has one.
    pub fn mark(mut self, mark: Mark) -> Self {
        self.mark = Some(mark);
        self
    }

    /// How many of something wait in it, in a disc after its name (the
    /// look's notice colour, as what waits is marked elsewhere); none
    /// shows nothing, and keeps a disc's room in the control's width
    /// and in what fits, so the control is no wider and loses no mark
    /// when the first one comes.
    pub fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }

    /// As [`Segment::count`], of what waits for the person's answer (agents
    /// asking): the disc in the orange an agent asking is marked with
    /// (`agent_marks`), so the count reads as the marks in its list do,
    /// not as a notification's blue. Opus's look at Rex's Activity.
    pub fn waiting(mut self, n: usize) -> Self {
        self.count = Some(n);
        self.waiting = true;
        self
    }

    fn badge(&self) -> Option<String> {
        self.count.filter(|n| *n > 0).map(|n| if n > 99 { "99+".to_owned() } else { n.to_string() })
    }
}

impl<T: Into<SharedString>> From<T> for Segment {
    fn from(name: T) -> Segment {
        Segment::new(name)
    }
}

pub struct SegmentedControl {
    id: SharedString,
    segments: Rc<[Segment]>,
    chosen: usize,
    width: Option<f32>,
    enabled: bool,
    on_choose: Option<ChooseFn>,
}

impl SegmentedControl {
    pub fn new(id: impl Into<SharedString>, segments: impl IntoIterator<Item = impl Into<Segment>>, chosen: usize) -> SegmentedControl {
        SegmentedControl { id: id.into(), segments: segments.into_iter().map(Into::into).collect(), chosen, width: None, enabled: true, on_choose: None }
    }

    /// As wide as this, its segments sharing it equally (a sidebar's
    /// width); else each is as wide as the widest needs.
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// A segment is chosen (clicked, or the arrows): its index.
    pub fn on_choose(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }
}

/// What the control is drawn with, in points. One size: as tall as a
/// push button, so it lines up with the buttons and fields beside it,
/// and no smaller one (there was one, 20 points for a dense bar: its
/// names had two points over and under them in their plate, and Ben
/// said of it in Phone's header, 2026-11-22, "this is way too
/// squashed").
#[derive(Clone, Copy)]
struct Sizes {
    height: f32,
    font: f32,
    /// A mark's box: how wide, how tall.
    mark: (f32, f32),
    /// The room either side of what a segment holds, where the control
    /// is as wide as it likes.
    pad: f32,
    /// A count's disc: how tall, and its numerals' size. Two points
    /// less than the plate's face is tall, so a point of the face shows
    /// over it and under it when its segment is the chosen one; its
    /// numerals' capitals have three points of the disc over and under
    /// them, and are as many device pixels tall as leaves that even.
    count: (f32, f32),
    /// The trough's ground showing round the plate on every side.
    ground: f32,
    /// The trough's corners; the plate's are round the same centre.
    radius: f32,
}

/// The trough's line is a point and its ground another, and the plate's
/// outline and bevel two: that leaves the plate's face sixteen points
/// of the twenty-four, with 3.7 over and under a name's capitals in it.
const SIZES: Sizes = Sizes { height: m::CONTROL_HEIGHT, font: style::PLAIN_SIZE, mark: (12.0, 9.0), pad: m::CONTROL_PAD, count: (14.0, 11.0), ground: 1.0, radius: 4.0 };

/// How wide a count's disc is: round for one numeral, longer for more.
fn count_width(text: &str, sz: Sizes) -> f32 {
    let (h, font) = sz.count;
    (f32::from(style::text_width(text.to_owned(), font, style::ui_font(FontWeight::BOLD))) + 8.0).max(h).ceil()
}

/// What a segment holds, left to right: how wide its mark, its name (in
/// the chosen one's weight: a segment doesn't change its
/// width when chosen) and its count are. One that counts keeps a
/// numeral's disc of room while nothing waits.
fn parts(seg: &Segment, sz: Sizes) -> (f32, f32, f32) {
    let mark = if seg.mark.is_some() { sz.mark.0 } else { 0.0 };
    let name = f32::from(style::text_width(seg.name.clone(), sz.font, style::ui_font(CHOSEN))).ceil();
    let count = seg.count.map_or(0.0, |_| count_width(seg.badge().as_deref().unwrap_or("0"), sz));
    (mark, name, count)
}

fn content_width(seg: &Segment, sz: Sizes) -> f32 {
    let (mark, name, count) = parts(seg, sz);
    mark + if mark > 0.0 { MARK_GAP } else { 0.0 } + name + if count > 0.0 { GAP + count } else { 0.0 }
}

/// How wide a control of `segments` is when nothing sets its width: each
/// as wide as the widest needs, on the two-point grid.
pub fn segmented_width(segments: &[Segment]) -> f32 {
    let sz = SIZES;
    let widest = segments.iter().map(|s| content_width(s, sz)).fold(0.0_f32, f32::max);
    let each = ((widest + 2.0 * sz.pad) / 2.0).ceil() * 2.0;
    each * segments.len().max(1) as f32
}

/// How a control `width` points wide is shared between its segments,
/// as parts of the whole: equally, which is what a segmented control
/// looks like; but where an equal share would cut a name short and the
/// names would all be whole with what each needs (a pane drawn narrow,
/// one long name with a count beside a short one: Phone's Messages and
/// Contacts in a third of a screen, where the lint found "Messages"
/// cut), each gets what it needs and an equal part of what is over.
fn shares(segments: &[Segment], sz: Sizes, width: f32) -> Vec<f32> {
    let n = segments.len().max(1);
    let equal = vec![1.0 / n as f32; n];
    // (Inside the trough's line. What a segment needs at the least: its
    // name at the chosen one's weight and its count, the least room either side of them
    // in the plate, and the ground round the plate. Marks have given
    // way long before this.)
    let inside = width - 2.0;
    let needs: Vec<f32> = segments
        .iter()
        .map(|seg| {
            let (_, name, count) = parts(seg, sz);
            name + if count > 0.0 { GAP + count } else { 0.0 } + 2.0 * TIGHT_PAD + 2.0 * sz.ground
        })
        .collect();
    let all: f32 = needs.iter().sum();
    if inside <= 0.0 || needs.iter().all(|w| *w <= inside / n as f32) || all > inside {
        return equal;
    }
    let over = (inside - all) / n as f32;
    needs.iter().map(|w| (w + over) / inside).collect()
}

/// Each segment's place in `outer`, by its share of it: the edges
/// between them on device pixels, its own ends as they are given (the
/// first and the last take what rounding left).
fn places(scale: f32, outer: Bounds<Pixels>, shares: &[f32]) -> Vec<Bounds<Pixels>> {
    let snap = |v: f32| px((v * scale).round() / scale);
    let (left, width) = (f32::from(outer.left()), f32::from(outer.size.width));
    let n = shares.len();
    let mut before = 0.0;
    shares
        .iter()
        .enumerate()
        .map(|(i, share)| {
            let (x0, x1) = (left + width * before, left + width * (before + share));
            before += share;
            Bounds::from_corners(point(if i == 0 { outer.left() } else { snap(x0) }, outer.top()), point(if i + 1 == n { outer.right() } else { snap(x1) }, outer.bottom()))
        })
        .collect()
}

/// Where each segment's plate is when it is the chosen one, in the
/// trough `outer`: the trough inside its line is shared between the
/// segments (equally, but in a control too narrow for that: `shares`),
/// and a plate's place is its segment's share less `ground` on all
/// four sides (in whole device pixels). So a plate is the same size
/// wherever it is, first, middle or last (an end plate was a point and
/// a half narrower than a middle one: Opus), the ground showing round
/// it is as wide on every side at every scale, and between two places
/// there is twice that. What a segment holds is centred in its place
/// whether it is chosen or not.
fn slots(scale: f32, outer: Bounds<Pixels>, shares: &[f32], ground: f32) -> Vec<Bounds<Pixels>> {
    let line = px(style::line_pixels(scale) / scale);
    let inside = Bounds::from_corners(point(outer.left() + line, outer.top() + line), point(outer.right() - line, outer.bottom() - line));
    let round = px((ground * scale).round().max(1.0) / scale);
    places(scale, inside, shares).iter().map(|r| Bounds::from_corners(point(r.left() + round, r.top() + round), point(r.right() - round, r.bottom() - round))).collect()
}

impl IntoElement for SegmentedControl {
    fn into_element(self) -> Element {
        let SegmentedControl { id, segments, chosen, width, enabled, on_choose } = self;
        let sz = SIZES;
        let n = segments.len();
        let width = width.unwrap_or_else(|| segmented_width(&segments));
        let painted = segments.clone();
        let split: Rc<[f32]> = shares(&segments, sz, width).into();
        let (drawn, pressed_at) = (split.clone(), split);
        let mut el = control_canvas(move |g, b, state| paint(g, b, &painted, &drawn, chosen, sz, enabled, state.focused))
            .paint_key(style::key((&*segments, chosen, enabled)))
            .id(SharedString::from(format!("segmented-{id}")))
            .w(px(width))
            .h(px(sz.height))
            .flex_shrink_0();
        if !enabled || n == 0 {
            // (It draws its own look for that: not faded besides.)
            return el.disabled(!enabled).unfaded();
        }
        let (pressed, keys) = (on_choose.clone(), on_choose);
        el = el
            .focusable()
            .tab_stop()
            .on_mouse_down(MouseButton::Left, move |e, cx| {
                let hit = places(cx.scale_factor(), cx.listener_bounds(), &pressed_at).iter().position(|r| r.contains(&e.position));
                if let (Some(i), Some(f)) = (hit, &pressed)
                    && i != chosen
                {
                    f(&i, cx);
                }
            })
            .on_key_down(move |e: &KeyDownEvent, cx| {
                let to = match e.keystroke.key.as_str() {
                    "left" | "up" => chosen.saturating_sub(1),
                    "right" | "down" => (chosen + 1).min(n - 1),
                    "home" => 0,
                    "end" => n - 1,
                    _ => return,
                };
                if let Some(f) = &keys
                    && to != chosen
                {
                    f(&to, cx);
                }
                cx.stop_propagation();
            });
        el
    }
}

/// A shape filled from `top` at its top to `foot` at its foot.
fn shaded(top: Hsla, foot: Hsla) -> olo_interface::Background {
    linear_gradient(180.0, linear_color_stop(top, 0.0), linear_color_stop(foot, 1.0))
}

/// `b` less a line off its right and its foot: drawn over `b` in a
/// second colour, `b`'s own colour is left as a line along its right
/// and its foot, round its corners too (a bevel on a rounded shape).
fn less(b: Bounds<Pixels>, line: Pixels) -> Bounds<Pixels> {
    Bounds::from_corners(b.origin, point(b.right() - line, b.bottom() - line))
}

/// The trough: a sunken well with round corners, in the colours of the
/// kit's own ([`style::sunken`]): a line of shade along its top and its
/// left, a line of light along its foot and its right, and inside that
/// line its ground, a step under the panel and a little deeper at the
/// top. One line all round, so its inside edge is as far in on every
/// side and a plate can be set evenly in it (with the well's second,
/// inner bevel, dark on two sides only, the plate looked pushed up and
/// left: Gemini's look).
fn paint_trough(g: &mut Painter, outer: Bounds<Pixels>, radius: Pixels, p: &olo_theme::Palette, enabled: bool) {
    let bg = p.panel_background;
    let line = style::hairline(g);
    // (In a control that can't be used its shade steps back with what
    // is in it, half way to its ground, as the plate's outline does:
    // at full strength it was the darkest thing there, an empty well
    // that looked in use. Its light stays.)
    // (A shade softer than the plate's outline, which is the crisp edge
    // here: as dark as it, the two a point apart read as one heavy
    // double frame at 1×.)
    let full = olo_theme::tint(bg, 1.4);
    let shade = if enabled { full } else { olo_theme::mix(full, p.shade(bg, 1.1), 0.5) };
    g.paint_quad(fill(outer, style::c(p.shine)).corner_radii(radius));
    g.paint_quad(fill(less(outer, line), style::c(shade)).corner_radii(radius));
    let ground = style::inset(g, outer, 1.0);
    g.paint_quad(fill(ground, shaded(style::shade(p, bg, 1.13), style::shade(p, bg, 1.08))).corner_radii(radius - line));
    crate::review::gradient(g, ground);
}

/// The chosen segment's plate: a push button at rest (its outline, its
/// light along the top and left and its shade along the foot and right,
/// its face: `Button`'s colours) with round corners. In a control that
/// can't be used it is half way to the trough's ground, outline, bevel
/// and face alike: still seen to be the chosen one, in the light look
/// and the dark the same.
fn paint_plate(g: &mut Painter, plate: Bounds<Pixels>, radius: Pixels, p: &olo_theme::Palette, enabled: bool) {
    let (bg, base) = (p.panel_background, p.control_background);
    let line = style::hairline(g);
    let ground = p.shade(bg, 1.1);
    let role = |c: olo_theme::Rgba| style::c(if enabled { c } else { olo_theme::mix(c, ground, 0.5) });
    g.paint_quad(fill(plate, role(p.shade(bg, 1.6))).corner_radii(radius));
    let bevel = style::inset(g, plate, 1.0);
    g.paint_quad(fill(bevel, role(p.shade(base, 1.2))).corner_radii((radius - line).max(px(0.0))));
    g.paint_quad(fill(less(bevel, line), role(p.shine)).corner_radii((radius - line).max(px(0.0))));
    let face = style::inset(g, plate, 2.0);
    g.paint_quad(fill(face, shaded(role(olo_theme::tint(base, 0.85)), role(p.shade(base, 1.06)))).corner_radii((radius - line * 2.0).max(px(0.0))));
    // (The layout review is told of each ground this control paints,
    // since it paints its rounded shapes itself: its names are judged
    // in the plate's face and the trough's ground, centred and with
    // room, and the numerals in their disc.)
    crate::review::gradient(g, face);
}

#[allow(clippy::too_many_arguments)]
fn paint(g: &mut Painter, outer: Bounds<Pixels>, segments: &[Segment], shares: &[f32], chosen: usize, sz: Sizes, enabled: bool, focused: bool) {
    let p = theme::palette();
    let outer = style::snap(g, outer);
    let scale = g.scale_factor();
    let rects = places(scale, outer, shares);
    let at = slots(scale, outer, shares, sz.ground);
    // Marks are shown where every segment has room for its own beside
    // its name at the chosen one's weight and its count: in a control too narrow for that
    // (a sidebar drawn in), none has one and the names are whole
    // ("Sessi…" beside a mark said less than "Sessions" without).
    let marks = segments.iter().zip(&at).all(|(seg, slot)| content_width(seg, sz) <= f32::from(slot.size.width) - 2.0 * LEAST_PAD);
    paint_trough(g, outer, px(sz.radius), &p, enabled);
    // A short line between two neighbours neither of which is chosen,
    // in the middle of the ground between their places.
    for (i, r) in rects.iter().enumerate().skip(1) {
        if i != chosen && i - 1 != chosen {
            let slot = at[i];
            style::vline(g, r.left(), slot.top() + px(3.0), slot.bottom() - px(3.0), if enabled { style::well_rim(&p, 1.4) } else { style::disabled_text(&p).opacity(0.6) });
        }
    }
    if let Some(slot) = at.get(chosen) {
        // (Its corners round the trough's own centre.)
        paint_plate(g, *slot, px(sz.radius - 1.0 - sz.ground), &p, enabled);
        crate::review::frame(g, *slot);
    }
    crate::review::own_grid(g, outer);
    // (Every name in the text's own colour, the chosen one's heavier:
    // greyed, the ones not chosen read as switched off, and could not
    // be told from a control that can't be used: Opus, of the poll's
    // pictures. One that can't be used draws itself so, in the colour
    // for text that can't be used; it isn't faded besides.)
    let ink = if enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
    for (i, ((seg, r), slot)) in segments.iter().zip(&rects).zip(&at).enumerate() {
        let on = i == chosen;
        let font = style::ui_font(if on { CHOSEN } else { FontWeight::NORMAL });
        let (mark, name_room, count) = parts(seg, sz);
        let mark = if marks { mark } else { 0.0 };
        // What it holds, as one group with the same gaps whichever it
        // is, centred in its place: its name as wide as it is drawn
        // (the room a segment has was measured at the chosen weight, so the
        // segment never changes width; a lighter name leaves a little
        // more either side of the group, and its mark and its count
        // stay as near it as the chosen one's).
        let drawn = f32::from(style::text_width(seg.name.clone(), sz.font, font.clone())).ceil().min(name_room);
        let room = f32::from(slot.size.width) - 2.0 * if marks { LEAST_PAD } else { TIGHT_PAD };
        let badge = seg.badge();
        let disc = badge.as_deref().map_or(0.0, |b| count_width(b, sz));
        // (Centred as what is drawn: a name with nothing waiting sits
        // in the middle of its place, and moves over when its first
        // count comes. Kept to one side for a count that wasn't there,
        // it looked off its centre all the time it had none. The room
        // `count` keeps is for what fits, not for where things go.)
        let _ = count;
        let around = mark + if mark > 0.0 { MARK_GAP } else { 0.0 } + if disc > 0.0 { GAP + disc } else { 0.0 };
        let name_width = drawn.min((room - around).max(0.0));
        let mut x = slot.center().x - px((around + name_width) / 2.0);
        // (Its name's line, on a device pixel: the mark is centred on
        // that line's capitals and the count stands on it, or each is a
        // pixel off the name at some scales, as the review found.)
        let baseline = style::baseline_at(g, slot.center().y, sz.font);
        if let Some(which) = seg.mark.filter(|_| marks) {
            let middle = point(x + px(mark / 2.0), baseline - px(sz.font * style::CAP_HEIGHT / 2.0));
            paint_mark(g, which, middle, sz.mark, ink);
            x += px(mark + MARK_GAP);
        }
        if crate::review::recording(g) && drawn > name_width + 0.5 {
            crate::review::cut_name(g, *r, &seg.name);
        }
        let text = style::label_on_baseline_fit(g, x, baseline, style::Align::Left, seg.name.clone(), sz.font, font, ink, px(name_width + 1.0));
        if on && focused {
            focus_underline(g, text, &p);
        }
        if let Some(badge) = badge {
            // The disc, its middle on the middle of the name's capitals
            // (which is the plate's), on whole device pixels, and its
            // numerals in its middle, as a count in a disc is read: a
            // box of its own to the layout review, which would else
            // hold smaller numerals to the name's line, where they sat
            // low in the disc (Gemini's second look). What can't be
            // used counts nothing urgent: a disc in the colour its
            // name is in, its numerals the trough's ground showing
            // through.
            let (h, size) = sz.count;
            let tone = if seg.waiting { p.hue(olo_theme::Hue::Orange) } else { p.notice };
            // (On the asking orange, numerals of a warm near-black: white
            // was about 2.6 to 1 there, and smeared at 1×. Opus's look.)
            let ink_on = |tone| if seg.waiting { style::c([0x2b, 0x16, 0x00, 0xff]) } else { style::ink_on(tone) };
            let (ground, numerals) = if enabled { (style::c(tone), ink_on(tone)) } else { (ink, style::shade(&p, p.panel_background, 1.1)) };
            let middle = point(x + px(name_width + GAP + disc / 2.0), baseline - px(sz.font * style::CAP_HEIGHT / 2.0));
            let disc_at = style::snap_centred(g, middle, disc, h);
            g.paint_quad(fill(disc_at, ground).corner_radii(disc_at.size.height / 2.0));
            crate::review::fill(g, disc_at);
            // (The box: the disc's width, the plate's height, so the
            // numerals' whole line is in it at every scale.)
            crate::review::container(g, Bounds::from_corners(point(disc_at.left(), slot.top()), point(disc_at.right(), slot.bottom())));
            style::label(g, disc_at.center().x, disc_at.center().y, style::Align::Center, badge.clone(), size, style::ui_font(FontWeight::BOLD), numerals);
        }
        g.agent(agent::Node {
            role: "Segment",
            name: Some(seg.name.clone()),
            value: seg.badge().map(Into::into),
            bounds: *r,
            enabled,
            clickable: enabled,
            focused: on && focused,
            selected: Some(on),
            ..Default::default()
        });
    }
}

/// A mark, centred on `middle` in a box of `size` points, in whole
/// device lines: nothing of it between two pixels at any scale.
pub(crate) fn paint_mark(g: &mut Painter, which: Mark, middle: olo_interface::Point<Pixels>, size: (f32, f32), color: olo_interface::Hsla) {
    let s = g.scale_factor();
    let (w, h) = ((size.0 * s).round(), (size.1 * s).round());
    let b = style::snap_centred(g, middle, w / s, h / s);
    paint_mark_from(g, which, ((f32::from(b.left()) * s).round(), (f32::from(b.top()) * s).round()), size, color);
}

/// How many device lines across and down `which` of an `Indicator`'s
/// marks is in a box of `size` points at `scale`: an odd number of each
/// at every scale, so it has a middle line and what is drawn about it
/// is as far from one side as from the other (twenty lines across at
/// 1.5×: a row of keys a pixel nearer one side, Gemini's review); but
/// the keyboard where its keys are two lines wide ([`key_lines`]),
/// which is an even number across, so rows of them can be centred.
pub(crate) fn mark_lines(which: Mark, size: (f32, f32), scale: f32) -> (i32, i32) {
    let line = style::line_pixels(scale);
    let odd = |v: f32| 2 * ((v - 1.0) / 2.0).round() as i32 + 1;
    let across = size.0 * scale / line;
    (if which == Mark::Keyboard && key_lines(scale) % 2 == 0 { 2 * (across / 2.0).round() as i32 } else { odd(across) }, odd(size.1 * scale / line))
}

/// How many device lines wide a key of [`Mark::Keyboard`] is: one
/// where a line is a point, two where it is two thirds of one (single
/// lines there were specks that an OFF mark lost: Opus's review).
fn key_lines(scale: f32) -> i32 {
    ((scale / style::line_pixels(scale)).round() as i32).max(1)
}

/// [`paint_mark`], its box's corner at the device pixel `corner`: for
/// marks set in a row by device pixels (an `Indicator`'s: one of
/// [`mark_lines`] is drawn from that corner, whatever its box's size
/// came to).
pub(crate) fn paint_mark_from(g: &mut Painter, which: Mark, corner: (f32, f32), size: (f32, f32), color: olo_interface::Hsla) {
    let s = g.scale_factor();
    let line = style::line_pixels(s);
    let (w, h) = ((size.0 * s).round(), (size.1 * s).round());
    let (x0, y0) = corner;
    let at = |x: f32, y: f32| point(px((x0 + x) / s), px((y0 + y) / s));
    let stroke = |g: &mut Painter, points: &[(f32, f32)]| {
        // (A slanting stroke as wide as a level one looks thinner: a
        // quarter more, or the pulse is lighter than the terminal.)
        let mut path = PathBuilder::stroke(px(line * 1.25 / s));
        for (i, &(x, y)) in points.iter().enumerate() {
            // (A stroke's middle on the middle of a device line.)
            let to = at(x.floor() + line / 2.0, y.floor() + line / 2.0);
            if i == 0 { path.move_to(to) } else { path.line_to(to) }
        }
        if let Ok(path) = path.build() {
            g.paint_path(path, color);
        }
    };
    match which {
        Mark::Terminal => {
            // A screen's outline, its corners round, and on it a prompt:
            // an angle and a line after it.
            let frame = Bounds::from_corners(at(0.0, 0.0), at(w, h));
            g.paint_quad(quad(frame, px(2.0), olo_interface::transparent_black(), px(line / s), color, BorderStyle::Solid));
            // (The angle in squares a line wide, stepping in to its
            // point and out again: whole device pixels, where a slanted
            // stroke this small was a smudge at 1×.)
            let steps = ((h * 0.2 / line).round().max(1.0)) as i32;
            let (left, mid) = ((w * 0.24 / line).round() * line, ((h - line) / 2.0 / line).round() * line);
            let mut square = |x: f32, y: f32, across: f32| style::fill_rect(g, Bounds::from_corners(at(x, y), at(x + across, y + line)), color);
            for k in -steps..=steps {
                square(left + (steps - k.abs()) as f32 * line, mid + k as f32 * line, line);
            }
            let under = mid + steps as f32 * line;
            square(left + (steps as f32 + 2.0) * line, under, ((w * 0.26) / line).round().max(2.0) * line);
        }
        Mark::Pulse => {
            // A level line that leaps once: up, down past it, and back.
            let mid = (h / 2.0).floor();
            stroke(g, &[(0.0, mid), (w * 0.26, mid), (w * 0.42, 0.0), (w * 0.60, h - line), (w * 0.74, mid), (w - line, mid)]);
        }
        // The three an `Indicator` shows. Each fills its box (as tall
        // as the others, on one foot) and is of squares a device line
        // across and no other stroke: nothing of them is smoothed at
        // any scale. (The first drawings, to both design reviews: a
        // bridge of three piers read as a table and one of an arch in
        // a frame as a doorway, its curve grey at 1×; a book of two
        // plain pages as two columns; and at 1.5× the keys were
        // thicker than the keyboard's own outline.)
        Mark::Keyboard | Mark::Bridge | Mark::Book => {
            let (cols, rows) = mark_lines(which, size, s);
            // (So many lines across and down, from a line's place.)
            let mut cells = |x: i32, y: i32, across: i32, down: i32| {
                if across > 0 && down > 0 {
                    style::fill_rect(g, Bounds::from_corners(at(x as f32 * line, y as f32 * line), at((x + across) as f32 * line, (y + down) as f32 * line)), color);
                }
            };
            match which {
                Mark::Keyboard => {
                    // Its outline, a corner's square left out at each
                    // corner; two rows of keys, the second half a key
                    // along and a key shorter; and the space bar under
                    // them, two lines in from the keys' ends: one
                    // drawing at every scale. A key is a line wide
                    // and as far from the next where a line is a
                    // point (five over four), two lines wide and two
                    // from the next where there are more lines than
                    // points (four dashes over three at 1.5×). What
                    // was drawn there before, to the reviews: keys of
                    // one line three lines apart, a screen of dotted
                    // text; four rows of them two apart, a speaker's
                    // grille; three rows four apart, an empty tray.
                    cells(1, 0, cols - 2, 1);
                    cells(1, rows - 1, cols - 2, 1);
                    cells(0, 1, 1, rows - 2);
                    cells(cols - 1, 1, 1, rows - 2);
                    let key = key_lines(s);
                    let apart = 2 * key;
                    let below = ((2.0 * s / line).round() as i32).max(2);
                    let keys = (cols - 4 - key) / apart + 1;
                    let across = (keys - 1) * apart + key;
                    let left = (cols - across) / 2;
                    let top = 1 + (rows - 2 - (2 * below + 1)) / 2;
                    for k in 0..keys {
                        cells(left + k * apart, top, key, 1);
                    }
                    for k in 0..keys - 1 {
                        cells(left + apart / 2 + k * apart, top + below, key, 1);
                    }
                    cells(left + 2, top + 2 * below, across - 4, 1);
                }
                Mark::Bridge => {
                    // A bridge hung from two towers, each a fifth of
                    // the way in from its end and standing from the
                    // top to the foot: the roadway across the whole,
                    // two thirds of the way down, so the towers stand
                    // on under it as piers (a deck held up off the
                    // ground: what says "over water" at this size);
                    // the cable from two lines under each tower's
                    // top, hanging between them in a shallow U (level
                    // over the middle, two lines of light between it
                    // and the roadway), and run down outside each
                    // tower to the roadway's end; no hangers.
                    //
                    // Six rounds of two design reviews, and what each
                    // drawing was to them: three piers, a table; an
                    // arch in a frame, a doorway; the towers at the
                    // ends with the cable between them, a letter M
                    // (three times: from the tops to a point, with
                    // the towers taller and a level run, with
                    // hangers; two hangers under the level run closed
                    // a box); this one, to one review a sawhorse with
                    // arrowheads; the roadway low and the side spans
                    // steeper, perfect to one and a crown or a cat's
                    // head to the other; no side spans and a deeper
                    // sag, as that other then asked, an M on an
                    // underline to both. Shown the last three side by
                    // side at true size, one chose this and one the
                    // low roadway with this second: it is the one
                    // each has passed. Don't redraw it without
                    // showing candidates side by side at true size.
                    let tower = (cols as f32 * 0.2).round() as i32;
                    let far = cols - 1 - tower;
                    let road = (rows as f32 * 0.64).round() as i32;
                    let under = (rows as f32 * 0.18).round() as i32;
                    cells(tower, 0, 1, rows);
                    cells(far, 0, 1, rows);
                    cells(0, road, cols, 1);
                    // (A run of the cable from `under`: so many lines
                    // down over so many columns, and which of them
                    // column `k` has, never none.)
                    let part = |fall: i32, over: i32, k: i32| {
                        let (from, next) = (under + k * fall / over, under + (k + 1) * fall / over);
                        (from, (next - from).max(1))
                    };
                    // Between the towers: down as far again as it is
                    // under their tops, over the first and last
                    // quarter and a little, and level between: in
                    // the same proportion at every scale (by a curve
                    // of its own at 1.5× it fell further and was
                    // level over three lines of eleven: a V, and
                    // another drawing than at 1×, to the last design
                    // review).
                    let between = far - tower - 1;
                    let down = (between as f32 * 0.28).round() as i32;
                    for k in 0..down {
                        let (from, lines) = part(under, down, k);
                        cells(tower + 1 + k, from, 1, lines);
                        cells(far - 1 - k, from, 1, lines);
                    }
                    cells(tower + 1 + down, under + under, between - 2 * down, 1);
                    // Outside a tower: to the roadway at its end.
                    for k in 0..tower {
                        let (from, lines) = part(road - under, tower, k);
                        cells(tower - 1 - k, from, 1, lines);
                        cells(far + 1 + k, from, 1, lines);
                    }
                }
                _ => {
                    // An open book: two pages whose head and foot dip
                    // toward the fold beside it, in one step (a line
                    // down over a line at 1× and 2×; two down over two
                    // where a line is under a point: one alone didn't
                    // show at 1.5× and the book was a window of two
                    // panes, and two a step at a time were pages that
                    // curl, another drawing than at 1×), the fold
                    // between them, and two lines of text a page, a
                    // line clear of its edge and of the fold.
                    let fold = cols / 2;
                    let dip = ((s / line).round() as i32).max(1);
                    let inner = fold - dip;
                    for mirrored in [false, true] {
                        let x = |c: i32| if mirrored { cols - 1 - c } else { c };
                        cells(x(0), 0, 1, rows - dip);
                        for c in 1..fold {
                            let down = if c < inner { 0 } else { dip };
                            cells(x(c), down, 1, 1);
                            cells(x(c), rows - 1 - dip + down, 1, 1);
                        }
                        // (The step itself, where it is more than a line.)
                        cells(x(inner), 1, 1, dip - 1);
                        cells(x(inner), rows - dip, 1, dip - 1);
                        for row in [(rows as f32 * 0.3).round() as i32, (rows as f32 * 0.58).round() as i32] {
                            cells(if mirrored { fold + 2 } else { 2 }, row, fold - 3, 1);
                        }
                    }
                    cells(fold, dip, 1, rows - dip);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_interface::size;

    fn outer(x: f32, w: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(10.0)), size: size(px(w), px(20.0)) }
    }

    #[test]
    fn an_indicators_mark_is_an_odd_number_of_lines_each_way_at_every_scale() {
        // (So each has a middle line: a keyboard's keys, a bridge's
        // towers and a book's pages as far from one side as from the
        // other. Twenty lines across at 1.5× was a row of keys a
        // pixel nearer one side.)
        for (scale, lines) in [(1.0, (13, 11)), (1.25, (17, 13)), (1.5, (19, 17)), (1.75, (23, 19)), (2.0, (13, 11)), (3.0, (13, 11))] {
            assert_eq!(mark_lines(Mark::Book, (13.0, 11.0), scale), lines, "at {scale}");
        }
        for scale in [1.0_f32, 1.1, 1.25, 1.33, 1.5, 1.66, 1.75, 2.0, 2.25, 2.5, 3.0] {
            for which in [Mark::Book, Mark::Bridge] {
                let (across, down) = mark_lines(which, (13.0, 11.0), scale);
                assert!(across % 2 == 1 && down % 2 == 1, "{across} by {down} at {scale}");
            }
            // (The keyboard: as its keys are, so rows of them centre.)
            let (across, down) = mark_lines(Mark::Keyboard, (13.0, 11.0), scale);
            assert!(across % 2 == key_lines(scale) % 2 && down % 2 == 1, "{across} by {down} at {scale}");
        }
        assert_eq!(mark_lines(Mark::Keyboard, (13.0, 11.0), 1.5), (20, 17));
    }

    #[test]
    fn segments_share_the_width_on_device_pixels() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for (x, w) in [(0.0, 201.0), (7.0, 188.0), (3.5, 157.0)] {
                for n in 1..=4 {
                    let b = outer(x, w);
                    let at = places(scale, b, &vec![1.0 / n as f32; n]);
                    assert_eq!(at.len(), n);
                    // They tile it: no gap, none past its end.
                    assert_eq!(at[0].left(), b.left());
                    assert_eq!(at[n - 1].right(), b.right());
                    for pair in at.windows(2) {
                        assert_eq!(pair[0].right(), pair[1].left());
                        // An inner edge is on a device pixel.
                        let edge = f32::from(pair[1].left()) * scale;
                        assert!((edge - edge.round()).abs() < 1e-3, "{edge} at {scale}");
                    }
                    // And none is over a device pixel off its share.
                    for r in &at {
                        assert!((f32::from(r.size.width) - w / n as f32).abs() <= 1.0 / scale + 1e-3);
                    }
                }
            }
        }
    }

    #[test]
    fn a_plate_is_the_same_size_anywhere_and_as_far_from_the_trough_on_every_side() {
        for scale in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            for ground in [1.0_f32, 2.0] {
                for (x, w) in [(0.0, 201.0), (7.0, 188.0), (3.5, 157.0)] {
                    for n in 1..=4 {
                        // (As the control is painted: its own edges on device pixels.)
                        let snap = |v: f32| px((v * scale).round() / scale);
                        let b = Bounds::from_corners(point(snap(x), snap(10.0)), point(snap(x + w), snap(30.0)));
                        let at = slots(scale, b, &vec![1.0 / n as f32; n], ground);
                        let dev = |v: Pixels| f32::from(v) * scale;
                        let near = |a: f32, b: f32| (a - b).abs() < 1e-2;
                        // From the trough's line (one of the kit's lines in) to the plate, all round.
                        let round = style::line_pixels(scale) + (ground * scale).round();
                        assert!(near(dev(at[0].left()) - dev(b.left()), round), "left at {scale}");
                        assert!(near(dev(b.right()) - dev(at[n - 1].right()), round), "right at {scale}");
                        for slot in &at {
                            assert!(near(dev(slot.top()) - dev(b.top()), round) && near(dev(b.bottom()) - dev(slot.bottom()), round), "top and foot at {scale}");
                            // Every edge of a plate's place is on a device pixel.
                            for edge in [slot.left(), slot.right(), slot.top(), slot.bottom()] {
                                assert!(near(dev(edge), dev(edge).round()), "{} at {scale}", dev(edge));
                            }
                        }
                        // Twice the ground between two places; and no place over a device pixel wider than another.
                        for pair in at.windows(2) {
                            assert!(near(dev(pair[1].left()) - dev(pair[0].right()), 2.0 * (ground * scale).round()), "between two at {scale}");
                        }
                        let widths: Vec<f32> = at.iter().map(|r| dev(r.size.width)).collect();
                        let (least, most) = (widths.iter().cloned().fold(f32::MAX, f32::min), widths.iter().cloned().fold(0.0, f32::max));
                        assert!(most - least <= 1.0 + 1e-2, "{widths:?} at {scale}");
                    }
                }
            }
        }
    }

    #[test]
    fn one_that_counts_keeps_a_discs_room_while_nothing_waits() {
        let sz = SIZES;
        let (plain, counting, three) = (Segment::new("Activity"), Segment::new("Activity").count(0), Segment::new("Activity").count(3));
        assert!(content_width(&counting, sz) > content_width(&plain, sz));
        assert_eq!(content_width(&counting, sz), content_width(&three, sz));
        // (So the control is as wide before the first one comes as after.)
        assert_eq!(segmented_width(&[Segment::new("Sessions"), counting]), segmented_width(&[Segment::new("Sessions"), three]));
    }

    #[test]
    fn a_long_name_takes_what_a_short_one_can_spare_before_it_is_cut() {
        let sz = SIZES;
        let segments = [Segment::new("Messages").count(2), Segment::new("Contacts")];
        let need = |seg: &Segment| {
            let (_, name, count) = parts(seg, sz);
            name + if count > 0.0 { GAP + count } else { 0.0 } + 2.0 * TIGHT_PAD + 2.0 * sz.ground
        };
        let (long, short) = (need(&segments[0]), need(&segments[1]));
        assert!(long > short);
        // With room for two of the longer: halves.
        let wide = 2.0 * long + 2.0;
        assert_eq!(shares(&segments, sz, wide), vec![0.5, 0.5]);
        // Too narrow for halves, wide enough for what each needs: each
        // has it, and half of what is over.
        let width = long + short + 2.0 + 6.0;
        let split = shares(&segments, sz, width);
        assert!((split.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        let inside = width - 2.0;
        assert!((split[0] * inside - (long + 3.0)).abs() < 1e-2 && (split[1] * inside - (short + 3.0)).abs() < 1e-2, "{split:?}");
        // Too narrow even for that: halves again (and the review says the name is cut).
        assert_eq!(shares(&segments, sz, long + short - 4.0), vec![0.5, 0.5]);
    }

    #[test]
    fn a_count_over_ninety_nine_is_cut() {
        assert_eq!(Segment::new("A").count(0).badge(), None);
        assert_eq!(Segment::new("A").count(7).badge().as_deref(), Some("7"));
        assert_eq!(Segment::new("A").count(250).badge().as_deref(), Some("99+"));
    }
}
