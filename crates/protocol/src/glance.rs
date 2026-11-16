//! Glances: live widgets an app gives Deskbar (Ben, 2026-11-15: "the
//! weather app should make available a deskbar widget that is
//! customizable ... any new support/functionality you need try to add to
//! the kit in a way that can be reusable for future apps that want to
//! provide rich olo widgets for the deskbar"). BeOS let an app put a live
//! view of its own in the Deskbar's tray (a replicant); here the app
//! gives what to show and Olo draws it, so every app's looks like Olo's,
//! agents read it as it is, and the app needn't be running to keep it
//! going.
//!
//! - Which apps give which ([`PROVIDERS`]: each a [`Kind`] with its
//!   [`Opt`]ions, chosen in Preferences ▸ Deskbar, kept in the widget as
//!   [`crate::deskbar::Measure::App`], `glance` the kind's id).
//! - What one shows: a [`Timeline`], each [`Entry`] a [`Glance`] from a
//!   time on (minutes, so Deskbar's clock switches them as it ticks):
//!   the next hours' forecast goes on from hour to hour with nothing run.
//!   A glance is a headline (an icon, a value, a title, a detail), a note
//!   under it, and a [`Body`]: [`Slot`]s across (times, each an icon, a
//!   value and a level) or [`Span`]s down (days, each a low to a high on
//!   one scale). The kit draws it (`olo_kit::glance`), the same anywhere.
//! - How: Deskbar writes what it wants of each app ([`Requests`], one
//!   file an app, in [`dir`]) and runs its provider ([`Provider::program`]
//!   with [`Provider::arg`]), which writes each widget's timeline there
//!   ([`content_path`]) as it changes; Deskbar follows the files.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::deskbar::Size;

/// An app that gives Deskbar widgets.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    /// Its name in widgets' settings ("weather").
    pub app: &'static str,
    /// Its name for people ("Weather").
    pub name: &'static str,
    /// Its program, and the argument that runs it as a provider (no
    /// window).
    pub program: &'static str,
    pub arg: &'static str,
    pub icon: &'static str,
    pub kinds: &'static [Kind],
}

/// A widget an app gives.
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    /// Its name in widgets' settings ("hours").
    pub id: &'static str,
    /// Its name in Preferences' menu of what a widget shows ("Weather:
    /// Next Hours").
    pub menu: &'static str,
    /// What it shows, a line.
    pub about: &'static str,
    pub options: &'static [Opt],
}

/// One of a kind's options: a choice among values (the first, the one it
/// comes with).
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub id: &'static str,
    /// Its label in Preferences ("Hours:").
    pub label: &'static str,
    /// Each value and its name.
    pub values: &'static [(&'static str, &'static str)],
}

/// Every app that gives widgets. (An app adds its own here; what it
/// gives is its own to draw from: `Timeline`.)
pub const PROVIDERS: &[Provider] = &[
    Provider {
        app: "weather",
        name: "Weather",
        program: "olo-weather",
        arg: "--glance",
        icon: "weather",
        kinds: &[
            Kind { id: "now", menu: "Weather: Now", about: "The sky and temperature now, and what's coming.", options: &[] },
            Kind {
                id: "hours",
                menu: "Weather: Next Hours",
                about: "The next hours, each its sky and temperature, with the chance of rain or the cloud.",
                options: &[
                    Opt { id: "span", label: "Hours:", values: &[("6", "The next 6 hours"), ("3", "The next 3 hours"), ("12", "The next 12 hours")] },
                    Opt { id: "level", label: "Bars show:", values: &[("rain", "Chance of rain"), ("cloud", "Cloud cover"), ("none", "Nothing")] },
                ],
            },
            Kind {
                id: "days",
                menu: "Weather: Next Days",
                about: "The coming days, each its sky, low and high.",
                options: &[Opt { id: "days", label: "Days:", values: &[("7", "7 days"), ("5", "5 days"), ("3", "3 days")] }],
            },
        ],
    },
    Provider {
        app: "kache",
        name: "Kache",
        program: "olo-kache",
        arg: "--glance",
        icon: "kache",
        kinds: &[Kind {
            id: "status",
            menu: "Kache: Build Cache",
            about: "The day's hit rate, the pool's slots in use, and the last hours' builds and hits.",
            options: &[Opt { id: "span", label: "Hours:", values: &[("6", "The last 6 hours"), ("12", "The last 12 hours")] }],
        }],
    },
];

/// App `app`'s provider.
pub fn provider(app: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.app == app)
}

/// App `app`'s kind `kind`.
pub fn kind(app: &str, kind: &str) -> Option<&'static Kind> {
    provider(app)?.kinds.iter().find(|k| k.id == kind)
}

impl Kind {
    /// Option `id`'s value in `options`, else the one it comes with.
    pub fn value<'a>(&self, options: &'a BTreeMap<String, String>, id: &str) -> &'a str {
        let opt = self.options.iter().find(|o| o.id == id);
        match options.get(id) {
            Some(v) if opt.is_none_or(|o| o.values.iter().any(|(k, _)| k == v)) => v.as_str(),
            _ => opt.and_then(|o| o.values.first()).map_or("", |(k, _)| k),
        }
    }

    /// Its options as they come.
    pub fn defaults(&self) -> BTreeMap<String, String> {
        self.options.iter().filter_map(|o| o.values.first().map(|(v, _)| (o.id.to_owned(), (*v).to_owned()))).collect()
    }

    /// Its options, said ("the next 6 hours, chance of rain"), for lists.
    pub fn said(&self, options: &BTreeMap<String, String>) -> String {
        self.options
            .iter()
            .filter_map(|o| {
                let v = self.value(options, o.id);
                o.values.iter().find(|(k, _)| *k == v).map(|(_, name)| name.to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What a widget shows from each time on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Timeline {
    /// Oldest first; each from its time until the next's.
    pub entries: Vec<Entry>,
    /// When it was made (seconds since 1970).
    pub made: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Entry {
    /// From when (seconds since 1970; on a minute, as Deskbar's clock
    /// ticks).
    pub from: i64,
    pub glance: Glance,
}

impl Timeline {
    /// What shows at `now`: the last entry from before it (the first, if
    /// none is yet).
    pub fn at(&self, now: i64) -> Option<&Glance> {
        self.entries.iter().rev().find(|e| e.from <= now).or(self.entries.first()).map(|e| &e.glance)
    }
}

/// What a widget shows at once.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Glance {
    /// Its headline: an icon (named as Olo's icons are), a value ("44°"),
    /// a title ("Rain") and a detail ("Feels like 42° · 11 mph").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub value: String,
    pub title: String,
    pub detail: String,
    /// A line under it: what's coming ("Rain stopping in 34 minutes").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
    /// It all in words: its tip, and its value for agents.
    pub tip: String,
}

/// What's under a glance's headline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Body {
    /// Times across (the next hours): each a label over an icon over a
    /// value, and a level under it (a chance, a share) drawn as a bar in
    /// `hue` (none: the accent).
    Slots {
        slots: Vec<Slot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hue: Option<olo_theme::Hue>,
    },
    /// Rows down (the days): each a label, an icon, and its span from a
    /// low to a high on the scale from `min` to `max`, coloured by where
    /// it lies, cold to warm.
    Spans { spans: Vec<Span>, min: f32, max: f32 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Slot {
    /// "14", "Now".
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// "46°".
    pub value: String,
    /// 0 to 1, if it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Span {
    /// "Today", "Tue".
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub low: f32,
    pub high: f32,
    /// How they're said ("40°", "53°").
    pub low_text: String,
    pub high_text: String,
}

/// One widget an app's provider is asked for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    /// The widget's id (its file's name).
    pub widget: u32,
    pub kind: String,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    #[serde(default)]
    pub size: Size,
}

/// Every widget an app's provider is asked for (none: it ends).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Requests {
    pub widgets: Vec<Request>,
}

/// Where Deskbar and the providers meet: `OLO_GLANCE_DIR` (Deskbar sets it
/// for the providers it runs), else the runtime folder's, one a display
/// (a test session's Deskbar is another's), in a folder of their own: a
/// watch of a folder follows its parent too, and the runtime folder
/// itself changes several times a second (a terminal's tail as it
/// prints), each a wake for nothing.
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os(DIR_ENV).map(PathBuf::from).filter(|d| d.is_absolute()) {
        return d;
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").ok().filter(|d| !d.is_empty() && !d.contains('/')).unwrap_or_else(|| "wayland-0".into());
    runtime.join("olo-glance").join(display)
}

/// The variable naming [`dir`] for a provider.
pub const DIR_ENV: &str = "OLO_GLANCE_DIR";

/// What app `app`'s provider is asked for.
pub fn requests_path(dir: &std::path::Path, app: &str) -> PathBuf {
    dir.join(format!("{app}.requests.json"))
}

/// Widget `id`'s timeline.
pub fn content_path(dir: &std::path::Path, id: u32) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Held by app `app`'s provider while it runs (its pid in it).
pub fn lock_path(dir: &std::path::Path, app: &str) -> PathBuf {
    dir.join(format!("{app}.lock"))
}

/// `value` written to `path` whole (a new file renamed over it: a reader
/// never sees half).
pub fn write_json(path: &std::path::Path, value: &impl Serialize) -> std::io::Result<()> {
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, serde_json::to_vec(value).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_their_options() {
        let hours = kind("weather", "hours").unwrap();
        let mut o = hours.defaults();
        assert_eq!(o.get("span").map(String::as_str), Some("6"));
        assert_eq!(hours.value(&o, "level"), "rain");
        o.insert("span".into(), "12".into());
        assert_eq!(hours.value(&o, "span"), "12");
        // One it doesn't offer: the one it comes with.
        o.insert("span".into(), "99".into());
        assert_eq!(hours.value(&o, "span"), "6");
        assert_eq!(hours.said(&hours.defaults()), "the next 6 hours, chance of rain");
        assert!(kind("weather", "nope").is_none() && provider("nope").is_none());
    }

    /// A widget of an app's kind: kept and read back, named by it, one
    /// kind of the app's not another.
    #[test]
    fn an_app_widget_is_kept() {
        use crate::deskbar::{Measure, Widget};
        let m = Measure::app("weather", "hours");
        let w = Widget::new(4, m.clone());
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"kind\":\"app\"") && json.contains("\"span\":\"6\""), "{json}");
        assert_eq!(serde_json::from_str::<Widget>(&json).unwrap(), w);
        assert!(m.same_kind(&Measure::app("weather", "hours")) && !m.same_kind(&Measure::app("weather", "days")));
        assert_eq!(Measure::kinds().iter().filter(|k| matches!(k, Measure::App { .. })).count(), PROVIDERS.iter().map(|p| p.kinds.len()).sum::<usize>(), "every app's kinds");
        assert_eq!((m.kind_name(), w.label()), ("Weather: Next Hours", "Weather".to_owned()));
    }

    #[test]
    fn a_timeline_shows_its_entry_for_the_time() {
        let at = |from: i64, value: &str| Entry { from, glance: Glance { value: value.into(), ..Glance::default() } };
        let t = Timeline { entries: vec![at(100, "a"), at(200, "b"), at(300, "c")], made: 90 };
        assert_eq!(t.at(50).unwrap().value, "a", "before the first: the first");
        assert_eq!(t.at(100).unwrap().value, "a");
        assert_eq!(t.at(299).unwrap().value, "b");
        assert_eq!(t.at(10_000).unwrap().value, "c");
        assert!(Timeline::default().at(0).is_none());
        // As written and read back.
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Timeline>(&json).unwrap(), t);
        let body = Body::Spans { spans: vec![Span { label: "Tue".into(), low: 3.0, high: 9.0, ..Span::default() }], min: 0.0, max: 10.0 };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"kind\":\"spans\""), "{json}");
        assert_eq!(serde_json::from_str::<Body>(&json).unwrap(), body);
    }
}
