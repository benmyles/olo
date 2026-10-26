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
        atomic::{AtomicUsize, Ordering},
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
    video::{self, Options, Video},
};

/// Bytes to have before looking at the file, unless it's all there.
const START_BYTES: usize = 64 * 1024;

/// How often the page hears the position while playing.
const POSITION_EVERY: Duration = Duration::from_millis(250);

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
  function touch(e) { try { void e.textTracks; } catch (x) {} }
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
  var play = P.play;
  P.play = function() {
    touch(this);
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
        can_play(media_type)
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

/// The file as Servo has fetched it so far.
#[derive(Default)]
struct Bytes {
    data: Vec<u8>,
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
            let have = b.data.len() as u64;
            if self.pos < have {
                let n = buf.len().min((have - self.pos) as usize);
                buf[..n].copy_from_slice(&b.data[self.pos as usize..self.pos as usize + n]);
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
        let (d, s) = (download.clone(), status.clone());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                let mut driver = Driver { id, events, renderer, status: s, rx, playing: false, seeking: None, ended: false, told: None };
                if let Err(e) = driver.run(&d) {
                    tracing::debug!("media {id}: {e}");
                    let _ = driver.events.send(PlayerEvent::Error(e));
                }
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
        let mut b = self.download.bytes.lock().unwrap();
        if b.data.is_empty() {
            b.data = data;
        } else {
            b.data.extend_from_slice(&data);
        }
        drop(b);
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
            (false, Some(size)) => (b.data.len() as f64 / size.max(1) as f64).min(1.0),
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
    /// The page wants it playing.
    playing: bool,
    /// A seek waiting for its picture.
    seeking: Option<f64>,
    /// The end was reported.
    ended: bool,
    /// The position last reported, and when.
    told: Option<(f64, Instant)>,
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
                let enough = b.size.is_some_and(|s| b.data.len() as u64 >= s.min(START_BYTES as u64));
                if b.complete || enough {
                    break;
                }
                b = download.arrived.wait(b).unwrap();
            }
            let size = if b.complete { b.data.len() as u64 } else { b.size.unwrap_or(0) };
            (b.data[..b.data.len().min(16)].to_vec(), size)
        };
        if size == 0 {
            return Err("The media file is empty.".into());
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
        let player = Video::start_with(&input, duration.unwrap_or(f64::MAX), Options { playing: false, picture, sound: info.sound });
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
        self.play_loop(&player)
    }

    fn play_loop(&mut self, player: &Video) -> Result<(), String> {
        let mut wait = Duration::from_millis(15);
        // A first position even when paused: it fires the `timeupdate`
        // Servo's controls wait for before showing the duration, and
        // Servo fires none in an element's first 250 ms.
        let first = Instant::now() + Duration::from_millis(300);
        loop {
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => self.command(player, cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
            while let Ok(cmd) = self.rx.try_recv() {
                self.command(player, cmd);
            }
            let (picture, next) = player.tick();
            if let Some(image) = picture {
                self.render(&image);
                if let Some(t) = self.seeking.take() {
                    self.send(PlayerEvent::SeekDone(t));
                    self.told = None;
                }
            }
            if let Some(e) = player.error() {
                return Err(e);
            }
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
            // Paused with nothing due: sleep until the page asks for something.
            wait = match next {
                Some(n) => n.min(POSITION_EVERY),
                None if self.told.is_none() => first.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)),
                None => Duration::from_secs(3600),
            };
        }
    }

    fn command(&mut self, player: &Video, cmd: Cmd) {
        match cmd {
            Cmd::Play => {
                if player.finished() {
                    player.seek(0.0);
                }
                self.ended = false;
                self.playing = true;
                player.set_playing(true);
                self.send(PlayerEvent::StateChanged(PlaybackState::Playing));
            }
            Cmd::Pause => {
                self.playing = false;
                player.set_playing(false);
                self.send(PlayerEvent::StateChanged(PlaybackState::Paused));
            }
            Cmd::Seek(t) => {
                self.ended = false;
                player.seek(t);
                // Looping seeks back from the end: carry on playing.
                if self.playing && !player.playing() {
                    player.set_playing(true);
                }
                if self.renderer.is_some() && player.has_picture() {
                    self.seeking = Some(t);
                } else {
                    self.send(PlayerEvent::SeekDone(t));
                }
            }
            Cmd::Volume(v) => player.set_volume(v),
        }
    }

    fn render(&self, image: &tabula_kit::egui::ColorImage) {
        let Some(r) = &self.renderer else { return };
        let mut bgra = Vec::with_capacity(image.pixels.len() * 4);
        for p in &image.pixels {
            bgra.extend_from_slice(&[p.b(), p.g(), p.r(), p.a()]);
        }
        let [w, h] = image.size;
        if let Some(frame) = VideoFrame::new(w as i32, h as i32, Arc::new(Bgra(Arc::new(bgra)))) {
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
    fn reader_waits_for_bytes() {
        let d = Arc::new(Download::default());
        d.bytes.lock().unwrap().data = b"abc".to_vec();
        let mut r = Reader { download: d.clone(), pos: 0, size: 6 };
        let feeder = {
            let d = d.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                d.bytes.lock().unwrap().data.extend_from_slice(b"def");
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
