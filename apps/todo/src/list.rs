//! A place's to-dos down a list, under their headings (a day, a list, a
//! section). Its first row is where a new one is typed (a dashed circle
//! where a to-do's check is, the field where its title is); each row
//! after is a to-do: its check circle in its list's colour, its title,
//! the marks of what it has beside it (notes, attachments, a reminder),
//! and at the right, in columns that hold down the list, its list (a dot
//! and a name, where the place has several lists') and when it's due,
//! red once it's overdue. Rows are made as they come into view, each
//! drawn again only when what it shows changes. A click on the circle
//! checks it off; anywhere else on the row chooses it.

use olo_kit::{
    Bounds, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, Painter, ParentElement as _, Pixels, Styled as _, agent, canvas, div, point, px,
    size, style, theme,
    widgets::{EmptyState, ScrollView},
};
use olo_todo::view::{Heading, Place, Row, overdue};

use crate::{
    Msg, ToDoApp,
    marks::{self, Glyph, Look, Mark},
    words,
};

/// A row's height, a heading's too (one height: the list is made as it
/// comes into view).
pub const ROW: f32 = 30.0;
/// A heading's row after the first: taller, the more over its words, so
/// each group stands clear of the one before it (a heading as near the
/// rows over it as its own made an even stripe of a place of many small
/// groups, Upcoming's days: Opus's review, 2026-11-19).
pub const HEADING: f32 = 36.0;

/// Where each of a place's rows starts down its list, and last where
/// they end (`sized_rows`), on a display of scale `scale`: each row a
/// whole number of device pixels tall, so every row's words fall on its
/// pixels alike (30 points are 52.5 pixels at 1.75×, and rows' titles
/// stood 51 to 54 apart).
pub fn tops(rows: &[Row], scale: f32) -> std::rc::Rc<[f32]> {
    let whole = |points: f32| (points * scale).round().max(1.0) / scale;
    let (row, heading) = (whole(ROW), whole(HEADING));
    olo_kit::row_tops(rows.iter().enumerate().map(|(i, r)| if i > 0 && matches!(r, Row::Heading(_)) { heading } else { row }))
}
/// Room above the first row and below the last.
const PAD: f32 = 4.0;
/// The check circle: its left, and across.
pub const CHECK_X: f32 = 16.0;
pub const CHECK: f32 = 18.0;
/// The title's left.
pub const TEXT_X: f32 = CHECK_X + CHECK + 10.0;
/// Between a row's columns at its right, between its marks, and from
/// its title to its first mark.
const COLUMN_GAP: f32 = 14.0;
const MARK_GAP: f32 = 5.0;
const TITLE_GAP: f32 = 8.0;
/// A day heading's name: its column ("Wednesday" and a gap), its date
/// after it.
const DAY_NAME: f32 = 88.0;
/// A list's dot in its column, and from it to its name.
const DOT: f32 = 7.0;
/// The × after a due read from a new to-do's words: its square, and
/// between the due and it.
const KEEP: f32 = 14.0;
const KEEP_GAP: f32 = 7.0;
const DOT_GAP: f32 = 6.0;

/// A line between things on the list's ground: lighter than its text by
/// far, darker (or, by night, lighter) than the ground enough to see.
pub fn rule(p: &olo_theme::Palette) -> Hsla {
    style::c(olo_theme::mix(p.document_background, p.panel_text, if p.mode == olo_theme::ThemeMode::Light { 0.13 } else { 0.2 }))
}

/// The rows' look, for a row to know when it must be drawn again.
pub fn row_key(v: &ToDoApp, i: usize) -> u64 {
    let selected = |uid: &str| v.selected.as_deref() == Some(uid);
    match v.rows.get(i) {
        Some(Row::ToDo(n)) => match v.book.items.get(*n) {
            Some(item) => style::key((
                &item.todo.title,
                item.todo.open(),
                v.lingering.contains_key(&item.todo.uid),
                due_said(v, item),
                item.todo.remind.is_some(),
                item.todo.attachments.len(),
                item.todo.notes.is_empty(),
                v.hue_of(&item.list),
                selected(&item.todo.uid),
                v.book.list(&item.list).map(|l| &l.name),
                (v.due_col.to_bits(), v.list_col.to_bits()),
            )),
            None => 0,
        },
        Some(Row::Heading(h)) => style::key((heading_words(v, h), heading_hue(v, h), matches!(h, Heading::Day(_)), under(v, i))),
        Some(Row::Nothing) => 1,
        None => 0,
    }
}

fn heading_words(v: &ToDoApp, h: &Heading) -> String {
    match h {
        Heading::Overdue => olo_kit::words::tr("Overdue").into(),
        // (Today's own, under what's overdue: not the place's name again.)
        Heading::Day(_) if v.place == Place::Today => olo_kit::words::tr("Due Today").into(),
        Heading::Day(d) => words::day_heading(*d, v.now.date()),
        Heading::List(id) => v.book.list(id).map(|l| l.name.clone()).unwrap_or_default(),
        Heading::Section(Some(s)) => s.clone(),
        Heading::Section(None) => olo_kit::words::tr("No Section").into(),
        Heading::Done => olo_kit::words::tr("Done").into(),
    }
}

fn heading_hue(v: &ToDoApp, h: &Heading) -> Option<olo_theme::Hue> {
    match h {
        Heading::Overdue => Some(olo_theme::Hue::Red),
        Heading::List(id) => Some(v.hue_of(id)),
        _ => None,
    }
}

/// How many to-dos are under the heading at row `i`: those still to do
/// (one just checked off, struck through there a moment yet, isn't
/// counted: the window's other counts dropped as it was checked), or,
/// under what's done, those.
fn under(v: &ToDoApp, i: usize) -> usize {
    let of_done = v.place == Place::Done || matches!(v.rows.get(i), Some(Row::Heading(Heading::Done)));
    v.rows
        .get(i + 1..)
        .unwrap_or_default()
        .iter()
        .take_while(|r| !matches!(r, Row::Heading(_)))
        .filter(|r| match r {
            Row::ToDo(n) => of_done || v.book.items.get(*n).is_some_and(|item| item.todo.open()),
            _ => false,
        })
        .count()
}

/// A ring's width on this display: a point and a half, in whole device
/// pixels (a ring of a pixel and a half is a blur).
fn ring_width(g: &Painter) -> f32 {
    let s = g.scale_factor();
    (1.5 * s).round().max(1.0) / s
}

/// A to-do's check circle in `hue`, in `r` (its box): a ring while it's
/// to do, filled with a check once it's done.
pub fn paint_check(g: &mut Painter, r: Bounds<Pixels>, hue: olo_theme::Rgba, done: bool, ground: olo_theme::Rgba) {
    let c = r.center();
    let disc = |g: &mut Painter, across: f32, colour: Hsla| {
        let b = style::snap_centred(g, c, across, across);
        g.paint_quad(olo_kit::fill(b, colour).corner_radii(b.size.width / 2.0));
    };
    disc(g, CHECK, style::c(hue));
    if done {
        let p = theme::palette();
        let ink = style::c(if olo_theme::contrast(hue, [255, 255, 255, 255]) >= 2.2 { [255, 255, 255, 255] } else { p.panel_text });
        let m = Bounds { origin: point(c.x - px(4.5), c.y - px(3.5)), size: size(px(9.0), px(7.0)) };
        style::check_mark(g, m, ink);
    } else {
        let ring = ring_width(g);
        disc(g, CHECK - 2.0 * ring, style::c(ground));
    }
}

/// The circle where a new to-do's check will be: a fine quiet ring,
/// a plus in it. (Dashes broke into loose dots at 1×, and are no
/// manner of this desktop's.)
pub fn paint_new_circle(g: &mut Painter, r: Bounds<Pixels>, across: f32, ink: Hsla, ground: Hsla) {
    let c = r.center();
    let line = 1.0f32.max(g.scale_factor().round()) / g.scale_factor();
    for (across, colour) in [(across, ink), (across - 2.0 * line, ground)] {
        let b = style::snap_centred(g, c, across, across);
        g.paint_quad(olo_kit::fill(b, colour).corner_radii(b.size.width / 2.0));
    }
    let plus = Bounds { origin: point(c.x - px(6.0), c.y - px(6.0)), size: size(px(12.0), px(12.0)) };
    marks::paint_glyph(g, Glyph::Plus, plus, ink);
}

/// What a row says of its to-do's due, and whether it's overdue. Under
/// a heading that says the day (Today's, Upcoming's days), only its time:
/// the day said again at every row was noise. Done's rows say none (their
/// heading is the day they were done); what's done at the end of a list's
/// own place says when it was, as its card does on the board.
pub fn due_said(v: &ToDoApp, item: &olo_todo::Item) -> Option<(String, bool)> {
    let settled = !item.todo.open() && !v.lingering.contains_key(&item.todo.uid);
    if let (Place::List(_), true, Some(at)) = (&v.place, settled, item.todo.done_at) {
        return Some((olo_kit::tr!("Done {}", words::in_words(at.to_zoned(v.zone.clone()).date(), v.now.date())), false));
    }
    let due = item.todo.due.as_ref()?;
    let late = item.todo.open() && overdue(due, &v.now);
    let zone = v.now.time_zone();
    let day = olo_todo::view::due_day(due, zone);
    let time = (!due.is_date()).then(|| words::time(due.moment(zone).time(), v.h24));
    match &v.place {
        Place::Done => None,
        Place::Upcoming => time.map(|t| (t, late)),
        Place::Today if day >= v.now.date() => time.map(|t| (t, late)),
        // (Overdue: the day it was due is what matters.)
        Place::Today => Some((words::day(day, v.now.date()), late)),
        _ => Some((words::due(due, &v.now, v.h24), late)),
    }
}

/// The columns at the rows' right end for the place as it is: how wide
/// its dues are at the most, and its lists' names (none, in a list's own
/// place).
pub fn columns(v: &ToDoApp) -> (f32, f32) {
    let font = style::ui_font(FontWeight::NORMAL);
    let (mut due, mut list) = (0.0f32, 0.0f32);
    // (Its list said on each row where lists are mixed down the place;
    // not under a list's own heading, in All, nor in a list's own place.)
    let several = matches!(v.place, Place::Today | Place::Upcoming | Place::Done);
    // (What a row says of its due is said by its day and its time: each
    // of those said and measured once, however many rows share it. Said
    // and measured for each of 4,500 rows, it was half of what checking
    // one off cost.)
    let zone = v.now.time_zone();
    let mut measured: std::collections::HashSet<(bool, jiff::civil::Date, Option<jiff::civil::Time>)> = std::collections::HashSet::new();
    for row in v.rows.iter() {
        let Row::ToDo(n) = row else { continue };
        let Some(item) = v.book.items.get(*n) else { continue };
        let t = &item.todo;
        let done_on = t.done_at.filter(|_| !t.open() && matches!(v.place, Place::List(_)) && !v.lingering.contains_key(&t.uid));
        let shape = match (done_on, &t.due) {
            (Some(at), _) => (true, at.to_zoned(zone.clone()).date(), None),
            (None, Some(due)) if due.is_date() => (false, olo_todo::view::due_day(due, zone), None),
            (None, Some(due)) => {
                let at = due.moment(zone);
                (false, at.date(), Some(at.time()))
            }
            (None, None) => continue,
        };
        if measured.insert(shape)
            && let Some((words, _)) = due_said(v, item)
        {
            due = due.max(style::text_width_app(&words, style::DETAIL_SIZE, &font).ceil());
        }
    }
    // (As wide as the widest of every list's names, not only those the
    // place shows now: the lists' dots stand at one place from view to
    // view.)
    if several && !v.rows.is_empty() {
        for l in &v.book.lists {
            list = list.max(style::text_width_app(&l.name, style::DETAIL_SIZE, &font).ceil());
        }
    }
    // (A list's name no wider than a long word: the title's is the room.)
    // (No column kept for dues where none shows: an empty one at the
    // rows' end read as something missing.)
    (due, if list > 0.0 { list.min(96.0) + DOT + DOT_GAP } else { 0.0 })
}

/// Row `i` of the place, drawn.
fn paint_row(v: &ToDoApp, g: &mut Painter, r: Bounds<Pixels>, i: usize) {
    let p = theme::palette();
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    match v.rows.get(i) {
        Some(Row::Heading(h)) => {
            // (A heading stands low in its row, over its rule: not one of
            // the rows' even steps, which the review holds a column of
            // words to.)
            olo_kit::review::own_grid(g, r);
            let text = heading_words(v, h);
            // (Low in its row: nearer its own to-dos than those before it.
            // Its rule a point clear of the first of them: a chosen row's
            // ground right against the rule read as one thick line.)
            let base = r.bottom() - px(8.0);
            let at = match h {
                // A day: its name (Tomorrow, Saturday), then its date,
                // quiet, in a column of its own down the headings: every
                // day's the same, a second Monday too (its date tells it
                // from the first; said by its date alone in bold, it was
                // the one heading of another form, and the loudest).
                // (Today's own heading, under what's overdue, is the one word.)
                Heading::Day(d) if v.place != Place::Today => {
                    let named = match (*d - v.now.date()).get_days() {
                        -1..=1 => words::day(*d, v.now.date()),
                        _ => words::weekday_of(*d).to_owned(),
                    };
                    let name = style::label_on_baseline(g, r.left() + px(CHECK_X), base, style::Align::Left, named, style::PLAIN_SIZE, bold, style::c(p.panel_text));
                    let date = style::label_on_baseline(
                        g,
                        r.left() + px(CHECK_X + DAY_NAME),
                        base,
                        style::Align::Left,
                        words::short_date(*d, v.now.date()),
                        style::PLAIN_SIZE,
                        plain,
                        style::secondary_text(&p),
                    );
                    Bounds::from_corners(name.origin, date.bottom_right())
                }
                Heading::List(id) => {
                    let dot = Bounds { origin: point(r.left() + px(CHECK_X + (CHECK - 16.0) / 2.0), base - px(12.0)), size: size(px(16.0), px(16.0)) };
                    olo_kit::widgets::sidebar::paint_dot(g, dot, p.hue(v.hue_of(id)), &p);
                    style::label_on_baseline_fit(g, r.left() + px(TEXT_X), base, style::Align::Left, text.clone(), style::PLAIN_SIZE, bold, style::c(p.panel_text), r.size.width - px(TEXT_X + CHECK_X))
                }
                _ => {
                    let ink = match heading_hue(v, h) {
                        Some(hue) => style::c(p.hue_text(hue)),
                        None => style::c(p.panel_text),
                    };
                    style::label_on_baseline_fit(g, r.left() + px(CHECK_X), base, style::Align::Left, text.clone(), style::PLAIN_SIZE, bold, ink, r.size.width - px(2.0 * CHECK_X))
                }
            };
            // (How many it has, quiet, after its name, as a lane says on
            // the board; a day's date is after its name already.)
            if !matches!(h, Heading::Day(_)) || v.place == Place::Today {
                style::label_on_baseline(g, at.right() + px(8.0), base, style::Align::Left, under(v, i).to_string(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
            }
            style::hline(g, r.left() + px(CHECK_X), r.right() - px(CHECK_X), r.bottom() - px(2.0), rule(&p));
            g.agent(agent::Node { role: "Heading", name: Some(text.into()), bounds: at, enabled: true, ..Default::default() });
        }
        Some(Row::ToDo(n)) => {
            let Some(item) = v.book.items.get(*n) else { return };
            let t = &item.todo;
            let selected = v.selected.as_deref() == Some(t.uid.as_str());
            // (Struck and quiet the moment it's checked, while it lingers
            // too: what happened is seen before it goes.)
            let done = !t.open();
            let checked = done;
            let ground = if selected { p.list_selected_background } else { p.document_background };
            if selected {
                style::fill_rect(g, r, style::c(ground));
            }
            let text_ink = if selected {
                style::c(p.list_selected_item_text)
            } else if done {
                style::secondary_text(&p)
            } else {
                style::c(p.panel_text)
            };
            let quiet = if selected { style::c(p.list_selected_item_text) } else { style::secondary_text(&p) };
            let hue = p.hue(v.hue_of(&item.list));
            let check = Bounds { origin: point(r.left() + px(CHECK_X), r.center().y - px(CHECK / 2.0)), size: size(px(CHECK), px(CHECK)) };
            paint_check(g, check, hue, checked, ground);
            g.agent(agent::Node {
                role: "CheckBox",
                name: Some(olo_kit::tr!("Done: {}", t.title).into()),
                bounds: check,
                enabled: true,
                clickable: true,
                checked: Some(checked),
                ..Default::default()
            });
            // At the right, in columns that hold down the list: its due at
            // the end; before it its list, a dot and a name. (Smaller than
            // its title, on its title's line: each centred, they were a
            // pixel off it at 1.25× and 1.5×.)
            let base = style::baseline_at(g, r.center().y, style::PLAIN_SIZE);
            let mut right = r.right() - px(CHECK_X);
            let mut said = Vec::new();
            if v.due_col > 0.0 {
                if let Some((words, late)) = due_said(v, item) {
                    let ink = if late && !selected { style::c(p.hue_text(olo_theme::Hue::Red)) } else { quiet };
                    style::label_on_baseline(g, right, base, style::Align::Right, words, style::DETAIL_SIZE, plain.clone(), ink);
                }
                right -= px(v.due_col + COLUMN_GAP);
            }
            if let Some(due) = &t.due {
                let words = words::due_in_words(due, &v.now, v.h24);
                said.push(if t.open() && overdue(due, &v.now) { olo_kit::tr!("overdue, {}", words) } else { olo_kit::tr!("due {}", words) });
            }
            if v.list_col > 0.0 {
                let left = right - px(v.list_col);
                if let Some(list) = v.book.list(&item.list) {
                    let dot = style::snap_centred(g, point(left + px(DOT / 2.0), r.center().y), DOT, DOT);
                    g.paint_quad(olo_kit::fill(dot, style::c(hue)).corner_radii(dot.size.width / 2.0));
                    style::label_on_baseline_fit(g, left + px(DOT + DOT_GAP), base, style::Align::Left, list.name.clone(), style::DETAIL_SIZE, plain.clone(), quiet, px(v.list_col - DOT - DOT_GAP));
                    said.push(list.name.clone());
                }
                right = left - px(COLUMN_GAP);
            }
            // Its title, and beside it the marks of what it has.
            let has = [(t.notes.is_empty(), Mark::Notes), (t.attachments.is_empty(), Mark::Attachments), (t.remind.is_none(), Mark::Reminder)];
            let shown: Vec<Mark> = has.iter().filter(|(none, _)| !none).map(|(_, m)| *m).collect();
            let marks_w = if shown.is_empty() { 0.0 } else { TITLE_GAP + shown.len() as f32 * marks::MARK + (shown.len() - 1) as f32 * MARK_GAP };
            let room = (right - (r.left() + px(TEXT_X)) - px(marks_w)).max(px(0.0));
            let title = style::label_fit(g, r.left() + px(TEXT_X), r.center().y, style::Align::Left, t.title.clone(), style::PLAIN_SIZE, plain, text_ink, room);
            // (Struck through among what's still to do; not in Done, where
            // every row is, and a page of struck lines is only harder to
            // read.)
            if done && v.place != Place::Done {
                style::hline(g, title.left(), title.right(), title.center().y, text_ink);
            }
            let mut x = title.right() + px(TITLE_GAP);
            for mark in shown {
                let b = Bounds { origin: point(x, r.center().y - px(marks::MARK / 2.0)), size: size(px(marks::MARK), px(marks::MARK)) };
                marks::paint(g, mark, b, quiet);
                x = b.right() + px(MARK_GAP);
                said.push(olo_kit::words::tr(mark.word()).into());
            }
            g.agent(agent::Node {
                role: "ListItem",
                name: Some(t.title.clone().into()),
                value: Some(said.join(", ").into()),
                bounds: r,
                enabled: true,
                clickable: true,
                selected: Some(selected),
                ..Default::default()
            });
        }
        Some(Row::Nothing) => {
            style::label(g, r.left() + px(TEXT_X), r.center().y, style::Align::Left, olo_kit::words::tr("No to-dos"), style::PLAIN_SIZE, plain, style::secondary_text(&p));
        }
        None => {}
    }
}

/// The row a new to-do is typed in, the list's first: a dashed circle
/// where its check will be, the field where its title will; and at its
/// right, as it's typed, what its words say of its due, with a way to
/// keep them as words.
fn entry_row(v: &mut ToDoApp, cx: &mut Cx<ToDoApp>) -> Element {
    let p = theme::palette();
    let plus = canvas(|g, b| {
        let p = theme::palette();
        let r = Bounds { origin: point(b.left() + px(CHECK_X), b.center().y - px(CHECK / 2.0)), size: size(px(CHECK), px(CHECK)) };
        paint_new_circle(g, r, CHECK, style::secondary_text(&p), style::c(p.document_background));
    })
    .paint_key(1)
    .w(px(TEXT_X))
    .h_full()
    .flex_shrink_0()
    .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut ToDoApp, _: &MouseDownEvent, cx| v.entry.focus(cx)));
    // (The due read from its words, as a row says one, where the rows'
    // dues are: blue while it's not yet kept.)
    let said = match (&v.typed.due, &v.typed.said) {
        (Some(due), Some(_)) => Some(words::due(due, &v.now, v.h24)),
        _ => None,
    };
    // (Where it will go, where that isn't plain: a board's lane asked for.)
    let into = match (&v.entry_section, v.layout()) {
        (Some(Some(section)), crate::Layout::Board) => Some(section.clone()),
        _ => None,
    };
    // (The field with no frame of its own: the row is its frame.)
    let empty = v.entry.read(cx).text().is_empty();
    let hint = empty.then(|| div().absolute().inset_0().flex().flex_row().items_center().child(olo_kit::widgets::StringView::new("New To-Do").dim(true)));
    let field = div().relative().flex_1().min_w_0().h(px(olo_kit::metrics::CONTROL_HEIGHT)).children(hint).child(div().absolute().inset_0().child(&v.entry));
    let mut row = div().w_full().h(px(ROW + 2.0)).flex().flex_row().items_center().child(plus).child(field);
    if let Some(section) = into {
        row = row.child(div().flex_shrink_0().pl(px(COLUMN_GAP)).child(olo_kit::widgets::StringView::new(olo_kit::tr!("in {}", section)).dim(true)));
    }
    if let Some(due) = said {
        let ink = style::c(crate::board::due_ink(&p, false));
        row = row.child(div().flex_shrink_0().pl(px(COLUMN_GAP)).pr(px(KEEP_GAP)).child(olo_kit::widgets::StringView::new(due).small(true).color(ink))).child(marks::glyph_button(
            "keep-words",
            Glyph::Cross,
            "Not a due: keep the words in its title",
            KEEP,
            Look::Quiet,
            cx.listener(|_: &mut ToDoApp, _: &MouseDownEvent, cx| cx.send(Msg::KeepWords)),
        ));
    }
    let line = canvas(|g, b| {
        let p = theme::palette();
        style::hline(g, b.left(), b.right(), b.bottom() - px(1.0), rule(&p));
    })
    .paint_key(2)
    .absolute()
    .inset_0();
    // (The ×'s mark, not its square, ends where the rows' dues do.)
    div().relative().w_full().flex_shrink_0().pr(px(CHECK_X - (KEEP - 7.0) / 2.0)).child(line).child(row).into_element()
}

/// The place's rows under the row a new one is typed in, or a word where
/// it has none.
pub fn view(v: &mut ToDoApp, cx: &mut Cx<ToDoApp>) -> Element {
    let n = v.rows.len();
    // (The rows' places are for the display's scale: made again where it
    // changed.)
    if v.tops_scale != cx.scale_factor() {
        v.tops_scale = cx.scale_factor();
        v.row_tops = tops(&v.rows, v.tops_scale);
    }
    let entry = (v.place != Place::Done).then(|| entry_row(v, cx));
    let body = if n == 0 && !v.loading {
        let (text, detail) = match &v.place {
            Place::Today => ("Nothing due today", "What's due today, and anything overdue, shows here."),
            Place::Upcoming => ("Nothing coming up", "To-dos due after today show here, by day."),
            Place::All => ("Nothing to do", "Every list's to-dos show here."),
            Place::Done => ("Nothing done yet", "What's checked off shows here, the newest first."),
            Place::List(_) => ("Nothing to do here", "Type a to-do above, and press Return."),
        };
        EmptyState::new(text).icon("todo").detail(detail).into_element()
    } else {
        let rows_el = olo_kit::sized_rows(v.row_tops.clone(), move |near| {
            near.map(|i| olo_kit::view_canvas::<ToDoApp>(move |v, g, r| paint_row(v, g, r, i)).key_by::<ToDoApp>(move |v| row_key(v, i)).id(i).w_full()).collect()
        })
        .row_key_by::<ToDoApp>(row_key)
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .top(px(PAD));
        // (Its rows are on a grid of their own: not held level with the
        // labels of the pane beside them.)
        let grid = canvas(|g, b| olo_kit::review::own_grid(g, b)).absolute().inset_0();
        let inner = div()
            .relative()
            .w_full()
            .h(px(v.row_tops.last().copied().unwrap_or(0.0) + 2.0 * PAD))
            .min_h_full()
            .child(grid)
            .child(rows_el)
            .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut ToDoApp, e: &MouseDownEvent, cx| press(v, e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|v: &mut ToDoApp, e: &MouseDownEvent, cx| menu(v, e, cx)))
            // (Files dropped on a to-do are attached to it.)
            .on_drop(cx.listener(|v: &mut ToDoApp, paths: &olo_kit::ExternalPaths, cx| {
                let at = cx.mouse_position();
                if let Some(uid) = row_at(v, at).and_then(|(row, _)| uid_at(v, row)) {
                    v.select(Some(uid.clone()), cx);
                    cx.send(Msg::Attach(uid, paths.paths().to_vec()));
                }
            }));
        ScrollView::new(&v.list_scroll).border(false).auto_hide(true).child(inner).into_element()
    };
    div().size_full().flex().flex_col().children(entry).child(div().flex_1().min_h_0().child(body)).into_element()
}

/// Which row a point in the window is on, and whether it's on the row's
/// check circle.
fn row_at(v: &ToDoApp, at: olo_kit::Point<Pixels>) -> Option<(usize, bool)> {
    let b = v.list_scroll.bounds();
    if !b.contains(&at) {
        return None;
    }
    let top = b.top() + v.list_scroll.offset().y + px(PAD);
    let y = f32::from(at.y - top);
    // (The first row that ends past the point.)
    let row = v.row_tops.get(1..).unwrap_or_default().partition_point(|end| *end <= y);
    if y < 0.0 || row >= v.rows.len() {
        return None;
    }
    let x = f32::from(at.x - b.left());
    Some((row, (CHECK_X - 6.0..=CHECK_X + CHECK + 6.0).contains(&x)))
}

fn uid_at(v: &ToDoApp, row: usize) -> Option<String> {
    match v.rows.get(row)? {
        Row::ToDo(n) => v.book.items.get(*n).map(|i| i.todo.uid.clone()),
        Row::Heading(_) | Row::Nothing => None,
    }
}

fn press(v: &mut ToDoApp, e: &MouseDownEvent, cx: &mut Cx<ToDoApp>) {
    // (The to-dos take the keyboard: ↑ ↓, Space, Return and Delete are
    // theirs from here, not a field's that had it.)
    v.focus.focus(cx);
    let Some((row, on_check)) = row_at(v, e.position) else {
        v.select(None, cx);
        return;
    };
    let Some(uid) = uid_at(v, row) else { return };
    if on_check {
        v.check(&uid, cx);
        return;
    }
    v.select(Some(uid), cx);
    // (Twice: its details, the caret in its title.)
    if e.click_count >= 2 {
        v.details_pane.show_pane();
        if !v.settings.details {
            v.settings.details = true;
            v.save();
        }
        v.title.focus(cx);
    }
    cx.invalidate();
}

/// A to-do's menu (a secondary click): what the menus say of it.
fn menu(v: &mut ToDoApp, e: &MouseDownEvent, cx: &mut Cx<ToDoApp>) {
    let Some((row, _)) = row_at(v, e.position) else { return };
    let Some(uid) = uid_at(v, row) else { return };
    v.select(Some(uid), cx);
    let done = v.selected_item().is_some_and(|i| !i.todo.open());
    let menu = olo_kit::Menu::new("To-Do")
        .item(if done { "Mark as Not Done" } else { "Mark as Done" }, "check")
        .separator()
        .item("Due Today", "due:today")
        .item("Due Tomorrow", "due:tomorrow")
        .item("Due Next Week", "due:next-week")
        .item("No Due Date", "due:none")
        .separator()
        .item("Delete", "delete");
    let view = cx.view_ref();
    olo_kit::menu::context_menu(cx, menu, e.position, move |cmd, cx| {
        let cmd = cmd.to_owned();
        view.update(cx, move |v: &mut ToDoApp, cx| {
            use olo_kit::View as _;
            v.command(&cmd, cx);
        });
    });
}

/// The `n`th to-do's row brought into view.
pub fn reveal(v: &ToDoApp, n: usize) {
    let mut seen = 0;
    for (i, r) in v.rows.iter().enumerate() {
        if matches!(r, Row::ToDo(_)) {
            if seen == n {
                let top = |row: usize| PAD + v.row_tops.get(row).copied().unwrap_or(0.0);
                // (Its heading too, where it's the first under one.)
                let from = if i > 0 && matches!(v.rows[i - 1], Row::Heading(_)) { i - 1 } else { i };
                v.list_scroll.reveal_span(px(top(from)), px(top(i + 1)));
                return;
            }
            seen += 1;
        }
    }
}
