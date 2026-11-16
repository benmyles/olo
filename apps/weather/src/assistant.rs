//! Weather's assistant (Ben, 2026-11-14: "an in-built assistant (model
//! configured in prefs -> ai and using astra xhigh through token pool by
//! default) where i can ask it questions like "when's a good time to run
//! this afternoon?" or "will it be socked in tomorrow?" or "will it be
//! foggy all day?" and the model should get the full set of raw data to
//! answer me effectively").
//!
//! A question goes to Preferences ▸ AI's Weather model (`AiSettings::
//! weather`: Astra 6 thinking at Medium by default, through the Token
//! Pool; Ben, later the same day: "change the default thinking level for
//! astra to medium") on
//! a worker, with the place, its clock now, the person's units and
//! everything the provider answered, as it answered it (every layer, by
//! the minute, hour and day). The answer is JSON: its words, told as they
//! come, and the spans of time it points to, which the chart marks
//! (`windows`). A follow-up carries the conversation before it. Escape or
//! Stop lets go of the answer where it is.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_protocol::{Client, Reply, Request};
use serde_json::{Value, json};

use crate::{model::Forecast, places::Place, units::Units};

/// A span the answer points to: when, and a few words.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

/// One question and its answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub question: String,
    /// Empty when it was stopped before a word came.
    pub answer: String,
    pub windows: Vec<Window>,
    /// The answer went wrong: `answer` says why.
    pub failed: bool,
}

/// What the worker tells the window, by the question's number.
#[derive(Clone)]
pub enum Asked {
    /// The model's name and effort, as it starts.
    Model(u64, String),
    /// The words as far as they've come.
    SoFar(u64, String),
    /// The answer, or why not.
    Done(u64, Result<(String, Vec<Window>), String>),
}

/// The questions offered before any is asked: about ten of what people
/// ask of a forecast most (Ben, 2026-11-15: "i want about 10 of them that
/// cover all the common things someone might want to ask about the
/// weather"): rain, what to wear, how warm and how cold, wind, storms,
/// the weekend, the week, the sun, a time to be outside, and the clouds;
/// none assuming what the person does (Ben, 2026-11-15: "the prompts
/// shouldn't assume that they should be general enough to apply to any
/// olo user"). For the place's own `hour` (0–23): after five in the
/// evening the day's questions are tomorrow's; and while it's `raining`
/// there, when it stops.
pub fn suggestions(hour: u32, raining: bool) -> Vec<String> {
    let late = hour >= 17;
    let day = if late { "tomorrow" } else { "today" };
    let rain = match (raining, late) {
        (true, _) => "When will the rain stop?".to_owned(),
        (false, false) => "Will it rain today?".to_owned(),
        (false, true) => "Will it rain tomorrow?".to_owned(),
    };
    // (Short: a link is one line, and the assistant's column is 260
    // points at its narrowest.)
    let warmth = if late { "How cold will it get tonight?" } else { "How warm will it get today?" };
    let sun = if late { "When does the sun rise tomorrow?" } else { "Do I need sunscreen today?" };
    vec![
        rain,
        format!("What should I wear {day}?"),
        warmth.to_owned(),
        format!("How windy will it be {day}?"),
        "Any storms on the way?".to_owned(),
        "How’s the weekend looking?".to_owned(),
        "Which day this week looks best?".to_owned(),
        sun.to_owned(),
        format!("Best time for a run {day}?"),
        format!("How cloudy will it be {day}?"),
    ]
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "windows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "start": {"type": "string"},
                        "end": {"type": "string"},
                        "label": {"type": "string"}
                    },
                    "required": ["start", "end", "label"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["answer", "windows"],
        "additionalProperties": false
    })
}

/// The instructions, with the place, its clock and the units.
pub fn system(place: &Place, forecast: &Forecast, units: &Units, now: i64) -> String {
    let offset = place.offset_at_or(now, forecast.offset);
    let local = chrono::DateTime::from_timestamp(now + offset as i64, 0).map(|d| d.format("%A %-d %B %Y, %H:%M").to_string()).unwrap_or_default();
    let sign = if offset < 0 { '-' } else { '+' };
    let zone = place.timezone.clone().unwrap_or_else(|| format!("UTC{sign}{:02}:{:02}", offset.abs() / 3600, (offset.abs() % 3600) / 60));
    let elevation = place.elevation.map(|e| format!(", {e:.0} m above sea level")).unwrap_or_default();
    let region = if place.region.is_empty() { String::new() } else { format!(", {}", place.region) };
    let q = crate::model::Quantity::Temperature;
    let units_said = format!(
        "temperatures in {}, wind in {}, cloud heights in {}, visibility in {}, precipitation in {}, pressure in {}",
        units.unit(q),
        units.unit(crate::model::Quantity::Speed),
        units.unit(crate::model::Quantity::Height),
        units.unit(crate::model::Quantity::Distance),
        units.unit(crate::model::Quantity::Amount),
        units.unit(crate::model::Quantity::Pressure)
    );
    format!(
        "You are the assistant in Olo Weather, a desktop weather app. You answer questions about the forecast for {name}{region} \
({coords}{elevation}; time zone {zone}). It is now {local} there.\n\n\
Answer from the forecast data below: everything {provider} answered, as it answered it — by the minute for the next hour, \
by the hour for 5 days and by the day for 7, every layer (the forecast's timelines, then more layers by the hour and day). \
Its values are metric: °C; m/s for wind and gusts; hPa; mm/h for intensities and mm for amounts; cm for snow depth; km for \
visibility; km above the ground for cloudBase and cloudCeiling (null: no cloud); % for cover, humidity and chances; weather \
codes are tomorrow.io's. Its times are UTC.\n\n\
Answer in the person's units ({units_said}), and give every time in the place's own clock, 24-hour, with the day when it isn't \
today (\"14:00\", \"tomorrow 09:00\", \"Tuesday 16:00\"). Be direct and specific: say when (to the hour), how likely (the \
chances), and why (the layers that decide it). Use the data, not the climate in general; if the data can't tell, say what's \
missing. Keep it short: a sentence or two, then a few short lines if they help. Plain text: no Markdown headings, tables or \
emphasis; lines starting with \"• \" are fine.\n\n\
Don't assume what the person does: answer what they ask, for the plans they mention, weighing what decides them, and with \
none, for an ordinary day out (the temperature and feels-like, rain and its chance, wind and gusts). What decides a plan \
depends on it: being outside, the temperature and feels-like, rain and its chance, wind, UV and daylight (the sun's times are \
in the daily data); anything in the air (flying, drones, paragliding, skydiving), the cloud base and ceiling, cover, \
visibility, wind and gusts, thunderstorms; on the water or the hills, wind and gusts, visibility, storms, the temperature \
falling; the garden, frost (the night's low), rain amounts, wind and heat; driving, ice, snow, fog and heavy rain. Say what \
the data shows for it; leave its rules (limits, thresholds) to the person unless they're well known and you say they're \
typical.\n\n\
In \"windows\", give the spans of time the answer points to, if any (a dry spell, when the base lifts, the fog's hours): each \
its start and end as ISO 8601 with the place's UTC offset, and a label of two to four words (\"Dry and calm\", \"Base over \
1,500 m\"). None when nothing in particular.\n\n\
Forecast data:\n{data}",
        name = match place.found_as() {
            Some(found) => format!("{} (the person's name for it; it's {found})", place.title()),
            None => place.name.clone(),
        },
        coords = place.coordinates(),
        provider = crate::provider::by_id(&forecast.provider).name(),
        data = forecast.raw,
    )
}

/// The answer's words and spans.
pub fn read(answer: &Value) -> (String, Vec<Window>) {
    let words = answer["answer"].as_str().unwrap_or("").trim().to_owned();
    let windows = answer["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let t = |k: &str| chrono::DateTime::parse_from_rfc3339(w[k].as_str()?.trim()).ok().map(|d| d.timestamp());
            let (start, end) = (t("start")?, t("end")?);
            (end > start).then(|| Window { start, end, label: w["label"].as_str().unwrap_or("").trim().to_owned() })
        })
        .take(8)
        .collect();
    (words, windows)
}

/// Ask `question` (after `history`) on a worker, telling `send` as it
/// comes; stops once `cancel` is set.
pub fn ask(n: u64, question: String, system: String, history: Vec<olo_ai::Turn>, cancel: Arc<AtomicBool>, send: impl Fn(Asked) + Send + 'static) {
    let _ = std::thread::Builder::new().name("weather-ask".into()).spawn(move || {
        let done = |r: Result<(String, Vec<Window>), String>| send(Asked::Done(n, r));
        let settings = match Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&Request::GetSettings).map_err(|e| e.to_string())) {
            Ok(Reply::Settings { settings }) => settings,
            Ok(_) => return done(Err("The window server didn’t give its settings.".into())),
            Err(e) => return done(Err(e)),
        };
        let feature = settings.ai.weather.clone();
        if !feature.enabled {
            return done(Err("The assistant is off in Preferences ▸ AI ▸ Weather.".into()));
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
    use super::*;

    /// About ten, each asked once; the day's become tomorrow's after five
    /// in the evening, and the rain's is when it stops while it rains.
    #[test]
    fn suggestions_follow_the_places_day() {
        let morning = suggestions(9, false);
        assert!((9..=12).contains(&morning.len()), "{}", morning.len());
        let mut seen = morning.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), morning.len(), "none twice");
        assert_eq!(morning[0], "Will it rain today?");
        assert!(morning.iter().any(|q| q.contains("wear today")));
        // (One line each in the assistant's narrowest column.)
        for q in suggestions(9, false).iter().chain(&suggestions(19, true)) {
            assert!(q.chars().count() <= 42, "too long for one line: {q}");
        }
        let evening = suggestions(19, false);
        assert_eq!(evening[0], "Will it rain tomorrow?");
        assert!(evening.iter().all(|q| !q.contains("today")), "{evening:?}");
        assert_eq!(suggestions(19, true)[0], "When will the rain stop?");
    }

    #[test]
    fn an_answer_is_read_with_its_spans() {
        let v = json!({
            "answer": " The rain stops by 14:00. ",
            "windows": [
                {"start": "2026-11-15T14:00:00-08:00", "end": "2026-11-15T17:00:00-08:00", "label": "Dry"},
                {"start": "bad", "end": "2026-11-15T17:00:00-08:00", "label": "x"},
                {"start": "2026-11-15T17:00:00-08:00", "end": "2026-11-15T16:00:00-08:00", "label": "backwards"}
            ]
        });
        let (words, windows) = read(&v);
        assert_eq!(words, "The rain stops by 14:00.");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].end - windows[0].start, 3 * 3600);
        assert_eq!(windows[0].label, "Dry");
    }

    #[test]
    fn the_assistant_is_given_the_place_and_all_the_data() {
        let place = Place::demo();
        let t = 1_794_711_600;
        let f = crate::provider::Provider::parse(&crate::demo::Demo, &crate::demo::raw(&place, t), t).unwrap();
        let s = system(&place, &f, &Units::METRIC, t);
        assert!(s.contains("Sample Field"));
        assert!(s.contains("Europe/Rome"));
        assert!(s.contains("\"hourly\""));
        assert!(s.contains("cloud heights in m"));
        assert_eq!(schema()["required"], json!(["answer", "windows"]));
    }
}
