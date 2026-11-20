//! A list as a board: its sections side by side, each a lane of its
//! to-dos as cards (the title, what it has, when it's due), dragged
//! within a lane to order them and between lanes to move them (Trello's
//! board; Todoist's and Reminders' columns are sections too, one model
//! for the list and the board). A lane of those in no section comes
//! first where there are any.
//!
//! In Olo's own manner: each lane a sunken well under its heading, as a
//! list box is; its cards plain sheets with a fine edge, square as the
//! rest of the window is, each as tall as what it holds. The lanes share
//! the board's width (down to a least: then the board scrolls).
//!
//! Where a drag would drop is worked out from the cards' heights and
//! where the lanes are scrolled to, never from where a card was last
//! painted (a scrolled lane is moved, not painted again).

use olo_kit::{
    Bounds, ClickEvent, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, Pixels,
    ScrollHandle, Styled as _, agent, canvas, div, metrics as m, point, px, size, style, theme,
    widgets::{Button, ScrollView, StringView},
};
use olo_todo::view::{Place, overdue};

use crate::{
    Drag, Lane, Msg, ToDoApp, list,
    marks::{self, Glyph, Look, Mark},
    view::{Asking, Naming},
    words,
};

/// A lane's width: what it's given of the board's, between these.
pub const LANE_LEAST: f32 = 176.0;
const LANE_MOST: f32 = 288.0;
/// The gap between lanes.
pub const GAP: f32 = 12.0;
/// A lane's heading, and the gap between its cards.
const HEAD: f32 = 26.0;
const CARD_GAP: f32 = 5.0;
/// A card's inset in its lane's well (inside the well's own two lines),
/// and its own content's.
const INSET: f32 = 5.0;
const PAD: f32 = 8.0;
/// A line of a card: its title's, and the one under them (its due, its
/// marks), all on one pitch, so cards side by side are level line for
/// line.
const LINE: f32 = 16.0;
/// A press moved this far is a drag.
const DRAG: f32 = 4.0;

/// How wide each of `n` lanes is in a board `width` wide.
pub fn lane_width(n: usize, width: f32) -> f32 {
    let n = n.max(1) as f32;
    // (Not in whole points: the last lane ends where the board does, with
    // the header's switch over it; each lane is drawn on whole pixels.)
    ((width - (n - 1.0) * GAP) / n).clamp(LANE_LEAST, LANE_MOST)
}

/// The most lines a card gives its title.
const LINES: usize = 3;

/// A title on lines `room` wide, three at the most: each as many words
/// as fit (the last takes what's left, cut with an ellipsis where it's
/// drawn if that's still too much). Every line is filled before the
/// next starts, but that a last line of one word takes the word before
/// it where the line it leaves stays the longer ("… and draft / the
/// goals", not "… and draft the / goals"; but "Write up the design /
/// review": taken down there, "design" left room over it that read, three
/// reviews running, as a line cut short).
fn wrap(title: &str, size: f32, font: &olo_kit::Font, room: f32) -> Vec<String> {
    let wide = |words: &[&str]| style::text_width_app(&words.join(" "), size, font);
    let words: Vec<&str> = title.split_whitespace().collect();
    let mut lines: Vec<Vec<&str>> = vec![Vec::new()];
    for word in &words {
        let line = lines.last_mut().expect("a line");
        let mut with = line.clone();
        with.push(word);
        if line.is_empty() || wide(&with) <= room || lines.len() == LINES {
            lines.last_mut().expect("a line").push(word);
        } else {
            lines.push(vec![word]);
        }
    }
    let n = lines.len();
    if n >= 2 && lines[n - 1].len() == 1 && lines[n - 2].len() >= 2 {
        let (mut above, mut last) = (lines[n - 2].clone(), lines[n - 1].clone());
        last.insert(0, above.pop().expect("a word"));
        if wide(&last) <= room && wide(&above) >= wide(&last) {
            (lines[n - 2], lines[n - 1]) = (above, last);
        }
    }
    lines.into_iter().map(|l| l.join(" ")).collect()
}

/// The room a card's title has, in a lane `lane` wide.
fn title_room(lane: f32) -> f32 {
    lane - 4.0 - 2.0 * INSET - PAD - list::CHECK - 8.0 - PAD
}

/// The marks a to-do's card shows, in order.
fn marks_of(t: &olo_todo::ToDo) -> Vec<Mark> {
    [(t.notes.is_empty(), Mark::Notes), (t.attachments.is_empty(), Mark::Attachments), (t.remind.is_none(), Mark::Reminder)].into_iter().filter(|(none, _)| !none).map(|(_, m)| m).collect()
}

const MARK_GAP: f32 = 5.0;

fn marks_width(n: usize) -> f32 {
    if n == 0 { 0.0 } else { n as f32 * marks::MARK + (n - 1) as f32 * MARK_GAP }
}

/// What a card says under its title, and whether in red: when it was
/// done, once it's among what's done (not while it's struck through a
/// moment where it was: its card would grow a line and shrink again);
/// else when it's due (red: overdue).
fn foot(v: &ToDoApp, t: &olo_todo::ToDo) -> Option<(String, bool)> {
    let settled = !t.open() && !v.lingering.contains_key(&t.uid);
    match (t.done_at.filter(|_| settled), &t.due) {
        (Some(at), _) => Some((olo_kit::tr!("Done {}", words::in_words(at.to_zoned(v.zone.clone()).date(), v.now.date())), false)),
        (None, Some(due)) => Some((words::due(due, &v.now, v.h24), t.open() && overdue(due, &v.now))),
        (None, None) => None,
    }
}

/// How a to-do's card is laid out in a lane `lane` wide: its title's
/// lines, and whether a line follows them (its foot, with its marks at
/// that line's right; or its marks alone, where they don't fit after its
/// title's last line). Its marks are at the card's right edge always, on
/// its last line: one column of them down a lane.
struct Laid {
    lines: Vec<String>,
    under: bool,
}

fn lay(v: &ToDoApp, t: &olo_todo::ToDo, lane: f32) -> Laid {
    let font = style::ui_font(FontWeight::NORMAL);
    let room = title_room(lane);
    let lines = wrap(&t.title, style::PLAIN_SIZE, &font, room);
    let marks = marks_of(t).len();
    if foot(v, t).is_some() {
        return Laid { lines, under: true };
    }
    if marks == 0 {
        return Laid { lines, under: false };
    }
    let last = lines.last().map_or(0.0, |l| style::text_width_app(l, style::PLAIN_SIZE, &font));
    Laid { lines, under: last + 8.0 + marks_width(marks) > room }
}

/// How tall a to-do's card is in a lane `lane` wide.
pub fn card_height(v: &ToDoApp, t: &olo_todo::ToDo, lane: f32) -> f32 {
    let laid = lay(v, t, lane);
    PAD + laid.lines.len() as f32 * LINE + if laid.under { LINE } else { 0.0 } + PAD
}

/// Where the board's lanes and cards are, in the window, as scrolled
/// now.
struct Geo {
    area: Bounds<Pixels>,
    left: Pixels,
    top: Pixels,
    lane: f32,
}

fn geo(v: &ToDoApp) -> Geo {
    let area = v.board_area.get();
    Geo { area, left: area.left() + v.board_scroll.offset().x, top: area.top(), lane: v.lane.get() }
}

fn lane_scroll(v: &ToDoApp, lane: &Lane) -> ScrollHandle {
    v.column_scrolls.borrow_mut().entry(lane.clone()).or_default().clone()
}

/// Whether a new to-do is being typed into lane `which`: its card to be
/// is at the lane's top (a lane's +, or New To-Do on a board: its
/// first lane's).
fn adding(v: &ToDoApp, which: &Lane) -> bool {
    matches!(which, Lane::Section(s) if v.entry_section.as_ref() == Some(s))
}

/// The due read from what's typed for a new to-do, as its card will say
/// it, and whether at a time (it will remind then), where one was read.
fn typed_due(v: &ToDoApp) -> Option<(String, bool)> {
    match (&v.typed.due, &v.typed.said) {
        (Some(due), Some(_)) => Some((words::due(due, &v.now, v.h24), !due.is_date())),
        _ => None,
    }
}

/// The blue of what's read from a new to-do's words and not yet kept
/// (the words themselves in its field, the due they say): the look's,
/// made to read on the ground it's on, a board's card or the list's
/// sheet.
pub fn due_ink(p: &olo_theme::Palette, on_card: bool) -> olo_theme::Rgba {
    olo_theme::readable(p.hue(olo_theme::Hue::Blue), if on_card { grounds(p).1 } else { p.document_background }, 4.5)
}

/// A new to-do's card while it's typed: a card of one line, its field
/// where a title is (and a line under it for the due read from its
/// words).
fn editor_height(v: &ToDoApp) -> f32 {
    2.0 * PAD + LINE + if typed_due(v).is_some() { LINE } else { 0.0 }
}

/// The field in a new to-do's card: as tall as its words need, about
/// the middle of a card's first line (a control's height, it reached
/// into the line under it, where the due read from its words is).
const EDIT_FIELD: f32 = 18.0;
/// The × after that due: its square, in the card's second line.
const EDIT_KEEP: f32 = 14.0;

/// Whether a point in the window is on the new to-do's card.
fn on_editor(v: &ToDoApp, at: olo_kit::Point<Pixels>) -> bool {
    let Some(k) = v.columns.iter().position(|(which, _)| adding(v, which)) else { return false };
    let g = geo(v);
    let x = f32::from(at.x - g.left) - k as f32 * (g.lane + GAP);
    let y = f32::from(at.y - g.top - lane_scroll(v, &v.columns[k].0).offset().y) - HEAD - 2.0;
    (0.0..=g.lane).contains(&x) && (0.0..=INSET + editor_height(v) + CARD_GAP / 2.0).contains(&y)
}

/// A lane's cards laid out: where each is under the first's top and how
/// tall, and where each one's row starts (its card and the gap under
/// it: `sized_rows`). Kept while the board and the lane's width are what
/// it was made for: laying a card out measures its title's words, and a
/// board of nine hundred cards, laid out again each time the window said
/// what it shows, took 40 ms to choose a card on (measured, 2026-11-20).
#[derive(Clone)]
pub struct LaneLayout {
    key: (u64, u32, u32),
    cards: std::rc::Rc<[(f32, f32)]>,
    tops: std::rc::Rc<[f32]>,
}

fn lane_layout(v: &ToDoApp, k: usize, lane: f32) -> LaneLayout {
    let key = (v.board_version, lane.to_bits(), v.tops_scale.to_bits());
    if let Some(Some(kept)) = v.lane_layouts.borrow().get(k)
        && kept.key == key
    {
        return kept.clone();
    }
    let mut top = 0.0;
    let cards: Vec<(f32, f32)> = v.columns[k]
        .1
        .iter()
        .map(|&i| {
            let h = v.book.items.get(i).map_or(2.0 * PAD + LINE, |item| card_height(v, &item.todo, lane));
            let at = (top, h);
            top += h + CARD_GAP;
            at
        })
        .collect();
    let laid = LaneLayout { key, tops: olo_kit::row_tops(cards.iter().map(|(_, h)| h + CARD_GAP)), cards: cards.into() };
    let mut all = v.lane_layouts.borrow_mut();
    if all.len() <= k {
        all.resize(k + 1, None);
    }
    all[k] = Some(laid.clone());
    laid
}

/// Where lane `k`'s first card starts in its well: under the card a new
/// one is typed in, where the lane has it.
fn cards_origin(v: &ToDoApp, k: usize) -> f32 {
    INSET + if adding(v, &v.columns[k].0) { editor_height(v) + CARD_GAP } else { 0.0 }
}

/// Lane `k`'s cards: each one's top (in the lane's cards) and height.
fn card_tops(v: &ToDoApp, k: usize, lane: f32) -> Vec<(f32, f32)> {
    let origin = cards_origin(v, k);
    lane_layout(v, k, lane).cards.iter().map(|(top, h)| (top + origin, *h)).collect()
}

/// The lane a point is over, and the place among its cards it would
/// drop at (not counting `dragged`, where it's that lane's).
fn target(v: &ToDoApp, at: olo_kit::Point<Pixels>, dragged: &str) -> Option<(usize, usize)> {
    let g = geo(v);
    if v.columns.is_empty() {
        return None;
    }
    let k = (f32::from(at.x - g.left) / (g.lane + GAP)).floor().clamp(0.0, v.columns.len() as f32 - 1.0) as usize;
    let (which, items) = &v.columns[k];
    let y = f32::from(at.y - g.top - lane_scroll(v, which).offset().y) - HEAD - 2.0;
    // The place: before the first card whose middle the point is above
    // (its own card not counted: it's what moves).
    let tops = card_tops(v, k, g.lane);
    let mut slot = 0;
    for (n, &i) in items.iter().enumerate() {
        if v.book.items.get(i).is_some_and(|it| it.todo.uid == dragged) {
            continue;
        }
        let (top, h) = tops.get(n).copied().unwrap_or((0.0, 0.0));
        if y < top + h / 2.0 {
            break;
        }
        slot += 1;
    }
    Some((k, slot))
}

/// The card a point is on: its lane, its place, and whether the point
/// is on its check circle.
fn card_at(v: &ToDoApp, at: olo_kit::Point<Pixels>) -> Option<(usize, usize, bool)> {
    let g = geo(v);
    if !g.area.contains(&at) {
        return None;
    }
    let x = f32::from(at.x - g.left);
    let k = (x / (g.lane + GAP)).floor();
    if k < 0.0 || k as usize >= v.columns.len() || x - k * (g.lane + GAP) > g.lane {
        return None;
    }
    let k = k as usize;
    let which = &v.columns[k].0;
    if f32::from(at.y - g.top) < HEAD {
        return None;
    }
    let y = f32::from(at.y - g.top - lane_scroll(v, which).offset().y) - HEAD - 2.0;
    let (i, (top, _)) = card_tops(v, k, g.lane).into_iter().enumerate().find(|(_, (top, h))| y >= *top && y <= top + h)?;
    let in_x = x - k as f32 * (g.lane + GAP) - 2.0 - INSET;
    Some((k, i, in_x <= PAD + list::CHECK + 6.0 && y - top <= PAD + LINE + 6.0))
}

fn press(v: &mut ToDoApp, e: &MouseDownEvent, cx: &mut Cx<ToDoApp>) {
    // (On the new to-do's card: its field's own. Anywhere else, with
    // nothing typed in it yet, the card goes.)
    // (Nor on a lane's heading: its + has just put the card there.)
    if on_editor(v, e.position) || f32::from(e.position.y - geo(v).top) < HEAD {
        return;
    }
    if v.entry_section.is_some() && v.entry.read(cx).text().is_empty() {
        v.entry_section = None;
        cx.invalidate();
    }
    v.focus.focus(cx);
    let Some((k, i, on_check)) = card_at(v, e.position) else {
        v.select(None, cx);
        return;
    };
    let Some(uid) = v.columns[k].1.get(i).and_then(|&n| v.book.items.get(n)).map(|it| it.todo.uid.clone()) else { return };
    if on_check {
        v.check(&uid, cx);
        return;
    }
    v.select(Some(uid.clone()), cx);
    if e.click_count >= 2 {
        v.details_pane.show_pane();
        v.title.focus(cx);
    } else {
        v.drag = Some(Drag { uid, from: e.position, at: None, to: None });
    }
    cx.invalidate();
}

fn moved(v: &mut ToDoApp, e: &MouseMoveEvent, cx: &mut Cx<ToDoApp>) {
    let Some(drag) = v.drag.clone() else { return };
    if e.pressed_button != Some(MouseButton::Left) {
        return;
    }
    let d = e.position - drag.from;
    if drag.at.is_none() && (f32::from(d.x).powi(2) + f32::from(d.y).powi(2)).sqrt() < DRAG {
        return;
    }
    let to = target(v, e.position, &drag.uid).map(|(k, slot)| (v.columns[k].0.clone(), slot));
    v.drag = Some(Drag { at: Some(e.position), to, ..drag });
    cx.invalidate();
}

fn release(v: &mut ToDoApp, _: &MouseUpEvent, cx: &mut Cx<ToDoApp>) {
    let Some(drag) = v.drag.take() else { return };
    if let (Some(_), Some((lane, slot)), Place::List(id)) = (drag.at, drag.to, v.place.clone()) {
        cx.send(match lane {
            Lane::Section(section) => Msg::Drop(drag.uid, id, section, slot),
            // (On the lane of what's done: it's done.)
            Lane::Done => Msg::DropDone(drag.uid),
        });
    }
    cx.invalidate();
}

/// A lane's ground and a card's face in the look in force: by day the
/// lane a step darker than the panel and the cards the document's
/// white; by night the lane the dark of a well and the cards a step
/// lighter than the panel (cards the dark of a well looked like holes).
fn grounds(p: &olo_theme::Palette) -> (olo_theme::Rgba, olo_theme::Rgba) {
    if p.mode == olo_theme::ThemeMode::Light {
        (olo_theme::mix(p.panel_background, p.panel_text, 0.08), p.document_background)
    } else {
        (p.document_background, olo_theme::mix(p.panel_background, [255, 255, 255, 255], 0.07))
    }
}

/// A to-do's card, drawn: a sheet with a fine edge (the selection's
/// ground while it's chosen), its check circle, its title on a line or
/// two, and under them its due at the left and what it has at the right.
fn paint_card(v: &ToDoApp, g: &mut Painter, b: Bounds<Pixels>, n: usize, ghost: bool) {
    // (By its place among the book's to-dos, as a lane has it: looked
    // for by its name, each card's drawing was a search of them all.)
    let Some(item) = v.book.items.get(n) else { return };
    let p = theme::palette();
    let t = &item.todo;
    let uid = t.uid.as_str();
    let dragged = v.drag.as_ref().is_some_and(|d| d.uid == uid && d.at.is_some()) && !ghost;
    // (What's being dragged stays where it was, faint, its place held:
    // its copy under the pointer is the card now.)
    let selected = v.selected.as_deref() == Some(uid) && !ghost && !dragged;
    let done = !t.open();
    let b = style::snap(g, b);
    let (lane_ground, card) = grounds(&p);
    let face = match (selected, dragged) {
        (_, true) => olo_theme::mix(card, lane_ground, 0.8),
        (true, _) => p.list_selected_background,
        _ => card,
    };
    style::fill_rect(g, b, style::c(face));
    // (Its edge: a step off its face, darker by day, lighter by night; a
    // pale line round every card on a dark lane made a wireframe of it.)
    let edge = if p.mode == olo_theme::ThemeMode::Light { olo_theme::mix(lane_ground, p.panel_text, 0.28) } else { olo_theme::mix(card, [255, 255, 255, 255], 0.1) };
    style::frame_rect(g, b, style::c(if selected { p.control_highlight } else { edge }));
    if ghost {
        // (Lifted: a line of shadow under it and at its right.)
        let line = style::hairline(g);
        let dark = style::c(olo_theme::mix(lane_ground, [0, 0, 0, 255], 0.35));
        style::fill_rect(g, Bounds { origin: point(b.left() + line * 2.0, b.bottom()), size: size(b.size.width, line * 2.0) }, dark);
        style::fill_rect(g, Bounds { origin: point(b.right(), b.top() + line * 2.0), size: size(line * 2.0, b.size.height) }, dark);
    }
    let plain = style::ui_font(FontWeight::NORMAL);
    let hue = p.hue(v.hue_of(&item.list));
    // (Its check's middle on its title's first line.)
    let check = Bounds { origin: point(b.left() + px(PAD), b.top() + px(PAD + LINE / 2.0 - list::CHECK / 2.0)), size: size(px(list::CHECK), px(list::CHECK)) };
    list::paint_check(g, check, hue, !t.open(), face);
    let ink = if selected {
        style::c(p.list_selected_item_text)
    } else if dragged {
        style::disabled_text(&p)
    } else if done {
        style::secondary_text(&p)
    } else {
        style::c(p.panel_text)
    };
    let quiet = if selected {
        style::c(p.list_selected_item_text)
    } else if dragged {
        style::disabled_text(&p)
    } else {
        style::secondary_text(&p)
    };
    let left = check.right() + px(8.0);
    let room = b.right() - px(PAD) - left;
    let laid = lay(v, t, v.lane.get());
    for (i, line) in laid.lines.iter().enumerate() {
        let shown = style::label_fit(g, left, b.top() + px(PAD + LINE * (i as f32 + 0.5)), style::Align::Left, line.clone(), style::PLAIN_SIZE, plain.clone(), ink, room);
        // (Done: struck through, every line of it.)
        if done {
            style::hline(g, shown.left(), shown.right(), shown.center().y, ink);
        }
    }
    let shown = marks_of(t);
    let mut said: Vec<String> = Vec::new();
    // Under its title: its foot (its due; when it was done) at the left.
    // What it has, at the card's right edge on its last line: that foot's,
    // or with none its title's last, or a line of their own where they
    // don't fit after it.
    let base = b.top() + px(PAD + (laid.lines.len() as f32 + 0.5) * LINE);
    if let Some((text, late)) = foot(v, t) {
        let colour = if late && !selected { style::c(p.hue_text(olo_theme::Hue::Red)) } else { quiet };
        let right = b.right() - px(PAD + marks_width(shown.len()) + if shown.is_empty() { 0.0 } else { 8.0 });
        style::label_fit(g, left, base, style::Align::Left, text, style::DETAIL_SIZE, plain.clone(), colour, (right - left).max(px(0.0)));
    }
    if let Some(due) = &t.due {
        let text = words::due_in_words(due, &v.now, v.h24);
        said.push(if t.open() && overdue(due, &v.now) { olo_kit::tr!("overdue, {}", text) } else { olo_kit::tr!("due {}", text) });
    }
    let mut x = b.right() - px(PAD + marks_width(shown.len()));
    let y = if laid.under { base } else { base - px(LINE) };
    for mark in shown {
        let at = Bounds { origin: point(x, y - px(marks::MARK / 2.0)), size: size(px(marks::MARK), px(marks::MARK)) };
        marks::paint(g, mark, at, quiet);
        x = at.right() + px(MARK_GAP);
        said.push(olo_kit::words::tr(mark.word()).into());
    }
    if ghost {
        return;
    }
    g.agent(agent::Node { role: "CheckBox", name: Some(olo_kit::tr!("Done: {}", t.title).into()), bounds: check, enabled: true, clickable: true, checked: Some(!t.open()), ..Default::default() });
    g.agent(agent::Node {
        role: "ListItem",
        name: Some(t.title.clone().into()),
        value: Some(said.join(", ").into()),
        bounds: b,
        enabled: true,
        clickable: true,
        selected: Some(selected),
        ..Default::default()
    });
}

fn card_key(v: &ToDoApp, n: usize) -> u64 {
    let Some(item) = v.book.items.get(n) else { return 0 };
    let t = &item.todo;
    let uid = t.uid.as_str();
    style::key((
        &t.title,
        t.open(),
        v.lingering.contains_key(uid),
        t.due.as_ref().map(|d| words::due(d, &v.now, v.h24)),
        t.due.as_ref().is_some_and(|d| overdue(d, &v.now)),
        t.remind.is_some(),
        t.attachments.len(),
        t.notes.is_empty(),
        v.hue_of(&item.list),
        v.selected.as_deref() == Some(uid),
        v.drag.as_ref().is_some_and(|d| d.uid == uid && d.at.is_some()),
        v.lane.get().to_bits(),
    ))
}

/// A lane's name as it says it.
fn lane_name(lane: &Lane) -> olo_kit::SharedString {
    match lane {
        Lane::Section(Some(s)) => s.clone().into(),
        Lane::Section(None) => "No Section".into(),
        Lane::Done => "Done".into(),
    }
}

/// One lane: its heading (the section, how many, a + for a new to-do in
/// it), and its cards in a well under it.
fn lane(v: &mut ToDoApp, k: usize, lane_w: f32, height: f32, cx: &mut Cx<ToDoApp>) -> Element {
    let p = theme::palette();
    let (which, items) = v.columns[k].clone();
    let laid = lane_layout(v, k, lane_w);
    let origin = cards_origin(v, k);
    let scroll = lane_scroll(v, &which);
    let name = lane_name(&which);
    let section = match &which {
        Lane::Section(s) => s.clone(),
        Lane::Done => None,
    };
    let named = matches!(which, Lane::Section(Some(_)));
    let menu_section = section.clone();
    // (Its name as wide as it is, or as there's room for beside its count
    // and its +; its words start where its cards do.)
    let name_w = f32::from(style::text_width(name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil() + 1.0;
    let add_section = section.clone();
    let head = div()
        .h(px(HEAD))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .pl(px(2.0 + INSET))
        .pr(px(2.0 + INSET))
        .child(div().w(px(name_w.min(lane_w - 80.0))).flex_shrink_0().child(StringView::new(name.clone()).bold(true).cut(true).color(if which != Lane::Section(None) {
            style::c(p.panel_text)
        } else {
            style::secondary_text(&p)
        })))
        .child(div().flex_shrink_0().child(StringView::new(items.len().to_string()).dim(true)))
        .child(div().flex_1())
        // (A new to-do in this section: a card at the lane's top to type
        // it in. None in what's done.)
        .children((which != Lane::Done).then(|| {
            marks::glyph_button(
                format!("lane-add-{k}"),
                Glyph::Plus,
                olo_kit::tr!("New To-Do in {}", name),
                20.0,
                Look::Raised,
                cx.listener(move |v: &mut ToDoApp, _: &MouseDownEvent, cx| {
                    v.entry_section = Some(add_section.clone());
                    v.entry.focus(cx);
                    cx.invalidate();
                }),
            )
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |v: &mut ToDoApp, e: &MouseDownEvent, cx| {
                if !named {
                    return;
                }
                let (Some(section), Place::List(id)) = (menu_section.clone(), v.place.clone()) else { return };
                let menu = olo_kit::Menu::new("Section").item("Rename…", "rename").separator().item("Delete Section", "delete");
                let view = cx.view_ref();
                olo_kit::menu::context_menu(cx, menu, e.position, move |cmd, cx| {
                    let (cmd, id, section) = (cmd.to_owned(), id.clone(), section.clone());
                    view.update(cx, move |v: &mut ToDoApp, cx| match cmd.as_str() {
                        "rename" => Naming::ask(v, Asking::RenameSection(id, section), cx),
                        "delete" => cx.send(Msg::RemoveSection(id, section)),
                        _ => {}
                    });
                });
            }),
        );
    // Its cards, one under another; the line where a drag would drop.
    let typing = adding(v, &which);
    let edit_h = editor_height(v);
    let total = laid.cards.last().map_or(INSET + if typing { edit_h + INSET } else { 0.0 }, |(top, h)| origin + top + h + INSET);
    // (Its cards are on a grid of their own: not held level with the
    // labels of the pane beside the board.)
    let mut cards = div().relative().w_full().h(px(total)).child(canvas(|g, b| olo_kit::review::own_grid(g, b)).absolute().inset_0());
    // (A new to-do typed in: a card at the lane's top, where it will be:
    // the ring a new one has, its field where a card's title is, and
    // under it the due its words were read as, with a way to keep them
    // as words.)
    if typing {
        let due = typed_due(v);
        let font = style::ui_font(FontWeight::NORMAL);
        let left = PAD + list::CHECK + 8.0;
        let face = canvas({
            let due = due.clone();
            move |g, b| {
                let p = theme::palette();
                let b = style::snap(g, b);
                let card = grounds(&p).1;
                style::fill_rect(g, b, style::c(card));
                style::frame_rect(g, b, style::c(p.control_highlight));
                let ring = Bounds { origin: point(b.left() + px(PAD), b.top() + px(PAD + LINE / 2.0 - list::CHECK / 2.0)), size: size(px(list::CHECK), px(list::CHECK)) };
                list::paint_new_circle(g, ring, list::CHECK, style::secondary_text(&p), style::c(card));
                // (Its foot as the card to be will have it: when it's due
                // where a card says so, in the blue of what's read from
                // words and not yet kept; its bell at the right, where a
                // time was typed, which it will ring at.)
                if let Some((text, timed)) = &due {
                    let base = b.top() + px(PAD + 1.5 * LINE);
                    let room = b.right() - px(PAD + marks::MARK + 8.0 + EDIT_KEEP + 2.0) - (b.left() + px(left));
                    style::label_fit(
                        g,
                        b.left() + px(left),
                        base,
                        style::Align::Left,
                        text.clone(),
                        style::DETAIL_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::c(due_ink(&p, true)),
                        room.max(px(0.0)),
                    );
                    if *timed {
                        let at = Bounds { origin: point(b.right() - px(PAD + marks::MARK), base - px(marks::MARK / 2.0)), size: size(px(marks::MARK), px(marks::MARK)) };
                        marks::paint(g, Mark::Reminder, at, style::secondary_text(&p));
                    }
                }
            }
        })
        .paint_key(style::key(&due))
        .absolute()
        .inset_0();
        let empty = v.entry.read(cx).text().is_empty();
        let hint = empty.then(|| div().absolute().inset_0().flex().flex_row().items_center().child(StringView::new("New To-Do").dim(true)));
        let mut editor = div()
            .absolute()
            .left(px(INSET))
            .right(px(INSET))
            .top(px(INSET))
            .h(px(edit_h))
            .child(face)
            .child(div().absolute().left(px(left)).right(px(PAD)).top(px(PAD + (LINE - EDIT_FIELD) / 2.0)).h(px(EDIT_FIELD)).children(hint).child(div().absolute().inset_0().child(&v.entry)));
        // (The way to keep those words as words: an × right after what
        // they were read as, not at the card's corner, where it would be
        // read as doing away with the card.)
        if let Some((text, _)) = &due {
            let room = lane_w - 2.0 * INSET - 4.0 - left - PAD - marks::MARK - 8.0 - EDIT_KEEP - 2.0;
            let after = style::text_width_app(text, style::DETAIL_SIZE, &font).ceil().min(room.max(0.0));
            editor = editor.child(div().absolute().left(px(left + after + 2.0)).top(px(PAD + LINE + (LINE - EDIT_KEEP) / 2.0)).child(marks::glyph_button(
                "keep-words",
                Glyph::Cross,
                "Not a due: keep the words in its title",
                EDIT_KEEP,
                Look::Quiet,
                cx.listener(|_: &mut ToDoApp, _: &MouseDownEvent, cx| cx.send(Msg::KeepWords)),
            )));
        }
        cards = cards.child(editor);
    }
    // (A lane with nothing in it says so: an empty well looked turned off.)
    if items.is_empty() && !typing {
        let said = if which == Lane::Done { "Nothing done yet" } else { "No to-dos" };
        cards = div().relative().w_full().h(px(48.0)).child(div().absolute().left(px(0.0)).right(px(0.0)).top(px(16.0)).flex().flex_row().justify_center().child(StringView::new(said).dim(true)));
    }
    // (Only the cards that show, and some to spare, are made: a lane of
    // hundreds costs what its screenful does. Each is a row as tall as
    // its card and the gap under it.)
    if !items.is_empty() {
        let (made, keyed): (std::rc::Rc<[usize]>, std::rc::Rc<[usize]>) = (items.clone().into(), items.clone().into());
        let (heights, key_heights) = (laid.cards.clone(), laid.cards.clone());
        cards = cards.child(
            olo_kit::sized_rows(laid.tops.clone(), move |near| {
                near.map(|i| {
                    let (n, h) = (made[i], heights[i].1);
                    div().id(i).w_full().child(
                        olo_kit::view_canvas::<ToDoApp>(move |v, g, b| paint_card(v, g, b, n, false))
                            .key_by::<ToDoApp>(move |v| card_key(v, n))
                            .absolute()
                            .left(px(INSET))
                            .right(px(INSET))
                            .top(px(0.0))
                            .h(px(h)),
                    )
                })
                .collect()
            })
            .row_key_by::<ToDoApp>(move |v, i| keyed.get(i).map_or(0, |&n| style::key((card_key(v, n), n, key_heights[i].1.to_bits()))))
            .absolute()
            .left(px(0.0))
            .right(px(0.0))
            .top(px(origin)),
        );
    }
    if let Some(Drag { uid, at: Some(_), to: Some((to, slot)), .. }) = &v.drag
        && *to == which
    {
        // (Before the `slot`th of the others: above that card, or under the last.)
        let dragged = items.iter().position(|&n| v.book.items.get(n).is_some_and(|it| it.todo.uid == *uid));
        let others: Vec<(f32, f32)> = laid.cards.iter().enumerate().filter(|(i, _)| Some(*i) != dragged).map(|(_, (top, h))| (top + origin, *h)).collect();
        let y = match others.get(*slot) {
            // (What's done has its own order: at its top.)
            _ if which == Lane::Done => INSET / 2.0,
            Some((top, _)) => top - CARD_GAP / 2.0,
            None => others.last().map_or(INSET / 2.0, |(top, h)| top + h + CARD_GAP / 2.0),
        };
        cards = cards.child(
            canvas(|g, b| {
                let p = theme::palette();
                let r = style::snap_centred(g, b.center(), f32::from(b.size.width), 2.0);
                style::fill_rect(g, r, style::c(p.control_highlight));
            })
            .absolute()
            .left(px(INSET))
            .right(px(INSET))
            .top(px((y - 1.0).max(0.0)))
            .h(px(2.0)),
        );
    }
    // The well its cards are in.
    let well = canvas(|g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, style::inset(g, b, 2.0), style::c(grounds(&p).0));
        style::sunken(g, b, &p, false);
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div()
        .w(px(lane_w))
        .h(px(height))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(head)
        .child(div().relative().w_full().flex_1().min_h_0().child(well).child(div().absolute().inset(px(2.0)).child(ScrollView::new(&scroll).border(false).auto_hide(true).child(cards))))
        .into_element()
}

/// The board: every lane. A list of no sections yet has one lane, and
/// beside it a way to its first sections.
pub fn view(v: &mut ToDoApp, width: f32, height: f32, cx: &mut Cx<ToDoApp>) -> Element {
    let Place::List(id) = v.place.clone() else { return div().into_element() };
    let n = v.columns.len();
    let starter = v.book.list(&id).is_some_and(|l| l.sections.is_empty());
    let lane_w = if starter { lane_width(3, width) } else { lane_width(n, width) };
    v.lane.set(lane_w);
    // (Words typed for a new to-do and kept, through a reload or from
    // the list: their card is the first lane's.)
    if v.entry_section.is_none()
        && !v.entry.read(cx).text().is_empty()
        && let Some((Lane::Section(first), _)) = v.columns.first()
    {
        v.entry_section = Some(first.clone());
    }
    let height = height.max(HEAD + 80.0);
    let mut row = div().flex().flex_row().gap(px(GAP)).h(px(height));
    for k in 0..n {
        row = row.child(lane(v, k, lane_w, height, cx));
    }
    // (A list with no sections: what a board is made of, and three to
    // start with, beside its one lane.)
    if starter {
        let (start_id, add_id) = (id.clone(), id.clone());
        let wide = olo_kit::widgets::button_width("Start with To Do and Doing", false).max(olo_kit::widgets::button_width("Add a Section…", false));
        row = row.child(
            div()
                .w(px((width - lane_w - GAP).clamp(180.0, 300.0)))
                .flex_shrink_0()
                .pt(px(HEAD + m::SPACING))
                .px(px(m::SPACING))
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(StringView::new("A board's lanes are the list's sections, one for each stage of the work, and last what's done. Drag to-dos between them.").dim(true).wrap(true))
                .child(div().flex().child(
                    Button::new("Start with To Do and Doing").id("starter").min_width(wide).on_click(cx.listener(move |_: &mut ToDoApp, _: &ClickEvent, cx| cx.send(Msg::Starter(start_id.clone())))),
                ))
                .child(
                    div().flex().child(
                        Button::new("Add a Section…")
                            .id("add-section")
                            .min_width(wide)
                            .on_click(cx.listener(move |v: &mut ToDoApp, _: &ClickEvent, cx| Naming::ask(v, Asking::NewSection(add_id.clone()), cx))),
                    ),
                ),
        );
    }
    let beside = if starter { GAP + (width - lane_w - GAP).clamp(180.0, 300.0) } else { 0.0 };
    let overflows = n as f32 * lane_w + n.saturating_sub(1) as f32 * GAP + beside > width + 0.5;
    // The card being dragged, under the pointer.
    let ghost = v.drag.as_ref().and_then(|d| d.at.map(|at| (d.uid.clone(), at, d.from))).and_then(|(uid, at, from)| {
        let g = geo(v);
        let (k, i, _) = card_at(v, from)?;
        let which = v.columns.get(k)?.0.clone();
        let n = *v.columns.get(k)?.1.get(i)?;
        if v.book.items.get(n)?.todo.uid != uid {
            return None;
        }
        let (top, h) = card_tops(v, k, lane_w).get(i).copied()?;
        let card_left = g.left + px(k as f32 * (lane_w + GAP) + INSET + 2.0);
        let card_top = g.top + px(HEAD + 2.0 + top) + lane_scroll(v, &which).offset().y;
        let (dx, dy) = (at.x - from.x, at.y - from.y);
        let (x, y) = (f32::from(card_left + dx - g.area.left()), f32::from(card_top + dy - g.area.top()));
        Some(
            olo_kit::view_canvas::<ToDoApp>(move |v, g, b| paint_card(v, g, b, n, true))
                .key_by::<ToDoApp>(move |v| card_key(v, n) ^ 1)
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(lane_w - 2.0 * INSET - 4.0))
                .h(px(h)),
        )
    });
    // (Where the board is in the window, kept for presses and drags.)
    let area = olo_kit::view_canvas::<ToDoApp>(|v, _, b| v.board_area.set(b)).paint_key(style::key((width.to_bits(), height.to_bits()))).absolute().inset_0();
    div()
        .relative()
        .size_full()
        .child(area)
        .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut ToDoApp, e: &MouseDownEvent, cx| press(v, e, cx)))
        .on_drag_move(cx.listener(|v: &mut ToDoApp, e: &MouseMoveEvent, cx| moved(v, e, cx)))
        .on_mouse_up(MouseButton::Left, cx.listener(|v: &mut ToDoApp, e: &MouseUpEvent, cx| release(v, e, cx)))
        .on_mouse_up_out(MouseButton::Left, cx.listener(|v: &mut ToDoApp, e: &MouseUpEvent, cx| release(v, e, cx)))
        // (Scrolled across only where its lanes don't fit at their least:
        // a scroll view that can scroll across shows its bar always.)
        .child(ScrollView::new(&v.board_scroll).border(false).horizontal(overflows).vertical(false).child(row))
        .children(ghost)
        .into_element()
}
