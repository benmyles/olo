//! The month: its weeks down, the days across, each day's number over
//! what's on it. Events of a whole day or of several lie across their
//! days as bars in their calendar's colour; the others are a mark in
//! that colour, their time and their title; what doesn't fit a day is
//! counted under it ("+2 more": the day itself, clicked).

use jiff::civil::Date;
use olo_kit::{Bounds, Cx, Element, FontWeight, InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Styled as _, agent, point, px, size, style, theme};

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
    /// The days' edges across and the weeks' down, and the first day
    /// (for a drag: which day the pointer is over).
    xs: Vec<Pixels>,
    ys: Vec<Pixels>,
    start: Option<Date>,
}

impl State {
    /// The day at `at` (the nearest, outside them), and which of its
    /// week's rows of events.
    fn day_at(&self, at: olo_kit::Point<Pixels>) -> Option<(Date, usize)> {
        let start = self.start?;
        if self.xs.len() < 2 || self.ys.len() < 2 {
            return None;
        }
        let col = self.xs.windows(2).position(|c| at.x < c[1]).unwrap_or(self.xs.len() - 2);
        let week = self.ys.windows(2).position(|r| at.y < r[1]).unwrap_or(self.ys.len() - 2);
        let lane = (f32::from(at.y - self.ys[week] - px(NUMBER)) / LANE).floor().max(0.0) as usize;
        Some((layout::day_of(layout::day_number(start) + (7 * week + col) as i32), lane))
    }
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
                // (Being dragged to other days: where it was, faint.)
                let moving = v.day_drag.as_ref().is_some_and(|d| d.to.is_some() && d.key == s.key());
                let mid = r.center().y;
                let room = r.size.width - px(2.0 * PAD);
                // (Its words start where the day's do, whether or not its
                // ground runs on from the week before.)
                let words_at = xs[piece.from] + px(INSET);
                if s.bar() || chosen {
                    // On its calendar's colour (lighter, taking no time).
                    let (ground, lined) = crate::view::ground(&p, s, chosen);
                    let fill = if moving { olo_theme::mix(hue, p.document_background, 0.7) } else { ground };
                    style::fill_rect(g, r, style::c(fill));
                    if chosen && !moving {
                        style::frame_rect(g, r, text);
                    } else if lined && !moving {
                        style::frame_rect(g, r, style::c(hue));
                    }
                    let ink = style::c(p.ink_on(fill));
                    let said = if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) };
                    let t = style::label_fit(g, words_at, mid, style::Align::Left, said, style::DETAIL_SIZE, regular.clone(), ink, room.max(px(0.0)));
                    crate::view::struck(g, s, t, ink);
                } else {
                    // A mark in its calendar's colour, its time, its title.
                    // (An invitation not answered: the mark hollow; one
                    // declined: its title quiet, with a line through it.)
                    let going = s.going;
                    let mark = style::snap_centred(g, point(words_at + px(3.5), mid), 7.0, 7.0);
                    if going == Some(olo_calendar::event::Answer::NotYet) {
                        style::frame_rect(g, mark, style::c(hue));
                    } else {
                        style::fill_rect(g, mark, style::c(hue));
                    }
                    let x = words_at + px(7.0 + PAD);
                    let time = words::time(s.start.time(), v.h24);
                    let t = style::label_fit(g, x, mid, style::Align::Left, time, style::DETAIL_SIZE, regular.clone(), quiet, (r.right() - x).max(px(0.0)));
                    let x = t.right() + px(4.0);
                    if r.right() - x > px(12.0) {
                        let ink = if going == Some(olo_calendar::event::Answer::No) { quiet } else { text };
                        let t = style::label_fit(g, x, mid, style::Align::Left, s.title.to_string(), style::DETAIL_SIZE, regular.clone(), ink, r.right() - x);
                        crate::view::struck(g, s, t, ink);
                    }
                }
                let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                let name = format!("{}, {}, {calendar}{}", s.title, words::when(s.start, s.end, s.all_day, v.h24), crate::view::going_said(s));
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
        // One being dragged to other days: where it would be, in each week
        // it would be in, in the row it was pressed in.
        if let Some(drag) = &v.day_drag
            && let Some(to) = drag.to
            && let Some(s) = v.shown.iter().find(|s| s.key() == drag.key)
        {
            let from = layout::day_number(to) - layout::day_number(start);
            let mut said = Some(if s.all_day { s.title.to_string() } else { format!("{} {}", words::time(s.start.time(), v.h24), s.title) });
            for week in 0..weeks as i32 {
                let (a, z) = (from.max(7 * week), (from + drag.days - 1).min(7 * week + 6));
                if a > z {
                    continue;
                }
                let (ca, cz) = ((a - 7 * week) as usize, (z - 7 * week) as usize);
                let y = ys[week as usize] + px(NUMBER + drag.lane.min(lanes.saturating_sub(1)) as f32 * LANE);
                let left = xs[ca] + if a > from { px(0.0) } else { px(EDGE) };
                let right = xs[cz + 1] - if z < from + drag.days - 1 { px(0.0) } else { px(EDGE) };
                let r = style::snap(g, Bounds::from_corners(point(left, y), point(right, y + px(BAR))));
                let fill = p.hue(s.hue);
                style::fill_rect(g, r, style::c(fill));
                style::frame_rect(g, r, text);
                // (Its words where it starts: once.)
                if let Some(said) = said.take() {
                    style::label_fit(
                        g,
                        xs[ca] + px(INSET),
                        r.center().y,
                        style::Align::Left,
                        said,
                        style::DETAIL_SIZE,
                        regular.clone(),
                        style::c(p.ink_on(fill)),
                        (r.size.width - px(2.0 * PAD)).max(px(0.0)),
                    );
                }
            }
        }
        let state = g.node_state::<State>();
        (state.parts, state.origin) = (parts, b.origin);
        (state.xs, state.ys, state.start) = (xs, ys, Some(start));
    })
    .key_by(|v: &Calendar| style::key((v.cursor, v.today, v.week_start(), v.shown_version, &v.selected, v.h24, v.day_drag.as_ref().and_then(|d| d.to))))
    .size_full()
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(|v: &mut Calendar, e: &MouseDownEvent, cx| {
            let now = cx.listener_bounds();
            let state = cx.node_state::<State>();
            let at = crate::view::in_drawing(e.position, state.origin, now);
            let over = state.parts.iter().rev().find(|(r, _)| r.contains(&at)).map(|(_, part)| part.clone());
            // An event pressed may be dragged to other days.
            if let (1, Some(Part::Event(key))) = (e.click_count, &over)
                && let Some((day, lane)) = state.day_at(at)
            {
                v.day_drag_start(key, at, day, lane);
            }
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
    // (An event pressed is dragged to other days, followed out of the
    // month too, and goes there when the button is let go.)
    .on_drag_move(cx.listener(|v: &mut Calendar, e: &MouseMoveEvent, cx| {
        if v.day_drag.is_none() {
            return;
        }
        let now = cx.listener_bounds();
        let state = cx.node_state::<State>();
        let at = crate::view::in_drawing(e.position, state.origin, now);
        if let Some((day, _)) = state.day_at(at) {
            v.day_drag_over(at, day, cx);
        }
    }))
    .on_window_mouse_up(cx.listener(|v: &mut Calendar, _: &MouseUpEvent, cx| v.day_drag_drop(cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The day under the pointer, for a drag: by the days' edges across
    /// and the weeks' down, the nearest outside them; and which row of
    /// its week's events.
    #[test]
    fn the_day_under_the_pointer() {
        let xs: Vec<Pixels> = (0..=7).map(|i| px(100.0 * i as f32)).collect();
        let ys: Vec<Pixels> = (0..=5).map(|i| px(22.0 + 120.0 * i as f32)).collect();
        let state = State { xs, ys, start: Some(jiff::civil::date(2026, 11, 1)), ..Default::default() };
        let at = |x: f32, y: f32| state.day_at(point(px(x), px(y))).map(|(day, lane)| (day.to_string(), lane));
        assert_eq!(at(50.0, 30.0), Some(("2026-11-01".to_owned(), 0)));
        // (Tuesday the 17th, its third row of events.)
        assert_eq!(at(250.0, 22.0 + 240.0 + NUMBER + 2.0 * LANE + 3.0), Some(("2026-11-17".to_owned(), 2)));
        // (Outside: the nearest day.)
        assert_eq!(at(-40.0, 5000.0).map(|(day, _)| day), Some("2026-11-29".to_owned()));
        assert_eq!(at(9000.0, -10.0), Some(("2026-11-07".to_owned(), 0)));
        assert_eq!(State::default().day_at(point(px(1.0), px(1.0))), None);
    }
}
