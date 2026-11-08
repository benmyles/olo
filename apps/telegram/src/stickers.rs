//! Animated stickers played (Lottie: `olo_images::lottie`). The whole
//! file is fetched when the sticker is first drawn, read once on the
//! worker, and drawn there a frame at a time at the very size the bubble
//! shows it (`snap_centred`'s device pixels: one to one, nothing
//! resampled). Each frame is drawn into the conversation as it comes, and
//! the next is asked for when it's due, by a timer: nothing is polled,
//! and the conversation is drawn once a frame, no more.
//!
//! A sticker plays once as it comes into view, and again for as long as
//! the pointer is over it; then it rests on its first frame (drawn at its
//! size too, sharper than Telegram's thumbnail). Out of view it stops and
//! lets its frames go (and plays again when it's back); frames replaced
//! go from the window's atlas when the conversation next paints.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use olo_images::lottie::{self, Lottie, Painter};
use olo_tg::{ChatId, MsgId};
use olo_ui::{Proxy, gpui::RenderImage};

pub type Key = (ChatId, MsgId);

/// From the worker.
pub enum Msg {
    Read(Key, Result<Arc<Lottie>, String>),
    Frame { key: Key, generation: u64, frame: f64, size: (u16, u16), image: Arc<RenderImage> },
}

enum Job {
    Read(Key, PathBuf),
    Paint { key: Key, generation: u64, lottie: Arc<Lottie>, frame: f64, size: (u16, u16) },
}

#[derive(Default)]
struct Sticker {
    lottie: Option<Arc<Lottie>>,
    reading: bool,
    failed: bool,
    /// Playing since (None: resting on its first frame).
    since: Option<Instant>,
    /// Come into view before it was read: it plays once it is.
    to_play: bool,
    hovered: bool,
    /// The device size it's shown at.
    size: (u16, u16),
    /// The frame shown: its number, size, picture.
    shown: Option<(f64, (u16, u16), Arc<RenderImage>)>,
    /// A frame asked of the worker and not come yet.
    asked: Option<(f64, (u16, u16))>,
    /// Frames asked before it last stopped are no use.
    generation: u64,
}

pub struct Stickers {
    stickers: HashMap<Key, Sticker>,
    /// Drawn in the conversation's last paint.
    visible: HashSet<Key>,
    work: mpsc::Sender<Job>,
    /// Pictures replaced, to drop from the window's atlas.
    pub dropped: Vec<Arc<RenderImage>>,
    /// Playing stickers whose frame due was already shown (as a play
    /// begins on the first frame it rests on): when to ask again. The
    /// caller sets the timers (`take_due`).
    due: Vec<(Key, Duration)>,
}

/// Animations kept read (not drawn) for chats left.
const KEPT: usize = 32;

impl Stickers {
    pub fn new(proxy: Proxy) -> Stickers {
        let (work, jobs) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new().name("telegram-stickers".into()).spawn(move || {
            // A painter for the size last asked (a sticker's, at the
            // window's scale): its buffers kept.
            let mut painter: Option<Painter> = None;
            while let Ok(job) = jobs.recv() {
                match job {
                    Job::Read(key, path) => proxy.send(Msg::Read(key, Lottie::open(&path).map(Arc::new))),
                    Job::Paint { key, generation, lottie, frame, size } => {
                        let p = match &mut painter {
                            Some(p) if p.size() == size => p,
                            p => p.insert(Painter::new(size.0, size.1)),
                        };
                        let Ok(px) = p.paint(&lottie, frame) else { continue };
                        let mut bgra = px.to_vec();
                        lottie::to_straight_bgra(&mut bgra);
                        let Some(buf) = image::RgbaImage::from_raw(u32::from(size.0), u32::from(size.1), bgra) else { continue };
                        let image = Arc::new(RenderImage::new([image::Frame::new(buf)]));
                        proxy.send(Msg::Frame { key, generation, frame, size, image });
                    }
                }
            }
        });
        Stickers { stickers: HashMap::new(), visible: HashSet::new(), work, dropped: Vec::new(), due: Vec::new() }
    }

    /// Whether `key` is an animated sticker being shown (for the pointer
    /// coming over it or leaving).
    pub fn is_shown(&self, key: Key) -> bool {
        self.visible.contains(&key)
    }

    /// An animated sticker drawn now, `size` device pixels, its whole
    /// file at `file` once it's here, the pointer over it or not: the
    /// picture to draw (None: its thumbnail, till a frame comes). Asks
    /// for the frame due.
    pub fn shown(&mut self, key: Key, file: Option<&std::path::Path>, size: (u16, u16), hovered: bool, now: Instant) -> Option<Arc<RenderImage>> {
        let new_in_view = self.visible.insert(key);
        let s = self.stickers.entry(key).or_default();
        s.size = size;
        s.to_play |= new_in_view;
        if s.lottie.is_none() {
            if let Some(path) = file
                && !s.reading
                && !s.failed
            {
                s.reading = true;
                let _ = self.work.send(Job::Read(key, path.to_owned()));
            }
            return None;
        }
        // Into view, or the pointer onto it: it plays.
        if (std::mem::take(&mut s.to_play) || hovered && !s.hovered) && s.since.is_none() {
            s.since = Some(now);
            s.generation += 1;
        }
        s.hovered = hovered;
        let image = s.shown.as_ref().map(|f| f.2.clone());
        self.ask(key, now);
        image
    }

    /// The conversation painted: those not drawn this time are out of view.
    pub fn painted(&mut self, drawn: &HashSet<Key>) {
        let gone: Vec<Key> = self.visible.iter().filter(|k| !drawn.contains(k)).copied().collect();
        for key in gone {
            self.visible.remove(&key);
            if let Some(s) = self.stickers.get_mut(&key) {
                s.since = None;
                s.to_play = false;
                s.hovered = false;
                s.asked = None;
                s.generation += 1;
                if let Some((_, _, img)) = s.shown.take() {
                    self.dropped.push(img);
                }
            }
        }
        // Read ones not in view: a few kept, for coming back.
        if self.stickers.len() > KEPT {
            let visible = &self.visible;
            let mut idle: Vec<Key> = self.stickers.keys().filter(|k| !visible.contains(k)).copied().collect();
            idle.truncate(self.stickers.len() - KEPT);
            for k in idle {
                self.stickers.remove(&k);
            }
        }
    }

    /// The frame due now for `key`, asked of the worker unless it's shown
    /// or asked already. Past its one time through (and not hovered), it
    /// stops, and its first frame is asked for to rest on.
    pub fn ask(&mut self, key: Key, now: Instant) {
        if !self.visible.contains(&key) {
            return;
        }
        let Some(s) = self.stickers.get_mut(&key) else { return };
        let Some(lottie) = s.lottie.clone() else { return };
        let mut frame = lottie.frames().start;
        if let Some(since) = s.since {
            let t = now.saturating_duration_since(since);
            if t >= lottie.duration() && !s.hovered {
                s.since = None;
                s.generation += 1;
            } else {
                frame = lottie.frame_at(t);
            }
        }
        let want = (frame, s.size);
        if s.asked.is_some() {
            return;
        }
        if s.shown.as_ref().is_some_and(|f| (f.0, f.1) == want) {
            // Shown already: playing, the next one when it's due.
            if let Some(wait) = next_frame(s) {
                self.due.push((key, wait));
            }
            return;
        }
        s.asked = Some(want);
        let _ = self.work.send(Job::Paint { key, generation: s.generation, lottie, frame, size: s.size });
    }

    /// Timers to set: after each, [`Stickers::ask`] for that sticker.
    pub fn take_due(&mut self) -> Vec<(Key, Duration)> {
        std::mem::take(&mut self.due)
    }

    /// From the worker. True if the conversation should be drawn again;
    /// with when the next frame is due, if it's playing.
    pub fn message(&mut self, msg: Msg) -> (bool, Option<(Key, Duration)>) {
        match msg {
            Msg::Read(key, r) => {
                let Some(s) = self.stickers.get_mut(&key) else { return (false, None) };
                s.reading = false;
                match r {
                    Ok(l) => s.lottie = Some(l),
                    Err(e) => {
                        tracing::debug!("telegram: an animated sticker couldn’t be read: {e}");
                        s.failed = true;
                    }
                }
                (true, None)
            }
            Msg::Frame { key, generation, frame, size, image } => {
                let Some(s) = self.stickers.get_mut(&key).filter(|_| self.visible.contains(&key)) else {
                    self.dropped.push(image);
                    return (false, None);
                };
                s.asked = None;
                if generation != s.generation {
                    // Asked before it began again (or stopped): what's due
                    // now is asked instead.
                    self.dropped.push(image);
                    self.ask(key, Instant::now());
                    return (false, None);
                }
                if let Some((_, _, old)) = s.shown.replace((frame, size, image)) {
                    self.dropped.push(old);
                }
                // Playing: the next frame at the next frame's time.
                (true, next_frame(s).map(|wait| (key, wait)))
            }
        }
    }
}

/// Playing: how long till its next frame's time.
fn next_frame(s: &Sticker) -> Option<Duration> {
    let (since, l) = (s.since?, s.lottie.as_ref()?);
    let elapsed = since.elapsed().as_secs_f64() * l.frame_rate();
    Some(Duration::from_secs_f64((elapsed.floor() + 1.0 - elapsed) / l.frame_rate()))
}
