//! A recording's sound: what the computer plays (the default output's
//! monitor) and the microphone, either or both, mixed into stereo at
//! 48 kHz and encoded as AAC as it comes (`olo_video::aac`), its access
//! units handed to the MP4 (`take`). The streams' pieces come on the
//! sound client's thread; a thread of its own mixes and encodes them,
//! woken by each piece (nothing polled). While the recording is paused
//! the sound is dropped too, so the two stay together.

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Instant,
};

use olo_system::recording::{COMPUTER, Capture, RATE};
use olo_video::aac::{self, Aac};

/// Bits a second of the sound.
pub const BITRATE: u32 = 160_000;

/// Frames (of `aac::FRAME`) every stream has before the mixer is woken:
/// the sound only goes into a file, so it's taken in batches (85 ms), not
/// a wakeup for every piece.
const BATCH: usize = 4;

/// What's recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Nothing,
    Computer,
    Microphone,
    Both,
}

impl Which {
    pub fn parse(s: &str) -> Option<Which> {
        Some(match s {
            "none" | "off" => Which::Nothing,
            "computer" => Which::Computer,
            "microphone" | "mic" => Which::Microphone,
            "both" => Which::Both,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Which::Nothing => "none",
            Which::Computer => "computer",
            Which::Microphone => "microphone",
            Which::Both => "both",
        }
    }
}

/// Each stream's samples (stereo, interleaved) not mixed yet, and how
/// many it has had in all (silence it was filled with too), from the
/// recording's start.
struct Bufs {
    data: Vec<Vec<f32>>,
    had: Vec<usize>,
}

impl Bufs {
    /// Stream `i` filled with silence to `len` samples not yet mixed.
    fn fill(&mut self, i: usize, len: usize) {
        let now = self.data[i].len();
        if len > now {
            self.data[i].resize(len, 0.0);
            self.had[i] += len - now;
        }
    }
}

struct Shared {
    bufs: Mutex<Bufs>,
    woken: Condvar,
    done: AtomicBool,
}

/// The sound being recorded.
pub struct Sound {
    _streams: Vec<Capture>,
    shared: Arc<Shared>,
    /// Access units encoded, not yet in the file.
    out: Arc<Mutex<Vec<Vec<u8>>>>,
    track: aac::Track,
    worker: Option<JoinHandle<()>>,
}

impl Sound {
    /// Start recording `which` from now (`start`, the video's first
    /// moment: a stream that comes later is lined up with silence);
    /// `paused` says when to drop what comes.
    pub fn start(which: Which, start: Instant, paused: &'static AtomicBool) -> Result<Sound, String> {
        let sources: Vec<(Option<&str>, &str)> = match which {
            Which::Nothing => return Err("no sound asked for".into()),
            Which::Computer => vec![(Some(COMPUTER), "Recording the screen: the computer's sound")],
            Which::Microphone => vec![(None, "Recording the screen: the microphone")],
            Which::Both => vec![(Some(COMPUTER), "Recording the screen: the computer's sound"), (None, "Recording the screen: the microphone")],
        };
        let aac = Aac::new(RATE, 2, BITRATE)?;
        let track = aac.track();
        let n = sources.len();
        let shared = Arc::new(Shared { bufs: Mutex::new(Bufs { data: vec![Vec::new(); n], had: vec![0; n] }), woken: Condvar::new(), done: AtomicBool::new(false) });
        let mut streams = Vec::new();
        for (i, (source, name)) in sources.into_iter().enumerate() {
            let sh = shared.clone();
            let mut first = true;
            let got = move |samples: &[f32]| {
                if paused.load(Ordering::Relaxed) {
                    return;
                }
                let mut bufs = sh.bufs.lock().unwrap_or_else(|e| e.into_inner());
                // Lined up with the video: silence for the time before
                // this stream's first sound (less the piece itself), but
                // what it was given already while it was late.
                if std::mem::take(&mut first) {
                    let before = (start.elapsed().as_secs_f64() * f64::from(RATE)) as usize;
                    let lead = before.saturating_sub(samples.len() / 2) * 2;
                    let missing = lead.saturating_sub(bufs.had[i]);
                    let len = bufs.data[i].len() + missing;
                    bufs.fill(i, len);
                }
                bufs.data[i].extend_from_slice(samples);
                bufs.had[i] += samples.len();
                // (Woken once every stream has a batch, or one has run a
                // second ahead.)
                let have = bufs.data.iter().map(Vec::len).min().unwrap_or(0);
                let ahead = bufs.data[i].len() > have + RATE as usize * 2;
                drop(bufs);
                if have >= BATCH * aac::FRAME * 2 || ahead {
                    sh.woken.notify_one();
                }
            };
            streams.push(Capture::start(source, 2, name, got)?);
        }
        let out: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let worker = {
            let (shared, out) = (shared.clone(), out.clone());
            std::thread::Builder::new().name("recording sound".into()).spawn(move || mix(aac, &shared, &out)).map_err(|e| e.to_string())?
        };
        Ok(Sound { _streams: streams, shared, out, track, worker: Some(worker) })
    }

    /// The MP4's description of the sound.
    pub fn track(&self) -> &aac::Track {
        &self.track
    }

    /// A handle on the access units encoded so far, for the file.
    pub fn units(&self) -> Arc<Mutex<Vec<Vec<u8>>>> {
        self.out.clone()
    }

    /// Stop: what's left mixed, encoded and handed over.
    pub fn finish(mut self) {
        self._streams.clear();
        self.shared.done.store(true, Ordering::SeqCst);
        self.shared.woken.notify_one();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The mixer and encoder: whole frames as both streams have them (one
/// falling more than a second behind is filled with silence), the rest
/// at the end.
fn mix(mut aac: Aac, shared: &Shared, out: &Mutex<Vec<Vec<u8>>>) {
    let frame = aac::FRAME * 2;
    let mut pcm: Vec<i16> = Vec::with_capacity(frame);
    loop {
        let (ready, done) = {
            let mut bufs = shared.bufs.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let done = shared.done.load(Ordering::SeqCst);
                let longest = bufs.data.iter().map(Vec::len).max().unwrap_or(0);
                // (A stream that has stalled, or not started: silence for
                // it, a second behind.)
                for i in 0..bufs.data.len() {
                    if longest > bufs.data[i].len() + RATE as usize * 2 || (done && bufs.data[i].len() < longest) {
                        bufs.fill(i, longest);
                    }
                }
                let have = bufs.data.iter().map(Vec::len).min().unwrap_or(0);
                let whole = have / frame * frame;
                if whole >= BATCH * frame || done {
                    let take = if done { have } else { whole };
                    let mut mixed = vec![0f32; take];
                    for b in bufs.data.iter_mut() {
                        for (m, s) in mixed.iter_mut().zip(b.drain(..take)) {
                            *m += s;
                        }
                    }
                    break (mixed, done);
                }
                bufs = shared.woken.wait(bufs).unwrap_or_else(|e| e.into_inner());
            }
        };
        let mut units = Vec::new();
        for chunk in ready.chunks(frame) {
            pcm.clear();
            pcm.extend(chunk.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
            // (The last, short, made whole with silence.)
            pcm.resize(frame, 0);
            match aac.encode(&pcm) {
                Ok(Some(au)) => units.push(au),
                Ok(None) => {}
                Err(e) => eprintln!("olo-screenshot: sound: {e}"),
            }
        }
        if done {
            units.extend(aac.flush().unwrap_or_default());
        }
        out.lock().unwrap_or_else(|e| e.into_inner()).extend(units);
        if done {
            return;
        }
    }
}
