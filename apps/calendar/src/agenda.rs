//! The agenda: what's on from a day on, day by day down a list, each
//! event with its time, its calendar's colour, its title and where.

use jiff::civil::Date;
use olo_kit::{
    Bounds, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Styled as _, agent, div, metrics as m, point, px, style,
    theme,
    widgets::{EmptyState, ScrollView},
};

use crate::{Calendar, Key, Msg, layout, words};

/// A day's heading, and an event's row.
const HEADING: f32 = 30.0;
const ROW: f32 = 22.0;
/// The times' column.
const TIMES: f32 = 132.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Line {
    Day(Date),
    /// An occurrence (its place in what's shown), on this day.
    Event(usize, Date),
}

impl Line {
    fn tall(self) -> f32 {
        match self {
            Line::Day(_) => HEADING,
            Line::Event(..) => ROW,
        }
    }
}

#[derive(Default)]
struct State {
    parts: Vec<(Bounds<Pixels>, Key)>,
    /// Where the drawing's corner was when they were noted.
    origin: olo_kit::Point<Pixels>,
}

/// The agenda's lines from the day the view is at: each day with
/// something on it, then what's on it in order (whole-day events first).
fn lines(v: &Calendar) -> Vec<Line> {
    let (first, after) = v.visible();
    let mut out = Vec::new();
    for n in layout::day_number(first)..layout::day_number(after) {
        let day = layout::day_of(n);
        let mut on: Vec<usize> = v.shown.iter().enumerate().filter(|(_, s)| s.days().0 <= day && day <= s.days().1).map(|(i, _)| i).collect();
        if on.is_empty() {
            continue;
        }
        on.sort_by_key(|&i| (!v.shown[i].bar(), v.shown[i].start.max(day.at(0, 0, 0, 0))));
        out.push(Line::Day(day));
        out.extend(on.into_iter().map(|i| Line::Event(i, day)));
    }
    out
}

pub fn view(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
    let all = lines(v);
    if all.is_empty() {
        return EmptyState::new("Nothing on").icon("calendar").detail(format!("Nothing in the {} weeks from {}.", crate::view::AGENDA_DAYS / 7, words::long_day(v.cursor))).into_element();
    }
    let height: f32 = all.iter().map(|l| l.tall()).sum::<f32>() + m::TIGHT;
    let list = cx
        .canvas(move |v: &Calendar, g, b| {
            let p = theme::palette();
            let (text, quiet) = (style::c(p.document_text), style::secondary_text(&p));
            let line = style::c(olo_theme::mix(p.document_text, p.document_background, 0.88));
            let (regular, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
            let seen = g.visible_bounds();
            let mut parts = Vec::new();
            let x0 = b.left() + px(m::WINDOW_INSET);
            let right = b.right() - px(m::WINDOW_INSET);
            let mut y = b.top();
            for l in &all {
                let row = Bounds::from_corners(point(b.left(), y), point(b.right(), y + px(l.tall())));
                y += px(l.tall());
                // (Only what shows is drawn.)
                if row.bottom() < seen.top() || row.top() > seen.bottom() {
                    continue;
                }
                match *l {
                    Line::Day(day) => {
                        let mid = row.bottom() - px(11.0);
                        let said = format!("{}, {} {}", words::weekday_name(day), words::month_name(day), day.day());
                        let r = style::label(g, x0, mid, style::Align::Left, said.clone(), style::PLAIN_SIZE, bold.clone(), text);
                        g.agent(agent::Node { role: "Heading", name: Some(words::long_day(day).into()), bounds: r, enabled: true, ..Default::default() });
                        let note = match layout::day_number(day) - layout::day_number(v.today) {
                            0 => "Today",
                            1 => "Tomorrow",
                            -1 => "Yesterday",
                            _ => "",
                        };
                        // (At the right, over the calendars' names.)
                        if !note.is_empty() {
                            style::label_on_baseline(g, right, style::baseline_at(g, mid, style::PLAIN_SIZE), style::Align::Right, note, style::SMALL_SIZE, regular.clone(), quiet);
                        }
                        style::hline(g, x0, right, row.bottom() - px(1.0), line);
                    }
                    Line::Event(i, day) => {
                        let s = &v.shown[i];
                        let chosen = v.selected.as_ref().is_some_and(|key| *key == s.key());
                        let mid = row.center().y;
                        let band = style::snap(g, Bounds::from_corners(point(x0 - px(4.0), row.top() + px(1.0)), point(right + px(4.0), row.bottom() - px(1.0))));
                        if chosen {
                            style::fill_rect(g, band, style::c(p.list_selected_background));
                        }
                        let ink = if chosen { style::c(p.list_selected_item_text) } else { text };
                        let dim = if chosen { ink } else { quiet };
                        // Its time on this day: all day, or from when to when.
                        let (first, last) = s.days();
                        let when = if s.all_day {
                            "all day".to_owned()
                        } else if first == last {
                            words::when(s.start, s.end, false, v.h24)
                        } else if day == first {
                            format!("from {}", words::time(s.start.time(), v.h24))
                        } else if day == last {
                            format!("until {}", words::time(s.end.time(), v.h24))
                        } else {
                            "all day".to_owned()
                        };
                        style::label(g, x0, mid, style::Align::Left, when, style::PLAIN_SIZE, regular.clone(), dim);
                        let mark = style::snap_centred(g, point(x0 + px(TIMES + 4.0), mid), 8.0, 8.0);
                        style::fill_rect(g, mark, style::c(p.hue(s.hue)));
                        let x = x0 + px(TIMES + 8.0 + m::TIGHT + 2.0);
                        // The calendar's name at the right; the title and where, as far as there's room.
                        let calendar = v.book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
                        let c = style::label(g, right, mid, style::Align::Right, calendar.clone(), style::SMALL_SIZE, regular.clone(), dim);
                        let room = (c.left() - px(m::SPACING) - x).max(px(0.0));
                        let t = style::label_fit(g, x, mid, style::Align::Left, s.title.to_string(), style::PLAIN_SIZE, regular.clone(), ink, room);
                        let after = t.right() + px(m::SPACING);
                        if !s.location.is_empty() && c.left() - px(m::SPACING) - after > px(40.0) {
                            style::label_on_baseline_fit(
                                g,
                                after,
                                style::baseline_at(g, mid, style::PLAIN_SIZE),
                                style::Align::Left,
                                s.location.to_string(),
                                style::SMALL_SIZE,
                                regular.clone(),
                                dim,
                                c.left() - px(m::SPACING) - after,
                            );
                        }
                        let name = format!("{}, {}, {calendar}", s.title, words::when(s.start, s.end, s.all_day, v.h24));
                        g.agent(agent::Node { role: "ListItem", name: Some(name.into()), bounds: band, enabled: true, clickable: true, selected: Some(chosen), ..Default::default() });
                        parts.push((band, s.key()));
                    }
                }
            }
            let state = g.node_state::<State>();
            (state.parts, state.origin) = (parts, b.origin);
        })
        .key_by(|v: &Calendar| style::key((v.cursor, v.today, v.shown_version, &v.selected, v.h24)))
        .w_full()
        .h(px(height))
        .flex_shrink_0()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_: &mut Calendar, e: &MouseDownEvent, cx| {
                let now = cx.listener_bounds();
                let state = cx.node_state::<State>();
                let at = crate::view::in_drawing(e.position, state.origin, now);
                let over = state.parts.iter().find(|(r, _)| r.contains(&at)).map(|(_, key)| key.clone());
                match over {
                    Some(key) if e.click_count >= 2 => cx.send(Msg::Open(key)),
                    over => cx.send(Msg::Select(over)),
                }
            }),
        );
    let p = theme::palette();
    div().size_full().child(ScrollView::new(&v.agenda_scroll).border(false).auto_hide(true).background(p.document_background).child(list)).into_element()
}
