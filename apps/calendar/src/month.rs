//! The month: its weeks down, the days across, each day's number over
//! what's on it. Events of a whole day or of several lie across their
//! days as bars in their calendar's colour; the others are a mark in
//! that colour, their time and their title; what doesn't fit a day is
//! counted under it ("+2 more": the day itself, clicked).

use jiff::civil::Date;
use olo_kit::{Bounds, Cx, Element, FontWeight, InteractiveElement as _, MouseButton, MouseDownEvent, Pixels, Styled as _, agent, point, px, size, style, theme};

use crate::{Calendar, Key, Mode, Msg, layout, words};

/// The weekdays' row.
const HEADER: f32 = 22.0;
/// A day's number's row.
const NUMBER: f32 = 22.0;
/// An event's row, and how tall it is in it.
const LANE: f32 = 17.0;
const BAR: f32 = 15.0;
/// Where a day's words start from its left line: its number, its
/// events' words (an event's own ground starts `EDGE` in, its words
/// `PAD` inside that).
const EDGE: f32 = 3.0;
const PAD: f32 = 4.0;
const INSET: f32 = EDGE + PAD;

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Event(Key),
    /// A day's "+2 more".
    More(Date),
    Day(Date),
}

/// Where its parts were drawn (the last first: what's on top).
#[derive(Default)]
struct State {
    parts: Vec<(Bounds<Pixels>, Part)>,
    /// Where the drawing's corner was when they were noted.
    origin: olo_kit::Point<Pixels>,
}

pub fn view(cx: &mut Cx<Calendar>) -> Element {
    cx.canvas(|v: &Calendar, g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        let (text, dim, quiet) = (style::c(p.document_text), style::disabled_text(&p), style::secondary_text(&p));
        let line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.88));
        let (regular, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
        let (start, weeks) = layout::month_grid(v.cursor.year(), v.cursor.month(), v.week_start());
        let mut parts: Vec<(Bounds<Pixels>, Part)> = Vec::new();
        // Columns and rows on device pixels, as even as they come.
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height) - HEADER);
        let xs: Vec<Pixels> = (0..=7).map(|i| style::snap_coord(g, b.left() + px(w * i as f32 / 7.0))).collect();
        let top = b.top() + px(HEADER);
        let ys: Vec<Pixels> = (0..=weeks).map(|i| style::snap_coord(g, top + px(h * i as f32 / weeks as f32))).collect();
        olo_kit::review::own_grid(g, b);
        // The weekdays.
        for col in 0..7 {
            let day = layout::day_of(layout::day_number(start) + col as i32);
            let name = words::weekday_name(day);
            let wide = f32::from(xs[col + 1] - xs[col]) >= 96.0;
            let said = if wide { name.to_owned() } else { name[..3].to_owned() };
            let r = style::label(g, xs[col] + px(INSET), b.top() + px(HEADER / 2.0), style::Align::Left, said, style::SMALL_SIZE, regular.clone(), quiet);
            g.agent(agent::Node::label(name, r));
        }
        // Each day's ground; then the lines between them; then what's on them.
        for week in 0..weeks {
            for col in 0..7 {
                let day = layout::day_of(layout::day_number(start) + (7 * week + col) as i32);
                let cell = Bounds::from_corners(point(xs[col], ys[week]), point(xs[col + 1], ys[week + 1]));
                if day.month() != v.cursor.month() {
                    style::fill_rect(g, cell, style::c(olo_theme::mix(p.document_text, p.document_background, 0.965)));
                }
                if day == v.cursor && v.selected.is_none() {
                    style::fill_rect(g, cell, style::c(p.list_selected_background).opacity(0.45));
                }
            }
        }
        for y in &ys[..weeks] {
            style::hline(g, b.left(), b.right(), *y, line);
        }
        for x in &xs[1..7] {
            style::vline(g, *x, top, b.bottom(), line);
        }
        let lanes = (((f32::from(ys[1] - ys[0]) - NUMBER - 2.0) / LANE).floor() as usize).min(12);
        for week in 0..weeks {
            let (y0, y1) = (ys[week], ys[week + 1]);
            let first = layout::day_of(layout::day_number(start) + 7 * week as i32);
            // Each day's number.
            for col in 0..7 {
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let cell = Bounds::from_corners(point(xs[col], y0), point(xs[col + 1], y1));
                let inside = day.month() == v.cursor.month();
                let mid = y0 + px(NUMBER / 2.0);
                let said = if day.day() == 1 { format!("{} {}", &words::month_name(day)[..3], day.day()) } else { day.day().to_string() };
                let x = xs[col] + px(INSET);
                let number = if day == v.today {
                    // Today: its number on the look's mark colour.
                    let width = f32::from(style::text_width(said.clone(), style::DETAIL_SIZE, bold.clone())) + 10.0;
                    let pill = style::snap(g, Bounds { origin: point(x - px(PAD), mid - px(8.0)), size: size(px(width.max(18.0)), px(16.0)) });
                    g.paint_quad(olo_kit::fill(pill, style::c(p.control_mark)).corner_radii(px(8.0)));
                    style::label(g, pill.center().x, mid, style::Align::Center, said, style::DETAIL_SIZE, bold.clone(), style::c(p.ink_on(p.control_mark)))
                } else {
                    style::label(g, x, mid, style::Align::Left, said, style::DETAIL_SIZE, regular.clone(), if inside { text } else { dim })
                };
                g.agent(agent::Node { role: "Cell", name: Some(words::long_day(day).into()), bounds: number, enabled: true, clickable: true, selected: Some(day == v.cursor), ..Default::default() });
                parts.push((cell, Part::Day(day)));
            }
            // What's on in it.
            let laid = layout::row(&v.shown, first, 7, lanes, false);
            for piece in &laid.pieces {
                let s = &v.shown[piece.item];
                let y = y0 + px(NUMBER + piece.lane as f32 * LANE);
                // (A bar that goes on runs to the week's edge; one that
                // ends, and every other event, keeps clear of the lines.)
                let left = xs[piece.from] + if piece.before { px(0.0) } else { px(EDGE) };
                let right = xs[piece.to + 1] - if piece.after { px(0.0) } else { px(EDGE) };
                let r = style::snap(g, Bounds::from_corners(point(left, y), point(right, y + px(BAR))));
                let chosen = v.selected.as_ref().is_some_and(|key| *key == s.key());
                let hue = p.hue(s.hue);
                let mid = r.center().y;
                let room = r.size.width - px(2.0 * PAD);
                // (Its words start where the day's do, whether or not its
                // ground runs on from the week before.)
                let words_at = xs[piece.from] + px(INSET);
                if s.bar() || chosen {
                    // On its calendar's colour (lighter, taking no time).
                    let fill = if s.free && !chosen { olo_theme::mix(hue, p.document_background, 0.45) } else { hue };
                    style::fill_rect(g, r, style::c(fill));
                    if chosen {
                        style::frame_rect(g, r, text);
                    }
                    let ink = style::c(p.ink_on(fill));
                    let said = if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) };
                    style::label_fit(g, words_at, mid, style::Align::Left, said, style::DETAIL_SIZE, regular.clone(), ink, room.max(px(0.0)));
                } else {
                    // A mark in its calendar's colour, its time, its title.
                    let mark = style::snap_centred(g, point(words_at + px(3.5), mid), 7.0, 7.0);
                    style::fill_rect(g, mark, style::c(hue));
                    let x = words_at + px(7.0 + PAD);
                    let time = words::time(s.start.time(), v.h24);
                    let t = style::label_fit(g, x, mid, style::Align::Left, time, style::DETAIL_SIZE, regular.clone(), quiet, (r.right() - x).max(px(0.0)));
                    let x = t.right() + px(4.0);
                    if r.right() - x > px(12.0) {
                        style::label_fit(g, x, mid, style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, regular.clone(), text, r.right() - x);
                    }
                }
                let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                let name = format!("{}, {}, {calendar}", s.title, words::when(s.start, s.end, s.all_day, v.h24));
                g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: r, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                parts.push((r, Part::Event(s.key())));
            }
            // What didn't fit, counted.
            for col in 0..7 {
                if laid.more[col] == 0 || lanes == 0 {
                    continue;
                }
                let day = layout::day_of(layout::day_number(first) + col as i32);
                let y = y0 + px(NUMBER + (lanes - 1) as f32 * LANE);
                let r = style::snap(g, Bounds::from_corners(point(xs[col] + px(EDGE), y), point(xs[col + 1] - px(EDGE), y + px(BAR))));
                let said = format!("+{} more", laid.more[col]);
                let l = style::label_fit(g, xs[col] + px(INSET), r.center().y, style::Align::Left, said.clone(), style::DETAIL_SIZE, regular.clone(), quiet, r.size.width - px(2.0 * PAD));
                g.agent(agent::Node { role: "Link", name: Some(format!("{said}, {}", words::long_day(day)).into()), bounds: l, enabled: true, clickable: true, ..Default::default() });
                parts.push((r, Part::More(day)));
            }
        }
        let state = g.node_state::<State>();
        (state.parts, state.origin) = (parts, b.origin);
    })
    .key_by(|v: &Calendar| style::key((v.cursor, v.today, v.week_start(), v.shown_version, &v.selected, v.h24)))
    .size_full()
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| {
            let now = cx.listener_bounds();
            let state = cx.node_state::<State>();
            let at = crate::view::in_drawing(e.position, state.origin, now);
            let over = state.parts.iter().rev().find(|(r, _)| r.contains(&at)).map(|(_, part)| part.clone());
            match over {
                // (Twice: opened.)
                Some(Part::Event(key)) if e.click_count >= 2 => cx.send(Msg::Open(key)),
                Some(Part::Event(key)) => cx.send(Msg::Select(Some(key))),
                // (A day clicked twice: a new event on it.)
                Some(Part::Day(day)) if e.click_count >= 2 => cx.send(Msg::New(day.at(9, 0, 0, 0), false)),
                Some(Part::More(day)) => {
                    v.go(day, cx);
                    v.set_mode(Mode::Day, cx);
                }
                Some(Part::Day(day)) => {
                    cx.send(Msg::Select(None));
                    v.go(day, cx);
                }
                None => {}
            }
        }),
    )
}
