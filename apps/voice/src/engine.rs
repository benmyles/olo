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
    /// The speakers' node name, when echo is to be cancelled.
    pub speakers: Option<String>,
    pub settings: Settings,
}

impl Config {
    /// The buffering around the processing: the graph's quantum asked for,
    /// and what the source keeps ahead (samples).
    fn buffering(&self) -> (&'static str, usize) {
        match self.settings.mode {
            // A frame ahead: enough for any quantum's phase against the
            // frames; with a 10 ms quantum, nothing more.
            Mode::Calls => ("480/48000", FRAME),
            Mode::Recording => ("2048/48000", 2048 + 4 * FRAME),
        }
    }
}

/// What the engine has been doing, for the state file.
#[derive(Debug, Default)]
pub struct Stats {
    pub active: AtomicBool,
    pub frames: AtomicU64,
    pub frame_ns: AtomicU64,
    pub dropouts: AtomicU64,
    pub latency_samples: AtomicU32,
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
    /// What the source keeps ahead, and whether it has that yet.
    ahead: usize,
    primed: bool,
}

impl Processing {
    fn new(config: &Config) -> Result<Processing, String> {
        let mut settings = config.settings;
        settings.echo_cancellation &= config.speakers.is_some();
        let voice = Voice::new(settings)?;
        Ok(Processing { voice, mic: Vec::with_capacity(4096), far: Vec::with_capacity(4096), out: VecDeque::with_capacity(16384), ahead: config.buffering().1, primed: false })
    }

    fn reset(&mut self) {
        self.mic.clear();
        self.far.clear();
        self.out.clear();
        self.primed = false;
    }
}

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
) -> Result<(pw::stream::StreamRc, Box<dyn std::any::Any>), String> {
    let name = if monitor { "Olo Voice: echo reference" } else { "Olo Voice: microphone" };
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => if monitor { "olo_voice.echo_reference" } else { "olo_voice.microphone" },
        *pw::keys::NODE_LATENCY => latency,
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

fn captures(core: &pw::core::CoreRc, config: &Config, processing: &Arc<Mutex<Processing>>, stats: &Arc<Stats>) -> Result<Captures, String> {
    let latency = config.buffering().0;
    let mut streams = vec![capture(core, &config.microphone, false, latency, processing.clone(), stats.clone())?];
    if let (Some(speakers), true) = (&config.speakers, config.settings.echo_cancellation) {
        streams.push(capture(core, speakers, true, latency, processing.clone(), stats.clone())?);
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
    stats.latency_samples.store(processing.lock().unwrap().voice.latency_samples() as u32 + config.buffering().1 as u32, Relaxed);
    let current = Rc::new(RefCell::new((config.clone(), captures(&core, &config, &processing, &stats)?)));

    let source = pw::stream::StreamRc::new(
        core.clone(),
        "Olo Voice",
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => NODE,
            *pw::keys::NODE_DESCRIPTION => "Microphone (Voice Processing)",
            *pw::keys::AUDIO_CHANNELS => "1",
            *pw::keys::NODE_LATENCY => config.buffering().0,
            "device.icon-name" => "audio-input-microphone",
        },
    )
    .map_err(|e| e.to_string())?;
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
                    if !p.primed && p.out.len() >= p.ahead + n {
                        p.primed = true;
                    }
                    if p.primed && p.out.len() < n {
                        // Ran dry: count it, and wait to be ahead again.
                        stats.dropouts.fetch_add(1, Relaxed);
                        p.primed = false;
                    }
                    if p.primed {
                        for (o, s) in out.iter_mut().zip(p.out.drain(..n)) {
                            *o = s;
                        }
                        // Never more than a frame beyond what's kept ahead
                        // (the captures' clock a hair fast).
                        let most = p.ahead + n + FRAME;
                        if p.out.len() > most {
                            let extra = p.out.len() - most;
                            p.out.drain(..extra);
                        }
                    } else {
                        out.fill(0.0);
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
    source.connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS, &mut params).map_err(|e| e.to_string())?;

    let weak = mainloop.downgrade();
    let _rx = rx.attach(mainloop.loop_(), {
        let (core, processing, stats, source) = (core.clone(), processing.clone(), stats.clone(), source.downgrade());
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
                            stats.latency_samples.store(p.voice.latency_samples() as u32 + config.buffering().1 as u32, Relaxed);
                            *processing.lock().unwrap() = p;
                        }
                        Err(e) => tracing::warn!("voice: {e}"),
                    }
                }
                // Drop the old captures before making the new.
                cur.1 = Captures { _streams: Vec::new(), weak: Vec::new() };
                match captures(&core, &config, &processing, &stats) {
                    Ok(c) => {
                        let streaming = source.upgrade().is_some_and(|s| matches!(s.state(), pw::stream::StreamState::Streaming));
                        set_active(&c.weak, streaming);
                        cur.1 = c;
                    }
                    Err(e) => tracing::warn!("voice: {e}"),
                }
                cur.0 = *config;
            }
        }
    });
    tracing::info!("voice: source {NODE}, listening to {}", config.microphone);
    mainloop.run();
    Ok(())
}
