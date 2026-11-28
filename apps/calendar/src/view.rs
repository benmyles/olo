//! The window: the small month and the calendars down the left (each
//! with its colour and a check box to show it), and beside them the days
//! themselves, by Day, Week, Month or as an Agenda, under the arrows that
//! step through them and the name of what shows.

use std::rc::Rc;

use olo_kit::{
    ClickEvent, Cx, Element, Hsla, InteractiveElement as _, IntoElement as _, MenuBar, ParentElement as _, Styled as _, canvas, div, metrics as m, px, style, theme,
    widgets::{ArrowButton, Button, CheckBox, EmptyState, LinkButton, MenuField, MonthGrid, Panes, ScrollView, StringView, TabView, TextControl, button_width, month as mini},
};

use crate::{Calendar, Mode, Msg, agenda, hue_of, layout, month, week, words};

/// The search field's width, and the least it narrows to where the
/// toolbar has less room.
const SEARCH: f32 = 190.0;
const SEARCH_LEAST: f32 = 100.0;
/// The calendars' column: the small month and a little room round it.
pub const SIDEBAR: f32 = mini::WIDTH + 2.0 * m::TIGHT + 4.0;
/// What the views keep beside a column (the calendars', the
/// assistant's): seven days across, each wide enough for a time and a
/// word. Less, the column gives way (CLAUDE.md, "Every tiling layout").
const MAIN_BESIDE: f32 = 7.0 * 60.0;
/// The window's least: a third of the smallest screen, either way, as
/// tiling has it beside Deskbar (349 points of content across, 226
/// down): its days alone, seven across; a few hours of them.
pub const MIN_WIDTH: f32 = 346.0;
pub const MIN_HEIGHT: f32 = 220.0;
/// The views' least height for the empty state's detail (three lines
/// under its line, over its button, a little room round them): less,
/// its line and button alone.
const EMPTY_DETAIL_ROOM: f32 = 130.0;
/// The views' least height beside the preview under them: less, the
/// preview gives way.
const VIEWS_BESIDE_PREVIEW: f32 = 300.0;
/// The views' column shorter than this (a third of a screen's height),
/// Day, Week, Month and Agenda fold from tabs into a pop-up in the
/// toolbar: the tabs' row is the days'.
const TABS_FOLD: f32 = 270.0;
/// The assistant's column, and the least it's any use at.
pub const ASSISTANT: f32 = 264.0;
pub const ASSISTANT_MIN: f32 = 216.0;
/// How many days on the agenda shows.
pub const AGENDA_DAYS: i32 = 56;

fn edge(cx: &olo_kit::WindowCx) -> f32 {
    let s = cx.scale_factor();
    style::line_pixels(s) * 2.0 / s
}

/// Where a pointer at `at` is in a drawing as it was painted (its
/// origin then, `painted`), given where the drawing is now (`now`:
/// `cx.listener_bounds()`). A drawing in a scroll view is painted once
/// and moved by the display thread: what it kept of where things are is
/// where they were painted, not where they show. (The week's hours,
/// scrolled, took a click on an event for one on the empty day behind
/// it.)
pub fn in_drawing(at: olo_kit::Point<olo_kit::Pixels>, painted: olo_kit::Point<olo_kit::Pixels>, now: olo_kit::Bounds<olo_kit::Pixels>) -> olo_kit::Point<olo_kit::Pixels> {
    olo_kit::point(at.x - (now.left() - painted.x), at.y - (now.top() - painted.y))
}

/// How an occurrence's ground is drawn, among the hours and across the
/// days: its fill, and whether a line in its calendar's colour goes
/// round it. An invitation not answered is pale with the line (asked,
/// not yet on); one declined paler still (its title struck through,
/// [`struck`]); one answered maybe, a tentative event and one that takes
/// no time are lighter; the one chosen is always its colour.
pub fn ground(p: &olo_theme::Palette, s: &crate::model::Shown, chosen: bool) -> (olo_theme::Rgba, bool) {
    use olo_calendar::event::Answer;
    let hue = p.hue(s.hue);
    match s.going {
        _ if chosen => (solid(p, hue), false),
        Some(Answer::NotYet) => (olo_theme::mix(hue, p.document_background, 0.85), true),
        Some(Answer::No) => (olo_theme::mix(hue, p.document_background, 0.75), false),
        Some(Answer::Maybe) => (olo_theme::mix(hue, p.document_background, 0.45), false),
        _ if s.free || s.tentative => (olo_theme::mix(hue, p.document_background, 0.45), false),
        _ => (solid(p, hue), false),
    }
}

/// An event filled with its calendar's colour, its words on it: in light,
/// as dark as white words need (4.5:1), one ink on every calendar's
/// events (black on the green beside white on the blue read muddy, and
/// low: Opus's and Neon's reviews, 2026-11-28); in dark, as it is.
pub fn solid(p: &olo_theme::Palette, hue: olo_theme::Rgba) -> olo_theme::Rgba {
    if p.is_dark() { hue } else { olo_theme::readable(hue, olo_theme::WHITE, 4.5) }
}

/// An invitation declined: a line through its title (drawn at `title`).
pub fn struck(g: &mut olo_kit::Painter, s: &crate::model::Shown, title: olo_kit::Bounds<olo_kit::Pixels>, ink: Hsla) {
    if s.going == Some(olo_calendar::event::Answer::No) {
        style::hline(g, title.left(), title.right(), title.center().y, ink);
    }
}

/// What an invitation's answer adds to its name, for an agent (and
/// nothing, of what isn't one, or is accepted).
pub fn going_said(s: &crate::model::Shown) -> &'static str {
    use olo_calendar::event::Answer;
    match s.going {
        Some(Answer::NotYet) => ", not answered",
        Some(Answer::No) => ", declined",
        Some(Answer::Maybe) => ", maybe",
        _ => "",
    }
}

/// A sunken well of `ground` with `content` inside its lines.
fn well(e: f32, ground: Hsla, content: impl olo_kit::IntoElement) -> Element {
    let back = canvas(move |g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, b, ground);
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(style::ink_key(Some(ground))))
    .absolute()
    .inset_0();
    div().relative().size_full().child(back).child(div().absolute().left(px(e)).top(px(e)).right(px(e)).bottom(px(e)).overflow_hidden().child(content)).into_element()
}

pub fn body(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
    let p = theme::palette();
    let vs = cx.viewport_size();
    let (w, h) = (f32::from(vs.width) - 2.0 * m::WINDOW_INSET, f32::from(vs.height));
    let mut row = div().flex_1().min_h_0().flex().flex_row().gap(px(m::SPACING)).px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).pb(px(m::WINDOW_INSET));
    // What's beside the days gives way to them, never squeezing them
    // (CLAUDE.md, "Every tiling layout"): the assistant's column takes
    // what they can spare beside what they keep, up to its width, and
    // the calendars' column what's left after it. A column without the
    // room gives way; View ▸ Show brings it in front of the days, alone,
    // with a way back (the kit's SidePane).
    v.assistant_pane.set_collapsed(!v.settings.assistant);
    v.sidebar_pane.set_collapsed(!v.settings.sidebar);
    let assistant = v.assistant_pane.panes(w, MAIN_BESIDE + m::SPACING);
    let left = if assistant.width() > 0.0 { w - assistant.width() - m::SPACING } else { w };
    let sidebar = if assistant == Panes::Pane { Panes::Rest } else { v.sidebar_pane.panes(left, MAIN_BESIDE + m::SPACING) };
    // The views' column: its toolbar, the days, the preview under them.
    let main_h = h - olo_kit::menu::BAR_HEIGHT - m::SPACING - m::WINDOW_INSET;
    let preview_room = main_h - (main_h / 3.0).max(crate::preview::LEAST) - m::TIGHT >= VIEWS_BESIDE_PREVIEW;
    v.preview_fits = preview_room;
    if assistant == Panes::Pane {
        row = row.child(div().flex_1().min_w_0().h_full().child(assistant_pane(v, w, true, cx)));
    } else if sidebar == Panes::Pane {
        row = row.child(div().flex_1().min_w_0().h_full().child(sidebar_column(v, true, cx)));
    } else {
        if let Panes::Both(width) = sidebar {
            row = row.child(div().w(px(width)).flex_shrink_0().h_full().child(sidebar_column(v, false, cx)));
        }
        let main_w = left - if sidebar.width() > 0.0 { sidebar.width() + m::SPACING } else { 0.0 };
        let folded = main_h < TABS_FOLD;
        let (toolbar, wrapped) = nav(v, main_w, folded, cx);
        // (The views' height: under the toolbar, one row or two.)
        let room = main_h - if wrapped { 2.0 * m::CONTROL_HEIGHT + m::TIGHT } else { m::CONTROL_HEIGHT } - m::TIGHT;
        let mut main = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(m::TIGHT)).child(toolbar).child(div().flex_1().min_h_0().child(views(v, folded, room, cx)));
        // (The event chosen, under the views, a third of their column:
        // View ▸ Show Preview. It gives way where the days would be left
        // too short: a quarter of a screen; and in a narrow column (a
        // third of a screen) shows only while an event is chosen, its
        // hint a third of the window under days cut short: Opus's
        // review, 2026-11-28.)
        if v.settings.preview && preview_room && preview_shows(v.selected.is_some(), main_w) {
            crate::preview::follow(v, cx);
            main = main.child(div().w_full().h(olo_kit::relative(1.0 / 3.0)).min_h(px(crate::preview::LEAST)).flex_shrink_0().pt(px(m::TIGHT)).child(&v.preview));
        }
        row = row.child(main);
        if let Panes::Both(width) = assistant {
            row = row.child(div().w(px(width)).flex_shrink_0().h_full().child(assistant_pane(v, width, false, cx)));
        }
    }
    // (The menus after what shows is settled: View ▸ Show Calendars and
    // Show Assistant are checked as they show now.)
    let bar = MenuBar::of(v, cx, |v, _| v.menus());
    // Keys, where nothing that's typed in has the keyboard: Return opens
    // the event chosen, Delete (or Backspace) deletes it, the arrows step
    // through the days (a week up or down), Escape lets go of what's
    // chosen.
    let keys = cx.listener(|v: &mut Calendar, e: &olo_kit::KeyDownEvent, cx| {
        use olo_kit::View as _;
        if e.keystroke.modifiers.modified() {
            return;
        }
        let step = |v: &mut Calendar, days: i32, cx: &mut Cx<Calendar>| {
            let to = layout::day_of(layout::day_number(v.cursor) + days);
            v.go(to, cx);
        };
        // (An answer on its way is let go of first.)
        if e.keystroke.key == "escape" && v.answering.is_some() {
            v.stop_asking();
            cx.invalidate();
            cx.stop_propagation();
            return;
        }
        // Find, while it shows what it found: the arrows step through
        // it, Return goes to the one chosen, Escape ends the search.
        if !v.finding.is_empty() {
            match e.keystroke.key.as_str() {
                "up" => v.step_found(true, cx),
                "down" => v.step_found(false, cx),
                "enter" => {
                    if let Some(i) = v.found_chosen {
                        cx.send(Msg::FoundOpen(i));
                    }
                }
                "escape" => {
                    v.search.update(cx, |f, cx| f.set_text("", cx));
                    v.find(String::new(), cx);
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        match e.keystroke.key.as_str() {
            "enter" if v.selected.is_some() => {
                v.command("edit-event", cx);
            }
            "delete" | "backspace" if v.selected.is_some() => {
                v.command("delete-event", cx);
            }
            // (A drag under way is let go of first: the event stays.)
            "escape" if v.drag.is_some() || v.day_drag.is_some() => {
                (v.drag, v.day_drag) = (None, None);
                cx.invalidate();
            }
            "escape" if v.selected.is_some() => cx.send(Msg::Select(None)),
            "left" => step(v, -1, cx),
            "right" => step(v, 1, cx),
            "up" => step(v, -7, cx),
            "down" => step(v, 7, cx),
            _ => return,
        }
        cx.stop_propagation();
    });
    // (With the caret in the search field, Up and Down are the list's
    // all the same: taken before the field has them.)
    let in_field = cx.listener(|v: &mut Calendar, e: &olo_kit::KeyDownEvent, cx| {
        let up = match e.keystroke.key.as_str() {
            "up" => true,
            "down" => false,
            _ => return,
        };
        if e.keystroke.modifiers.modified() || v.finding.is_empty() || !v.search.read(cx).is_focused(cx) {
            return;
        }
        v.step_found(up, cx);
        cx.stop_propagation();
    });
    div().size_full().flex().flex_col().bg(style::c(p.panel_background)).on_window_key_down(keys).capture_key_down(in_field).child(bar).child(row).into_element()
}

/// The back arrow of a column shown alone in front of the days (a
/// window too narrow for both): the days again.
fn back(cx: &mut Cx<Calendar>, assistant: bool) -> Element {
    ArrowButton::back("Back to the Days")
        .on_click(cx.listener(move |v: &mut Calendar, _: &ClickEvent, cx| {
            if assistant {
                v.assistant_pane.show_rest();
            } else {
                v.sidebar_pane.show_rest();
            }
            cx.invalidate();
        }))
        .into_element()
}

/// What the view shows, in words, longest first: the month, the week's
/// days, the day; the toolbar takes the longest that fits its room
/// ("Thursday, November 19, 2026", "Thu, Nov 19, 2026", "Nov 19, 2026",
/// "Nov 19").
pub fn titles(v: &Calendar) -> Vec<String> {
    // (Find: how many were found.)
    if !v.finding.is_empty() {
        return vec![match (v.found.len(), v.found_total) {
            (0, _) => olo_kit::words::tr("Nothing Found").to_owned(),
            (n, all) if all > n => olo_kit::tr!("{} of {} Events Found", style::thousands(n), style::thousands(all)),
            (n, _) => olo_kit::words::count(n as u64, "1 Event Found", "{n} Events Found"),
        }];
    }
    let (first, after) = v.visible();
    let last = layout::day_of(layout::day_number(after) - 1);
    match v.settings.mode {
        Mode::Month => words::month_forms(v.cursor),
        Mode::Day => words::day_forms(v.cursor),
        Mode::Week => words::span_forms(first, last),
        Mode::Agenda => words::day_forms(v.cursor).into_iter().map(|d| olo_kit::tr!("From {}", d)).collect(),
    }
}

/// The arrows, Today, what shows, and how things stand with the servers.
/// The toolbar, and whether it took two rows (its title under the rest).
fn nav(v: &mut Calendar, width: f32, folded: bool, cx: &mut Cx<Calendar>) -> (Element, bool) {
    let (back, on) = match v.settings.mode {
        Mode::Day => ("Previous Day", "Next Day"),
        Mode::Week | Mode::Agenda => ("Previous Week", "Next Week"),
        Mode::Month => ("Previous Month", "Next Month"),
    };
    let (status, wrong) = v.status();
    let p = theme::palette();
    // Its room, a toolbar's that folds (CLAUDE.md, "Every tiling
    // layout"): the arrows, Today and the gaps keep theirs; the search
    // field narrows to its least before the title takes a shorter form,
    // and the status line has what's left (none at all, below a few
    // words' room).
    let bold = style::ui_font(olo_kit::FontWeight::BOLD);
    // (A point or two more than its words: a box exactly as wide as them
    // cut the last one short.)
    let measure = |t: &str| f32::from(style::text_width(t.to_owned(), style::LEAD_SIZE, bold.clone())).ceil() + 2.0;
    // Folded (a window a third of a screen high): Day, Week, Month and
    // Agenda in a pop-up after Today, as wide as the widest of them.
    let modes: Vec<olo_kit::SharedString> = Mode::ALL.iter().map(|mode| olo_kit::SharedString::from(mode.name())).collect();
    let regular = style::ui_font(olo_kit::FontWeight::NORMAL);
    let widest = Mode::ALL.iter().map(|mode| f32::from(style::text_width(olo_kit::words::tr(mode.name()).to_owned(), style::PLAIN_SIZE, regular.clone()))).fold(0.0, f32::max);
    let mode_w = (m::TEXT_INSET + widest + 30.0).ceil();
    let titles = titles(v);
    let longest = measure(&titles[0]);
    // (Today's width and whether the title has a row of its own are the
    // same for every view, from the longest and the widest shortest of
    // all their titles: from the shown view's, a third of a screen had
    // Day's title beside the arrows and Week's under them, the tabs moved
    // under the pointer as one was chosen; Round 2's pictures.)
    let (longest_any, shortest_any) = if v.finding.is_empty() { every_views_titles(v, &measure) } else { (longest, measure(&titles[titles.len() - 1])) };
    // (Where the search field would be narrower than its own width,
    // Today as wide as its word and its insets: the rest of a button's
    // least went to nothing while the field's words ran to its end;
    // Opus's and Neon's reviews, 2026-11-28.)
    let today_word = f32::from(style::text_width(olo_kit::words::tr("Today").to_owned(), style::PLAIN_SIZE, regular.clone()));
    let fixed_with = |today: f32| 2.0 * m::CONTROL_HEIGHT + today + 4.0 * m::SPACING + if folded { mode_w + m::SPACING } else { 0.0 };
    let today_w = today_width(button_width(olo_kit::words::tr("Today"), false), today_word, width - fixed_with(button_width(olo_kit::words::tr("Today"), false)) - longest_any);
    let fixed = fixed_with(today_w);
    // Too narrow for even the shortest title beside the search field at
    // its least (a third of a screen beside Deskbar, 349 points: the week
    // was "No…"), the title has a row of its own under the rest, the
    // search field the room the title had beside Today.
    let below = title_below(width - fixed, shortest_any);
    let search = if below { 0.0 } else { (width - fixed - longest).clamp(SEARCH_LEAST, SEARCH) };
    let room = if below { width } else { (width - fixed - search).max(0.0) };
    let title = titles.iter().find(|t| measure(t) <= room).unwrap_or(&titles[titles.len() - 1]).clone();
    let title_w = measure(&title).min(room);
    // (Its room less the gap before it where the title has a row of its
    // own, the search field's place empty.)
    let status_room = room - title_w - if below { m::SPACING } else { 0.0 };
    // (The title and the status on the line of Today's own word. When
    // things were read, whole or not at all: "Updated ju…" said nothing;
    // Neon's review, 2026-11-28. What's wrong, cut where it must be.)
    let status_w = f32::from(style::text_width(status.clone(), style::SMALL_SIZE, regular.clone())).ceil();
    let shows = status_shows(wrong, status_w, status_room);
    let mut said = StringView::new(status).small(true).cut(true).beside(style::PLAIN_SIZE);
    said = if wrong { said.color(style::c(p.failure)) } else { said.dim(true) };
    let row = || div().w_full().flex().flex_row().items_center().gap(px(m::SPACING));
    let controls = row()
        .child(
            div()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .child(ArrowButton::back(back).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.step(-1, cx))))
                .child(ArrowButton::on(on).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.step(1, cx)))),
        )
        .child(Button::new("Today").id("today").min_width(today_w).enabled(v.cursor != v.today).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| {
            let today = v.today;
            v.go(today, cx);
        })))
        .children(folded.then(|| {
            let at = Mode::ALL.iter().position(|mode| *mode == v.settings.mode).unwrap_or(2);
            // (Nothing to view with no calendars yet.)
            let any = v.loading || !v.sources.is_empty();
            MenuField::new("calendar-mode", modes, at).name("View").width(mode_w).enabled(any).on_change(cx.msg_with(|i: &usize| Msg::Mode(Mode::ALL[(*i).min(3)]))).into_element()
        }));
    let title = div().w(px(title_w)).flex_shrink_0().child(StringView::new(title).size(style::LEAD_SIZE).bold(true).cut(true).height(m::CONTROL_HEIGHT).beside(style::PLAIN_SIZE));
    let status = div().flex_1().min_w_0().flex().justify_end().children(shows.then_some(said));
    // Find: what's typed here is looked for in every calendar shown.
    if below {
        let controls = controls.child(div().flex_1().min_w_0().child(&v.search));
        return (div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(controls).child(row().child(title).child(status)).into_element(), true);
    }
    (controls.child(title).child(status).child(div().w(px(search)).flex_shrink_0().child(&v.search)).into_element(), false)
}

/// The room left under the small month (`month` tall) in a column that
/// scrolls, `shown` tall, where what follows it (the calendars' heading
/// and a row, `next` tall) would be cut at the column's foot: they start
/// just past it then. None where the month itself doesn't fit, or they do.
fn fold_gap(shown: f32, month: f32, next: f32) -> f32 {
    if shown > month && shown < month + next { shown - month } else { 0.0 }
}

/// Every view's titles at the day chosen (Day's, Week's, Month's and
/// Agenda's, as [`titles`] says them), measured: the longest of them all,
/// and the widest of each view's shortest.
fn every_views_titles(v: &Calendar, measure: &dyn Fn(&str) -> f32) -> (f32, f32) {
    let first = layout::week_of(v.cursor, v.week_start());
    let last = layout::day_of(layout::day_number(first) + 6);
    let forms = [words::day_forms(v.cursor), words::span_forms(first, last), words::month_forms(v.cursor), words::day_forms(v.cursor).into_iter().map(|d| olo_kit::tr!("From {}", d)).collect()];
    let longest = forms.iter().filter_map(|f| f.first()).map(|t| measure(t)).fold(0.0, f32::max);
    let shortest = forms.iter().filter_map(|f| f.last()).map(|t| measure(t)).fold(0.0, f32::max);
    (longest, shortest)
}

/// Whether the preview shows under the days (View ▸ Show Preview, and
/// room for it): in a column `main_w` wide, as wide as a quarter of a
/// screen's at least, always (its hint when nothing is chosen); in a
/// narrower one, only while an event is chosen.
fn preview_shows(chosen: bool, main_w: f32) -> bool {
    chosen || main_w >= PREVIEW_HINT_FROM
}

/// Narrower than this, the preview's hint gives its room to the days.
const PREVIEW_HINT_FROM: f32 = 480.0;

/// Whether the toolbar's status shows in the `room` left it, its words
/// `width` wide: what's wrong where a few words of it fit (cut, its whole
/// in a tip); when things were read only whole.
fn status_shows(wrong: bool, width: f32, room: f32) -> bool {
    if wrong { room >= 64.0 } else { width <= room }
}

/// Today's width: a button's own (`full`); or, where the search field
/// would have less than its own width beside it (`left`, the room the
/// title leaves it with Today `full`), as wide as its word (`word`) and
/// a button's insets, on whole points of two.
fn today_width(full: f32, word: f32, left: f32) -> f32 {
    if left >= SEARCH { full } else { (((word + 2.0 * m::CONTROL_PAD) / 2.0).ceil() * 2.0).min(full) }
}

/// The room over and under the small month in a column `shown` tall:
/// a tight spacing; where that leaves no room for all of it, what there
/// is (none, at the least), so its weeks show whole. (A horizontal third
/// of the smallest screen, 177 points, cut its sixth week in half under
/// a scroll bar, with 14 points over its name: Opus's and Neon's
/// reviews, 2026-11-28.)
fn month_pad(shown: f32) -> f32 {
    ((shown - mini::HEIGHT) / 2.0).floor().clamp(0.0, m::TIGHT)
}

/// Whether the small month shows in a column `shown` tall: only whole.
fn month_shows(shown: f32) -> bool {
    shown >= mini::HEIGHT
}

/// Whether the title goes under the toolbar's row: there's `room`
/// beside the arrows, Today and their gaps, and the shortest title is
/// `shortest` wide (it and the search field at its least don't fit).
fn title_below(room: f32, shortest: f32) -> bool {
    room - SEARCH_LEAST < shortest
}

/// The assistant: the conversation (each answer with the events it
/// points to, a click going to each, and what it proposes, a click
/// opening it to save or not), suggestions before any, and the field to
/// ask in. `column` is how wide it is; `alone`, in front of the days in
/// a window too narrow for both, with a way back to them.
fn assistant_pane(v: &mut Calendar, column: f32, alone: bool, cx: &mut Cx<Calendar>) -> Element {
    let p = theme::palette();
    let e = edge(cx);
    let failure = style::c(p.failure);
    // (What's wider than the column wraps: its width less its well's
    // edges, its padding and the scroll bar's room while there is one.)
    let bar = if v.chat_scroll.max_offset().height > px(0.5) { olo_kit::widgets::scroll::SCROLL_BAR } else { 0.0 };
    let wrap = column - 2.0 * e - 2.0 * m::SPACING - bar;
    let mut conv = div().w_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING));
    for (i, t) in v.turns.iter().enumerate() {
        conv = conv.child(StringView::new(t.question.clone()).bold(true).wrap(true));
        // (Stopped before a word came: said so, quietly.)
        let answer = if t.answer.is_empty() { StringView::new("Stopped.").dim(true) } else { StringView::new(t.answer.clone()).wrap(true) };
        conv = conv.child(if t.failed { answer.color(failure) } else { answer });
        if !t.events.is_empty() || !t.proposals.is_empty() {
            let mut list = div().flex().flex_col().gap(px(m::TIGHT)).mt(px(-(m::SPACING - m::TIGHT)));
            // The events it points to: each goes to it in the view.
            for (j, event) in t.events.iter().enumerate() {
                let (href, start) = (event.href.clone(), event.start);
                list = list.child(
                    div().flex().child(LinkButton::new(event.said.clone()).wrap_at(wrap).id(format!("event-{i}-{j}")).on_click(cx.msg_with(move |_: &ClickEvent| Msg::Reveal(href.clone(), start)))),
                );
            }
            // What it proposes: each opens in the event's window, to save or not.
            for (j, proposal) in t.proposals.iter().enumerate() {
                let said = format!("Add {}…", crate::assistant::proposal_said(proposal, v.h24));
                list = list.child(div().flex().child(LinkButton::new(said).wrap_at(wrap).id(format!("proposal-{i}-{j}")).on_click(cx.msg(Msg::Propose(i, j)))));
            }
            conv = conv.child(list);
        }
    }
    if let Some(a) = &v.answering {
        conv = conv.child(StringView::new(a.question.clone()).bold(true).wrap(true));
        let words = match (a.so_far.is_empty(), a.model.is_empty()) {
            (true, true) => olo_kit::words::tr("Thinking…").to_owned(),
            (true, false) => olo_kit::tr!("Thinking ({})…", a.model),
            (false, _) => a.so_far.clone(),
        };
        conv = conv.child(StringView::new(words).wrap(true).dim(a.so_far.is_empty()));
    }
    if v.turns.is_empty() && v.answering.is_none() {
        conv = conv.child(
            StringView::new(
                "Ask about your calendars here: the assistant is given the events of the ones you show, a month back and four months on. It can draft events; nothing is added until you save it.",
            )
            .dim(true)
            .wrap(true),
        );
        let hour = crate::now(&v.zone).hour();
        for (i, s) in crate::assistant::suggestions(hour).into_iter().enumerate() {
            let q = s.clone();
            conv =
                conv.child(div().flex().child(LinkButton::new(s).wrap_at(wrap).id(format!("suggestion-{i}")).on_click(cx.listener(move |v: &mut Calendar, _: &ClickEvent, cx| v.ask(q.clone(), cx)))));
        }
    }
    let scroller = ScrollView::new(&v.chat_scroll).border(false).auto_hide(true).background(p.document_background).child(conv);
    let answering = v.answering.is_some();
    let button = if answering {
        Button::new("Stop").id("stop-asking").on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| {
            v.stop_asking();
            cx.invalidate();
        }))
    } else {
        Button::new("Ask").id("ask").on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.ask_typed(cx)))
    };
    let title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .h(px(m::CONTROL_HEIGHT))
        .children(alone.then(|| back(cx, true)))
        .child(StringView::new("Assistant").bold(true))
        .child(div().flex_1())
        .child(
            LinkButton::new("New Conversation").id("new-conversation").enabled(!v.turns.is_empty() || answering).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.new_conversation(cx))),
        );
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::TIGHT))
        .child(title)
        .child(div().flex_1().min_h_0().child(well(e, style::c(p.document_background), scroller)))
        .child(div().flex().flex_row().gap(px(m::TIGHT)).items_center().child(div().flex_1().min_w_0().child(TextControl::new(cx, &v.question))).child(button))
        .into_element()
}

/// Day, Week, Month and Agenda, as tabs.
fn views(v: &mut Calendar, folded: bool, room: f32, cx: &mut Cx<Calendar>) -> Element {
    let tabs: Rc<[olo_kit::SharedString]> = Rc::from(Mode::ALL.map(|mode| olo_kit::SharedString::from(mode.name())).to_vec());
    let at = Mode::ALL.iter().position(|mode| *mode == v.settings.mode).unwrap_or(2);
    let p = theme::palette();
    let e = edge(cx);
    let content = if !v.loading && v.sources.is_empty() {
        // (Folded, a window a third of a screen high or at its least: no
        // icon, so its button fits; with too little room for its detail
        // too, its line and its button, clear of the well's foot.)
        let mut empty = EmptyState::new("No calendars yet").icon("calendar").action("Add an Account…", |_: &ClickEvent, _| olo_kit::app_menu::open_settings("integrations")).compact(folded);
        if room >= EMPTY_DETAIL_ROOM {
            // (Not "in Preferences": its button, under it, says where.)
            empty = empty.detail("Connect a Google or Fastmail account (or any CalDAV server), and its calendars show here, all together.");
        }
        empty.into_element()
    } else if !v.finding.is_empty() {
        // Find: what was found, in the views' place while it's looked for.
        agenda::found(v, cx)
    } else {
        match v.settings.mode {
            // (Each week a number's row and an event's at least: in a
            // window too short for that, the weeks scroll.)
            Mode::Month => ScrollView::new(&v.month_scroll)
                .border(false)
                .auto_hide(true)
                .background(p.document_background)
                .child(div().w_full().h_full().min_h(px(month::least(v))).child(month::view(cx)))
                .into_element(),
            Mode::Day | Mode::Week => week::view(v, cx),
            Mode::Agenda => agenda::view(v, cx),
        }
    };
    // (Folded, the tabs are a pop-up in the toolbar: the days have their row.)
    if folded {
        return div().size_full().child(well(e, style::c(p.document_background), content)).into_element();
    }
    TabView::new("calendar-mode", tabs, at)
        .on_select(cx.msg_with(|i: &usize| Msg::Mode(Mode::ALL[(*i).min(3)])))
        .child(div().size_full().child(well(e, style::c(p.document_background), content)))
        .into_element()
}

/// The small month, and the calendars under it; `alone`, in front of the
/// days in a window too narrow for both, with a way back to them.
fn sidebar_column(v: &mut Calendar, alone: bool, cx: &mut Cx<Calendar>) -> Element {
    let p = theme::palette();
    let ground = olo_kit::widgets::sidebar::ground(&p);
    // The days with something on them, in the month it shows.
    let mut marked: Vec<mini::Day> = Vec::new();
    for s in v.shown.iter() {
        let (first, last) = s.days();
        for n in layout::day_number(first)..=layout::day_number(last) {
            marked.push(words::day_of(layout::day_of(n)));
        }
    }
    marked.sort_unstable();
    marked.dedup();
    let (first, after) = v.visible();
    let last = layout::day_of(layout::day_number(after) - 1);
    let (from, to) = match v.settings.mode {
        Mode::Week => (first, last),
        _ => (v.cursor, v.cursor),
    };
    let month = MonthGrid::new("Small Month", v.mini.0 as i32, v.mini.1 as u8, words::day_of(v.today))
        .week_start(v.week_start())
        .chosen(words::day_of(from), words::day_of(to))
        .marked(marked)
        .on_pick(cx.msg_with(|d: &mini::Day| Msg::Go(words::date_of(*d))))
        .on_turn(cx.msg_with(|(year, month): &(i32, u8)| Msg::Mini(*year as i16, *month as i8)));
    // The calendars, under each account's address.
    let mut list = div().w_full().flex().flex_col().px(px(m::TIGHT + 2.0)).pb(px(m::TIGHT));
    let mut account = String::new();
    for (i, cal) in v.book.cals.iter().enumerate() {
        if cal.kept.account != account {
            account = cal.kept.account.clone();
            // An account's address over its calendars (with who keeps it,
            // where two accounts have the same address); one account's
            // says nothing the window doesn't.
            let source = v.sources.iter().find(|s| s.account.id() == account);
            let name = match source {
                Some(s) if v.sources.iter().filter(|o| o.account.address == s.account.address).count() > 1 => format!("{} ({})", s.account.address, s.account.provider.name()),
                Some(s) => s.account.address.clone(),
                None => account.clone(),
            };
            if v.sources.len() > 1 {
                list = list.child(div().pt(px(m::TIGHT)).pb(px(2.0)).child(StringView::new(name).small(true).dim(true).cut(true).height(m::LABEL_ROW)));
            }
        }
        let shown = !v.settings.choices.hidden.contains(&cal.kept.href);
        let fill = style::c(p.hue(hue_of(v, i)));
        let swatch = canvas(move |g, b| {
            let r = style::snap_centred(g, b.center(), 10.0, 10.0);
            style::fill_rect(g, r, fill);
        })
        .paint_key(style::key(style::ink_key(Some(fill))))
        .w(px(10.0))
        .h(px(m::MARK_ROW))
        .flex_shrink_0();
        list = list.child(
            div()
                .id(("calendar", i))
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::MARK_GAP))
                .child(swatch)
                .child(div().flex_1().min_w_0().child(CheckBox::new(cal.kept.name.clone(), shown).id(format!("calendar-{i}")).on_toggle(cx.msg_with(move |on: &bool| Msg::Show(i, *on))))),
        );
    }
    let heading = div().px(px(m::TIGHT + 2.0)).pt(px(m::TIGHT)).child(StringView::new("Calendars").bold(true).height(m::LABEL_ROW));
    let e = edge(cx);
    // The calendars scroll under the small month; in a window too short
    // for the month and a few of them (a third of a screen's height), the
    // whole column does, the month with them, not cut off.
    let room = f32::from(cx.viewport_size().height) - olo_kit::menu::BAR_HEIGHT - m::SPACING - m::WINDOW_INSET - if alone { m::CONTROL_HEIGHT + m::TIGHT } else { 0.0 };
    let pad = month_pad(room - 2.0 * e);
    let month = div().w_full().flex().justify_center().pt(px(pad)).pb(px(pad)).child(month);
    let short = room < mini::HEIGHT + 2.0 * m::TIGHT + m::LABEL_ROW + m::TIGHT + 3.0 * m::MARK_ROW + 2.0 * e;
    let content = if !month_shows(room - 2.0 * e) {
        // (Not even its weeks alone fit, a horizontal third of the
        // smallest screen beside Deskbar having 171 points for its 174:
        // the small month gives way, the calendars in its place. Cut in
        // half under a scroll bar, it looked broken (Opus's and Neon's
        // reviews, 2026-11-28); the toolbar's arrows and Today still go
        // anywhere. A month the kit draws tighter where it's short:
        // docs/TODO.md.)
        div().size_full().flex().flex_col().child(heading).child(div().flex_1().min_h_0().child(ScrollView::new(&v.calendars_scroll).border(false).auto_hide(true).child(list)))
    } else if short {
        // (What shows of it before it's scrolled: the small month whole,
        // and the calendars' heading under it whole or not at all, just
        // past the column's foot: a horizontal third of 1440 × 900 cut it
        // in half there, QA 2026-11-28.)
        let gap = fold_gap(room - 2.0 * e, mini::HEIGHT + 2.0 * pad, m::TIGHT + m::LABEL_ROW + m::MARK_ROW);
        div()
            .size_full()
            .child(ScrollView::new(&v.calendars_scroll).border(false).auto_hide(true).child(div().w_full().flex().flex_col().child(month).child(div().h(px(gap))).child(heading).child(list)))
    } else {
        div().size_full().flex().flex_col().child(month).child(heading).child(div().flex_1().min_h_0().child(ScrollView::new(&v.calendars_scroll).border(false).auto_hide(true).child(list)))
    };
    if !alone {
        return well(e, ground, content);
    }
    // Alone in front of the days: the column as it is beside them,
    // centred, the small month and the calendars on one inset (the month
    // centred across the window, the calendars at its left, looked like
    // two things; Codex, 2026-11-19).
    let column = div().size_full().flex().justify_center().child(div().w(px(SIDEBAR)).h_full().child(content));
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::TIGHT))
        .child(div().flex().flex_row().items_center().h(px(m::CONTROL_HEIGHT)).child(back(cx, false)))
        .child(div().flex_1().min_h_0().child(well(e, ground, column)))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toolbar in a third of a screen: when things were read only
    /// whole, what's wrong cut; Today as wide as its word where the
    /// search field would be squeezed; the preview's hint only where
    /// there's room for it.
    #[test]
    fn a_narrow_toolbar_keeps_what_it_says_whole() {
        assert!(status_shows(false, 85.0, 90.0));
        assert!(!status_shows(false, 85.0, 70.0), "\"Updated ju…\" never");
        assert!(status_shows(true, 300.0, 70.0), "what's wrong, cut");
        assert!(!status_shows(true, 300.0, 40.0));
        assert_eq!(today_width(80.0, 35.0, 250.0), 80.0);
        assert_eq!(today_width(80.0, 35.0, 120.0), 60.0, "its word and insets, on whole points of two");
        assert_eq!(today_width(80.0, 60.0, 120.0), 80.0, "never wider than its own");
        assert!(!preview_shows(false, 317.0) && preview_shows(true, 317.0));
        assert!(preview_shows(false, 600.0));
    }

    /// The small month whole in a horizontal third of the smallest
    /// screen (177 points), its spacing where there's room for it.
    #[test]
    fn the_small_month_shows_whole() {
        assert_eq!(month_pad(177.0), 1.0);
        assert!(mini::HEIGHT + 2.0 * month_pad(177.0) <= 177.0);
        assert_eq!(month_pad(400.0), m::TIGHT);
        assert_eq!(month_pad(100.0), 0.0);
        // Shorter than its weeks (a horizontal third beside Deskbar, 171
        // points): not shown at all, never cut.
        assert!(!month_shows(171.0) && month_shows(mini::HEIGHT));
    }

    /// The toolbar's title goes under it where even its shortest form
    /// and the search field at its least don't fit beside the rest.
    #[test]
    fn the_title_goes_under_where_it_doesnt_fit() {
        // (A third beside Deskbar: 317 inside the window's insets, 176
        // of it the arrows, Today and the gaps; "Nov 15 – 21" ~85.)
        assert!(title_below(317.0 - 176.0, 85.0));
        assert!(!title_below(498.0 - 176.0, 85.0));
    }

    /// In a column that scrolls, what follows the small month is whole
    /// at its foot or starts past it, never cut in half there.
    #[test]
    fn the_heading_under_the_month_is_never_cut_in_half() {
        let (month, next) = (186.0, 46.0);
        assert_eq!(fold_gap(204.0, month, next), 18.0, "it would be cut: past the foot");
        assert_eq!(fold_gap(240.0, month, next), 0.0, "it fits");
        assert_eq!(fold_gap(172.0, month, next), 0.0, "the month itself doesn't");
    }
}
