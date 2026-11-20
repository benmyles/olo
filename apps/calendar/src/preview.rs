//! The preview pane (Ben, 2026-11-18: "there should also be a preview
//! pane where if i single click an event i see the details in the bottom
//! 1/3 of the main area"): the event chosen with one click, as the event
//! window would show it but to read, under the views, a third of the
//! main area's height (View ▸ Show Preview). Its title, its calendar's
//! colour before it and Edit… after; its facts on the left (when, on
//! another clock said, where, a link where that's an address, its
//! calendar, how it repeats, its reminder, the person's answer, its
//! guests with theirs), scrolled if they're more than there's room for;
//! its notes on the right in a text view (scrolled, selected, copied).
//! Nothing chosen, it says so, keeping its place: the views above don't
//! jump as a click chooses an event or lets it go.
//!
//! A view of its own, told what to show by Calendar ([`follow`]) only
//! when the event chosen or what it's made from changes: Calendar's
//! body, which runs on every change of its own, doesn't make the pane
//! again, and its facts' bar coming or going (their scroll view's, as
//! they overflow or don't) makes the pane again alone, not Calendar's
//! views.

use std::sync::{Arc, Weak};

use jiff::civil::DateTime;
use olo_kit::{
    Child, ClickEvent, Cx, Element, IntoElement as _, Messenger, ParentElement as _, ScrollHandle, Styled as _, View, canvas, div, metrics as m, point, px, relative, style, theme,
    widgets::{Button, LinkButton, ScrollView, StringView, TextView, text_view},
};
use olo_theme::Hue;

use crate::{
    Calendar, Key, Msg, edit, editor, hue_of,
    model::{Book, Shown},
    words,
};

/// The labels' column, as the event window has it (at the least).
const LABELS: f32 = 84.0;

/// The facts' labels, the widest of which (in the language shown) sets
/// how wide their column is, whichever are shown: the values one column
/// from event to event.
const FACT_LABELS: [&str; 7] = ["When:", "Where:", "Calendar:", "Repeats:", "Reminder:", "Going:", "Guests:"];

/// The pane's height at the least: the title and five facts.
pub const LEAST: f32 = 150.0;

/// The event the pane shows, as Calendar opened it.
pub struct Showing {
    key: Key,
    opening: editor::Opening,
    /// Its times on the person's clock, as the views show it (the
    /// opening's are on its own, to change it there).
    mine: (DateTime, DateTime),
    /// Its calendar's colour.
    hue: Hue,
}

/// What Calendar last told the pane: the event chosen, and what it was
/// opened from (the calendars, the occurrences shown, the clock's style,
/// its calendar's colour), each compared by what it is, not by what it
/// holds.
pub type Told = Option<(Key, Weak<Book>, Weak<Vec<Shown>>, bool, Hue)>;

/// The pane: its notes' view, its facts' scroller, the event it shows,
/// and a way to Calendar (Edit… opens the event there).
pub struct Pane {
    notes: Child<TextView>,
    scroll: ScrollHandle,
    shown: Option<Showing>,
    calendar: Messenger<Msg>,
}

impl Pane {
    pub fn new(cx: &mut Cx<Calendar>) -> Child<Pane> {
        let calendar = cx.messenger();
        cx.new_view(move |cx| Pane { notes: text_view(cx, |t| t.empty_text("No notes.").auto_hide(true).links(true)), scroll: ScrollHandle::new(), shown: None, calendar })
    }

    fn show(&mut self, shown: Option<Showing>, cx: &mut Cx<Pane>) {
        // (Another event's facts start at their top.)
        if self.shown.as_ref().map(|s| &s.key) != shown.as_ref().map(|s| &s.key) {
            self.scroll.set_offset(cx, point(px(0.0), px(0.0)));
        }
        let notes = shown.as_ref().map_or(String::new(), |s| s.opening.draft.notes.trim().to_owned());
        if self.notes.read(cx).text() != notes {
            self.notes.update(cx, |t, cx| t.set_text(notes, None, cx));
        }
        self.shown = shown;
        cx.invalidate();
    }
}

/// Tells the pane what's chosen, when that or what it's made from has
/// changed since it was last told (on each of Calendar's bodies, a look
/// at a few pointers; an event opened only then: opening one looks
/// through every occurrence shown).
pub fn follow(v: &mut Calendar, cx: &mut Cx<Calendar>) {
    let told: Told = v.selected.clone().map(|key| {
        let hue = hue_of(v, key.0);
        (key, Arc::downgrade(&v.book), Arc::downgrade(&v.shown), v.h24, hue)
    });
    let same = match (&v.preview_told, &told) {
        (Some(was), Some(now)) => was.0 == now.0 && was.1.ptr_eq(&now.1) && was.2.ptr_eq(&now.2) && was.3 == now.3 && was.4 == now.4,
        (was, now) => was.is_none() && now.is_none(),
    };
    if same {
        return;
    }
    let showing = told.as_ref().and_then(|(key, .., hue)| {
        let opening = v.opening_of(key)?;
        let mine = v.shown.iter().find(|s| s.key() == *key).map(|s| (s.start, s.end))?;
        Some(Showing { key: key.clone(), opening, mine, hue: *hue })
    });
    v.preview_told = told;
    v.preview.update(cx, |pane, cx| pane.show(showing, cx));
}

/// When an event is, its day said in full, as the window's title says
/// the day shown: "Wednesday, November 18, 2026, 2 PM – 3:30 PM",
/// "Thursday, November 19, 2026, all day", "November 21 – 22, 2026". An
/// all-day `end` is its last day, as a draft has it (the event window's).
fn when(start: DateTime, end: DateTime, all_day: bool, h24: bool) -> String {
    let last = end.date();
    let one_day = last == start.date() || (!all_day && end.time() == jiff::civil::Time::midnight() && end.date().yesterday().ok() == Some(start.date()));
    match (all_day, one_day) {
        (true, true) => olo_kit::tr!("{}, all day", words::long_day(start.date())),
        (true, false) => words::days_span(start.date(), last),
        (false, true) => format!("{}, {}", words::long_day(start.date()), words::when(start, end, false, h24)),
        (false, false) => words::when(start, end, false, h24),
    }
}

impl View for Pane {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let pane = div().size_full().flex().flex_col().gap(px(m::TIGHT));
        let Some(shown) = &self.shown else {
            return pane.items_center().justify_center().child(StringView::new("Click an event to see it here; double-click it to change it.").dim(true)).into_element();
        };
        let (o, d) = (&shown.opening, &shown.opening.draft);
        let p = theme::palette();
        // Its title, its calendar's colour before it.
        let fill = style::c(p.hue(shown.hue));
        let swatch = canvas(move |g, b| {
            let r = style::snap_centred(g, b.center(), 10.0, 10.0);
            style::fill_rect(g, r, fill);
        })
        .paint_key(style::key(style::ink_key(Some(fill))))
        .w(px(10.0))
        .h(px(m::CONTROL_HEIGHT))
        .flex_shrink_0();
        let title = if d.title.trim().is_empty() { olo_kit::words::tr("(No title)").to_owned() } else { d.title.trim().to_owned() };
        let (calendar, key) = (self.calendar.clone(), shown.key.clone());
        let edit = Button::new("Edit…").id("preview-edit").on_click(move |_: &ClickEvent, _| {
            calendar.send(Msg::Open(key.clone()));
        });
        let head = div().w_full().flex().flex_row().items_center().gap(px(m::SPACING)).child(swatch).child(div().flex_1().min_w_0().child(StringView::new(title).bold(true).cut(true))).child(edit);
        // Its facts: label, then what it is, set as the notes beside them
        // are (in their face, on their lines: the first lines of both boxes
        // on one line).
        let (across, down) = TextView::MARGINS;
        let line = px(TextView::line_height());
        let (ink, quiet) = (style::c(p.document_text), style::secondary_text(&p));
        // (At the least the event window's, wider for a longer label, as a
        // labelled field's is: "Wiederholung:" ran into its value.)
        let font = style::ui_font(olo_kit::FontWeight::NORMAL);
        let labels = FACT_LABELS.iter().map(|l| f32::from(style::text_width(olo_kit::words::of(*l), style::PLAIN_SIZE, font.clone())).ceil() + m::LABEL_GAP).fold(LABELS, f32::max);
        let fact = |label: &'static str, value: Element| {
            let label = div().w(px(labels)).flex_shrink_0().line_height(line).text_color(quiet).child(olo_kit::words::of(label));
            div().w_full().flex().flex_row().child(label).child(div().flex_1().min_w_0().child(value)).into_element()
        };
        let text = |s: String| div().w_full().line_height(line).text_color(ink).child(s).into_element();
        let mut facts: Vec<Element> = Vec::new();
        // When, on the person's clock as the views above show it; on
        // another clock, its own times under it.
        let (start, end) = if d.all_day { (d.start, d.end) } else { shown.mine };
        facts.push(fact("When:", text(when(start, end, d.all_day, o.h24))));
        if let Some(clock) = &o.clock {
            let theirs = olo_kit::tr!("{} in {}.", words::when(d.start, d.end, false, o.h24), clock);
            facts.push(fact("", StringView::new(theirs).small(true).dim(true).wrap(true).into_element()));
        }
        let location = d.location.trim().to_owned();
        if !location.is_empty() {
            // (An address is a link: a meeting's, to join it.)
            let value = if location.starts_with("https://") || location.starts_with("http://") {
                let to = location.clone();
                // (In the facts' size, on their line, cut to the column: a
                // meeting's address is often longer.)
                LinkButton::new(location)
                    .size(style::PLAIN_SIZE)
                    .line(TextView::line_height())
                    .cut()
                    .id("preview-where")
                    .on_click(move |_: &ClickEvent, _| {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(to.clone()));
                    })
                    .into_element()
            } else {
                text(location)
            };
            facts.push(fact("Where:", value));
        }
        if let Some((_, name)) = o.calendars.get(o.calendar) {
            facts.push(fact("Calendar:", text(name.clone())));
        }
        if d.repeat != edit::Repeat::Never {
            let repeats = match d.until {
                Some(until) => olo_kit::tr!("{}, until {}", olo_kit::words::of_text(&d.repeat.said()), words::long_day(until)),
                None => olo_kit::words::of_text(&d.repeat.said()).to_string(),
            };
            facts.push(fact("Repeats:", text(repeats)));
        }
        // (Values said as in a sentence, not as the menus name them.)
        if let Some(said) = d.reminder.and_then(|r| edit::REMINDERS.iter().position(|(at, _)| *at == Some(r))).map(|i| edit::REMINDERS_SAID[i]) {
            facts.push(fact("Reminder:", text(olo_kit::words::tr(said).to_owned())));
        }
        if let Some(answer) = o.going {
            let said = match answer {
                olo_calendar::event::Answer::Yes => "Yes",
                olo_calendar::event::Answer::No => "No",
                olo_calendar::event::Answer::Maybe => "Maybe",
                olo_calendar::event::Answer::NotYet => "Not answered",
            };
            facts.push(fact("Going:", text(olo_kit::words::tr(said).to_owned())));
        }
        if !o.guests.is_empty() {
            // Their marks hang to the left, so their names start where every
            // other fact's value does, and what they are ends where the
            // values do (the rows' own inset is for the event window's well;
            // the box here has its own). Their rows are a little taller than
            // a line, the first one's middle where a line's is; the label is
            // drawn in a row as theirs are (set as a line, it sat a pixel
            // below the first name at 1×).
            let lifted = (m::LIST_ROW - TextView::line_height()) / 2.0;
            let label = div().w(px(labels)).flex_shrink_0().child(StringView::new("Guests:").dim(true).height(m::LIST_ROW));
            let rows = div().flex_1().min_w_0().ml(px(-editor::GUEST_MARK)).mr(px(-m::TIGHT)).child(editor::guest_rows(o.guests.clone()));
            facts.push(div().w_full().flex().flex_row().mt(px(-lifted)).child(label).child(rows).into_element());
        }
        // In a frame of their own, as the notes beside them are: more than
        // there's room for, they scroll inside it, its bar only then (without
        // one, the bar stood alone in the middle of the pane). Inset as the
        // notes' text is.
        let facts =
            ScrollView::new(&self.scroll).auto_hide(true).background(p.document_background).child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).px(px(across)).py(px(down)).children(facts));
        let body = div()
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(div().w(relative(0.5)).flex_shrink_0().h_full().child(facts))
            .child(div().flex_1().min_w_0().h_full().child(&self.notes));
        pane.child(head).child(body).into_element()
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    #[test]
    fn when_said() {
        let (d, e) = (date(2026, 11, 18), date(2026, 11, 19));
        assert_eq!(super::when(d.at(14, 0, 0, 0), d.at(15, 30, 0, 0), false, false), "Wednesday, November 18, 2026, 2 PM – 3:30 PM");
        assert_eq!(super::when(d.at(19, 0, 0, 0), e.at(0, 0, 0, 0), false, true), "Wednesday, November 18, 2026, 19:00 – 00:00");
        assert_eq!(super::when(d.at(22, 0, 0, 0), e.at(6, 0, 0, 0), false, false), "Wed Nov 18 10 PM – Thu Nov 19 6 AM");
        // (All day: the end its last day, as a draft has it.)
        assert_eq!(super::when(e.at(0, 0, 0, 0), e.at(0, 0, 0, 0), true, false), "Thursday, November 19, 2026, all day");
        assert_eq!(super::when(date(2026, 11, 21).at(0, 0, 0, 0), date(2026, 11, 22).at(0, 0, 0, 0), true, false), "November 21 – 22, 2026");
    }

    #[test]
    fn reminders_said_as_named() {
        // (A sentence's words are the menu's, in a sentence's case.)
        for ((_, named), said) in crate::edit::REMINDERS.iter().zip(crate::edit::REMINDERS_SAID) {
            assert_eq!(named.to_lowercase(), said.to_lowercase());
        }
        for repeat in crate::edit::Repeat::OFFERED {
            assert_eq!(repeat.name().to_lowercase(), repeat.said().to_lowercase());
        }
    }
}
