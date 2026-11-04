//! Recording a microphone and playing sound back, over the PulseAudio
//! native protocol (which PipeWire serves too), as [`audio`](crate::audio)
//! talks to the sound server: mono, 48 kHz, 32-bit float, in memory only
//! (nothing is written out). For trying things on the person's own voice
//! (Preferences ▸ Sound ▸ Voice Processing ▸ Try It…): a few seconds
//! recorded ([`Recording`]), then played as one of several takes of the
//! same length, switching between them at the same place ([`Player`]).
//!
//! The streams run on the client's own thread; starting and stopping one
//! is a round trip to the server, so callers do it off their UI thread.

use std::{
    ffi::CString,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use olo_sync::Mutex;
use pulseaudio::{
    AsPlaybackSource, Client, PlaybackStream, RecordStream,
    protocol::{
        self, ChannelMap, PlaybackStreamParams, RecordStreamParams, SampleFormat, SampleSpec,
        stream::{BufferAttr, StreamFlags},
    },
};

/// Samples a second.
pub const RATE: u32 = 48_000;

fn spec() -> SampleSpec {
    SampleSpec { format: SampleFormat::Float32Le, channels: 1, sample_rate: RATE }
}

/// `ms` of sound, in bytes.
fn bytes(ms: u32) -> u32 {
    RATE / 1000 * ms * 4
}

/// The sound server, found as [`audio`](crate::audio) finds it (the
/// crate's own lookup gives up at the first place it isn't).
fn connect() -> Result<Client, String> {
    let path = crate::audio::socket_path().ok_or("no sound server")?;
    let socket = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("can't reach the sound server: {e}"))?;
    Client::new_unix(c"Olo", socket, Some(crate::audio::cookie())).map_err(|e| format!("can't reach the sound server: {e}"))
}

fn props(name: &str) -> protocol::Props {
    let mut p = protocol::Props::new();
    p.set(protocol::Prop::ApplicationName, CString::new("Olo").unwrap_or_default());
    p.set(protocol::Prop::MediaName, CString::new(name).unwrap_or_default());
    p
}

/// A microphone being recorded.
pub struct Recording {
    got: Arc<Mutex<Vec<f32>>>,
    stream: Option<RecordStream>,
    _client: Client,
}

impl Recording {
    /// Start recording `source` (a source's name, as the sound server has
    /// it; None: the default).
    pub fn start(source: Option<&str>) -> Result<Recording, String> {
        let client = connect()?;
        let got: Arc<Mutex<Vec<f32>>> = Arc::default();
        let sink = {
            let got = got.clone();
            move |data: &[u8]| {
                let mut g = got.lock().unwrap();
                g.extend(data.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            }
        };
        let params = RecordStreamParams {
            sample_spec: spec(),
            channel_map: ChannelMap::mono(),
            source_name: Some(match source {
                Some(s) => CString::new(s).map_err(|e| e.to_string())?,
                None => protocol::DEFAULT_SOURCE.to_owned(),
            }),
            // 20 ms at a time: a live view keeps up.
            buffer_attr: BufferAttr { fragment_size: bytes(20), ..Default::default() },
            flags: StreamFlags { adjust_latency: true, ..Default::default() },
            props: props("Trying voice processing"),
            ..Default::default()
        };
        let stream = futures_lite::future::block_on(client.create_record_stream(params, sink)).map_err(|e| format!("can't record: {e}"))?;
        Ok(Recording { got, stream: Some(stream), _client: client })
    }

    /// How many samples have come.
    pub fn len(&self) -> usize {
        self.got.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The last `n` samples (fewer at first): for a live view.
    pub fn tail(&self, n: usize) -> Vec<f32> {
        let g = self.got.lock().unwrap();
        g[g.len().saturating_sub(n)..].to_vec()
    }

    /// Stop, with everything recorded.
    pub fn finish(mut self) -> Vec<f32> {
        if let Some(s) = self.stream.take() {
            let _ = futures_lite::future::block_on(s.delete());
        }
        std::mem::take(&mut *self.got.lock().unwrap())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if let Some(s) = self.stream.take() {
            let _ = futures_lite::future::block_on(s.delete());
        }
    }
}

/// What a [`Player`] and its stream share.
struct Playing {
    takes: Vec<Arc<[f32]>>,
    which: AtomicUsize,
    /// The next sample handed to the server (of every take: they line up).
    at: AtomicUsize,
    done: AtomicBool,
}

/// One of several takes of the same sound playing, switchable at the same
/// place.
pub struct Player {
    playing: Arc<Playing>,
    stream: Option<PlaybackStream>,
    _client: Client,
}

impl Player {
    /// Play take `which` of `takes` from sample `from`, to the default
    /// output.
    pub fn start(takes: Vec<Arc<[f32]>>, which: usize, from: usize) -> Result<Player, String> {
        Player::start_to(None, takes, which, from)
    }

    /// [`Player::start`] to output `sink` (a sink's name; None: the
    /// default).
    pub fn start_to(sink: Option<&str>, takes: Vec<Arc<[f32]>>, which: usize, from: usize) -> Result<Player, String> {
        let client = connect()?;
        let playing = Arc::new(Playing { takes, which: AtomicUsize::new(which), at: AtomicUsize::new(from), done: AtomicBool::new(false) });
        let source = {
            let p = playing.clone();
            move |buf: &mut [u8]| {
                let take = &p.takes[p.which.load(Ordering::Relaxed).min(p.takes.len().saturating_sub(1))];
                let at = p.at.load(Ordering::Relaxed);
                let n = (buf.len() / 4).min(take.len().saturating_sub(at));
                if n == 0 {
                    p.done.store(true, Ordering::Relaxed);
                    return 0;
                }
                for (i, s) in take[at..at + n].iter().enumerate() {
                    buf[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                }
                p.at.store(at + n, Ordering::Relaxed);
                n * 4
            }
        };
        let params = PlaybackStreamParams {
            sample_spec: spec(),
            channel_map: ChannelMap::mono(),
            sink_name: Some(match sink {
                Some(s) => CString::new(s).map_err(|e| e.to_string())?,
                None => protocol::DEFAULT_SINK.to_owned(),
            }),
            // 60 ms ahead at most: a switch is heard at once.
            buffer_attr: BufferAttr { target_length: bytes(60), minimum_request_length: bytes(10), pre_buffering: bytes(20), ..Default::default() },
            flags: StreamFlags { adjust_latency: true, ..Default::default() },
            props: props("Trying voice processing"),
            ..Default::default()
        };
        let stream = futures_lite::future::block_on(client.create_playback_stream(params, source.as_playback_source())).map_err(|e| format!("can't play: {e}"))?;
        Ok(Player { playing, stream: Some(stream), _client: client })
    }

    /// Go on with take `which`, from where it is.
    pub fn choose(&self, which: usize) {
        self.playing.which.store(which, Ordering::Relaxed);
    }

    /// The sample being handed to the server now (heard a moment later).
    pub fn position(&self) -> usize {
        self.playing.at.load(Ordering::Relaxed)
    }

    /// Every sample has been handed over.
    pub fn finished(&self) -> bool {
        self.playing.done.load(Ordering::Relaxed)
    }

    /// Stop at once.
    pub fn stop(mut self) {
        if let Some(s) = self.stream.take() {
            let _ = futures_lite::future::block_on(s.delete());
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(s) = self.stream.take() {
            let _ = futures_lite::future::block_on(s.delete());
        }
    }
}

/// In a private sound server only (`tools/dev/pw-sandbox`, its "talk"
/// sink heard by its "mic"): `eval "$(tools/dev/pw-sandbox DIR env)";
/// cargo test --release -p olo-system recording -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, seconds: f32) -> Arc<[f32]> {
        (0..(RATE as f32 * seconds) as usize).map(|i| 0.3 * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()).collect()
    }

    /// How much of `hz` is in `x` (its correlation with a sine there).
    fn at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate() {
            let t = std::f32::consts::TAU * hz * i as f32 / RATE as f32;
            re += v * t.cos();
            im += v * t.sin();
        }
        (re * re + im * im).sqrt() / x.len().max(1) as f32
    }

    #[test]
    #[ignore]
    fn records_what_plays_and_switches_takes_in_place() {
        assert!(std::env::var("PULSE_RUNTIME_PATH").is_ok_and(|s| s.starts_with("/var/tmp/")), "only in a pw-sandbox");
        let rec = Recording::start(Some("mic")).expect("recording");
        let player = Player::start_to(Some("talk"), vec![tone(440.0, 2.0), tone(880.0, 2.0)], 0, 0).expect("playing");
        std::thread::sleep(std::time::Duration::from_millis(1000));
        player.choose(1);
        let switched = player.position();
        while !player.finished() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let got = rec.finish();
        eprintln!("recorded {} samples; switched at {switched}", got.len());
        assert!(got.len() > RATE as usize * 2, "{}", got.len());
        assert!((RATE as usize * 9 / 10..RATE as usize * 12 / 10).contains(&switched), "{switched}");
        // The first half the one tone, the second the other.
        let start = got.iter().position(|v| v.abs() > 0.05).expect("sound");
        let first = &got[start + 4800..start + switched - 9600];
        let second = &got[start + switched + 9600..(start + 2 * RATE as usize - 4800).min(got.len())];
        let (a1, b1, a2, b2) = (at(first, 440.0), at(first, 880.0), at(second, 440.0), at(second, 880.0));
        eprintln!("first: 440 {a1:.3}, 880 {b1:.3}; second: 440 {a2:.3}, 880 {b2:.3}");
        assert!(a1 > 10.0 * b1 && b2 > 10.0 * a2);
    }
}
