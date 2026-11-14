//! Playing a film on a receiver by its address: the receiver is told
//! where the film is, fetches it and plays it itself, so the picture
//! and the sound are the file's own, untouched ([`Play::begin`]). A file
//! on this computer is served to it for the length of the play
//! (`serve`); something on the web is played from where it is.
//!
//! How a play is asked for is AirPlay 2's, which isn't published: this
//! follows what pyatv, which plays to Apple TVs, has found, in both the
//! ways it has known. *The newer* (what tvOS 26 and 27 take): after the
//! session is set up (SETUP, the event channel, RECORD) a stream for
//! commands is set up and the film is put in the receiver's queue by a
//! command, started by another, and the receiver says how its player is
//! doing on the event channel. *The older* (`POST /play`, then asking
//! `/playback-info` every second) is fallen back to when a receiver
//! doesn't take the newer.
//!
//! The session is kept alive by a word every two seconds (`/feedback`),
//! as Apple's senders do; it ends when the film does, when the receiver
//! goes on to something else, or when the [`Play`] is dropped.

pub(crate) mod events;
#[doc(hidden)]
pub mod fake;
pub(crate) mod lists;
mod media;
pub(crate) mod rtsp;
mod serve;
pub(crate) mod timing;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use ::plist::Value;

pub use self::media::{Kind, kind_of};
use self::{
    events::{Event, Events},
    lists as pl,
    rtsp::{BPLIST, Rtsp},
    serve::Served,
    timing::Timing,
};
use crate::pairing::{self, Answer, Cipher, Closer, Credentials, Http, hap::random, verify};

const EVENTS_SALT: &str = "Events-Salt";
/// What this sender says it is, where a receiver may mind: as pyatv
/// says, which receivers are known to take.
const AGENT: &str = "AirPlay/550.10";
const COMMAND_AGENT: &str = "AirPlay/870.14.1";
/// The kind of client a stream of commands is for.
const REMOTE_CONTROL: &str = "A6B27562-B43A-4F2D-B75F-82391E250194";

/// What is to be played.
#[derive(Debug, Clone, PartialEq)]
pub enum What {
    /// A file on this computer: served to the receiver while it plays.
    File(PathBuf),
    /// Something on the web (http or https), which the receiver fetches
    /// from there.
    Address(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The receiver couldn't be reached, isn't the one paired with, or
    /// doesn't know us ([`pairing::Error`]).
    Receiver(pairing::Error),
    /// The file can't be read.
    File(String),
    /// Not something a receiver plays by itself: why.
    CantPlay(&'static str),
    /// Not an address a receiver can fetch.
    Address(String),
    /// The receiver refused a step of the play: which, and its status.
    Refused(&'static str, u16),
    /// It answered a step with something that isn't that step's answer.
    Unexpected(&'static str),
}

impl From<pairing::Error> for Error {
    fn from(e: pairing::Error) -> Error {
        Error::Receiver(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Receiver(e) => write!(f, "{e}"),
            Error::File(e) => write!(f, "{e}"),
            Error::CantPlay(why) => write!(f, "{why}"),
            Error::Address(a) => write!(f, "{a} isn't an address a receiver can fetch (http:// or https://)"),
            Error::Refused(step, 403) => write!(f, "the receiver refused {step} (403: it doesn't take it from this sender)"),
            Error::Refused(step, status) => write!(f, "the receiver refused {step} ({status})"),
            Error::Unexpected(what) => write!(f, "the receiver answered with something unexpected ({what})"),
        }
    }
}

impl std::error::Error for Error {}

/// Told each thing said to the receiver and by it.
pub type Trace = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    /// Where in the film to begin, in seconds.
    pub from: f64,
    /// What the receiver is to know this sender as.
    pub name: String,
    /// How long the receiver is waited on for each answer.
    pub wait: Duration,
    /// How often it is told the session is alive (Apple's senders: 2 s).
    pub feedback: Duration,
    /// How long it has to begin playing before it's given up on.
    pub start: Duration,
    /// For finding out why a receiver won't play: each step and what it
    /// said to it, and each thing it says by itself.
    pub trace: Option<Trace>,
}

impl Default for Options {
    fn default() -> Options {
        Options { from: 0.0, name: "Olo".into(), wait: Duration::from_secs(8), feedback: Duration::from_secs(2), start: Duration::from_secs(30), trace: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Asked for; the receiver is fetching it (or fetching more).
    Loading,
    Playing,
    Paused,
    /// It played to its end.
    Ended,
    /// The receiver left it: stopped there, or gone on to something
    /// else, or gone.
    Left,
    /// It never played: [`Now::why`].
    Failed,
}

impl State {
    /// Nothing more comes of this play.
    pub fn over(self) -> bool {
        matches!(self, State::Ended | State::Left | State::Failed)
    }
}

/// How a play is doing.
#[derive(Debug, Clone)]
pub struct Now {
    pub state: State,
    /// Where in the film it was when this was learnt, in seconds
    /// ([`Now::position_now`] is where it is by now).
    pub position: Option<f64>,
    pub duration: Option<f64>,
    /// How fast it plays (1: as filmed; 0: not).
    pub rate: f64,
    /// Why, where the state wants saying why.
    pub why: Option<String>,
    learnt: Instant,
}

impl PartialEq for Now {
    fn eq(&self, other: &Now) -> bool {
        (self.state, self.position, self.duration, self.rate, &self.why) == (other.state, other.position, other.duration, other.rate, &other.why)
    }
}

impl Now {
    /// Asked for, to begin at `from` (which is where it is until the
    /// receiver says otherwise).
    fn loading(from: f64) -> Now {
        Now { state: State::Loading, position: (from > 0.0).then_some(from), duration: None, rate: 0.0, why: None, learnt: Instant::now() }
    }

    /// Where in the film it is by now: a receiver says where it is when
    /// it starts and when that changes, not as it goes.
    pub fn position_now(&self) -> Option<f64> {
        let at = self.position?;
        let at = if self.state == State::Playing { at + self.learnt.elapsed().as_secs_f64() * self.rate } else { at };
        Some(self.duration.map_or(at, |d| at.min(d)))
    }
}

struct Shared {
    now: Mutex<Now>,
    changed: Condvar,
    /// Asked to stop: no more is said to the receiver but goodbye.
    stop: AtomicBool,
    /// It has played, so its stopping is the film's end and not its
    /// beginning.
    started: AtomicBool,
    trace: Option<Trace>,
}

impl Shared {
    fn say(&self, text: impl FnOnce() -> String) {
        if let Some(trace) = &self.trace {
            trace(&text());
        }
    }

    fn set(&self, change: impl FnOnce(&mut Now)) {
        let mut now = self.now.lock().unwrap();
        if now.state.over() {
            return;
        }
        let before = now.clone();
        change(&mut now);
        if *now != before {
            self.changed.notify_all();
        }
    }

    fn over(&self, state: State, why: Option<String>) {
        self.set(|now| {
            (now.position, now.learnt) = (now.position_now(), Instant::now());
            (now.state, now.rate, now.why) = (state, 0.0, why);
        });
    }

    fn stopping(&self) -> bool {
        self.stop.load(Relaxed)
    }
}

/// The control connection and what is said on it, by whichever way the
/// receiver took.
struct Control {
    rtsp: Rtsp,
    session: String,
    /// The newer way's stream of commands; None: the older way.
    stream: Option<String>,
}

impl Control {
    fn step(&mut self, method: &str, uri: Option<&str>, body: Option<&Value>) -> Result<Answer, pairing::Error> {
        self.rtsp.send(method, uri, "RTSP/1.0", AGENT, &[], body)
    }

    /// A command to the receiver's player (the newer way).
    fn command(&mut self, command: &Value) -> Result<Answer, pairing::Error> {
        let body = pl::dict([("params", pl::dict([("data", Value::Data(pl::write(command)))]))]);
        let stream = self.stream.as_deref().unwrap_or("1");
        self.rtsp.send("POST", Some("/command"), "HTTP/1.1", COMMAND_AGENT, &[("X-Apple-ProtocolVersion", "1"), ("X-Apple-Session-ID", &self.session), ("X-Apple-StreamID", stream)], Some(&body))
    }

    fn rate(&mut self, rate: f64) -> Result<Answer, pairing::Error> {
        match self.stream {
            Some(_) => self.command(&pl::dict([("type", Value::from("setRate")), ("rate", Value::from(rate))])),
            None => self.step("POST", Some(&format!("/rate?value={rate:.6}")), None),
        }
    }
}

/// A film playing on a receiver. Dropped, it's stopped there.
pub struct Play {
    shared: Arc<Shared>,
    control: Arc<Mutex<Control>>,
    closer: Option<Closer>,
    keep: Option<JoinHandle<()>>,
    events: Option<Events>,
    served: Option<Served>,
    timing: Option<Timing>,
}

/// A play's controls for another thread (a signal's): it wakes whoever
/// waits in [`Play::next`], and pauses.
#[derive(Clone)]
pub struct Remote {
    shared: Arc<Shared>,
    control: Arc<Mutex<Control>>,
}

impl Remote {
    /// Whoever waits in [`Play::next`] is woken, to stop the play.
    pub fn interrupt(&self) {
        let _now = self.shared.now.lock().unwrap();
        self.shared.stop.store(true, Relaxed);
        self.shared.changed.notify_all();
    }

    /// Pause it, or go on (the receiver says when it has).
    pub fn pause(&self, paused: bool) -> Result<(), Error> {
        let answer = self.control.lock().unwrap().rate(if paused { 0.0 } else { 1.0 })?;
        if answer.status < 300 { Ok(()) } else { Err(Error::Refused(if paused { "to pause" } else { "to go on" }, answer.status)) }
    }
}

impl Play {
    /// Ask the receiver at `at`, paired with (`credentials`), to play
    /// `what`. It returns when the receiver has taken the request: the
    /// film is then loading ([`Play::next`] says how it goes on). It
    /// blocks for as long as the receiver takes to answer each step:
    /// off a looper.
    pub fn begin(at: SocketAddr, credentials: &Credentials, what: &What, options: &Options) -> Result<Play, Error> {
        // What it is, before anything is said to anyone.
        let file = match what {
            What::File(path) => Some((path.clone(), media::kind_of_file(path)?)),
            What::Address(a) if a.starts_with("http://") || a.starts_with("https://") => None,
            What::Address(a) => return Err(Error::Address(a.clone())),
        };
        let shared =
            Arc::new(Shared { now: Mutex::new(Now::loading(options.from)), changed: Condvar::new(), stop: AtomicBool::new(false), started: AtomicBool::new(false), trace: options.trace.clone() });
        let told = |step: &str, answer: &Answer| shared.say(|| format!("{step}: {}{}", answer.status, pl::read(&answer.body).map(|v| format!(" {}", brief(&v))).unwrap_or_default()));
        let refused = |step: &'static str, answer: &Answer| if answer.status < 300 { Ok(()) } else { Err(Error::Refused(step, answer.status)) };

        // The connection, each side proved to the other, enciphered.
        let mut http = Http::connect(at, options.wait)?;
        let session = verify(&mut http, credentials)?;
        http.encipher(session.control());
        shared.say(|| "connected, and each side proved".to_owned());
        let (Some(local), Some(peer)) = (http.local(), http.peer()) else { return Err(Error::Unexpected("a connection without its ends")) };
        let timing = Timing::start(local.ip()).map_err(|e| Error::Receiver(pairing::Error::Io(e.to_string())))?;
        let closer = http.closer();
        let mut control = Control { rtsp: Rtsp::new(http), session: uuid(), stream: None };

        // The session.
        let sender = sender_id();
        let setup = control.step(
            "SETUP",
            None,
            Some(&pl::dict([
                ("deviceID", Value::from(sender.as_str())),
                ("sessionUUID", Value::from(control.session.as_str())),
                ("sessionCorrelationUUID", Value::from(uuid())),
                ("timingPort", Value::from(i64::from(timing.port))),
                ("timingProtocol", Value::from("NTP")),
                ("isMultiSelectAirPlay", Value::from(true)),
                ("groupContainsGroupLeader", Value::from(false)),
                ("macAddress", Value::from(sender.as_str())),
                ("model", Value::from("iPhone14,3")),
                ("name", Value::from(options.name.as_str())),
                ("osBuildVersion", Value::from("20F66")),
                ("osName", Value::from("iPhone OS")),
                ("osVersion", Value::from("16.5")),
                ("senderSupportsRelay", Value::from(false)),
                ("sourceVersion", Value::from("690.7.1")),
                ("statsCollectionEnabled", Value::from(false)),
            ])),
        )?;
        told("SETUP", &setup);
        refused("the session", &setup)?;
        let event_port = pl::read(&setup.body)
            .and_then(|v| pl::at(&v, &["eventPort"]).and_then(pl::number))
            .filter(|p| (1.0..65536.0).contains(p))
            .ok_or(Error::Unexpected("no event port in its answer to SETUP"))? as u16;

        // The event channel: its keys are named from the receiver's side
        // (what it writes, we read).
        let event_keys = || Cipher::new(&session.key(EVENTS_SALT, "Events-Read-Encryption-Key"), &session.key(EVENTS_SALT, "Events-Write-Encryption-Key"));
        let mut tries = 0;
        let events = loop {
            let hearer = shared.clone();
            match Events::connect(SocketAddr::new(peer.ip(), event_port), event_keys(), options.wait, move |event| heard(&hearer, event)) {
                Ok(events) => break events,
                // (Some receivers open the port a moment after naming it.)
                Err(_) if tries < 5 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(e) => return Err(Error::Receiver(pairing::Error::Io(format!("its event channel: {e}")))),
            }
        };

        // Where the film is, for the receiver.
        let (address, served) = match (&file, what) {
            (Some((path, kind)), _) => {
                let served = Served::start(path, *kind, local.ip()).map_err(|e| Error::File(format!("the film couldn't be served: {e}")))?;
                (served.url.clone(), Some(served))
            }
            (None, What::Address(a)) => (a.clone(), None),
            (None, What::File(_)) => unreachable!("a file has its kind"),
        };

        told("GET /info", &control.step("GET", Some("/info"), None)?);
        let record = control.step("RECORD", None, None)?;
        told("RECORD", &record);
        refused("the session's start", &record)?;

        // The newer way: a stream of commands, and the film put in the
        // receiver's queue.
        let streams = control.step(
            "SETUP",
            None,
            Some(&pl::dict([(
                "streams",
                Value::Array(vec![pl::dict([
                    ("clientUUID", Value::from(uuid())),
                    ("clientTypeUUID", Value::from(REMOTE_CONTROL)),
                    ("channelID", Value::from(format!("{sender}-RCS-1"))),
                    ("controlType", Value::from(1i64)),
                    ("type", Value::from(130i64)),
                ])]),
            )])),
        )?;
        told("SETUP of a stream of commands", &streams);
        control.stream = (streams.status < 300).then(|| pl::read(&streams.body)).flatten().and_then(|v| {
            let id = v.as_dictionary()?.get("streams")?.as_array()?.first()?.as_dictionary()?.get("streamID")?;
            id.as_string().map(str::to_owned).or_else(|| pl::number(id).map(|n| (n as i64).to_string()))
        });
        let item = uuid();
        if control.stream.is_some() {
            let mut film = vec![
                ("uuid", Value::from(item.as_str())),
                ("mediaType", Value::from("file")),
                ("Content-Location", Value::from(address.as_str())),
                ("Start-Position-Seconds", Value::from(options.from)),
            ];
            if options.from > 0.0 {
                // (tvOS 27 reads where to begin from this one.)
                film.push(("Start-Position", pl::time(options.from)));
            }
            let film = Value::Dictionary(film.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
            let put = control.command(&pl::dict([("type", Value::from("insertPlayQueueItem")), ("item", film)]))?;
            told("insertPlayQueueItem", &put);
            if put.status >= 300 {
                shared.say(|| "it doesn't take the film that way: the older way".to_owned());
                control.stream = None;
            }
        }
        if control.stream.is_some() {
            let about = pl::dict([("uuid", Value::from(item.as_str()))]);
            for (name, value, of) in [("isInterestedInDateRange", Value::from(true), Some(about)), ("actionAtItemEnd", Value::from(1i64), None)] {
                let mut property = vec![("type".to_owned(), Value::from("setProperty")), ("value".to_owned(), value), ("property".to_owned(), Value::from(name))];
                property.extend(of.map(|of| ("item".to_owned(), of)));
                told(name, &control.command(&Value::Dictionary(property.into_iter().collect()))?);
            }
            let rate = control.rate(1.0)?;
            told("setRate", &rate);
            refused("to start the film", &rate)?;
        } else {
            ask_the_older_way(&mut control, &address, options.from, &told)?;
        }

        let older = control.stream.is_none();
        let control = Arc::new(Mutex::new(control));
        let sent = served.as_ref().map(|s| Box::new(s.counter()) as Box<dyn Fn() -> u64 + Send>);
        let (theirs, with, options) = (shared.clone(), control.clone(), options.clone());
        let keep =
            std::thread::Builder::new().name("airplay-keep".into()).spawn(move || keep(&theirs, &with, &options, older, sent)).map_err(|e| Error::Receiver(pairing::Error::Io(e.to_string())))?;
        Ok(Play { shared, control, closer, keep: Some(keep), events: Some(events), served, timing: Some(timing) })
    }

    /// How it is doing.
    pub fn now(&self) -> Now {
        self.shared.now.lock().unwrap().clone()
    }

    /// How it is doing once that differs from `seen`: waits for it, for
    /// `wait` at most, or until interrupted.
    pub fn next(&self, seen: &Now, wait: Option<Duration>) -> Now {
        let deadline = wait.map(|w| Instant::now() + w);
        let mut now = self.shared.now.lock().unwrap();
        loop {
            if *now != *seen || self.shared.stopping() {
                return now.clone();
            }
            now = match deadline {
                None => self.shared.changed.wait(now).unwrap(),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return now.clone();
                    }
                    self.shared.changed.wait_timeout(now, left).unwrap().0
                }
            };
        }
    }

    pub fn remote(&self) -> Remote {
        Remote { shared: self.shared.clone(), control: self.control.clone() }
    }

    pub fn interrupted(&self) -> bool {
        self.shared.stopping()
    }

    /// Pause it, or go on (the receiver says when it has).
    pub fn pause(&self, paused: bool) -> Result<(), Error> {
        self.remote().pause(paused)
    }

    /// How many bytes of the file the receiver has fetched (None: it
    /// plays from the web).
    pub fn fetched(&self) -> Option<u64> {
        self.served.as_ref().map(Served::sent)
    }

    /// Stop it: the receiver leaves the film.
    pub fn stop(self) {}
}

impl Drop for Play {
    fn drop(&mut self) {
        {
            let _now = self.shared.now.lock().unwrap();
            self.shared.stop.store(true, Relaxed);
            self.shared.changed.notify_all();
        }
        // Goodbye, if the connection is free to say it on (it isn't
        // while the receiver is being waited on: closed, then).
        if let Ok(mut control) = self.control.try_lock() {
            control.rtsp.http.wait(Duration::from_secs(1));
            if let Ok(answer) = control.step("TEARDOWN", None, None) {
                self.shared.say(|| format!("TEARDOWN: {}", answer.status));
            }
        }
        if let Some(closer) = &self.closer {
            closer.close();
        }
        if let Some(keep) = self.keep.take() {
            let _ = keep.join();
        }
        (self.events, self.served, self.timing) = (None, None, None);
    }
}

/// The older way of asking: `/play` with the address, then the
/// properties and the rate pyatv has found Apple's senders to set
/// (without the rate it stays paused).
fn ask_the_older_way(control: &mut Control, address: &str, from: f64, told: &dyn Fn(&str, &Answer)) -> Result<(), Error> {
    let body = pl::write(&pl::dict([
        ("Content-Location", Value::from(address)),
        ("Start-Position-Seconds", Value::from(from)),
        ("uuid", Value::from(uuid().to_lowercase())),
        ("streamType", Value::from(1i64)),
        ("mediaType", Value::from("file")),
        ("mightSupportStorePastisKeyRequests", Value::from(true)),
        ("playbackRestrictions", Value::from(0i64)),
        ("secureConnectionMs", Value::from(22i64)),
        ("volume", Value::from(1.0)),
        ("infoMs", Value::from(122i64)),
        ("connectMs", Value::from(18i64)),
        ("authMs", Value::from(0i64)),
        ("bonjourMs", Value::from(0i64)),
        ("referenceRestrictions", Value::from(3i64)),
        ("SenderMACAddress", Value::from(sender_id())),
        ("model", Value::from("iPhone14,3")),
        ("postAuthMs", Value::from(0i64)),
        ("clientBundleID", Value::from("org.olo.airplay")),
        ("clientProcName", Value::from("org.olo.airplay")),
        ("osBuildVersion", Value::from("20G1116")),
        ("rate", Value::from(1.0)),
    ]));
    let (length, session) = (body.len().to_string(), uuid().to_lowercase());
    let head =
        [("User-Agent", AGENT), ("Content-Length", length.as_str()), ("Content-Type", BPLIST), ("X-Apple-ProtocolVersion", "1"), ("X-Apple-Session-ID", session.as_str()), ("X-Apple-Stream-ID", "1")];
    let mut tries = 0;
    loop {
        let answer = control.rtsp.http.exchange("POST /play HTTP/1.1", &head, &body)?;
        told("POST /play", &answer);
        match answer.status {
            // (A receiver sometimes answers 500 and then takes it.)
            500 if tries < 2 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
            status if status >= 400 => return Err(Error::Refused("to play it", status)),
            _ => break,
        }
    }
    let never = pl::dict([("value", pl::dict([("flags", Value::from(0i64)), ("value", Value::from(0i64)), ("epoch", Value::from(0i64)), ("timescale", Value::from(0i64))]))]);
    let steps = [
        ("PUT", "/setProperty?isInterestedInDateRange", Some(pl::dict([("value", Value::from(true))]))),
        ("PUT", "/setProperty?actionAtItemEnd", Some(pl::dict([("value", Value::from(0i64))]))),
        ("POST", "/rate?value=1.000000", None),
        ("PUT", "/setProperty?forwardEndTime", Some(never.clone())),
        ("PUT", "/setProperty?reverseEndTime", Some(never)),
    ];
    for (method, uri, body) in steps {
        told(uri, &control.step(method, Some(uri), body.as_ref())?);
    }
    Ok(())
}

/// Something the receiver said on the event channel.
fn heard(shared: &Shared, event: Event) {
    let said = match event {
        Event::Said(said) => said,
        Event::Closed => {
            if !shared.stopping() {
                shared.over(State::Left, Some("the receiver closed the session".into()));
            }
            return;
        }
    };
    shared.say(|| format!("it says: {}", brief(&said)));
    let params = pl::at(&said, &["params"]);
    let time = |key: &str| params.and_then(|p| pl::at(p, &[key])).or_else(|| pl::at(&said, &[key])).and_then(pl::seconds);
    match pl::text(&said, &["type"]) {
        Some("playbackState") => {
            if let Some(error) = params.and_then(|p| pl::at(p, &["error"])) {
                return shared.over(State::Failed, Some(format!("the receiver's player gave an error ({})", brief(error))));
            }
            let state = params.and_then(|p| pl::text(p, &["playbackState"])).or_else(|| pl::text(&said, &["name"])).unwrap_or_default().to_lowercase();
            match state.as_str() {
                "playing" => {
                    shared.started.store(true, Relaxed);
                    shared.set(|now| {
                        now.position = time("position").or_else(|| now.position_now()).or(Some(0.0));
                        now.duration = time("duration").filter(|d| *d > 0.0).or(now.duration);
                        (now.state, now.rate, now.learnt) = (State::Playing, time("rate").filter(|r| *r > 0.0).unwrap_or(1.0), Instant::now());
                    });
                }
                "paused" => shared.set(|now| {
                    now.position = time("position").or_else(|| now.position_now());
                    (now.state, now.rate, now.learnt) = (State::Paused, 0.0, Instant::now());
                }),
                "loading" => shared.set(|now| {
                    now.position = now.position_now();
                    (now.state, now.rate, now.learnt) = (State::Loading, 0.0, Instant::now());
                }),
                // (Before it has played, "stopped" is where it starts from.)
                "stopped" | "idle" | "ended" if shared.started.load(Relaxed) => {
                    let now = shared.now.lock().unwrap().clone();
                    let at_its_end = now.position_now().zip(now.duration).is_some_and(|(at, all)| at >= all - 2.0);
                    let ended = state == "ended" || params.and_then(|p| pl::text(p, &["reason"])) == Some("ended") || at_its_end;
                    shared.over(if ended { State::Ended } else { State::Left }, (!ended).then(|| "it was stopped at the receiver".to_owned()));
                }
                _ => {}
            }
        }
        // (Where it is, when that jumps: begun from a place, or sought.)
        Some("notification") if pl::text(&said, &["name"]) == Some("timeJumped") || params.and_then(|p| pl::text(p, &["name"])) == Some("timeJumped") => {
            if let Some(at) = time("position") {
                shared.set(|now| (now.position, now.learnt) = (Some(at), Instant::now()));
            }
        }
        _ => {}
    }
}

/// The session kept alive while the film plays, and (the older way) the
/// receiver asked every second how it goes; given up on if it never
/// starts.
fn keep(shared: &Shared, control: &Mutex<Control>, options: &Options, older: bool, sent: Option<Box<dyn Fn() -> u64 + Send>>) {
    let begun = Instant::now();
    let tick = if older { options.feedback.min(Duration::from_secs(1)) } else { options.feedback };
    let mut fed = Instant::now();
    loop {
        // Asleep for a tick, or until it's over or stopped.
        let wake = Instant::now() + tick;
        let mut now = shared.now.lock().unwrap();
        loop {
            if shared.stopping() || now.state.over() {
                return;
            }
            let left = wake.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            now = shared.changed.wait_timeout(now, left).unwrap().0;
        }
        drop(now);
        let mut control = control.lock().unwrap();
        if shared.stopping() {
            return;
        }
        let lost = |e: pairing::Error| {
            if !shared.stopping() {
                shared.over(State::Left, Some(format!("the session was lost: {e}")));
            }
        };
        if fed.elapsed() >= options.feedback {
            fed = Instant::now();
            if let Err(e) = control.step("POST", Some("/feedback"), None) {
                return lost(e);
            }
        }
        if older {
            let info = match control.rtsp.http.exchange("GET /playback-info HTTP/1.1", &[("User-Agent", AGENT)], &[]) {
                Ok(answer) if answer.status >= 400 => return shared.over(State::Failed, Some(format!("the receiver doesn't say how the play goes ({})", answer.status))),
                Ok(answer) => pl::read(&answer.body),
                Err(e) => return lost(e),
            };
            let get = |key: &str| info.as_ref().and_then(|i| pl::at(i, &[key])).and_then(pl::seconds);
            if let Some(error) = info.as_ref().and_then(|i| pl::at(i, &["error"])) {
                return shared.over(State::Failed, Some(format!("the receiver's player gave an error ({})", brief(error))));
            }
            match get("duration") {
                // (It says how long the film is only while it has one.)
                Some(duration) => {
                    shared.started.store(true, Relaxed);
                    let rate = get("rate").unwrap_or(1.0);
                    shared.set(|now| {
                        (now.position, now.duration, now.rate, now.learnt) = (get("position").or(Some(0.0)), Some(duration), rate, Instant::now());
                        now.state = if rate > 0.0 { State::Playing } else { State::Paused };
                    });
                }
                None if shared.started.load(Relaxed) => return shared.over(State::Ended, None),
                None => {}
            }
        }
        drop(control);
        if !shared.started.load(Relaxed) && begun.elapsed() >= options.start {
            let why = match sent.as_ref().map(|sent| sent()) {
                Some(0) => "the receiver never came for the film (is a firewall on this computer in its way?)",
                Some(_) => "the receiver fetched the film and didn't play it (not a film of a kind it plays?)",
                None => "the receiver didn't play it (can it reach that address, and is it a film of a kind it plays?)",
            };
            return shared.over(State::Failed, Some(why.to_owned()));
        }
    }
}

/// A property list in a line, for the trace: not all of a long one.
fn brief(value: &Value) -> String {
    let mut text = pl::line(value);
    if text.len() > 700 {
        let mut end = 700;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn uuid() -> String {
    let mut b = random::<16>();
    (b[6], b[8]) = (b[6] & 0x0f | 0x40, b[8] & 0x3f | 0x80);
    let h: String = b.iter().map(|b| format!("{b:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// An address for this sender to go by, as a network card's is written:
/// made up for each play (a locally-made one, by its second bit).
fn sender_id() -> String {
    let mut b = random::<6>();
    b[0] = b[0] & 0xfc | 0x02;
    b.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::pairing::{Setup, fake::Receiver};

    /// A receiver of the test's own, paired with.
    fn paired() -> (Arc<Mutex<fake::Tv>>, Credentials) {
        let mut receiver = Receiver::new("1234");
        let credentials = Setup::begin(&mut receiver).unwrap().finish(&mut receiver, "1234", "Olo").unwrap().credentials;
        (Arc::new(Mutex::new(fake::Tv::new(Arc::new(Mutex::new(receiver))))), credentials)
    }

    /// A made-up film (an MP4 by how it begins) in a folder of the
    /// test's own.
    fn film(name: &str) -> (PathBuf, Vec<u8>) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(format!("/var/tmp/olo-airplay-play-test-{}-{}", std::process::id(), N.fetch_add(1, Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41".to_vec();
        bytes.extend((0..40_000u32).map(|i| (i % 253) as u8));
        std::fs::write(dir.join(name), &bytes).unwrap();
        (dir.join(name), bytes)
    }

    fn quick() -> Options {
        Options { name: "Olo of the test".into(), wait: Duration::from_secs(5), feedback: Duration::from_millis(50), start: Duration::from_millis(1500), ..Options::default() }
    }

    /// The states it goes through until it's over.
    fn watched(play: &Play) -> Vec<Now> {
        let mut seen = vec![play.now()];
        while !seen.last().unwrap().state.over() {
            let next = play.next(seen.last().unwrap(), Some(Duration::from_secs(10)));
            assert_ne!(&next, seen.last().unwrap(), "nothing came of it");
            seen.push(next);
        }
        seen
    }

    /// The states gone through, after its loading (which may be over
    /// before the asking returns: the receiver plays when it's ready).
    fn states(seen: &[Now]) -> Vec<State> {
        let mut states: Vec<State> = seen.iter().map(|n| n.state).skip_while(|s| *s == State::Loading).collect();
        states.dedup();
        states
    }

    fn gone(address: &str) -> bool {
        let host = address.strip_prefix("http://").unwrap().split('/').next().unwrap();
        std::net::TcpStream::connect(host).is_err()
    }

    /// The newer way, from the session's setting up to the film's end:
    /// each step asked in its order, the film fetched by the receiver
    /// whole and in part from an address only it was told, the receiver's
    /// news heard and answered, the session kept alive, and all of it
    /// gone at the end.
    #[test]
    fn a_film_played_to_its_end() {
        let (tv, credentials) = paired();
        let (path, bytes) = film("A film.mp4");
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &quick()).unwrap();
        let seen = watched(&play);
        assert_eq!(states(&seen), [State::Playing, State::Ended]);
        let playing = seen.iter().find(|n| n.state == State::Playing).unwrap();
        assert_eq!((playing.position, playing.duration, playing.rate), (Some(0.0), Some(600.0), 1.0));
        assert_eq!(play.fetched(), Some(bytes.len() as u64 + 10));
        drop(play);
        let tv = tv.lock().unwrap();
        assert_eq!(tv.asked, ["SETUP", "GET /info", "RECORD", "SETUP streams", "command insertPlayQueueItem", "command setProperty", "command setProperty", "command setRate", "TEARDOWN"]);
        assert_eq!(tv.sender, "Olo of the test");
        assert!(tv.timed, "its question of the time was answered");
        assert!(tv.fetched == bytes && tv.part == bytes[10..20], "it fetched the film, and a part of it");
        assert_eq!(tv.kind, "video/mp4");
        assert_eq!((tv.from, tv.from_time), (0.0, None));
        // What it said (what it is, loading, playing, stopped) was
        // answered; it was told the session lives, more than once.
        assert_eq!(tv.answered, 4);
        assert!(tv.feedbacks >= 2, "{}", tv.feedbacks);
        // The address: on the loopback as the connection was, unguessable,
        // not the file's name; and nothing answers there now.
        let address = tv.address.as_deref().unwrap();
        assert!(address.starts_with("http://127.0.0.1:") && address.ends_with("/film.mp4") && !address.contains("A%20film"), "{address}");
        assert!(gone(address));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A receiver that takes only the older way is asked that way: the
    /// film from a place in it, and how it goes asked each second.
    #[test]
    fn the_older_way_from_a_place_in_the_film() {
        let (tv, credentials) = paired();
        tv.lock().unwrap().commands = false;
        let (path, bytes) = film("old.mp4");
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &Options { from: 12.5, ..quick() }).unwrap();
        let seen = watched(&play);
        assert_eq!(states(&seen), [State::Playing, State::Ended]);
        let playing = seen.iter().find(|n| n.state == State::Playing).unwrap();
        assert!(playing.position.unwrap() >= 12.5 && playing.duration == Some(600.0));
        drop(play);
        let tv = tv.lock().unwrap();
        assert_eq!(tv.asked[..5], ["SETUP", "GET /info", "RECORD", "SETUP streams", "POST /play"]);
        assert_eq!(
            tv.asked[5..],
            [
                "PUT /setProperty?isInterestedInDateRange",
                "PUT /setProperty?actionAtItemEnd",
                "POST /rate?value=1.000000",
                "PUT /setProperty?forwardEndTime",
                "PUT /setProperty?reverseEndTime",
                "TEARDOWN"
            ]
        );
        assert_eq!(tv.from, 12.5);
        assert!(tv.fetched == bytes);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// From a place, the newer way: said both ways a receiver has been
    /// found to read it. Paused and gone on with; stopped by dropping it.
    #[test]
    fn paused_gone_on_with_and_stopped() {
        let (tv, credentials) = paired();
        tv.lock().unwrap().film = Duration::from_secs(60);
        let (path, _) = film("long.mp4");
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &Options { from: 90.0, ..quick() }).unwrap();
        let wait = Some(Duration::from_secs(10));
        let mut now = play.now();
        while now.state != State::Playing {
            now = play.next(&now, wait);
        }
        assert_eq!(now.position, Some(90.0));
        std::thread::sleep(Duration::from_millis(60));
        // (Where it is by now: on from where it said it was, never past the end.)
        let by_now = now.position_now().unwrap();
        assert!(by_now > 90.0 && by_now < 100.0, "{by_now}");
        play.pause(true).unwrap();
        let paused = play.next(&now, wait);
        assert_eq!((paused.state, paused.rate), (State::Paused, 0.0));
        assert!(paused.position.unwrap() > 90.0);
        play.pause(false).unwrap();
        assert_eq!(play.next(&paused, wait).state, State::Playing);
        play.stop();
        let tv = tv.lock().unwrap();
        assert_eq!((tv.from, tv.from_time), (90.0, Some(90.0)));
        assert_eq!(tv.asked.last().map(String::as_str), Some("TEARDOWN"));
        assert_eq!(tv.asked.iter().filter(|a| *a == "command setRate").count(), 3);
        assert!(gone(tv.address.as_deref().unwrap()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A receiver that takes the request and never comes for the film,
    /// in either way: given up on, saying so.
    #[test]
    fn one_that_never_comes_for_the_film() {
        for commands in [true, false] {
            let (tv, credentials) = paired();
            (tv.lock().unwrap().fetches, tv.lock().unwrap().commands) = (false, commands);
            let (path, _) = film("unfetched.mp4");
            let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &Options { start: Duration::from_millis(300), ..quick() }).unwrap();
            let seen = watched(&play);
            assert_eq!(seen.iter().map(|n| n.state).collect::<Vec<_>>(), [State::Loading, State::Failed]);
            assert!(seen.last().unwrap().why.as_deref().unwrap().contains("never came for the film"), "{:?}", seen.last());
            assert_eq!(play.fetched(), Some(0));
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    /// Someone at the receiver goes on to something else: the play is
    /// over, and says it was left.
    #[test]
    fn left_at_the_receiver() {
        let (tv, credentials) = paired();
        (tv.lock().unwrap().film, tv.lock().unwrap().leaves) = (Duration::from_secs(60), Some(Duration::from_millis(150)));
        let (path, _) = film("left.mp4");
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &quick()).unwrap();
        let seen = watched(&play);
        assert_eq!(states(&seen), [State::Playing, State::Left]);
        assert!(seen.last().unwrap().why.is_some());
        drop(play);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// What can't be played is said before anything is sent, and what a
    /// receiver refuses is said as that.
    #[test]
    fn refused_here_or_there() {
        let (tv, credentials) = paired();
        let nobody: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let dir = PathBuf::from(format!("/var/tmp/olo-airplay-play-test-{}-kinds", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("film.mkv"), [0x1a, 0x45, 0xdf, 0xa3, 0, 0, 0, 0]).unwrap();
        // (Not a kind it plays, not there, not an address: nobody is asked.)
        assert!(matches!(Play::begin(nobody, &credentials, &What::File(dir.join("film.mkv")), &quick()), Err(Error::CantPlay(why)) if why.contains("Matroska")));
        assert!(matches!(Play::begin(nobody, &credentials, &What::File(dir.join("none.mp4")), &quick()), Err(Error::File(_))));
        assert!(matches!(Play::begin(nobody, &credentials, &What::File(dir.clone()), &quick()), Err(Error::File(_))));
        assert!(matches!(Play::begin(nobody, &credentials, &What::Address("file:///etc/passwd".into()), &quick()), Err(Error::Address(_))));
        // Nobody there.
        let (path, _) = film("refused.mp4");
        assert!(matches!(Play::begin(nobody, &credentials, &What::File(path.clone()), &Options { wait: Duration::from_millis(500), ..quick() }), Err(Error::Receiver(pairing::Error::Io(_)))));
        // It refuses the film (the newer way, then the older).
        tv.lock().unwrap().refuses = Some(403);
        assert_eq!(Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &quick()).err(), Some(Error::Refused("to play it", 403)));
        assert_eq!(tv.lock().unwrap().asked[4..], ["command insertPlayQueueItem", "POST /play"]);
        // It doesn't know us any more.
        tv.lock().unwrap().pairing.lock().unwrap().paired.clear();
        assert_eq!(Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &quick()).err(), Some(Error::Receiver(pairing::Error::Forgotten)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Something on the web is played from where it is: its address
    /// given as it is, nothing served from here.
    #[test]
    fn an_address_is_passed_on() {
        let (tv, credentials) = paired();
        tv.lock().unwrap().fetches = false;
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::Address("https://example.org/film.m3u8".into()), &Options { start: Duration::from_millis(200), ..quick() }).unwrap();
        assert_eq!(play.fetched(), None);
        let seen = watched(&play);
        assert!(seen.last().unwrap().why.as_deref().unwrap().contains("can it reach that address"));
        assert_eq!(tv.lock().unwrap().address.as_deref(), Some("https://example.org/film.m3u8"));
    }

    fn said(text: &str, more: &[(&str, Value)]) -> Event {
        let mut all = vec![("type".to_owned(), Value::from(text))];
        all.extend(more.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
        Event::Said(Value::Dictionary(all.into_iter().collect()))
    }

    /// What a receiver says of its player, in the shapes it has been
    /// seen to say it.
    #[test]
    fn what_a_receiver_says_of_its_player() {
        let shared = || Shared { now: Mutex::new(Now::loading(0.0)), changed: Condvar::new(), stop: AtomicBool::new(false), started: AtomicBool::new(false), trace: None };
        let now = |s: &Shared| s.now.lock().unwrap().clone();
        let params = |state: &str, more: &[(&str, Value)]| {
            let mut all = vec![("playbackState".to_owned(), Value::from(state))];
            all.extend(more.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
            ("params", Value::Dictionary(all.into_iter().collect()))
        };

        // "Stopped" is where it starts from; then its state by name
        // alone, or in its params (which win), in any case of letters.
        let s = shared();
        heard(&s, said("playbackState", &[("name", Value::from("stopped"))]));
        assert_eq!(now(&s).state, State::Loading);
        heard(&s, said("playbackState", &[("name", Value::from("Playing"))]));
        assert_eq!((now(&s).state, now(&s).position, now(&s).rate), (State::Playing, Some(0.0), 1.0));
        heard(&s, said("playbackState", &[("name", Value::from("playing")), params("paused", &[])]));
        assert_eq!(now(&s).state, State::Paused);
        // Times as seconds or as a player's time; where it jumped to.
        heard(&s, said("playbackState", &[params("playing", &[("position", pl::time(30.0)), ("duration", Value::from(100i64)), ("rate", Value::from(2.0))])]));
        assert_eq!((now(&s).position, now(&s).duration, now(&s).rate), (Some(30.0), Some(100.0), 2.0));
        heard(&s, said("notification", &[("name", Value::from("timeJumped")), ("params", pl::dict([("position", pl::time(75.0))]))]));
        assert_eq!(now(&s).position, Some(75.0));
        // Other news changes nothing.
        heard(&s, said("updateInfo", &[]));
        assert_eq!(now(&s).state, State::Playing);
        // Stopped in the middle: left; and nothing after it counts.
        heard(&s, said("playbackState", &[params("stopped", &[])]));
        assert_eq!((now(&s).state, now(&s).rate), (State::Left, 0.0));
        heard(&s, said("playbackState", &[params("playing", &[])]));
        assert_eq!(now(&s).state, State::Left);

        // Stopped saying it ended, or at its end without saying: ended.
        let s = shared();
        heard(&s, said("playbackState", &[params("playing", &[("position", Value::from(1.0)), ("duration", Value::from(100.0))])]));
        heard(&s, said("playbackState", &[params("stopped", &[("reason", Value::from("ended"))])]));
        assert_eq!((now(&s).state, &now(&s).why), (State::Ended, &None));
        let s = shared();
        heard(&s, said("playbackState", &[params("playing", &[("position", Value::from(99.5)), ("duration", Value::from(100.0))])]));
        heard(&s, said("playbackState", &[("name", Value::from("idle"))]));
        assert_eq!(now(&s).state, State::Ended);

        // Begun from a place and not told where it is: there.
        let s = Shared { now: Mutex::new(Now::loading(42.0)), ..shared() };
        assert_eq!(now(&s).position_now(), Some(42.0));
        heard(&s, said("playbackState", &[("name", Value::from("playing"))]));
        assert_eq!(now(&s).position, Some(42.0));

        // Its player's error, and its closing the channel.
        let s = shared();
        heard(&s, said("playbackState", &[params("stopped", &[("error", pl::dict([("code", Value::from(-11828i64))]))])]));
        assert!(now(&s).state == State::Failed && now(&s).why.unwrap().contains("-11828"));
        let s = shared();
        heard(&s, Event::Closed);
        assert_eq!(now(&s).state, State::Left);
        // (Closed because we stopped: not news.)
        let s = shared();
        s.stop.store(true, Relaxed);
        heard(&s, Event::Closed);
        assert_eq!(now(&s).state, State::Loading);
    }

    /// Whoever waits for news is woken by an interrupt, and by nothing
    /// when nothing happens.
    #[test]
    fn waiting_is_interrupted() {
        let (tv, credentials) = paired();
        tv.lock().unwrap().film = Duration::from_secs(60);
        let (path, _) = film("waited.mp4");
        let play = Play::begin(fake::serve(tv.clone()), &credentials, &What::File(path.clone()), &quick()).unwrap();
        let mut now = play.now();
        while now.state != State::Playing {
            now = play.next(&now, Some(Duration::from_secs(10)));
        }
        let began = Instant::now();
        assert_eq!(play.next(&now, Some(Duration::from_millis(80))), now);
        assert!(began.elapsed() >= Duration::from_millis(80) && !play.interrupted());
        let remote = play.remote();
        let waker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            remote.interrupt();
        });
        assert_eq!(play.next(&now, None), now);
        assert!(play.interrupted());
        waker.join().unwrap();
        drop(play);
        assert_eq!(tv.lock().unwrap().asked.last().map(String::as_str), Some("TEARDOWN"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
