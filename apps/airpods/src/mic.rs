//! The AirPods' own microphone, as a source apps record from: their
//! wideband stream (AAC-ELD at 64 kHz, decoded by `olo-eld`), which leaves
//! their sound at music quality, where the headset profile would drop it
//! to call quality.
//!
//! Each pair whose channel is open has a PipeWire source of its own
//! ("AirPods Pro Microphone"), made in process (pipewire-rs) on a thread
//! of its own. The AirPods' stream runs only while something records from
//! it: the source streaming asks the service to start it, and a moment
//! after it stops, to stop it, so idle costs nothing, neither here nor in
//! their battery. Frames go through a short buffer (Bluetooth's timing
//! jitters) to the source's process callback.
//!
//! `OLO_AIRPODS_CAPTURE=FILE` also keeps the frames as they come (each as
//! its timestamp, u32 little-endian, its length, u16, and its bytes), to
//! study and to test the decoder against.

use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    sync::{Arc, OnceLock},
    time::Duration,
};

use olo_sync::Mutex;
use pipewire as pw;
use pw::{properties::properties, spa};

/// The stream's rate (the config says 48 kHz; its frames come 133 a
/// second, of 480 samples).
pub const RATE: u32 = 64_000;
/// The source's rate (the sound server's usual; it converts for others).
const OUT_RATE: u32 = 48_000;
/// What's kept waiting, in the AirPods' samples: AirPods Pro 2 send four
/// frames (30 ms) a message, a message every 31 ms on median and up to
/// 53 ms apart (measured), so 60 ms covers the longest gap with the
/// sound server's cycle (21 ms here) to spare. The resampler steers to
/// keep it there (resample.rs).
const TARGET: usize = 8 * olo_eld::FRAME;
/// For recordings (voice processing tuned for them), where delay doesn't
/// matter and an unbroken take does: 150 ms, room for a Bluetooth
/// hiccup three times the longest gap measured.
const TARGET_RECORDING: usize = 20 * olo_eld::FRAME;

/// What to keep waiting, for what the microphone is for now.
fn target() -> usize {
    match olo_protocol::voice::Settings::load().mode {
        olo_protocol::voice::Mode::Calls => TARGET,
        olo_protocol::voice::Mode::Recording => TARGET_RECORDING,
    }
}
/// A timestamp's step a frame (24 kHz ticks, 7.5 ms).
const TICKS: u32 = 180;
/// How long recording must have stopped before the AirPods' stream is.
const LINGER: Duration = Duration::from_millis(1500);

/// Decoded samples on their way to the source, and the log's counts.
#[derive(Default)]
struct Ring {
    samples: VecDeque<f32>,
    /// Frames decoded, frames that didn't decode, frames missing (filled
    /// in), times the source ran dry.
    frames: u64,
    bad: u64,
    lost: u64,
    dry: u64,
}

struct Mic {
    ring: Arc<Mutex<Ring>>,
    decoder: olo_eld::Decoder,
    /// The last frame's timestamp (a gap is frames lost on the air).
    last_ts: Option<u32>,
    /// Ends the source's thread.
    quit: pw::channel::Sender<()>,
    /// The source's node name, once it has one.
    node: Arc<Mutex<Option<String>>>,
}

struct Capture {
    file: Option<std::fs::File>,
    frames: u64,
}

fn mics() -> &'static Mutex<HashMap<String, Mic>> {
    static M: OnceLock<Mutex<HashMap<String, Mic>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn capture() -> &'static Mutex<Capture> {
    static C: OnceLock<Mutex<Capture>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Capture { file: None, frames: 0 }))
}

/// A pair's channel opened: make its microphone's source.
pub fn linked(addr: &str, name: &str) {
    if mics().lock().unwrap().contains_key(addr) {
        return;
    }
    let Ok(decoder) = olo_eld::Decoder::new(&[0xF8, 0xE6, 0x30, 0x00]) else { return };
    let ring: Arc<Mutex<Ring>> = Default::default();
    let node: Arc<Mutex<Option<String>>> = Default::default();
    let (quit, quit_rx) = pw::channel::channel::<()>();
    let (r, n, a, label) = (ring.clone(), node.clone(), addr.to_owned(), format!("{name} Microphone"));
    let spawned = std::thread::Builder::new().name("airpods-mic".into()).spawn(move || {
        if let Err(e) = source(&a, &label, r, n, quit_rx) {
            tracing::warn!("airpods {a}: the microphone's source: {e}");
        }
    });
    if spawned.is_ok() {
        mics().lock().unwrap().insert(addr.to_owned(), Mic { ring, decoder, last_ts: None, quit, node });
    }
}

/// Their channel closed: the source goes.
pub fn unlinked(addr: &str) {
    if let Some(m) = mics().lock().unwrap().remove(addr) {
        let _ = m.quit.send(());
    }
}

/// The source's node name ("airpods_mic.AA_BB_…"), once made.
pub fn node(addr: &str) -> Option<String> {
    mics().lock().unwrap().get(addr).and_then(|m| m.node.lock().unwrap().clone())
}

/// The stream was started or stopped.
pub fn switched(addr: &str, on: bool) {
    let mut c = capture().lock().unwrap();
    if on {
        c.file = std::env::var_os("OLO_AIRPODS_CAPTURE").and_then(|p| std::fs::File::create(p).ok());
        c.frames = 0;
        tracing::info!("airpods {addr}: microphone on");
    } else {
        tracing::info!("airpods {addr}: microphone off ({} frames)", c.frames);
        c.file = None;
    }
    if let Some(m) = mics().lock().unwrap().get_mut(addr) {
        let r = std::mem::take(&mut *m.ring.lock().unwrap());
        if !on {
            tracing::info!("airpods {addr}: microphone: {} frames, {} not read, {} lost on the air (filled in), ran dry {} times", r.frames, r.bad, r.lost, r.dry);
        }
        m.last_ts = None;
    }
}

/// Frames from their microphone: decoded into the source's buffer.
pub fn frames(addr: &str, frames: Vec<(u32, Vec<u8>)>) {
    {
        let mut c = capture().lock().unwrap();
        c.frames += frames.len() as u64;
        if let Some(f) = &mut c.file {
            for (ts, au) in &frames {
                let _ = f.write_all(&ts.to_le_bytes()).and_then(|_| f.write_all(&(au.len() as u16).to_le_bytes())).and_then(|_| f.write_all(au));
            }
        }
    }
    let mut mics = mics().lock().unwrap();
    let Some(m) = mics.get_mut(addr) else { return };
    let mut pcm = [0f32; olo_eld::FRAME];
    for (ts, au) in frames {
        // Frames lost on the air: as many decoded empty, which lets the
        // last ones' sound die away through the filter bank (no gap, no
        // click), up to a quarter of a second.
        let lost = m.last_ts.map_or(0, |l| (ts.wrapping_sub(l) / TICKS).saturating_sub(1)).min(32);
        m.last_ts = Some(ts);
        let mut ring = m.ring.lock().unwrap();
        for _ in 0..lost {
            let _ = m.decoder.decode(&[], &mut pcm);
            ring.samples.extend(pcm);
        }
        ring.lost += lost as u64;
        ring.bad += m.decoder.decode(&au, &mut pcm).is_err() as u64;
        ring.frames += 1;
        ring.samples.extend(pcm);
        // Nobody taking them (recording stalled): keep a second at most.
        let most = RATE as usize;
        if ring.samples.len() > most {
            let extra = ring.samples.len() - TARGET_RECORDING;
            ring.samples.drain(..extra);
        }
    }
}

/// The source, on its own thread until told to quit: PipeWire's loop.
fn source(addr: &str, label: &str, ring: Arc<Mutex<Ring>>, node: Arc<Mutex<Option<String>>>, quit: pw::channel::Receiver<()>) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| format!("can't reach PipeWire: {e}"))?;
    let name = format!("airpods_mic.{}", addr.replace(':', "_"));
    let stream = pw::stream::StreamBox::new(
        &core,
        label,
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => name.as_str(),
            *pw::keys::NODE_DESCRIPTION => label,
            *pw::keys::AUDIO_CHANNELS => "1",
            "device.icon-name" => "audio-headset",
            // Never the input by itself (the computer's microphones are,
            // unless the person chooses this).
            "priority.session" => "100",
            "priority.driver" => "100",
            "api.bluez5.address" => addr,
        },
    )
    .map_err(|e| e.to_string())?;
    // Recording or not, as the stream says (it streams while something
    // records from it): the AirPods' stream follows, stopping only once
    // recording has stayed stopped a moment.
    let changes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let a = addr.to_owned();
    let weak = mainloop.downgrade();
    let _listener = stream
        .add_local_listener_with_user_data(crate::resample::Resampler::new(RATE, OUT_RATE, TARGET))
        .state_changed({
            let (a, node, name, ring) = (a.clone(), node.clone(), name.clone(), ring.clone());
            move |_, resampler, _, new| {
                use std::sync::atomic::Ordering::Relaxed;
                let n = changes.fetch_add(1, Relaxed) + 1;
                match &new {
                    pw::stream::StreamState::Streaming => {
                        resampler.reset();
                        resampler.set_target(target());
                        ring.lock().unwrap().samples.clear();
                        crate::service::microphone_wanted(&a, true);
                    }
                    pw::stream::StreamState::Paused => {
                        *node.lock().unwrap() = Some(name.clone());
                        let (a, changes) = (a.clone(), changes.clone());
                        std::thread::spawn(move || {
                            std::thread::sleep(LINGER);
                            if changes.load(Relaxed) == n {
                                let _ = crate::service::microphone_wanted(&a, false);
                            }
                        });
                    }
                    pw::stream::StreamState::Error(e) => tracing::warn!("airpods: the microphone's source: {e}"),
                    _ => {}
                }
            }
        })
        .process(move |stream, resampler| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            // Exactly what this cycle asks for (the buffer holds more).
            let requested = buffer.requested() as usize;
            {
                let mut r = ring.lock().unwrap();
                let was_primed = resampler.waiting() > 0;
                resampler.push(r.samples.drain(..));
                if was_primed && resampler.waiting() == 0 {
                    r.dry += 1;
                }
            }
            let data = &mut buffer.datas_mut()[0];
            let n = if let Some(slice) = data.data() {
                let n = if requested == 0 { slice.len() / 4 } else { requested.min(slice.len() / 4) };
                let out: &mut [f32] = bytemuck_f32(&mut slice[..n * 4]);
                resampler.pull(out);
                n
            } else {
                0
            };
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = 4;
            *chunk.size_mut() = (n * 4) as u32;
        })
        .register()
        .map_err(|e| e.to_string())?;
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(OUT_RATE);
    info.set_channels(1);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let format = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object { type_: spa::sys::SPA_TYPE_OBJECT_Format, id: spa::sys::SPA_PARAM_EnumFormat, properties: info.into() }),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner();
    let mut params = [pw::spa::pod::Pod::from_bytes(&format).ok_or("format")?];
    stream
        .connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS, &mut params)
        .map_err(|e| e.to_string())?;
    let _quit = quit.attach(mainloop.loop_(), move |_| {
        if let Some(l) = weak.upgrade() {
            l.quit();
        }
    });
    tracing::info!("airpods {addr}: microphone source {name}");
    mainloop.run();
    let _ = crate::service::microphone_wanted(addr, false);
    Ok(())
}

/// A buffer's bytes as samples (PipeWire's buffers are aligned for them).
fn bytemuck_f32(bytes: &mut [u8]) -> &mut [f32] {
    let (head, samples, _) = unsafe { bytes.align_to_mut::<f32>() };
    debug_assert!(head.is_empty());
    samples
}
