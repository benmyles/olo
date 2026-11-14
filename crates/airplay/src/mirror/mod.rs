//! Showing a screen on a receiver, live (AirPlay's screen mirroring):
//! this computer captures and encodes, the receiver decodes and shows,
//! a few dozen milliseconds behind (`Options::lead`): a window, a whole
//! display, an agent screen, with what the computer plays.
//!
//! How it's asked for is AirPlay 2's, which isn't published: as
//! doubletake, a sender for Linux that shows on today's Apple TVs, Macs
//! and televisions, has found it (its code is LGPL and isn't used here:
//! what it says of the wire is). On a connection proved by pair-verify
//! (the pairing [`crate::pairing`] made) everything is keyed from that
//! connection's secret, so nothing of FairPlay's is needed:
//!
//! 1. SETUP of the session alone (control first), which says how time
//!    is kept ([`clock`]: NTP, or the receiver's PTP timeline) and names
//!    the receiver's event port; its answer may say what its screen is.
//!    A receiver that refuses that is set up the older way, the session
//!    with its sound stream.
//! 2. SETUP of a sound stream (96: ALAC, [`sound`], its key ours), then
//!    of a video stream (110: H.264, [`video`], its key the connection's),
//!    each at a URI of its own id; the video stream's answer names the
//!    port pictures go to, over TCP.
//! 3. RECORD (unless the receiver said to skip it), and the volume.
//!
//! Pictures go as they're encoded ([`Mirror::show`]), sound as it's
//! captured ([`Mirror::sound`]); a still screen sends nothing but a
//! heartbeat a second. The session is kept alive by `/feedback` every
//! two seconds, and ends when the receiver closes the data connection
//! or the event channel (someone there stopped it), or when the
//! [`Mirror`] is dropped (TEARDOWN).

pub(crate) mod clock;
#[doc(hidden)]
pub mod fake;
pub(crate) mod sound;
pub(crate) mod video;

use std::{
    io::{Read as _, Write as _},
    net::{IpAddr, SocketAddr, TcpStream, UdpSocket},
    os::fd::AsRawFd as _,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use ::plist::Value;
use serde::Serialize;

use self::{
    clock::{Clock, Timeline},
    sound::{Now, Sound},
    video::Sealer,
};
use crate::{
    pairing::{self, Answer, Cipher, Closer, Credentials, Http, hap::random, verify},
    play::{
        Error, Trace,
        events::{Event, Events},
        lists as pl,
        rtsp::Rtsp,
        timing::Timing,
    },
};

/// What this sender says it is (doubletake's, which receivers take).
const AGENT: &str = "AirPlay/935.7.1";
const SOURCE_VERSION: &str = "980.71.1";
const EVENTS_SALT: &str = "Events-Salt";

/// AirPlay's feature bits that change how a stream is set up.
const SHOWS_1080: u32 = 28;
const PTP: u32 = 41;
const STREAM_CONNECTIONS: u32 = 59;
/// The sound formats a screen's stream takes (`supportedFormats`).
const SCREEN_ALAC: u64 = 0x40000;

/// The receiver's screen, as it says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Screen {
    /// The size it shows a mirrored screen at.
    pub width: u32,
    pub height: u32,
    /// The largest picture its decoder takes.
    pub largest: (u32, u32),
    /// Frames a second it shows.
    pub fps: u32,
}

impl Screen {
    /// The size a picture of `w` × `h` is encoded at: as it is where the
    /// receiver takes it, else made smaller, its shape kept, to fit the
    /// receiver's screen. Even (H.264's), never 0.
    pub fn fit(&self, w: u32, h: u32) -> (u32, u32) {
        let (w, h) = (w.max(2), h.max(2));
        let (fw, fh) = if w <= self.largest.0 && h <= self.largest.1 {
            (w, h)
        } else {
            let k = (f64::from(self.width) / f64::from(w)).min(f64::from(self.height) / f64::from(h)).min(1.0);
            ((f64::from(w) * k).round() as u32, (f64::from(h) * k).round() as u32)
        };
        ((fw & !1).max(2), (fh & !1).max(2))
    }
}

#[derive(Clone)]
pub struct Options {
    /// What the receiver is to know this sender as.
    pub name: String,
    /// How long the receiver is waited on for each answer.
    pub wait: Duration,
    /// The computer's sound sent too (else pictures only).
    pub sound: bool,
    /// How far behind its capture the receiver shows a picture. None:
    /// by the connection, 40 ms on a wire and 75 over Wi-Fi (sound 10 ms
    /// more), as doubletake has found works.
    pub lead: Option<Duration>,
    /// Told (on another thread) when the receiver stops showing it.
    pub ended: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Each step and what was said to it, for finding out why a receiver
    /// won't show.
    pub trace: Option<Trace>,
    /// Keep time by NTP even where the receiver offers PTP.
    pub ntp: bool,
    /// How loud the receiver plays the sound, as the computer's volume
    /// has it: 0 to 1 (its slider's place; 0, or muted, silent). Changed
    /// with [`Mirror::set_volume`].
    pub volume: f32,
}

impl Default for Options {
    fn default() -> Options {
        Options { name: "Olo".into(), wait: Duration::from_secs(8), sound: true, lead: None, ended: None, trace: None, ntp: false, volume: 1.0 }
    }
}

/// A volume (0 to 1, a slider's place) as AirPlay says it: decibels from
/// -30 (the quietest) to 0 (the loudest), the slider's place spread
/// evenly between them (the receiver puts its own curve on it); -144 is
/// silence.
fn decibels(level: f32) -> f32 {
    if level.is_nan() || level <= 0.0 { -144.0 } else { -30.0 + 30.0 * level.min(1.0) }
}

/// The SET_PARAMETER body that sets the receiver's volume to `level`.
fn volume_body(level: f32) -> Vec<u8> {
    format!("volume: {:.6}\r\n", decibels(level)).into_bytes()
}

struct Shared {
    ended: Mutex<Option<String>>,
    changed: Condvar,
    stop: AtomicBool,
    /// A volume to tell the receiver ([`Mirror::set_volume`]), told by
    /// the keeping-alive thread (never the caller's: it may be slow).
    volume: Mutex<f32>,
    volume_due: AtomicBool,
    trace: Option<Trace>,
    on_end: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Shared {
    fn say(&self, text: impl FnOnce() -> String) {
        if let Some(trace) = &self.trace {
            trace(&text());
        }
    }

    /// It's over, for this reason (the first said is kept).
    fn end(&self, why: &str) {
        let mut ended = self.ended.lock().unwrap();
        if ended.is_some() || self.stop.load(Relaxed) {
            return;
        }
        *ended = Some(why.to_owned());
        self.changed.notify_all();
        drop(ended);
        self.say(|| format!("ended: {why}"));
        if let Some(f) = &self.on_end {
            f();
        }
    }
}

/// The data connection's writing side.
struct Pictures {
    stream: TcpStream,
    sealer: Sealer,
    /// The parameter sets the receiver was last given.
    sets: Option<(Vec<u8>, Vec<u8>)>,
    shown: bool,
    /// The last stamp sent (stamps only go forward).
    last: u64,
}

/// This computer's clock, and the receiver's laid against it (PTP).
struct Times {
    clock: Clock,
    timeline: Option<Timeline>,
}

impl Times {
    /// When a picture captured at `t` is shown, and on which timeline.
    fn stamp(&self, t: Instant, lead: Duration) -> Option<(u64, u64)> {
        match &self.timeline {
            Some(tl) => tl.at(t, lead).map(|s| (s, tl.id)),
            None => Some((clock::compact(self.clock.of(t) + lead), 0)),
        }
    }

    /// Where the receiver's clock is at `t`, for sound's sync.
    fn now(&self, t: Instant) -> Option<Now> {
        match &self.timeline {
            Some(tl) => tl.at(t, Duration::ZERO).map(|s| Now::Ptp(s, tl.id)),
            None => Some(Now::Ntp(clock::ntp(self.clock.of(t)))),
        }
    }

    /// Its clock as an answer says it was when it had the request.
    fn anchor(&mut self, answer: &Answer, at: Instant) {
        if let (Some(tl), Some(theirs)) = (&mut self.timeline, clock::stamped(answer.header("x-apple-requestreceivedtimestamp"), answer.header("x-apple-processingtime"))) {
            tl.anchor(at, theirs);
        }
    }
}

/// What the receiver said of itself (`GET /info`), and what follows.
struct Info {
    features: u64,
    version: String,
    screen: Screen,
    sound: u64,
}

fn read_info(said: Option<&Value>, mut info: Info) -> Info {
    let Some(said) = said else { return info };
    if let Some(f) = pl::at(said, &["features"]).and_then(|v| v.as_unsigned_integer().or(v.as_signed_integer().map(|n| n as u64))) {
        info.features = f;
    }
    if let Some(v) = pl::text(said, &["sourceVersion"]) {
        info.version = v.to_owned();
    }
    if let Some(mask) = pl::at(said, &["supportedFormats", "screenStream"]).and_then(|v| v.as_unsigned_integer().or(v.as_signed_integer().map(|n| n as u64))) {
        info.sound = mask;
    }
    if let Some(display) = pl::at(said, &["displays"]).and_then(Value::as_array).and_then(|a| a.first()) {
        let d = |k: &str| pl::at(display, &[k]).and_then(pl::number).filter(|n| *n > 0.0).map(|n| n as u32);
        if let (Some(w), Some(h)) = (d("widthPixels").or(d("width")), d("heightPixels").or(d("height"))) {
            (info.screen.width, info.screen.height) = (w, h);
            info.screen.largest = (d("widthPixelsMax").unwrap_or(w).max(w), d("heightPixelsMax").unwrap_or(h).max(h));
        }
        if let Some(fps) = d("maxFPS").or(d("refreshRate")) {
            info.screen.fps = fps.clamp(1, 120);
        }
    }
    info
}

/// The version it runs takes PTP (from 354.54.6, but not 377.40, whose
/// PTP doesn't show).
fn takes_ptp(version: &str) -> bool {
    let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let (major, minor, patch) = (parts.first().copied().unwrap_or(0), parts.get(1).copied().unwrap_or(0), parts.get(2).copied().unwrap_or(0));
    !(major == 377 && minor == 40) && (major, minor, patch) >= (354, 54, 6)
}

/// Whether the connection to `ip` goes over Wi-Fi.
fn wireless(ip: IpAddr) -> bool {
    // (The interface that has this address, by asking getifaddrs.)
    let mut found = None;
    // SAFETY: getifaddrs/freeifaddrs as their manual says; read only.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) == 0 {
            let mut at = list;
            while !at.is_null() {
                let a = &*at;
                if !a.ifa_addr.is_null() {
                    let family = i32::from((*a.ifa_addr).sa_family);
                    let addr: Option<IpAddr> = if family == libc::AF_INET {
                        let s = &*(a.ifa_addr as *const libc::sockaddr_in);
                        Some(IpAddr::from(u32::from_be(s.sin_addr.s_addr).to_be_bytes()))
                    } else if family == libc::AF_INET6 {
                        let s = &*(a.ifa_addr as *const libc::sockaddr_in6);
                        Some(IpAddr::from(s.sin6_addr.s6_addr))
                    } else {
                        None
                    };
                    if addr == Some(ip) {
                        found = Some(std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned());
                    }
                }
                at = a.ifa_next;
            }
            libc::freeifaddrs(list);
        }
    }
    match found {
        Some(name) if name == "lo" => false,
        Some(name) => std::path::Path::new("/sys/class/net").join(&name).join("wireless").exists() || std::path::Path::new("/sys/class/net").join(&name).join("phy80211").exists(),
        None => true,
    }
}

fn uuid() -> String {
    let b = random::<16>();
    format!(
        "{:08X}-{:04X}-4{:03X}-{:04X}-{:012X}",
        u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_be_bytes([b[4], b[5]]),
        u16::from_be_bytes([b[6], b[7]]) & 0xfff,
        u16::from_be_bytes([b[8], b[9]]) & 0x3fff | 0x8000,
        u64::from_be_bytes([0, 0, b[10], b[11], b[12], b[13], b[14], b[15]])
    )
}

/// An id for a stream (it names the stream's URI and keys).
fn stream_id() -> i64 {
    (u64::from_le_bytes(random::<8>()) & 0x7fff_ffff_ffff_ffff) as i64
}

/// This sender's id, as a MAC address's form (locally administered).
fn sender_id() -> String {
    let b = random::<6>();
    format!("02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", b[1], b[2], b[3], b[4], b[5])
}

/// The ports a stream's answer names, for the stream of `kind`.
fn ports(answer: &Answer, kind: i64) -> (u16, u16) {
    let Some(v) = pl::read(&answer.body) else { return (0, 0) };
    let Some(streams) = pl::at(&v, &["streams"]).and_then(Value::as_array) else { return (0, 0) };
    let port = |s: &Value, k: &str| pl::at(s, &[k]).and_then(pl::number).filter(|p| (1.0..65536.0).contains(p)).map_or(0, |p| p as u16);
    streams.iter().find(|s| pl::at(s, &["type"]).and_then(pl::number) == Some(kind as f64)).map_or((0, 0), |s| (port(s, "dataPort"), port(s, "controlPort")))
}

/// A screen being shown on a receiver. Dropped, the receiver stops
/// showing it.
pub struct Mirror {
    shared: Arc<Shared>,
    control: Arc<Mutex<Rtsp>>,
    uri: String,
    session: String,
    pictures: Arc<Mutex<Pictures>>,
    sound: Option<Arc<Mutex<Sound>>>,
    times: Arc<Mutex<Times>>,
    lead: Duration,
    screen: Screen,
    keep: Option<JoinHandle<()>>,
    watch: Option<JoinHandle<()>>,
    asks: Option<JoinHandle<()>>,
    events: Option<Events>,
    timing: Option<Timing>,
    closer: Option<Closer>,
    data: Option<TcpStream>,
}

impl Mirror {
    /// Set up showing a screen on the receiver at `at`, paired with
    /// (`credentials`). `features` and `version`: what it announced (its
    /// own `/info` is believed over them). Returns once the receiver
    /// waits for pictures. Blocks for as long as it takes to answer each
    /// step: off a looper.
    pub fn begin(at: SocketAddr, credentials: &Credentials, features: u64, version: &str, options: &Options) -> Result<Mirror, Error> {
        let shared = Arc::new(Shared {
            ended: Mutex::new(None),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            volume: Mutex::new(options.volume),
            volume_due: AtomicBool::new(false),
            trace: options.trace.clone(),
            on_end: options.ended.clone(),
        });
        let told = |step: &str, answer: &Answer| shared.say(|| format!("{step}: {}{}", answer.status, pl::read(&answer.body).map(|v| format!(" {}", pl::line(&v))).unwrap_or_default()));
        let refused = |step: &'static str, answer: &Answer| if answer.status < 300 { Ok(()) } else { Err(Error::Refused(step, answer.status)) };
        let io = |e: std::io::Error| Error::Receiver(pairing::Error::Io(e.to_string()));

        // The connection, each side proved to the other, enciphered.
        let mut http = Http::connect(at, options.wait)?;
        let session = verify(&mut http, credentials)?;
        let control_keys = (session.key("Control-Salt", "Control-Write-Encryption-Key"), session.key("Control-Salt", "Control-Read-Encryption-Key"));
        http.encipher(session.control());
        shared.say(|| "connected, and each side proved".to_owned());
        let (Some(local), Some(peer)) = (http.local(), http.peer()) else { return Err(Error::Unexpected("a connection without its ends")) };
        let closer = http.closer();
        let mut rtsp = Rtsp::new(http);

        // What it is.
        let one_080 = features >> SHOWS_1080 & 1 == 1;
        let guessed = Screen { width: if one_080 { 1920 } else { 1280 }, height: if one_080 { 1080 } else { 720 }, largest: if one_080 { (1920, 1080) } else { (1280, 720) }, fps: 60 };
        let mut info = Info { features, version: version.to_owned(), screen: guessed, sound: SCREEN_ALAC };
        let got = rtsp.send("GET", Some("/info"), "RTSP/1.0", AGENT, &[], None)?;
        told("GET /info", &got);
        if got.status < 300 {
            info = read_info(pl::read(&got.body).as_ref(), info);
        }
        let said_features = info.features;
        let bit = |n: u32| said_features >> n & 1 == 1;
        let ptp = !options.ntp && bit(PTP) && takes_ptp(&info.version);
        let lead = options.lead.unwrap_or(if wireless(local.ip()) { Duration::from_millis(75) } else { Duration::from_millis(40) });
        let sound_lead = lead + Duration::from_millis(10);
        let sound_lead_samples = (sound_lead.as_secs_f64() * f64::from(sound::RATE)) as u32;
        let with_sound = options.sound && info.sound & SCREEN_ALAC != 0;
        shared.say(|| format!("time kept by {}; shown {} ms behind; sound {}", if ptp { "PTP" } else { "NTP" }, lead.as_millis(), if with_sound { "too" } else { "not" }));

        // Where time is answered (NTP), and where sound goes from.
        let timing = Timing::start(local.ip()).map_err(io)?;
        let (sound_data, sound_control) = (UdpSocket::bind((local.ip(), 0)).map_err(io)?, UdpSocket::bind((local.ip(), 0)).map_err(io)?);
        let our_control_port = sound_control.local_addr().map_err(io)?.port();

        let sender = sender_id();
        let session_id = uuid();
        let sound_id = stream_id();
        let uri = format!("rtsp://{}/{}", SocketAddr::new(peer.ip(), peer.port()), sound_id as u64);
        let mut session_dict = vec![
            ("deviceID", Value::from(sender.as_str())),
            ("macAddress", Value::from(sender.as_str())),
            ("sessionUUID", Value::from(session_id.as_str())),
            ("sourceVersion", Value::from(SOURCE_VERSION)),
            ("isScreenMirroringSession", Value::from(true)),
            ("timingProtocol", Value::from(if ptp { "PTP" } else { "NTP" })),
            ("osBuildVersion", Value::from("13F69")),
            ("model", Value::from("Linux")),
            ("name", Value::from(options.name.as_str())),
        ];
        if ptp {
            let peer_info = pl::dict([
                ("ID", Value::from(uuid())),
                ("SupportsClockPortMatchingOverride", Value::from(true)),
                ("DeviceType", Value::from(0i64)),
                ("Addresses", Value::Array(vec![Value::from(local.ip().to_string())])),
            ]);
            session_dict.push(("timingPeerInfo", peer_info.clone()));
            session_dict.push(("timingPeerList", Value::Array(vec![peer_info])));
        } else {
            session_dict.push(("timingPort", Value::from(i64::from(timing.port))));
        }
        let dict = |pairs: Vec<(&str, Value)>| Value::Dictionary(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());

        let times = Arc::new(Mutex::new(Times { clock: Clock::new(), timeline: None }));
        // What an answer that sets the session up says: its event port
        // and whether to skip RECORD (returned), how it keeps time, its
        // screen.
        let took = |answer: &Answer, at: Instant, info: &mut Info| -> (u16, bool) {
            let Some(v) = pl::read(&answer.body) else { return (0, false) };
            let event_port = pl::at(&v, &["eventPort"]).and_then(pl::number).filter(|p| (1.0..65536.0).contains(p)).map_or(0, |p| p as u16);
            if let Some(port) = pl::at(&v, &["timingPort"]).and_then(pl::number).filter(|p| (1.0..65536.0).contains(p)) {
                timing.probe(SocketAddr::new(peer.ip(), port as u16));
            }
            let skip = pl::at(&v, &["skipRecord"]).and_then(Value::as_boolean) == Some(true);
            if ptp && let Some(id) = pl::at(&v, &["timingPeerInfo", "ClockID"]).and_then(|v| v.as_unsigned_integer().or(v.as_signed_integer().map(|n| n as u64))).filter(|id| *id != 0) {
                let mut times = times.lock().unwrap();
                let mut tl = times.timeline.take().filter(|tl| tl.id == id).unwrap_or(Timeline::new(id));
                if let Some(theirs) = clock::stamped(answer.header("x-apple-requestreceivedtimestamp"), answer.header("x-apple-processingtime")) {
                    tl.anchor(at, theirs);
                }
                times.timeline = Some(tl);
            }
            if pl::at(&v, &["info"]).is_some() {
                let was = Info { features: info.features, version: info.version.clone(), screen: info.screen, sound: info.sound };
                *info = read_info(pl::at(&v, &["info"]), was);
            }
            (event_port, skip)
        };
        let (mut event_port, mut skip_record) = (0u16, false);

        // 1. The session alone, else (refused as a receiver of the older
        // form does) with its sound stream.
        let mut first = session_dict.clone();
        first.push(("updateSessionRequest", Value::from(false)));
        first.push(("combinedGetInfoWithControlSetup", Value::from(true)));
        let answer = rtsp.send("SETUP", Some(&uri), "RTSP/1.0", AGENT, &[], Some(&dict(first)))?;
        told("SETUP of the session", &answer);
        let control_first = match answer.status {
            200..=299 => {
                (event_port, skip_record) = took(&answer, Instant::now(), &mut info);
                true
            }
            400 | 405 | 406 | 415 | 455 | 501 => false,
            status => return Err(Error::Refused("the session", status)),
        };
        // The event channel, as soon as its port is known (a receiver
        // waits for it before it reads on): its keys named from its side.
        let connect_events = |port: u16| -> Result<Option<Events>, Error> {
            if port == 0 {
                return Ok(None);
            }
            let keys = Cipher::new(&session.key(EVENTS_SALT, "Events-Read-Encryption-Key"), &session.key(EVENTS_SALT, "Events-Write-Encryption-Key"));
            let hearer = shared.clone();
            let heard = move |e: Event| {
                if let Event::Closed = e {
                    hearer.end("the receiver stopped showing it");
                }
            };
            Events::connect(SocketAddr::new(peer.ip(), port), keys, options.wait, heard).map(Some).map_err(|e| Error::Receiver(pairing::Error::Io(format!("its event channel: {e}"))))
        };
        let mut events = if control_first { connect_events(event_port)? } else { None };

        // 2. The sound stream (set up whether sound goes or not: some
        // receivers wait for it), then the video stream.
        let sound_key = random::<32>();
        let mut stream = vec![
            ("type", Value::from(96i64)),
            ("streamConnectionID", Value::from(sound_id)),
            ("ct", Value::from(sound::ALAC)),
            ("spf", Value::from(sound::FRAME as i64)),
            ("sr", Value::from(i64::from(sound::RATE))),
            ("audioFormat", Value::from(sound::ALAC_44100_16_2)),
            ("audioMode", Value::from("default")),
            ("usingScreen", Value::from(true)),
            ("latencyMin", Value::from(0i64)),
            ("latencyMax", Value::from(i64::from(sound_lead_samples))),
            ("shk", Value::Data(sound_key.to_vec())),
        ];
        if bit(STREAM_CONNECTIONS) {
            stream.extend([
                ("isMedia", Value::from(false)),
                ("supportsDynamicStreamID", Value::from(true)),
                (
                    "streamConnections",
                    pl::dict([
                        ("streamConnectionTypeRTP", pl::dict([("streamConnectionKeyUseStreamEncryptionKey", Value::from(true))])),
                        ("streamConnectionTypeRTCP", pl::dict([("streamConnectionKeyPort", Value::from(i64::from(our_control_port)))])),
                    ]),
                ),
            ]);
        } else {
            stream.push(("controlPort", Value::from(i64::from(our_control_port))));
        }
        let mut setup = if control_first { Vec::new() } else { session_dict.clone() };
        setup.push(("streams", Value::Array(vec![dict(stream)])));
        let answer = rtsp.send("SETUP", Some(&uri), "RTSP/1.0", AGENT, &[], Some(&dict(setup)))?;
        told("SETUP of the sound", &answer);
        refused("the sound's stream", &answer)?;
        if !control_first {
            (event_port, skip_record) = took(&answer, Instant::now(), &mut info);
            events = connect_events(event_port)?;
        }
        let (sound_port, sound_control_port) = ports(&answer, 96);

        let video_id = stream_id();
        let video_uri = format!("rtsp://{}/{}", SocketAddr::new(peer.ip(), peer.port()), video_id as u64);
        let names = ["SubSu", "BePxT", "AfPxT", "BefEn", "EmEnc"].map(|n| pl::dict([("name", Value::from(n))]));
        let video = pl::dict([(
            "streams",
            Value::Array(vec![dict(vec![
                ("type", Value::from(110i64)),
                ("streamConnectionID", Value::from(video_id)),
                ("latencyMs", Value::from(lead.as_millis() as i64)),
                ("timestampInfo", Value::Array(names.to_vec())),
                // (A key for the older receivers' cipher; this connection's
                // pictures are keyed from the pairing, below.)
                ("shk", Value::Data(control_keys.0[..16].to_vec())),
                ("shiv", Value::Data(control_keys.1[..16].to_vec())),
            ])]),
        )]);
        let answer = rtsp.send("SETUP", Some(&video_uri), "RTSP/1.0", AGENT, &[], Some(&video))?;
        told("SETUP of the pictures", &answer);
        refused("the pictures' stream", &answer)?;
        let (data_port, _) = ports(&answer, 110);
        if data_port == 0 {
            return Err(Error::Unexpected("no port for the pictures in its answer"));
        }
        let data = TcpStream::connect_timeout(&SocketAddr::new(peer.ip(), data_port), options.wait).map_err(io)?;
        let _ = data.set_nodelay(true);
        let _ = data.set_write_timeout(Some(Duration::from_secs(2)));
        // (A small send buffer: under a slow network, pictures wait here
        // to be replaced by newer ones, not queue up as lag.)
        let size: libc::c_int = 256 * 1024;
        // SAFETY: setsockopt on our socket with an int.
        unsafe { libc::setsockopt(data.as_raw_fd(), libc::SOL_SOCKET, libc::SO_SNDBUF, (&size as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as u32) };
        let video_key = session.key(&format!("DataStream-Salt{}", video_id as u64), "DataStream-Output-Encryption-Key");

        // 3. Started, and the volume (the computer's).
        if !skip_record {
            let record = rtsp.send("RECORD", Some(&uri), "RTSP/1.0", AGENT, &[("Session", &session_id), ("Range", "npt=0-"), ("RTP-Info", "seq=0;rtptime=0")], None)?;
            told("RECORD", &record);
            refused("to start showing", &record)?;
        }
        if with_sound {
            for _ in 0..2 {
                let answer = rtsp.send_raw("SET_PARAMETER", Some(&uri), "RTSP/1.0", AGENT, &[], "text/parameters", &volume_body(options.volume))?;
                told(&format!("SET_PARAMETER volume {:.1} dB", decibels(options.volume)), &answer);
            }
        }

        let sound = (with_sound && sound_port > 0 && sound_control_port > 0).then(|| {
            Arc::new(Mutex::new(Sound::new(sound_data, SocketAddr::new(peer.ip(), sound_port), sound_control, SocketAddr::new(peer.ip(), sound_control_port), &sound_key, sound_lead_samples)))
        });
        let screen = info.screen;
        shared.say(|| format!("its screen {}×{} at {} a second (at most {}×{})", screen.width, screen.height, screen.fps, screen.largest.0, screen.largest.1));
        let pictures = Arc::new(Mutex::new(Pictures { stream: data.try_clone().map_err(io)?, sealer: Sealer::new(&video_key), sets: None, shown: false, last: 0 }));
        let control = Arc::new(Mutex::new(rtsp));

        // The data connection watched: the receiver closes it when it
        // stops showing.
        let watch = {
            let (mut reading, shared) = (data.try_clone().map_err(io)?, shared.clone());
            std::thread::Builder::new()
                .name("airplay-watch".into())
                .spawn(move || {
                    let _ = reading.set_read_timeout(None);
                    let mut buf = [0u8; 256];
                    while let Ok(1..) = reading.read(&mut buf) {}
                    shared.end("the receiver stopped showing it");
                })
                .map_err(io)?
        };
        // Sound it missed, asked for again on its control port.
        let asks = match &sound {
            Some(s) => {
                let (listening, sound) = (s.lock().unwrap().control().map_err(io)?, s.clone());
                let shared = shared.clone();
                Some(
                    std::thread::Builder::new()
                        .name("airplay-asks".into())
                        .spawn(move || {
                            let mut buf = [0u8; 64];
                            while let Ok((n, from)) = listening.recv_from(&mut buf) {
                                if shared.stop.load(Relaxed) {
                                    break;
                                }
                                let again = sound.lock().unwrap().again(&buf[..n]);
                                for p in again {
                                    let _ = listening.send_to(&p, from);
                                }
                            }
                        })
                        .map_err(io)?,
                )
            }
            None => None,
        };
        // Kept alive: a heartbeat on the data connection and sound's sync
        // every second, a word on the control connection every two; the
        // volume told as it changes.
        let keep = {
            let (shared, control, pictures, sound, times, uri) = (shared.clone(), control.clone(), pictures.clone(), sound.clone(), times.clone(), uri.clone());
            std::thread::Builder::new().name("airplay-keep".into()).spawn(move || keep(&shared, &control, &uri, &pictures, sound.as_deref(), &times)).map_err(io)?
        };
        Ok(Mirror {
            shared,
            control,
            uri,
            session: session_id,
            pictures,
            sound,
            times,
            lead,
            screen,
            keep: Some(keep),
            watch: Some(watch),
            asks,
            events,
            timing: Some(timing),
            closer,
            data: Some(data),
        })
    }

    /// The receiver's screen: what to encode for ([`Screen::fit`]).
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// How far behind capture it's shown.
    pub fn lead(&self) -> Duration {
        self.lead
    }

    /// Whether the computer's sound goes too.
    pub fn sounds(&self) -> bool {
        self.sound.is_some()
    }

    /// The receiver's volume set to `level` (as [`Options::volume`]):
    /// told it at once by another thread, so this never waits on the
    /// receiver.
    pub fn set_volume(&self, level: f32) {
        let _ended = self.shared.ended.lock().unwrap();
        *self.shared.volume.lock().unwrap() = level;
        self.shared.volume_due.store(true, Relaxed);
        self.shared.changed.notify_all();
    }

    /// One encoded picture (H.264, Annex B, as an encoder writes it) of
    /// `size`, captured at `captured`, sent. Pictures before the first
    /// key frame with its parameter sets are left out (the receiver
    /// couldn't decode them). An error: the receiver is gone.
    pub fn show(&self, annex_b: &[u8], size: (u32, u32), captured: Instant) -> Result<(), String> {
        if let Some(why) = self.ended() {
            return Err(why);
        }
        let p = video::picture(annex_b);
        if p.units.is_empty() {
            return Ok(());
        }
        let Some((stamp, timeline)) = self.times.lock().unwrap().stamp(captured, self.lead) else { return Ok(()) };
        let mut pictures = self.pictures.lock().unwrap();
        let stamp = stamp.max(pictures.last + 1);
        pictures.last = stamp;
        let sets = match (p.sps, p.pps) {
            (Some(sps), Some(pps)) => Some((sps, pps)),
            _ => None,
        };
        let mut out = Vec::new();
        if let Some(sets) = sets.filter(|s| pictures.sets.as_ref() != Some(s)) {
            let config = video::configuration(&sets.0, &sets.1);
            out.extend(video::configuration_header(config.len(), stamp, size.0, size.1));
            out.extend(config);
            pictures.sets = Some(sets);
        }
        if pictures.sets.is_none() || (!pictures.shown && !p.key) {
            return Ok(());
        }
        out.extend(pictures.sealer.seal(&p.units, p.key, stamp, timeline));
        if let Err(e) = pictures.stream.write_all(&out) {
            drop(pictures);
            self.shared.end(&format!("the receiver can't be reached: {e}"));
            return Err(e.to_string());
        }
        pictures.shown = true;
        Ok(())
    }

    /// Sound captured at `captured` (its first sample): 44.1 kHz stereo,
    /// interleaved. Sent once pictures are (the receiver lines the two up
    /// from the first picture on).
    pub fn sound(&self, samples: &[i16], captured: Instant) {
        let Some(sound) = &self.sound else { return };
        if !self.pictures.lock().unwrap().shown {
            return;
        }
        let times = self.times.clone();
        let mut sound = sound.lock().unwrap();
        let now = move |t: Instant| times.lock().unwrap().now(t);
        if let Err(e) = sound.push(samples, captured, now) {
            self.shared.say(|| format!("sound not sent: {e}"));
        }
    }

    /// Why it ended, once it has: the receiver stopped showing it, or
    /// went.
    pub fn ended(&self) -> Option<String> {
        self.shared.ended.lock().unwrap().clone()
    }

    /// Wait for it to end, `wait` at most: why, if it has.
    pub fn wait_ended(&self, wait: Duration) -> Option<String> {
        let ended = self.shared.ended.lock().unwrap();
        let (ended, _) = self.shared.changed.wait_timeout_while(ended, wait, |e| e.is_none()).unwrap();
        ended.clone()
    }
}

/// The keeping-alive thread's work, until stopped or the receiver goes.
fn keep(shared: &Shared, control: &Mutex<Rtsp>, uri: &str, pictures: &Mutex<Pictures>, sound: Option<&Mutex<Sound>>, times: &Mutex<Times>) {
    let feedback = |times: &Mutex<Times>| -> bool {
        let answer = control.lock().unwrap().send("POST", Some("/feedback"), "RTSP/1.0", AGENT, &[], None);
        match answer {
            Ok(answer) => {
                times.lock().unwrap().anchor(&answer, Instant::now());
                true
            }
            Err(e) => {
                shared.end(&format!("the receiver can't be reached: {e}"));
                false
            }
        }
    };
    if !feedback(times) {
        return;
    }
    let mut beat = 0u64;
    let mut next = Instant::now() + Duration::from_secs(1);
    loop {
        {
            let ended = shared.ended.lock().unwrap();
            let left = next.saturating_duration_since(Instant::now());
            let (ended, _) = shared.changed.wait_timeout_while(ended, left, |e| e.is_none() && !shared.stop.load(Relaxed) && !shared.volume_due.load(Relaxed)).unwrap();
            if ended.is_some() || shared.stop.load(Relaxed) {
                return;
            }
        }
        // (A volume key held: only the newest is told.)
        if shared.volume_due.swap(false, Relaxed) {
            let level = *shared.volume.lock().unwrap();
            let answer = control.lock().unwrap().send_raw("SET_PARAMETER", Some(uri), "RTSP/1.0", AGENT, &[], "text/parameters", &volume_body(level));
            match answer {
                Ok(answer) => shared.say(|| format!("SET_PARAMETER volume {:.1} dB: {}", decibels(level), answer.status)),
                Err(e) => {
                    shared.end(&format!("the receiver can't be reached: {e}"));
                    return;
                }
            }
        }
        if Instant::now() < next {
            continue;
        }
        next = Instant::now() + Duration::from_secs(1);
        beat += 1;
        {
            let mut p = pictures.lock().unwrap();
            if p.shown
                && let Err(e) = p.stream.write_all(&video::heartbeat())
            {
                drop(p);
                shared.end(&format!("the receiver can't be reached: {e}"));
                return;
            }
        }
        if let Some(sound) = sound {
            let at = Instant::now();
            let now = times.lock().unwrap().now(at);
            if let Some(now) = now {
                let _ = sound.lock().unwrap().resync(at, now);
            }
        }
        if beat.is_multiple_of(2) && !feedback(times) {
            return;
        }
    }
}

impl Drop for Mirror {
    fn drop(&mut self) {
        {
            let _ended = self.shared.ended.lock().unwrap();
            self.shared.stop.store(true, Relaxed);
            self.shared.changed.notify_all();
        }
        if let Some(keep) = self.keep.take() {
            let _ = keep.join();
        }
        // Goodbye, if the connection is free to say it on.
        if let Ok(mut control) = self.control.try_lock() {
            control.http.wait(Duration::from_secs(1));
            if let Ok(answer) = control.send("TEARDOWN", Some(&self.uri), "RTSP/1.0", AGENT, &[("Session", &self.session)], None) {
                self.shared.say(|| format!("TEARDOWN: {}", answer.status));
            }
        }
        if let Some(closer) = &self.closer {
            closer.close();
        }
        if let Some(data) = self.data.take() {
            let _ = data.shutdown(std::net::Shutdown::Both);
        }
        if let Some(watch) = self.watch.take() {
            let _ = watch.join();
        }
        if let (Some(asks), Some(sound)) = (self.asks.take(), &self.sound) {
            // (Woken by a word from ourselves: it waits on nothing else.)
            if let Ok(listening) = sound.lock().unwrap().control()
                && let Ok(at) = listening.local_addr()
                && let Ok(waker) = UdpSocket::bind((at.ip(), 0))
            {
                let _ = waker.send_to(&[], at);
            }
            let _ = asks.join();
        }
        (self.events, self.timing) = (None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pairing::{Setup, fake::Receiver},
        play::fake::{self, Tv},
    };

    /// A receiver of the test's own, paired with.
    fn paired() -> (Arc<Mutex<Tv>>, Credentials) {
        let mut receiver = Receiver::new("1234");
        let credentials = Setup::begin(&mut receiver).unwrap().finish(&mut receiver, "1234", "Olo").unwrap().credentials;
        (Arc::new(Mutex::new(Tv::new(Arc::new(Mutex::new(receiver))))), credentials)
    }

    fn quick() -> Options {
        Options { name: "Olo of the test".into(), wait: Duration::from_secs(5), ..Options::default() }
    }

    /// Until `done` says so of what the receiver was shown (10 s at most).
    fn until(tv: &Arc<Mutex<Tv>>, done: impl Fn(&fake_shown::Shown) -> bool) {
        let shown = tv.lock().unwrap().shown.clone();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&shown.lock().unwrap()) {
            assert!(Instant::now() < deadline, "the receiver wasn't shown it: {:?}", shown.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    use super::fake as fake_shown;

    const SPS: [u8; 4] = [0x67, 0x42, 0xc0, 0x1f];
    const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];

    fn key_frame() -> Vec<u8> {
        [&[0, 0, 0, 1][..], &SPS, &[0, 0, 0, 1], &PPS, &[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x21]].concat()
    }

    /// The volume as AirPlay says it: the slider's place between -30 dB
    /// and 0, silence -144.
    #[test]
    fn volumes_in_decibels() {
        assert_eq!([decibels(1.0), decibels(1.5), decibels(0.5), decibels(0.0), decibels(-1.0)], [0.0, 0.0, -15.0, -144.0, -144.0]);
        assert_eq!(volume_body(0.25), b"volume: -22.500000\r\n");
    }

    /// The receiver's volume, the computer's at the start and as it
    /// changes, told without the caller waiting.
    #[test]
    fn the_volume_follows() {
        let (tv, credentials) = paired();
        let at = fake::serve(tv.clone());
        let mirror = Mirror::begin(at, &credentials, 0, "", &Options { volume: 0.5, ..quick() }).unwrap();
        mirror.set_volume(0.8);
        mirror.set_volume(0.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while tv_parameters(&tv).last().map(String::as_str) != Some("volume: -144.000000") {
            assert!(Instant::now() < deadline, "not told: {:?}", tv_parameters(&tv));
            std::thread::sleep(Duration::from_millis(20));
        }
        let said = tv_parameters(&tv);
        assert_eq!(&said[..2], ["volume: -15.000000", "volume: -15.000000"]);
        // (The newest at least; one in between if it was told in time.)
        assert!(said[2..].iter().all(|p| p == "volume: -6.000000" || p == "volume: -144.000000"), "{said:?}");
    }

    fn tv_parameters(tv: &Arc<Mutex<Tv>>) -> Vec<String> {
        tv.lock().unwrap().parameters.clone()
    }

    /// A screen shown, as Apple's senders set it up: the session alone,
    /// its sound, its pictures, RECORD and the volume; the pictures come
    /// sealed with the pairing's key, the first a key frame after its
    /// configuration, stamped a lead after their capture on the clock
    /// the receiver asked the time of; the sound comes as it was, with a
    /// sync before it; a heartbeat a second; and TEARDOWN at the end.
    #[test]
    fn a_screen_shown_with_its_sound() {
        let (tv, credentials) = paired();
        let at = fake::serve(tv.clone());
        let lead = Duration::from_millis(60);
        let mirror = Mirror::begin(at, &credentials, 0, "", &Options { lead: Some(lead), ..quick() }).unwrap();
        assert_eq!(mirror.screen(), Screen { width: 1920, height: 1080, largest: (1920, 1080), fps: 60 });
        assert!(mirror.sounds());
        // A picture before any key frame: left out.
        mirror.show(&[0, 0, 0, 1, 0x41, 1, 2, 3], (640, 360), Instant::now()).unwrap();
        let t = Instant::now();
        mirror.show(&key_frame(), (640, 360), t).unwrap();
        mirror.show(&[0, 0, 0, 1, 0x41, 0x9a, 2], (640, 360), t + Duration::from_millis(16)).unwrap();
        // A tenth of a second of sound: twelve frames, the rest waiting.
        let samples: Vec<i16> = (0..4410 * 2).map(|i| (i * 7) as i16).collect();
        mirror.sound(&samples, t);
        until(&tv, |s| s.pictures.len() == 2 && s.samples.len() >= 12 * 352 * 2 && s.heartbeats >= 1 && !s.syncs.is_empty());
        {
            let tv = tv.lock().unwrap();
            assert_eq!(tv.asked, ["GET /info", "SETUP", "SETUP sound", "SETUP pictures", "RECORD"]);
            assert!(tv.mirrored && tv.timed && tv.feedbacks >= 1);
            assert_eq!(tv.parameters, ["volume: 0.000000", "volume: 0.000000"]);
            let s = tv.shown.lock().unwrap();
            assert_eq!(s.refused, 0);
            assert_eq!(s.configurations, [(video::configuration(&SPS, &PPS), (640.0, 360.0))]);
            let ((key, stamp, timeline, units), (later_key, later, ..)) = (&s.pictures[0], &s.pictures[1]);
            assert!(*key && !*later_key && later > stamp && *timeline == 0);
            assert_eq!(units, &[0, 0, 0, 4, 0x65, 0x88, 0x84, 0x21]);
            // Its stamp: its capture on the monotonic clock, plus the lead.
            let expected = clock::compact(Clock::new().of(t) + lead);
            assert!(stamp.abs_diff(expected) < clock::compact(Duration::from_millis(2)), "{stamp:x} {expected:x}");
            assert_eq!(&s.samples[..12 * 352 * 2], &samples[..12 * 352 * 2]);
            assert!(s.rtp.windows(2).all(|w| w[1] == w[0].wrapping_add(352)));
            // The sync said the first sample plays a lead (and 10 ms) on.
            assert_eq!(&s.syncs[0][..2], &[0x90, 0xd4]);
            let synced = u32::from_be_bytes(s.syncs[0][4..8].try_into().unwrap());
            assert_eq!(s.rtp[0].wrapping_sub(synced), (0.07 * 44100.0) as u32);
        }
        drop(mirror);
        until(&tv, |s| s.closed);
        assert_eq!(tv.lock().unwrap().asked.last().map(String::as_str), Some("TEARDOWN"));
    }

    /// A receiver that takes the session only with its sound is set up
    /// that way; and when someone there stops it, it ends, and says so.
    #[test]
    fn the_older_form_and_stopped_there() {
        let (tv, credentials) = paired();
        {
            let tv = tv.lock().unwrap();
            tv.shown.lock().unwrap().stop_after = Some(1);
        }
        tv.lock().unwrap().older_mirroring = true;
        let at = fake::serve(tv.clone());
        let ended = Arc::new(AtomicBool::new(false));
        let told = ended.clone();
        let mirror = Mirror::begin(at, &credentials, 0, "", &Options { sound: false, ended: Some(Arc::new(move || told.store(true, Relaxed))), ..quick() }).unwrap();
        assert!(!mirror.sounds());
        assert_eq!(tv.lock().unwrap().asked, ["GET /info", "SETUP refused", "SETUP sound", "SETUP pictures", "RECORD"]);
        assert!(tv.lock().unwrap().mirrored && tv.lock().unwrap().parameters.is_empty());
        mirror.show(&key_frame(), (1280, 720), Instant::now()).unwrap();
        assert_eq!(mirror.wait_ended(Duration::from_secs(5)).as_deref(), Some("the receiver stopped showing it"));
        assert!(ended.load(Relaxed));
        assert!(mirror.show(&key_frame(), (1280, 720), Instant::now()).is_err());
    }

    /// A receiver that isn't the one paired with is refused.
    #[test]
    fn not_the_one_paired_with() {
        let (tv, _) = paired();
        let (_, theirs) = paired();
        let at = fake::serve(tv);
        assert!(matches!(Mirror::begin(at, &theirs, 0, "", &quick()), Err(Error::Receiver(_))));
    }

    #[test]
    fn a_picture_fitted_to_the_receiver() {
        let tv = Screen { width: 1920, height: 1080, largest: (1920, 1080), fps: 60 };
        // A 16:10 display at 2×: made smaller, its shape kept.
        assert_eq!(tv.fit(2880, 1800), (1728, 1080));
        // A window that fits is sent as it is (made even).
        assert_eq!(tv.fit(801, 601), (800, 600));
        assert_eq!(tv.fit(1, 1), (2, 2));
        // A receiver that takes 4K takes it as it is.
        let tv4k = Screen { largest: (3840, 2160), ..tv };
        assert_eq!(tv4k.fit(2880, 1800), (2880, 1800));
        assert_eq!(tv4k.fit(5120, 2880), (1920, 1080));
    }

    #[test]
    fn what_it_says_of_its_screen() {
        let said = pl::dict([
            ("features", Value::from(1i64 << 41 | 1 << 7)),
            ("sourceVersion", Value::from("870.14.1")),
            ("supportedFormats", pl::dict([("screenStream", Value::from(0x0104_0000i64))])),
            ("displays", Value::Array(vec![pl::dict([("widthPixels", Value::from(3840i64)), ("heightPixels", Value::from(2160i64)), ("maxFPS", Value::from(60i64))])])),
        ]);
        let info = read_info(Some(&said), Info { features: 0, version: String::new(), screen: Screen { width: 1280, height: 720, largest: (1280, 720), fps: 30 }, sound: 0 });
        assert_eq!((info.features >> 41 & 1, info.version.as_str(), info.sound & SCREEN_ALAC != 0), (1, "870.14.1", true));
        assert_eq!(info.screen, Screen { width: 3840, height: 2160, largest: (3840, 2160), fps: 60 });
        assert!(takes_ptp("870.14.1") && takes_ptp("354.54.6") && !takes_ptp("354.54.5") && !takes_ptp("377.40.1") && !takes_ptp(""));
    }

    #[test]
    fn ids_of_the_right_form() {
        let u = uuid();
        assert_eq!((u.len(), &u[14..15]), (36, "4"));
        assert!(stream_id() >= 0);
        assert!(sender_id().starts_with("02:") && sender_id().len() == 17);
        assert!(!wireless("127.0.0.1".parse().unwrap()));
    }
}
