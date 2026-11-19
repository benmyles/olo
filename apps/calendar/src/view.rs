//! The window: the small month and the calendars down the left (each
//! with its colour and a check box to show it), and beside them the days
//! themselves, by Day, Week, Month or as an Agenda, under the arrows that
//! step through them and the name of what shows.

use std::rc::Rc;

use olo_kit::{
    ClickEvent, Cx, Element, Hsla, InteractiveElement as _, IntoElement as _, MenuBar, ParentElement as _, Styled as _, canvas, div, metrics as m, px, style, theme,
    widgets::{ArrowButton, Button, CheckBox, EmptyState, LinkButton, MonthGrid, ScrollView, StringView, TabView, TextControl, month as mini},
};

use crate::{Calendar, Mode, Msg, agenda, hue_of, layout, month, week, words};

/// The search field's width.
const SEARCH: f32 = 190.0;
/// The calendars' column: the small month and a little room round it.
const SIDEBAR: f32 = mini::WIDTH + 2.0 * m::TIGHT + 4.0;
/// The least the views need: seven days across, each wide enough for a
/// time and a few words.
const MAIN_MIN: f32 = 7.0 * 78.0;
pub const MIN_WIDTH: f32 = 2.0 * m::WINDOW_INSET + SIDEBAR + m::SPACING + MAIN_MIN;
pub const MIN_HEIGHT: f32 = 520.0;
/// The assistant's column, and the least it's any use at.
const ASSISTANT: f32 = 264.0;
const ASSISTANT_MIN: f32 = 216.0;
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
        _ if chosen => (hue, false),
        Some(Answer::NotYet) => (olo_theme::mix(hue, p.document_background, 0.85), true),
        Some(Answer::No) => (olo_theme::mix(hue, p.document_background, 0.75), false),
        Some(Answer::Maybe) => (olo_theme::mix(hue, p.document_background, 0.45), false),
        _ if s.free || s.tentative => (olo_theme::mix(hue, p.document_background, 0.45), false),
        _ => (hue, false),
    }
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
    let bar = MenuBar::of(v, cx, |v, _| v.menus());
    let p = theme::palette();
    let mut row = div().flex_1().min_h_0().flex().flex_row().gap(px(m::SPACING)).px(px(m::WINDOW_INSET)).pt(px(m::SPACING)).pb(px(m::WINDOW_INSET));
    // The assistant's column takes what the views can spare, up to its
    // width; in a window too narrow for all three, the calendars' column
    // gives way to it while it shows.
    let room = |sidebar: bool| f32::from(cx.viewport_size().width) - 2.0 * m::WINDOW_INSET - if sidebar { SIDEBAR + m::SPACING } else { 0.0 } - MAIN_MIN - m::SPACING;
    let (sidebar_shown, column) = match (v.settings.assistant, v.settings.sidebar) {
        (false, sidebar) => (sidebar, 0.0),
        (true, true) if room(true) >= ASSISTANT_MIN => (true, room(true).min(ASSISTANT)),
        (true, _) => (false, room(false).clamp(ASSISTANT_MIN, ASSISTANT)),
    };
    if sidebar_shown {
        row = row.child(div().w(px(SIDEBAR)).flex_shrink_0().h_full().child(sidebar(v, cx)));
    }
    let main = div().flex_1().min_w(px(MAIN_MIN)).h_full().flex().flex_col().gap(px(m::TIGHT)).child(nav(v, cx)).child(div().flex_1().min_h_0().child(views(v, cx)));
    let pane = v.settings.assistant.then(|| div().w(px(column)).flex_shrink_0().h_full().child(assistant_pane(v, column, cx)));
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
    div().size_full().flex().flex_col().bg(style::c(p.panel_background)).on_window_key_down(keys).capture_key_down(in_field).child(bar).child(row.child(main).children(pane)).into_element()
}

/// What the view shows, in words: the month, the week's days, the day.
pub fn title(v: &Calendar) -> String {
    // (Find: how many were found.)
    if !v.finding.is_empty() {
        return match (v.found.len(), v.found_total) {
            (0, _) => "Nothing Found".to_owned(),
            (1, 1) => "1 Event Found".to_owned(),
            (n, all) if all > n => format!("{} of {} Events Found", style::thousands(n), style::thousands(all)),
            (n, _) => format!("{n} Events Found"),
        };
    }
    let (first, after) = v.visible();
    let last = layout::day_of(layout::day_number(after) - 1);
    match v.settings.mode {
        Mode::Month => words::month_year(v.cursor),
        Mode::Day => words::long_day(v.cursor),
        Mode::Week => words::days_span(first, last),
        Mode::Agenda => format!("From {}", words::long_day(v.cursor)),
    }
}

/// The arrows, Today, what shows, and how things stand with the servers.
fn nav(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
    let (back, on) = match v.settings.mode {
        Mode::Day => ("Previous Day", "Next Day"),
        Mode::Week | Mode::Agenda => ("Previous Week", "Next Week"),
        Mode::Month => ("Previous Month", "Next Month"),
    };
    let (status, wrong) = v.status();
    let p = theme::palette();
    // (The title and the status on the line of Today's own word.)
    let mut said = StringView::new(status).small(true).cut(true).beside(style::PLAIN_SIZE);
    said = if wrong { said.color(style::c(p.failure)) } else { said.dim(true) };
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .child(ArrowButton::back(back).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.step(-1, cx))))
                .child(ArrowButton::on(on).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| v.step(1, cx)))),
        )
        .child(Button::new("Today").id("today").enabled(v.cursor != v.today).on_click(cx.listener(|v: &mut Calendar, _: &ClickEvent, cx| {
            let today = v.today;
            v.go(today, cx);
        })))
        .child(div().flex_shrink_0().child(StringView::new(title(v)).size(style::LEAD_SIZE).bold(true).height(m::CONTROL_HEIGHT).beside(style::PLAIN_SIZE)))
        .child(div().flex_1().min_w_0().flex().justify_end().child(said))
        // Find: what's typed here is looked for in every calendar shown.
        .child(div().w(px(SEARCH)).flex_shrink_0().child(&v.search))
        .into_element()
}

/// The assistant: the conversation (each answer with the events it
/// points to, a click going to each, and what it proposes, a click
/// opening it to save or not), suggestions before any, and the field to
/// ask in. `column` is how wide it is.
fn assistant_pane(v: &mut Calendar, column: f32, cx: &mut Cx<Calendar>) -> Element {
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
        let words = if a.so_far.is_empty() { format!("Thinking{}…", if a.model.is_empty() { String::new() } else { format!(" ({})", a.model) }) } else { a.so_far.clone() };
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
    let title = div().flex().flex_row().items_center().h(px(m::CONTROL_HEIGHT)).child(StringView::new("Assistant").bold(true)).child(div().flex_1()).child(
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
fn views(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
    let tabs: Rc<[olo_kit::SharedString]> = Rc::from(Mode::ALL.map(|mode| olo_kit::SharedString::from(mode.name())).to_vec());
    let at = Mode::ALL.iter().position(|mode| *mode == v.settings.mode).unwrap_or(2);
    let p = theme::palette();
    let e = edge(cx);
    let content = if !v.loading && v.sources.is_empty() {
        EmptyState::new("No calendars yet")
            .icon("calendar")
            .detail("Connect a Google or Fastmail account (or any CalDAV server) in Preferences, and its calendars show here, all together.")
            .action("Add an Account…", |_: &ClickEvent, _| olo_kit::app_menu::open_settings("integrations"))
            .into_element()
    } else if !v.finding.is_empty() {
        // Find: what was found, in the views' place while it's looked for.
        agenda::found(v, cx)
    } else {
        match v.settings.mode {
            Mode::Month => month::view(cx),
            Mode::Day | Mode::Week => week::view(v, cx),
            Mode::Agenda => agenda::view(v, cx),
        }
    };
    TabView::new("calendar-mode", tabs, at)
        .on_select(cx.msg_with(|i: &usize| Msg::Mode(Mode::ALL[(*i).min(3)])))
        .child(div().size_full().child(well(e, style::c(p.document_background), content)))
        .into_element()
}

/// The small month, and the calendars under it.
fn sidebar(v: &mut Calendar, cx: &mut Cx<Calendar>) -> Element {
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
    let content = div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().w_full().flex().justify_center().pt(px(m::TIGHT)).pb(px(m::TIGHT)).child(month))
        .child(heading)
        .child(div().flex_1().min_h_0().child(ScrollView::new(&v.calendars_scroll).border(false).auto_hide(true).child(list)));
    well(e, ground, content)
}
