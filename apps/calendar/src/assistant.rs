//! Calendar's assistant (the TODO's "the assistant", as Weather's is): a
//! question about what's on is answered from the events themselves.
//!
//! A question goes to Preferences ▸ AI's Calendar model
//! (`AiSettings::calendar`: Astra 6 at Medium by default, through the
//! Token Pool) on a worker, with the day and hour it is, the person's
//! zone, and the events of the calendars shown from a month back to
//! four months on, one a line ([`listing`]). The answer is JSON: its
//! words, told as they come; the events it points to (by their number
//! in the listing: a click goes to each); and events it proposes
//! (a title, when, where), each opened in the event's window for the
//! person to save or not. It never writes anything itself. A follow-up
//! carries the conversation before it. Escape or Stop lets go of the
//! answer where it is.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jiff::civil::DateTime;
use olo_calendar::event::Answer;
use olo_protocol::{Client, Reply, Request};
use serde_json::{Value, json};

use crate::{model::Shown, words};

/// How far back and on the events it's given go (days), and the most
/// of them.
pub const BACK_DAYS: i32 = 31;
pub const ON_DAYS: i32 = 124;
pub const MOST: usize = 2000;

/// An event the answer points to: which (its address and its start, as
/// `Msg::Reveal` takes them) and how it's said.
#[derive(Clone, Debug, PartialEq)]
pub struct Pointed {
    pub href: String,
    pub start: DateTime,
    pub said: String,
}

/// An event the answer proposes: opened in the event's window, saved
/// only by the person.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub title: String,
    pub start: DateTime,
    pub end: DateTime,
    pub all_day: bool,
    pub location: String,
    pub notes: String,
}

/// One question and its answer.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Turn {
    pub question: String,
    /// Empty when it was stopped before a word came.
    pub answer: String,
    pub events: Vec<Pointed>,
    pub proposals: Vec<Proposal>,
    /// The answer went wrong: `answer` says why.
    pub failed: bool,
}

/// What the answer was: its words, what it points to, what it proposes.
pub type Answered = (String, Vec<Pointed>, Vec<Proposal>);

/// What the worker tells the window, by the question's number.
#[derive(Clone)]
pub enum Asked {
    /// The model's name and effort, as it starts.
    Model(u64, String),
    /// The words as far as they've come.
    SoFar(u64, String),
    /// The answer, or why not.
    Done(u64, Result<Answered, String>),
}

/// The questions offered before any is asked: what people ask of a
/// calendar most, none assuming what theirs holds. After five in the
/// evening the day's are tomorrow's. (One line each in the column.)
pub fn suggestions(hour: i8) -> Vec<String> {
    // (Each said whole: "today" sits differently in each language.)
    let tomorrow = hour >= 17;
    vec![
        (if tomorrow { olo_kit::words::tr("What’s on tomorrow?") } else { olo_kit::words::tr("What’s on today?") }).to_owned(),
        olo_kit::words::tr("What’s next?").to_owned(),
        (if tomorrow { olo_kit::words::tr("When am I free tomorrow?") } else { olo_kit::words::tr("When am I free today?") }).to_owned(),
        olo_kit::words::tr("What does my week look like?").to_owned(),
        olo_kit::words::tr("What’s on this weekend?").to_owned(),
        olo_kit::words::tr("Which invitations haven’t I answered?").to_owned(),
        olo_kit::words::tr("Find a free hour this week").to_owned(),
        olo_kit::words::tr("What’s coming up next week?").to_owned(),
        olo_kit::words::tr("Add lunch tomorrow at noon").to_owned(),
    ]
}

/// The events as the model is given them, one a line, numbered from 1,
/// and which occurrence each number is (its address, its start). In
/// order; `calendar` names an event's calendar.
pub fn listing(shown: &[Shown], calendar: impl Fn(&Shown) -> String) -> (String, Vec<(String, DateTime)>) {
    let mut lines = String::new();
    let mut index = Vec::new();
    for s in shown.iter().take(MOST) {
        index.push((s.href.to_string(), s.start));
        let when = if s.all_day {
            let (first, last) = s.days();
            if first == last { format!("{first} all day") } else { format!("{first} to {last} all day") }
        } else {
            format!(
                "{} {:02}:{:02} to {}{:02}:{:02}",
                s.start.date(),
                s.start.hour(),
                s.start.minute(),
                if s.end.date() == s.start.date() { String::new() } else { format!("{} ", s.end.date()) },
                s.end.hour(),
                s.end.minute()
            )
        };
        let mut more = Vec::new();
        if !s.location.is_empty() {
            more.push(format!("at {}", s.location.replace(['|', '\n'], " ")));
        }
        if s.repeats {
            more.push("repeats".to_owned());
        }
        match s.going {
            Some(Answer::NotYet) => more.push("invitation not answered".to_owned()),
            Some(Answer::No) => more.push("invitation declined".to_owned()),
            Some(Answer::Maybe) => more.push("answered maybe".to_owned()),
            Some(Answer::Yes) => more.push("invitation accepted".to_owned()),
            None => {}
        }
        if s.free {
            more.push("takes no time".to_owned());
        }
        lines.push_str(&format!(
            "{} | {} ({}) | {} | {}{}\n",
            index.len(),
            when,
            &words::weekday_name(s.start.date())[..3],
            s.title.replace(['|', '\n'], " "),
            calendar(s).replace(['|', '\n'], " "),
            if more.is_empty() { String::new() } else { format!(" | {}", more.join(", ")) }
        ));
    }
    (lines, index)
}

/// The answer's shape: its words, the events it points to, the events
/// it proposes.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "events": {"type": "array", "items": {"type": "integer"}},
            "propose": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string"},
                        "end": {"type": "string"},
                        "all_day": {"type": "boolean"},
                        "location": {"type": "string"},
                        "notes": {"type": "string"}
                    },
                    "required": ["title", "start", "end", "all_day", "location", "notes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["answer", "events", "propose"],
        "additionalProperties": false
    })
}

/// The instructions, with the day and hour it is (`now`, on the
/// person's clock), their zone, their calendars and the events.
pub fn system(now: DateTime, zone: &str, calendars: &[String], events: &str, h24: bool) -> String {
    format!(
        "You are the assistant in Olo Calendar, a desktop calendar app. You answer questions about the person's calendars and \
help them plan. It is now {day}, {date} {hour:02}:{minute:02} for them (time zone {zone}). Their calendars: {calendars}.\n\n\
Below are their events from {back} days back to {on} days on, one a line, every time on their own clock: \
number | when (weekday) | title | calendar | more (where it is, whether it repeats: then each line is one occurrence, \
an invitation and its answer, an event that takes no time). Days with no line have nothing on. A declined invitation \
isn't something they're going to.\n\n\
Answer from these events: be direct and specific, with days and times as people say them ({clock}; \"today\", \
\"tomorrow\", \"Friday\", a date further off). Free time is what no event that takes time covers, within an ordinary day \
(9:00 to 18:00) unless they say otherwise. If the events can't tell, say what's missing. Keep it short: a sentence or two, \
then a few short lines if they help. Plain text: no Markdown headings, tables or emphasis; lines starting with \"• \" are \
fine.\n\n\
In \"events\", give the numbers of the events your answer is about (the next one, the ones on that day, the ones that \
clash), at most 8, in order; none when it's about none in particular.\n\n\
In \"propose\", only when they ask for something to be added or planned (\"add lunch tomorrow at noon\", \"book an hour \
for the report on Friday\"): each event with its title, its start and end as YYYY-MM-DDTHH:MM on their clock (for a whole \
day: all_day true, the start that day at 00:00 and the end the day after its last at 00:00), where (or \"\") and notes (or \
\"\"). Choose a free time when they leave it to you, and an hour's length unless they say. You can't add, change or delete \
anything yourself: a proposal opens for them to look over and save, so say \"I've drafted\", never \"I've added\". To \
change or delete an event, tell them which one and that they can open it.\n\n\
Events:\n{events}",
        day = words::weekday_name(now.date()),
        date = now.date(),
        hour = now.hour(),
        minute = now.minute(),
        calendars = if calendars.is_empty() { "none shown".to_owned() } else { calendars.join(", ") },
        back = BACK_DAYS,
        on = ON_DAYS,
        clock = if h24 { "24-hour times, \"14:00\"" } else { "12-hour times, \"2 PM\", \"9:30 AM\"" },
    )
}

/// The answer's words, the events it points to (by `index`, the
/// listing's: a number that names none is passed over) with how each
/// is said, and what it proposes (one that isn't a time, or ends before
/// it starts, passed over).
pub fn read(answer: &Value, index: &[(String, DateTime)], shown: &[Shown], h24: bool) -> Answered {
    let text = answer["answer"].as_str().unwrap_or("").trim().to_owned();
    let mut events: Vec<Pointed> = Vec::new();
    for n in answer["events"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
        let Some((href, start)) = (n as usize).checked_sub(1).and_then(|i| index.get(i)) else { continue };
        let Some(s) = shown.iter().find(|s| *s.href == **href && s.start == *start) else { continue };
        let said = format!("{} · {}", s.title, words::when(s.start, s.end, s.all_day, h24));
        if !events.iter().any(|e| e.href == *href && e.start == *start) {
            events.push(Pointed { href: href.clone(), start: *start, said });
        }
    }
    events.truncate(8);
    let time = |v: &Value| -> Option<DateTime> {
        let text = v.as_str()?.trim();
        text.parse::<DateTime>().ok().or_else(|| text.parse::<jiff::civil::Date>().ok().map(|d| d.at(0, 0, 0, 0)))
    };
    let proposals = answer["propose"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let (start, end) = (time(&p["start"])?, time(&p["end"])?);
            let all_day = p["all_day"].as_bool().unwrap_or(false);
            let title = p["title"].as_str().unwrap_or("").trim().to_owned();
            (end > start && !title.is_empty()).then(|| Proposal {
                title,
                start,
                end,
                all_day,
                location: p["location"].as_str().unwrap_or("").trim().to_owned(),
                notes: p["notes"].as_str().unwrap_or("").trim().to_owned(),
            })
        })
        .take(6)
        .collect();
    (text, events, proposals)
}

/// How a proposal is said on its link: "Lunch · Thu, Nov 19, 12 PM – 1 PM".
pub fn proposal_said(p: &Proposal, h24: bool) -> String {
    let day = format!("{}, {} {}", &words::weekday_name(p.start.date())[..3], &words::month_name(p.start.date())[..3], p.start.date().day());
    if p.all_day { format!("{} · {day}", p.title) } else { format!("{} · {day}, {}", p.title, words::when(p.start, p.end, false, h24)) }
}

/// What turns the model's answer into what the window shows.
pub type Reader = Box<dyn Fn(&Value) -> Answered>;

/// Ask `question` (after `history`) on a worker, telling `send` as it
/// comes; stops once `cancel` is set. `given` makes what the model is
/// given, on the worker (the events worked out over five months: not
/// the window's to wait for): the instructions, and what reads its
/// answer.
pub fn ask(n: u64, question: String, given: impl FnOnce() -> (String, Reader) + Send + 'static, history: Vec<olo_ai::Turn>, cancel: Arc<AtomicBool>, send: impl Fn(Asked) + Send + 'static) {
    let _ = std::thread::Builder::new().name("calendar-ask".into()).spawn(move || {
        let done = |r: Result<Answered, String>| send(Asked::Done(n, r));
        let (system, read) = given();
        let settings = match Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&Request::GetSettings).map_err(|e| e.to_string())) {
            Ok(Reply::Settings { settings }) => settings,
            Ok(_) => return done(Err(olo_kit::words::tr("The window server didn’t give its settings.").into())),
            Err(e) => return done(Err(e)),
        };
        let feature = settings.ai.calendar.clone();
        if !feature.enabled {
            return done(Err(olo_kit::words::tr("The assistant is off in Preferences ▸ AI ▸ Calendar.").into()));
        }
        let effort = olo_protocol::AI_EFFORTS.iter().find(|(id, _)| *id == feature.effort).map_or(feature.effort.as_str(), |(_, name)| name);
        send(Asked::Model(n, format!("{} · {effort}", feature.model_name())));
        let ask = olo_ai::Ask { system, content: vec![olo_ai::text_block(question)], schema: schema(), max_tokens: 8000, history };
        let mut told = String::new();
        let result = olo_ai::ask_streaming(&settings.ai, &feature, &ask, &cancel, |so_far| {
            if let Some(words) = olo_ai::partial_string(so_far, "answer")
                && words.len() > told.len()
            {
                told.clone_from(&words);
                send(Asked::SoFar(n, words));
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        done(result.map(|v| read(&v)).map_err(|e| e.to_string()));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as A;

    use olo_theme::Hue;

    use super::*;

    fn shown(title: &str, start: DateTime, end: DateTime) -> Shown {
        Shown {
            cal: 0,
            href: A::from(format!("https://x/{title}.ics").as_str()),
            title: A::from(title),
            location: A::from(""),
            start,
            end,
            all_day: false,
            hue: Hue::Blue,
            id: None,
            repeats: false,
            tentative: false,
            going: None,
            free: false,
        }
    }

    /// About ten, each asked once and short enough for one line; the
    /// day's become tomorrow's after five in the evening.
    #[test]
    fn suggestions_follow_the_day() {
        let morning = suggestions(9);
        assert!((8..=12).contains(&morning.len()));
        let mut seen = morning.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), morning.len(), "none twice");
        assert_eq!(morning[0], "What’s on today?");
        for q in morning.iter().chain(&suggestions(19)) {
            assert!(q.chars().count() <= 40, "too long for one line: {q}");
        }
        assert_eq!(suggestions(19)[0], "What’s on tomorrow?");
        assert!(suggestions(19).contains(&"When am I free tomorrow?".to_owned()));
    }

    /// The events as the model has them: numbered, each with when, its
    /// weekday, its title and calendar, and what else matters; and the
    /// model is told the day, the zone and that it only proposes.
    #[test]
    fn the_events_as_the_model_is_given_them() {
        let day = jiff::civil::date(2026, 11, 18);
        let mut review = shown("Design review", day.at(14, 0, 0, 0), day.at(15, 30, 0, 0));
        review.location = A::from("Room 2");
        review.going = Some(Answer::NotYet);
        let mut release = shown("Release 2.4", day.tomorrow().unwrap().at(0, 0, 0, 0), jiff::civil::date(2026, 11, 21).at(0, 0, 0, 0));
        release.all_day = true;
        let mut standup = shown("Stand|up", day.tomorrow().unwrap().at(9, 30, 0, 0), day.tomorrow().unwrap().at(9, 45, 0, 0));
        standup.repeats = true;
        let (lines, index) = listing(&[review.clone(), release, standup], |_| "Work".to_owned());
        assert_eq!(
            lines,
            "1 | 2026-11-18 14:00 to 15:30 (Wed) | Design review | Work | at Room 2, invitation not answered\n\
2 | 2026-11-19 to 2026-11-20 all day (Thu) | Release 2.4 | Work\n\
3 | 2026-11-19 09:30 to 09:45 (Thu) | Stand up | Work | repeats\n"
        );
        assert_eq!(index[0], ("https://x/Design review.ics".to_owned(), day.at(14, 0, 0, 0)));
        let s = system(day.at(10, 30, 0, 0), "Europe/Amsterdam", &["Work".to_owned(), "Home".to_owned()], &lines, false);
        assert!(s.contains("It is now Wednesday, 2026-11-18 10:30") && s.contains("Europe/Amsterdam") && s.contains("Their calendars: Work, Home."));
        assert!(s.contains("never \"I've added\"") && s.ends_with(&lines) && s.contains("12-hour times"));
        assert_eq!(schema()["required"], json!(["answer", "events", "propose"]));
    }

    /// An answer read: its words; the events it points to, by number
    /// (one twice, or that names none, passed over); what it proposes
    /// (a date alone is its midnight; one that ends before it starts, or
    /// has no title, passed over).
    #[test]
    fn an_answer_is_read() {
        let day = jiff::civil::date(2026, 11, 18);
        let all = [shown("Design review", day.at(14, 0, 0, 0), day.at(15, 30, 0, 0)), shown("Interview", day.at(15, 0, 0, 0), day.at(15, 45, 0, 0))];
        let (_, index) = listing(&all, |_| "Work".to_owned());
        let v = json!({
            "answer": " Two things, and they overlap. I've drafted lunch. ",
            "events": [2, 1, 2, 9, 0],
            "propose": [
                {"title": "Lunch", "start": "2026-11-19T12:00", "end": "2026-11-19T13:00", "all_day": false, "location": "", "notes": ""},
                {"title": "Day off", "start": "2026-11-20", "end": "2026-11-21", "all_day": true, "location": "", "notes": "Booked"},
                {"title": "Backwards", "start": "2026-11-19T13:00", "end": "2026-11-19T12:00", "all_day": false, "location": "", "notes": ""},
                {"title": " ", "start": "2026-11-19T12:00", "end": "2026-11-19T13:00", "all_day": false, "location": "", "notes": ""}
            ]
        });
        let (words, events, proposals) = read(&v, &index, &all, false);
        assert_eq!(words, "Two things, and they overlap. I've drafted lunch.");
        assert_eq!(events.iter().map(|e| e.said.as_str()).collect::<Vec<_>>(), ["Interview · 3 PM – 3:45 PM", "Design review · 2 PM – 3:30 PM"]);
        assert_eq!(events[0].start, day.at(15, 0, 0, 0));
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposal_said(&proposals[0], false), "Lunch · Thu, Nov 19, 12 PM – 1 PM");
        assert_eq!((proposal_said(&proposals[1], true).as_str(), proposals[1].all_day, proposals[1].notes.as_str()), ("Day off · Fri, Nov 20", true, "Booked"));
        // (Nothing there to read: nothing.)
        assert_eq!(read(&json!({}), &index, &all, false), (String::new(), Vec::new(), Vec::new()));
    }
}
