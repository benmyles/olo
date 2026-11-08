//! The PipeWire side: the processed source ([`NODE`]), and while something
//! records from it, the microphone it listens to and (for echo
//! cancellation) what the speakers play. All on one thread: PipeWire's
//! loop; the sound itself on PipeWire's real-time thread.
//!
//! The source stays for as long as the service runs, whatever changes
//! behind it (another microphone, other settings: [`Engine::reconfigure`]),
//! so a call in progress never loses its device. The captures exist only
//! while the source streams: nothing records the microphone, and nothing
//! runs, while no one listens.
//!
//! Nothing waits that needn't (Ben, 2026-11-07, of 40 ms: "we need that to
//! be way more optimized so that it is not introducing any kind of
//! noticeable lag"), as PipeWire's own echo-cancel does it: the captures
//! and the source are one group (`node.group`: one driver, one clock, so
//! nothing drifts), the source is triggered (`StreamFlags::TRIGGER`) from
//! the microphone's process once its frames are done, and for calls the
//! graph runs a frame a cycle (`node.force-quantum` 480, while something
//! records), so nothing is held back for the frames' fit. With a quantum
//! that doesn't fit the frames (another program forcing its own) only its
//! misfit is held (`misfit`), exactly, from the start.
//!
//! Triggered, the source gives each cycle's frames in that cycle: an app
//! that records through Pulse (Chrome's, every call app's: scheduled in
//! the cycle) gets the voice 0 ms behind the microphone with the
//! processing off, the noise model's own delay with it on (10 ms the
//! low-delay one, 30 DeepFilterNet3). Only a client PipeWire schedules
//! asynchronously (pw-record) reads it a cycle late, as it reads any
//! source that isn't a device. Measured by tools/dev/voice-latency (a
//! synchronous probe, speech cross-correlated; 2026-11-07): 0.0, 10.0
//! and 30.0 ms. (An earlier pw-record probe had said 10 ms more of each,
//! and so had this service.)

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use olo_protocol::voice::{Mode, NODE, Settings};
use olo_sync::Mutex;
use olo_voice::{FRAME, RATE, Voice};
use pipewire as pw;
use pw::{properties::properties, spa};

/// What the engine does.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The microphone's node name.
    pub microphone: String,
    /// What the microphone is called ("AirPods"): the source is named
    /// after it (`description`), so it's told apart from the microphone
    /// itself wherever devices are listed.
    pub label: String,
    /// The speakers' node name, when echo is to be cancelled.
    pub speakers: Option<String>,
    pub settings: Settings,
}

impl Config {
    /// The source's name: "AirPods (Voice Processing)".
    pub fn description(&self) -> String {
        let label = if self.label.trim().is_empty() { "Microphone" } else { self.label.trim() };
        format!("{label} (Voice Processing)")
    }

    /// The buffering around the processing: the graph's quantum asked for
    /// (samples), whether it's forced (calls: a frame a cycle), and what
    /// the source keeps ahead beyond what the quantum's fit needs.
    fn buffering(&self) -> (u32, bool, usize) {
        match self.settings.mode {
            Mode::Calls => (FRAME as u32, true, 0),
            // (Not to the millisecond: a long quantum, a margin of frames.)
            Mode::Recording => (2048, false, 4 * FRAME),
        }
    }

    fn latency(&self) -> String {
        format!("{}/{RATE}", self.buffering().0)
    }
}

/// What the source must hold back for `quantum` samples a cycle when
/// frames of `FRAME` come as the microphone's samples make them, so it
/// never runs dry, which is what it delays the sound by: nothing when
/// the frames fit the quantum (a multiple), else the most a frame can be
/// short of done at a cycle's end, `FRAME - gcd(quantum, FRAME)` (448
/// samples for any power of two). Held from the start (silence before
/// the sound), the frames done never fall short of a quantum: each cycle
/// adds a quantum and takes one, so what's held stays, and the samples
/// not yet a frame are at most that.
fn misfit(quantum: usize) -> usize {
    let q = quantum.max(1);
    if q % FRAME == 0 { 0 } else { FRAME - gcd(q, FRAME) }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The source, to trigger from the microphone's process once its frames
/// are done (null until it's made).
type Trigger = Arc<std::sync::atomic::AtomicPtr<pw::sys::pw_stream>>;

/// What the engine has been doing, for the state file.
#[derive(Debug, Default)]
pub struct Stats {
    pub active: AtomicBool,
    pub frames: AtomicU64,
    pub frame_ns: AtomicU64,
    pub dropouts: AtomicU64,
    /// What it delays the sound by in all: the processing's, and what the
    /// source holds for the quantum it runs at.
    pub latency_samples: AtomicU32,
    /// The graph's quantum, as the source last ran (samples; 0 not yet).
    pub quantum: AtomicU32,
}

pub enum Message {
    Reconfigure(Box<Config>),
    Quit,
}

pub struct Engine {
    tx: pw::channel::Sender<Message>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub stats: Arc<Stats>,
}

impl Engine {
    /// Start, telling `changed` (from PipeWire's thread) when the source
    /// starts or stops streaming.
    pub fn start(config: Config, changed: impl Fn() + Send + 'static) -> Engine {
        let (tx, rx) = pw::channel::channel();
        let stats = Arc::new(Stats::default());
        let s = stats.clone();
        let thread = std::thread::Builder::new()
            .name("pipewire".into())
            .spawn(move || {
                if let Err(e) = run(config, rx, s, changed) {
                    tracing::warn!("voice: {e}");
                }
            })
            .expect("thread");
        Engine { tx, thread: Some(thread), stats }
    }

    pub fn reconfigure(&self, config: Config) {
        let _ = self.tx.send(Message::Reconfigure(Box::new(config)));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The processing between the captures and the source.
struct Processing {
    voice: Voice,
    /// Samples not yet a whole frame.
    mic: Vec<f32>,
    far: Vec<f32>,
    /// Processed, waiting for the source.
    out: VecDeque<f32>,
    /// What the source keeps ahead beyond its quantum's fit (recording's
    /// margin).
    extra: usize,
    /// The quantum the source last ran at (0: to find out, and hold
    /// back for), and what it holds back for it (`misfit`).
    quantum: usize,
    hold: usize,
}

impl Processing {
    fn new(config: &Config) -> Result<Processing, String> {
        let mut settings = config.settings;
        settings.echo_cancellation &= config.speakers.is_some();
        let voice = Voice::new(settings)?;
        Ok(Processing { voice, mic: Vec::with_capacity(4096), far: Vec::with_capacity(4096), out: VecDeque::with_capacity(16384), extra: config.buffering().2, quantum: 0, hold: 0 })
    }

    fn reset(&mut self) {
        self.mic.clear();
        self.far.clear();
        self.out.clear();
        self.quantum = 0;
    }

    /// What it delays the sound by, in samples: the processing's, and
    /// what the source holds for its quantum (as an app recording through
    /// Pulse gets it: in the cycle the microphone's frames came in).
    fn latency(&self) -> u32 {
        (self.voice.latency_samples() + self.hold + self.extra) as u32
    }
}

/// The quantum the source forces on the graph while something records
/// from it: calls a frame a cycle, so nothing waits; "0" forces none.
fn force_quantum(config: &Config) -> String {
    let (quantum, force, _) = config.buffering();
    if force { quantum.to_string() } else { "0".into() }
}

/// One group for the captures and the source: one driver, one clock (so
/// nothing drifts and the source can be fed in the cycle its frames came
/// in), and never linked to each other.
const GROUP: &str = "olo-voice";

fn format_param() -> Result<Vec<u8>, String> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(1);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    Ok(pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object { type_: spa::sys::SPA_TYPE_OBJECT_Format, id: spa::sys::SPA_PARAM_EnumFormat, properties: info.into() }),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner())
}

/// A buffer's bytes as samples (PipeWire's buffers are aligned for them).
fn samples(bytes: &[u8]) -> &[f32] {
    // SAFETY: f32 has no invalid bit patterns.
    let (head, s, _) = unsafe { bytes.align_to::<f32>() };
    debug_assert!(head.is_empty());
    s
}

fn samples_mut(bytes: &mut [u8]) -> &mut [f32] {
    // SAFETY: as above.
    let (head, s, _) = unsafe { bytes.align_to_mut::<f32>() };
    debug_assert!(head.is_empty());
    s
}

/// The captures (microphone, speakers) for one configuration.
struct Captures {
    _streams: Vec<(pw::stream::StreamRc, Box<dyn std::any::Any>)>,
    weak: Vec<pw::stream::StreamWeak>,
}

fn capture(
    core: &pw::core::CoreRc,
    target: &str,
    monitor: bool,
    latency: &str,
    processing: Arc<Mutex<Processing>>,
    stats: Arc<Stats>,
    trigger: Option<Trigger>,
) -> Result<(pw::stream::StreamRc, Box<dyn std::any::Any>), String> {
    let name = if monitor { "Olo Voice: echo reference" } else { "Olo Voice: microphone" };
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => if monitor { "olo_voice.echo_reference" } else { "olo_voice.microphone" },
        *pw::keys::NODE_LATENCY => latency,
        *pw::keys::NODE_GROUP => GROUP,
        *pw::keys::NODE_LINK_GROUP => GROUP,
        *pw::keys::TARGET_OBJECT => target,
        // Ours alone: not something for the person to move about.
        "node.dont-reconnect" => "true",
        "stream.dont-remix" => "false",
    };
    if monitor {
        props.insert("stream.capture.sink", "true");
    }
    let stream = pw::stream::StreamRc::new(core.clone(), name, props).map_err(|e| e.to_string())?;
    let listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let data = &mut buffer.datas_mut()[0];
            let size = data.chunk().size() as usize;
            let offset = data.chunk().offset() as usize;
            let Some(bytes) = data.data() else { return };
            let end = (offset + size).min(bytes.len());
            let input = samples(&bytes[offset.min(end)..end]);
            {
                let mut p = processing.lock().unwrap();
                let p = &mut *p;
                if monitor {
                    p.far.extend_from_slice(input);
                    let whole = p.far.len() / FRAME * FRAME;
                    for f in p.far[..whole].as_chunks::<FRAME>().0 {
                        p.voice.render(f);
                    }
                    p.far.drain(..whole);
                } else {
                    // The processing's thread on a fast core while it works
                    // (once: it is PipeWire's realtime thread, for good).
                    thread_local!(static FAST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
                    if !FAST.replace(true) {
                        let _ = olo_system::priority::util_min_self(olo_protocol::performance::AUDIO_UTIL_MIN);
                    }
                    p.mic.extend_from_slice(input);
                    let whole = p.mic.len() / FRAME * FRAME;
                    let mut out = [0f32; FRAME];
                    for f in p.mic[..whole].as_chunks::<FRAME>().0 {
                        let started = std::time::Instant::now();
                        p.voice.capture(f, &mut out);
                        stats.frame_ns.fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
                        stats.frames.fetch_add(1, Relaxed);
                        p.out.extend(out);
                    }
                    p.mic.drain(..whole);
                }
            }
            // The source given this cycle's frames now (it takes the lock).
            if let Some(t) = &trigger {
                let source = t.load(std::sync::atomic::Ordering::Acquire);
                if !source.is_null() {
                    // SAFETY: the source outlives the captures (made before
                    // them, gone after: `run`), and this is the data loop's
                    // thread, where its process runs.
                    unsafe { pw::sys::pw_stream_trigger_process(source) };
                }
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let format = format_param()?;
    let mut params = [pw::spa::pod::Pod::from_bytes(&format).ok_or("format")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS | pw::stream::StreamFlags::INACTIVE,
            &mut params,
        )
        .map_err(|e| e.to_string())?;
    Ok((stream, Box::new(listener)))
}

fn captures(core: &pw::core::CoreRc, config: &Config, processing: &Arc<Mutex<Processing>>, stats: &Arc<Stats>, trigger: &Trigger) -> Result<Captures, String> {
    let latency = config.latency();
    let mut streams = vec![capture(core, &config.microphone, false, &latency, processing.clone(), stats.clone(), Some(trigger.clone()))?];
    if let (Some(speakers), true) = (&config.speakers, config.settings.echo_cancellation) {
        streams.push(capture(core, speakers, true, &latency, processing.clone(), stats.clone(), None)?);
    }
    let weak = streams.iter().map(|(s, _)| s.downgrade()).collect();
    Ok(Captures { _streams: streams, weak })
}

fn set_active(weak: &[pw::stream::StreamWeak], on: bool) {
    for w in weak {
        if let Some(s) = w.upgrade() {
            let _ = s.set_active(on);
        }
    }
}

fn run(config: Config, rx: pw::channel::Receiver<Message>, stats: Arc<Stats>, changed: impl Fn() + Send + 'static) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| format!("can't reach PipeWire: {e}"))?;

    let processing = Arc::new(Mutex::new(Processing::new(&config)?));
    stats.latency_samples.store(processing.lock().unwrap().latency(), Relaxed);
    let trigger: Trigger = Arc::new(std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()));

    let source = pw::stream::StreamRc::new(
        core.clone(),
        "Olo Voice",
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => NODE,
            *pw::keys::NODE_DESCRIPTION => config.description(),
            *pw::keys::AUDIO_CHANNELS => "1",
            *pw::keys::NODE_LATENCY => config.latency(),
            *pw::keys::NODE_FORCE_QUANTUM => force_quantum(&config),
            *pw::keys::NODE_GROUP => GROUP,
            *pw::keys::NODE_LINK_GROUP => GROUP,
            "device.icon-name" => "audio-input-microphone",
        },
    )
    .map_err(|e| e.to_string())?;
    // (The captures, made after it, trigger it; it goes after them.)
    trigger.store(source.as_raw_ptr(), std::sync::atomic::Ordering::Release);
    let current = Rc::new(RefCell::new((config.clone(), captures(&core, &config, &processing, &stats, &trigger)?)));
    let _source_listener = source
        .add_local_listener_with_user_data(())
        .state_changed({
            let (current, processing, stats) = (current.clone(), processing.clone(), stats.clone());
            move |_, _, _, new| {
                let on = matches!(new, pw::stream::StreamState::Streaming);
                if on {
                    processing.lock().unwrap().reset();
                }
                set_active(&current.borrow().1.weak, on);
                stats.active.store(on, Relaxed);
                changed();
            }
        })
        .process({
            let (processing, stats) = (processing.clone(), stats.clone());
            move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let requested = buffer.requested() as usize;
                let data = &mut buffer.datas_mut()[0];
                let n = if let Some(bytes) = data.data() {
                    let out = samples_mut(bytes);
                    let n = if requested == 0 { out.len() } else { requested.min(out.len()) };
                    let out = &mut out[..n];
                    let mut p = processing.lock().unwrap();
                    let p = &mut *p;
                    // The quantum it runs at: what it holds back for it
                    // (`misfit`: the delay), held from now exactly, with
                    // silence before the sound to make it up (starting) or
                    // the oldest let go. Each cycle then adds a quantum and
                    // takes one: what's held stays, and never runs dry.
                    if n != p.quantum {
                        p.quantum = n;
                        p.hold = misfit(n);
                        let (want, have) = (p.hold + p.extra + n, p.mic.len() + p.out.len());
                        if have < want {
                            for _ in have..want {
                                p.out.push_front(0.0);
                            }
                        } else {
                            let extra = (have - want).min(p.out.len());
                            p.out.drain(..extra);
                        }
                        stats.quantum.store(n as u32, Relaxed);
                        stats.latency_samples.store(p.latency(), Relaxed);
                    }
                    if p.out.len() < n {
                        // Ran dry (it shouldn't): count it, silence, and
                        // held again next cycle.
                        stats.dropouts.fetch_add(1, Relaxed);
                        out.fill(0.0);
                        p.quantum = 0;
                    } else {
                        for (o, s) in out.iter_mut().zip(p.out.drain(..n)) {
                            *o = s;
                        }
                        // Never more than a frame beyond what's held (should
                        // a capture's clock run a hair fast).
                        let most = p.hold + p.extra + FRAME;
                        let held = p.mic.len() + p.out.len();
                        if held > most {
                            let extra = (held - most).min(p.out.len());
                            p.out.drain(..extra);
                        }
                    }
                    n
                } else {
                    0
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = 4;
                *chunk.size_mut() = (n * 4) as u32;
            }
        })
        .register()
        .map_err(|e| e.to_string())?;
    let format = format_param()?;
    let mut params = [pw::spa::pod::Pod::from_bytes(&format).ok_or("format")?];
    // (Triggered by the microphone's process: `TRIGGER`.)
    source
        .connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS | pw::stream::StreamFlags::TRIGGER, &mut params)
        .map_err(|e| e.to_string())?;

    let weak = mainloop.downgrade();
    let _rx = rx.attach(mainloop.loop_(), {
        let (core, processing, stats, source, trigger) = (core.clone(), processing.clone(), stats.clone(), source.downgrade(), trigger.clone());
        move |message| match message {
            Message::Quit => {
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
            }
            Message::Reconfigure(config) => {
                let mut cur = current.borrow_mut();
                if cur.0 == *config {
                    return;
                }
                if cur.0.settings != config.settings || cur.0.speakers != config.speakers {
                    match Processing::new(&config) {
                        Ok(p) => {
                            stats.latency_samples.store(p.latency(), Relaxed);
                            *processing.lock().unwrap() = p;
                        }
                        Err(e) => tracing::warn!("voice: {e}"),
                    }
                }
                // Drop the old captures before making the new.
                cur.1 = Captures { _streams: Vec::new(), weak: Vec::new() };
                match captures(&core, &config, &processing, &stats, &trigger) {
                    Ok(c) => {
                        let streaming = source.upgrade().is_some_and(|s| matches!(s.state(), pw::stream::StreamState::Streaming));
                        set_active(&c.weak, streaming);
                        cur.1 = c;
                    }
                    Err(e) => tracing::warn!("voice: {e}"),
                }
                // Named after the microphone it listens to now, and at the
                // mode's quantum.
                if (cur.0.description() != config.description() || cur.0.settings.mode != config.settings.mode)
                    && let Some(s) = source.upgrade()
                {
                    let props = properties! {
                        *pw::keys::NODE_DESCRIPTION => config.description(),
                        *pw::keys::NODE_LATENCY => config.latency(),
                        *pw::keys::NODE_FORCE_QUANTUM => force_quantum(&config),
                    };
                    // SAFETY: the stream is alive (upgraded), on its own
                    // loop's thread; PipeWire copies the dictionary.
                    unsafe { pw::sys::pw_stream_update_properties(s.as_raw_ptr(), props.dict().as_raw()) };
                }
                cur.0 = *config;
            }
        }
    });
    tracing::info!("voice: source {NODE}, listening to {}", config.microphone);
    mainloop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The processed source is named after the microphone it listens to,
    /// so it's never mistaken for it.
    #[test]
    fn named_after_its_microphone() {
        let c = |label: &str| Config { microphone: "m".into(), label: label.into(), speakers: None, settings: Settings::default() };
        assert_eq!(c("AirPods").description(), "AirPods (Voice Processing)");
        assert_eq!(c("MacBook Pro Microphone").description(), "MacBook Pro Microphone (Voice Processing)");
        assert_eq!(c(" ").description(), "Microphone (Voice Processing)");
    }

    /// What the source holds back for each quantum: nothing for one the
    /// frames fit (a call's, a frame a cycle), else the most a frame can
    /// be short of done; and run through, it never runs dry.
    #[test]
    fn what_the_source_holds_for_its_quantum() {
        assert_eq!(misfit(480), 0);
        assert_eq!(misfit(960), 0);
        for q in [128, 256, 512, 1024, 2048] {
            assert_eq!(misfit(q), 448, "{q}");
        }
        assert_eq!(misfit(100), 460);
        assert_eq!(misfit(240), 240);
        for q in [32, 64, 100, 128, 160, 192, 240, 256, 300, 441, 480, 512, 640, 960, 1000, 1024, 2048] {
            let hold = misfit(q);
            let (mut mic, mut out) = (0usize, 0usize);
            for cycle in 0..20_000 {
                mic += q;
                out += mic / FRAME * FRAME;
                mic %= FRAME;
                if cycle == 0 {
                    // (Held from the start: silence before the sound.)
                    out += hold + q - (mic + out).min(hold + q);
                }
                assert!(out >= q, "{q}: dry at cycle {cycle}");
                out -= q;
                // (What's held stays the same: the delay.)
                assert_eq!(mic + out, hold, "{q} at cycle {cycle}");
            }
        }
    }
}
