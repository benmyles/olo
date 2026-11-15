//! The places a person keeps (File ▸ Add Place…), and finding one by its
//! name: Open-Meteo's geocoding (free, no key; only what was typed is
//! sent), or coordinates typed as "47.61, -122.33". A place keeps its
//! time zone, so its hours read as its own clock wherever the computer is.

use chrono::{Offset as _, TimeZone as _};
use serde::{Deserialize, Serialize};

use crate::provider::{Error, agent};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// Its name ("Seattle").
    pub name: String,
    /// Where it is, to tell it apart ("Washington, United States").
    #[serde(default)]
    pub region: String,
    pub lat: f64,
    pub lon: f64,
    /// Its time zone's name ("America/Los_Angeles"), if known.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Its height above sea level, metres.
    #[serde(default)]
    pub elevation: Option<f64>,
}

impl Place {
    /// The made-up place the demo forecasts.
    pub fn demo() -> Place {
        Place { name: "Sample Field".into(), region: "A made-up place".into(), lat: 45.0, lon: 7.0, timezone: Some("Europe/Rome".into()), elevation: Some(240.0) }
    }

    /// Its offset from UTC at time `t` (seconds): its time zone's, else
    /// `fallback` (the provider's), else none.
    pub fn offset_at_or(&self, t: i64, fallback: Option<i32>) -> i32 {
        self.timezone
            .as_deref()
            .and_then(|z| z.parse::<chrono_tz::Tz>().ok())
            .and_then(|tz| chrono::DateTime::from_timestamp(t, 0).map(|d| tz.offset_from_utc_datetime(&d.naive_utc()).fix().local_minus_utc()))
            .or(fallback)
            .unwrap_or(0)
    }

    pub fn offset_at(&self, t: i64) -> i32 {
        self.offset_at_or(t, None)
    }

    /// Its coordinates as people write them ("47.61° N, 122.33° W").
    pub fn coordinates(&self) -> String {
        let ns = if self.lat >= 0.0 { "N" } else { "S" };
        let ew = if self.lon >= 0.0 { "E" } else { "W" };
        format!("{:.2}° {ns}, {:.2}° {ew}", self.lat.abs(), self.lon.abs())
    }

    /// Whether it's the same place as `o` (within ~100 m).
    pub fn same(&self, o: &Place) -> bool {
        (self.lat - o.lat).abs() < 0.001 && (self.lon - o.lon).abs() < 0.001
    }
}

/// Coordinates typed as a place: "47.61, -122.33", "47.61 -122.33".
pub fn coordinates(text: &str) -> Option<Place> {
    let parts: Vec<f64> = text.split([',', ' ', ';']).filter(|s| !s.is_empty()).map(|s| s.trim().parse::<f64>()).collect::<Result<_, _>>().ok()?;
    let [lat, lon] = parts[..] else { return None };
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then(|| Place { name: format!("{lat:.3}, {lon:.3}"), region: String::new(), lat, lon, timezone: None, elevation: None })
}

/// Places called `query`, the likeliest first (on a worker).
pub fn search(query: &str) -> Result<Vec<Place>, Error> {
    let query = query.trim();
    if let Some(p) = coordinates(query) {
        return Ok(vec![p]);
    }
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let base = std::env::var("OLO_WEATHER_GEOCODE_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "https://geocoding-api.open-meteo.com".into());
    let reply = agent().get(format!("{base}/v1/search")).query("name", query).query("count", "10").query("language", "en").query("format", "json").call();
    let mut reply = reply.map_err(|e| Error::Unreachable(format!("Places can’t be looked up: {e}")))?;
    if !reply.status().is_success() {
        return Err(Error::Answer(format!("The place search answered {}.", reply.status().as_u16())));
    }
    let v: serde_json::Value = reply.body_mut().read_json().map_err(|e| Error::Answer(format!("The place search’s answer can’t be read: {e}")))?;
    Ok(parse_search(&v))
}

/// Open-Meteo's answer as places.
pub fn parse_search(v: &serde_json::Value) -> Vec<Place> {
    v["results"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .filter_map(|r| {
                    let region: Vec<&str> = [r["admin1"].as_str(), r["country"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
                    Some(Place {
                        name: r["name"].as_str()?.to_owned(),
                        region: region.join(", "),
                        lat: r["latitude"].as_f64()?,
                        lon: r["longitude"].as_f64()?,
                        timezone: r["timezone"].as_str().map(str::to_owned),
                        elevation: r["elevation"].as_f64(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_a_place() {
        let p = coordinates("47.61, -122.33").unwrap();
        assert_eq!((p.lat, p.lon), (47.61, -122.33));
        assert!(coordinates("47.61 -122.33").is_some());
        assert!(coordinates("Seattle").is_none());
        assert!(coordinates("95, 10").is_none());
        assert_eq!(p.coordinates(), "47.61° N, 122.33° W");
    }

    #[test]
    fn a_place_reads_its_own_clock() {
        let p = Place { timezone: Some("America/Los_Angeles".into()), ..Place::demo() };
        // 2026-11-15T03:00Z: standard time, -8 h; in July, -7 h.
        assert_eq!(p.offset_at(1_794_711_600), -8 * 3600);
        assert_eq!(p.offset_at(1_783_000_000), -7 * 3600);
        let none = Place { timezone: None, ..Place::demo() };
        assert_eq!(none.offset_at_or(0, Some(3600)), 3600);
    }

    #[test]
    fn search_answers_are_read() {
        let v = serde_json::json!({"results": [
            {"name": "Seattle", "latitude": 47.60621, "longitude": -122.33207, "elevation": 56.0, "timezone": "America/Los_Angeles", "country": "United States", "admin1": "Washington"},
            {"name": "Nowhere"}
        ]});
        let ps = parse_search(&v);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].region, "Washington, United States");
        assert_eq!(ps[0].timezone.as_deref(), Some("America/Los_Angeles"));
    }
}
