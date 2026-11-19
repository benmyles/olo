//! The places a person keeps (File ▸ Add Place…), and finding one: by its
//! name (Open-Meteo's geocoding: towns, airfields), by its street address
//! (Photon, komoot's OpenStreetMap geocoder, made for search as you
//! type), both free with no key and sent only what was typed, or by
//! coordinates typed as "47.61, -122.33". A place keeps its time zone, so
//! its hours read as its own clock wherever the computer is; one found
//! without (an address, coordinates) is given it, and its height, when
//! it's added (`locate`).

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
    /// The name the person gave it ("Home", "The drop zone"), shown for
    /// it everywhere (Ben, 2026-11-15: "in the weather app i should be
    /// able to set custom names for places i've added"); `name` is what
    /// it was found as, still shown beside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Place {
    /// The made-up place the demo forecasts.
    pub fn demo() -> Place {
        Place { name: "Sample Field".into(), region: "A made-up place".into(), lat: 45.0, lon: 7.0, timezone: Some("Europe/Rome".into()), elevation: Some(240.0), label: None }
    }

    /// What it's called: the person's own name for it, else the one it
    /// was found as.
    pub fn title(&self) -> &str {
        self.label.as_deref().filter(|l| !l.trim().is_empty()).unwrap_or(&self.name)
    }

    /// Whether `name` names it (its own name or the one it was found as,
    /// whatever their case): an agent's `place:NAME`, `--print NAME`.
    pub fn named(&self, name: &str) -> bool {
        self.title().eq_ignore_ascii_case(name) || self.name.eq_ignore_ascii_case(name)
    }

    /// What it was found as, with where, when it has a name of its own
    /// ("Seattle, Washington, United States"): shown beside that name.
    pub fn found_as(&self) -> Option<String> {
        self.label.as_ref().filter(|l| !l.trim().is_empty())?;
        Some(if self.region.is_empty() { self.name.clone() } else { format!("{}, {}", self.name, self.region) })
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
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then(|| Place {
        name: format!("{lat:.3}, {lon:.3}"),
        region: String::new(),
        lat,
        lon,
        timezone: None,
        elevation: None,
        label: None,
    })
}

/// The most places a search offers.
const MOST: usize = 12;

/// A service's address: `var` (tests point it at a stand-in), else `or`.
fn base(var: &str, or: &str) -> String {
    std::env::var(var).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| or.into())
}

/// Places called `query`, or at that address, the likeliest first (on a
/// worker): towns and addresses asked for at once, the addresses first
/// when what's typed has a number in it (a house's, a postcode).
pub fn search(query: &str) -> Result<Vec<Place>, Error> {
    let query = query.trim();
    if let Some(p) = coordinates(query) {
        return Ok(vec![p]);
    }
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let (towns, addresses) = std::thread::scope(|s| {
        let addresses = s.spawn(|| search_addresses(query));
        (search_towns(query), addresses.join().unwrap_or_else(|_| Ok(Vec::new())))
    });
    merge(query, towns, addresses)
}

/// Towns and addresses found for `query`, as one list: the addresses first
/// when it has a number in it, else the towns, with an address search's
/// own towns left out when the town search has one of that name nearby.
/// One search failing, the other's; both, the town search's error.
pub fn merge(query: &str, towns: Result<Vec<Place>, Error>, addresses: Result<Vec<Place>, Error>) -> Result<Vec<Place>, Error> {
    let (towns, addresses) = match (towns, addresses) {
        (Err(e), Err(_)) => return Err(e),
        (Err(e), Ok(a)) if a.is_empty() => return Err(e),
        (Ok(t), Err(e)) if t.is_empty() => return Err(e),
        (Ok(t), Err(_)) => (t, Vec::new()),
        (Err(_), Ok(a)) => (Vec::new(), a),
        (Ok(t), Ok(a)) => (t, a),
    };
    let addresses: Vec<Place> = addresses.into_iter().filter(|a| !towns.iter().any(|t| t.name.eq_ignore_ascii_case(&a.name) && (t.lat - a.lat).abs() < 0.1 && (t.lon - a.lon).abs() < 0.1)).collect();
    let (first, then) = if query.chars().any(|c| c.is_ascii_digit()) { (addresses, towns) } else { (towns, addresses) };
    let mut all: Vec<Place> = Vec::new();
    for p in first.into_iter().chain(then) {
        if !all.iter().any(|q| q.same(&p) && q.name == p.name) {
            all.push(p);
        }
    }
    all.truncate(MOST);
    Ok(all)
}

/// Places at a street address (or a street, a building, a field) like
/// `query`: Photon's.
pub fn search_addresses(query: &str) -> Result<Vec<Place>, Error> {
    let base = base("OLO_WEATHER_ADDRESS_URL", "https://photon.komoot.io");
    let reply = agent().get(format!("{base}/api/")).query("q", query).query("limit", "8").query("lang", "en").call();
    let mut reply = reply.map_err(|e| Error::Unreachable(olo_kit::tr!("Addresses can’t be looked up: {}", e)))?;
    if !reply.status().is_success() {
        return Err(Error::Answer(olo_kit::tr!("The address search answered {}.", reply.status().as_u16())));
    }
    let v: serde_json::Value = reply.body_mut().read_json().map_err(|e| Error::Answer(olo_kit::tr!("The address search’s answer can’t be read: {}", e)))?;
    Ok(parse_addresses(&v))
}

/// Where the house number goes before the street ("1600 Pennsylvania
/// Avenue"); elsewhere after ("Unter den Linden 77").
const NUMBER_FIRST: &[&str] = &["US", "CA", "GB", "IE", "AU", "NZ", "FR", "LU", "BE", "IN", "ZA", "SG", "MY", "PH", "IL", "TH", "LK", "HK"];

/// Photon's answer (GeoJSON) as places: a house by its address, the
/// building's own name (if any) and its town after; a street, a field or
/// a town by its name.
pub fn parse_addresses(v: &serde_json::Value) -> Vec<Place> {
    let Some(features) = v["features"].as_array() else { return Vec::new() };
    features
        .iter()
        .filter_map(|f| {
            let p = &f["properties"];
            let text = |k: &str| p[k].as_str().map(str::trim).filter(|s| !s.is_empty());
            let coords = f["geometry"]["coordinates"].as_array()?;
            let (lon, lat) = (coords.first()?.as_f64()?, coords.get(1)?.as_f64()?);
            let number_first = text("countrycode").is_some_and(|c| NUMBER_FIRST.iter().any(|n| n.eq_ignore_ascii_case(c)));
            let address = match (text("housenumber"), text("street")) {
                (Some(n), Some(s)) if number_first => Some(format!("{n} {s}")),
                (Some(n), Some(s)) => Some(format!("{s} {n}")),
                _ => None,
            };
            // (A building named for its address, "1600 Pennsylvania Ave SE
            // Apartments", says nothing the address doesn't.)
            let restates = |n: &str| text("housenumber").is_some_and(|h| n.starts_with(&format!("{h} ")) || n.ends_with(&format!(" {h}")));
            let (name, mut region) = match (address, text("name")) {
                (Some(a), Some(n)) if n != a && !restates(n) => (a, vec![n.to_owned()]),
                (Some(a), _) => (a, vec![]),
                (None, Some(n)) => (n.to_owned(), text("street").filter(|s| *s != n).map(|s| vec![s.to_owned()]).unwrap_or_default()),
                (None, None) => (text("street")?.to_owned(), vec![]),
            };
            for k in ["city", "state", "country"] {
                if let Some(t) = text(k)
                    && t != name
                    && !region.iter().any(|r| r == t)
                {
                    region.push(t.to_owned());
                }
            }
            ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then(|| Place { name, region: region.join(", "), lat, lon, timezone: None, elevation: None, label: None })
        })
        .collect()
}

/// A point's time zone and height above sea level (metres), from Open-Meteo
/// (its elevation model, 90 m across): for a place found without them.
pub fn locate(lat: f64, lon: f64) -> Result<(Option<String>, Option<f64>), Error> {
    let base = base("OLO_WEATHER_ZONE_URL", "https://api.open-meteo.com");
    let reply = agent().get(format!("{base}/v1/forecast")).query("latitude", format!("{lat:.5}")).query("longitude", format!("{lon:.5}")).query("timezone", "auto").call();
    let mut reply = reply.map_err(|e| Error::Unreachable(olo_kit::tr!("The place’s time zone can’t be looked up: {}", e)))?;
    if !reply.status().is_success() {
        return Err(Error::Answer(olo_kit::tr!("The time zone lookup answered {}.", reply.status().as_u16())));
    }
    let v: serde_json::Value = reply.body_mut().read_json().map_err(|e| Error::Answer(olo_kit::tr!("The time zone lookup’s answer can’t be read: {}", e)))?;
    Ok(parse_located(&v))
}

/// Open-Meteo's answer: a time zone it names that chrono-tz knows ("GMT"
/// out at sea), and the height.
pub fn parse_located(v: &serde_json::Value) -> (Option<String>, Option<f64>) {
    let zone = v["timezone"].as_str().filter(|z| z.parse::<chrono_tz::Tz>().is_ok()).map(str::to_owned);
    (zone, v["elevation"].as_f64().filter(|e| e.is_finite()))
}

/// Places by their name: Open-Meteo's.
fn search_towns(query: &str) -> Result<Vec<Place>, Error> {
    let base = base("OLO_WEATHER_GEOCODE_URL", "https://geocoding-api.open-meteo.com");
    let reply = agent().get(format!("{base}/v1/search")).query("name", query).query("count", "10").query("language", "en").query("format", "json").call();
    let mut reply = reply.map_err(|e| Error::Unreachable(olo_kit::tr!("Places can’t be looked up: {}", e)))?;
    if !reply.status().is_success() {
        return Err(Error::Answer(olo_kit::tr!("The place search answered {}.", reply.status().as_u16())));
    }
    let v: serde_json::Value = reply.body_mut().read_json().map_err(|e| Error::Answer(olo_kit::tr!("The place search’s answer can’t be read: {}", e)))?;
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
                        label: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A place's own name is what it's called, found by (as is the name it
    /// was found as), and kept only when it says something.
    #[test]
    fn a_place_goes_by_its_own_name() {
        let mut p = Place { name: "Seattle".into(), region: "Washington, United States".into(), ..Place::demo() };
        assert_eq!((p.title(), p.found_as()), ("Seattle", None));
        p.label = Some("Home".into());
        assert_eq!(p.title(), "Home");
        assert_eq!(p.found_as().as_deref(), Some("Seattle, Washington, United States"));
        assert!(p.named("home") && p.named("SEATTLE") && !p.named("Tacoma"));
        // Kept with it, and a place from before has none.
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Place>(&json).unwrap().title(), "Home");
        p.label = None;
        assert!(!serde_json::to_string(&p).unwrap().contains("label"));
        p.label = Some("  ".into());
        assert_eq!(p.title(), "Seattle", "blank: its found name");
    }

    /// A place moved: the one shown is still the one shown, wherever it
    /// went.
    #[test]
    fn the_shown_place_follows_a_move() {
        use crate::moved_index;
        // [a, b, c, d]: b (1) moved to 3 is [a, c, d, b].
        assert_eq!((moved_index(1, 1, 3), moved_index(2, 1, 3), moved_index(3, 1, 3), moved_index(0, 1, 3)), (3, 1, 2, 0));
        // d (3) moved to 0 is [d, a, b, c].
        assert_eq!((moved_index(3, 3, 0), moved_index(0, 3, 0), moved_index(2, 3, 0)), (0, 1, 3));
    }

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

    /// Photon's answer (as it came for "1600 pennsylvania ave", trimmed):
    /// a house by its address, its building's name after; the number
    /// where the country puts it; a street and a town by their names.
    #[test]
    fn addresses_are_read() {
        let v = serde_json::json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {"osm_key": "office", "type": "house", "housenumber": "1600", "name": "White House",
                "street": "Pennsylvania Avenue Northwest", "district": "Ward 2", "city": "Washington", "state": "District of Columbia",
                "country": "United States", "postcode": "20500", "countrycode": "US"},
             "geometry": {"type": "Point", "coordinates": [-77.0365, 38.8977]}},
            {"type": "Feature", "properties": {"type": "house", "housenumber": "1600", "name": "1600 Pennsylvania Ave SE Apartments",
                "street": "Pennsylvania Avenue Southeast", "city": "Washington", "state": "District of Columbia", "country": "United States",
                "countrycode": "US"}, "geometry": {"type": "Point", "coordinates": [-76.98195, 38.87923]}},
            {"type": "Feature", "properties": {"type": "house", "housenumber": "77", "street": "Unter den Linden", "city": "Berlin",
                "country": "Germany", "countrycode": "DE"}, "geometry": {"type": "Point", "coordinates": [13.3807, 52.5163]}},
            {"type": "Feature", "properties": {"type": "street", "name": "Pike Street", "city": "Seattle", "state": "Washington",
                "country": "United States", "countrycode": "US"}, "geometry": {"type": "Point", "coordinates": [-122.33, 47.61]}},
            {"type": "Feature", "properties": {"type": "city", "osm_key": "place", "name": "Seattle", "state": "Washington",
                "country": "United States"}, "geometry": {"type": "Point", "coordinates": [-122.33, 47.60]}},
            {"type": "Feature", "properties": {"name": "Nowhere"}, "geometry": {"type": "Point"}}
        ]});
        let ps = parse_addresses(&v);
        let read: Vec<(&str, &str)> = ps.iter().map(|p| (p.name.as_str(), p.region.as_str())).collect();
        assert_eq!(
            read,
            [
                ("1600 Pennsylvania Avenue Northwest", "White House, Washington, District of Columbia, United States"),
                ("1600 Pennsylvania Avenue Southeast", "Washington, District of Columbia, United States"),
                ("Unter den Linden 77", "Berlin, Germany"),
                ("Pike Street", "Seattle, Washington, United States"),
                ("Seattle", "Washington, United States"),
            ]
        );
        assert_eq!((ps[0].lat, ps[0].lon), (38.8977, -77.0365));
        assert!(ps.iter().all(|p| p.timezone.is_none() && p.elevation.is_none()), "looked up when added");
    }

    /// Towns first for a name, addresses first for what has a number; an
    /// address search's town of a name the towns have nearby left out; one
    /// search failing, the other's results, or its error when there are
    /// none.
    #[test]
    fn towns_and_addresses_are_one_list() {
        let at = |name: &str, lat: f64, lon: f64| Place { name: name.into(), region: String::new(), lat, lon, timezone: None, elevation: None, label: None };
        let towns = || Ok(vec![at("Seattle", 47.606, -122.332), at("Seattle Hill", 47.87, -122.15)]);
        let addresses = || Ok(vec![at("Seattle", 47.60, -122.33), at("Pike Street", 47.61, -122.34)]);
        let names = |r: Result<Vec<Place>, Error>| r.unwrap().into_iter().map(|p| p.name).collect::<Vec<_>>();
        assert_eq!(names(merge("seattle", towns(), addresses())), ["Seattle", "Seattle Hill", "Pike Street"]);
        assert_eq!(names(merge("1500 pike", towns(), addresses())), ["Pike Street", "Seattle", "Seattle Hill"]);
        let down = || Err(Error::Unreachable("down".into()));
        assert_eq!(names(merge("seattle", down(), addresses())), ["Seattle", "Pike Street"]);
        assert_eq!(names(merge("seattle", towns(), down())), ["Seattle", "Seattle Hill"]);
        assert!(merge("seattle", Ok(vec![]), down()).is_err(), "nothing, and a search failed: say why");
        assert!(merge("seattle", down(), down()).is_err());
        let many = (0..20).map(|i| at(&format!("Place {i}"), i as f64, 0.0)).collect();
        assert_eq!(merge("place", Ok(many), Ok(vec![])).unwrap().len(), MOST);
    }

    #[test]
    fn a_points_zone_and_height_are_read() {
        let v = serde_json::json!({"latitude": 38.89, "longitude": -77.03, "utc_offset_seconds": -18000, "timezone": "America/New_York", "elevation": 20.0});
        assert_eq!(parse_located(&v), (Some("America/New_York".into()), Some(20.0)));
        assert_eq!(parse_located(&serde_json::json!({"timezone": "GMT", "elevation": 0.0})).0.as_deref(), Some("GMT"), "out at sea");
        assert_eq!(parse_located(&serde_json::json!({"timezone": "Not/AZone"})), (None, None));
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
