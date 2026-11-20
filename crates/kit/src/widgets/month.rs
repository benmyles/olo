//! A month to pick a day from: its name and year over the days of its
//! weeks, the days of the months either side dim at its ends, today
//! marked, a span of days chosen, and a dot under each day that has
//! something on it. Calendar's small month beside its views; a date
//! field's; anything that asks "which day?".
//!
//! BeOS had no such control (its Time preferences drew a month of their
//! own); this is the one every desktop since has had, in Olo's look: no
//! frames round the days, the chosen ones bold on the list's selection,
//! today in a fine outline of the look's highlight, the box a chosen
//! day's ground fills: outlined is today, filled is chosen. (Today was a
//! disc of the mark colour, the heaviest thing in the month, then a
//! ring: louder than the day chosen, and another shape. Where a day is
//! picked from a month, the one chosen is what it's for. Opus's reviews
//! of To-Do's, 2026-11-19.)
//!
//! The widget keeps nothing: the month shown, the days chosen and
//! what's marked are its owner's, which is told of a day picked
//! ([`MonthGrid::on_pick`]) and of the month turned
//! ([`MonthGrid::on_turn`]).

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

/// A day of the calendar (the proleptic Gregorian one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Day {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
}

pub const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

impl Day {
    pub fn new(year: i32, month: u8, day: u8) -> Day {
        Day { year, month, day }
    }

    /// Days from 1970-01-01.
    pub fn number(self) -> i64 {
        let (y, m, d) = (self.year as i64 - (self.month <= 2) as i64, self.month as i64, self.day as i64);
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_number(number: i64) -> Day {
        let z = number + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Day { year: (yoe + era * 400 + (m <= 2) as i64) as i32, month: m as u8, day: d as u8 }
    }

    /// 0 Monday … 6 Sunday.
    pub fn weekday(self) -> u8 {
        (self.number() + 3).rem_euclid(7) as u8
    }

    pub fn plus(self, days: i64) -> Day {
        Day::from_number(self.number() + days)
    }

    /// "Wednesday, November 18, 2026".
    pub fn said(self) -> String {
        format!("{}, {} {}, {}", WEEKDAYS[self.weekday() as usize % 7], MONTHS[(self.month as usize + 11) % 12], self.day, self.year)
    }
}

/// The month `by` months after (before, if negative) `year`'s `month`.
pub fn turned(year: i32, month: u8, by: i32) -> (i32, u8) {
    let n = year * 12 + month as i32 - 1 + by;
    (n.div_euclid(12), (n.rem_euclid(12) + 1) as u8)
}

type DayFn = Rc<dyn Fn(&Day, &mut WindowCx)>;
type TurnFn = Rc<dyn Fn(&(i32, u8), &mut WindowCx)>;

/// A column's width and a week's height.
pub const COLUMN: f32 = 24.0;
pub const ROW: f32 = 22.0;
/// The month's name's row, and the weekdays'.
const TITLE: f32 = m::CONTROL_HEIGHT;
const NAMES: f32 = 18.0;
/// Six weeks always, so nothing under it moves as the months turn.
const WEEKS: usize = 6;
/// An arrow's square.
const ARROW: f32 = 20.0;

pub const WIDTH: f32 = 7.0 * COLUMN;
pub const HEIGHT: f32 = TITLE + NAMES + WEEKS as f32 * ROW;

pub struct MonthGrid {
    name: SharedString,
    year: i32,
    month: u8,
    today: Day,
    chosen: Option<(Day, Day)>,
    marked: Rc<[Day]>,
    week_start: u8,
    on_pick: Option<DayFn>,
    on_turn: Option<TurnFn>,
}

impl MonthGrid {
    /// `year`'s `month` (1 to 12), with `today` marked.
    pub fn new(name: impl Into<SharedString>, year: i32, month: u8, today: Day) -> MonthGrid {
        MonthGrid { name: name.into(), year, month: month.clamp(1, 12), today, chosen: None, marked: Rc::from([]), week_start: 0, on_pick: None, on_turn: None }
    }

    /// The days chosen: from the first to the last, both included.
    pub fn chosen(mut self, first: Day, last: Day) -> Self {
        self.chosen = Some((first.min(last), first.max(last)));
        self
    }

    /// Days with something on them (a dot under each), in order.
    pub fn marked(mut self, days: impl Into<Rc<[Day]>>) -> Self {
        self.marked = days.into();
        self
    }

    /// The day weeks start on: 0 Monday … 6 Sunday.
    pub fn week_start(mut self, day: u8) -> Self {
        self.week_start = day % 7;
        self
    }

    /// A day was clicked.
    pub fn on_pick(mut self, f: impl Fn(&Day, &mut WindowCx) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// An arrow was clicked: the month to show now.
    pub fn on_turn(mut self, f: impl Fn(&(i32, u8), &mut WindowCx) + 'static) -> Self {
        self.on_turn = Some(Rc::new(f));
        self
    }
}

/// What the pointer can be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Day(Day),
    Back,
    On,
}

/// Where its parts were drawn, and which the pointer is over.
#[derive(Default)]
struct State {
    parts: Vec<(Bounds<Pixels>, Part)>,
    hover: Option<Part>,
}

impl IntoElement for MonthGrid {
    fn into_element(self) -> Element {
        let MonthGrid { name, year, month, today, chosen, marked, week_start, on_pick, on_turn } = self;
        let first = Day::new(year, month, 1);
        // The grid starts on the week holding the 1st.
        let start = first.plus(-(((first.weekday() + 7 - week_start) % 7) as i64));
        let key = style::key((year, month, today, chosen, &marked[..], week_start));
        let (pickable, turnable) = (on_pick.is_some(), on_turn.is_some());
        let mut element = canvas(move |g, b| {
            let p = theme::palette();
            // (Three steps: the month's days, the weekdays' letters over
            // them quieter, the days of the months either side quietest.)
            let (text, quiet, dim) = (style::c(p.list_item_text), style::secondary_text(&p), style::disabled_text(&p));
            let hover = g.node_state::<State>().hover;
            let mut parts: Vec<(Bounds<Pixels>, Part)> = Vec::with_capacity(WEEKS * 7 + 2);
            // Centred in what it's given, on device pixels.
            let b = style::snap(g, Bounds { origin: point(b.left() + (b.size.width - px(WIDTH)) / 2.0, b.top()), size: size(px(WIDTH), px(HEIGHT)) });
            crate::review::own_grid(g, b);
            // Its month and year; the arrows that turn it.
            let title = format!("{} {year}", MONTHS[(month as usize + 11) % 12]);
            let mid = b.top() + px(TITLE / 2.0);
            let t = style::label(g, b.left() + px(COLUMN / 2.0 - 5.0), mid, style::Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), text);
            g.agent(agent::Node { role: "Heading", name: Some(title.into()), bounds: t, enabled: true, ..Default::default() });
            if turnable {
                for (i, (part, said)) in [(Part::Back, "Previous Month"), (Part::On, "Next Month")].into_iter().enumerate() {
                    let r = style::snap(g, Bounds { origin: point(b.right() - px(ARROW * (2 - i) as f32), b.top() + px((TITLE - ARROW) / 2.0)), size: size(px(ARROW), px(ARROW)) });
                    if hover == Some(part) {
                        style::fill_rect(g, r, style::shade(&p, p.list_background, 0.9));
                    }
                    style::arrow(g, r, part == Part::Back, if hover == Some(part) { text } else { style::secondary_text(&p) });
                    g.agent(agent::Node::button(format!("{said} ({name})"), r));
                    g.note_pointer_edge(r);
                    parts.push((r, part));
                }
            }
            // The weekdays' letters.
            let top = b.top() + px(TITLE);
            for col in 0..7 {
                let x = b.left() + px(col as f32 * COLUMN + COLUMN / 2.0);
                let letter = &WEEKDAYS[((week_start as usize) + col) % 7][..1];
                style::label(g, x, top + px(NAMES / 2.0), style::Align::Center, letter, style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), quiet);
            }
            let top = top + px(NAMES);
            for i in 0..WEEKS * 7 {
                let day = start.plus(i as i64);
                let (row, col) = (i / 7, i % 7);
                let cell = style::snap(g, Bounds { origin: point(b.left() + px(col as f32 * COLUMN), top + px(row as f32 * ROW)), size: size(px(COLUMN), px(ROW)) });
                let inside = day.month == month && day.year == year;
                let picked = chosen.is_some_and(|(a, z)| a <= day && day <= z);
                if picked {
                    style::fill_rect(g, cell, style::c(p.list_selected_background));
                } else if hover == Some(Part::Day(day)) && pickable {
                    style::fill_rect(g, cell, style::shade(&p, p.list_background, 0.93));
                }
                let centre = cell.center();
                let is_today = day == today;
                if is_today {
                    // Today: its box outlined in the look's highlight, a
                    // point wide in whole device pixels.
                    let s = g.scale_factor();
                    let line = px(s.round().max(1.0) / s);
                    g.paint_quad(olo_interface::outline(cell, style::c(p.control_highlight), Default::default()).border_widths(line));
                }
                let ink = if picked {
                    style::c(p.list_selected_item_text)
                } else if inside || is_today {
                    text
                } else {
                    dim
                };
                let weight = if picked { FontWeight::BOLD } else { FontWeight::NORMAL };
                style::label(g, centre.x, centre.y, style::Align::Center, day.day.to_string(), style::DETAIL_SIZE, style::ui_font(weight), ink);
                // Something's on it: a dot under its number.
                if marked.binary_search(&day).is_ok() && !is_today {
                    let dot = style::snap_centred(g, point(centre.x, cell.bottom() - px(3.0)), 3.0, 3.0);
                    style::fill_rect(g, dot, if picked { style::c(p.list_selected_item_text) } else { style::secondary_text(&p) });
                }
                g.agent(agent::Node { role: "Button", name: Some(day.said().into()), bounds: cell, enabled: true, clickable: pickable, selected: Some(picked), ..Default::default() });
                if pickable {
                    g.note_pointer_edge(cell);
                }
                parts.push((cell, Part::Day(day)));
            }
            g.node_state::<State>().parts = parts;
        })
        .paint_key(key)
        .w(px(WIDTH))
        .h(px(HEIGHT))
        .flex_shrink_0()
        .moves_matter_at_edges()
        .on_mouse_move(|e: &MouseMoveEvent, cx| {
            let state = cx.node_state::<State>();
            let over = state.parts.iter().find(|(r, _)| r.contains(&e.position)).map(|(_, part)| *part);
            if state.hover != over {
                state.hover = over;
                cx.repaint();
            }
        })
        .on_hover(|over, cx| {
            if !over && cx.node_state::<State>().hover.take().is_some() {
                cx.repaint();
            }
        });
        if pickable || turnable {
            element = element.on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
                let over = cx.node_state::<State>().parts.iter().find(|(r, _)| r.contains(&e.position)).map(|(_, part)| *part);
                match over {
                    Some(Part::Day(day)) => {
                        if let Some(f) = &on_pick {
                            f(&day, cx);
                        }
                    }
                    Some(part) => {
                        if let Some(f) = &on_turn {
                            f(&turned(year, month, if part == Part::Back { -1 } else { 1 }), cx);
                        }
                    }
                    None => return,
                }
                cx.stop_propagation();
            });
        }
        element
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_counted_and_named() {
        assert_eq!(Day::new(1970, 1, 1).number(), 0);
        assert_eq!(Day::new(2026, 11, 18).number(), 20_775);
        for n in [-800_000, -1, 0, 59, 60, 11_016, 20_775, 47_540, 2_932_896] {
            assert_eq!(Day::from_number(n).number(), n);
        }
        assert_eq!(Day::from_number(11_016), Day::new(2000, 2, 29));
        assert_eq!((Day::new(2026, 11, 18).weekday(), Day::new(2026, 11, 22).weekday(), Day::new(1970, 1, 1).weekday()), (2, 6, 3));
        assert_eq!(Day::new(2026, 12, 31).plus(1), Day::new(2027, 1, 1));
        assert_eq!(Day::new(2028, 3, 1).plus(-1), Day::new(2028, 2, 29));
        assert_eq!(Day::new(2026, 11, 18).said(), "Wednesday, November 18, 2026");
        assert_eq!((turned(2026, 12, 1), turned(2026, 1, -1), turned(2026, 11, -23), turned(2026, 6, 0)), ((2027, 1), (2025, 12), (2024, 12), (2026, 6)));
    }
}
