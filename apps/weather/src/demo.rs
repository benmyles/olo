//! A made-up forecast, as tomorrow.io would answer it (so it goes through
//! the same reading as a real one): for tests, screenshots and agent
//! screens, with no key and no network. `OLO_WEATHER_DEMO=1` starts
//! Weather with it and a made-up place.
//!
//! The weather it tells, from the hour it's asked: rain easing off and
//! stopping within the hour; low cloud lifting through the afternoon;
//! a clear, cold night; fog the next morning, burning off by ten; a
//! fine day; a front with gusts and showers on the fourth day and a
//! thunderstorm in the evening; then clearing.

use std::sync::atomic::AtomicBool;

use serde_json::{Map, Value, json};

use crate::{
    model::Forecast,
    places::Place,
    provider::{Error, Provider, now},
};

pub struct Demo;

impl Provider for Demo {
    fn id(&self) -> &'static str {
        "demo"
    }

    fn name(&self) -> &'static str {
        "Demo (made up)"
    }

    fn needs_key(&self) -> bool {
        false
    }

    fn about(&self) -> &'static str {
        "A made-up forecast, for trying Weather out without a key."
    }

    fn fetch(&self, _: &str, place: &Place, _: &AtomicBool) -> Result<Forecast, Error> {
        let t = now();
        crate::tomorrow::parse(&raw(place, t), t).map(|f| Forecast { provider: "demo".into(), ..f })
    }

    fn parse(&self, raw: &Value, fetched: i64) -> Result<Forecast, Error> {
        crate::tomorrow::parse(raw, fetched).map(|f| Forecast { provider: "demo".into(), ..f })
    }
}

fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default()
}

/// Smooth 0 to 1 between `a` and `b`.
fn ramp(x: f64, a: f64, b: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A bump: 1 at `at`, falling away over `width` either side.
fn bump(x: f64, at: f64, width: f64) -> f64 {
    (-((x - at) / width).powi(2)).exp()
}

/// The values at `h` hours from when it's asked; `local` the place's hour
/// of day then.
fn values(h: f64, local: f64) -> Map<String, Value> {
    let hod = local.rem_euclid(24.0);
    // The sun's warmth through the day, the coldest at dawn.
    let diurnal = (((hod - 15.0) / 24.0) * std::f64::consts::TAU).cos();
    let front = bump(h, 76.0, 8.0);
    let base_temp = 9.0 + 2.5 * diurnal - 3.0 * bump(h, 30.0, 6.0) + 1.5 * ramp(h, 50.0, 70.0) - 2.0 * ramp(h, 80.0, 90.0);
    let rain_now = 0.8 * (1.0 - ramp(h, 0.0, 0.7));
    let showers = 2.5 * front * (0.6 + 0.4 * (h * 1.7).sin().abs());
    let storm = bump(h, 80.0, 1.5);
    let rain = (rain_now + showers + 4.0 * storm).max(0.0);
    let fog = bump(h, 28.0, 3.5) * (1.0 - ramp(hod, 9.0, 10.5));
    let low_cloud = 1.0 - ramp(h, 1.0, 8.0);
    let cover = (100.0 * (0.95 * low_cloud + 0.15 + 0.7 * front + 0.4 * fog)).clamp(0.0, 100.0) * if h > 9.0 && h < 24.0 { 0.25 } else { 1.0 };
    let base_km = if cover < 8.0 { Value::Null } else { json!((0.3 + 1.3 * ramp(h, 1.0, 8.0) - 0.25 * fog + 0.6 * (1.0 - front) * ramp(h, 40.0, 60.0)).max(0.05)) };
    let ceiling_km = if cover < 8.0 { Value::Null } else { json!(2.2 + 4.0 * front + 1.5 * ramp(h, 1.0, 8.0)) };
    let wind = 3.0 + 2.0 * (1.0 - ramp(h, 0.0, 6.0)) + 7.0 * front + 1.2 * diurnal.max(0.0);
    let gust = wind * (1.35 + 0.4 * front);
    let dir = 200.0 + 70.0 * ramp(h, 4.0, 20.0) - 110.0 * ramp(h, 70.0, 84.0);
    let humidity = (70.0 + 25.0 * low_cloud + 25.0 * fog - 15.0 * diurnal.max(0.0) + 15.0 * front).clamp(25.0, 100.0);
    let dew_spread = (6.0 * (1.0 - humidity / 100.0) * 4.0).max(0.1);
    let visibility = (16.0 - 15.6 * fog - 6.0 * (rain / 3.0).min(1.0)).max(0.2);
    let pressure = 1009.0 + 8.0 * ramp(h, 0.0, 30.0) - 14.0 * bump(h, 76.0, 14.0);
    let uv = if (7.0..17.0).contains(&hod) { (4.5 * (((hod - 12.0) / 10.0) * std::f64::consts::PI).cos() * (1.0 - cover / 140.0)).max(0.0) } else { 0.0 };
    let prob = (100.0 * ((rain / 1.2).min(1.0) * 0.9 + 0.5 * front)).clamp(0.0, 100.0);
    let code = if storm > 0.3 {
        8000
    } else if rain > 2.0 {
        4201
    } else if rain > 0.6 {
        4001
    } else if rain > 0.05 {
        4200
    } else if fog > 0.4 {
        2000
    } else if cover > 85.0 {
        1001
    } else if cover > 60.0 {
        1102
    } else if cover > 30.0 {
        1101
    } else if cover > 10.0 {
        1100
    } else {
        1000
    };
    let temp = base_temp;
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        m.insert(k.into(), v);
    };
    put("temperature", json!(temp));
    put("temperatureApparent", json!(temp - 0.25 * wind));
    put("dewPoint", json!(temp - dew_spread));
    put("humidity", json!(humidity.round()));
    put("windSpeed", json!(wind));
    put("windGust", json!(gust));
    put("windDirection", json!(dir.rem_euclid(360.0).round()));
    put("pressureSeaLevel", json!(pressure));
    put("pressureSurfaceLevel", json!(pressure - 6.0));
    put("altimeterSetting", json!(pressure + 0.4));
    put("cloudCover", json!(cover.round()));
    put("cloudBase", base_km);
    put("cloudCeiling", ceiling_km);
    put("visibility", json!(visibility));
    put("precipitationProbability", json!(prob.round()));
    put("rainIntensity", json!(rain));
    put("snowIntensity", json!(0.0));
    put("sleetIntensity", json!(0.0));
    put("freezingRainIntensity", json!(0.0));
    put("rainAccumulation", json!(rain));
    put("snowAccumulation", json!(0.0));
    put("snowAccumulationLwe", json!(0.0));
    put("sleetAccumulation", json!(0.0));
    put("sleetAccumulationLwe", json!(0.0));
    put("iceAccumulation", json!(0.0));
    put("iceAccumulationLwe", json!(0.0));
    put("snowDepth", json!(0.0));
    put("uvIndex", json!(uv.round()));
    put("uvHealthConcern", json!((uv / 3.0).floor().min(4.0)));
    put("evapotranspiration", json!(0.05 + 0.1 * diurnal.max(0.0)));
    put("weatherCode", json!(code));
    m
}

/// The extra layers the timelines give.
fn more(h: f64, local: f64) -> Map<String, Value> {
    let v = values(h, local);
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let mut m = Map::new();
    m.insert("precipitationType".into(), json!(if f("rainIntensity") > 0.01 { 1 } else { 0 }));
    m.insert("thunderstormProbability".into(), json!((60.0 * bump(h, 80.0, 2.5)).round()));
    m.insert("dewPointDepression".into(), json!(f("temperature") - f("dewPoint")));
    m.insert("ezHeatStressIndex".into(), json!(0));
    m.insert("weatherCodeFullDay".into(), json!(f("weatherCode") as u32));
    m.insert("weatherCodeDay".into(), json!(f("weatherCode") as u32 * 10));
    m.insert("weatherCodeNight".into(), json!(f("weatherCode") as u32 * 10 + 1));
    m.insert("moonPhase".into(), json!(((local / 24.0 / 3.7).floor() as i64 + 1).rem_euclid(8)));
    m.insert("gdd10To30".into(), json!((f("temperature") - 10.0).max(0.0) / 24.0));
    m
}

/// The forecast for `place` asked at `t`, as tomorrow.io answers.
pub fn raw(place: &Place, t: i64) -> Value {
    let offset = place.offset_at(t);
    let local_hours = |s: i64| (s + offset as i64) as f64 / 3600.0;
    let hour0 = t - t.rem_euclid(3600);
    let minute0 = t - t.rem_euclid(60);
    let at = |s: i64| (s - t) as f64 / 3600.0;
    let minutely: Vec<Value> = (0..60).map(|i| minute0 + i * 60).map(|s| json!({"time": iso(s), "values": values(at(s), local_hours(s))})).collect();
    let hourly: Vec<Value> = (0..120).map(|i| hour0 + i * 3600).map(|s| json!({"time": iso(s), "values": values(at(s), local_hours(s))})).collect();
    // Days from this morning's six o'clock, as tomorrow.io's start.
    let local_midnight = (t + offset as i64).div_euclid(86_400) * 86_400 - offset as i64;
    let daily: Vec<Value> = (0..7)
        .map(|d| {
            let start = local_midnight + d * 86_400 + 6 * 3600;
            let hours: Vec<Map<String, Value>> = (0..24).map(|i| start + i * 3600).map(|s| values(at(s), local_hours(s))).collect();
            let mut out = Map::new();
            for key in hours[0].keys() {
                let xs: Vec<f64> = hours.iter().filter_map(|h| h.get(key).and_then(Value::as_f64)).collect();
                if xs.is_empty() || key == "weatherCode" {
                    continue;
                }
                let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
                out.insert(format!("{key}Min"), json!(lo));
                out.insert(format!("{key}Max"), json!(hi));
                out.insert(format!("{key}Avg"), json!(xs.iter().sum::<f64>() / xs.len() as f64));
                if key.ends_with("Accumulation") || key.ends_with("Lwe") || key == "evapotranspiration" {
                    out.insert(format!("{key}Sum"), json!(xs.iter().sum::<f64>()));
                }
            }
            let codes: Vec<u32> = hours.iter().filter_map(|h| h.get("weatherCode").and_then(Value::as_u64)).map(|c| c as u32).collect();
            let worst = codes.iter().copied().max_by_key(|c| (c / 1000, *c)).unwrap_or(1000);
            out.insert("weatherCodeMax".into(), json!(worst));
            out.insert("weatherCodeMin".into(), json!(codes.iter().copied().min().unwrap_or(1000)));
            out.insert("sunriseTime".into(), json!(iso(local_midnight + d * 86_400 + 7 * 3600 + 12 * 60)));
            out.insert("sunsetTime".into(), json!(iso(local_midnight + d * 86_400 + 16 * 3600 + 48 * 60)));
            out.insert("moonriseTime".into(), json!(iso(local_midnight + d * 86_400 + 13 * 3600 + d * 50 * 60)));
            out.insert("moonsetTime".into(), json!(iso(local_midnight + d * 86_400 + 23 * 3600 + d * 50 * 60)));
            json!({"time": iso(start), "values": out})
        })
        .collect();
    let tz = |s: i64| {
        let local = s + offset as i64;
        let sign = if offset < 0 { '-' } else { '+' };
        let o = offset.abs();
        chrono::DateTime::from_timestamp(local, 0).map(|d| format!("{}{sign}{:02}:{:02}", d.format("%Y-%m-%dT%H:%M:%S"), o / 3600, (o % 3600) / 60)).unwrap_or_default()
    };
    let tl_hours: Vec<Value> = (0..121).map(|i| hour0 + i * 3600).map(|s| json!({"startTime": tz(s), "values": more(at(s), local_hours(s))})).collect();
    let tl_days: Vec<Value> = (0..7)
        .map(|d| {
            let start = local_midnight + d * 86_400 + 6 * 3600;
            let mut m = more(at(start + 6 * 3600), local_hours(start + 6 * 3600));
            let day_code = (0..12).map(|i| values(at(start + i * 3600), local_hours(start + i * 3600))["weatherCode"].as_u64().unwrap_or(1000)).max_by_key(|c| (c / 1000, *c)).unwrap_or(1000);
            let night_code = (12..24).map(|i| values(at(start + i * 3600), local_hours(start + i * 3600))["weatherCode"].as_u64().unwrap_or(1000)).max_by_key(|c| (c / 1000, *c)).unwrap_or(1000);
            m.insert("weatherCodeDay".into(), json!(day_code * 10));
            m.insert("weatherCodeNight".into(), json!(night_code * 10 + 1));
            m.insert("weatherCodeFullDay".into(), json!(day_code.max(night_code)));
            m.insert("thunderstormProbability".into(), json!((0..24).map(|i| (60.0 * bump(at(start + i * 3600), 80.0, 2.5)).round() as i64).max().unwrap_or(0)));
            json!({"startTime": tz(start), "values": m})
        })
        .collect();
    json!({
        "forecast": {
            "timelines": {"minutely": minutely, "hourly": hourly, "daily": daily},
            "location": {"lat": place.lat, "lon": place.lon}
        },
        "timelines": {"data": {"timelines": [
            {"timestep": "1h", "intervals": tl_hours},
            {"timestep": "1d", "intervals": tl_days}
        ]}}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layer, Stat};

    #[test]
    fn the_demo_reads_as_a_whole_forecast() {
        let place = Place::demo();
        let t = 1_794_711_600;
        let f = Demo.parse(&raw(&place, t), t).unwrap();
        assert_eq!(f.minutely.len(), 60);
        assert_eq!(f.hourly.len(), 120);
        assert_eq!(f.daily.len(), 7);
        let now = f.now(t).unwrap();
        // Raining now, stopped within the hour.
        assert!(now.get(Layer::RainIntensity).unwrap() > 0.3);
        assert!(f.minutely.last().unwrap().get(Layer::RainIntensity).unwrap() < 0.05);
        assert!(f.hourly[3].get(Layer::ThunderstormProbability).is_some());
        assert!(f.daily[0].get(Layer::Temperature, Stat::Max).unwrap() > f.daily[0].get(Layer::Temperature, Stat::Min).unwrap());
        assert!(f.daily[1].sunrise.is_some());
        assert!(f.daily[0].code(Layer::WeatherCodeDay) >= 10_000);
    }
}
