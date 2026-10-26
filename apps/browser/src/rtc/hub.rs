//! Where live media meets: sources (a camera, a microphone, the far end
//! of a call) publish frames; tracks are a page's handles on a source
//! (several can share one, and each can be switched off); streams are a
//! page's groups of tracks, which media elements play. Subscribers (an
//! element's connection, a call sending the track) get the frames of a
//! track while it's on: a switched-off track gives black pictures once a
//! second and no sound.

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

use servo::WebViewId;

use super::frames::{Frame, Picture};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Audio,
    Video,
}

pub type SourceId = u64;
pub type SubId = u64;

/// A track, by page and the page's name for it.
pub type TrackKey = (WebViewId, String);

struct Source {
    kind: Kind,
    /// Set to stop whatever feeds it (once no track uses it).
    stop: Arc<AtomicBool>,
    /// Tracks on it.
    users: usize,
    /// The newest picture, for a subscriber that arrives between frames.
    last: Option<Arc<Picture>>,
    /// Ended by its feeder (a camera unplugged, a call's track gone).
    ended: bool,
}

struct Track {
    source: SourceId,
    kind: Kind,
    enabled: bool,
    /// When a switched-off video track last sent black.
    black_at: Option<Instant>,
}

struct Sub {
    track: TrackKey,
    tx: SyncSender<(Kind, Frame)>,
}

/// A page's stream: its tracks, and a count that moves when they change
/// (connections watch it).
struct Stream {
    view: WebViewId,
    tracks: Vec<String>,
    version: u64,
}

#[derive(Default)]
struct Inner {
    sources: HashMap<SourceId, Source>,
    tracks: HashMap<TrackKey, Track>,
    streams: HashMap<String, Stream>,
    subs: HashMap<SubId, Sub>,
    next: u64,
    /// Called (outside the lock) when a source ends with tracks on it.
    on_end: Option<Arc<dyn Fn(TrackKey) + Send + Sync>>,
}

pub struct Hub(Mutex<Inner>);

pub static HUB: LazyLock<Hub> = LazyLock::new(|| Hub(Mutex::new(Inner::default())));

impl Hub {
    /// Tell `f` about each track whose source ended by itself.
    pub fn on_end(&self, f: impl Fn(TrackKey) + Send + Sync + 'static) {
        self.0.lock().unwrap().on_end = Some(Arc::new(f));
    }

    /// A new source; its feeder stops when `stop` is set.
    pub fn add_source(&self, kind: Kind, stop: Arc<AtomicBool>) -> SourceId {
        let mut h = self.0.lock().unwrap();
        h.next += 1;
        let id = h.next;
        h.sources.insert(id, Source { kind, stop, users: 0, last: None, ended: false });
        id
    }

    /// The feeder is done (the device went away).
    pub fn end_source(&self, id: SourceId) {
        let (ended, f) = {
            let mut h = self.0.lock().unwrap();
            let Some(s) = h.sources.get_mut(&id) else { return };
            s.ended = true;
            let ended: Vec<TrackKey> = h.tracks.iter().filter(|(_, t)| t.source == id).map(|(k, _)| k.clone()).collect();
            (ended, h.on_end.clone())
        };
        if let Some(f) = f {
            ended.into_iter().for_each(|k| f(k));
        }
    }

    /// Put a page's track `name` on `source`.
    pub fn add_track(&self, key: TrackKey, source: SourceId) -> bool {
        let mut h = self.0.lock().unwrap();
        let Some(s) = h.sources.get_mut(&source) else { return false };
        s.users += 1;
        let kind = s.kind;
        if let Some(old) = h.tracks.insert(key, Track { source, kind, enabled: true, black_at: None }) {
            release(&mut h, old.source);
        }
        true
    }

    /// A second track on the first one's source (`clone()`).
    pub fn clone_track(&self, from: &TrackKey, to: TrackKey) -> bool {
        let source = self.0.lock().unwrap().tracks.get(from).map(|t| t.source);
        source.is_some_and(|s| self.add_track(to, s))
    }

    pub fn set_enabled(&self, key: &TrackKey, on: bool) {
        if let Some(t) = self.0.lock().unwrap().tracks.get_mut(key) {
            t.enabled = on;
            t.black_at = None;
        }
    }

    /// The page stopped the track (or went away): its source stops when
    /// no track uses it any more.
    pub fn remove_track(&self, key: &TrackKey) {
        let mut h = self.0.lock().unwrap();
        if let Some(t) = h.tracks.remove(key) {
            release(&mut h, t.source);
            for s in h.streams.values_mut().filter(|s| s.view == key.0 && s.tracks.contains(&key.1)) {
                s.version += 1;
            }
        }
    }

    /// Everything of a page that's gone or moved on.
    pub fn remove_view(&self, view: WebViewId) {
        let keys: Vec<TrackKey> = self.0.lock().unwrap().tracks.keys().filter(|k| k.0 == view).cloned().collect();
        keys.iter().for_each(|k| self.remove_track(k));
        self.0.lock().unwrap().streams.retain(|_, s| s.view != view);
    }

    /// The page's stream `secret` now holds `tracks`.
    pub fn set_stream(&self, view: WebViewId, secret: String, tracks: Vec<String>) {
        let mut h = self.0.lock().unwrap();
        let s = h.streams.entry(secret).or_insert(Stream { view, tracks: Vec::new(), version: 0 });
        if s.view == view {
            s.tracks = tracks;
            s.version += 1;
        }
    }

    /// The stream's first live audio and video tracks, and its version
    /// (None once the page dropped it).
    pub fn stream_tracks(&self, secret: &str) -> Option<(u64, Option<TrackKey>, Option<TrackKey>)> {
        let h = self.0.lock().unwrap();
        let s = h.streams.get(secret)?;
        let first = |kind| {
            s.tracks.iter().map(|t| (s.view, t.clone())).find(|k| h.tracks.get(k).is_some_and(|t| t.kind == kind && h.sources.get(&t.source).is_some_and(|s| !s.ended)))
        };
        let (a, v) = (first(Kind::Audio), first(Kind::Video));
        Some((s.version, a, v))
    }

    pub fn subscribe(&self, track: TrackKey, tx: SyncSender<(Kind, Frame)>) -> SubId {
        let mut h = self.0.lock().unwrap();
        h.next += 1;
        let id = h.next;
        let last = h.tracks.get(&track).and_then(|t| h.sources.get(&t.source)).and_then(|s| s.last.clone());
        if let Some(p) = last {
            let _ = tx.try_send((Kind::Video, Frame::Picture(p)));
        }
        h.subs.insert(id, Sub { track, tx });
        id
    }

    pub fn unsubscribe(&self, id: SubId) {
        self.0.lock().unwrap().subs.remove(&id);
    }

    /// A source's new frame, to everyone on a track of it.
    pub fn publish(&self, source: SourceId, frame: Frame) {
        let mut guard = self.0.lock().unwrap();
        let h = &mut *guard;
        let Some(s) = h.sources.get_mut(&source) else { return };
        let kind = s.kind;
        if let Frame::Picture(p) = &frame {
            s.last = Some(p.clone());
        }
        let mut gone = Vec::new();
        let mut black: Option<Frame> = None;
        for (&id, sub) in &h.subs {
            let Some(t) = h.tracks.get(&sub.track) else { continue };
            if t.source != source {
                continue;
            }
            let frame = if t.enabled {
                frame.clone()
            } else {
                match &frame {
                    Frame::Picture(p) if t.black_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) => black
                        .get_or_insert_with(|| Frame::Picture(Arc::new(Picture::black(p.width, p.height))))
                        .clone(),
                    _ => continue,
                }
            };
            match sub.tx.try_send((kind, frame)) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => gone.push(id),
            }
        }
        if black.is_some() {
            let now = Instant::now();
            for sub in h.subs.values() {
                if let Some(t) = h.tracks.get_mut(&sub.track)
                    && t.source == source
                    && !t.enabled
                {
                    t.black_at = Some(now);
                }
            }
        }
        for id in gone {
            h.subs.remove(&id);
        }
    }
}

fn release(h: &mut Inner, source: SourceId) {
    if let Some(s) = h.sources.get_mut(&source) {
        s.users = s.users.saturating_sub(1);
        if s.users == 0 {
            s.stop.store(true, Ordering::Relaxed);
            h.sources.remove(&source);
        }
    }
}
