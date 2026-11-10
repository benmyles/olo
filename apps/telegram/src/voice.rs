//! Voice messages recorded: the default microphone
//! (`olo_system::recording::Capture`), coded to Ogg Opus as it comes
//! (`olo_video::opus`, a page a second to a file in Telegram's cache), its
//! loudness kept for the message's waveform and for the composer's live
//! bars (a level every 100 ms: what changes, nothing more). Starting and
//! stopping the stream is a round trip to the sound server, so both are
//! done on a thread; the coding is on the sound client's.

use std::{
    fs::File,
    io::BufWriter,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use olo_system::recording::{Capture, RATE};
use olo_ui::Proxy;
use olo_video::opus::OggOpus;

/// Bits a second: a voice, clearly (SILK has nothing to do with more:
/// `olo_video::opus`).
const BITRATE: i32 = 24_000;
/// A level to the composer every this many samples (100 ms).
const LEVEL_EVERY: usize = RATE as usize / 10;
/// Bars in a voice message's waveform, as Telegram's apps make them.
const BARS: usize = 100;
/// The longest a voice message records (then it stops by itself).
pub const LONGEST: f32 = 60.0 * 60.0;
/// Shorter than this, it isn't sent (a slip of the button).
pub const SHORTEST: f32 = 0.5;

/// From the recorder to the app, each with the recording's token (a
/// recording ended or begun again leaves the old one's late messages
/// unheard).
pub enum Msg {
    Started(u64, Result<Recorder, String>),
    /// How loud the last 100 ms were (0–1), and the seconds so far.
    Level(u64, f32, f32),
    /// Finished: the note, or why not.
    Done(u64, Result<Note, String>),
}

/// A voice message recorded, ready to send.
pub struct Note {
    pub path: PathBuf,
    pub seconds: f32,
    /// Its bars, 0–31 (Telegram packs them in five bits).
    pub waveform: Vec<u8>,
}

struct Shared {
    out: Option<OggOpus<BufWriter<File>>>,
    /// The loudest sample of each level's stretch, all of them.
    peaks: Vec<f32>,
    peak: f32,
    since_level: usize,
    error: Option<String>,
}

/// The microphone being recorded.
pub struct Recorder {
    capture: Capture,
    shared: Arc<Mutex<Shared>>,
    path: PathBuf,
}

/// Start recording, on a thread: `Msg::Started` comes with the recorder.
pub fn start(token: u64, proxy: Proxy) {
    std::thread::Builder::new()
        .name("voice".into())
        .spawn(move || {
            let r = open(token, &proxy);
            proxy.send(Msg::Started(token, r));
        })
        .expect("thread");
}

fn open(token: u64, proxy: &Proxy) -> Result<Recorder, String> {
    let dir = olo_tg::account::private_dir().map_err(|e| e.to_string())?.join("voice");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tidy(&dir);
    let path = dir.join(format!("voice-{}-{token}.ogg", std::process::id()));
    let file = File::create(&path).map_err(|e| e.to_string())?;
    let out = OggOpus::new(BufWriter::new(file), BITRATE).map_err(|e| e.to_string())?;
    let shared = Arc::new(Mutex::new(Shared { out: Some(out), peaks: Vec::new(), peak: 0.0, since_level: 0, error: None }));
    let (s, p) = (shared.clone(), proxy.clone());
    let capture = Capture::start(None, 1, "Voice message", move |pcm| {
        let Ok(mut s) = s.lock() else { return };
        let s = &mut *s;
        let Some(out) = s.out.as_mut() else { return };
        if let Err(e) = out.push(pcm) {
            s.error = Some(e.to_string());
            s.out = None;
            return;
        }
        let seconds = out.seconds() as f32;
        for &v in pcm {
            s.peak = s.peak.max(v.abs());
            s.since_level += 1;
            if s.since_level == LEVEL_EVERY {
                s.peaks.push(s.peak);
                p.send(Msg::Level(token, s.peak, seconds));
                s.peak = 0.0;
                s.since_level = 0;
            }
        }
    });
    match capture {
        Ok(capture) => Ok(Recorder { capture, shared, path }),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

/// Voice messages left from before (sent, or a crash's): gone after a
/// day.
fn tidy(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 24 * 3600);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl Recorder {
    /// Stop and finish the file, on a thread: `Msg::Done` comes with the
    /// note.
    pub fn finish(self, token: u64, proxy: Proxy) {
        std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || {
                let r = self.end();
                proxy.send(Msg::Done(token, r));
            })
            .expect("thread");
    }

    /// Stop and throw it away, on a thread.
    pub fn cancel(self) {
        std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || {
                let path = self.path.clone();
                drop(self.capture);
                let _ = std::fs::remove_file(path);
            })
            .expect("thread");
    }

    fn end(self) -> Result<Note, String> {
        // Stopped first: nothing more comes to the file.
        drop(self.capture);
        let (out, peaks, error) = {
            let mut s = self.shared.lock().map_err(|_| "the recording failed".to_owned())?;
            if s.peak > 0.0 {
                let p = s.peak;
                s.peaks.push(p);
            }
            (s.out.take(), std::mem::take(&mut s.peaks), s.error.take())
        };
        let fail = |why: String| {
            let _ = std::fs::remove_file(&self.path);
            Err(why)
        };
        if let Some(e) = error {
            return fail(e);
        }
        let Some(out) = out else { return fail("the recording failed".into()) };
        let (_, seconds) = match out.finish() {
            Ok(r) => r,
            Err(e) => return fail(e.to_string()),
        };
        Ok(Note { path: self.path, seconds: seconds as f32, waveform: waveform(&peaks) })
    }
}

/// `BARS` bars (0–31) from the loudness of each stretch: the loudest in
/// each bar's share, against the loudest of all, as Telegram's apps draw
/// a voice message.
pub fn waveform(peaks: &[f32]) -> Vec<u8> {
    if peaks.is_empty() {
        return vec![0; BARS];
    }
    let top = peaks.iter().copied().fold(0.0f32, f32::max).max(1e-4);
    (0..BARS)
        .map(|i| {
            let (a, b) = (i * peaks.len() / BARS, ((i + 1) * peaks.len() / BARS).max(i * peaks.len() / BARS + 1).min(peaks.len()));
            let p = peaks[a.min(peaks.len() - 1)..b].iter().copied().fold(0.0f32, f32::max);
            (p / top * 31.0).round().clamp(0.0, 31.0) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::waveform;

    #[test]
    fn a_waveform_of_a_hundred_bars() {
        assert_eq!(waveform(&[]), vec![0; 100]);
        // Fewer stretches than bars: each stretch stands for several.
        let w = waveform(&[0.1, 0.5, 0.25]);
        assert_eq!(w.len(), 100);
        assert_eq!(*w.iter().max().unwrap(), 31);
        assert_eq!(w[0], 6);
        assert_eq!(w[99], 16);
        // More: the loudest of each bar's share.
        let peaks: Vec<f32> = (0..1000).map(|i| if i == 505 { 1.0 } else { 0.1 }).collect();
        let w = waveform(&peaks);
        assert_eq!(w[50], 31);
        assert_eq!(w[49], 3);
    }
}
