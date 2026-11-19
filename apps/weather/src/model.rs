//! A forecast as Weather keeps it, whichever provider gave it: every
//! layer by the hour (and the minute, for the next hour), and by the day
//! as its lowest, average, highest and total, in metric units (°C, m/s,
//! hPa, mm, km). The layers are tomorrow.io's core layers, by its names
//! (https://docs.tomorrow.io/reference/data-layers-core): another
//! provider gives what it has of them.

/// What kind of quantity a layer is: how it converts and reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    /// °C.
    Temperature,
    /// A difference of temperatures, °C.
    TemperatureSpread,
    Percent,
    /// m/s.
    Speed,
    /// Degrees clockwise from north, where it comes from.
    Direction,
    /// hPa.
    Pressure,
    /// mm/h.
    Rate,
    /// mm.
    Amount,
    /// cm (snow on the ground).
    Depth,
    /// km.
    Distance,
    /// km above the ground.
    Height,
    /// A number on a scale of its own (the UV index).
    Index,
    /// A code (weather, precipitation type, moon phase).
    Code,
    /// A count (growing degree days).
    Count,
}

/// One layer: its name in the data, what people call it, its quantity,
/// and what it means.
pub struct LayerInfo {
    pub key: &'static str,
    pub label: &'static str,
    pub quantity: Quantity,
    pub about: &'static str,
}

macro_rules! layers {
    ($($variant:ident = $key:literal, $label:literal, $q:ident, $about:literal;)*) => {
        /// Every layer, in the order they're listed.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(usize)]
        pub enum Layer { $($variant),* }
        pub const LAYERS: &[LayerInfo] = &[$(LayerInfo { key: $key, label: $label, quantity: Quantity::$q, about: $about }),*];
        impl Layer {
            pub const ALL: &[Layer] = &[$(Layer::$variant),*];
        }
    };
}

layers! {
    Temperature = "temperature", "Temperature", Temperature, "The air's temperature 2 m above the ground.";
    TemperatureApparent = "temperatureApparent", "Feels like", Temperature, "How warm or cold it feels, with the wind and humidity.";
    DewPoint = "dewPoint", "Dew point", Temperature, "The temperature the air must cool to for its water to condense.";
    DewPointDepression = "dewPointDepression", "Dew point spread", TemperatureSpread, "The temperature less the dew point: small, and fog, low cloud or icing are likely.";
    Humidity = "humidity", "Humidity", Percent, "How much water vapour the air holds, of what it could.";
    WindSpeed = "windSpeed", "Wind", Speed, "The wind's speed 10 m above the ground.";
    WindGust = "windGust", "Gusts", Speed, "The wind's brief peaks (under 20 s) 10 m above the ground.";
    WindDirection = "windDirection", "Wind from", Direction, "Where the wind comes from, clockwise from north.";
    PressureSeaLevel = "pressureSeaLevel", "Pressure", Pressure, "The air's pressure at mean sea level.";
    PressureSurfaceLevel = "pressureSurfaceLevel", "Surface pressure", Pressure, "The air's pressure at the ground.";
    AltimeterSetting = "altimeterSetting", "Altimeter setting", Pressure, "The pressure to set an altimeter to, to read the field's elevation.";
    CloudCover = "cloudCover", "Cloud cover", Percent, "How much of the sky the clouds cover.";
    CloudBase = "cloudBase", "Cloud base", Height, "The lowest visible cloud's height above the ground.";
    CloudCeiling = "cloudCeiling", "Cloud ceiling", Height, "The highest visible cloud's height above the ground.";
    Visibility = "visibility", "Visibility", Distance, "How far away things can be seen clearly.";
    PrecipitationProbability = "precipitationProbability", "Chance of precipitation", Percent, "The chance of at least 0.25 mm of rain (or its water in snow).";
    PrecipitationType = "precipitationType", "Precipitation type", Code, "What reaches the ground: rain, snow, freezing rain or sleet.";
    PrecipitationIntensity = "precipitationIntensity", "Precipitation", Rate, "How hard it rains or snows, of every kind, as water.";
    RainIntensity = "rainIntensity", "Rain", Rate, "How hard it rains.";
    SnowIntensity = "snowIntensity", "Snow", Rate, "How hard it snows.";
    SleetIntensity = "sleetIntensity", "Sleet", Rate, "How hard ice pellets fall.";
    FreezingRainIntensity = "freezingRainIntensity", "Freezing rain", Rate, "How hard freezing rain falls.";
    RainAccumulation = "rainAccumulation", "Rain in the hour", Amount, "The rain that falls in the hour.";
    SnowAccumulation = "snowAccumulation", "Snow in the hour", Amount, "The snow that falls in the hour.";
    SnowAccumulationLwe = "snowAccumulationLwe", "Snow in the hour, as water", Amount, "The water in the hour's snow.";
    SleetAccumulation = "sleetAccumulation", "Sleet in the hour", Amount, "The ice pellets that fall in the hour.";
    SleetAccumulationLwe = "sleetAccumulationLwe", "Sleet in the hour, as water", Amount, "The water in the hour's ice pellets.";
    IceAccumulation = "iceAccumulation", "Ice in the hour", Amount, "The ice freezing rain leaves in the hour.";
    IceAccumulationLwe = "iceAccumulationLwe", "Ice in the hour, as water", Amount, "The water in the hour's ice.";
    SnowDepth = "snowDepth", "Snow on the ground", Depth, "All the snow on the ground, new and old.";
    ThunderstormProbability = "thunderstormProbability", "Chance of thunderstorms", Percent, "The chance of lightning nearby in the hour.";
    UvIndex = "uvIndex", "UV index", Index, "How strong the sun's burning ultraviolet is: 0–2 low, 3–5 moderate, 6–7 high, 8–10 very high, 11+ extreme.";
    UvHealthConcern = "uvHealthConcern", "UV concern", Index, "How much protection the UV calls for, from low to extreme.";
    Evapotranspiration = "evapotranspiration", "Evapotranspiration", Amount, "The water the ground and plants give to the air.";
    EzHeatStressIndex = "ezHeatStressIndex", "Heat stress", Index, "How hard the heat is on the body: under 22 none, 22–24 mild, 24–26 moderate, 26–28 medium, 28–30 severe, over 30 extreme.";
    WeatherCode = "weatherCode", "Conditions", Code, "The weather most of all.";
    WeatherCodeDay = "weatherCodeDay", "Conditions by day", Code, "The weather most of all from sunrise to sunset.";
    WeatherCodeNight = "weatherCodeNight", "Conditions by night", Code, "The weather most of all from sunset to sunrise.";
    WeatherCodeFullDay = "weatherCodeFullDay", "Conditions all day", Code, "The weather most of all from sunrise to the next.";
    MoonPhase = "moonPhase", "Moon", Code, "The moon's phase.";
    Gdd10To30 = "gdd10To30", "Growing degree days (10–30 °C)", Count, "Warmth for maize and soybeans: degrees over 10 °C, up to 30.";
    Gdd10To31 = "gdd10To31", "Growing degree days (10–31 °C)", Count, "Warmth for sunflowers: degrees over 10 °C, up to 31.";
    Gdd08To30 = "gdd08To30", "Growing degree days (8–30 °C)", Count, "Warmth for green gram and sorghum: degrees over 8 °C, up to 30.";
    Gdd03To25 = "gdd03To25", "Growing degree days (3–25 °C)", Count, "Warmth for potatoes: degrees over 3 °C, up to 25.";
}

pub const COUNT: usize = Layer::ALL.len();

impl Layer {
    pub fn info(self) -> &'static LayerInfo {
        &LAYERS[self as usize]
    }

    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn by_key(key: &str) -> Option<Layer> {
        Layer::ALL.iter().copied().find(|l| l.key() == key)
    }
}

/// One time's values: an hour, a minute, or now. NaN: not given.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Seconds (Unix time) at its start.
    pub time: i64,
    pub v: [f32; COUNT],
}

impl Step {
    pub fn new(time: i64) -> Step {
        Step { time, v: [f32::NAN; COUNT] }
    }

    pub fn get(&self, l: Layer) -> Option<f64> {
        let v = self.v[l as usize];
        v.is_finite().then_some(v as f64)
    }

    pub fn set(&mut self, l: Layer, v: f64) {
        self.v[l as usize] = v as f32;
    }

    /// Its weather code (0 for none).
    pub fn code(&self) -> u32 {
        self.get(Layer::WeatherCode).map_or(0, |c| c as u32)
    }

    /// How hard it rains or snows, of every kind: the provider's, else the
    /// kinds' together.
    pub fn precipitation(&self) -> Option<f64> {
        self.get(Layer::PrecipitationIntensity).or_else(|| {
            let kinds = [Layer::RainIntensity, Layer::SnowIntensity, Layer::SleetIntensity, Layer::FreezingRainIntensity];
            let given: Vec<f64> = kinds.iter().filter_map(|l| self.get(*l)).collect();
            (!given.is_empty()).then(|| given.iter().sum())
        })
    }

    /// What's falling most, by the kinds' rates (or the provider's type).
    pub fn kind(&self) -> Kind {
        let rates = [(Kind::Rain, Layer::RainIntensity), (Kind::Snow, Layer::SnowIntensity), (Kind::Sleet, Layer::SleetIntensity), (Kind::FreezingRain, Layer::FreezingRainIntensity)];
        let most = rates.iter().filter_map(|(k, l)| self.get(*l).filter(|v| *v > 0.0).map(|v| (*k, v))).max_by(|a, b| a.1.total_cmp(&b.1));
        match most {
            Some((k, _)) => k,
            None => match self.get(Layer::PrecipitationType).map(|t| t as u32) {
                Some(1) => Kind::Rain,
                Some(2) => Kind::Snow,
                Some(3) => Kind::FreezingRain,
                Some(4) => Kind::Sleet,
                _ => Kind::None,
            },
        }
    }
}

/// What falls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None,
    Rain,
    Snow,
    Sleet,
    FreezingRain,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::None => "Nothing",
            Kind::Rain => "Rain",
            Kind::Snow => "Snow",
            Kind::Sleet => "Sleet",
            Kind::FreezingRain => "Freezing rain",
        }
    }

    /// Its name in the person's language (`name` is the English, for the
    /// assistant and the command line).
    pub fn said(self) -> &'static str {
        match self {
            Kind::None => olo_kit::words::tr("Nothing"),
            Kind::Rain => olo_kit::words::tr("Rain"),
            Kind::Snow => olo_kit::words::tr("Snow"),
            Kind::Sleet => olo_kit::words::tr("Sleet"),
            Kind::FreezingRain => olo_kit::words::tr("Freezing rain"),
        }
    }

    /// With how hard it comes (0 light, 1 steady, 2 heavy), said whole in
    /// the person's language: an adjective agrees with its noun and goes
    /// before or after it as the language has it.
    pub fn said_with(self, strength: u8) -> &'static str {
        use olo_kit::words::tr;
        match (self, strength) {
            (Kind::Snow, 2) => tr("Heavy snow"),
            (Kind::Snow, 1) => tr("Steady snow"),
            (Kind::Snow, _) => tr("Light snow"),
            (Kind::Sleet, 2) => tr("Heavy sleet"),
            (Kind::Sleet, 1) => tr("Steady sleet"),
            (Kind::Sleet, _) => tr("Light sleet"),
            (Kind::FreezingRain, 2) => tr("Heavy freezing rain"),
            (Kind::FreezingRain, 1) => tr("Steady freezing rain"),
            (Kind::FreezingRain, _) => tr("Light freezing rain"),
            (_, 2) => tr("Heavy rain"),
            (_, 1) => tr("Steady rain"),
            (_, _) => tr("Light rain"),
        }
    }
}

/// What a day's value is of its hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    Min = 0,
    Avg = 1,
    Max = 2,
    Sum = 3,
}

impl Stat {
    pub const ALL: [Stat; 4] = [Stat::Min, Stat::Avg, Stat::Max, Stat::Sum];

    /// Its suffix in tomorrow.io's daily names ("temperatureMax").
    pub fn suffix(self) -> &'static str {
        match self {
            Stat::Min => "Min",
            Stat::Avg => "Avg",
            Stat::Max => "Max",
            Stat::Sum => "Sum",
        }
    }
}

/// One day: each layer's lowest, average, highest and total (NaN: not
/// given), its codes, and the sun's and moon's times.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    /// Seconds at its start (the provider's: the morning, for tomorrow.io).
    pub time: i64,
    pub v: [[f32; 4]; COUNT],
    pub sunrise: Option<i64>,
    pub sunset: Option<i64>,
    pub moonrise: Option<i64>,
    pub moonset: Option<i64>,
}

impl Day {
    pub fn new(time: i64) -> Day {
        Day { time, v: [[f32::NAN; 4]; COUNT], sunrise: None, sunset: None, moonrise: None, moonset: None }
    }

    pub fn get(&self, l: Layer, s: Stat) -> Option<f64> {
        let v = self.v[l as usize][s as usize];
        v.is_finite().then_some(v as f64)
    }

    pub fn set(&mut self, l: Layer, s: Stat, v: f64) {
        self.v[l as usize][s as usize] = v as f32;
    }

    /// A code layer's value (they're the same in every stat).
    pub fn code(&self, l: Layer) -> u32 {
        Stat::ALL.iter().find_map(|s| self.get(l, *s)).map_or(0, |c| c as u32)
    }
}

/// A place's forecast.
#[derive(Clone, Debug)]
pub struct Forecast {
    /// The provider's id ("tomorrow.io").
    pub provider: String,
    /// When it was fetched (seconds).
    pub fetched: i64,
    /// The next hour, a step a minute (premium for some providers: may
    /// be empty).
    pub minutely: Vec<Step>,
    pub hourly: Vec<Step>,
    pub daily: Vec<Day>,
    /// The place's offset from UTC, as the provider said it (seconds),
    /// for a place without a named time zone.
    pub offset: Option<i32>,
    /// Everything the provider answered, as it answered it: what the
    /// assistant is given.
    pub raw: serde_json::Value,
}

impl Forecast {
    /// The weather now: the minute's, else the hour's.
    pub fn now(&self, t: i64) -> Option<&Step> {
        fn pick(steps: &[Step], len: i64, t: i64) -> Option<&Step> {
            steps.iter().rev().find(|s| s.time <= t && t < s.time + len).or_else(|| steps.first().filter(|s| s.time >= t))
        }
        pick(&self.minutely, 60, t).or_else(|| pick(&self.hourly, 3600, t))
    }

    /// The hour holding `t`.
    pub fn hour(&self, t: i64) -> Option<&Step> {
        self.hourly.iter().find(|s| s.time <= t && t < s.time + 3600)
    }

    /// The nights in it: each sunset to the next sunrise.
    pub fn nights(&self) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        // Before the first sunrise.
        if let Some(first) = self.daily.first().and_then(|d| d.sunrise) {
            out.push((first - 86_400 / 2, first));
        }
        for (i, d) in self.daily.iter().enumerate() {
            if let Some(set) = d.sunset {
                let rise = self.daily.get(i + 1).and_then(|n| n.sunrise).unwrap_or(set + 12 * 3600);
                out.push((set, rise));
            }
        }
        out
    }

    /// Whether `t` is at night.
    pub fn is_night(&self, t: i64) -> bool {
        self.nights().iter().any(|(a, z)| *a <= t && t < *z)
    }
}

/// tomorrow.io's weather codes (its `weatherCodeFullDay` table, the
/// widest; `weatherCode`'s are among them, and a day's or night's is
/// one of these with 0 or 1 after it).
const CODES: &[(u32, &str)] = &[
    // ("Clear sky", not "Clear": a button's Clear is another word in every
    // language.)
    (1000, "Clear sky"),
    (1100, "Mostly clear"),
    (1101, "Partly cloudy"),
    (1102, "Mostly cloudy"),
    (1001, "Cloudy"),
    (1103, "Partly cloudy and mostly clear"),
    (2100, "Light fog"),
    (2101, "Mostly clear and light fog"),
    (2102, "Partly cloudy and light fog"),
    (2103, "Mostly cloudy and light fog"),
    (2106, "Mostly clear and fog"),
    (2107, "Partly cloudy and fog"),
    (2108, "Mostly cloudy and fog"),
    (2000, "Fog"),
    (4204, "Partly cloudy and drizzle"),
    (4203, "Mostly clear and drizzle"),
    (4205, "Mostly cloudy and drizzle"),
    (4000, "Drizzle"),
    (4200, "Light rain"),
    (4213, "Mostly clear and light rain"),
    (4214, "Partly cloudy and light rain"),
    (4215, "Mostly cloudy and light rain"),
    (4209, "Mostly clear and rain"),
    (4208, "Partly cloudy and rain"),
    (4210, "Mostly cloudy and rain"),
    (4001, "Rain"),
    (4211, "Mostly clear and heavy rain"),
    (4202, "Partly cloudy and heavy rain"),
    (4212, "Mostly cloudy and heavy rain"),
    (4201, "Heavy rain"),
    (5115, "Mostly clear and flurries"),
    (5116, "Partly cloudy and flurries"),
    (5117, "Mostly cloudy and flurries"),
    (5001, "Flurries"),
    (5100, "Light snow"),
    (5102, "Mostly clear and light snow"),
    (5103, "Partly cloudy and light snow"),
    (5104, "Mostly cloudy and light snow"),
    (5122, "Drizzle and light snow"),
    (5105, "Mostly clear and snow"),
    (5106, "Partly cloudy and snow"),
    (5107, "Mostly cloudy and snow"),
    (5000, "Snow"),
    (5101, "Heavy snow"),
    (5119, "Mostly clear and heavy snow"),
    (5120, "Partly cloudy and heavy snow"),
    (5121, "Mostly cloudy and heavy snow"),
    (5110, "Drizzle and snow"),
    (5108, "Rain and snow"),
    (5114, "Snow and freezing rain"),
    (5112, "Snow and ice pellets"),
    (6000, "Freezing drizzle"),
    (6003, "Mostly clear and freezing drizzle"),
    (6002, "Partly cloudy and freezing drizzle"),
    (6004, "Mostly cloudy and freezing drizzle"),
    (6204, "Drizzle and freezing drizzle"),
    (6206, "Light rain and freezing drizzle"),
    (6205, "Mostly clear and light freezing rain"),
    (6203, "Partly cloudy and light freezing rain"),
    (6209, "Mostly cloudy and light freezing rain"),
    (6200, "Light freezing rain"),
    (6213, "Mostly clear and freezing rain"),
    (6214, "Partly cloudy and freezing rain"),
    (6215, "Mostly cloudy and freezing rain"),
    (6001, "Freezing rain"),
    (6212, "Drizzle and freezing rain"),
    (6220, "Light rain and freezing rain"),
    (6222, "Rain and freezing rain"),
    (6207, "Mostly clear and heavy freezing rain"),
    (6202, "Partly cloudy and heavy freezing rain"),
    (6208, "Mostly cloudy and heavy freezing rain"),
    (6201, "Heavy freezing rain"),
    (7110, "Mostly clear and light ice pellets"),
    (7111, "Partly cloudy and light ice pellets"),
    (7112, "Mostly cloudy and light ice pellets"),
    (7102, "Light ice pellets"),
    (7108, "Mostly clear and ice pellets"),
    (7107, "Partly cloudy and ice pellets"),
    (7109, "Mostly cloudy and ice pellets"),
    (7000, "Ice pellets"),
    (7105, "Drizzle and ice pellets"),
    (7106, "Freezing rain and ice pellets"),
    (7115, "Light rain and ice pellets"),
    (7117, "Rain and ice pellets"),
    (7103, "Freezing rain and heavy ice pellets"),
    (7113, "Mostly clear and heavy ice pellets"),
    (7114, "Partly cloudy and heavy ice pellets"),
    (7116, "Mostly cloudy and heavy ice pellets"),
    (7101, "Heavy ice pellets"),
    (8001, "Mostly clear and thunderstorm"),
    (8003, "Partly cloudy and thunderstorm"),
    (8002, "Mostly cloudy and thunderstorm"),
    (8000, "Thunderstorm"),
];

/// A day's or night's code (five digits) as the day's (four).
fn four(code: u32) -> u32 {
    if code >= 10_000 { code / 10 } else { code }
}

/// What a weather code says ("Partly cloudy and light rain").
pub fn describe(code: u32) -> &'static str {
    let c = four(code);
    CODES.iter().find(|(k, _)| *k == c).map_or("Unknown", |(_, d)| *d)
}

/// [`describe`] in the person's language (`describe` is the English, for
/// the assistant and Edit ▸ Copy).
pub fn describe_said(code: u32) -> &'static str {
    olo_kit::words::tr(describe(code))
}

/// The picture for a weather code, by day or by night.
pub fn icon(code: u32, night: bool) -> &'static str {
    let c = four(code);
    let sky = |day: &'static str, dark: &'static str| if night { dark } else { day };
    match c {
        1000 => sky("weather-clear-day", "weather-clear-night"),
        1100 => sky("weather-mostly-clear-day", "weather-mostly-clear-night"),
        1101 | 1103 => sky("weather-partly-cloudy-day", "weather-partly-cloudy-night"),
        1102 => "weather-mostly-cloudy",
        1001 => "weather-cloudy",
        2000..=2999 => "weather-fog",
        4000 | 4203..=4205 => "weather-drizzle",
        4201 | 4202 | 4211 | 4212 => "weather-heavy-rain",
        4000..=4999 => "weather-rain",
        5001 | 5115..=5117 | 5100 | 5102..=5104 => "weather-flurries",
        5101 | 5119..=5121 => "weather-heavy-snow",
        5108 | 5110 | 5112 | 5122 => "weather-sleet",
        5114 => "weather-freezing-rain",
        5000..=5999 => "weather-snow",
        6000..=6999 => "weather-freezing-rain",
        7000..=7999 => "weather-sleet",
        8000..=8999 => "weather-thunderstorm",
        _ => "weather-cloudy",
    }
}

/// The moon's phase by its code.
pub fn moon(code: u32) -> &'static str {
    ["New moon", "Waxing crescent", "First quarter", "Waxing gibbous", "Full moon", "Waning gibbous", "Third quarter", "Waning crescent"].get(code as usize).copied().unwrap_or("–")
}

/// The UV index's level.
pub fn uv_level(index: f64) -> &'static str {
    match index {
        i if i < 3.0 => "Low",
        i if i < 6.0 => "Moderate",
        i if i < 8.0 => "High",
        i if i < 11.0 => "Very high",
        _ => "Extreme",
    }
}

/// A compass point for a direction the wind comes from ("SW").
pub fn compass(degrees: f64) -> &'static str {
    const POINTS: [&str; 16] = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"];
    POINTS[(((degrees.rem_euclid(360.0)) / 22.5).round() as usize) % 16]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layer_is_found_by_its_name() {
        for l in Layer::ALL {
            assert_eq!(Layer::by_key(l.key()), Some(*l));
        }
        assert_eq!(Layer::ALL.len(), LAYERS.len());
    }

    #[test]
    fn codes_read_by_day_and_night() {
        assert_eq!(describe(4214), "Partly cloudy and light rain");
        assert_eq!(describe(42141), "Partly cloudy and light rain");
        assert_eq!(describe(10000), "Clear sky");
        assert_eq!(icon(1000, false), "weather-clear-day");
        assert_eq!(icon(10001, true), "weather-clear-night");
        assert_eq!(icon(4201, false), "weather-heavy-rain");
        assert_eq!(icon(4200, false), "weather-rain");
        assert_eq!(icon(2106, true), "weather-fog");
        assert_eq!(icon(8002, false), "weather-thunderstorm");
        assert_eq!(compass(225.0), "SW");
        assert_eq!(compass(359.0), "N");
    }

    #[test]
    fn what_falls_goes_by_the_heaviest_kind() {
        let mut s = Step::new(0);
        assert_eq!(s.kind(), Kind::None);
        s.set(Layer::RainIntensity, 0.4);
        s.set(Layer::SnowIntensity, 1.2);
        assert_eq!(s.kind(), Kind::Snow);
        assert!((s.precipitation().unwrap() - 1.6).abs() < 1e-6);
    }
}
