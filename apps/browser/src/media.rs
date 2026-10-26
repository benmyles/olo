//! Audio and video in pages. Servo plays media through a servo-media
//! backend, which in a build without gstreamer plays nothing; ours plays
//! it with the decoders Viewer uses (tabula-preview: H.264 and AV1 in
//! mp4 / WebM / Matroska, and every sound format symphonia reads), sound
//! going to the sound server over the PulseAudio protocol in process.
//!
//! Servo fetches the file and pushes its bytes to the player as they
//! arrive. They gather in a [`Download`]; the decoders read it through
//! readers that wait for bytes not yet there, so playback can start
//! before the download ends and seeking ahead just waits. We never ask
//! Servo to stop or restart the fetch. A driver thread per player keeps
//! the clock, hands Servo each picture as BGRA, and reports position,
//! state and the end. Everything that isn't the media element (Web
//! Audio, streams, WebRTC) stays with servo-media's dummy backend.

use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use servo_base::generic_channel::GenericCallback;
use servo_media::{
    Backend, BackendInit, MediaInstanceError, ServoMedia, SupportsMediaType,
    audio::{
        context::{AudioContext, AudioContextOptions},
        sink::AudioSinkError,
    },
    player::{
        Player, PlayerError, PlayerEvent, PlaybackState, StreamType,
        audio::AudioRenderer,
        context::PlayerGLContext,
        metadata::Metadata,
        video::{Buffer, VideoFrame, VideoFrameData, VideoFrameRenderer},
    },
    streams::{
        MediaOutput, MediaSocket, MediaStreamType, capture::MediaTrackConstraintSet, device_monitor::MediaDeviceMonitor, registry::MediaStreamId,
    },
    traits::{ClientContextId, MediaInstance},
    webrtc::{WebRtcController, WebRtcSignaller},
};
use servo_media_dummy::DummyBackend;
use tabula_preview::{
    input::{Input, ReadSeek},
    video::{self, Options, Picture, Video},
};

/// Bytes to have before looking at the file, unless it's all there.
const START_BYTES: usize = 64 * 1024;

/// How often the page hears the position while playing.
const POSITION_EVERY: Duration = Duration::from_millis(250);

/// Pictures a page's players may be decoding at once, in pixels: about
/// three 2920×2000 videos or eight 1080p ones, less where a page may use
/// less than 1.1 GB ([`crate::guard::page_memory`], 64 bytes a pixel).
/// OpenH264 needs some 20 bytes a pixel and the frames ahead and on show
/// another 20, and pages (slack.com) autoplay twenty large videos at once,
/// mostly out of sight: past this, the players that started playing
/// longest ago hold their picture and let their decoders go until they're
/// played or sought again.
fn decode_pixels() -> u64 {
    (crate::guard::page_memory() / 128).min(18_000_000)
}

/// A paused player keeps its decoder this long, then lets it go (keeping
/// its picture) until it plays again.
const IDLE_RELEASE: Duration = Duration::from_secs(2);

/// A decoder is never asked to let go sooner than this after it started
/// (so it shows a picture, and twenty players can't take turns forever).
const TENURE: Duration = Duration::from_secs(1);

/// How often a player waiting for room in the budget asks again (and
/// one waiting patiently).
const RETRY: Duration = Duration::from_millis(50);
const PATIENT_RETRY: Duration = Duration::from_millis(250);

/// The players decoding pictures in this process.
static DECODING: Mutex<Vec<Decoding>> = Mutex::new(Vec::new());

struct Decoding {
    id: usize,
    pixels: u64,
    /// When it last started playing (the latest are likeliest in view).
    since: Instant,
    /// Heard, so never held (only muted or silent videos are).
    audible: bool,
    /// Asked to let go, and still counted until its decoder has gone.
    leaving: bool,
    tx: mpsc::Sender<Cmd>,
}

/// May player `id` decode `pixels`-sized pictures? Yes while the page
/// stays within [`decode_pixels`] (or nothing else is decoding). If not,
/// and it may `evict`, the players that started longest ago (and at least
/// [`TENURE`] ago) are asked to let go; either way it's no for now: they
/// count until they've gone, and the player asks again.
fn start_decoding(id: usize, pixels: u64, audible: bool, evict: bool, tx: &mpsc::Sender<Cmd>) -> bool {
    let mut d = DECODING.lock().unwrap();
    if let Some(x) = d.iter_mut().find(|x| x.id == id) {
        if x.leaving {
            return false;
        }
        (x.since, x.audible) = (Instant::now(), audible);
        return true;
    }
    let total: u64 = d.iter().map(|x| x.pixels).sum();
    let budget = decode_pixels();
    if d.is_empty() || total + pixels <= budget {
        d.push(Decoding { id, pixels, since: Instant::now(), audible, leaving: false, tx: tx.clone() });
        return true;
    }
    if !evict {
        return false;
    }
    let mut over = (total + pixels).saturating_sub(budget);
    let mut going: Vec<usize> = (0..d.len()).filter(|&i| !d[i].leaving && !d[i].audible && d[i].since.elapsed() >= TENURE).collect();
    going.sort_by_key(|&i| d[i].since);
    for i in going {
        if over == 0 {
            break;
        }
        tracing::debug!("media {}: holding its picture (page decoding {total} pixels)", d[i].id);
        d[i].leaving = true;
        let _ = d[i].tx.send(Cmd::Hold);
        over = over.saturating_sub(d[i].pixels);
    }
    false
}

fn stop_decoding(id: usize) {
    DECODING.lock().unwrap().retain(|x| x.id != id);
}

/// A page script working around two things in Servo 0.7's media
/// element:
///
/// - It runs "time marches on" (and so fires `timeupdate`, which time
///   displays follow) only once something has read `textTracks`.
/// - Once playback reaches the end it stays at HAVE_CURRENT_DATA, so it
///   never plays again: `play()` on an element that ended reloads it (the
///   bytes come from the cache) and goes back to where it was scrubbed to.
pub fn script() -> String {
    r#"(function() {
  var P = window.HTMLMediaElement && HTMLMediaElement.prototype;
  if (!P) return;
  var ended = new WeakSet();
  // Muted videos out of sight pause until they're back in view (as other
  // browsers do), so their decoders go (see decode_pixels in media.rs).
  var away = new WeakSet(), hidden = new WeakSet(), pause = P.pause, play = P.play;
  function quiet(v) { return v.muted || v.volume == 0; }
  function leave(v) {
    if (hidden.has(v) && !v.paused && quiet(v)) {
      away.add(v);
      pause.call(v);
    }
  }
  var seen = window.IntersectionObserver && new IntersectionObserver(function(entries) {
    entries.forEach(function(e) {
      var v = e.target;
      if (!e.isIntersecting) {
        hidden.add(v);
        leave(v);
      } else {
        hidden.delete(v);
        if (away.has(v)) {
          away.delete(v);
          play.call(v).catch(function() {});
        }
      }
    });
  });
  // Autoplay (or a script) starting one already out of sight.
  document.addEventListener('playing', function(e) { var m = media(e); if (m) leave(m); }, true);
  function touch(e) {
    try { void e.textTracks; } catch (x) {}
    if (seen && e instanceof HTMLVideoElement) seen.observe(e);
  }
  function media(e) { return e.target instanceof HTMLMediaElement ? e.target : null; }
  // Elements as the parser or script inserts them; loadstart for any
  // loaded while out of the document.
  function scan(n) {
    if (n instanceof HTMLMediaElement) touch(n);
    else if (n.querySelectorAll) Array.prototype.forEach.call(n.querySelectorAll('video, audio'), touch);
  }
  new MutationObserver(function(records) {
    records.forEach(function(r) { Array.prototype.forEach.call(r.addedNodes, scan); });
  }).observe(document, {childList: true, subtree: true});
  document.addEventListener('loadstart', function(e) { var m = media(e); if (m) touch(m); }, true);
  document.addEventListener('ended', function(e) { var m = media(e); if (m) ended.add(m); }, true);
  P.pause = function() {
    away.delete(this);
    return pause.apply(this, arguments);
  };
  P.play = function() {
    touch(this);
    away.delete(this);
    if (ended.has(this)) {
      ended.delete(this);
      var m = this, t = this.currentTime;
      if (t > 0 && t < this.duration) {
        m.addEventListener('loadedmetadata', function() { m.currentTime = t; }, {once: true});
      }
      this.load();
    }
    return play.apply(this, arguments);
  };
})();"#
        .into()
}

/// Make ours the media backend. Servo installs the dummy one as it
/// starts, but the first to arrive wins, so this runs (and waits until
/// it has) before Servo is built.
pub fn install() {
    ServoMedia::init_with_backend(|| Box::new(Media));
    let _ = ServoMedia::get();
}

struct Media;

impl BackendInit for Media {
    fn init() -> Box<dyn Backend> {
        Box::new(Media)
    }
}

impl Backend for Media {
    fn create_player(
        &self,
        _: &ClientContextId,
        _: StreamType,
        events: GenericCallback<PlayerEvent>,
        renderer: Option<Arc<Mutex<dyn VideoFrameRenderer>>>,
        _: Option<Arc<Mutex<dyn AudioRenderer>>>,
        _: Box<dyn PlayerGLContext>,
    ) -> Arc<Mutex<dyn Player>> {
        Arc::new(Mutex::new(MediaPlayer::new(events, renderer)))
    }

    fn create_audiostream(&self) -> MediaStreamId {
        DummyBackend.create_audiostream()
    }

    fn create_videostream(&self) -> MediaStreamId {
        DummyBackend.create_videostream()
    }

    fn create_stream_output(&self) -> Box<dyn MediaOutput> {
        DummyBackend.create_stream_output()
    }

    fn create_stream_and_socket(&self, ty: MediaStreamType) -> (Box<dyn MediaSocket>, MediaStreamId) {
        DummyBackend.create_stream_and_socket(ty)
    }

    fn create_audioinput_stream(&self, set: MediaTrackConstraintSet) -> Option<MediaStreamId> {
        DummyBackend.create_audioinput_stream(set)
    }

    fn create_videoinput_stream(&self, set: MediaTrackConstraintSet) -> Option<MediaStreamId> {
        DummyBackend.create_videoinput_stream(set)
    }

    fn create_audio_context(&self, id: &ClientContextId, options: AudioContextOptions) -> Result<Arc<Mutex<AudioContext>>, AudioSinkError> {
        DummyBackend.create_audio_context(id, options)
    }

    fn create_webrtc(&self, signaller: Box<dyn WebRtcSignaller>) -> WebRtcController {
        DummyBackend.create_webrtc(signaller)
    }

    fn can_play_type(&self, media_type: &str) -> SupportsMediaType {
        match crate::guard::pid_bit(media_type) {
            Some(true) => SupportsMediaType::Maybe,
            Some(false) => SupportsMediaType::No,
            None => can_play(media_type),
        }
    }

    fn get_device_monitor(&self) -> Box<dyn MediaDeviceMonitor> {
        DummyBackend.get_device_monitor()
    }
}

/// `canPlayType`: what we decode. Container types with codecs we don't
/// know of answer "maybe", as the spec asks.
fn can_play(media_type: &str) -> SupportsMediaType {
    let mut parts = media_type.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let codecs: Vec<String> = parts
        .filter_map(|p| p.trim().strip_prefix("codecs=").map(|c| c.trim_matches(['"', '\'']).to_ascii_lowercase()))
        .flat_map(|c| c.split(',').map(|c| c.trim().to_owned()).collect::<Vec<_>>())
        .collect();
    let known = |c: &str| {
        ["avc1", "avc3", "av01", "mp4a", "opus", "vorbis", "flac", "mp3", "mp4a.40", "alac", "pcm"].iter().any(|k| c == *k || c.starts_with(&format!("{k}.")))
    };
    let container = matches!(
        mime.as_str(),
        "video/mp4"
            | "audio/mp4"
            | "audio/x-m4a"
            | "video/webm"
            | "audio/webm"
            | "video/x-matroska"
            | "audio/x-matroska"
            | "audio/mpeg"
            | "audio/mp3"
            | "audio/ogg"
            | "application/ogg"
            | "audio/flac"
            | "audio/x-flac"
            | "audio/wav"
            | "audio/wave"
            | "audio/x-wav"
            | "audio/aac"
            | "audio/aiff"
            | "audio/x-aiff"
    );
    if !container {
        SupportsMediaType::No
    } else if codecs.is_empty() {
        SupportsMediaType::Maybe
    } else if codecs.iter().all(|c| known(c)) {
        SupportsMediaType::Probably
    } else {
        SupportsMediaType::No
    }
}

/// How much of a download stays in memory (the start, which probing and
/// playback read most); the rest goes to disk, so a page full of videos
/// holds only what it's decoding, not every file it fetched.
const IN_MEMORY: usize = 1 << 20;

/// The file as Servo has fetched it so far.
#[derive(Default)]
struct Bytes {
    /// The first [`IN_MEMORY`] bytes (all of them, if no file could be made).
    data: Vec<u8>,
    /// The rest.
    rest: Option<Arc<std::fs::File>>,
    /// How much has come.
    len: u64,
    /// The size the server gave, if any.
    size: Option<u64>,
    /// All of it is here.
    complete: bool,
    /// The player is gone: readers give up.
    closed: bool,
}

#[derive(Default)]
struct Download {
    bytes: Mutex<Bytes>,
    arrived: Condvar,
}

impl Bytes {
    fn push(&mut self, data: Vec<u8>) {
        use std::os::unix::fs::FileExt;
        let mut data = &data[..];
        if self.rest.is_none() && self.data.len() < IN_MEMORY {
            let n = data.len().min(IN_MEMORY - self.data.len());
            self.data.extend_from_slice(&data[..n]);
            self.len += n as u64;
            data = &data[n..];
            if data.is_empty() {
                return;
            }
            self.rest = tabula_xdg::scratch_file().inspect_err(|e| tracing::debug!("media: keeping a download in memory: {e}")).ok().map(Arc::new);
        }
        if let Some(f) = &self.rest {
            if f.write_all_at(data, self.len - self.data.len() as u64).is_ok() {
                self.len += data.len() as u64;
                return;
            }
            tracing::debug!("media: could not write a download to disk; keeping it in memory");
            // Bring back what's on disk and carry on in memory.
            let mut back = vec![0; (self.len - self.data.len() as u64) as usize];
            if f.read_exact_at(&mut back, 0).is_err() {
                self.closed = true;
                return;
            }
            self.data.extend_from_slice(&back);
            self.rest = None;
        }
        self.data.extend_from_slice(data);
        self.len += data.len() as u64;
    }
}

impl Download {
    fn close(&self) {
        self.bytes.lock().unwrap().closed = true;
        self.arrived.notify_all();
    }
}

/// Reads the download, waiting for bytes still to come.
struct Reader {
    download: Arc<Download>,
    pos: u64,
    size: u64,
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut b = self.download.bytes.lock().unwrap();
        loop {
            if b.closed {
                return Err(io::Error::other("stopped"));
            }
            let have = b.len;
            if self.pos < have {
                let mut n = buf.len().min((have - self.pos) as usize);
                let mem = b.data.len() as u64;
                if self.pos < mem {
                    n = n.min((mem - self.pos) as usize);
                    buf[..n].copy_from_slice(&b.data[self.pos as usize..self.pos as usize + n]);
                } else {
                    // From disk, without holding up the bytes coming in.
                    let f = b.rest.clone().ok_or_else(|| io::Error::other("lost"))?;
                    drop(b);
                    n = std::os::unix::fs::FileExt::read_at(&*f, &mut buf[..n], self.pos - mem)?;
                }
                self.pos += n as u64;
                return Ok(n);
            }
            if b.complete || self.pos >= self.size {
                return Ok(0);
            }
            b = self.download.arrived.wait(b).unwrap();
        }
    }
}

impl Seek for Reader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(d) => self.size as i64 + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if to < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "before the start"));
        }
        self.pos = to as u64;
        Ok(self.pos)
    }
}

/// The container, from its first bytes: the decoders take a file
/// extension as the hint.
fn sniff(head: &[u8]) -> &'static str {
    if head.get(4..8) == Some(b"ftyp") {
        "mp4"
    } else if head.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        "webm"
    } else if head.starts_with(b"OggS") {
        "ogg"
    } else if head.starts_with(b"fLaC") {
        "flac"
    } else if head.starts_with(b"RIFF") {
        "wav"
    } else if head.starts_with(b"FORM") {
        "aiff"
    } else if head.starts_with(b"ID3") || (head.len() > 1 && head[0] == 0xff && head[1] & 0xe0 == 0xe0) {
        "mp3"
    } else {
        ""
    }
}

enum Cmd {
    Play,
    Pause,
    Seek(f64),
    Volume(f64),
    /// Let the decoder go, keeping the picture (see [`decode_pixels`]).
    Hold,
}

/// What the page's element sees of the player, kept up to date by the
/// driver and by calls from script.
struct Status {
    paused: bool,
    muted: bool,
    volume: f64,
    duration: Option<f64>,
}

struct MediaPlayer {
    id: usize,
    download: Arc<Download>,
    status: Arc<Mutex<Status>>,
    tx: mpsc::Sender<Cmd>,
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

impl MediaPlayer {
    fn new(events: GenericCallback<PlayerEvent>, renderer: Option<Arc<Mutex<dyn VideoFrameRenderer>>>) -> MediaPlayer {
        let download = Arc::new(Download::default());
        let status = Arc::new(Mutex::new(Status { paused: true, muted: false, volume: 1.0, duration: None }));
        let (tx, rx) = mpsc::channel();
        let (d, s, me) = (download.clone(), status.clone(), tx.clone());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                let mut driver = Driver {
                    id,
                    events,
                    renderer,
                    status: s,
                    rx,
                    me,
                    playing: false,
                    seeking: None,
                    ended: false,
                    told: None,
                    volume: 1.0,
                    pixels: 0,
                    sound: false,
                    shown: false,
                    idle: None,
                    want: true,
                    patient: true,
                    leaving: None,
                };
                if let Err(e) = driver.run(&d) {
                    tracing::debug!("media {id}: {e}");
                    let _ = driver.events.send(PlayerEvent::Error(e));
                }
                stop_decoding(id);
            })
            .ok();
        MediaPlayer { id, download, status, tx }
    }

    fn send(&self, cmd: Cmd) -> Result<(), PlayerError> {
        self.tx.send(cmd).map_err(|_| PlayerError::Backend("The player has stopped.".into()))
    }

    fn send_volume(&self) -> Result<(), PlayerError> {
        let v = {
            let s = self.status.lock().unwrap();
            if s.muted { 0.0 } else { s.volume }
        };
        self.send(Cmd::Volume(v))
    }
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        self.download.close();
    }
}

impl MediaInstance for MediaPlayer {
    fn get_id(&self) -> usize {
        self.id
    }

    fn mute(&self, val: bool) -> Result<(), MediaInstanceError> {
        self.set_mute(val).map_err(|_| MediaInstanceError)
    }

    fn suspend(&self) -> Result<(), MediaInstanceError> {
        self.pause().map_err(|_| MediaInstanceError)
    }

    fn resume(&self) -> Result<(), MediaInstanceError> {
        self.play().map_err(|_| MediaInstanceError)
    }
}

impl Player for MediaPlayer {
    fn play(&self) -> Result<(), PlayerError> {
        self.status.lock().unwrap().paused = false;
        self.send(Cmd::Play)
    }

    fn pause(&self) -> Result<(), PlayerError> {
        self.status.lock().unwrap().paused = true;
        self.send(Cmd::Pause)
    }

    fn paused(&self) -> bool {
        self.status.lock().unwrap().paused
    }

    fn can_resume(&self) -> bool {
        true
    }

    fn stop(&self) -> Result<(), PlayerError> {
        self.download.close();
        Ok(())
    }

    fn seek(&self, time: f64) -> Result<(), PlayerError> {
        self.send(Cmd::Seek(time))
    }

    fn seekable(&self) -> Vec<Range<f64>> {
        self.status.lock().unwrap().duration.map(|d| 0.0..d).into_iter().collect()
    }

    fn set_mute(&self, muted: bool) -> Result<(), PlayerError> {
        self.status.lock().unwrap().muted = muted;
        self.send_volume()
    }

    fn muted(&self) -> bool {
        self.status.lock().unwrap().muted
    }

    fn set_volume(&self, volume: f64) -> Result<(), PlayerError> {
        self.status.lock().unwrap().volume = volume;
        self.send_volume()
    }

    fn volume(&self) -> f64 {
        self.status.lock().unwrap().volume
    }

    fn set_input_size(&self, size: u64) -> Result<(), PlayerError> {
        let mut b = self.download.bytes.lock().unwrap();
        if size > 0 {
            b.size = Some(size);
        }
        drop(b);
        self.download.arrived.notify_all();
        Ok(())
    }

    fn set_seekable(&self, _: bool) -> Result<(), PlayerError> {
        Ok(())
    }

    fn set_download_buffering_enabled(&self, _: bool) -> Result<(), PlayerError> {
        Ok(())
    }

    fn set_playback_rate(&self, rate: f64) -> Result<(), PlayerError> {
        // Only normal speed for now.
        if rate == 1.0 { Ok(()) } else { Err(PlayerError::Backend("Only normal speed is supported.".into())) }
    }

    fn playback_rate(&self) -> f64 {
        1.0
    }

    fn push_data(&self, data: Vec<u8>) -> Result<(), PlayerError> {
        self.download.bytes.lock().unwrap().push(data);
        self.download.arrived.notify_all();
        Ok(())
    }

    fn end_of_stream(&self) -> Result<(), PlayerError> {
        self.download.bytes.lock().unwrap().complete = true;
        self.download.arrived.notify_all();
        Ok(())
    }

    fn buffered(&self) -> Vec<Range<f64>> {
        let Some(duration) = self.status.lock().unwrap().duration else { return Vec::new() };
        let b = self.download.bytes.lock().unwrap();
        let part = match (b.complete, b.size) {
            (true, _) => 1.0,
            (false, Some(size)) => (b.len as f64 / size.max(1) as f64).min(1.0),
            (false, None) => 0.0,
        };
        vec![0.0..duration * part]
    }

    fn set_stream(&self, _: &MediaStreamId, _: bool) -> Result<(), PlayerError> {
        Err(PlayerError::SetStreamFailed)
    }

    fn render_use_gl(&self) -> bool {
        false
    }

    fn set_audio_track(&self, _: i32, _: bool) -> Result<(), PlayerError> {
        Ok(())
    }

    fn set_video_track(&self, _: i32, _: bool) -> Result<(), PlayerError> {
        Ok(())
    }
}

/// A picture as Servo takes it.
/// What the live stream's reader follows of the element.
struct Live {
    playing: AtomicBool,
    volume: Mutex<f64>,
}

struct Bgra(Arc<Vec<u8>>);

impl Buffer for Bgra {
    fn to_vec(&self) -> Option<VideoFrameData> {
        Some(VideoFrameData::Raw(self.0.clone()))
    }
}

struct Driver {
    id: usize,
    events: GenericCallback<PlayerEvent>,
    renderer: Option<Arc<Mutex<dyn VideoFrameRenderer>>>,
    status: Arc<Mutex<Status>>,
    rx: mpsc::Receiver<Cmd>,
    /// Our own channel, for others to ask us to hold.
    me: mpsc::Sender<Cmd>,
    /// The page wants it playing.
    playing: bool,
    /// A seek waiting for its picture.
    seeking: Option<f64>,
    /// The end was reported.
    ended: bool,
    /// The position last reported, and when.
    told: Option<(f64, Instant)>,
    volume: f64,
    /// The picture's size in pixels (0 without one).
    pixels: u64,
    sound: bool,
    /// A picture has been shown since the decoder started.
    shown: bool,
    /// Paused (with the decoder still there) since.
    idle: Option<Instant>,
    /// It should have a decoder, and is waiting for room for one.
    want: bool,
    /// Waiting only for room others leave, not taking it from them: only
    /// playing or seeking takes it (a first picture, or one held while
    /// playing, waits).
    patient: bool,
    /// The decoder let go, still finishing.
    leaving: Option<std::thread::JoinHandle<()>>,
}

impl Driver {
    fn send(&self, e: PlayerEvent) {
        let _ = self.events.send(e);
    }

    fn run(&mut self, download: &Arc<Download>) -> Result<(), String> {
        // Servo holds the bytes back until asked.
        self.send(PlayerEvent::NeedData);
        // Wait for enough of the file to tell what it is.
        let (head, size) = {
            let mut b = download.bytes.lock().unwrap();
            loop {
                if b.closed {
                    return Ok(());
                }
                let enough = b.size.is_some_and(|s| b.len >= s.min(START_BYTES as u64));
                if b.complete || enough {
                    break;
                }
                b = download.arrived.wait(b).unwrap();
            }
            let size = if b.complete { b.len } else { b.size.unwrap_or(0) };
            (b.data[..b.data.len().min(64)].to_vec(), size)
        };
        if size == 0 {
            return Err("The media file is empty.".into());
        }
        // A stream from the page (rtc/live.rs).
        if let Some(secret) = head.strip_prefix(crate::rtc::live::MAGIC.as_bytes()) {
            return self.live(String::from_utf8_lossy(secret).trim());
        }
        let ext = sniff(&head);
        let d = download.clone();
        let input = Input::new(ext, &format!("page media ({ext})"), move || Ok((Box::new(Reader { download: d.clone(), pos: 0, size }) as Box<dyn ReadSeek>, size)));
        let (info, picture) = match ext {
            "mp4" | "webm" => {
                let info = video::probe(&input)?;
                if !info.picture && !info.sound {
                    return Err("This media's format can't be played.".into());
                }
                let picture = info.picture;
                (info, picture)
            }
            _ => {
                let duration = tabula_preview::audio::probe(&input)?;
                (video::Info { duration: (duration > 0.0).then_some(duration), sound: true, ..Default::default() }, false)
            }
        };
        let duration = info.duration;
        tracing::debug!("media {}: {size} bytes of {ext:?}: {info:?}", self.id);
        self.status.lock().unwrap().duration = duration;
        self.pixels = if picture { info.width as u64 * info.height as u64 } else { 0 };
        self.sound = info.sound;
        let length = duration.unwrap_or(f64::MAX);
        let make = move |playing: bool| Video::start_with(&input, length, Options { playing, picture, sound: info.sound, bgra: true });
        self.send(PlayerEvent::MetadataUpdated(Metadata {
            duration: duration.map(Duration::from_secs_f64),
            width: info.width,
            height: info.height,
            format: ext.into(),
            is_seekable: true,
            video_tracks: if picture { vec!["video".into()] } else { Vec::new() },
            audio_tracks: if info.sound { vec!["audio".into()] } else { Vec::new() },
            is_live: false,
            title: None,
        }));
        // Ready to play (Servo moves the element to HAVE_ENOUGH_DATA).
        self.send(PlayerEvent::StateChanged(PlaybackState::Paused));
        self.play_loop(&make)
    }

    /// The decoder, made afresh (at `at`, playing or not) if it was let
    /// go and there's room for it; false while it waits for room.
    fn wake(&mut self, player: &mut Option<Video>, at: f64, make: &dyn Fn(bool) -> Video) -> bool {
        self.want = true;
        if self.pixels > 0 && (self.leaving.is_some() || !start_decoding(self.id, self.pixels, self.sound && self.volume > 0.0, !self.patient, &self.me)) {
            return player.is_some();
        }
        if player.is_none() {
            let v = make(self.playing);
            if at > 0.0 {
                v.seek(at);
            }
            v.set_volume(self.volume);
            *player = Some(v);
            self.shown = false;
        }
        true
    }

    /// Let the decoder go (keeping the picture on show) until the page
    /// plays or seeks it again, or (if it's playing) there's room again.
    fn let_go(&mut self, player: &mut Option<Video>, held: &mut f64) {
        (self.want, self.patient) = (self.playing, true);
        self.idle = None;
        if let Some(v) = player.take() {
            *held = v.position();
            self.leaving = v.close();
        }
        if self.leaving.is_none() {
            stop_decoding(self.id);
        }
    }

    fn play_loop(&mut self, make: &dyn Fn(bool) -> Video) -> Result<(), String> {
        let mut wait = Duration::from_millis(15);
        // A first position even when paused: it fires the `timeupdate`
        // Servo's controls wait for before showing the duration, and
        // Servo fires none in an element's first 250 ms.
        let first = Instant::now() + Duration::from_millis(300);
        // The decoder, counted against the page's budget from the start (a
        // page may make twenty at once before playing any).
        let mut player = None;
        self.wake(&mut player, 0.0, make);
        // Where a let-go decoder stopped.
        let mut held = 0.0;
        self.idle = Some(Instant::now());
        loop {
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => self.command(&mut player, &mut held, make, cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
            while let Ok(cmd) = self.rx.try_recv() {
                self.command(&mut player, &mut held, make, cmd);
            }
            // Paused a while with its picture up: let the decoder go.
            if !self.playing && self.shown && self.seeking.is_none() && self.idle.is_some_and(|t| t.elapsed() >= IDLE_RELEASE) && player.is_some() {
                self.let_go(&mut player, &mut held);
            }
            // A decoder let go has finished: its memory is free (give it
            // back to the system) and no longer counted.
            if self.leaving.as_ref().is_some_and(|w| w.is_finished()) {
                self.leaving = None;
                stop_decoding(self.id);
                // SAFETY: malloc_trim only returns free memory to the system.
                unsafe { libc::malloc_trim(0) };
            }
            if player.is_none() && self.want {
                self.wake(&mut player, held, make);
            }
            let Some(p) = player.as_ref() else {
                // Waiting for room, or for the old decoder to finish;
                // else holding, with nothing to do until the page asks.
                wait = match (self.want, self.patient) {
                    _ if self.leaving.is_some() => RETRY,
                    (true, false) => RETRY,
                    (true, true) => PATIENT_RETRY,
                    (false, _) => Duration::from_secs(3600),
                };
                continue;
            };
            let (picture, next) = p.tick_picture();
            if let Some(picture) = picture {
                self.render(picture);
                self.shown = true;
                if let Some(t) = self.seeking.take() {
                    self.send(PlayerEvent::SeekDone(t));
                    self.told = None;
                }
            }
            if let Some(e) = p.error() {
                return Err(e);
            }
            let player = p;
            let pos = player.position();
            let due = if self.playing { self.told.is_none_or(|(p, at)| p != pos && at.elapsed() >= POSITION_EVERY) } else { self.told.is_none() && Instant::now() >= first };
            if due {
                self.send(PlayerEvent::PositionChanged(pos));
                self.told = Some((pos, Instant::now()));
            }
            if player.finished() && !self.ended {
                self.ended = true;
                self.send(PlayerEvent::PositionChanged(pos));
                self.send(PlayerEvent::EndOfStream);
            }
            // Paused with nothing due: sleep until the page asks for
            // something (or it's time to let the decoder go).
            wait = match next {
                Some(n) => n.min(POSITION_EVERY),
                None if self.told.is_none() => first.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)),
                None if self.idle.is_some() && self.shown => IDLE_RELEASE.saturating_sub(self.idle.map_or(Duration::ZERO, |t| t.elapsed())).max(Duration::from_millis(1)),
                None => Duration::from_secs(3600),
            };
        }
    }

    /// Play a live stream: pictures and sound as they come, no seeking,
    /// and an endless duration.
    fn live(&mut self, secret: &str) -> Result<(), String> {
        let mut reader = crate::rtc::live::Reader::connect(secret).map_err(|e| e.to_string())?;
        let closer = reader.closer().map_err(|e| e.to_string())?;
        let shared = Arc::new(Live { playing: AtomicBool::new(false), volume: Mutex::new(1.0) });
        let (sh, events, renderer, status, id) = (shared.clone(), self.events.clone(), self.renderer.clone(), self.status.clone(), self.id);
        let started = Instant::now();
        std::thread::Builder::new()
            .name("live-media".into())
            .spawn(move || {
                let send = |e| {
                    let _ = events.send(e);
                };
                let mut ready = false;
                let mut size = None;
                let mut kinds = (false, false);
                let mut sink: Option<(u32, u8, tabula_preview::audio::LiveSink)> = None;
                let mut bgra = Vec::new();
                let mut told = Instant::now();
                let announce = |size: Option<(u32, u32)>, kinds: (bool, bool)| {
                    send(PlayerEvent::MetadataUpdated(Metadata {
                        duration: None,
                        width: size.map_or(0, |s| s.0),
                        height: size.map_or(0, |s| s.1),
                        format: "live".into(),
                        is_seekable: false,
                        video_tracks: if kinds.1 { vec!["video".into()] } else { Vec::new() },
                        audio_tracks: if kinds.0 { vec!["audio".into()] } else { Vec::new() },
                        is_live: true,
                        title: None,
                    }));
                };
                loop {
                    let message = match reader.next() {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::debug!("media {id}: live stream ended: {e}");
                            break;
                        }
                    };
                    let playing = sh.playing.load(Ordering::Relaxed);
                    match message {
                        crate::rtc::live::Message::Kinds { audio, video } => {
                            kinds = (audio, video);
                            // Sound alone is ready at once; pictures once one is here.
                            if !ready && !video {
                                ready = true;
                                announce(None, kinds);
                                send(PlayerEvent::StateChanged(PlaybackState::Paused));
                            }
                        }
                        crate::rtc::live::Message::Picture(p) => {
                            if size != Some((p.width, p.height)) {
                                size = Some((p.width, p.height));
                                announce(size, kinds);
                            }
                            if !ready || playing {
                                if let Some(r) = &renderer {
                                    p.to_bgra(&mut bgra);
                                    if let Some(frame) = VideoFrame::new(p.width as i32, p.height as i32, Arc::new(Bgra(Arc::new(std::mem::take(&mut bgra))))) {
                                        r.lock().unwrap().render(frame);
                                        send(PlayerEvent::VideoFrameUpdated);
                                    }
                                }
                            }
                            if !ready {
                                ready = true;
                                send(PlayerEvent::StateChanged(PlaybackState::Paused));
                            }
                        }
                        crate::rtc::live::Message::Sound(s) => {
                            let muted = status.lock().unwrap().muted;
                            if !playing || muted {
                                continue;
                            }
                            if sink.as_ref().is_none_or(|(r, c, _)| *r != s.rate || *c != s.channels) {
                                sink = tabula_preview::audio::LiveSink::start(s.rate, s.channels).ok().map(|k| (s.rate, s.channels, k));
                            }
                            if let Some((_, _, k)) = &sink {
                                k.set_volume(*sh.volume.lock().unwrap());
                                k.push(&s.samples);
                            }
                        }
                    }
                    if playing && told.elapsed() >= POSITION_EVERY {
                        told = Instant::now();
                        send(PlayerEvent::PositionChanged(started.elapsed().as_secs_f64()));
                    }
                }
                send(PlayerEvent::EndOfStream);
            })
            .map_err(|e| e.to_string())?;
        // Commands until the element lets go of us.
        loop {
            match self.rx.recv() {
                Ok(Cmd::Play) => {
                    shared.playing.store(true, Ordering::Relaxed);
                    self.send(PlayerEvent::StateChanged(PlaybackState::Playing));
                }
                Ok(Cmd::Pause) => {
                    shared.playing.store(false, Ordering::Relaxed);
                    self.send(PlayerEvent::StateChanged(PlaybackState::Paused));
                }
                Ok(Cmd::Seek(t)) => self.send(PlayerEvent::SeekDone(t)),
                Ok(Cmd::Volume(v)) => *shared.volume.lock().unwrap() = v,
                // Live streams aren't counted against the decoding budget.
                Ok(Cmd::Hold) => {}
                Err(_) => break,
            }
        }
        let _ = closer.shutdown(std::net::Shutdown::Both);
        Ok(())
    }

    fn command(&mut self, slot: &mut Option<Video>, held: &mut f64, make: &dyn Fn(bool) -> Video, cmd: Cmd) {
        match cmd {
            Cmd::Play => {
                self.playing = true;
                self.idle = None;
                self.patient = false;
                if !self.wake(slot, *held, make) {
                    // It starts playing once there's room for it.
                    self.send(PlayerEvent::StateChanged(PlaybackState::Playing));
                    return;
                }
                let player = slot.as_ref().unwrap();
                if player.finished() {
                    player.seek(0.0);
                }
                self.ended = false;
                player.set_playing(true);
                self.send(PlayerEvent::StateChanged(PlaybackState::Playing));
            }
            Cmd::Pause => {
                self.playing = false;
                self.patient = true;
                // Held, with its picture up: nothing to wait for now.
                if slot.is_none() && self.shown {
                    self.want = false;
                }
                if let Some(player) = slot.as_ref() {
                    player.set_playing(false);
                    self.idle = Some(Instant::now());
                }
                self.send(PlayerEvent::StateChanged(PlaybackState::Paused));
            }
            Cmd::Seek(t) => {
                self.ended = false;
                self.patient = false;
                if !self.wake(slot, t, make) {
                    *held = t;
                    self.send(PlayerEvent::SeekDone(t));
                    return;
                }
                let player = slot.as_ref().unwrap();
                player.seek(t);
                // Looping seeks back from the end: carry on playing.
                if self.playing && !player.playing() {
                    player.set_playing(true);
                }
                if !self.playing {
                    self.idle = Some(Instant::now());
                }
                if self.renderer.is_some() && player.has_picture() {
                    self.seeking = Some(t);
                } else {
                    self.send(PlayerEvent::SeekDone(t));
                }
            }
            Cmd::Volume(v) => {
                self.volume = v;
                if let Some(player) = slot.as_ref() {
                    player.set_volume(v);
                }
            }
            Cmd::Hold => self.let_go(slot, held),
        }
    }

    fn render(&self, picture: Picture) {
        let Some(r) = &self.renderer else { return };
        // The worker made it BGRA, as Servo takes it: no copy here.
        let (size, data) = match picture {
            Picture::Bgra { size, data } => (size, data),
            rgba => {
                let image = rgba.into_image();
                (image.size, Arc::new(image.pixels.iter().flat_map(|p| [p.b(), p.g(), p.r(), p.a()]).collect()))
            }
        };
        let [w, h] = size;
        if let Some(frame) = VideoFrame::new(w as i32, h as i32, Arc::new(Bgra(data))) {
            r.lock().unwrap().render(frame);
            self.send(PlayerEvent::VideoFrameUpdated);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_can_play_type() {
        let p = |t| can_play(t) == SupportsMediaType::Probably;
        let maybe = |t| can_play(t) == SupportsMediaType::Maybe;
        let no = |t| can_play(t) == SupportsMediaType::No;
        assert!(p(r#"video/mp4; codecs="avc1.42E01E, mp4a.40.2""#));
        assert!(p(r#"video/webm; codecs="av01.0.05M.08, opus""#));
        assert!(maybe("audio/mpeg"));
        assert!(maybe("VIDEO/MP4"));
        assert!(no(r#"video/webm; codecs="vp9""#));
        assert!(no("video/ogg"));
        assert!(no("application/x-mpegurl"));
    }

    #[test]
    fn sniffs_containers() {
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), "mp4");
        assert_eq!(sniff(&[0x1a, 0x45, 0xdf, 0xa3, 1]), "webm");
        assert_eq!(sniff(b"ID3\x04"), "mp3");
        assert_eq!(sniff(&[0xff, 0xfb, 0x90]), "mp3");
        assert_eq!(sniff(b"OggS"), "ogg");
        assert_eq!(sniff(b"<html>"), "");
    }

    #[test]
    fn big_downloads_go_to_disk() {
        let d = Arc::new(Download::default());
        let all: Vec<u8> = (0..3 * IN_MEMORY + 12345).map(|i| (i * 7 % 251) as u8).collect();
        for chunk in all.chunks(100_000) {
            d.bytes.lock().unwrap().push(chunk.to_vec());
        }
        {
            let b = d.bytes.lock().unwrap();
            assert_eq!(b.len, all.len() as u64);
            if b.rest.is_some() {
                assert_eq!(b.data.len(), IN_MEMORY);
            }
        }
        d.bytes.lock().unwrap().complete = true;
        let mut r = Reader { download: d.clone(), pos: 0, size: all.len() as u64 };
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert!(back == all);
        r.seek(SeekFrom::Start(IN_MEMORY as u64 - 3)).unwrap();
        let mut six = [0; 6];
        r.read_exact(&mut six).unwrap();
        assert_eq!(six, all[IN_MEMORY - 3..IN_MEMORY + 3]);
    }

    #[test]
    fn reader_waits_for_bytes() {
        let d = Arc::new(Download::default());
        d.bytes.lock().unwrap().push(b"abc".to_vec());
        let mut r = Reader { download: d.clone(), pos: 0, size: 6 };
        let feeder = {
            let d = d.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                d.bytes.lock().unwrap().push(b"def".to_vec());
                d.arrived.notify_all();
            })
        };
        // Past what has arrived: waits for the rest.
        r.seek(SeekFrom::End(-2)).unwrap();
        let mut buf = [0; 2];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ef");
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        feeder.join().unwrap();
        // Closing wakes a waiting reader with an error.
        let d = Arc::new(Download::default());
        let mut r = Reader { download: d.clone(), pos: 0, size: 10 };
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            d.close();
        });
        assert!(r.read(&mut buf).is_err());
        closer.join().unwrap();
    }
}
