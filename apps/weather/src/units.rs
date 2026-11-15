//! The units a person reads, each chosen on its own (View ▸ Units): a
//! skydiver in Europe reads the clouds in feet and the temperature in °C.
//! Forecasts are kept metric; each value is converted as it's shown.

use serde::{Deserialize, Serialize};

use crate::model::Quantity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Temperature {
    Celsius,
    Fahrenheit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speed {
    Kmh,
    Mph,
    Ms,
    Knots,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Height {
    Metres,
    Feet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Distance {
    Km,
    Miles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precipitation {
    Mm,
    Inches,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pressure {
    Hpa,
    InHg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Units {
    pub temperature: Temperature,
    pub speed: Speed,
    pub height: Height,
    pub distance: Distance,
    pub precipitation: Precipitation,
    pub pressure: Pressure,
}

impl Default for Units {
    /// The computer's: the United States' (and Liberia's and Myanmar's)
    /// by its locale, else metric.
    fn default() -> Self {
        let lang = ["LC_MEASUREMENT", "LC_ALL", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default();
        if ["_US", "_LR", "_MM"].iter().any(|c| lang.contains(c)) { Units::IMPERIAL } else { Units::METRIC }
    }
}

impl Units {
    pub const METRIC: Units = Units { temperature: Temperature::Celsius, speed: Speed::Kmh, height: Height::Metres, distance: Distance::Km, precipitation: Precipitation::Mm, pressure: Pressure::Hpa };
    pub const IMPERIAL: Units =
        Units { temperature: Temperature::Fahrenheit, speed: Speed::Mph, height: Height::Feet, distance: Distance::Miles, precipitation: Precipitation::Inches, pressure: Pressure::InHg };

    /// A metric value as shown.
    pub fn convert(&self, q: Quantity, v: f64) -> f64 {
        match q {
            Quantity::Temperature => match self.temperature {
                Temperature::Celsius => v,
                Temperature::Fahrenheit => v * 9.0 / 5.0 + 32.0,
            },
            Quantity::TemperatureSpread => match self.temperature {
                Temperature::Celsius => v,
                Temperature::Fahrenheit => v * 9.0 / 5.0,
            },
            Quantity::Speed => match self.speed {
                Speed::Kmh => v * 3.6,
                Speed::Mph => v * 2.236_936,
                Speed::Ms => v,
                Speed::Knots => v * 1.943_844,
            },
            Quantity::Height => match self.height {
                Height::Metres => v * 1000.0,
                Height::Feet => v * 3280.84,
            },
            Quantity::Distance => match self.distance {
                Distance::Km => v,
                Distance::Miles => v * 0.621_371,
            },
            Quantity::Rate | Quantity::Amount => match self.precipitation {
                Precipitation::Mm => v,
                Precipitation::Inches => v / 25.4,
            },
            Quantity::Depth => match self.precipitation {
                Precipitation::Mm => v,
                Precipitation::Inches => v / 2.54,
            },
            Quantity::Pressure => match self.pressure {
                Pressure::Hpa => v,
                Pressure::InHg => v * 0.029_53,
            },
            _ => v,
        }
    }

    /// Back to metric (a scale's tick shown in the person's units).
    pub fn unconvert(&self, q: Quantity, shown: f64) -> f64 {
        let one = self.convert(q, 1.0) - self.convert(q, 0.0);
        let zero = self.convert(q, 0.0);
        if one.abs() < 1e-12 { shown } else { (shown - zero) / one }
    }

    /// Its unit's symbol.
    pub fn unit(&self, q: Quantity) -> &'static str {
        match q {
            Quantity::Temperature | Quantity::TemperatureSpread => match self.temperature {
                Temperature::Celsius => "°C",
                Temperature::Fahrenheit => "°F",
            },
            Quantity::Percent => "%",
            Quantity::Speed => match self.speed {
                Speed::Kmh => "km/h",
                Speed::Mph => "mph",
                Speed::Ms => "m/s",
                Speed::Knots => "kt",
            },
            Quantity::Direction => "°",
            Quantity::Pressure => match self.pressure {
                Pressure::Hpa => "hPa",
                Pressure::InHg => "inHg",
            },
            Quantity::Rate => match self.precipitation {
                Precipitation::Mm => "mm/h",
                Precipitation::Inches => "in/h",
            },
            Quantity::Amount => match self.precipitation {
                Precipitation::Mm => "mm",
                Precipitation::Inches => "in",
            },
            Quantity::Depth => match self.precipitation {
                Precipitation::Mm => "cm",
                Precipitation::Inches => "in",
            },
            Quantity::Distance => match self.distance {
                Distance::Km => "km",
                Distance::Miles => "mi",
            },
            Quantity::Height => match self.height {
                Height::Metres => "m",
                Height::Feet => "ft",
            },
            Quantity::Index | Quantity::Code | Quantity::Count => "",
        }
    }

    /// How many decimals a value shows.
    fn decimals(&self, q: Quantity, shown: f64) -> usize {
        match q {
            Quantity::Rate | Quantity::Amount => match self.precipitation {
                Precipitation::Mm => usize::from(shown.abs() < 10.0),
                Precipitation::Inches => 2,
            },
            Quantity::Pressure => match self.pressure {
                Pressure::Hpa => 0,
                Pressure::InHg => 2,
            },
            Quantity::Distance if shown.abs() < 10.0 => 1,
            Quantity::Speed if self.speed == Speed::Ms => 1,
            Quantity::Index | Quantity::Count | Quantity::TemperatureSpread | Quantity::Depth => usize::from(shown.fract().abs() > 0.05 && shown.abs() < 10.0),
            _ => 0,
        }
    }

    /// The number alone, in the person's units ("12", "0.4").
    pub fn number(&self, q: Quantity, v: f64) -> String {
        let shown = self.convert(q, v);
        // Heights to tens of feet or metres: what a forecast can tell.
        let shown = if q == Quantity::Height { (shown / 10.0).round() * 10.0 } else { shown };
        let d = self.decimals(q, shown);
        let text = format!("{shown:.d$}");
        let text = if q == Quantity::Height && shown.abs() >= 1000.0 { thousands(shown.round() as i64) } else { text };
        // (No "-0".)
        if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { text.trim_start_matches('-').to_owned() } else { text }
    }

    /// A value with its unit ("12 °C", "1,200 ft", "40%").
    pub fn say(&self, q: Quantity, v: f64) -> String {
        let n = self.number(q, v);
        match q {
            Quantity::Percent => format!("{n}%"),
            Quantity::Temperature | Quantity::TemperatureSpread => format!("{n}{}", self.unit(q)),
            Quantity::Direction => format!("{n}°"),
            _ if self.unit(q).is_empty() => n,
            _ => format!("{n} {}", self.unit(q)),
        }
    }

    /// A temperature as a big figure reads it ("12°").
    pub fn degrees(&self, v: f64) -> String {
        format!("{}°", self.number(Quantity::Temperature, v))
    }
}

/// 12,345.
fn thousands(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_in_the_persons_units() {
        let m = Units::METRIC;
        assert_eq!(m.say(Quantity::Temperature, 12.4), "12°C");
        assert_eq!(m.say(Quantity::Height, 1.234), "1,230 m");
        assert_eq!(m.say(Quantity::Speed, 5.0), "18 km/h");
        assert_eq!(m.say(Quantity::Rate, 0.42), "0.4 mm/h");
        assert_eq!(m.say(Quantity::Percent, 40.0), "40%");
        assert_eq!(m.say(Quantity::Temperature, -0.2), "0°C");
        let i = Units::IMPERIAL;
        assert_eq!(i.say(Quantity::Temperature, 20.0), "68°F");
        assert_eq!(i.say(Quantity::Height, 0.3048), "1,000 ft");
        assert_eq!(i.say(Quantity::Pressure, 1013.25), "29.92 inHg");
        assert_eq!(i.say(Quantity::Distance, 16.0), "9.9 mi");
        let feet = Units { height: Height::Feet, ..Units::METRIC };
        assert_eq!(feet.say(Quantity::Height, 0.5), "1,640 ft");
        assert!((i.unconvert(Quantity::Temperature, 68.0) - 20.0).abs() < 1e-9);
    }
}
