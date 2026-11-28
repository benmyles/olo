//! The window: a button, how far the test is, and four cards on the
//! dashboard's gutter (as Activity Monitor's): Download and Upload (the
//! rate, its graph, and what TCP did while it ran: this computer's
//! kernel's side and the server's), Latency (the round trip idle and
//! while the line is busy, and what the busy line adds), Connection
//! (where the server is and how the link to it is made).
//!
//! Everything is live as the test runs (the engine's [`Update`]s, each a
//! message), stopped at once by Stop, and kept: the last test's results
//! are there when the window opens again.

use std::{
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    time::Instant,
};

use olo_kit::{
    Cx, Element, IntoElement as _, Menu, MenuBar, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, WindowOptions, div, metrics as m,
    palette::Hue,
    px, style,
    widgets::{
        Button, Graph, ScrollView, Series, StatusBar, StringView,
        dashboard::{self, Card, Facts, facts_label_width},
        open_alert,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    engine::{self, Config, Latency, Ours, Phase, Results, Transfer, Update},
    http::Opening,
    words,
};

const WIDTH: f32 = 760.0;
/// Narrower than this, the four cards go one under another, not two by
/// two: a card's facts need about 280 points, their values whole. (At
/// 500, a quarter of a screen had 13 of the 16 values cut: Opus's and
/// Neon's reviews, 2026-11-28.)
const TWO_ACROSS: f32 = 600.0;

/// The strip over the cards: as tall as the progress bar with its words
/// over it, whether or not a test runs (nothing under it moves as one
/// starts).
const STRIP: f32 = m::LABEL_ROW + m::TIGHT + 14.0;

/// The window: as wide and as tall as what it shows, and no larger: it
/// floats over tiled windows and is never a tile or a tab, where it was
/// stretched to a size its four cards don't fill (Ben, 2026-11-16: "should
/// be floating with max size so it doesn't look broken").
pub fn options() -> WindowOptions {
    // (220 points its least: as tall as its cards where it's let be,
    // what tiling gives it where held. Designed at 700, held shorter it
    // was measured at 700 all the same, and the window scrolled, its
    // menus with it, beside its cards' own bar: Astra's, Neon's and
    // Opus's reviews, 2026-11-28.)
    WindowOptions { fixed: true, ..WindowOptions::new("Speed Test", WIDTH, DESIGN_HEIGHT).fits_content().floating() }
}
/// The height it's designed at: its least (a horizontal third's); let
/// be, it's as tall as its cards.
const DESIGN_HEIGHT: f32 = 220.0;

/// Narrower than this inside, a card's facts each have their label over
/// their value, the value whole on as many lines as it takes: beside its
/// label it was cut ("4.1 ms least, 55 m…": Astra's, Opus's and Neon's
/// reviews, 2026-11-28).
const FACTS_STACK_BELOW: f32 = 300.0;

/// A card's room inside, in a window `vw` wide: one column or two;
/// `held`, its cards beside their scroll bar.
fn card_inner(vw: f32, held: bool) -> f32 {
    let mut w = vw - 2.0 * m::WINDOW_INSET - 2.0 * dashboard::GAP - if held { olo_kit::widgets::SCROLL_BAR } else { 0.0 };
    if vw >= TWO_ACROSS {
        w = (w - dashboard::GAP) / 2.0;
    }
    w - 2.0 * m::CARD_INSET
}

/// A narrow card's facts: each label in the small size and the card's
/// quiet ink over its value, 2 points apart, the value wrapped to the
/// card (never cut), a tight spacing between one and the next.
fn stacked_facts(facts: Vec<(&'static str, String)>, p: &olo_kit::palette::Palette) -> Element {
    let ink = style::card_ink(p);
    let mut col = div().w_full().flex().flex_col().gap(px(m::TIGHT));
    for (label, value) in facts {
        col = col.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(StringView::new(olo_kit::words::tr(label)).size(style::SMALL_SIZE).color(ink.dim))
                .child(StringView::new(value).size(style::DETAIL_SIZE).wrap(true).color(ink.text)),
        );
    }
    col.into_element()
}

/// A card's graph.
const GRAPH: f32 = 64.0;
/// How long a test is taken to last, for how far along it is: opening
/// and the idle round trips, then each way.
const BEFORE: f64 = 1.5;

/// The link the test ran over, as NetworkManager has it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// "Wi-Fi" or "Ethernet".
    pub kind: String,
    pub interface: String,
    /// A Wi-Fi network's name.
    pub name: Option<String>,
    /// The link's own rate: the radio's now, or the cable's.
    pub rate_mbps: Option<u32>,
    pub band: Option<String>,
    /// Wi-Fi's signal, 0 to 100.
    pub signal: Option<u32>,
    /// Paid for by the byte, as far as is known.
    pub metered: bool,
}

impl Link {
    /// The link that carries the default route now (blocking: the bus).
    pub fn now() -> Option<Link> {
        use olo_system::network::{self, Kind};
        let links = network::links().ok()?;
        let metered = network::metered().unwrap_or(false);
        match links.primary? {
            Kind::Wifi => {
                let radio = network::wifi_radio().ok().flatten();
                Some(Link {
                    kind: "Wi-Fi".into(),
                    interface: links.wifi_device.as_ref().map(|d| d.interface.clone()).unwrap_or_default(),
                    name: links.wifi.as_ref().and_then(|w| w.ssid.clone()),
                    rate_mbps: radio.map(|r| r.rate_mbps),
                    band: radio.map(|r| r.band().to_owned()),
                    signal: links.wifi.as_ref().map(|w| w.strength),
                    metered,
                })
            }
            Kind::Ethernet => {
                let wired = links.wired?;
                Some(Link { kind: "Ethernet".into(), interface: wired.interface.clone(), name: None, rate_mbps: wired.speed, band: None, signal: None, metered })
            }
            Kind::Other => None,
        }
    }
}

type Samples = Rc<VecDeque<(f64, f64)>>;
/// A card in words: its title, figure, caption and facts (each a label
/// and a value).
// (A fact's label stays a literal, which the kit looks up as it shows it.)
type CardWords = (&'static str, String, String, Vec<(&'static str, String)>);

/// What the window shows of a test: the one under way as it's told, or
/// the last one's.
#[derive(Default)]
struct Shown {
    /// Megabits a second by seconds into its phase.
    down: Samples,
    up: Samples,
    /// Milliseconds by seconds into the test.
    latency: Samples,
    down_now: Option<f64>,
    up_now: Option<f64>,
    ours_down: Option<Ours>,
    ours_up: Option<Ours>,
}

/// A test under way.
struct Running {
    which: u32,
    stop: Arc<AtomicBool>,
    began: Instant,
    phase: Phase,
    phase_began: Instant,
}

/// What's kept from one opening of the window to the next.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    results: Option<Results>,
    link: Option<Link>,
    latency: Vec<(f32, f32)>,
}

pub struct Speed {
    config: Config,
    /// Made-up results, nothing connected to (demo.rs).
    demo: bool,
    runs: u32,
    /// Start was pressed, and the link is being asked about (whether it's
    /// metered) before anything moves.
    asking: bool,
    running: Option<Running>,
    /// The last test's, or what a stopped one measured.
    results: Option<Results>,
    link: Option<Link>,
    shown: Shown,
    /// Where the server is and how long opening the first connection to
    /// it took, once it has answered.
    place: Option<(String, engine::Place, Opening)>,
    error: Option<String>,
    /// The cards' scroll, held shorter than they are (`body`).
    scroll: ScrollHandle,
}

#[derive(Clone)]
pub enum Msg {
    Start,
    /// The link, as asked about at Start; and go (it isn't metered, or
    /// the person said to anyway), or don't.
    Linked(Option<Link>),
    Begin,
    Never,
    Stop,
    /// From the test's threads: test N says this happened.
    Told(u32, Update),
    Done(u32, Result<Box<Results>, String>),
    /// What was kept is on the disk.
    Kept,
}

fn saved_file() -> std::path::PathBuf {
    olo_xdg::state_dir().join("speedtest.json")
}

impl Speed {
    fn menus(&self) -> Vec<Menu> {
        let running = self.running.is_some();
        vec![
            Menu::new("File").app_items(&[]).item("Start Test", "start").key("r").enabled(!running && !self.asking).item("Stop", "stop").key(".").enabled(running).close_quit(),
            Menu::new("Edit").item("Copy", "copy").key("c").enabled(self.results.is_some()),
        ]
    }

    pub fn new(_: &mut Cx<Speed>) -> Speed {
        let demo = std::env::var_os("OLO_SPEEDTEST_DEMO").is_some();
        let saved: Saved = if demo {
            Saved { results: Some(crate::demo::results()), link: Some(crate::demo::link()), latency: crate::demo::latency() }
        } else {
            olo_kit::restored_state().or_else(|| std::fs::read_to_string(saved_file()).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
        };
        Speed::of(saved, demo)
    }

    fn of(saved: Saved, demo: bool) -> Speed {
        let mut speed = Speed {
            config: Config::default(),
            demo,
            runs: 0,
            asking: false,
            running: None,
            results: None,
            link: saved.link,
            shown: Shown::default(),
            place: None,
            error: None,
            scroll: ScrollHandle::new(),
        };
        if let Some(results) = saved.results {
            speed.show(results, saved.latency.iter().map(|(t, v)| (*t as f64, *v as f64)).collect());
        }
        speed
    }

    /// A finished test's results as what the window shows.
    fn show(&mut self, results: Results, latency: VecDeque<(f64, f64)>) {
        let series = |t: &Option<Transfer>| -> Samples { Rc::new(t.iter().flat_map(|t| t.series.iter().map(|(at, v)| (*at as f64, *v as f64))).collect()) };
        self.shown = Shown {
            down: series(&results.download),
            up: series(&results.upload),
            latency: Rc::new(latency),
            down_now: results.download.as_ref().map(|t| t.mbps),
            up_now: results.upload.as_ref().map(|t| t.mbps),
            ours_down: results.download.as_ref().map(|t| t.ours.clone()),
            ours_up: results.upload.as_ref().map(|t| t.ours.clone()),
        };
        self.place = Some((results.address.clone(), results.place.clone(), Opening { dns_ms: results.dns_ms, connect_ms: results.connect_ms, tls_ms: results.tls_ms }));
        self.results = Some(results);
    }

    fn saved(&self) -> Option<String> {
        serde_json::to_string(&Saved { results: self.results.clone(), link: self.link.clone(), latency: self.shown.latency.iter().map(|(t, v)| (*t as f32, *v as f32)).collect() }).ok()
    }

    /// Start pressed: first the link is asked about, off the looper (the
    /// bus). A metered one is said, and the person asked.
    fn start(&mut self, cx: &mut Cx<Self>) {
        if self.running.is_some() || self.asking {
            return;
        }
        self.error = None;
        if self.demo {
            *self = Speed::of(Saved { results: Some(crate::demo::results()), link: Some(crate::demo::link()), latency: crate::demo::latency() }, true);
            return;
        }
        self.asking = true;
        cx.work(Link::now, Msg::Linked);
    }

    fn linked(&mut self, link: Option<Link>, cx: &mut Cx<Self>) {
        let metered = link.as_ref().is_some_and(|l| l.metered);
        self.link = link;
        if !metered {
            return self.begin(cx);
        }
        let chosen = cx.msg_with(|choice: &Option<usize>| if *choice == Some(1) { Msg::Begin } else { Msg::Never });
        open_alert(
            cx,
            "Speed Test",
            "warning",
            "This connection is metered: what moves over it may be paid for by the amount. The test moves as much data as the line carries in sixteen seconds, each way’s rate for eight (2 GB on a line of a gigabit a second, 20 MB on one of ten megabits).",
            &["Cancel", "Test Anyway"],
            chosen,
        );
    }

    fn begin(&mut self, cx: &mut Cx<Self>) {
        self.asking = false;
        if self.running.is_some() {
            return;
        }
        self.runs += 1;
        let which = self.runs;
        let stop = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        self.running = Some(Running { which, stop: stop.clone(), began: now, phase: Phase::Connecting, phase_began: now });
        self.shown = Shown::default();
        (self.results, self.place, self.error) = (None, None, None);
        let (config, told) = (self.config.clone(), cx.messenger());
        cx.work(
            move || {
                engine::run(&config, &stop, &|update| {
                    told.send(Msg::Told(which, update));
                })
            },
            move |results| Msg::Done(which, results.map(Box::new)),
        );
    }

    fn stop(&mut self) {
        if let Some(running) = &self.running {
            running.stop.store(true, SeqCst);
        }
    }

    fn told(&mut self, update: Update) {
        let Some(running) = &mut self.running else { return };
        let since = running.began.elapsed().as_secs_f64();
        let push = |samples: &mut Samples, at: f64, v: f64| Rc::make_mut(samples).push_back((at, v));
        // What's known of the test so far is its results, filled in as
        // it goes: a way that's over is shown whole while the other runs.
        let so_far = self.results.get_or_insert_with(|| Results { stopped: true, ..Results::default() });
        match update {
            Update::Phase(phase) => (running.phase, running.phase_began) = (phase, Instant::now()),
            Update::Idle { latency, tcp_rtt_ms } => (so_far.idle, so_far.idle_tcp_rtt_ms) = (latency, tcp_rtt_ms),
            Update::Measured { phase: Phase::Download, transfer } => {
                self.shown.down_now = Some(transfer.mbps);
                so_far.download = Some(*transfer);
            }
            Update::Measured { transfer, .. } => {
                self.shown.up_now = Some(transfer.mbps);
                so_far.upload = Some(*transfer);
            }
            Update::Opened { address, place, opening } => self.place = Some((address, place, opening)),
            Update::Latency { ms, .. } => push(&mut self.shown.latency, since, ms),
            Update::Rate { phase: Phase::Download, at, mbps } => {
                push(&mut self.shown.down, at, mbps);
                self.shown.down_now = Some(recent(&self.shown.down));
            }
            Update::Rate { at, mbps, .. } => {
                push(&mut self.shown.up, at, mbps);
                self.shown.up_now = Some(recent(&self.shown.up));
            }
            Update::Tcp { phase: Phase::Download, ours } => self.shown.ours_down = Some(ours),
            Update::Tcp { ours, .. } => self.shown.ours_up = Some(ours),
        }
    }

    /// How far along the test is (0 to 1), and what it's doing, in words.
    fn progress(&self) -> (f32, String) {
        let Some(running) = &self.running else {
            return match (&self.results, &self.error) {
                (_, Some(e)) => (0.0, olo_kit::tr!("The test didn't finish: {}.", e)),
                (Some(r), _) => (0.0, olo_kit::ago::ago_secs(r.when as i64, |d| if r.stopped { olo_kit::tr!("Stopped part way, {}", ago(d)) } else { olo_kit::tr!("Tested {}", ago(d)) })),
                _ => (0.0, olo_kit::words::tr("Not tested yet.").into()),
            };
        };
        let (down, up) = (self.config.download.as_secs_f64(), self.config.upload.as_secs_f64());
        let into = running.phase_began.elapsed().as_secs_f64();
        let (done, words) = match running.phase {
            Phase::Connecting => (0.0, olo_kit::words::tr("Finding the server…").to_owned()),
            Phase::Latency => (into.min(BEFORE), olo_kit::words::tr("Measuring the round trip…").to_owned()),
            Phase::Download => (BEFORE + into.min(down), self.shown.down_now.map_or(olo_kit::words::tr("Downloading…").to_owned(), |r| olo_kit::tr!("Downloading: {}", words::rate(r)))),
            Phase::Upload => (BEFORE + down + into.min(up), self.shown.up_now.map_or(olo_kit::words::tr("Uploading…").to_owned(), |r| olo_kit::tr!("Uploading: {}", words::rate(r)))),
        };
        ((done / (BEFORE + down + up)) as f32, words)
    }

    /// The results as text, for the clipboard: every card, as it reads.
    pub fn as_text(&self) -> String {
        let mut out = String::from("Speed Test\n");
        for (title, figure, caption, facts) in self.cards() {
            out += &format!("\n{}: {figure}\n", olo_kit::words::tr(title));
            if !caption.is_empty() {
                out += &format!("  {caption}\n");
            }
            for (label, value) in facts {
                out += &format!("  {}: {value}\n", olo_kit::words::tr(label));
            }
        }
        out
    }

    /// The four cards: each its title, figure, caption and facts. (A
    /// card's facts are the same rows whatever is known, each a few
    /// words: nothing moves or is cut short as the test fills them in.)
    fn cards(&self) -> [CardWords; 4] {
        let r = self.results.as_ref();
        let idle = r.map(|r| &r.idle).filter(|l| l.samples > 0);
        let running = self.running.is_some();
        let waiting = if running { "Waiting its turn" } else { "Not measured yet" };
        let transfer = |t: Option<&Transfer>, now: Option<f64>| -> (String, String) {
            match (t, now) {
                (Some(t), _) => (words::rate(t.mbps), olo_kit::tr!("Best second {}; {} moved", words::rate(t.peak_mbps), words::bytes(t.bytes))),
                (None, Some(now)) => (words::rate(now), "Measuring…".into()),
                (None, None) => ("–".into(), waiting.into()),
            }
        };
        // A round trip while busy, and how much more than idle it is.
        let meanwhile = |t: Option<&Transfer>| -> String {
            match (t.map(|t| &t.latency).filter(|l| l.samples > 0), idle) {
                (Some(l), Some(idle)) => olo_kit::tr!("{} ({} more)", words::ms(l.median), words::ms((l.median - idle.median).max(0.0))),
                (Some(l), None) => words::ms(l.median),
                _ => "–".into(),
            }
        };
        let fact = |label: &'static str, value: String| (label, value);
        let or_dash = |v: Option<String>| v.unwrap_or_else(|| "–".into());
        let share = |pct: f64, packets: u64| olo_kit::tr!("{} ({} packets)", words::percent(pct), words::count(packets));
        let trips = |least: f64, now: f64| olo_kit::tr!("{} least, {} now", words::ms(least), words::ms(now));

        // Download: the server is the sender, so what was sent again and
        // lost is its to say; ours is what arrived, and in what order.
        let down = r.and_then(|r| r.download.as_ref());
        let (down_figure, down_caption) = transfer(down, self.shown.down_now);
        let theirs = down.and_then(|t| t.theirs.as_ref());
        let ours = self.shown.ours_down.as_ref();
        let packets_in = ours.filter(|o| o.mss > 0).map(|o| o.bytes_received / o.mss as u64);
        let download = vec![
            fact("Round trip meanwhile", meanwhile(down)),
            fact("Sent again by the server", or_dash(theirs.map(|t| share(t.retransmitted_pct(), t.retransmissions)))),
            fact("Lost, by its count", or_dash(theirs.map(|t| olo_kit::tr!("{} packets", words::count(t.lost))))),
            fact("Arrived out of order", or_dash(ours.zip(packets_in).map(|(o, n)| share(o.out_of_order as f64 * 100.0 / n.max(1) as f64, o.out_of_order as u64)))),
            fact("Server’s round trip", or_dash(theirs.map(|t| trips(t.min_rtt_ms, t.rtt_ms)))),
            fact("Server’s window", or_dash(theirs.map(|t| olo_kit::tr!("{} packets", words::count(t.cwnd))))),
            fact("Server delivers at", or_dash(theirs.map(|t| words::rate(t.delivery_mbps)))),
            fact("Connections", or_dash(down.map(|t| t.streams.to_string()))),
        ];

        // Upload: this computer is the sender, and its kernel says all of it.
        let up = r.and_then(|r| r.upload.as_ref());
        let (up_figure, up_caption) = transfer(up, self.shown.up_now);
        let ours = self.shown.ours_up.as_ref();
        let upload = vec![
            fact("Round trip meanwhile", meanwhile(up)),
            fact("Sent again", or_dash(ours.map(|o| share(o.retransmitted_pct(), o.retransmissions as u64)))),
            fact("Timeouts", or_dash(ours.map(|o| o.timeouts.to_string()))),
            fact("TCP’s round trip", or_dash(ours.map(|o| trips(o.min_rtt_ms, o.rtt_ms)))),
            fact("Sends with", or_dash(ours.map(|o| o.congestion_control.clone()))),
            fact("BBR’s model", or_dash(ours.and_then(|o| o.bbr.as_ref()).map(|b| format!("{}, {}", words::rate(b.bandwidth_mbps), phases(&b.phases))))),
            fact("Window", or_dash(ours.map(|o| olo_kit::tr!("{} packets", words::count(o.cwnd as u64))))),
            fact("Held by the server", or_dash(ours.map(|o| olo_kit::tr!("{} of the time", words::percent(o.rwnd_limited_pct))))),
        ];

        // Latency.
        let latest = self.shown.latency.back().map(|(_, v)| *v);
        let (latency_figure, latency_caption) = match (idle, latest) {
            (Some(l), _) => (words::ms(l.median), olo_kit::tr!("Idle: the middle of {} round trips", l.samples)),
            (None, Some(now)) => (words::ms(now), "Measuring…".into()),
            _ => ("–".into(), waiting.into()),
        };
        // The middle one, and from the least to what nine in ten are under.
        let spread = |l: Option<&Latency>| or_dash(l.filter(|l| l.samples > 0).map(|l| olo_kit::tr!("{} ({} to {})", words::ms(l.median), words::ms(l.min), words::ms(l.p90))));
        let latency = vec![
            fact("Idle", spread(idle)),
            fact("One differs from the next by", or_dash(idle.map(|l| words::ms(l.jitter)))),
            fact("TCP’s own, idle", or_dash(r.filter(|r| r.idle_tcp_rtt_ms > 0.0).map(|r| words::ms(r.idle_tcp_rtt_ms)))),
            fact("While downloading", spread(down.map(|t| &t.latency))),
            fact("While uploading", spread(up.map(|t| &t.latency))),
            fact("Added by a busy line", or_dash(r.and_then(Results::added_latency_ms).map(words::ms))),
            fact(
                "Responsiveness",
                or_dash(r.and_then(Results::responsiveness).map(|rpm| olo_kit::tr!("{} ({} a minute)", olo_kit::words::tr(words::responsiveness(rpm)), words::count(rpm.round() as u64)))),
            ),
        ];

        // Connection.
        let (place_figure, place_caption) = match &self.place {
            Some((_, place, _)) if !place.colo.is_empty() => (place.colo.clone(), olo_kit::tr!("{}’s data centre", self.config.server.host)),
            Some(_) => ("–".into(), self.config.server.host.clone()),
            None => ("–".into(), self.config.server.host.clone()),
        };
        let any = self.shown.ours_up.as_ref().or(self.shown.ours_down.as_ref());
        let opened = |of: fn(&Opening) -> f64| or_dash(self.place.as_ref().map(|(_, _, o)| words::ms(of(o))));
        let link = self.link.as_ref();
        let connection = vec![
            fact("Server", or_dash(self.place.as_ref().map(|(address, ..)| address.clone()))),
            fact(
                "Sees this computer in",
                or_dash(self.place.as_ref().map(|(_, p, _)| [p.city.as_str(), p.country.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty())),
            ),
            fact("Through", or_dash(self.place.as_ref().filter(|(_, p, _)| !p.asn.is_empty()).map(|(_, p, _)| format!("AS{}", p.asn)))),
            fact("Name looked up in", opened(|o| o.dns_ms)),
            fact("TCP opened in", opened(|o| o.connect_ms)),
            fact("TLS opened in", opened(|o| o.tls_ms)),
            fact("Segment size", or_dash(any.filter(|o| o.mss > 0).map(|o| olo_kit::tr!("{} bytes", words::count(o.mss as u64))))),
            fact(
                "Link",
                or_dash(link.map(|l| match &l.name {
                    Some(name) => format!("{} “{name}”", l.kind),
                    None => l.kind.clone(),
                })),
            ),
            fact("Link’s rate", or_dash(link.and_then(|l| l.rate_mbps).map(|rate| words::rate(rate as f64) + &link.and_then(|l| l.band.clone()).map(|b| format!(", {b}")).unwrap_or_default()))),
            fact("Signal", or_dash(link.and_then(|l| l.signal).map(|s| format!("{s}%")))),
        ];
        [
            ("Download", down_figure, down_caption, download),
            ("Upload", up_figure, up_caption, upload),
            ("Latency", latency_figure, latency_caption, latency),
            ("Connection", place_figure, place_caption, connection),
        ]
    }
}

/// A state the window shows, for the layout lint (lint.rs).
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub enum LintState {
    Fresh,
    Measuring(Phase),
    Finished,
    Failed,
}

#[cfg(test)]
impl Speed {
    pub fn for_lint(state: LintState) -> Speed {
        let demo = || Saved { results: Some(crate::demo::results()), link: Some(crate::demo::link()), latency: crate::demo::latency() };
        match state {
            LintState::Fresh => Speed::of(Saved::default(), false),
            LintState::Finished => Speed::of(demo(), false),
            LintState::Failed => {
                let mut speed = Speed::of(Saved::default(), false);
                speed.error = Some("speed.cloudflare.com stopped answering: the connection was reset by the peer while its answer was being read (os error 104)".into());
                speed
            }
            // Part way: what's over is there whole, what's running is
            // live, the rest waits.
            LintState::Measuring(phase) => {
                let mut speed = Speed::of(demo(), false);
                let whole = speed.results.take().expect("the demo's");
                let now = Instant::now();
                speed.running = Some(Running { which: 1, stop: Arc::new(AtomicBool::new(false)), began: now, phase, phase_began: now });
                let mut so_far = Results { stopped: true, address: whole.address.clone(), place: whole.place.clone(), ..Results::default() };
                let half = |samples: &Samples| -> Samples { Rc::new(samples.iter().take(samples.len() / 2).copied().collect()) };
                if phase == Phase::Connecting {
                    speed.place = None;
                    speed.shown = Shown::default();
                }
                if matches!(phase, Phase::Download | Phase::Upload) {
                    (so_far.idle, so_far.idle_tcp_rtt_ms) = (whole.idle.clone(), whole.idle_tcp_rtt_ms);
                }
                match phase {
                    Phase::Connecting => {}
                    Phase::Latency => speed.shown = Shown { latency: Rc::new(speed.shown.latency.iter().take(4).copied().collect()), ..Shown::default() },
                    Phase::Download => {
                        speed.shown = Shown {
                            down: half(&speed.shown.down),
                            latency: Rc::new(speed.shown.latency.iter().take(20).copied().collect()),
                            down_now: Some(911.0),
                            ours_down: speed.shown.ours_down.clone(),
                            ..Shown::default()
                        }
                    }
                    Phase::Upload => {
                        so_far.download = whole.download.clone();
                        speed.shown = Shown {
                            up: half(&speed.shown.up),
                            up_now: Some(0.512),
                            ours_down: speed.shown.ours_down.clone(),
                            ours_up: speed.shown.ours_up.clone(),
                            down: speed.shown.down.clone(),
                            down_now: speed.shown.down_now,
                            latency: Rc::new(speed.shown.latency.iter().take(60).copied().collect()),
                        };
                    }
                }
                speed.results = Some(so_far);
                speed
            }
        }
    }
}

/// The rate over the last half second of `samples`.
fn recent(samples: &Samples) -> f64 {
    let last: Vec<f64> = samples.iter().rev().take(5).map(|(_, v)| *v).collect();
    last.iter().sum::<f64>() / last.len().max(1) as f64
}

/// `samples`, each the mean of itself and the two before it.
fn smoothed(samples: &Samples) -> Samples {
    let values: Vec<f64> = samples.iter().map(|(_, v)| *v).collect();
    Rc::new(samples.iter().enumerate().map(|(i, (t, _))| (*t, values[i.saturating_sub(2)..=i].iter().sum::<f64>() / (i.min(2) + 1) as f64)).collect())
}

/// What BBR's connections are doing, each said once: "cruise", "cruise
/// and probe up".
fn phases(all: &[String]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for p in all {
        if !seen.contains(&p.as_str()) {
            seen.push(p);
        }
    }
    // (BBR's own names for what it does, as its papers name them; joined
    // by a comma, no "and" to translate, and short in its narrow cell.)
    seen.join(", ")
}

/// How long ago, in words.
fn ago(d: std::time::Duration) -> String {
    match d.as_secs() {
        0..=59 => olo_kit::words::tr("just now").into(),
        s @ 60..=3599 => olo_kit::tr!("{} min ago", s / 60),
        s @ 3600..=86_399 => olo_kit::tr!("{} h ago", s / 3600),
        s => olo_kit::tr!("{} days ago", s / 86_400),
    }
}

impl View for Speed {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        let ours = |which: u32, running: &Option<Running>| running.as_ref().is_some_and(|r| r.which == which);
        match message {
            Msg::Start => self.start(cx),
            Msg::Linked(link) => self.linked(link, cx),
            Msg::Begin => self.begin(cx),
            Msg::Never => self.asking = false,
            Msg::Stop => self.stop(),
            // (An earlier test's: old news.)
            Msg::Told(which, update) if ours(which, &self.running) => self.told(update),
            Msg::Done(which, results) if ours(which, &self.running) => {
                self.running = None;
                match results {
                    Ok(results) => {
                        let latency = (*self.shown.latency).clone();
                        self.show(*results, latency);
                        // Kept for the window's next opening.
                        if let Some(saved) = self.saved() {
                            cx.work(move || std::fs::write(saved_file(), saved).is_ok(), |_| Msg::Kept);
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            _ => return,
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "start" => cx.send(Msg::Start),
            "stop" => cx.send(Msg::Stop),
            "copy" => olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(self.as_text())),
            "close" => cx.close_window(),
            "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        self.saved()
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.stop();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = olo_kit::theme::palette();
        let running = self.running.is_some();
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let button = if running { Button::new("Stop").on_click(cx.msg(Msg::Stop)) } else { Button::new("Start Test").default(true).enabled(!self.asking).on_click(cx.msg(Msg::Start)) };
        let (fraction, doing) = self.progress();
        // (The bar only while a test runs: empty, it read as a field to
        // type in; its words then on the button's line. Opus's review,
        // 2026-11-28.)
        let progress = if running { StatusBar::new(fraction).label(doing).into_element() } else { StringView::new(doing).cut(true).into_element() };
        let top = div().h(px(STRIP)).flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(progress)).child(button);

        // Each way's graph is its own phase, start to end; the round
        // trip's is the whole test.
        let (down, up) = (self.config.download.as_secs_f64(), self.config.upload.as_secs_f64());
        let whole = self.shown.latency.back().map_or(0.0, |(t, _)| *t).max(BEFORE + down + up);
        let top_of = |samples: &Samples| olo_kit::widgets::charts::nice_ceiling(samples.iter().map(|(_, v)| *v).fold(0.0, f64::max));
        let graph = |id: &'static str, name: &'static str, hue: Hue, samples: &Samples, span: f64, unit: fn(f64) -> String| {
            let series = [Series::new(name, style::c(p.hue(hue)), samples.clone()).fill(true)];
            div().h(px(GRAPH)).flex_shrink_0().child(Graph::new(id, series, span).span(span).max(top_of(samples)).grid(span / 4.0).unit(unit).plain())
        };
        // (A rate is drawn as its mean over three tenths of a second: what
        // is acknowledged comes in bursts, a tenth with twice the line's
        // rate and the next with none.)
        let (down_shown, up_shown) = (smoothed(&self.shown.down), smoothed(&self.shown.up));
        let graphs = [
            Some(graph("down-graph", "Download", Hue::Blue, &down_shown, down, words::rate)),
            Some(graph("up-graph", "Upload", Hue::Green, &up_shown, up, words::rate)),
            Some(graph("latency-graph", "Round trip", Hue::Orange, &self.shown.latency, whole, words::ms)),
            None,
        ];
        let mut cards = Vec::new();
        let all = self.cards();
        // (Download's and Upload's values from one line, as Latency's and
        // Connection's are: each card its own, they stood 12 points apart
        // one over the other; Opus's and Neon's reviews, 2026-11-28.)
        let shared: Vec<f32> = all.chunks(2).map(|pair| facts_label_width(pair.iter().flat_map(|(.., facts)| facts.iter().map(|(label, _)| olo_kit::words::tr(label))))).collect();
        let held = cx.held_state().held();
        let narrow = card_inner(f32::from(cx.viewport_size().width), held) < FACTS_STACK_BELOW;
        for (i, ((title, figure, caption, facts), graph)) in all.into_iter().zip(graphs).enumerate() {
            let mut card = Card::new(title, title).figure(figure).caption(caption);
            if let Some(graph) = graph {
                card = card.child(graph);
            }
            if narrow {
                cards.push(card.child(stacked_facts(facts, &p)).into_element());
            } else {
                let rows: Vec<(SharedString, SharedString)> = facts.into_iter().map(|(label, value)| (label.into(), value.into())).collect();
                cards.push(card.child(Facts::new(rows).label_width(shared[i / 2])).into_element());
            }
        }
        // Two rows of two: the cards of a row are as tall as each other.
        // Narrower (held so by tiling, a third of a screen: their facts'
        // words were cut a third short, QA 2026-11-28), one under another.
        let mut board = div().flex().flex_col().gap(px(dashboard::GAP)).p(px(dashboard::GAP)).bg(style::c(dashboard::gutter(&p)));
        if f32::from(cx.viewport_size().width) < TWO_ACROSS {
            board = board.children(cards);
        } else {
            let mut cards = cards.into_iter();
            let half = |card: Element| div().flex_1().min_w_0().flex().child(card);
            let mut row = || div().flex().gap(px(dashboard::GAP)).child(half(cards.next().expect("four"))).child(half(cards.next().expect("four")));
            board = board.child(row()).child(row());
        }
        let note = "Against Cloudflare’s speed test servers: four connections each way, eight seconds each.";
        // (Wrapped to the window: one line, it ran past a narrow one.)
        let note = StringView::new(note).dim(true).wrap(true);
        if held {
            // Held at a size of another's choosing (shorter than the
            // cards, a tile's): the menu bar and the strip with Start Test
            // stay, the cards and the note scroll under them, their bar
            // against the cards, its right edge Start Test's (the window's
            // own scroll ran up beside the menus, Start Test scrolled away;
            // then the bar stood 16 points off the cards, at the window's
            // edge: Opus's reviews, 2026-11-28).
            let strip = div().px(px(m::WINDOW_INSET)).pt(px(m::WINDOW_INSET)).pb(px(m::SPACING)).child(top);
            let scrolled = div().flex().flex_col().gap(px(m::SPACING)).child(board).child(note);
            let cards = ScrollView::new(&self.scroll).border(false).background(p.panel_background).child(scrolled);
            // (As tall as the window, said: laid out at its own height, a
            // third of a screen had 55 points empty under the cards.)
            let tall = f32::from(cx.viewport_size().height);
            return div().w_full().h(px(tall)).flex().flex_col().child(bar).child(strip).child(div().flex_1().min_h_0().px(px(m::WINDOW_INSET)).pb(px(m::WINDOW_INSET)).child(cards)).into_element();
        }
        let content = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(top).child(board).child(note);
        // (As wide as its window, which is `WIDTH`: a window given
        // another width has its bar and cards across all of it.)
        div().w_full().flex().flex_col().child(bar).child(content).into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card's facts stacked, label over value, where it's narrow: a
    /// third of a screen; beside each other at its own width and in a
    /// quarter (one column there, its cards the window's width).
    #[test]
    fn narrow_cards_stack_their_facts() {
        assert!(card_inner(349.0, true) < FACTS_STACK_BELOW, "a third");
        assert!(card_inner(530.0, true) >= FACTS_STACK_BELOW, "a quarter");
        assert!(card_inner(WIDTH, false) >= FACTS_STACK_BELOW, "its own width, two across");
        assert_eq!(DESIGN_HEIGHT, 220.0, "measured, held, at what it's given, never 700");
    }

    #[test]
    fn small_things_are_said() {
        assert_eq!(phases(&["cruise".into(), "probe up".into(), "cruise".into()]), "cruise, probe up");
        let burst: Samples = Rc::new([(0.1, 0.0), (0.2, 200.0), (0.3, 0.0), (0.4, 100.0)].into_iter().collect());
        assert_eq!(smoothed(&burst).iter().map(|(_, v)| v.round()).collect::<Vec<_>>(), [0.0, 100.0, 67.0, 100.0]);
        assert_eq!([ago(std::time::Duration::from_secs(5)), ago(std::time::Duration::from_secs(190)), ago(std::time::Duration::from_secs(7300))], ["just now", "3 min ago", "2 h ago"]);
    }
}
