//! Weather's Deskbar widgets (`olo_protocol::glance`; Ben, 2026-11-15:
//! "the weather app should make available a deskbar widget that is
//! customizable and can show useful stuff like 3/6/12 hour rain/cloud
//! forecast, 7 day high level forecast, and any other interesting
//! things"). `olo-weather --glance`, started by Deskbar, no window:
//!
//! - **Now**: the sky and temperature, what it feels like and the wind,
//!   and what the next hour does ("Rain stopping in about 25 minutes"),
//!   else the day's high and low.
//! - **Next Hours** (3, 6 or 12): six times across, each its sky, its
//!   temperature and the chance of rain (or the cloud cover) as a bar;
//!   over them what the rain does ("Rain likely from 15:00").
//! - **Next Days** (3, 5 or 7): each day's sky and its low to its high
//!   on one scale.
//!
//! For the place chosen in Weather, in its units. Each a timeline (an
//! entry an hour, or each of the next hours' times, for a day), so it
//! goes on with nothing run; made again when Weather's settings or
//! forecast change, and fetched every 30 minutes (`crate::FRESH`) as
//! Weather does (the cache theirs together: whichever fetched, the other
//! has it).

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use olo_protocol::glance::{Body, Entry, Glance, Request, Slot, Span, Timeline};
use zeroize::Zeroizing;

use crate::{
    Settings, key,
    model::{self, Forecast, Kind, Layer, Quantity, Stat, Step},
    places::Place,
    provider::{self, Error},
    units::Units,
};

/// How far ahead a timeline goes.
const AHEAD: i64 = 24 * 3600;
/// How many times a Next Hours widget shows.
const SLOTS: usize = 6;

/// Serve Deskbar's Weather widgets until it asks for none.
pub fn serve() -> std::process::ExitCode {
    let demo = crate::demo();
    let watched = vec![olo_kit::config_dir(), provider::cache_dir()];
    let mut st = State::default();
    olo_kit::glance::serve("weather", watched, move |asked| make(asked, demo, &mut st))
}

#[derive(Default)]
struct State {
    /// The key, once read from Passwords.
    key: Option<Arc<Zeroizing<String>>>,
    /// After a fetch failed: when to try again, and why it failed.
    retry_at: Option<i64>,
    error: Option<String>,
}

/// Every widget asked for, from the forecast (fetched first if it's old),
/// and when to look again.
fn make(asked: &[Request], demo: bool, st: &mut State) -> (Vec<(u32, Timeline)>, Option<Duration>) {
    let settings: Settings =
        if demo { Settings { provider: "demo".into(), places: vec![Place::demo()], ..Settings::default() } } else { olo_kit::store::load(&crate::settings_path()).unwrap_or_default() };
    let now = provider::now();
    let say = |detail: &str| asked.iter().map(|r| (r.widget, message(detail, now))).collect::<Vec<_>>();
    let Some(place) = settings.places.get(settings.chosen).or(settings.places.first()).cloned() else {
        return (say("Add a place in Weather"), None);
    };
    let provider = provider::by_id(&settings.provider);
    let mut forecast = provider::cached(provider, &place).filter(|f| f.provider == settings.provider);
    let old = forecast.as_ref().is_none_or(|f| now - f.fetched >= crate::FRESH);
    if old && st.retry_at.is_none_or(|t| now >= t) {
        match fetch(provider, &settings, &place, st) {
            Ok(f) => {
                provider::keep(&f, &place);
                st.retry_at = None;
                st.error = None;
                forecast = Some(f);
            }
            Err(e) => {
                // (Unreachable: again in 5 minutes; anything else, 15.)
                st.retry_at = Some(now + if matches!(e, Error::Unreachable(_)) { 300 } else { 900 });
                st.error = Some(e.to_string());
            }
        }
    }
    let Some(f) = forecast else {
        let why = st.error.clone().unwrap_or_else(|| olo_kit::words::tr("Open Weather to set up its data provider").into());
        return (say(&why), st.retry_at.map(|t| Duration::from_secs((t - now).max(60) as u64)));
    };
    let made = asked.iter().map(|r| (r.widget, timeline(r, &f, &place, &settings.units, now))).collect();
    // Again when it's old, or when a failed fetch may be tried again.
    let next = st.retry_at.unwrap_or(f.fetched + crate::FRESH).max(now + 60);
    (made, Some(Duration::from_secs((next - now) as u64)))
}

fn fetch(provider: &'static dyn provider::Provider, settings: &Settings, place: &Place, st: &mut State) -> Result<Forecast, Error> {
    let key = match &st.key {
        Some(k) => k.clone(),
        None if !provider.needs_key() => Arc::new(Zeroizing::new(String::new())),
        None if settings.key == key::Source::None => return Err(Error::Key(olo_kit::words::tr("Open Weather to set up its data provider").into())),
        None => {
            let k = Arc::new(key::read(provider, &settings.key)?);
            st.key = Some(k.clone());
            k
        }
    };
    provider.fetch(&key, place, &AtomicBool::new(false))
}

/// A widget saying only `detail` (no place, no forecast yet).
fn message(detail: &str, now: i64) -> Timeline {
    let glance = Glance { icon: Some("weather".into()), title: "Weather".into(), detail: detail.into(), tip: format!("Weather: {detail}."), ..Glance::default() };
    Timeline { entries: vec![Entry { from: now - now.rem_euclid(60), glance }], made: now }
}

/// Widget `r`'s timeline: an entry now and at each of its times on.
fn timeline(r: &Request, f: &Forecast, place: &Place, units: &Units, now: i64) -> Timeline {
    let kind = olo_protocol::glance::kind("weather", &r.kind);
    let opt = |id: &str| kind.map_or("", |k| k.value(&r.options, id)).to_owned();
    let every = match (r.kind.as_str(), opt("span").as_str()) {
        ("hours", "3") => 1800,
        ("hours", "12") => 7200,
        _ => 3600,
    };
    let local = |t: i64| t + place.offset_at_or(t, f.offset) as i64;
    let start = now - now.rem_euclid(60);
    let last = f.hourly.last().map_or(now, |h| h.time).min(now + AHEAD);
    let mut times = vec![start];
    let mut t = next_boundary(start, every, &local);
    while t <= last {
        times.push(t);
        t = next_boundary(t, every, &local);
    }
    let entries = times
        .into_iter()
        .map(|t| {
            let glance = match r.kind.as_str() {
                "hours" => hours(f, place, units, t, every, &opt("level"), &local),
                "days" => days(f, place, units, t, opt("days").parse().unwrap_or(7), &local),
                _ => now_glance(f, place, units, t, &local),
            };
            Entry { from: t, glance }
        })
        .collect();
    Timeline { entries, made: now }
}

/// The first time after `t` on a whole `every` seconds of the place's
/// clock (an hour, half an hour, two hours).
fn next_boundary(t: i64, every: i64, local: &impl Fn(i64) -> i64) -> i64 {
    let l = local(t);
    let next = l - l.rem_euclid(every) + every;
    t + (next - l)
}

/// The weather at `t`: its minute's or hour's, each layer from the hour
/// where the minute hasn't it.
fn at(f: &Forecast, t: i64) -> (Step, impl Fn(Layer) -> Option<f64>) {
    let s = f.now(t).cloned().unwrap_or_else(|| Step::new(t));
    let h = f.hour(t).cloned();
    let s2 = s.clone();
    (s, move |l: Layer| s2.get(l).or_else(|| h.as_ref().and_then(|h| h.get(l))))
}

/// "14" (or "14:30") in the place's clock.
fn hour_label(local: i64, minutes: bool) -> String {
    let (h, mi, _, _) = olo_kit::widgets::timeline::parts(local);
    if minutes && mi != 0 { format!("{h:02}:{mi:02}") } else { format!("{h:02}") }
}

/// The day holding `t`, as the provider gives days.
fn day_of(f: &Forecast, t: i64) -> Option<usize> {
    f.daily.iter().rposition(|d| d.time <= t).or((!f.daily.is_empty()).then_some(0))
}

fn high_low(f: &Forecast, units: &Units, t: i64) -> Option<String> {
    let d = &f.daily[day_of(f, t)?];
    let (hi, lo) = (d.get(Layer::Temperature, Stat::Max)?, d.get(Layer::Temperature, Stat::Min)?);
    Some(format!("High {}, low {}", units.degrees(hi), units.degrees(lo)))
}

/// Now: the sky and temperature, feels like and wind, the next hour.
fn now_glance(f: &Forecast, place: &Place, units: &Units, t: i64, _local: &impl Fn(i64) -> i64) -> Glance {
    let (s, get) = at(f, t);
    let code = s.code();
    let value = get(Layer::Temperature).map(|v| units.degrees(v)).unwrap_or_default();
    let title = model::describe_said(code).to_owned();
    // (Short: a Deskbar's width. The tip says it in full.)
    let feels = get(Layer::TemperatureApparent).map(|v| olo_kit::tr!("Feels like {}", units.degrees(v)));
    let wind = get(Layer::WindSpeed).map(|v| units.say(Quantity::Speed, v));
    let detail = [feels.clone(), wind.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    // The next hour while the minutes reach, else the day's high and low.
    let minutes: Vec<(f64, Kind)> = f.minutely.iter().filter(|m| m.time + 60 > t).take(60).map(|m| (m.precipitation().unwrap_or(0.0), m.kind())).collect();
    let sentence = (minutes.len() >= 30).then(|| crate::view::next_hour_sentence(&minutes, f.hour(t)));
    let note = sentence.as_deref().map(short_sentence).or_else(|| high_low(f, units, t)).unwrap_or_default();
    let said = [feels, wind.map(|w| format!("wind {w}"))].into_iter().flatten().collect::<Vec<_>>().join(", ");
    let tip = format!("{}: {title}, {value}. {said}. {}.", place.title(), sentence.unwrap_or_else(|| note.clone()));
    Glance { icon: Some(model::icon(code, f.is_night(t)).into()), value, title, detail, note: Some(note), body: None, tip, ..Glance::default() }
}

/// The next hour's sentence for a Deskbar's width: "Rain stopping in about
/// 25 minutes" is "Rain stops in 25 min".
fn short_sentence(s: &str) -> String {
    s.replace("stopping in about", "stops in")
        .replace("starting in about", "starts in")
        .replace(" minutes", " min")
        .replace("No precipitation in the next hour", "Dry for the next hour")
        .replace("No precipitation this hour", "Dry this hour")
}

/// Wet: a fair chance of it, or falling now.
fn wet(get: &impl Fn(Layer) -> Option<f64>) -> bool {
    get(Layer::PrecipitationProbability).is_some_and(|p| p >= 50.0) || get(Layer::PrecipitationIntensity).is_some_and(|r| r >= 0.1)
}

/// Next Hours: six times from `t`, each `every` seconds on.
fn hours(f: &Forecast, place: &Place, units: &Units, t: i64, every: i64, level: &str, local: &impl Fn(i64) -> i64) -> Glance {
    let mut times = vec![t];
    let mut next = next_boundary(t, every, local);
    while times.len() < SLOTS {
        times.push(next);
        next += every;
    }
    let half_hours = every < 3600;
    let mut said = Vec::new();
    let slots: Vec<Slot> = times
        .iter()
        .enumerate()
        .map(|(i, &at_t)| {
            let (s, get) = at(f, at_t);
            let label = if i == 0 { olo_kit::words::tr("Now").into() } else { hour_label(local(at_t), half_hours) };
            let value = get(Layer::Temperature).map(|v| units.degrees(v)).unwrap_or_default();
            let level = match level {
                "cloud" => get(Layer::CloudCover).map(|c| (c / 100.0) as f32),
                "none" => None,
                _ => get(Layer::PrecipitationProbability).map(|p| (p / 100.0) as f32),
            };
            said.push(format!("{label} {value} {}", olo_kit::words::mid_sentence(model::describe_said(s.code()))));
            Slot { label, icon: Some(model::icon(s.code(), f.is_night(at_t)).into()), value, level }
        })
        .collect();
    let span_h = (every * SLOTS as i64) / 3600;
    let (s, get) = at(f, t);
    let detail = match level {
        "cloud" => {
            let covers: Vec<f64> = times.iter().filter_map(|&x| at(f, x).1(Layer::CloudCover)).collect();
            match (covers.iter().cloned().reduce(f64::min), covers.iter().cloned().reduce(f64::max)) {
                (Some(a), Some(b)) if (b - a).abs() < 5.0 => olo_kit::tr!("Cloud cover about {}%", b.round()),
                (Some(a), Some(b)) => olo_kit::tr!("Cloud cover {}–{}%", a.round(), b.round()),
                _ => model::describe_said(s.code()).to_owned(),
            }
        }
        "none" => high_low(f, units, t).unwrap_or_else(|| model::describe_said(s.code()).to_owned()),
        _ => {
            // What the rain does over the times shown.
            let wet_at: Vec<bool> = times.iter().map(|&x| wet(&at(f, x).1)).collect();
            let what = |x: i64| match at(f, x).0.kind() {
                Kind::None => Kind::Rain.said(),
                k => k.said(),
            };
            let clock = |x: i64| {
                let (h, mi, _, _) = olo_kit::widgets::timeline::parts(local(x));
                format!("{h:02}:{mi:02}")
            };
            match (wet(&get), wet_at.iter().position(|w| *w != wet(&get))) {
                (true, Some(i)) => olo_kit::tr!("{} until about {}", what(t), clock(times[i])),
                (true, None) => olo_kit::tr!("{} for the next {} hours", what(t), span_h),
                (false, Some(i)) => olo_kit::tr!("{} likely from {}", what(times[i]), clock(times[i])),
                (false, None) => olo_kit::tr!("Dry for the next {} hours", span_h),
            }
        }
    };
    let value = get(Layer::Temperature).map(|v| units.degrees(v)).unwrap_or_default();
    let hue = match level {
        "cloud" => Some(olo_theme::Hue::Gray),
        _ => Some(olo_theme::Hue::Blue),
    };
    let tip = format!("{}: {value}, {}. {detail}. {}.", place.title(), olo_kit::words::mid_sentence(model::describe_said(s.code())), said.join("; "));
    Glance { icon: Some(model::icon(s.code(), f.is_night(t)).into()), value, title: place.title().to_owned(), detail, note: None, body: Some(Body::Slots { slots, hue }), tip, ..Glance::default() }
}

/// Next Days: `n` days from the one holding `t`, each its sky and its
/// low to its high.
fn days(f: &Forecast, place: &Place, units: &Units, t: i64, n: usize, local: &impl Fn(i64) -> i64) -> Glance {
    let first = day_of(f, t).unwrap_or(0);
    let spans: Vec<Span> = f
        .daily
        .iter()
        .skip(first)
        .take(n.clamp(1, 14))
        .enumerate()
        .filter_map(|(i, d)| {
            let (lo, hi) = (d.get(Layer::Temperature, Stat::Min)?, d.get(Layer::Temperature, Stat::Max)?);
            let (_, _, wd, _) = olo_kit::widgets::timeline::parts(local(d.time + 12 * 3600));
            let label = if i == 0 { "Today".to_owned() } else { olo_kit::widgets::timeline::WEEKDAYS[wd].to_owned() };
            let code = [Layer::WeatherCodeFullDay, Layer::WeatherCodeDay, Layer::WeatherCode].iter().map(|l| d.code(*l)).find(|c| *c != 0).unwrap_or(0);
            Some(Span { label, icon: Some(model::icon(code, false).into()), low: lo as f32, high: hi as f32, low_text: units.degrees(lo), high_text: units.degrees(hi) })
        })
        .collect();
    let min = spans.iter().map(|s| s.low).fold(f32::INFINITY, f32::min);
    let max = spans.iter().map(|s| s.high).fold(f32::NEG_INFINITY, f32::max);
    let (s, get) = at(f, t);
    let value = get(Layer::Temperature).map(|v| units.degrees(v)).unwrap_or_default();
    let today = f.daily.get(first).map(|d| [Layer::WeatherCodeFullDay, Layer::WeatherCodeDay, Layer::WeatherCode].iter().map(|l| d.code(*l)).find(|c| *c != 0).unwrap_or(0)).unwrap_or(s.code());
    let detail = match high_low(f, units, t) {
        Some(hl) => format!("{} · {hl}", model::describe_said(today)),
        None => model::describe_said(today).to_owned(),
    };
    let said: Vec<String> = spans.iter().map(|s| format!("{} {} to {}", s.label, s.low_text, s.high_text)).collect();
    let tip = format!("{}: {value} now. {detail}. {}.", place.title(), said.join("; "));
    let body = (!spans.is_empty()).then_some(Body::Spans { spans, min: if min.is_finite() { min } else { 0.0 }, max: if max.is_finite() { max } else { 1.0 } });
    Glance { icon: Some(model::icon(s.code(), f.is_night(t)).into()), value, title: place.title().to_owned(), detail, note: None, body, tip, ..Glance::default() }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use olo_protocol::deskbar::Size;

    use super::*;

    fn demo() -> (Forecast, Place) {
        let place = Place::demo();
        let f = provider::by_id("demo").fetch("", &place, &AtomicBool::new(false)).expect("the demo's forecast");
        (f, place)
    }

    fn ask(kind: &str, options: &[(&str, &str)]) -> Request {
        Request { widget: 1, kind: kind.into(), options: options.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(), size: Size::Medium }
    }

    /// Each kind's timeline: an entry now, then one each of its times, on
    /// the minute; each says what it shows in words.
    #[test]
    fn each_kind_goes_on_by_itself() {
        let (f, place) = demo();
        let now = f.fetched;
        let units = Units::default();
        for (kind, options, step) in [("now", vec![], 3600), ("hours", vec![("span", "3")], 1800), ("hours", vec![("span", "12"), ("level", "cloud")], 7200), ("days", vec![("days", "5")], 3600)] {
            let t = timeline(&ask(kind, &options), &f, &place, &units, now);
            assert!(t.entries.len() >= 6, "{kind}: {} entries", t.entries.len());
            assert!(t.entries.iter().all(|e| e.from % 60 == 0), "{kind}: on the minute");
            assert!(t.entries.windows(2).skip(1).all(|w| w[1].from - w[0].from == step), "{kind}: each {step} s");
            let g = t.at(now).unwrap();
            assert!(!g.value.is_empty() && g.icon.is_some() && !g.tip.is_empty(), "{kind}: {g:?}");
        }
        let hours = timeline(&ask("hours", &[]), &f, &place, &units, now);
        let Some(Body::Slots { slots, .. }) = &hours.entries[0].glance.body else { panic!("slots") };
        assert_eq!(slots.len(), SLOTS);
        assert_eq!(slots[0].label, "Now");
        assert!(slots.iter().all(|s| s.level.is_some_and(|l| (0.0..=1.0).contains(&l))), "chance of rain each: {slots:?}");
        let days = timeline(&ask("days", &[("days", "5")]), &f, &place, &units, now);
        let Some(Body::Spans { spans, min, max }) = &days.entries[0].glance.body else { panic!("spans") };
        assert_eq!(spans.len(), 5);
        assert_eq!(spans[0].label, "Today");
        assert!(spans.iter().all(|s| *min <= s.low && s.low <= s.high && s.high <= *max));
    }

    /// The times are the place's own: a half-hour zone's hours start on
    /// its half hours.
    #[test]
    fn boundaries_are_the_places_clock() {
        let india = |t: i64| t + 19_800;
        // 10:10 UTC is 15:40 there; its next hour, 16:00, is 10:30 UTC.
        let t = 10 * 3600 + 600;
        assert_eq!(next_boundary(t, 3600, &india), 10 * 3600 + 1800);
        assert_eq!(next_boundary(t, 1800, &|t| t), 10 * 3600 + 1800);
    }

    #[test]
    fn the_next_hour_said_short() {
        assert_eq!(short_sentence("Rain stopping in about 25 minutes"), "Rain stops in 25 min");
        assert_eq!(short_sentence("Snow starting in about 5 minutes"), "Snow starts in 5 min");
        assert_eq!(short_sentence("No precipitation in the next hour"), "Dry for the next hour");
    }

    /// No place or no forecast: a widget that says so.
    #[test]
    fn nothing_to_show_says_why() {
        let t = message("Add a place in Weather", 125);
        assert_eq!(t.entries[0].from, 120);
        assert_eq!(t.at(125).unwrap().detail, "Add a place in Weather");
    }
}
