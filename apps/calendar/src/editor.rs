//! The event's window: a new event, or one being changed. Its title and
//! where, its calendar, all day or from when to when, how it repeats and
//! until when, a reminder, notes. Days and times are typed as people
//! write them ("Nov 20", "fri", "2:30pm": `edit::date`, `edit::time`);
//! moving the start moves the end with it. Save hands the main window a
//! draft, and it does the rest (which occurrences, the server).

use jiff::civil::{Date, DateTime};
use olo_calendar::event::When;
use olo_kit::{
    Child, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, Messenger, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div,
    metrics as m, px, style, theme,
    widgets::{Button, CheckBox, MenuField, StringView, TextControl, TextField, TextFieldEvent, button_row, button_width, text_field},
};

use crate::{
    Msg,
    edit::{self, Draft, REMINDERS, Repeat},
    words,
};

/// Where the fields begin, past their labels.
const DIVIDER: f32 = 84.0;
const DAY_FIELD: f32 = 150.0;
const TIME_FIELD: f32 = 92.0;
const WIDTH: f32 = 460.0;
const NOTES: f32 = 84.0;

/// Which event a draft is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A new one.
    New,
    /// One there: its calendar (its place in the book), its address,
    /// which occurrence (of a repeating one), and that occurrence's start
    /// on the event's own clock before anything was changed.
    Existing { cal: usize, href: String, id: Option<When>, from: DateTime, repeats: bool },
}

/// What Save hands over.
#[derive(Debug, Clone)]
pub struct Saving {
    pub target: Target,
    pub draft: Draft,
    /// The calendar chosen (its place in the book): a new event's.
    pub cal: usize,
}

/// What the window is opened with.
pub struct Opening {
    pub target: Target,
    pub draft: Draft,
    /// The calendars it can be put in: place in the book, name. And which
    /// is chosen (among these).
    pub calendars: Vec<(usize, String)>,
    pub calendar: usize,
    pub today: Date,
    pub h24: bool,
    pub month_first: bool,
    /// Its times are on another clock than the person's: said ("Times in
    /// New York").
    pub clock: Option<String>,
}

pub struct Editor {
    target: Target,
    calendars: Vec<(usize, String)>,
    calendar: usize,
    title: Child<TextField>,
    location: Child<TextField>,
    start_day: Child<TextField>,
    start_time: Child<TextField>,
    end_day: Child<TextField>,
    end_time: Child<TextField>,
    until: Child<TextField>,
    notes: Child<TextField>,
    all_day: bool,
    /// How it can repeat (what's offered, and its own way if that's
    /// another), and which is chosen.
    repeats: Vec<Repeat>,
    repeat: usize,
    reminder: Option<i64>,
    today: Date,
    h24: bool,
    month_first: bool,
    clock: Option<String>,
    /// The start as last read, to move the end with it.
    start: DateTime,
    /// The draft it was opened with: what isn't a field of the window
    /// stays as it was.
    wrong: Option<String>,
    to_main: Messenger<Msg>,
}

#[derive(Clone)]
pub enum Edit {
    AllDay(bool),
    Calendar(usize),
    Repeat(usize),
    Reminder(usize),
    /// The start was typed in: the end follows.
    StartTyped,
    Save,
    Delete,
    Cancel,
}

impl Editor {
    pub fn open(o: Opening, to_main: Messenger<Msg>) -> WindowHandle<Editor> {
        let name = if o.target == Target::New { "New Event" } else { "Event" };
        olo_kit::open_window(WindowOptions::new(name, WIDTH, 420.0).dialog().fits_content(), move |cx| {
            let d = &o.draft;
            let field = |cx: &mut Cx<Editor>, name: &'static str, text: String, hint: &'static str| text_field(cx, |f| f.name(name).hint(hint).with_text(text));
            let title = field(cx, "Title", d.title.clone(), "");
            let location = field(cx, "Where", d.location.clone(), "");
            let start_day = field(cx, "Start Day", edit::date_text(d.start.date()), "");
            let start_time = field(cx, "Start Time", words::time(d.start.time(), o.h24), "");
            let end_day = field(cx, "End Day", edit::date_text(d.end.date()), "");
            let end_time = field(cx, "End Time", words::time(d.end.time(), o.h24), "");
            let until = field(cx, "Repeats Until", d.until.map(edit::date_text).unwrap_or_default(), "for good");
            let notes = text_field(cx, |f| f.name("Notes").multiline(false).with_text(d.notes.clone()));
            for start in [&start_day, &start_time] {
                cx.subscribe(start, |_: &mut Editor, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        cx.send(Edit::StartTyped);
                    }
                });
            }
            title.focus(cx);
            let mut repeats = Repeat::OFFERED.to_vec();
            let repeat = match repeats.iter().position(|r| *r == d.repeat) {
                Some(i) => i,
                None => {
                    repeats.push(d.repeat.clone());
                    repeats.len() - 1
                }
            };
            Editor {
                target: o.target.clone(),
                calendars: o.calendars.clone(),
                calendar: o.calendar.min(o.calendars.len().saturating_sub(1)),
                title,
                location,
                start_day,
                start_time,
                end_day,
                end_time,
                until,
                notes,
                all_day: d.all_day,
                repeats,
                repeat,
                reminder: d.reminder,
                today: o.today,
                h24: o.h24,
                month_first: o.month_first,
                clock: o.clock.clone(),
                start: d.start,
                wrong: None,
                to_main,
            }
        })
    }

    /// A day and a time read from their fields (the time midnight, all
    /// day); or what's wrong with them.
    fn read(&self, day: &Child<TextField>, time: &Child<TextField>, which: &str, cx: &Cx<Self>) -> Result<DateTime, String> {
        let typed = day.read(cx).text().to_owned();
        let date = edit::date(&typed, self.today, self.month_first).ok_or_else(|| format!("“{}” isn’t a day I can read for when it {which}: try Nov 20, 2026-11-20 or Friday.", typed.trim()))?;
        if self.all_day {
            return Ok(date.at(0, 0, 0, 0));
        }
        let typed = time.read(cx).text().to_owned();
        let time = edit::time(&typed).ok_or_else(|| format!("“{}” isn’t a time I can read for when it {which}: try 14:30 or 2:30 PM.", typed.trim()))?;
        Ok(date.to_datetime(time))
    }

    /// The draft the fields say; or what's wrong with them.
    fn draft(&self, cx: &Cx<Self>) -> Result<Draft, String> {
        let start = self.read(&self.start_day, &self.start_time, "starts", cx)?;
        let end = self.read(&self.end_day, &self.end_time, "ends", cx)?;
        let repeat = self.repeats[self.repeat.min(self.repeats.len() - 1)].clone();
        let typed = self.until.read(cx).text().trim().to_owned();
        let until = match (typed.is_empty() || matches!(repeat, Repeat::Never | Repeat::Other(_)), edit::date(&typed, self.today, self.month_first)) {
            (true, _) => None,
            (false, Some(day)) => Some(day),
            (false, None) => return Err(format!("“{typed}” isn’t a day I can read for when it stops repeating: leave it empty to go on for good.")),
        };
        let draft = Draft {
            title: self.title.read(cx).text().to_owned(),
            location: self.location.read(cx).text().to_owned(),
            notes: self.notes.read(cx).text().to_owned(),
            all_day: self.all_day,
            start,
            end,
            repeat,
            until,
            reminder: self.reminder,
        };
        match draft.wrong() {
            Some(wrong) => Err(wrong.to_owned()),
            None => Ok(draft),
        }
    }

    fn set(&self, field: &Child<TextField>, text: String, cx: &mut Cx<Self>) {
        if field.read(cx).text() != text {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
    }
}

impl View for Editor {
    type Message = Edit;

    fn message(&mut self, message: Edit, cx: &mut Cx<Self>) {
        match message {
            Edit::AllDay(on) => self.all_day = on,
            Edit::Calendar(i) => self.calendar = i,
            Edit::Repeat(i) => self.repeat = i,
            Edit::Reminder(i) => self.reminder = REMINDERS.get(i).and_then(|r| r.0),
            Edit::StartTyped => {
                // The end keeps its distance from the start (once what's
                // typed reads as a day and a time).
                let (Ok(start), Ok(end)) = (self.read(&self.start_day, &self.start_time, "starts", cx), self.read(&self.end_day, &self.end_time, "ends", cx)) else { return };
                if start == self.start {
                    return;
                }
                let moved = start.since(self.start).unwrap_or_default();
                self.start = start;
                let Ok(end) = end.checked_add(moved) else { return };
                self.set(&self.end_day, edit::date_text(end.date()), cx);
                if !self.all_day {
                    self.set(&self.end_time, words::time(end.time(), self.h24), cx);
                }
                return;
            }
            Edit::Save => match self.draft(cx) {
                Ok(draft) => {
                    let cal = self.calendars.get(self.calendar).map_or(0, |c| c.0);
                    self.to_main.send(Msg::Save(Box::new(Saving { target: self.target.clone(), draft, cal })));
                    cx.close_window();
                    return;
                }
                Err(wrong) => self.wrong = Some(wrong),
            },
            Edit::Delete => {
                self.to_main.send(Msg::Delete(Box::new(self.target.clone())));
                cx.close_window();
                return;
            }
            Edit::Cancel => {
                cx.close_window();
                return;
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let existing = self.target != Target::New;
        let row = || div().w_full().flex().flex_row().items_center().gap(px(m::TIGHT));
        let names: Vec<SharedString> = self.calendars.iter().map(|c| SharedString::from(c.1.clone())).collect();
        let mut col = div()
            .w_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|_: &mut Editor, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    cx.send(Edit::Cancel);
                }
            }))
            .child(TextControl::new(cx, &self.title).label("Title:").divider(DIVIDER))
            .child(TextControl::new(cx, &self.location).label("Where:").divider(DIVIDER))
            // (An event there stays in its calendar.)
            .child(
                MenuField::new("event-calendar", names, self.calendar)
                    .label("Calendar:")
                    .divider(DIVIDER)
                    .width(DIVIDER + DAY_FIELD)
                    .enabled(!existing && self.calendars.len() > 1)
                    .on_change(cx.msg_with(|i: &usize| Edit::Calendar(*i))),
            )
            .child(div().pl(px(DIVIDER)).child(CheckBox::new("All day", self.all_day).id("all-day").on_toggle(cx.msg_with(|on: &bool| Edit::AllDay(*on)))));
        let mut starts = row().child(TextControl::new(cx, &self.start_day).label("Starts:").divider(DIVIDER).width(DIVIDER + DAY_FIELD));
        let mut ends = row().child(TextControl::new(cx, &self.end_day).label("Ends:").divider(DIVIDER).width(DIVIDER + DAY_FIELD));
        if !self.all_day {
            starts = starts.child(TextControl::new(cx, &self.start_time).width(TIME_FIELD));
            ends = ends.child(TextControl::new(cx, &self.end_time).width(TIME_FIELD));
        }
        col = col.child(starts).child(ends);
        if let Some(clock) = &self.clock {
            col = col.child(div().pl(px(DIVIDER)).child(StringView::new(format!("Times in {clock}.")).small(true).dim(true).height(m::LABEL_ROW)));
        }
        let repeats: Vec<SharedString> = self.repeats.iter().map(|r| SharedString::from(r.name())).collect();
        col = col.child(MenuField::new("event-repeat", repeats, self.repeat).label("Repeats:").divider(DIVIDER).width(DIVIDER + DAY_FIELD).on_change(cx.msg_with(|i: &usize| Edit::Repeat(*i))));
        if !matches!(self.repeats[self.repeat], Repeat::Never | Repeat::Other(_)) {
            col = col.child(TextControl::new(cx, &self.until).label("Until:").divider(DIVIDER).width(DIVIDER + DAY_FIELD));
        }
        let reminders: Vec<SharedString> = REMINDERS.iter().map(|r| SharedString::from(r.1)).collect();
        // (One it has that isn't offered shows as the nearest longer.)
        let reminder = REMINDERS.iter().position(|r| r.0 == self.reminder).or_else(|| REMINDERS.iter().position(|r| r.0 >= self.reminder)).unwrap_or(0);
        col = col.child(MenuField::new("event-reminder", reminders, reminder).label("Reminder:").divider(DIVIDER).width(DIVIDER + DAY_FIELD).on_change(cx.msg_with(|i: &usize| Edit::Reminder(*i))));
        // Notes: several lines, in a well.
        let well = canvas(|g, b| {
            let p = theme::palette();
            let inner = style::inset(g, b, 2.0);
            style::fill_rect(g, inner, style::c(p.document_background));
            style::sunken(g, b, &p, false);
        })
        .absolute()
        .inset_0();
        col = col.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .child(div().w(px(DIVIDER)).flex_shrink_0().child(StringView::new("Notes:")))
                .child(div().relative().flex_1().h(px(NOTES)).child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&self.notes))),
        );
        if let Some(wrong) = &self.wrong {
            col = col.child(div().pl(px(DIVIDER)).child(StringView::new(wrong.clone()).wrap(true).color(style::c(p.failure))));
        }
        let wide = button_width("Cancel", false).max(button_width("Save", false)).max(button_width("Delete", false));
        let mut buttons = div().w_full().mt(px(m::TIGHT)).flex().flex_row().items_center();
        if existing {
            buttons = buttons.child(Button::new("Delete").id("delete").min_width(wide).on_click(cx.msg(Edit::Delete)));
        }
        buttons = buttons.child(div().flex_1()).child(
            button_row()
                .child(Button::new("Cancel").id("cancel").min_width(wide).on_click(cx.msg(Edit::Cancel)))
                .child(Button::new("Save").id("save").default(true).min_width(wide).on_click(cx.msg(Edit::Save))),
        );
        col.child(buttons).into_element()
    }
}
