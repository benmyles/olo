//! The to-do chosen, beside the list, as a to-do and not a form: its
//! check and its title as a heading that's typed in where it is; then
//! when (its due, a day and a time typed as words or numbers, with a
//! menu of the usual days and a month to pick any other from; its
//! reminder), where (its list, its section), its notes, and what's
//! attached; Delete and when it was made at the pane's foot. What's
//! changed is kept as it's changed (typed: a moment after the last key,
//! at Return, or on leaving it), each change one Undo takes back.

use olo_calendar::event::When;
use olo_kit::{
    Bounds, ClickEvent, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, ParentElement as _, SharedString, Styled as _, canvas, control_canvas, div,
    metrics as m, point, px, size, style, theme,
    widgets::{Button, EmptyState, MenuField, MonthGrid, ScrollView, StringView, TextControl, month as mini},
};
use olo_todo::{Remind, ToDo, item::Kind, quick, view as places};

use crate::{
    Msg, ToDoApp, list,
    marks::{self, Glyph, Look},
    view::{self, Which},
    words,
};

/// The labels' column.
const DIVIDER: f32 = 62.0;
/// The pane's inset, and the gap between its groups (a line in it).
pub const INSET: f32 = 16.0;
const GROUP: f32 = 21.0;
/// The notes' well: how many lines tall at the least, and how tall at
/// the most (what's longer scrolls in it).
const NOTES_LINES: f32 = 3.0;
const NOTES_MOST: f32 = 280.0;
/// Between the foot's two lines (when it was made, when done).
const FACTS: f32 = 15.0;
/// The room over the month in its well, and under it (more: its name's
/// row has air of its own over its capitals, its last week has none
/// under its days, and the chosen one's ground stood on the bevel).
const MONTH_OVER: f32 = 4.0;
const MONTH_UNDER: f32 = 10.0;
/// The due's time field: "12:30 PM" and a little.
const TIME: f32 = 68.0;
/// An attachment's row, and the pane's small buttons (its +, a row's ×:
/// one size, so they stand in one column at the pane's right).
const ATTACHED: f32 = 24.0;
const SMALL: f32 = 20.0;
/// The pane's foot.
pub const FOOT: f32 = m::CONTROL_HEIGHT + 2.0 * m::WINDOW_INSET;

/// The due the day and time fields say; or what's wrong with them.
pub fn read_due(v: &ToDoApp, cx: &Cx<ToDoApp>) -> Result<Option<When>, String> {
    let (day, time) = (v.due_day.read(cx).text().to_owned(), v.due_time.read(cx).text().to_owned());
    quick::fields(&day, &time, &v.now, v.month_first).map_err(|which| match which {
        quick::Field::Day => olo_kit::tr!("“{}” isn’t a day I can read: try tomorrow, Friday, Nov 21 or 2026-11-21.", day.trim()),
        quick::Field::Time => olo_kit::tr!("“{}” isn’t a time I can read: try 14:30 or 2:30 PM.", time.trim()),
    })
}

/// A to-do's due set: a reminder that went by a due it no longer has
/// goes with it; one for a day's due and one for a time's are each the
/// other's nearest.
pub fn set_due(todo: &mut ToDo, due: Option<When>) {
    let was_day = todo.due.as_ref().map(When::is_date);
    let now_day = due.as_ref().map(When::is_date);
    todo.due = due;
    if let Some(Remind::Before(_)) = todo.remind {
        match (was_day, now_day) {
            (_, None) => todo.remind = None,
            (Some(false), Some(true)) => todo.remind = Some(Remind::Before(words::BEFORE_A_DAY[0].0)),
            (Some(true), Some(false)) => todo.remind = Some(Remind::Before(words::BEFORE_A_TIME[0].0)),
            _ => {}
        }
    }
}

/// The reminders offered for a to-do, each with what choosing it keeps:
/// none first; then those that suit its due (a time's, or a day's); and
/// the one it has, where that's none of them (another program's).
pub fn reminders(todo: &ToDo) -> Vec<(Option<Remind>, SharedString)> {
    let mut out: Vec<(Option<Remind>, SharedString)> = vec![(None, "None".into())];
    match &todo.due {
        Some(When::Date(_)) => out.extend(words::BEFORE_A_DAY.iter().map(|(s, w)| (Some(Remind::Before(*s)), SharedString::from(*w)))),
        Some(_) => out.extend(words::BEFORE_A_TIME.iter().map(|(s, w)| (Some(Remind::Before(*s)), SharedString::from(*w)))),
        None => {}
    }
    if let Some(r) = &todo.remind
        && !out.iter().any(|(o, _)| o.as_ref() == Some(r))
    {
        let said = match r {
            Remind::Before(s) if *s >= 0 => olo_kit::tr!("{} minutes before", s / 60),
            Remind::Before(s) => olo_kit::tr!("{} minutes after", -s / 60),
            Remind::At(_) => olo_kit::words::tr("At a time of its own").into(),
        };
        out.push((Some(r.clone()), said.into()));
    }
    out
}

/// An address typed for a link, as it's kept: with `https://` where it
/// names no scheme ("example.com/trip"). None: not an address.
pub fn link(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() || typed.contains(char::is_whitespace) {
        return None;
    }
    let schemed =
        typed.split_once(':').is_some_and(|(scheme, rest)| !rest.is_empty() && !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && !scheme.contains('.'));
    if schemed {
        return Some(typed.to_owned());
    }
    // (A host: a dot in it, with something either side.)
    let host = typed.split('/').next().unwrap_or_default();
    (host.contains('.') && !host.starts_with('.') && !host.ends_with('.')).then(|| format!("https://{typed}"))
}

/// What a link is called where it has no name of its own: its address
/// without the scheme.
pub fn link_name(uri: &str) -> String {
    let bare = uri.split_once("://").map_or_else(|| uri.split_once(':').map_or(uri, |(_, rest)| rest), |(_, rest)| rest);
    bare.trim_end_matches('/').to_owned()
}

fn day_of(d: jiff::civil::Date) -> mini::Day {
    mini::Day::new(i32::from(d.year()), d.month() as u8, d.day() as u8)
}

/// A line across the pane, between its groups.
fn rule() -> Element {
    canvas(|g, b| {
        let p = theme::palette();
        style::hline(g, b.left(), b.right(), b.center().y, style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.16)));
    })
    .paint_key(3)
    .w_full()
    .h(px(GROUP))
    .flex_shrink_0()
    .into_element()
}

/// One attachment's row: what it is in a slot an icon wide (its kind's
/// icon; a picture: itself, small), its name and, quieter, where a
/// link leads; a click opens it. Its × takes it off.
fn attachment(a: &olo_todo::Attachment, i: usize, cx: &mut Cx<ToDoApp>) -> Element {
    let kind = a.kind();
    let (name, uri) = (a.name.clone(), a.uri.clone());
    let path = if kind == Kind::Picture { a.path() } else { None };
    // (A link's host, where its name doesn't say it already.)
    let host = (kind == Kind::Link).then(|| link_name(&uri).split('/').next().unwrap_or_default().to_owned()).filter(|h| !name.contains(h.as_str()));
    let row = control_canvas(move |g, b, state| {
        let p = theme::palette();
        if state.hovered {
            style::fill_rect(g, style::snap(g, b), style::shade(&p, p.panel_background, 1.06));
        }
        let plain = style::ui_font(FontWeight::NORMAL);
        let slot = Bounds { origin: point(b.left(), b.center().y - px(8.0)), size: size(px(16.0), px(16.0)) };
        // (A picture: its middle, filling the slot, a line round it. Made
        // as tall as the slot and cut at its sides, or, a tall one, as
        // wide and cut above and below: never squeezed, never a strip.)
        let pictured = path.as_ref().and_then(|path| {
            let s = g.scale_factor();
            let side = (16.0 * s).round() as u32;
            let wide = olo_kit::pictures::fitted(g, path, (side * 6, side))?;
            let img = if wide.size(0).width.0 as u32 >= side { wide } else { olo_kit::pictures::fitted(g, path, (side, side * 6))? };
            let isz = img.size(0);
            let frame = style::snap(g, slot);
            let at = style::snap_centred(g, frame.center(), isz.width.0 as f32 / s, isz.height.0 as f32 / s);
            g.with_content_mask(Some(olo_kit::ContentMask { bounds: frame }), |g| {
                let _ = g.paint_image(at, Default::default(), img, 0, false);
            });
            // (Its edge as dark as an icon's own: a pale picture in a
            // pale frame read as a thing turned off beside the icons.)
            style::frame_rect(g, frame, style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.7)));
            Some(())
        });
        if pictured.is_none() {
            olo_kit::icons::paint(
                g,
                slot,
                match kind {
                    Kind::Link => "internet",
                    Kind::Picture => "image",
                    Kind::File => "document",
                },
            );
        }
        let left = slot.right() + px(8.0);
        let room = b.right() - px(2.0) - left;
        let shown = style::label_fit(g, left, b.center().y, style::Align::Left, name.clone(), style::PLAIN_SIZE, plain.clone(), style::c(p.panel_text), room);
        if state.hovered {
            style::hline(g, shown.left(), shown.right(), shown.bottom() - px(1.0), style::c(p.panel_text));
        }
        if let Some(host) = &host {
            let rest = b.right() - px(2.0) - shown.right() - px(8.0);
            if rest > px(40.0) {
                style::label_fit(g, shown.right() + px(8.0), b.center().y, style::Align::Left, host.clone(), style::DETAIL_SIZE, plain, style::secondary_text(&p), rest);
            }
        }
        g.agent(olo_kit::agent::Node { role: "Link", name: Some(name.clone().into()), value: Some(uri.clone().into()), bounds: b, enabled: true, clickable: true, ..Default::default() });
    })
    .paint_key(style::key((&a.name, &a.uri, kind == Kind::Picture)))
    .flex_1()
    .min_w_0()
    .h(px(ATTACHED))
    .on_mouse_down(MouseButton::Left, cx.listener(move |_: &mut ToDoApp, _: &MouseDownEvent, cx| cx.send(Msg::OpenAttachment(i))));
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .child(row)
        .child(marks::glyph_button(
            format!("detach-{i}"),
            Glyph::Cross,
            olo_kit::tr!("Remove {}", a.name),
            SMALL,
            Look::Quiet,
            cx.listener(move |_: &mut ToDoApp, _: &MouseDownEvent, cx| cx.send(Msg::Detach(i))),
        ))
        .into_element()
}

/// A group's heading in the pane, as the list's own headings are said.
fn heading(text: &'static str) -> StringView {
    StringView::new(text).bold(true)
}

/// What's attached: a heading with a + (a file, a link, what's on the
/// clipboard), then a row each.
fn attachments(t: &ToDo, cx: &mut Cx<ToDoApp>) -> Element {
    let add = marks::glyph_button(
        "attach",
        Glyph::Plus,
        "Attach a file or a link",
        SMALL,
        Look::Raised,
        cx.listener(|_: &mut ToDoApp, e: &MouseDownEvent, cx| {
            let menu = olo_kit::Menu::new("Attach").item("File…", "attach-file").item("Link…", "attach-link").separator().item("What's on the Clipboard", "paste-attachment");
            let view = cx.view_ref();
            olo_kit::menu::context_menu(cx, menu, e.position, move |cmd, cx| {
                let cmd = cmd.to_owned();
                view.update(cx, move |v: &mut ToDoApp, cx| {
                    use olo_kit::View as _;
                    v.command(&cmd, cx);
                });
            });
        }),
    );
    let mut col = div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(m::TIGHT))
        .child(div().w_full().h(px(SMALL)).flex().flex_row().items_center().child(heading("Attachments")).child(div().flex_1()).child(add));
    if t.attachments.is_empty() {
        col = col.child(StringView::new("No attachments").dim(true));
    }
    for (i, a) in t.attachments.iter().enumerate() {
        col = col.child(attachment(a, i, cx));
    }
    col.into_element()
}

/// The pane: the chosen to-do's, or a word where none is. `alone`: in
/// front of the list in a window too narrow for both, with a way back.
/// `width`, `height`: the pane's.
pub fn pane(v: &mut ToDoApp, alone: bool, width: f32, height: f32, cx: &mut Cx<ToDoApp>) -> Element {
    let p = theme::palette();
    let head = alone.then(|| {
        div().flex().flex_row().items_center().gap(px(m::TIGHT)).h(px(view::HEADER)).px(px(INSET)).flex_shrink_0().child(view::back(cx, Which::Details)).child(StringView::new("To-Do").bold(true))
    });
    let Some(item) = v.selected_item().cloned() else {
        let empty = EmptyState::new("No To-Do Chosen").icon("todo").detail("Choose a to-do to see its details.");
        // (No foot with nothing in it: kept, its line over an empty band
        // read as something that hadn't loaded. Both reviews, 2026-11-20.)
        return div().size_full().flex().flex_col().children(head).child(div().flex_1().min_h_0().child(empty)).into_element();
    };
    let t = &item.todo;
    let inner = width - 2.0 * INSET;
    let row = || div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT));
    // Its check, and its title: a heading, typed in where it is.
    let hue = p.hue(v.hue_of(&item.list));
    let (checked, uid) = (!t.open(), t.uid.clone());
    let check = canvas(move |g, b| {
        let p = theme::palette();
        let c = Bounds { origin: point(b.left(), b.top() + px(1.0)), size: size(px(list::CHECK), px(list::CHECK)) };
        list::paint_check(g, c, hue, checked, p.panel_background);
        g.agent(olo_kit::agent::Node { role: "CheckBox", name: Some("Done".into()), bounds: c, enabled: true, clickable: true, checked: Some(checked), ..Default::default() });
    })
    .paint_key(style::key((hue, checked)))
    .w(px(list::CHECK))
    .h(px(list::CHECK + 2.0))
    .flex_shrink_0()
    .on_mouse_down(MouseButton::Left, cx.listener(move |_: &mut ToDoApp, _: &MouseDownEvent, cx| cx.send(Msg::Check(uid.clone()))));
    let title = div().w_full().flex_shrink_0().flex().flex_row().items_start().gap(px(10.0)).child(check).child(div().flex_1().min_w_0().child(&v.title));
    // Where it lives: its list, and its section where the list has any.
    let lists = v.lists();
    let list_names: Vec<SharedString> = lists.iter().map(|l| SharedString::from(l.name.clone())).collect();
    let list_at = lists.iter().position(|l| l.id == item.list).unwrap_or(0);
    let mut where_ = div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(m::STACK))
        .child(MenuField::new("todo-list", list_names, list_at).label("List:").divider(DIVIDER).width(inner).enabled(lists.len() > 1).on_change(cx.msg_with(|i: &usize| Msg::MoveTo(*i))));
    let sections = places::sections(&v.book, &item.list);
    if !sections.is_empty() {
        let mut names: Vec<SharedString> = vec!["None".into()];
        names.extend(sections.iter().map(|s| SharedString::from(s.clone())));
        let section_at = t.section.as_ref().and_then(|s| sections.iter().position(|o| o == s)).map_or(0, |i| i + 1);
        where_ = where_.child(MenuField::new("todo-section", names, section_at).label("Section:").divider(DIVIDER).width(inner).on_change(cx.msg_with(|i: &usize| Msg::Section(*i))));
    }
    // When: its due (a day and a time, typed; the usual days and a month
    // behind the small calendar beside them), and its reminder.
    let more = marks::glyph_button(
        "due-more",
        Glyph::Calendar,
        "Pick a day",
        m::CONTROL_HEIGHT,
        // (Pressed in while its month is showing.)
        if v.month_open { Look::On } else { Look::Raised },
        cx.listener(|v: &mut ToDoApp, e: &MouseDownEvent, cx| {
            let has = v.selected_item().is_some_and(|i| i.todo.due.is_some());
            let menu = olo_kit::Menu::new("Due")
                .item("Today", "due:today")
                .item("Tomorrow", "due:tomorrow")
                .item("Next Week", "due:next-week")
                .separator()
                .item("Pick from a Month", "due:month")
                .checked(v.month_open)
                .separator()
                .item("No Due Date", "due:none")
                .enabled(has);
            let view = cx.view_ref();
            olo_kit::menu::context_menu(cx, menu, e.position, move |cmd, cx| {
                let cmd = cmd.to_owned();
                view.update(cx, move |v: &mut ToDoApp, cx| {
                    use olo_kit::View as _;
                    v.command(&cmd, cx);
                });
            });
        }),
    );
    let mut when = div().w_full().flex_shrink_0().flex().flex_col().gap(px(m::STACK)).child(
        row()
            .child(div().flex_1().min_w_0().child(TextControl::new(cx, &v.due_day).label("Due:").divider(DIVIDER)))
            .child(div().w(px(TIME)).flex_shrink_0().child(TextControl::new(cx, &v.due_time)))
            .child(more),
    );
    if let Some(wrong) = &v.due_wrong {
        when = when.child(div().pl(px(DIVIDER)).child(StringView::new(wrong.clone()).small(true).wrap(true).color(style::c(p.failure))));
    }
    let today = v.now.date();
    let due_day = t.due.as_ref().map(|d| places::due_day(d, &v.zone));
    if v.month_open {
        let shown = v.month.unwrap_or_else(|| {
            let d = due_day.unwrap_or(today);
            (i32::from(d.year()), d.month() as u8)
        });
        let mut month = MonthGrid::new("Due Day", shown.0, shown.1, day_of(today))
            .on_pick(cx.listener(|v: &mut ToDoApp, d: &mini::Day, cx| {
                let Ok(day) = jiff::civil::Date::new(d.year as i16, d.month as i8, d.day as i8) else { return };
                // (Its time, where it has one, stays.)
                let due = match v.selected_item().and_then(|i| i.todo.due.clone()) {
                    Some(was) if !was.is_date() => {
                        let time = was.moment(&v.zone).time();
                        day.to_datetime(time).to_zoned(v.zone.clone()).map(When::Zoned).unwrap_or(When::Date(day))
                    }
                    _ => When::Date(day),
                };
                v.month = None;
                cx.send(Msg::Due(Some(due)));
            }))
            .on_turn(cx.listener(|v: &mut ToDoApp, (year, month): &(i32, u8), cx| {
                v.month = Some((*year, *month));
                cx.invalidate();
            }));
        if let Some(d) = due_day {
            month = month.chosen(day_of(d), day_of(d));
        }
        // (In a well as wide as the fields over it, as their own are: a
        // month loose on the pane's ground read as stray text.)
        let well = canvas(|g, b| {
            let p = theme::palette();
            let b = style::snap(g, b);
            style::fill_rect(g, b, style::c(p.document_background));
            style::sunken(g, b, &p, false);
        })
        .paint_key(5)
        .absolute()
        .inset_0();
        when = when.child(
            div().pl(px(DIVIDER)).child(
                div()
                    .relative()
                    .w_full()
                    .h(px(mini::HEIGHT + MONTH_OVER + MONTH_UNDER))
                    .child(well)
                    .child(div().absolute().inset_0().pt(px(MONTH_OVER)).flex().flex_row().items_start().justify_center().child(month)),
            ),
        );
    }
    let offered = reminders(t);
    let at = offered.iter().position(|(r, _)| *r == t.remind).unwrap_or(0);
    let names: Vec<SharedString> = offered.iter().map(|(_, w)| w.clone()).collect();
    // (Nothing to remind of once it's done: its row stays, not to be
    // changed. Taken away, what's under it jumped up as a to-do was
    // checked.)
    when = when.child(
        MenuField::new("todo-remind", names, at)
            .label("Remind:")
            .divider(DIVIDER)
            .width(inner)
            .enabled(t.open() && (t.due.is_some() || t.remind.is_some()))
            .on_change(cx.msg_with(|i: &usize| Msg::Remind(*i))),
    );
    // Its notes: several lines, in a well that takes what room the pane
    // has left (what's longer scrolls in it).
    let notes_focused = v.notes.read(cx).is_focused();
    // (As tall as what's written, between a few lines and a good many:
    // an empty well the height of the pane was the loudest thing in the
    // window.)
    let lines = v.notes.read(cx).rows_at(px(inner - 2.0 * m::TIGHT - 20.0)) as f32;
    let line = f32::from(olo_kit::widgets::TextField::line_height_for(cx.scale_factor(), style::PLAIN_SIZE));
    // (Its least in whole points, not in the lines' own height, which is
    // a little other at each scale: the well, and all under it, stood
    // three points higher at 1× than at 2×.)
    let least = NOTES_LINES * (style::PLAIN_SIZE * style::LINE_HEIGHT).ceil();
    let notes_h = ((lines * line).max(least) + m::TIGHT + 6.0).min(NOTES_MOST);
    let notes = div().w_full().flex_shrink_0().flex().flex_col().gap(px(m::TIGHT)).child(heading("Notes")).child(div().w_full().h(px(notes_h)).child(
        ScrollView::new(&v.notes_scroll).auto_hide(true).focused(notes_focused).background(p.document_background).child(
            div().min_h(px(notes_h - 6.0)).flex().flex_col().px(px(m::TIGHT)).py(px(m::TIGHT / 2.0)).child(&v.notes).child(div().flex_1().on_mouse_down(
                MouseButton::Left,
                cx.listener(|v: &mut ToDoApp, _: &MouseDownEvent, cx| {
                    v.notes.focus(cx);
                    v.notes.update(cx, |f, cx| f.caret_to_end(cx));
                }),
            )),
        ),
    ));
    let attached = attachments(t, cx);
    // (All of it scrolls where the window is too short for it: a tile a
    // third of a screen's height. With room, it's as tall as the pane,
    // and its notes take what's left.)
    let uid = t.uid.clone();
    let room = height - FOOT - 1.0 - if alone { view::HEADER } else { 0.0 };
    let all = div()
        .w_full()
        .min_h(px(room.max(0.0)))
        .flex()
        .flex_col()
        .px(px(INSET))
        .pt(px(view::TITLE_TOP))
        .pb(px(m::SPACING))
        .child(title)
        .child(rule())
        .child(where_)
        .child(rule())
        .child(when)
        .child(rule())
        .child(notes)
        .child(div().h(px(m::SPACING)).flex_shrink_0())
        .child(attached);
    // Its foot: Delete, and at its right, small, when it was made and
    // (under that) when it was done.
    let on = |at: jiff::Timestamp| words::in_words(at.to_zoned(v.zone.clone()).date(), today);
    let made = t.created.map(|c| olo_kit::tr!("Made {}", on(c)));
    let done = t.done_at.filter(|_| !t.open()).map(|d| olo_kit::tr!("Done {}", on(d)));
    let facts = canvas({
        let (made, done) = (made.clone(), done.clone());
        move |g, b| {
            let p = theme::palette();
            let (ink, font) = (style::secondary_text(&p), style::ui_font(FontWeight::NORMAL));
            match (&made, &done) {
                // Two: one over the other, about the foot's middle.
                (Some(first), Some(second)) => {
                    for (said, dy) in [(first, -FACTS / 2.0), (second, FACTS / 2.0)] {
                        style::label_fit(g, b.right(), b.center().y + px(dy), style::Align::Right, said.clone(), style::DETAIL_SIZE, font.clone(), ink, b.size.width);
                    }
                }
                // One: on the line Delete's word stands on.
                (Some(one), None) | (None, Some(one)) => {
                    let base = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
                    style::label_on_baseline_fit(g, b.right(), base, style::Align::Right, one.clone(), style::DETAIL_SIZE, font, ink, b.size.width);
                }
                (None, None) => {}
            }
        }
    })
    .paint_key(style::key((&made, &done)))
    .flex_1()
    .min_w_0()
    .h_full();
    let foot = div()
        .w_full()
        .h(px(FOOT))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .px(px(INSET))
        .child(Button::new("Delete").id("delete-todo").on_click(cx.listener(|v: &mut ToDoApp, _: &ClickEvent, cx| v.delete_selected(cx))))
        .child(div().w(px(m::SPACING)).flex_shrink_0())
        .child(facts);
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        // (Files dropped on it are attached to it.)
        .on_drop(cx.listener(move |_: &mut ToDoApp, paths: &olo_kit::ExternalPaths, cx| cx.send(Msg::Attach(uid.clone(), paths.paths().to_vec()))))
        .children(head)
        .child(div().flex_1().min_h_0().child(ScrollView::new(&v.details_scroll).border(false).auto_hide(true).child(all)))
        .child(div().w_full().h(px(1.0)).flex_shrink_0())
        .child(foot)
        .child(view::foot_line(false))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_typed() {
        assert_eq!(link("example.com/trip").as_deref(), Some("https://example.com/trip"));
        assert_eq!(link("https://example.com").as_deref(), Some("https://example.com"));
        assert_eq!(link("mailto:ann@example.com").as_deref(), Some("mailto:ann@example.com"));
        assert_eq!(link("tel:+15551234").as_deref(), Some("tel:+15551234"));
        for not in ["", "not an address", "word", "example.", ".com"] {
            assert_eq!(link(not), None, "{not}");
        }
        assert_eq!(link_name("https://example.com/trip/"), "example.com/trip");
        assert_eq!(link_name("mailto:ann@example.com"), "ann@example.com");
    }

    #[test]
    fn a_reminder_follows_its_due() {
        let mut t = ToDo::new("a", "a");
        let day = When::Date(jiff::civil::date(2026, 11, 21));
        let time = When::Zoned(jiff::civil::date(2026, 11, 21).at(15, 0, 0, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap());
        set_due(&mut t, Some(time.clone()));
        assert_eq!(reminders(&t).len(), 1 + words::BEFORE_A_TIME.len());
        t.remind = Some(Remind::Before(900));
        // To a day alone: the day's first reminder; back to a time: the time's.
        set_due(&mut t, Some(day));
        assert_eq!(t.remind, Some(Remind::Before(-9 * 3600)));
        assert_eq!(reminders(&t).len(), 1 + words::BEFORE_A_DAY.len());
        set_due(&mut t, Some(time));
        assert_eq!(t.remind, Some(Remind::Before(0)));
        // One another program set is offered as it is; with no due, it goes.
        t.remind = Some(Remind::Before(420));
        assert_eq!(reminders(&t).last().unwrap().1.as_ref(), "7 minutes before");
        set_due(&mut t, None);
        assert_eq!(t.remind, None);
    }
}
