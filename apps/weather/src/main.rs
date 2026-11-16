//! Weather: the forecast for the places a person keeps, every detail of
//! it, and an assistant to ask about it (Ben, 2026-11-14: "create an olo
//! weather native app that has configurable data providers and comes with
//! for now tomorrow.io as the provider … the weather app needs to be
//! extremely detailed, showing the data from every core data layer … it
//! needs to support saving locations, showing current weather and rest of
//! the day weather as well as next 3 days and next 7 days … i will use
//! this not just for personal needs like knowing when the rain might stop
//! but also to know when the clouds might be high enough to skydive or
//! when the cloud bases are not too low or when it's likely to be socked
//! in all day … the app needs to be very intuitive and beautifully
//! designed").
//!
//! - provider.rs: where forecasts come from (tomorrow.rs, demo.rs), the
//!   cache; model.rs: a forecast, every layer; units.rs: what's read.
//! - key.rs: the provider's key, in Passwords (kept, or a reference).
//! - places.rs: the places kept, and finding one.
//! - assistant.rs: questions about the forecast, answered from all of it.
//! - view.rs: the window; dialogs.rs: Add Place and Data Provider.
//!
//! `olo-weather --print [PLACE]` prints a kept place's forecast as JSON
//! (the cached one; with `--fetch`, fetched now), for agents and scripts,
//! with no window.

mod assistant;
mod demo;
mod dialogs;
mod key;
mod model;
mod places;
mod provider;
mod tomorrow;
mod units;
mod view;

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use olo_kit::{
    Child, Cx, Element, Messenger, ScrollHandle, View, WindowHandle, WindowOptions,
    instance::{self, Launch},
    widgets::{TextField, TextFieldEvent, text_field},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    model::Forecast,
    places::Place,
    provider::{Error, Provider},
    units::Units,
};

pub const APP_ID: &str = "olo.weather";
/// How old a forecast may be before it's fetched again.
pub const FRESH: i64 = 30 * 60;

/// What Weather keeps (`$XDG_CONFIG_HOME/olo/weather.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub key: key::Source,
    pub places: Vec<Place>,
    pub chosen: usize,
    pub units: Units,
    /// Rest of Today, Next 3 Days, Next 7 Days.
    pub tab: usize,
    /// The assistant shown beside the forecast.
    pub assistant: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { provider: "tomorrow.io".into(), key: key::Source::None, places: Vec::new(), chosen: 0, units: Units::default(), tab: 0, assistant: true }
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("weather.json")
}

/// The demo: a made-up place and forecast, nothing saved
/// (`OLO_WEATHER_DEMO=1`: tests, screenshots, agent screens).
fn demo() -> bool {
    std::env::var("OLO_WEATHER_DEMO").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// A place's forecast, and how fetching it goes.
#[derive(Default)]
pub struct Entry {
    pub forecast: Option<Arc<Forecast>>,
    /// The fetch under way: its number, its stop, and what it's doing.
    pub fetching: Option<(u64, Arc<AtomicBool>, &'static str)>,
    pub error: Option<Error>,
    /// The cache has been looked in.
    pub loaded: bool,
}

/// A question being answered.
pub struct Asking {
    pub n: u64,
    pub cancel: Arc<AtomicBool>,
    pub question: String,
    pub so_far: String,
    pub model: String,
}

#[derive(Clone)]
pub enum Msg {
    /// What the cache had for a place (by its id).
    Loaded(String, Option<Arc<Forecast>>),
    /// What fetch `n` is doing now.
    Stage(u64, &'static str),
    /// Fetch `n` for a place is done; the key, if it was read for it.
    Fetched(u64, String, Result<Arc<Forecast>, Error>, Option<Arc<Zeroizing<String>>>),
    /// Time to look whether the shown forecast is old (by its number).
    Due(u64),
    Asked(assistant::Asked),
    /// A place to keep (Add Place).
    AddPlace(Place),
    /// A provider and where its key is (Data Provider).
    Provider(String, key::Source),
    /// A place's time zone and height looked up (by its id), for one found
    /// without them.
    Located(String, Result<(Option<String>, Option<f64>), Error>),
    /// A kept place's own name (Rename Place; empty: the name it was
    /// found as).
    RenamePlace(usize, String),
}

pub struct Weather {
    pub settings: Settings,
    demo: bool,
    pub entries: HashMap<String, Entry>,
    key: Option<Arc<Zeroizing<String>>>,
    fetches: u64,
    due: u64,
    /// Each range's own scroll (each keeps its place).
    pub scrolls: [ScrollHandle; 3],
    pub chat_scroll: ScrollHandle,
    pub question: Child<TextField>,
    pub turns: Vec<assistant::Turn>,
    pub asking: Option<Asking>,
    asks: u64,
    /// The spans the last answer pointed to, marked on the chart.
    pub spans: Vec<assistant::Window>,
    key_dialog: Option<WindowHandle<dialogs::KeyDialog>>,
    add_dialog: Option<WindowHandle<dialogs::AddPlace>>,
    rename_dialog: Option<WindowHandle<dialogs::RenamePlace>>,
    /// The places whose time zone is being looked up, by id.
    locating: std::collections::HashSet<String>,
}

/// What's kept across an upgrade in place.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    chosen: usize,
    tab: usize,
    assistant: bool,
    turns: Vec<(String, String, bool)>,
}

/// A place's id (its cache's and its entry's).
pub fn place_id(p: &Place) -> String {
    format!("{:.3},{:.3}", p.lat, p.lon)
}

impl Weather {
    fn new(cx: &mut Cx<Self>) -> Weather {
        let demo = demo();
        let mut settings: Settings = if demo { Settings::default() } else { olo_kit::store::load(&settings_path()).unwrap_or_default() };
        if demo {
            settings.provider = "demo".into();
            settings.places = vec![Place::demo()];
        }
        let saved: Option<Saved> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
        let mut turns = Vec::new();
        if let Some(s) = &saved {
            settings.chosen = s.chosen;
            settings.tab = s.tab;
            settings.assistant = s.assistant;
            turns = s.turns.iter().map(|(q, a, failed)| assistant::Turn { question: q.clone(), answer: a.clone(), windows: Vec::new(), failed: *failed }).collect();
        }
        let question = text_field(cx, |f| f.name("Question").hint("Ask about the weather…"));
        cx.subscribe(&question, |v: &mut Weather, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                v.ask_typed(cx);
            }
        });
        let mut w = Weather {
            settings,
            demo,
            entries: HashMap::new(),
            key: None,
            fetches: 0,
            due: 0,
            scrolls: [ScrollHandle::new(), ScrollHandle::new(), ScrollHandle::new()],
            chat_scroll: ScrollHandle::new(),
            question,
            turns,
            asking: None,
            asks: 0,
            spans: Vec::new(),
            key_dialog: None,
            add_dialog: None,
            rename_dialog: None,
            locating: Default::default(),
        };
        w.show_chosen(false, cx);
        w.locate_missing(cx);
        w
    }

    /// The time zone and height of each place kept without them (found by
    /// its address, or typed as coordinates), looked up on a worker and
    /// kept: once each, or again at the next start if it couldn't be.
    fn locate_missing(&mut self, cx: &mut Cx<Self>) {
        if self.demo {
            return;
        }
        for p in &self.settings.places {
            let id = place_id(p);
            if p.timezone.is_some() || !self.locating.insert(id.clone()) {
                continue;
            }
            let (lat, lon) = (p.lat, p.lon);
            cx.work(move || places::locate(lat, lon), move |r| Msg::Located(id, r));
        }
    }

    pub fn provider(&self) -> &'static dyn Provider {
        provider::by_id(&self.settings.provider)
    }

    pub fn place(&self) -> Option<&Place> {
        self.settings.places.get(self.settings.chosen).or(self.settings.places.first())
    }

    pub fn entry(&self) -> Option<&Entry> {
        self.place().and_then(|p| self.entries.get(&place_id(p)))
    }

    /// Whether a key is wanted and not given.
    pub fn needs_key(&self) -> bool {
        self.provider().needs_key() && self.settings.key == key::Source::None
    }

    fn save(&self) {
        if !self.demo {
            olo_kit::store::save(settings_path(), &self.settings);
        }
    }

    /// The chosen place shown: what the cache has at once, then fetched
    /// if that's old (or `force`).
    fn show_chosen(&mut self, force: bool, cx: &mut Cx<Self>) {
        let Some(place) = self.place().cloned() else { return };
        let id = place_id(&place);
        let entry = self.entries.entry(id.clone()).or_default();
        if !entry.loaded {
            entry.loaded = true;
            let provider = self.provider();
            let (p, i) = (place.clone(), id.clone());
            cx.work(move || provider::cached(provider, &p).map(Arc::new), move |f| Msg::Loaded(i, f));
            return;
        }
        let old = entry.forecast.as_ref().is_none_or(|f| provider::now() - f.fetched >= FRESH || f.provider != self.settings.provider);
        if (old || force) && entry.fetching.is_none() {
            self.fetch(place, cx);
        }
    }

    /// Fetch `place`'s forecast on a worker (the key read from Passwords
    /// first, if it isn't held yet).
    fn fetch(&mut self, place: Place, cx: &mut Cx<Self>) {
        if self.needs_key() {
            return;
        }
        self.fetches += 1;
        let n = self.fetches;
        let stop = Arc::new(AtomicBool::new(false));
        let id = place_id(&place);
        let provider = self.provider();
        let source = self.settings.key.clone();
        let held = self.key.clone();
        let first = if held.is_none() && provider.needs_key() { "Waiting for Passwords…" } else { "Fetching…" };
        let entry = self.entries.entry(id.clone()).or_default();
        entry.fetching = Some((n, stop.clone(), first));
        let stage = cx.messenger();
        cx.work(
            move || {
                let (key, read) = match held {
                    Some(k) => (k, None),
                    None if !provider.needs_key() => (Arc::new(Zeroizing::new(String::new())), None),
                    None => match key::read(provider, &source) {
                        Ok(k) => {
                            let k = Arc::new(k);
                            (k.clone(), Some(k))
                        }
                        Err(e) => return (Err(e), None),
                    },
                };
                stage.send(Msg::Stage(n, "Fetching…"));
                let result = provider.fetch(&key, &place, &stop).map(|f| {
                    provider::keep(&f, &place);
                    Arc::new(f)
                });
                (result, read)
            },
            move |(result, read)| Msg::Fetched(n, id, result, read),
        );
        cx.invalidate();
    }

    /// Look again in `seconds` whether the shown forecast is old.
    fn look_again_in(&mut self, seconds: i64, cx: &mut Cx<Self>) {
        self.due += 1;
        cx.after(Duration::from_secs(seconds.clamp(60, FRESH) as u64), Msg::Due(self.due));
    }

    pub fn choose(&mut self, i: usize, cx: &mut Cx<Self>) {
        if i < self.settings.places.len() && i != self.settings.chosen {
            self.settings.chosen = i;
            self.spans.clear();
            self.save();
            self.show_chosen(false, cx);
            cx.invalidate();
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Cx<Self>) {
        if tab < 3 && tab != self.settings.tab {
            self.settings.tab = tab;
            self.save();
            cx.invalidate();
        }
    }

    /// Mark `spans` on the chart, in a tab that shows them all: the
    /// narrowest (`always`), or another only if this one doesn't.
    fn mark(&mut self, spans: Vec<assistant::Window>, always: bool, cx: &mut Cx<Self>) {
        let now = provider::now();
        let wanted = self.place().cloned().zip(self.entry().and_then(|e| e.forecast.clone())).and_then(|(place, f)| {
            let want = view::tab_for(&place, &f, &spans, now)?;
            let shows = view::tab_range(&place, &f, self.settings.tab, now).1 >= spans.iter().map(|s| s.end).max()? as f64;
            (always || !shows).then_some(want)
        });
        self.spans = spans;
        if let Some(tab) = wanted {
            self.set_tab(tab, cx);
        }
        cx.invalidate();
    }

    /// Show turn `i`'s spans on the chart (a span under its answer
    /// clicked).
    pub fn show_spans(&mut self, i: usize, cx: &mut Cx<Self>) {
        if let Some(t) = self.turns.get(i) {
            let spans = t.windows.clone();
            self.mark(spans, true, cx);
        }
    }

    pub fn remove_place(&mut self, i: usize, cx: &mut Cx<Self>) {
        if i >= self.settings.places.len() {
            return;
        }
        let gone = self.settings.places.remove(i);
        provider::forget(&gone);
        self.entries.remove(&place_id(&gone));
        if self.settings.chosen >= self.settings.places.len() {
            self.settings.chosen = self.settings.places.len().saturating_sub(1);
        }
        self.spans.clear();
        self.save();
        self.show_chosen(false, cx);
        cx.invalidate();
        cx.invalidate();
    }

    pub fn set_units(&mut self, units: Units, cx: &mut Cx<Self>) {
        if units != self.settings.units {
            self.settings.units = units;
            self.save();
            cx.invalidate();
        }
    }

    pub fn open_add_place(&mut self, cx: &mut Cx<Self>) {
        if olo_kit::raise(self.add_dialog.as_ref()) {
            return;
        }
        self.add_dialog = Some(dialogs::AddPlace::open(cx.messenger()));
    }

    /// Rename Place… for place `i`.
    pub fn open_rename(&mut self, i: usize, cx: &mut Cx<Self>) {
        if olo_kit::raise(self.rename_dialog.as_ref()) {
            return;
        }
        let Some(place) = self.settings.places.get(i) else { return };
        self.rename_dialog = Some(dialogs::RenamePlace::open(i, place, cx.messenger()));
    }

    /// Place `i` called `name` (empty, or the name it was found as: no
    /// name of its own), kept.
    pub fn rename_place(&mut self, i: usize, name: &str, cx: &mut Cx<Self>) {
        let Some(place) = self.settings.places.get_mut(i) else { return };
        let name = name.trim();
        place.label = (!name.is_empty() && name != place.name).then(|| name.to_owned());
        self.save();
        cx.invalidate();
    }

    pub fn open_key_dialog(&mut self, cx: &mut Cx<Self>) {
        if olo_kit::raise(self.key_dialog.as_ref()) {
            return;
        }
        self.key_dialog = Some(dialogs::KeyDialog::open(self.settings.provider.clone(), self.settings.key.clone(), cx.messenger()));
    }

    /// Ask what's typed in the assistant's field.
    pub fn ask_typed(&mut self, cx: &mut Cx<Self>) {
        let q = self.question.read(cx).text().trim().to_owned();
        if !q.is_empty() {
            self.question.update(cx, |f, cx| f.set_text("", cx));
            self.ask(q, cx);
        }
    }

    /// Ask the assistant `question` about the shown forecast.
    pub fn ask(&mut self, question: String, cx: &mut Cx<Self>) {
        let (Some(place), Some(forecast)) = (self.place().cloned(), self.entry().and_then(|e| e.forecast.clone())) else { return };
        self.stop_asking();
        if !self.settings.assistant {
            self.settings.assistant = true;
            self.save();
        }
        self.asks += 1;
        let n = self.asks;
        let cancel = Arc::new(AtomicBool::new(false));
        // (Answers that came: not what went wrong, nor one stopped before
        // a word.)
        let history: Vec<olo_ai::Turn> = self.turns.iter().filter(|t| !t.failed && !t.answer.is_empty()).map(|t| olo_ai::Turn { question: t.question.clone(), answer: t.answer.clone() }).collect();
        let system = assistant::system(&place, &forecast, &self.settings.units, provider::now());
        let send = cx.messenger();
        assistant::ask(n, question.clone(), system, history, cancel.clone(), move |a| {
            send.send(Msg::Asked(a));
        });
        self.asking = Some(Asking { n, cancel, question, so_far: String::new(), model: String::new() });
        self.chat_scroll.scroll_to_bottom(cx);
        cx.invalidate();
    }

    pub fn stop_asking(&mut self) {
        if let Some(a) = self.asking.take() {
            a.cancel.store(true, Ordering::Relaxed);
            // (What was asked stays, with as much as came: none, if it
            // was stopped before anything did.)
            let answer = if a.so_far.is_empty() { String::new() } else { format!("{} …", a.so_far) };
            self.turns.push(assistant::Turn { question: a.question, answer, windows: Vec::new(), failed: false });
        }
    }

    pub fn new_conversation(&mut self, cx: &mut Cx<Self>) {
        self.stop_asking();
        self.turns.clear();
        self.spans.clear();
        cx.invalidate();
    }

    /// The whole of what the window shows, as text (Edit ▸ Copy): the
    /// place, now, and each day.
    pub fn as_text(&self) -> String {
        view::summary(self)
    }

    pub fn menus(&self) -> Vec<olo_kit::Menu> {
        use olo_kit::Menu;
        let has = self.place().is_some();
        let u = self.settings.units;
        let unit = |label: &'static str, command: &'static str, on: bool| (label, command, on);
        let groups: [(&str, Vec<(&str, &str, bool)>); 6] = [
            ("Temperature", vec![unit("Celsius (°C)", "units:c", u.temperature == units::Temperature::Celsius), unit("Fahrenheit (°F)", "units:f", u.temperature == units::Temperature::Fahrenheit)]),
            (
                "Wind",
                vec![
                    unit("Kilometres an Hour", "units:kmh", u.speed == units::Speed::Kmh),
                    unit("Miles an Hour", "units:mph", u.speed == units::Speed::Mph),
                    unit("Metres a Second", "units:ms", u.speed == units::Speed::Ms),
                    unit("Knots", "units:kt", u.speed == units::Speed::Knots),
                ],
            ),
            ("Cloud Heights", vec![unit("Metres", "units:m", u.height == units::Height::Metres), unit("Feet", "units:ft", u.height == units::Height::Feet)]),
            ("Visibility", vec![unit("Kilometres", "units:km", u.distance == units::Distance::Km), unit("Miles", "units:mi", u.distance == units::Distance::Miles)]),
            ("Precipitation", vec![unit("Millimetres", "units:mm", u.precipitation == units::Precipitation::Mm), unit("Inches", "units:in", u.precipitation == units::Precipitation::Inches)]),
            ("Pressure", vec![unit("Hectopascals", "units:hpa", u.pressure == units::Pressure::Hpa), unit("Inches of Mercury", "units:inhg", u.pressure == units::Pressure::InHg)]),
        ];
        let mut units_menu = Menu::new("Units").item("All Metric", "units:metric").checked(u == Units::METRIC).item("All Imperial", "units:imperial").checked(u == Units::IMPERIAL);
        for (title, items) in groups {
            let mut m = Menu::new(title);
            for (label, command, on) in items {
                m = m.item(label, command).checked(on);
            }
            units_menu = units_menu.separator().submenu(m);
        }
        let asking = self.asking.is_some();
        vec![
            Menu::new("File")
                .app_items(&["ai"])
                .item("Add Place…", "add-place")
                .key("n")
                .item("Rename Place…", "rename-place")
                .enabled(has)
                .item("Remove Place", "remove-place")
                .enabled(has)
                .separator()
                .item("Data Provider…", "provider")
                .item("Refresh", "refresh")
                .key("r")
                .enabled(has && !self.needs_key())
                .close_quit(),
            Menu::new("Edit").item("Copy", "copy").key("c").enabled(self.entry().is_some_and(|e| e.forecast.is_some())),
            Menu::new("View")
                .item("Rest of Today", "tab:0")
                .key("1")
                .checked(self.settings.tab == 0)
                .item("Next 3 Days", "tab:1")
                .key("2")
                .checked(self.settings.tab == 1)
                .item("Next 7 Days", "tab:2")
                .key("3")
                .checked(self.settings.tab == 2)
                .separator()
                .submenu(units_menu)
                .separator()
                .item("Show Assistant", "assistant")
                .key("j")
                .checked(self.settings.assistant),
            Menu::new("Assistant")
                .item("Ask…", "ask")
                .key("l")
                .enabled(has)
                .item("Stop", "stop")
                .enabled(asking)
                .separator()
                .item("New Conversation", "new-conversation")
                .enabled(!self.turns.is_empty() || asking),
        ]
    }
}

impl View for Weather {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Loaded(id, forecast) => {
                if let Some(e) = self.entries.get_mut(&id) {
                    if e.forecast.is_none() {
                        e.forecast = forecast;
                    }
                }
                // Then fetched, if what was kept is old.
                if self.place().is_some_and(|p| place_id(p) == id) {
                    self.show_chosen(false, cx);
                    if let Some(f) = self.entry().and_then(|e| e.forecast.clone()) {
                        self.look_again_in(FRESH - (provider::now() - f.fetched), cx);
                    }
                }
            }
            Msg::Stage(n, stage) => {
                for e in self.entries.values_mut() {
                    if let Some((m, _, s)) = &mut e.fetching
                        && *m == n
                    {
                        *s = stage;
                    }
                }
            }
            Msg::Fetched(n, id, result, read) => {
                let Some(e) = self.entries.get_mut(&id) else { return };
                if e.fetching.as_ref().is_none_or(|(m, ..)| *m != n) {
                    return;
                }
                e.fetching = None;
                if let Some(k) = read {
                    self.key = Some(k);
                }
                match result {
                    Ok(f) => {
                        e.forecast = Some(f);
                        e.error = None;
                        self.look_again_in(FRESH, cx);
                    }
                    Err(Error::Stopped) => {}
                    Err(err) => {
                        tracing::warn!("weather: {err}");
                        // A key refused is let go of, to be read again.
                        if matches!(err, Error::Key(_)) {
                            self.key = None;
                        }
                        let retry = match &err {
                            Error::Limit(_, Some(s)) => *s as i64,
                            Error::Limit(..) => 15 * 60,
                            Error::Unreachable(_) => 5 * 60,
                            _ => FRESH,
                        };
                        e.error = Some(err);
                        self.look_again_in(retry, cx);
                    }
                }
                cx.invalidate();
            }
            Msg::Due(n) => {
                if n == self.due {
                    self.show_chosen(false, cx);
                }
                return;
            }
            Msg::Asked(a) => match a {
                assistant::Asked::Model(n, model) => match &mut self.asking {
                    Some(asking) if asking.n == n => asking.model = model,
                    _ => return,
                },
                assistant::Asked::SoFar(n, words) => match &mut self.asking {
                    Some(asking) if asking.n == n => {
                        asking.so_far = words;
                        self.chat_scroll.scroll_to_bottom(cx);
                    }
                    _ => return,
                },
                assistant::Asked::Done(n, result) => {
                    let Some(asking) = self.asking.take_if(|a| a.n == n) else { return };
                    let turn = match result {
                        Ok((answer, windows)) => {
                            self.mark(windows.clone(), false, cx);
                            assistant::Turn { question: asking.question, answer, windows, failed: false }
                        }
                        Err(e) => assistant::Turn { question: asking.question, answer: e, windows: Vec::new(), failed: true },
                    };
                    self.turns.push(turn);
                    self.chat_scroll.scroll_to_bottom(cx);
                    cx.invalidate();
                }
            },
            Msg::AddPlace(place) => {
                let at = match self.settings.places.iter().position(|p| p.same(&place)) {
                    Some(i) => i,
                    None => {
                        self.settings.places.push(place);
                        self.settings.places.len() - 1
                    }
                };
                self.settings.chosen = usize::MAX;
                self.choose(at, cx);
                self.locate_missing(cx);
            }
            Msg::Located(id, result) => {
                self.locating.remove(&id);
                let Ok((zone, elevation)) = result else { return };
                let Some(p) = self.settings.places.iter_mut().find(|p| place_id(p) == id) else { return };
                if p.timezone.is_none() {
                    p.timezone = zone;
                }
                if p.elevation.is_none() {
                    p.elevation = elevation;
                }
                self.save();
                cx.invalidate();
            }
            Msg::RenamePlace(i, name) => self.rename_place(i, &name, cx),
            Msg::Provider(provider, source) => {
                let changed = provider != self.settings.provider || source != self.settings.key;
                self.settings.provider = provider;
                if source != key::Source::None || !self.provider().needs_key() {
                    self.settings.key = source;
                }
                self.key = None;
                self.save();
                if changed {
                    for e in self.entries.values_mut() {
                        e.error = None;
                    }
                    self.show_chosen(true, cx);
                }
                cx.invalidate();
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "add-place" => self.open_add_place(cx),
            "remove-place" => self.remove_place(self.settings.chosen, cx),
            "rename-place" => self.open_rename(self.settings.chosen, cx),
            // (An agent's: the chosen place's own name, at once.)
            c if c.starts_with("rename:") => self.rename_place(self.settings.chosen, &c[7..], cx),
            "provider" => self.open_key_dialog(cx),
            "refresh" => self.show_chosen(true, cx),
            "copy" => olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(self.as_text())),
            "assistant" => {
                self.settings.assistant = !self.settings.assistant;
                self.save();
                cx.invalidate();
            }
            "ask" => {
                if !self.settings.assistant {
                    self.settings.assistant = true;
                    self.save();
                    cx.invalidate();
                }
                self.question.focus(cx);
            }
            "stop" => {
                self.stop_asking();
                cx.invalidate();
            }
            "new-conversation" => {
                self.new_conversation(cx);
                cx.invalidate();
            }
            "close" | "quit" => olo_kit::quit(),
            c if c.starts_with("tab:") => self.set_tab(c[4..].parse().unwrap_or(0), cx),
            c if c.starts_with("place:") => {
                // (An agent's: by its place in the list, or its name.)
                let which = &c[6..];
                let i = which.parse::<usize>().ok().or_else(|| self.settings.places.iter().position(|p| p.named(which)));
                match i {
                    Some(i) => self.choose(i, cx),
                    None => return false,
                }
            }
            c if c.starts_with("ask:") => self.ask(c[4..].trim().to_owned(), cx),
            c if c.starts_with("units:") => {
                let mut u = self.settings.units;
                match &c[6..] {
                    "metric" => u = Units::METRIC,
                    "imperial" => u = Units::IMPERIAL,
                    "c" => u.temperature = units::Temperature::Celsius,
                    "f" => u.temperature = units::Temperature::Fahrenheit,
                    "kmh" => u.speed = units::Speed::Kmh,
                    "mph" => u.speed = units::Speed::Mph,
                    "ms" => u.speed = units::Speed::Ms,
                    "kt" => u.speed = units::Speed::Knots,
                    "m" => u.height = units::Height::Metres,
                    "ft" => u.height = units::Height::Feet,
                    "km" => u.distance = units::Distance::Km,
                    "mi" => u.distance = units::Distance::Miles,
                    "mm" => u.precipitation = units::Precipitation::Mm,
                    "in" => u.precipitation = units::Precipitation::Inches,
                    "hpa" => u.pressure = units::Pressure::Hpa,
                    "inhg" => u.pressure = units::Pressure::InHg,
                    _ => return false,
                }
                self.set_units(u, cx);
            }
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        view::body(self, cx)
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        let turns = self.turns.iter().map(|t| (t.question.clone(), t.answer.clone(), t.failed)).collect();
        serde_json::to_string(&Saved { chosen: self.settings.chosen, tab: self.settings.tab, assistant: self.settings.assistant, turns }).ok()
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.stop_asking();
        for e in self.entries.values() {
            if let Some((_, stop, _)) = &e.fetching {
                stop.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// `olo-weather --print [PLACE] [--fetch]`: a kept place's forecast as
/// JSON (the first, or the one named), with no window.
fn print(args: &[String]) -> std::process::ExitCode {
    let fetch = args.iter().any(|a| a == "--fetch");
    let name = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned();
    let settings: Settings = if demo() { Settings { provider: "demo".into(), places: vec![Place::demo()], ..Settings::default() } } else { olo_kit::store::load(&settings_path()).unwrap_or_default() };
    let Some(place) = (match &name {
        Some(n) => settings.places.iter().find(|p| p.named(n)),
        None => settings.places.get(settings.chosen).or(settings.places.first()),
    }) else {
        eprintln!("olo-weather: no such place kept{}", name.map(|n| format!(" (“{n}”)")).unwrap_or_default());
        return std::process::ExitCode::FAILURE;
    };
    let provider = provider::by_id(&settings.provider);
    let forecast = if fetch || provider::cached(provider, place).is_none() {
        let key = if provider.needs_key() {
            match key::read(provider, &settings.key) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("olo-weather: {e}");
                    return std::process::ExitCode::FAILURE;
                }
            }
        } else {
            Zeroizing::new(String::new())
        };
        match provider.fetch(&key, place, &AtomicBool::new(false)) {
            Ok(f) => {
                provider::keep(&f, place);
                f
            }
            Err(e) => {
                eprintln!("olo-weather: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    } else {
        provider::cached(provider, place).expect("just looked")
    };
    let out = view::as_json(place, &forecast, &settings.units);
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        return print(&args);
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("Weather", 1180.0, 760.0).min_size(view::MIN_WIDTH, view::MIN_HEIGHT).keep_size("main"), Weather::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}

/// For the dialogs: the main view's messages.
pub type ToMain = Messenger<Msg>;
