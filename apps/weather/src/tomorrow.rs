//! tomorrow.io (https://docs.tomorrow.io), in Rust over HTTPS: no SDK.
//!
//! A fetch is two requests, both every core layer it gives (a free key's
//! reach: the next hour by the minute, 120 hours by the hour, 7 days by
//! the day; 5 days for the timelines):
//! - `GET /v4/weather/forecast`: the minutes, hours and days of every
//!   layer it has (temperatures, wind, pressure, clouds' cover, base and
//!   ceiling, visibility, each precipitation's rate and amount, snow
//!   depth, UV, …), each day's lowest, average, highest and total, the
//!   sun's and moon's times.
//! - `POST /v4/timelines`: the core layers the forecast leaves out
//!   (precipitation's type, the chance of thunderstorms, the dew point's
//!   spread, heat stress, the codes by day and night, the moon's phase,
//!   growing degree days), by the hour and the day, merged in.
//!
//! The key goes as the `apikey` query parameter, as tomorrow.io asks;
//! URLs are never logged. A free key has 25 requests an hour and 500 a
//! day: Weather fetches a place when it's shown and its forecast is half
//! an hour old.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::{
    model::{Day, Forecast, Layer, Stat, Step},
    places::Place,
    provider::{Error, Provider, agent, check, now},
};

pub struct TomorrowIo;

/// Where it answers (a fake server's, in tests).
fn base() -> String {
    std::env::var("OLO_WEATHER_TOMORROW_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "https://api.tomorrow.io".into())
}

/// The core layers the forecast leaves out, asked of the timelines.
const MORE: &[&str] = &[
    "precipitationType",
    "precipitationIntensity",
    "thunderstormProbability",
    "dewPointDepression",
    "ezHeatStressIndex",
    "weatherCodeDay",
    "weatherCodeNight",
    "weatherCodeFullDay",
    "moonPhase",
    "gdd10To30",
    "gdd10To31",
    "gdd08To30",
    "gdd03To25",
];

impl Provider for TomorrowIo {
    fn id(&self) -> &'static str {
        "tomorrow.io"
    }

    fn name(&self) -> &'static str {
        "tomorrow.io"
    }

    fn key_url(&self) -> &'static str {
        "https://app.tomorrow.io/development/keys"
    }

    fn about(&self) -> &'static str {
        "Every core layer by the minute for the next hour, by the hour for 5 days and by the day for 7: clouds’ base, ceiling and cover, visibility, each kind of precipitation and its chance, thunderstorms, wind and gusts, pressure, humidity, dew point, UV and more."
    }

    fn fetch(&self, key: &str, place: &Place, stop: &AtomicBool) -> Result<Forecast, Error> {
        let location = format!("{:.4},{:.4}", place.lat, place.lon);
        let a = agent();
        check(stop)?;
        let forecast = olo_net::across_changes(std::time::Duration::from_secs(20), || {
            let reply = a.get(format!("{}/v4/weather/forecast", base())).query("location", &location).query("units", "metric").query("apikey", key).call();
            answer(reply)
        })?;
        check(stop)?;
        // (The second's layers are a bonus: without them the forecast
        // still shows, and says what it lacks.)
        let body = json!({
            "location": location,
            "fields": MORE,
            "units": "metric",
            "timesteps": ["1h", "1d"],
            "startTime": "now",
            "endTime": "nowPlus5d",
            "timezone": "auto",
        });
        let timelines = olo_net::across_changes(std::time::Duration::from_secs(20), || {
            let reply = a.post(format!("{}/v4/timelines", base())).query("apikey", key).send_json(&body);
            answer(reply)
        });
        let timelines = match timelines {
            Ok(t) => t,
            Err(Error::Key(m)) => return Err(Error::Key(m)),
            Err(e) => {
                tracing::warn!("tomorrow.io's timelines: {e}");
                json!({ "error": e.to_string() })
            }
        };
        check(stop)?;
        let raw = json!({ "forecast": forecast, "timelines": timelines });
        self.parse(&raw, now())
    }

    fn parse(&self, raw: &Value, fetched: i64) -> Result<Forecast, Error> {
        parse(raw, fetched)
    }
}

/// A reply's JSON, or what went wrong said plainly.
fn answer(reply: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, Error> {
    let mut reply = reply.map_err(|e| Error::Unreachable(format!("tomorrow.io can’t be reached: {}", plain(&e))))?;
    let status = reply.status().as_u16();
    let retry = reply.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
    let text = reply.body_mut().read_to_string().map_err(|e| Error::Unreachable(format!("tomorrow.io’s answer was cut short: {}", plain(&e))))?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let said = v["message"].as_str().map(str::to_owned);
    match status {
        200..=299 if v.is_object() => Ok(v),
        200..=299 => Err(Error::Answer("tomorrow.io’s answer wasn’t a forecast.".into())),
        401 => Err(Error::Key("tomorrow.io didn’t accept the API key. Check it in File ▸ Data Provider…".into())),
        429 => Err(Error::Limit(format!("tomorrow.io’s limit for your key is reached for now{}", said.map(|s| format!(" ({})", s.trim_end_matches('.'))).unwrap_or_default()) + ".", retry)),
        403 => Err(Error::Answer(said.unwrap_or_else(|| "tomorrow.io refused the request.".into()))),
        _ => Err(Error::Answer(format!("tomorrow.io answered {status}{}", said.map(|s| format!(": {s}")).unwrap_or_default()))),
    }
}

/// An HTTP error said without its URL (which holds the key).
fn plain(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Io(io) => io.to_string(),
        ureq::Error::Timeout(_) => "it took too long".into(),
        ureq::Error::HostNotFound => "its address can’t be found (is the computer online?)".into(),
        ureq::Error::ConnectionFailed => "the connection failed".into(),
        ureq::Error::Tls(m) => format!("a secure connection failed ({m})"),
        other => {
            let s = other.to_string();
            if s.contains("apikey") { "the request failed".into() } else { s }
        }
    }
}

/// A time as tomorrow.io writes it ("2026-11-15T03:00:00Z", or with an
/// offset), in seconds.
fn time(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|t| t.timestamp())
}

/// An offset in a time tomorrow.io wrote with one, in seconds.
fn offset(v: &Value) -> Option<i32> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|t| t.offset().local_minus_utc())
}

fn step_of(item: &Value) -> Option<Step> {
    let mut s = Step::new(time(&item["time"]).or_else(|| time(&item["startTime"]))?);
    if let Some(values) = item["values"].as_object() {
        for (k, v) in values {
            if let (Some(l), Some(x)) = (Layer::by_key(k), v.as_f64()) {
                s.set(l, x);
            }
        }
    }
    Some(s)
}

fn day_of(item: &Value) -> Option<Day> {
    let mut d = Day::new(time(&item["time"]).or_else(|| time(&item["startTime"]))?);
    if let Some(values) = item["values"].as_object() {
        for (k, v) in values {
            match k.as_str() {
                "sunriseTime" => d.sunrise = time(v),
                "sunsetTime" => d.sunset = time(v),
                "moonriseTime" => d.moonrise = time(v),
                "moonsetTime" => d.moonset = time(v),
                _ => {
                    let Some(x) = v.as_f64() else { continue };
                    // "temperatureMax" is the layer and the stat; a code or
                    // phase without one is the same in each.
                    match Stat::ALL.iter().find_map(|s| k.strip_suffix(s.suffix()).and_then(Layer::by_key).map(|l| (l, *s))) {
                        Some((l, s)) => d.set(l, s, x),
                        None => {
                            if let Some(l) = Layer::by_key(k) {
                                for s in Stat::ALL {
                                    if d.get(l, s).is_none() {
                                        d.set(l, s, x);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Some(d)
}

/// What both requests answered (`{"forecast": …, "timelines": …}`) as
/// Weather's forecast.
pub fn parse(raw: &Value, fetched: i64) -> Result<Forecast, Error> {
    let f = &raw["forecast"]["timelines"];
    if !f.is_object() {
        return Err(Error::Answer("tomorrow.io’s answer had no forecast in it.".into()));
    }
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let minutely: Vec<Step> = list(&f["minutely"]).iter().filter_map(step_of).collect();
    let mut hourly: Vec<Step> = list(&f["hourly"]).iter().filter_map(step_of).collect();
    let mut daily: Vec<Day> = list(&f["daily"]).iter().filter_map(day_of).collect();
    let mut offset_seen = None;
    // The timelines' layers, merged into the hour and the day they're of.
    for tl in list(&raw["timelines"]["data"]["timelines"]) {
        let step = tl["timestep"].as_str().unwrap_or("");
        for item in list(&tl["intervals"]) {
            offset_seen = offset_seen.or_else(|| offset(&item["startTime"]));
            match step {
                "1h" => {
                    let Some(more) = step_of(&item) else { continue };
                    if let Some(h) = hourly.iter_mut().find(|h| h.time == more.time) {
                        for l in Layer::ALL {
                            if let Some(v) = more.get(*l)
                                && h.get(*l).is_none()
                            {
                                h.set(*l, v);
                            }
                        }
                    }
                }
                "1d" => {
                    let Some(more) = step_of(&item) else { continue };
                    // The day it's of: the one starting within half a day.
                    if let Some(d) = daily.iter_mut().find(|d| (d.time - more.time).abs() < 12 * 3600) {
                        for l in Layer::ALL {
                            if let Some(v) = more.get(*l) {
                                for s in Stat::ALL {
                                    if d.get(*l, s).is_none() && s != Stat::Sum {
                                        d.set(*l, s, v);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Precipitation of every kind, where only the kinds were given.
    for h in hourly.iter_mut() {
        if h.get(Layer::PrecipitationIntensity).is_none()
            && let Some(p) = h.precipitation()
        {
            h.set(Layer::PrecipitationIntensity, p);
        }
    }
    hourly.sort_by_key(|h| h.time);
    daily.sort_by_key(|d| d.time);
    Ok(Forecast { provider: "tomorrow.io".into(), fetched, minutely, hourly, daily, offset: offset_seen, raw: raw.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forecast as tomorrow.io answers, cut short.
    fn sample() -> Value {
        json!({
            "forecast": {
                "timelines": {
                    "minutely": [
                        {"time": "2026-11-15T03:40:00Z", "values": {"temperature": 8.2, "rainIntensity": 0.6, "cloudBase": 0.42}},
                        {"time": "2026-11-15T03:41:00Z", "values": {"temperature": 8.2, "rainIntensity": 0.5}}
                    ],
                    "hourly": [
                        {"time": "2026-11-15T04:00:00Z", "values": {"temperature": 8.0, "cloudBase": 0.4, "cloudCeiling": 2.1, "cloudCover": 100, "rainIntensity": 0.4, "snowIntensity": 0, "weatherCode": 4200}},
                        {"time": "2026-11-15T03:00:00Z", "values": {"temperature": 8.4, "cloudBase": null, "visibility": 9.5}}
                    ],
                    "daily": [
                        {"time": "2026-11-14T14:00:00Z", "values": {"temperatureMax": 11.5, "temperatureMin": 6.1, "rainAccumulationSum": 3.2, "sunriseTime": "2026-11-14T15:13:00Z", "sunsetTime": "2026-11-15T00:34:00Z", "moonriseTime": "2026-11-14T18:00:00Z"}}
                    ]
                }
            },
            "timelines": {
                "data": {"timelines": [
                    {"timestep": "1h", "intervals": [
                        {"startTime": "2026-11-14T20:00:00-08:00", "values": {"precipitationType": 1, "thunderstormProbability": 5, "temperature": 99}}
                    ]},
                    {"timestep": "1d", "intervals": [
                        {"startTime": "2026-11-14T06:00:00-08:00", "values": {"moonPhase": 1, "weatherCodeDay": 10010, "weatherCodeNight": 42051}}
                    ]}
                ]}
            }
        })
    }

    #[test]
    fn a_forecast_is_read_with_the_timelines_merged_in() {
        let f = parse(&sample(), 100).unwrap();
        assert_eq!(f.minutely.len(), 2);
        assert_eq!(f.hourly.len(), 2);
        // Sorted, and null is no value.
        assert_eq!(f.hourly[0].time, 1_794_711_600);
        assert_eq!(f.hourly[0].get(Layer::CloudBase), None);
        let h = &f.hourly[1];
        assert_eq!(h.get(Layer::CloudBase), Some(0.4000000059604645));
        // The timelines' hour (20:00 at -08:00 is 04:00Z): its layers
        // added, the forecast's own kept.
        assert_eq!(h.get(Layer::PrecipitationType), Some(1.0));
        assert_eq!(h.get(Layer::ThunderstormProbability), Some(5.0));
        assert_eq!(h.get(Layer::Temperature), Some(8.0));
        assert!((h.get(Layer::PrecipitationIntensity).unwrap() - 0.4).abs() < 1e-6);
        let d = &f.daily[0];
        assert_eq!(d.get(Layer::Temperature, Stat::Max), Some(11.5));
        assert!((d.get(Layer::RainAccumulation, Stat::Sum).unwrap() - 3.2).abs() < 1e-6);
        assert_eq!(d.code(Layer::MoonPhase), 1);
        assert_eq!(d.code(Layer::WeatherCodeNight), 42051);
        assert_eq!(d.sunset, Some(1_794_702_840));
        assert_eq!(f.offset, Some(-8 * 3600));
    }

    #[test]
    fn what_isnt_a_forecast_is_said_so() {
        assert!(matches!(parse(&json!({"forecast": {"code": 401001}}), 0), Err(Error::Answer(_))));
    }
}
