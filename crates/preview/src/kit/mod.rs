//! The previews as olo-kit views (the Viewer's and Quick Look's): what
//! [`load`](crate::load) decodes, on its worker, told to the view as it
//! comes. Pictures are drawn exactly (the kit's `ImageView`: resampled to their device size, a texel a
//! device pixel); text and code by the rows in view; documents as a page
//! ([`rich`]); sheets with their search and sorting ([`table`]); sound
//! and video with a transport; fonts with samples set in them.

mod rich;
mod table;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, ParentElement as _, PathBuilder, Pixels,
    Point, RenderImage, ScrollHandle, SharedString, Styled as _, TextRun, TimerId, View, WindowCx, agent, canvas, div, icons, point, px, size,
    style::{self, Align},
    theme,
    widgets::{Cell, Column, ColumnListView, ImageView, ListModel, ScrollView, TextField, TextFieldEvent, image_view, image_view::Rgba, text_field},
};
use olo_kit_base::{files, metrics as m};

use crate::{Bitmap, Body, Item, SLOW, Update, audio, input, load, media, video};

impl Rgba for Bitmap {
    fn size(&self) -> [usize; 2] {
        self.size
    }

    fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    fn gamut(&self) -> olo_kit::Gamut {
        if self.p3 { olo_kit::Gamut::DisplayP3 } else { olo_kit::Gamut::Srgb }
    }
}

/// A text row's height, and the room either side of the line numbers.
const TEXT_ROW: f32 = 16.0;
const GUTTER_PAD: f32 = 10.0;
/// Below a video: its title and facts, and the transport when it plays.
const VIDEO_INFO: f32 = 2.0 * m::LABEL_ROW + 2.0 * m::SPACING;
const TRANSPORT: f32 = 34.0;
/// Full screen, how long the transport shows after the pointer last moved.
const CONTROLS_FOR: std::time::Duration = std::time::Duration::from_millis(2500);

/// How a video's picture fills its place: at most its own size (a frame
/// pixel a device pixel, smaller to fit), as big as fits, or filling it
/// (what overflows cut off); its proportions kept every way. Full screen,
/// `Actual` is as big as fits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoZoom {
    #[default]
    Actual,
    Fit,
    Fill,
}
/// What's under a video's picture (its transport and what it is), for a
/// window's own controls to go over (the Viewer's trim bar).
pub const UNDER_VIDEO: f32 = VIDEO_INFO + TRANSPORT;
/// A sound's cover, a side at most.
const ART: f32 = 160.0;

/// Told to whoever shows the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewEvent {
    /// What it shows arrived, or it failed: the window may want another
    /// size (see [`PreviewView::ideal_size`]).
    Shown,
    /// What it shows changed gamut ([`PreviewView::gamut`]): a wide
    /// video's first picture came (or it stopped showing one).
    Gamut,
    /// What decodes its video became known, or changed
    /// ([`PreviewView::decoding`]): a menu saying it is drawn again.
    Decoding,
    /// Its video started playing, or stopped
    /// ([`PreviewView::playing_video`]).
    Playing,
    /// Its video is shown again: another picture of it, or it was moved
    /// to another moment ([`PreviewView::position`]). For what follows
    /// the video's time beside it (the Viewer's editor's playhead, which
    /// passes over what's cut).
    Played,
}

/// What the view is told: by its loader's worker, its timers and the
/// workers that make its pictures.
pub enum Msg {
    /// Something loaded, of the item of that generation.
    Loaded(u64, Update),
    /// Loading that item has taken a while: say so.
    Slow(u64),
    /// A video's poster, as a picture to draw.
    Poster(u64, Option<Arc<RenderImage>>),
    /// The cover or poster made for a device size.
    Art((u32, u32), Option<Arc<RenderImage>>),
}

/// A preview of a file, as Quick Look and the Viewer show one.
pub struct PreviewView {
    pub item: Item,
    pub generation: u64,
    body: Option<Body>,
    /// A PDF's first page is in the picture's view (`image`): it shows
    /// there, the card until then.
    first_page: bool,
    failed: Option<String>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    want: load::Want,
    image: Child<ImageView>,
    player: Option<audio::Player>,
    video: Option<video::Video>,
    video_zoom: VideoZoom,
    /// Full screen, until when the transport shows (the pointer moved),
    /// and the timer that hides it then.
    controls_until: Option<Instant>,
    controls_hide: Option<TimerId>,
    /// The video's frame showing, or its poster until one comes.
    frame: Option<Arc<RenderImage>>,
    /// The video's frame showing as decoded (Y'CbCr, drawn by the GPU:
    /// `Window::paint_yuv`), in `frame`'s place.
    yuv_frame: Option<Arc<olo_kit::YuvFrame>>,
    /// What decodes video here (`set_decode_with`), and what was said to
    /// decode it last (`PreviewEvent::Decoding`).
    decode_with: video::Device,
    decoding_said: Option<&'static str>,
    /// How fast sound and video play (Playback Speed), kept from file to
    /// file: Quick Look stepping through a folder of lectures keeps 1.5×.
    rate: f64,
    /// Its video is known to be HDR (told as `PreviewEvent::Gamut`: its
    /// window may draw in HDR10).
    video_hdr: bool,
    /// The cover or poster as drawn (made for its device size).
    art: Option<((u32, u32), Arc<RenderImage>)>,
    art_making: Option<(u32, u32)>,
    /// A timer that draws it again (a playing sound's transport, the
    /// video's next frame).
    tick: Option<TimerId>,
    /// Whether the video played when last drawn (what keeps the screen
    /// awake follows it).
    video_played: bool,
    /// Trimming (the Viewer's Trim…): the stretch kept (seconds), and
    /// which end is being dragged (true: the end).
    trim: Option<(f64, f64)>,
    trim_drag: Option<bool>,
    /// Where the video's picture was drawn: its bounds, the place it's in
    /// (what of it shows), and its frame's size in pixels (an editor's
    /// crop frame over it).
    video_placed: std::cell::Cell<Option<(Bounds<Pixels>, Bounds<Pixels>, (u32, u32))>>,
    /// Where the transport's parts were last drawn.
    transport_at: std::cell::Cell<TransportAt>,
    /// The window is full screen, and has the keyboard (as `body` last
    /// found them: a drawing can only look).
    fullscreen: bool,
    active: bool,
    text_scroll: ScrollHandle,
    list: ListModel,
    sheet: usize,
    table: Child<table::TableView>,
    rich: Child<rich::RichView>,
    /// The font family registered for a font preview.
    font: Option<SharedString>,
    /// What text and listings show, shared with the views that draw them
    /// (taken from the body as it comes, not copied each frame).
    text: Option<Arc<TextRows>>,
    items: Option<Arc<Vec<crate::archive::Item>>>,
    /// The text selected (line, byte), where it began and where it goes.
    text_sel: Option<(TextAt, TextAt)>,
    text_drag: bool,
    /// How far into its rows a text starts (past the line numbers).
    text_left: f32,
    /// Find (a text's, a formatted document's): its field, what's looked
    /// for (lower case), and in a text where it is (line, bytes) and
    /// which is shown (a document's are its own).
    find: Option<Child<TextField>>,
    query: String,
    text_matches: Vec<(usize, usize, usize)>,
    text_current: Option<usize>,
}

/// A place in a text: its line, a byte in it.
type TextAt = (usize, usize);

/// A text's lines (shared: drawn and told to agents each frame without
/// a copy) and their token classes.
type TextRows = (Vec<SharedString>, Vec<Vec<(u32, u32, crate::text::Tok)>>);

fn text_rows(t: &mut crate::text::Text) -> Arc<TextRows> {
    Arc::new((std::mem::take(&mut t.lines).into_iter().map(SharedString::from).collect(), std::mem::take(&mut t.spans)))
}

impl PreviewView {
    /// Start loading `item` (`zoomable`: pictures at full size, to zoom
    /// and pan, as the Viewer's; else fitted, as Quick Look's).
    pub fn new(item: Item, generation: u64, zoomable: bool, cx: &mut Cx<Self>) -> PreviewView {
        let image = image_view(cx, zoomable);
        let mut v = PreviewView {
            item,
            generation,
            body: None,
            failed: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            want: load::Want { max_image: if zoomable { load::MAX_ZOOMABLE } else { load::MAX_IMAGE } },
            image,
            player: None,
            video: None,
            video_zoom: VideoZoom::default(),
            controls_until: None,
            controls_hide: None,
            frame: None,
            yuv_frame: None,
            first_page: false,
            decode_with: video::Device::default(),
            rate: 1.0,
            decoding_said: None,
            video_hdr: false,
            art: None,
            art_making: None,
            tick: None,
            video_played: false,
            trim: None,
            trim_drag: None,
            video_placed: Default::default(),
            transport_at: Default::default(),
            fullscreen: false,
            active: true,
            text_scroll: ScrollHandle::new(),
            list: ListModel::new(false),
            sheet: 0,
            table: cx.new_view(table::TableView::new),
            rich: cx.new_view(|_| rich::RichView::default()),
            font: None,
            text: None,
            items: None,
            text_sel: None,
            text_drag: false,
            find: None,
            query: String::new(),
            text_matches: Vec::new(),
            text_current: None,
            text_left: 0.0,
        };
        v.spawn(cx);
        v
    }

    fn spawn(&mut self, cx: &mut Cx<Self>) {
        let (item, cancel, generation, want) = (self.item.clone(), self.cancel.clone(), self.generation, self.want);
        let to = cx.messenger();
        std::thread::Builder::new()
            .name("quicklook".into())
            .spawn(move || {
                load::load(&item, want, &cancel, &mut |u| {
                    to.send(Msg::Loaded(generation, u));
                })
            })
            .ok();
        // "Loading" shows only if it takes a while.
        cx.after(SLOW, Msg::Slow(generation));
    }

    /// Show another item in the same window (what's shown stays until it
    /// comes, so stepping through a folder doesn't flash).
    pub fn retarget(&mut self, item: Item, generation: u64, cx: &mut Cx<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.item = item;
        self.generation = generation;
        self.failed = None;
        self.first_page = false;
        self.player = None;
        self.video = None;
        self.stop_tick(cx);
        self.sheet = 0;
        self.started = Instant::now();
        self.table.update(cx, |t, _| t.reset());
        self.spawn(cx);
    }

    fn apply(&mut self, u: Update, cx: &mut Cx<Self>) {
        match u {
            Update::Body(mut b) => {
                self.forget_pictures();
                self.font = None;
                match &mut b {
                    Body::Text(t) => {
                        self.text = Some(text_rows(t));
                        self.text_sel = None;
                    }
                    Body::Listing { items, .. } => self.items = Some(Arc::new(std::mem::take(items))),
                    Body::Rich { blocks, .. } => {
                        let (blocks, generation, base) = (Arc::new(std::mem::take(blocks)), self.generation, self.item.path.parent().map(Path::to_owned));
                        self.rich.update(cx, |r, cx| r.show(blocks, generation, base, cx));
                    }
                    _ => {}
                }
                if let Body::Media(md) = &b
                    && md.playable
                    && !md.video
                {
                    let player = audio::Player::start(&input::Input::file(&self.item.path));
                    if self.rate != 1.0 {
                        player.set_rate(self.rate);
                    }
                    self.player = Some(player);
                }
                self.video = match &b {
                    // (Frames as the kit takes them: BGRA.)
                    Body::Media(md) if md.playable && md.video => Some(video::Video::start_with(
                        &input::Input::file(&self.item.path),
                        md.duration.unwrap_or(0.0),
                        video::Options { playing: true, picture: true, sound: true, bgra: true, yuv: true, device: self.decode_with },
                    )),
                    _ => None,
                };
                if let Some(v) = &self.video
                    && self.rate != 1.0
                {
                    v.set_rate(self.rate);
                }
                // A video's poster shows until its first frame comes.
                if let Body::Media(md) = &b
                    && md.video
                    && let Some(pic) = md.picture.clone()
                {
                    let generation = self.generation;
                    cx.work(
                        move || image::RgbaImage::from_raw(pic.size[0] as u32, pic.size[1] as u32, pic.to_bgra_unmultiplied()).map(|img| Arc::new(RenderImage::new([image::Frame::new(img)]))),
                        move |made| Msg::Poster(generation, made),
                    );
                }
                if let Body::Image { image, full, note, .. } = &b {
                    let (picture, full, note) = (image.clone(), *full, note.clone());
                    self.image.update(cx, |v, cx| {
                        v.set_name(note);
                        v.set_picture(picture, full, cx);
                    });
                }
                self.body = Some(b);
                self.failed = None;
                cx.emit(PreviewEvent::Shown);
            }
            // A PDF's first page, as a picture (Ben, 2026-11-07: Quick
            // Look showed a PDF as its icon; pages are the Viewer's, its
            // Open with Viewer).
            Update::Page(0, page) => {
                if let Some(Body::Pages { note, .. }) = &self.body {
                    let (full, note) = ([page.size[0] as f32, page.size[1] as f32], note.clone());
                    self.image.update(cx, |v, cx| {
                        v.set_name(note);
                        v.set_picture(page, full, cx);
                    });
                    self.first_page = true;
                    cx.invalidate();
                }
            }
            Update::Page(..) => {}
            Update::Highlighted(mut t) => {
                if let Some(Body::Text(old)) = &mut self.body {
                    self.text = Some(text_rows(&mut t));
                    old.note = t.note;
                }
            }
            Update::Failed(e) => {
                self.body = None;
                self.failed = Some(e);
                cx.emit(PreviewEvent::Shown);
            }
        }
        cx.invalidate();
    }

    /// (A picture let go leaves the GPU's atlas by itself.)
    fn forget_pictures(&mut self) {
        self.frame = None;
        self.yuv_frame = None;
        self.art = None;
        self.art_making = None;
    }

    pub fn stop(&mut self, cx: &mut Cx<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.player = None;
        self.video = None;
        self.stop_tick(cx);
    }

    fn stop_tick(&mut self, cx: &mut Cx<Self>) {
        if let Some(timer) = self.tick.take() {
            cx.cancel(timer);
        }
    }

    /// What is shown, once loaded.
    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }

    /// The picture's view (zoom, scale, where it's drawn, dragging out).
    pub fn image_view(&self) -> &Child<ImageView> {
        &self.image
    }

    /// Whether the video it plays is HDR (as far as it has been seen).
    pub fn video_is_hdr(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.is_hdr())
    }

    /// Its video's HDR pictures drawn as HDR10 (a window drawing in it),
    /// or mapped for SDR.
    pub fn draw_video_hdr(&self, on: bool) {
        if let Some(v) = &self.video {
            v.draw_hdr(on);
        }
    }

    /// What its picture's numbers are in: a wide photo's or video's
    /// Display P3 (the window may draw in it, `WindowCx::set_gamut`), else
    /// sRGB.
    pub fn gamut(&self, cx: &WindowCx) -> olo_kit::Gamut {
        match &self.frame {
            _ if self.video.is_some() && self.yuv_frame.is_some() => olo_kit::Gamut::Srgb,
            Some(f) if self.video.is_some() => f.gamut(),
            _ => self.image.read(cx).gamut(),
        }
    }

    /// Whether a picture is shown (so zoom commands apply).
    /// Whether it shows a video that plays.
    pub fn is_video(&self) -> bool {
        self.video.is_some()
    }

    /// The video's chapters (seconds, title), if it has any.
    pub fn chapters(&self) -> Vec<(f64, String)> {
        match &self.body {
            Some(Body::Media(md)) => md.chapters.clone(),
            _ => Vec::new(),
        }
    }

    /// Where the video is (seconds), if it plays.
    pub fn position(&self) -> Option<f64> {
        self.video.as_ref().map(Transport::position)
    }

    /// The video shown from `secs`.
    pub fn seek_to(&mut self, secs: f64, cx: &mut Cx<Self>) {
        if let Some(v) = &self.video {
            Transport::seek(v, secs.max(0.0));
            cx.invalidate();
        }
    }

    /// What decodes video here, as asked (`Device::Default`: the
    /// person's choice in Preferences).
    pub fn decode_with(&self) -> video::Device {
        self.decode_with
    }

    /// What decodes the video showing now ("NVIDIA's GPU (NVDEC)"), once
    /// it has started.
    pub fn decoding(&self) -> Option<&'static str> {
        self.video.as_ref().map(|v| v.decoding()).filter(|d| !d.is_empty())
    }

    /// Which GPUs here decode the video showing (known with
    /// [`PreviewView::decoding`]).
    pub fn decoders(&self) -> video::Decoders {
        self.video.as_ref().map(|v| v.decoders()).unwrap_or_default()
    }

    /// The video decoded by `device` from now on: started again there, at
    /// the moment it's at, playing or not as it was.
    pub fn set_decode_with(&mut self, device: video::Device, cx: &mut Cx<Self>) {
        if device == self.decode_with {
            return;
        }
        self.decode_with = device;
        let Some(old) = self.video.take() else { return };
        let (at, playing) = (Transport::position(&old), Transport::playing(&old));
        let duration = Transport::duration(&old);
        drop(old);
        let v = video::Video::start_with(&input::Input::file(&self.item.path), duration, video::Options { playing, picture: true, sound: true, bgra: true, yuv: true, device });
        Transport::seek(&v, at);
        if self.rate != 1.0 {
            v.set_rate(self.rate);
        }
        self.video = Some(v);
        cx.invalidate();
    }

    /// The video's length (seconds), if it plays.
    pub fn duration(&self) -> Option<f64> {
        let v = self.video.as_ref()?;
        Some(self.body.as_ref().and_then(|b| if let Body::Media(md) = b { md.duration } else { None }).unwrap_or_else(|| Transport::duration(v)))
    }

    /// Where the video's picture was last drawn (window points), the
    /// place it's in, and its frame's size (pixels).
    pub fn video_picture(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>, (u32, u32))> {
        self.video_placed.get()
    }

    /// The stretch kept while trimming (seconds), if trimming.
    pub fn trimming(&self) -> Option<(f64, f64)> {
        self.trim
    }

    /// Trimming begun (all of it kept, the handles at its ends) or ended.
    pub fn set_trimming(&mut self, on: bool, cx: &mut Cx<Self>) {
        self.trim = match (on, &self.video) {
            (true, Some(v)) => {
                let dur = self.body.as_ref().and_then(|b| if let Body::Media(md) = b { md.duration } else { None }).unwrap_or_else(|| Transport::duration(v));
                Some((0.0, dur.max(0.0)))
            }
            _ => None,
        };
        self.trim_drag = None;
        cx.invalidate();
    }

    pub fn is_image(&self) -> bool {
        self.failed.is_none() && matches!(self.body, Some(Body::Image { .. }))
    }

    /// Whether the arrow keys are free for the caller (nothing shown
    /// scrolls or has a selection to move).
    pub fn arrows_free(&self) -> bool {
        self.failed.is_some() || matches!(self.body, None | Some(Body::Image { .. } | Body::Pages { .. } | Body::Media(_) | Body::Font { .. } | Body::Card { .. }))
    }

    /// Show an edited picture in place of the one loaded (see
    /// [`edit`](crate::edit)).
    pub fn show_edited(&mut self, image: load::Image, full: [f32; 2], note: String, cx: &mut Cx<Self>) {
        let picture = image.clone();
        self.image.update(cx, |v, cx| v.set_picture(picture, full, cx));
        self.body = Some(Body::Image { image, full, note });
        self.failed = None;
        cx.invalidate();
    }

    /// One line on what is shown ("PDF document, 10 pages").
    pub fn note(&self) -> String {
        match (&self.body, &self.failed) {
            (Some(b), None) => note_of(b),
            _ => load::summary(&self.item),
        }
    }

    /// A video is playing: keep the screen awake.
    pub fn playing_video(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.playing())
    }

    /// A video or a sound is showing: ← and → move its time
    /// (`skip`) rather than go to the next file.
    pub fn has_media_time(&self) -> bool {
        self.failed.is_none() && (self.video.is_some() || self.player.is_some())
    }

    /// How far a press of ← or → moves a video or a sound.
    pub const SKIP_SECS: f64 = 10.0;

    /// How a video's picture fills its place.
    pub fn video_zoom(&self) -> VideoZoom {
        self.video_zoom
    }

    pub fn set_video_zoom(&mut self, zoom: VideoZoom, cx: &mut Cx<Self>) {
        if self.video_zoom != zoom {
            self.video_zoom = zoom;
            cx.invalidate();
        }
    }

    /// Full screen: the transport shown a while (the pointer moved), and
    /// hidden again after, by one timer (re-armed as it fires, if the
    /// pointer moved since).
    fn show_controls(&mut self, cx: &mut Cx<Self>) {
        let was = self.controls_until.is_some_and(|t| t > Instant::now());
        self.controls_until = Some(Instant::now() + CONTROLS_FOR);
        if !was {
            cx.invalidate();
        }
        if self.controls_hide.is_none() {
            self.hide_controls_in(CONTROLS_FOR, cx);
        }
    }

    fn hide_controls_in(&mut self, wait: Duration, cx: &mut Cx<Self>) {
        self.controls_hide = Some(cx.after_do(wait, |v, cx| {
            v.controls_hide = None;
            match v.controls_until.map(|until| until.saturating_duration_since(Instant::now())) {
                // (The pointer moved since: once more, for what's left.)
                Some(left) if !left.is_zero() => v.hide_controls_in(left, cx),
                _ => cx.invalidate(),
            }
        }));
    }

    /// Whether the transport shows full screen: the pointer moved lately,
    /// or the video is paused.
    fn controls_showing(&self) -> bool {
        self.controls_until.is_some_and(|t| t > Instant::now()) || !self.playing_video()
    }

    /// Move the video's or sound's time by `secs` (Ben, 2026-11-05: "the
    /// left and right arrows should skip back and forward 10s in the
    /// video"), held at its start and end; false if nothing plays here.
    pub fn skip(&mut self, secs: f64, cx: &mut Cx<Self>) -> bool {
        let done = if let Some(v) = &self.video {
            v.seek((v.position() + secs).clamp(0.0, v.duration()));
            true
        } else if let Some(p) = &self.player {
            let at = p.state.position_ms.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
            let end = p.state.duration_ms.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
            p.seek((at + secs).clamp(0.0, end.max(0.0)));
            true
        } else {
            false
        };
        cx.invalidate();
        done
    }

    /// The speeds offered (Playback Speed; Ben, 2026-11-09: "the ability
    /// to watch videos at custom speeds from 0.25x up to 2x").
    pub const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

    /// The speed chosen here (`set_playback_rate`), whether or not what
    /// it shows now plays: the next file plays at it.
    pub fn chosen_rate(&self) -> f64 {
        self.rate
    }

    /// How fast its sound or video plays (1: as recorded), if it has one.
    pub fn playback_rate(&self) -> Option<f64> {
        (self.video.is_some() || self.player.is_some()).then_some(self.rate)
    }

    /// Play its sound or video at `rate` (0.25 to 2), the sound's pitch
    /// kept, from now on, and the files shown here after it too.
    pub fn set_playback_rate(&mut self, rate: f64, cx: &mut Cx<Self>) {
        let rate = rate.clamp(audio::MIN_RATE, audio::MAX_RATE);
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        // (A video's sound is its own.)
        if let Some(v) = &self.video {
            v.set_rate(rate);
        } else if let Some(p) = &self.player {
            p.set_rate(rate);
        }
        cx.invalidate();
    }

    /// The next speed of [`Self::SPEEDS`] up (`by` 1: `>`) or down (-1:
    /// `<`); false with nothing playing, or at the end.
    pub fn step_playback_rate(&mut self, by: i32, cx: &mut Cx<Self>) -> bool {
        let Some(now) = self.playback_rate() else { return false };
        let next = if by > 0 { Self::SPEEDS.iter().find(|&&r| r > now + 1e-6) } else { Self::SPEEDS.iter().rev().find(|&&r| r < now - 1e-6) };
        let Some(&next) = next else { return false };
        self.set_playback_rate(next, cx);
        true
    }

    /// Toggle sound or video playback (Space or P in a viewer).
    pub fn toggle_playback(&mut self, cx: &mut Cx<Self>) -> bool {
        let done = if let Some(v) = &self.video {
            v.toggle();
            true
        } else if let Some(p) = &self.player {
            p.toggle();
            true
        } else {
            false
        };
        cx.invalidate();
        done
    }

    /// The window size that suits what is shown, within the screen
    /// (`chrome`: the height the caller adds round it).
    pub fn ideal_size(&self, screen: Option<(u32, u32)>, chrome: f32) -> (u32, u32) {
        let (sw, sh) = screen.map_or((1440.0, 900.0), |(w, h)| (w as f32, h as f32));
        let (max_w, max_h) = (sw * 0.8, sh * 0.82 - chrome);
        let fit = |w: f32, h: f32| {
            let k = (max_w / w).min(max_h / h).min(1.0);
            ((w * k).round().max(320.0), (h * k).round().max(200.0))
        };
        let (w, h) = match (&self.body, &self.failed) {
            (_, Some(_)) => (420.0, 220.0),
            (None, None) => (420.0, 260.0),
            (Some(b), _) => match b {
                // One picture pixel a point, as a 72 dpi picture on a Mac.
                Body::Image { full, .. } => fit(full[0], full[1]),
                Body::Pages { page_size, .. } => {
                    let (pw, ph) = (page_size.0.max(1.0), page_size.1.max(1.0));
                    fit(max_h * pw / ph + 2.0 * m::SPACING, max_h)
                }
                Body::Text(_) => {
                    // (Its lines are with the view that draws them.)
                    let lines = self.text.as_ref().map_or(&[][..], |t| t.0.as_slice());
                    let longest = lines.iter().take(400).map(|l| l.chars().count()).max().unwrap_or(40);
                    let w = (longest.clamp(40, 110) as f32 * 7.3 + 60.0).min(max_w);
                    (w, (lines.len() as f32 * TEXT_ROW + 2.0 * m::SPACING).clamp(200.0, max_h))
                }
                Body::Rich { .. } => (744.0f32.min(max_w), max_h),
                Body::Table { sheets, .. } => {
                    let rows = sheets.first().map_or(0, |s| s.1.len());
                    let tabs = if sheets.len() > 1 { 26.0 } else { 0.0 };
                    (880.0f32.min(max_w), (rows as f32 * m::LIST_ROW + 22.0 + tabs).clamp(200.0, max_h))
                }
                Body::Listing { .. } => (560.0, (self.items.as_ref().map_or(0, |i| i.len()) as f32 * m::LIST_ROW + 22.0).clamp(200.0, max_h)),
                Body::Media(md) => match (&md.picture, md.video) {
                    (Some(p), true) => {
                        // Only the picture scales; the rows beneath keep their height.
                        let extra = if md.playable { VIDEO_INFO + TRANSPORT } else { VIDEO_INFO };
                        let (pw, ph) = (p.size[0] as f32, p.size[1] as f32);
                        let k = (max_w / pw).min((max_h - extra) / ph).min(1.0);
                        ((pw * k).round().max(480.0), (ph * k).round() + extra)
                    }
                    _ => (560.0, 220.0),
                },
                Body::Font { .. } => (760.0f32.min(max_w), 560.0f32.min(max_h)),
                Body::Card { .. } => (420.0, 220.0),
            },
        };
        (w.round() as u32, (h + chrome).round() as u32)
    }

    /// The cover or poster drawn at device size `want`: made on a worker
    /// for that size (a texel a device pixel).
    fn art(&mut self, picture: &load::Image, want: (u32, u32), cx: &mut Cx<Self>) -> Option<Arc<RenderImage>> {
        if let Some((at, img)) = &self.art
            && *at == want
        {
            return Some(img.clone());
        }
        if self.art_making != Some(want) {
            let src = picture.clone();
            self.art_making = Some(want);
            cx.work(move || image_view::resampled(&*src, want), move |made| Msg::Art(want, made));
        }
        // The last one, at whatever size, meanwhile.
        self.art.as_ref().map(|(_, a)| a.clone())
    }

    /// Draw it again in `d` (a playing transport, a video's next frame).
    fn tick_in(&mut self, d: Duration, cx: &mut Cx<Self>) {
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.after_do(d, |v, cx| {
            v.tick = None;
            cx.invalidate();
        }));
    }
}

fn note_of(b: &Body) -> String {
    match b {
        Body::Image { note, .. } | Body::Pages { note, .. } | Body::Rich { note, .. } | Body::Table { note, .. } | Body::Listing { note, .. } | Body::Font { note, .. } | Body::Card { note } => {
            note.clone()
        }
        Body::Text(t) => t.note.clone(),
        Body::Media(m) => m.note.clone(),
    }
}

/// The icon, name and facts (and why there's no preview, with `error`),
/// in the middle of `r`; a spinner under them while it loads slowly.
fn card(g: &mut Painter, r: Bounds<Pixels>, item: &Item, error: Option<&str>, spinning: bool) {
    let p = theme::palette();
    g.paint_quad(olo_kit::fill(r, style::c(p.panel_background)));
    let icon = 96.0;
    let top = f32::from(r.center().y) - (icon + 3.0 * m::LABEL_ROW) / 2.0;
    let ib = style::snap_centred(g, point(r.center().x, px(top + icon / 2.0)), icon, icon);
    icons::paint(g, ib, &item.icon);
    let mut y = px(top + icon + m::SPACING + m::LABEL_ROW / 2.0);
    let name =
        style::label_fit(g, r.center().x, y, Align::Center, item.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), r.size.width - px(2.0 * m::WINDOW_INSET));
    agent::record(g, agent::Node::label(item.name.clone(), name));
    y += px(m::LABEL_ROW);
    let facts = if item.is_dir { files::format_time(item.modified) } else { format!("{} — {}", files::format_size(item.size), files::format_time(item.modified)) };
    let dim = style::disabled_text(&p);
    let at = style::label(g, r.center().x, y, Align::Center, facts.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
    agent::record(g, agent::Node::label(facts, at));
    if let Some(e) = error {
        y += px(m::LABEL_ROW);
        let at = style::label_fit(g, r.center().x, y, Align::Center, e.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, r.size.width - px(2.0 * m::WINDOW_INSET));
        agent::record(g, agent::Node::label(e.to_owned(), at));
    }
    if spinning {
        style::spinner(g, point(r.center().x, r.bottom() - px(40.0)), 9.0, dim);
        g.paint_again_within(1.0 / 60.0);
    }
}

impl View for PreviewView {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Loaded(generation, u) => {
                if generation == self.generation {
                    self.apply(u, cx);
                }
            }
            Msg::Slow(generation) => {
                if generation == self.generation && self.body.is_none() {
                    cx.invalidate();
                }
            }
            Msg::Poster(generation, made) => {
                if generation == self.generation && self.frame.is_none() {
                    self.frame = made;
                    cx.invalidate();
                }
            }
            Msg::Art(want, made) => {
                if self.art_making == Some(want) {
                    self.art_making = None;
                    if let Some(img) = made {
                        self.art = Some((want, img));
                    }
                    cx.invalidate();
                }
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.active = cx.is_window_active();
        let content: Element = match (&self.failed, &self.body) {
            (Some(e), _) => {
                let (item, e) = (self.item.clone(), e.clone());
                canvas(move |g, b| card(g, b, &item, Some(&e), false)).size_full().into_any_element()
            }
            (None, None) => {
                let (item, spinning) = (self.item.clone(), self.started.elapsed() > SLOW);
                canvas(move |g, b| card(g, b, &item, None, spinning)).size_full().into_any_element()
            }
            (None, Some(Body::Image { .. })) => self.image.element(),
            (None, Some(Body::Pages { .. })) if self.first_page => self.image.element(),
            (None, Some(Body::Card { .. } | Body::Pages { .. })) => {
                let item = self.item.clone();
                canvas(move |g, b| card(g, b, &item, None, false)).size_full().into_any_element()
            }
            (None, Some(Body::Text(_))) => {
                let text = self.text_view(cx);
                match self.find_bar(cx) {
                    Some(bar) => div().size_full().flex().flex_col().child(bar).child(div().flex_1().min_h(px(0.0)).child(text)).into_any_element(),
                    None => text,
                }
            }
            (None, Some(Body::Listing { .. })) => self.listing(cx),
            (None, Some(Body::Font { .. })) => self.font_view(cx),
            (None, Some(Body::Media(_))) => self.media_view(cx),
            (None, Some(Body::Rich { .. })) => match self.find_bar(cx) {
                Some(bar) => div().size_full().flex().flex_col().child(bar).child(div().flex_1().min_h(px(0.0)).child(&self.rich)).into_any_element(),
                None => self.rich.element(),
            },
            (None, Some(Body::Table { .. })) => self.table_view(cx),
        };
        div().size_full().relative().child(content).into_any_element()
    }
}

impl PreviewView {
    // ---- the text's selection ----------------------------------------------

    /// It shows text that can be selected (a text, a formatted document).
    pub fn has_text(&self) -> bool {
        matches!(self.body, Some(Body::Text(_) | Body::Rich { .. }))
    }

    pub fn has_text_selection(&self, cx: &WindowCx) -> bool {
        match &self.body {
            Some(Body::Rich { .. }) => self.rich.read(cx).has_selection(),
            _ => self.text_sel.is_some_and(|(a, b)| a != b),
        }
    }

    /// The text selected, as it's copied.
    pub fn selected_text(&self, cx: &WindowCx) -> String {
        if matches!(self.body, Some(Body::Rich { .. })) {
            return self.rich.read(cx).selected_text();
        }
        let (Some(lines), Some(sel)) = (&self.text, self.text_sel) else { return String::new() };
        let (a, b) = if sel.0 <= sel.1 { sel } else { (sel.1, sel.0) };
        let mut out = String::new();
        for i in a.0..=b.0.min(lines.0.len().saturating_sub(1)) {
            let line = &lines.0[i];
            let from = if i == a.0 { a.1.min(line.len()) } else { 0 };
            let to = if i == b.0 { b.1.min(line.len()) } else { line.len() };
            out.push_str(line.get(from..to.max(from)).unwrap_or(""));
            if i < b.0 {
                out.push('\n');
            }
        }
        out
    }

    pub fn copy(&self, cx: &WindowCx) {
        let text = self.selected_text(cx);
        if !text.is_empty() {
            olo_kit::clipboard::write(olo_kit::ClipboardItem::new_string(text));
        }
    }

    pub fn select_all(&mut self, cx: &mut Cx<Self>) {
        if matches!(self.body, Some(Body::Rich { .. })) {
            self.rich.update(cx, |r, cx| r.select_all(cx));
            return;
        }
        let Some(lines) = &self.text else { return };
        let last = lines.0.len().saturating_sub(1);
        self.text_sel = Some(((0, 0), (last, lines.0.get(last).map_or(0, |l| l.len()))));
        cx.invalidate();
    }

    // ---- find ----------------------------------------------------------------

    pub fn open_find(&mut self, cx: &mut Cx<Self>) {
        if let Some(f) = &self.find {
            f.update(cx, |f, cx| f.select_all_text(cx));
            f.focus(cx);
            return;
        }
        let field = text_field(cx, |f| f.hint("Find"));
        cx.subscribe(&field, |v: &mut PreviewView, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                let Some(q) = v.find.as_ref().map(|f| f.read(cx).text().to_lowercase()) else { return };
                v.set_query(q, cx);
            }
            TextFieldEvent::Entered => v.find_step(cx.modifiers().shift, cx),
            _ => {}
        });
        field.focus(cx);
        self.find = Some(field);
        cx.invalidate();
    }

    pub fn close_find(&mut self, cx: &mut Cx<Self>) {
        self.find = None;
        self.query.clear();
        self.text_matches.clear();
        self.text_current = None;
        self.rich.update(cx, |r, cx| r.close_find(cx));
        cx.invalidate();
    }

    /// Something's found (Find Next can go to it).
    pub fn finding(&self, cx: &WindowCx) -> bool {
        match &self.body {
            Some(Body::Rich { .. }) => self.find.is_some() && self.rich.read(cx).found().0 > 0,
            _ => !self.text_matches.is_empty(),
        }
    }

    fn set_query(&mut self, q: String, cx: &mut Cx<Self>) {
        self.query = q.clone();
        if matches!(self.body, Some(Body::Rich { .. })) {
            self.rich.update(cx, |r, cx| r.set_query(q, cx));
            cx.invalidate();
            return;
        }
        self.text_matches.clear();
        if let Some(lines) = &self.text
            && !q.is_empty()
        {
            for (i, line) in lines.0.iter().enumerate() {
                let low = line.to_lowercase();
                // (Where lower case keeps every byte where it was.)
                if low.len() == line.len() {
                    self.text_matches.extend(low.match_indices(&q).map(|(a, m)| (i, a, a + m.len())));
                }
            }
        }
        self.text_current = (!self.text_matches.is_empty()).then_some(0);
        self.show_text_match(cx);
        cx.invalidate();
    }

    pub fn find_step(&mut self, back: bool, cx: &mut Cx<Self>) {
        if matches!(self.body, Some(Body::Rich { .. })) {
            self.rich.update(cx, |r, cx| r.step(back, cx));
            cx.invalidate();
            return;
        }
        let n = self.text_matches.len();
        if n == 0 {
            return;
        }
        self.text_current = Some(match self.text_current {
            Some(c) if back => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
            None => 0,
        });
        self.show_text_match(cx);
        cx.invalidate();
    }

    /// A text's match shown: selected, its line in the middle of the view.
    fn show_text_match(&mut self, cx: &mut Cx<Self>) {
        let Some(&(line, a, b)) = self.text_current.and_then(|c| self.text_matches.get(c)) else { return };
        self.text_sel = Some(((line, a), (line, b)));
        let view = f32::from(self.text_scroll.bounds().size.height);
        let x = self.text_scroll.offset().x;
        self.text_scroll.set_offset(cx, point(x, px(-((line as f32 + 0.5) * TEXT_ROW - view / 2.0).max(0.0))));
    }

    fn find_bar(&self, cx: &mut Cx<Self>) -> Option<Element> {
        let field = self.find.as_ref()?;
        let (n, current, more) = match &self.body {
            Some(Body::Rich { .. }) => self.rich.read(cx).found(),
            _ => (self.text_matches.len(), self.text_current, false),
        };
        let said = olo_kit::widgets::find_said(&self.query, n, current, more.then(|| "laying it out".to_owned()));
        let step = cx.listener(|v: &mut PreviewView, back: &bool, cx| v.find_step(*back, cx));
        let close = cx.listener(|v: &mut PreviewView, _: &(), cx| v.close_find(cx));
        Some(olo_kit::widgets::find_bar(cx, field, said, n > 0, step, close))
    }

    /// A line shaped as the text view draws it.
    fn shape_text_line(text: &SharedString) -> olo_kit::ShapedLine {
        let run = TextRun { len: text.len(), font: style::mono_font(FontWeight::NORMAL), color: olo_kit::black(), background_color: None, underline: None, strikethrough: None, size: None };
        olo_kit::thread_text_system().shape_line(text.clone(), px(style::MONO_SIZE), &[run], None)
    }

    /// The place in the text under window point `at`, its rows' top
    /// left at `origin` (where they are in the window now).
    fn text_at(&self, origin: Point<Pixels>, at: Point<Pixels>) -> Option<TextAt> {
        let lines = self.text.as_ref()?;
        let n = lines.0.len();
        if n == 0 {
            return None;
        }
        let left = origin.x + px(self.text_left);
        let row = f32::from(at.y - origin.y) / TEXT_ROW;
        if row < 0.0 {
            return Some((0, 0));
        }
        let row = row as usize;
        if row >= n {
            return Some((n - 1, lines.0[n - 1].len()));
        }
        let line = &lines.0[row];
        Some((row, Self::shape_text_line(line).closest_index_for_x(at.x - left)))
    }

    fn text_press(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        let Some(at) = self.text_at(cx.listener_bounds().origin, e.position) else { return };
        let Some(lines) = self.text.clone() else { return };
        let line = &lines.0[at.0];
        match e.click_count {
            2 => {
                // The word: letters, digits and underscores.
                let word = |c: char| c.is_alphanumeric() || c == '_';
                let a = line[..at.1].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(at.1, |(i, _)| i);
                let b = line[at.1..].char_indices().find(|(_, c)| !word(*c)).map_or(line.len(), |(i, _)| at.1 + i);
                self.text_sel = Some(((at.0, a), (at.0, b)));
            }
            n if n >= 3 => self.text_sel = Some(((at.0, 0), (at.0, line.len()))),
            _ => {
                self.text_sel = match self.text_sel {
                    Some((anchor, _)) if e.modifiers.shift => Some((anchor, at)),
                    _ => Some((at, at)),
                };
                self.text_drag = true;
            }
        }
        cx.invalidate();
    }

    fn text_moved(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        if !self.text_drag || e.pressed_button != Some(MouseButton::Left) {
            self.text_drag = false;
            return;
        }
        if let (Some(at), Some((anchor, old))) = (self.text_at(cx.listener_bounds().origin, e.position), self.text_sel)
            && at != old
        {
            self.text_sel = Some((anchor, at));
            cx.invalidate();
        }
    }

    fn text_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(lines) = self.text.clone() else { return div().into_any_element() };
        let p = theme::palette();
        let digits = lines.0.len().max(1).to_string().len();
        let longest = lines.0.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        // The face's advance: one width for every character.
        let char_w = style::text_width_app("0000000000", style::MONO_SIZE, &style::mono_font(FontWeight::NORMAL)) / 10.0;
        let gutter = (digits as f32 * char_w + 2.0 * GUTTER_PAD).ceil();
        let content_w = gutter + longest as f32 * char_w + 2.0 * GUTTER_PAD;
        let rows = lines.0.len();
        self.text_left = gutter + GUTTER_PAD;
        let (active, sel) = (self.active, self.text_sel.map(|s| if s.0 <= s.1 { s } else { (s.1, s.0) }));
        // What Find found, but the one shown (it's the selection).
        let found: Arc<Vec<(usize, usize, usize)>> = Arc::new(self.text_matches.iter().enumerate().filter(|(k, _)| Some(*k) != self.text_current).map(|(_, m)| *m).collect());
        let paint = canvas(move |g, b| {
            let p = theme::palette();
            // (Only the rows that show: drawn again as it's scrolled.)
            let view = g.visible_bounds();
            let font = style::mono_font(FontWeight::NORMAL);
            let gutter_bg = style::shade(&p, p.document_background, 1.04);
            let dim = style::disabled_text(&p);
            g.paint_quad(olo_kit::fill(Bounds::new(b.origin, size(px(gutter), b.size.height)), gutter_bg));
            let first = ((f32::from(view.top() - b.top()) / TEXT_ROW).floor().max(0.0)) as usize;
            let last = (((f32::from(view.bottom() - b.top())) / TEXT_ROW).ceil().max(0.0) as usize).min(rows);
            let left = b.left() + px(gutter + GUTTER_PAD);
            // Found, in view, behind its text.
            if !found.is_empty() {
                let ink = style::c(p.hue(olo_theme::Hue::Yellow)).opacity(0.45);
                let from = found.partition_point(|m| m.0 < first);
                for &(i, a, z) in found[from..].iter().take_while(|m| m.0 < last) {
                    let shaped = Self::shape_text_line(&lines.0[i]);
                    let top = b.top() + px(i as f32 * TEXT_ROW);
                    style::fill_rect(g, Bounds::from_corners(point(left + shaped.x_for_index(a), top), point(left + shaped.x_for_index(z), top + px(TEXT_ROW))), ink);
                }
            }
            // The selection in view, behind its text (past a line's
            // end, a character's width: the line break goes with it).
            if let Some((a, z)) = sel {
                let ink = style::selection_behind(&p, active);
                for i in first.max(a.0)..last.min(z.0 + 1) {
                    let line = &lines.0[i];
                    let shaped = Self::shape_text_line(line);
                    let from = if i == a.0 { a.1.min(line.len()) } else { 0 };
                    let to = if i == z.0 { z.1.min(line.len()) } else { line.len() };
                    let x0 = left + shaped.x_for_index(from);
                    let x1 = left + shaped.x_for_index(to) + if i < z.0 { px(char_w) } else { px(0.0) };
                    if x1 > x0 {
                        let top = b.top() + px(i as f32 * TEXT_ROW);
                        style::fill_rect(g, Bounds::from_corners(point(x0, top), point(x1, top + px(TEXT_ROW))), ink);
                    }
                }
            }
            let (lines, spans) = &*lines;
            // The rows' glyphs in one place in the scene's order, not a
            // place found for each (half the time of drawing them).
            g.paint_layer(view, |g| {
                for i in first..last {
                    let cy = b.top() + px(i as f32 * TEXT_ROW + TEXT_ROW / 2.0);
                    style::label(g, b.left() + px(gutter - GUTTER_PAD), cy, Align::Right, (i + 1).to_string(), style::MONO_SIZE, font.clone(), dim);
                    let line = &lines[i];
                    if line.is_empty() {
                        continue;
                    }
                    let runs = code_runs(line, spans.get(i).map_or(&[][..], Vec::as_slice), &p, &font);
                    let at = style::label_runs(g, b.left() + px(gutter + GUTTER_PAD), cy, Align::Left, line.clone(), style::MONO_SIZE, &runs);
                    agent::record(g, agent::Node::label(line.clone(), at));
                }
            });
        })
        .w(px(content_w))
        .h(px(rows as f32 * TEXT_ROW));
        let scroller = div()
            .id("ql-text-rows")
            .min_w_full()
            .w(px(content_w))
            .h(px(rows as f32 * TEXT_ROW))
            .cursor(olo_kit::CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut PreviewView, e: &MouseDownEvent, cx| v.text_press(e, cx)))
            .on_mouse_move(cx.listener(|v: &mut PreviewView, e: &MouseMoveEvent, cx| v.text_moved(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|v: &mut PreviewView, _: &MouseUpEvent, _| v.text_drag = false))
            .child(paint);
        div().size_full().child(ScrollView::new(&self.text_scroll).border(false).horizontal(true).background(p.document_background).child(scroller)).into_any_element()
    }

    fn listing(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(items) = self.items.clone() else { return div().into_any_element() };
        let cols = vec![Column::new("Name", 360.0), Column::new("Size", 90.0).right()];
        let n = items.len();
        ColumnListView::new(cx, format!("ql-list-{}", self.generation), &self.list, cols, n, move |_: &PreviewView, cell: &mut Cell, g| {
            let it = &items[cell.row];
            match cell.column {
                0 => {
                    let ib = style::snap_centred(g, point(cell.bounds.left() + px(12.0), cell.bounds.center().y), 16.0, 16.0);
                    let kind = files::kind_for(Path::new(&it.name), it.is_dir, 0o644);
                    icons::paint(g, ib, kind.icon());
                    let full = cell.bounds;
                    cell.bounds = Bounds::from_corners(point(full.left() + px(22.0), full.top()), full.bottom_right());
                    cell.text(g, it.name.clone());
                    cell.bounds = full;
                }
                _ => cell.text(g, if it.is_dir { "—".to_owned() } else { files::format_size(it.size) }),
            }
        })
        .into_any_element()
    }

    fn font_view(&mut self, _: &mut Cx<Self>) -> Element {
        let Some(Body::Font { data, .. }) = &self.body else { return div().into_any_element() };
        if self.font.is_none() {
            let family = family_name(data).unwrap_or_else(|| self.item.name.clone());
            let _ = olo_kit::thread_text_system().add_fonts(vec![std::borrow::Cow::Owned(data.to_vec())]);
            self.font = Some(family.into());
        }
        let family = self.font.clone().expect("registered");
        let p = theme::palette();
        let ink = style::c(p.document_text);
        let samples: [(f32, &str); 6] = [
            (72.0, "Aa Bb Gg"),
            (28.0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
            (28.0, "abcdefghijklmnopqrstuvwxyz"),
            (28.0, "0123456789 &@?!%(){}[]"),
            (18.0, "The quick brown fox jumps over the lazy dog."),
            (12.0, "The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs."),
        ];
        let mut col = div().flex().flex_col().px(px(2.0 * m::SPACING + 4.0)).py(px(m::SPACING * 2.0)).gap(px(10.0));
        let font = olo_kit::Font { family: family.clone(), ..style::ui_font(FontWeight::NORMAL) };
        for (size, text) in samples {
            col = col.child(div().font(font.clone()).text_size(px(size)).line_height(px((size * 1.3).ceil())).text_color(ink).child(text));
        }
        div().size_full().child(ScrollView::new(&self.text_scroll).border(false).background(p.document_background).child(col)).into_any_element()
    }

    fn table_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(Body::Table { sheets, .. }) = &self.body else { return div().into_any_element() };
        let names: Vec<String> = sheets.iter().map(|s| s.0.clone()).collect();
        let rows = sheets.get(self.sheet).map(|s| s.1.clone());
        let (generation, sheet) = (self.generation, self.sheet);
        if let Some(rows) = rows {
            self.table.update(cx, |t, cx| t.show(rows, (generation, sheet), cx));
        }
        let table = self.table.element();
        if names.len() < 2 {
            return table;
        }
        // The sheets as tabs over the table (where each was drawn, for
        // the press that picks one).
        let shown = self.sheet;
        let hits: std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>> = Default::default();
        let drawn = hits.clone();
        let tabs = canvas(move |g, b| {
            drawn.borrow_mut().clear();
            let p = theme::palette();
            g.paint_quad(olo_kit::fill(b, style::c(p.panel_background)));
            let mut x = b.left() + px(m::SPACING);
            for (k, name) in names.iter().enumerate() {
                let w = f32::from(style::text_width(name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 24.0;
                let br = Bounds::new(point(x, b.top() + px(3.0)), size(px(w), px(20.0)));
                drawn.borrow_mut().push(br);
                let selected = shown == k;
                if selected {
                    style::raised(g, br, &p);
                }
                let weight = if selected { FontWeight::BOLD } else { FontWeight::NORMAL };
                style::label(g, br.center().x, br.center().y, Align::Center, name.clone(), style::PLAIN_SIZE, style::ui_font(weight), style::c(p.panel_text));
                agent::record(g, agent::Node { role: "Tab", name: Some(name.clone().into()), bounds: br, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
                x += px(w + m::TIGHT);
            }
        })
        .size_full();
        let tab_bar = div().id("ql-sheets").h(px(26.0)).w_full().child(tabs).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v: &mut PreviewView, e: &MouseDownEvent, cx| {
                let picked = hits.borrow().iter().position(|b| b.contains(&e.position));
                if let Some(k) = picked
                    && v.sheet != k
                {
                    v.sheet = k;
                    v.table.update(cx, |t, _| t.reset());
                    cx.invalidate();
                }
            }),
        );
        div().size_full().flex().flex_col().child(tab_bar).child(div().flex_1().min_h_0().child(table)).into_any_element()
    }
}

/// A line of code's runs: its token classes' colours (byte ranges), the
/// rest the document's text colour.
fn code_runs(line: &str, spans: &[(u32, u32, crate::text::Tok)], p: &olo_theme::Palette, font: &olo_kit::Font) -> Vec<TextRun> {
    let run = |len: usize, tok: crate::text::Tok| TextRun { len, font: font.clone(), color: style::c(p.syntax(tok)), background_color: None, underline: None, strikethrough: None, size: None };
    let mut out = Vec::new();
    let mut at = 0;
    for &(a, b, tok) in spans {
        let (a, b) = (a as usize, (b as usize).min(line.len()));
        if a > at && line.is_char_boundary(at) && line.is_char_boundary(a) {
            out.push(run(a - at, crate::text::Tok::Plain));
            at = a;
        }
        if a >= at && a < b && line.is_char_boundary(a) && line.is_char_boundary(b) {
            out.push(run(b - a, tok));
            at = b;
        }
    }
    if at < line.len() {
        out.push(run(line.len() - at, crate::text::Tok::Plain));
    }
    out
}

/// A font file's family, as the text system will know it.
fn family_name(data: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let names = face.names();
    let find = |id: u16| names.into_iter().filter(|n| n.name_id == id && n.is_unicode()).find_map(|n| n.to_string());
    find(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| find(ttf_parser::name_id::FAMILY))
}

/// What the transport drives: a sound or a video.
trait Transport {
    fn toggle(&self);
    fn seek(&self, secs: f64);
    fn position(&self) -> f64;
    fn duration(&self) -> f64;
    fn playing(&self) -> bool;
    fn error(&self) -> Option<String>;
    fn rate(&self) -> f64;
}

impl Transport for audio::Player {
    fn toggle(&self) {
        audio::Player::toggle(self)
    }
    fn seek(&self, secs: f64) {
        audio::Player::seek(self, secs)
    }
    fn position(&self) -> f64 {
        self.state.position_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }
    fn duration(&self) -> f64 {
        self.state.duration_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }
    fn playing(&self) -> bool {
        self.state.playing.load(Ordering::Relaxed)
    }
    fn error(&self) -> Option<String> {
        self.state.error.lock().unwrap().clone()
    }
    fn rate(&self) -> f64 {
        audio::Player::rate(self)
    }
}

impl Transport for video::Video {
    fn toggle(&self) {
        video::Video::toggle(self)
    }
    fn seek(&self, secs: f64) {
        video::Video::seek(self, secs)
    }
    fn position(&self) -> f64 {
        video::Video::position(self)
    }
    fn duration(&self) -> f64 {
        video::Video::duration(self)
    }
    fn playing(&self) -> bool {
        video::Video::playing(self)
    }
    fn error(&self) -> Option<String> {
        video::Video::error(self)
    }
    fn rate(&self) -> f64 {
        video::Video::rate(self)
    }
}

/// A speed as said: "1×", "0.25×", "1.5×".
pub fn speed_label(rate: f64) -> String {
    let s = format!("{rate:.2}");
    format!("{}×", s.trim_end_matches('0').trim_end_matches('.'))
}

/// The speeds as menu items (commands "speed-1.5"), `current` checked:
/// the transport's speed button's menu, and the Viewer's View ▸ Playback
/// Speed.
pub fn speed_items(mut menu: olo_kit::Menu, current: f64) -> olo_kit::Menu {
    for r in PreviewView::SPEEDS {
        let label = if r == 1.0 { "1× (Normal)".to_owned() } else { speed_label(r) };
        menu = menu.item(label, format!("speed-{r}")).checked((current - r).abs() < 1e-6);
    }
    menu
}

/// The speed a command of [`speed_items`]' asks for.
pub fn speed_command(command: &str) -> Option<f64> {
    command.strip_prefix("speed-")?.parse().ok().filter(|r: &f64| (audio::MIN_RATE..=audio::MAX_RATE).contains(r))
}

/// `>` a speed faster (1), `<` slower (-1), as YouTube's keys: Shift
/// and the key only.
pub fn speed_key(k: &olo_kit::Keystroke) -> Option<i32> {
    let m = &k.modifiers;
    if m.control || m.alt || m.platform || m.function {
        return None;
    }
    match k.key.as_str() {
        ">" => Some(1),
        "<" => Some(-1),
        "." if m.shift => Some(1),
        "," if m.shift => Some(-1),
        _ => None,
    }
}

/// Where the transport's parts are, as drawn.
#[derive(Clone, Copy, Default)]
struct TransportAt {
    button: Bounds<Pixels>,
    bar: Bounds<Pixels>,
    /// Its speed's button (Playback Speed).
    speed: Bounds<Pixels>,
}

/// The speed button's width: room for the longest, "0.25×".
const SPEED_W: f32 = 44.0;

/// Play/pause, elapsed time, a seek bar and the time left, centred on
/// `y` between `x` and `right`.
#[allow(clippy::too_many_arguments)]
fn paint_transport(g: &mut Painter, x: Pixels, right: Pixels, y: Pixels, t: &dyn Transport, duration: Option<f64>, pressed: bool, trim: Option<(f64, f64)>, chapters: &[(f64, String)]) -> TransportAt {
    let p = theme::palette();
    let dim = style::disabled_text(&p);
    if let Some(e) = t.error() {
        let at = style::label(g, x, y, Align::Left, e.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
        agent::record(g, agent::Node::label(e, at));
        return TransportAt::default();
    }
    let br = style::snap_centred(g, point(x + px(13.0), y), 26.0, 22.0);
    let playing = t.playing();
    style::raised(g, br, &p);
    if pressed {
        style::sunken(g, br, &p, false);
    }
    let ink = style::c(p.control_text);
    let c = br.center();
    if playing {
        for dx in [-3.5, 2.5] {
            g.paint_quad(olo_kit::fill(style::snap_centred(g, point(c.x + px(dx), c.y), 3.0, 10.0), ink));
        }
    } else {
        let mut path = PathBuilder::fill();
        path.move_to(point(c.x - px(3.5), c.y - px(5.5)));
        path.line_to(point(c.x + px(5.5), c.y));
        path.line_to(point(c.x - px(3.5), c.y + px(5.5)));
        path.close();
        if let Ok(path) = path.build() {
            g.paint_path(path, ink);
        }
    }
    agent::record(g, agent::Node::button(if playing { "Pause" } else { "Play" }, br));
    // Its speed, at the right end: a button showing it, its menu the
    // speeds.
    let rate = t.rate();
    let sr = style::snap_centred(g, point(right - px(SPEED_W / 2.0), y), SPEED_W, 22.0);
    style::raised(g, sr, &p);
    let said = speed_label(rate);
    style::label(g, sr.center().x, y, Align::Center, said.clone(), 11.0, style::ui_font(FontWeight::NORMAL), ink);
    agent::record(g, agent::Node { role: "Button", name: Some("Playback Speed".into()), value: Some(said.into()), bounds: sr, enabled: true, clickable: true, ..Default::default() });
    let right = sr.left() - px(8.0);
    let pos = t.position();
    let dur = duration.unwrap_or_else(|| t.duration()).max(0.001);
    let font = style::mono_font(FontWeight::NORMAL);
    style::label(g, br.right() + px(8.0), y, Align::Left, media::format_duration(pos), 11.0, font.clone(), dim);
    style::label(g, right, y, Align::Right, format!("-{}", media::format_duration((dur - pos).max(0.0))), 11.0, font, dim);
    let bar = style::snap(g, Bounds::from_corners(point(br.right() + px(52.0), y - px(3.0)), point(right - px(52.0), y + px(3.0))));
    style::sunken(g, Bounds::from_corners(bar.origin - point(px(1.0), px(1.0)), bar.bottom_right() + point(px(1.0), px(1.0))), &p, false);
    g.paint_quad(olo_kit::fill(bar, style::c(p.document_background)));
    let frac = (pos / dur).clamp(0.0, 1.0) as f32;
    g.paint_quad(olo_kit::fill(Bounds::new(bar.origin, size(bar.size.width * frac, bar.size.height)), style::c(p.keyboard_navigation)));
    // Chapters: a tick where each starts, a little past the bar's edges
    // (agents have them in the Go menu, and the one playing in the
    // position's value).
    for (at, _) in chapters {
        let x = bar.left() + bar.size.width * (*at / dur).clamp(0.0, 1.0) as f32;
        g.paint_quad(olo_kit::fill(style::snap_centred(g, point(x, bar.center().y), 1.0, 12.0), style::c(p.control_text)));
    }
    // Trimming: what's cut dimmed, a handle at each end of what's kept.
    if let Some((from, to)) = trim {
        let at = |s: f64| bar.left() + bar.size.width * (s / dur).clamp(0.0, 1.0) as f32;
        let (a, b) = (at(from), at(to));
        let scrim = style::c(p.scrim);
        g.paint_quad(olo_kit::fill(Bounds::from_corners(bar.origin, point(a, bar.bottom())), scrim));
        g.paint_quad(olo_kit::fill(Bounds::from_corners(point(b, bar.top()), bar.bottom_right()), scrim));
        for (x, name, s) in [(a, "Trim Start", from), (b, "Trim End", to)] {
            let h = style::snap_centred(g, point(x, bar.center().y), 6.0, 18.0);
            g.paint_quad(olo_kit::fill(h, style::c(p.frame_focused[0])));
            g.paint_quad(olo_kit::fill(h.dilate(px(-1.0)), style::c(p.tab_focused)));
            agent::record(g, agent::Node { role: "Slider", name: Some(name.into()), value: Some(media::format_duration(s).into()), bounds: h, enabled: true, clickable: true, ..Default::default() });
        }
    }
    // (While trimming, the bar is the trim's: its two handles say it.)
    if trim.is_none() {
        agent::record(
            g,
            agent::Node {
                role: "Slider",
                name: Some("Position".into()),
                value: Some(match chapters.iter().rev().find(|(at, _)| *at <= pos + 0.05) {
                    Some((_, title)) => format!("{} — {title}", media::format_duration(pos)).into(),
                    None => media::format_duration(pos).into(),
                }),
                bounds: bar,
                enabled: true,
                clickable: true,
                ..Default::default()
            },
        );
    }
    TransportAt { button: br, bar, speed: sr }
}

impl PreviewView {
    fn media_view(&mut self, cx: &mut Cx<Self>) -> Element {
        let md = match &self.body {
            Some(Body::Media(md)) => md.clone(),
            _ => return div().into_any_element(),
        };
        let s = cx.scale_factor();
        self.fullscreen = cx.is_fullscreen();
        // The video's next frame, and when the one after is due.
        let mut next_due = None;
        if let Some(v) = &self.video {
            // What decodes it, once the worker says (or after a change).
            let decoding = Some(v.decoding()).filter(|d| !d.is_empty());
            if decoding != self.decoding_said {
                self.decoding_said = decoding;
                cx.emit(PreviewEvent::Decoding);
            }
            let (frame, next) = v.tick_picture();
            if v.is_hdr() != self.video_hdr {
                self.video_hdr = !self.video_hdr;
                cx.emit(PreviewEvent::Gamut);
            }
            // As decoded: the GPU makes RGB of it (no picture made here).
            let frame = match frame {
                Some(video::Picture::Yuv(f)) => {
                    let was = if self.yuv_frame.is_some() { Some(olo_kit::Gamut::Srgb) } else { self.frame.as_ref().map(|f| f.gamut()) };
                    self.frame = None;
                    self.yuv_frame = Some(f);
                    if was != Some(olo_kit::Gamut::Srgb) {
                        cx.emit(PreviewEvent::Gamut);
                    }
                    None
                }
                other => other,
            };
            if let Some(pic) = frame {
                self.yuv_frame = None;
                let gamut = if pic.p3() { olo_kit::Gamut::DisplayP3 } else { olo_kit::Gamut::Srgb };
                let img = match pic {
                    video::Picture::Bgra { size: [w, h], data, .. } => {
                        image::RgbaImage::from_raw(w as u32, h as u32, (*data).clone()).map(|img| RenderImage::new([image::Frame::new(img)]).with_gamut(gamut))
                    }
                    video::Picture::Rgba(b) => {
                        image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.to_bgra_unmultiplied()).map(|img| RenderImage::new([image::Frame::new(img)]).with_gamut(gamut))
                    }
                    // (HDR10's, for a window drawing in it.)
                    video::Picture::Deep { size: [w, h], data } => Some(RenderImage::deep(w as u32, h as u32, &data, olo_kit::Gamut::Hdr10)),
                    video::Picture::Yuv(_) => None,
                };
                if let Some(img) = img {
                    let was = self.frame.as_ref().map(|f| f.gamut());
                    let now = img.gamut();
                    self.frame = Some(Arc::new(img));
                    if was != Some(now) {
                        cx.emit(PreviewEvent::Gamut);
                    }
                }
            }
            next_due = next;
            cx.emit(PreviewEvent::Played);
        }
        if let Some(d) = next_due {
            self.tick_in(d, cx);
        }
        // Stopped (played to its end) or started as the clock ticked here:
        // whoever shows it is told (its window keeps the screen awake
        // while it plays).
        if self.playing_video() != self.video_played {
            self.video_played = !self.video_played;
            cx.emit(PreviewEvent::Playing);
        }
        // (A sound still opening is drawn again too: it says it's playing
        // only once its first sound is sent, after this was drawn.)
        let playing = self.player.as_ref().is_some_and(|p| Transport::playing(p) || p.opening()) || self.video.as_ref().is_some_and(Transport::playing);
        if playing && self.video.is_none() {
            self.tick_in(Duration::from_millis(100), cx);
        }
        // Cover art (audio) at its device size.
        let art = match (&md.picture, md.video) {
            (Some(pic), false) => {
                let side = ART * s;
                let want = (side.round() as u32, (side * pic.size[1] as f32 / pic.size[0].max(1) as f32).round() as u32);
                let pic = pic.clone();
                self.art(&pic, want, cx)
            }
            _ => None,
        };
        let paint = cx.canvas(move |v: &PreviewView, g, r| v.transport_at.set(v.paint_media(r, &md, art.as_ref(), g))).size_full();
        div()
            .id("ql-media")
            .size_full()
            .child(paint)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v: &mut PreviewView, e: &MouseDownEvent, cx| {
                    let t = v.transport_at.get();
                    // Its speed: the speeds, the one playing over the
                    // button (a menu field's way).
                    if t.speed.contains(&e.position) {
                        let current = PreviewView::SPEEDS.iter().position(|&r| (r - v.rate).abs() < 1e-6).unwrap_or(3);
                        let me = cx.view_ref();
                        olo_kit::menu::popup_over(cx, speed_items(olo_kit::Menu::new("Playback Speed"), v.rate), t.speed, current, move |command, cx| {
                            if let Some(rate) = speed_command(command) {
                                me.update(cx, move |v, cx| v.set_playback_rate(rate, cx));
                            }
                        });
                        cx.stop_propagation();
                        return;
                    }
                    let transport: Option<&dyn Transport> = match (&v.video, &v.player) {
                        (Some(vid), _) => Some(vid),
                        (None, Some(p)) => Some(p),
                        _ => None,
                    };
                    let Some(transport) = transport else { return };
                    // Trimming: a press on (or by) a handle takes it.
                    if let Some((from, to)) = v.trim {
                        let dur = v.body.as_ref().and_then(|b| if let Body::Media(md) = b { md.duration } else { None }).unwrap_or_else(|| transport.duration()).max(0.001);
                        let x = |s: f64| t.bar.left() + t.bar.size.width * (s / dur).clamp(0.0, 1.0) as f32;
                        let near = |s: f64| (e.position.x - x(s)).abs() < px(8.0) && (e.position.y - t.bar.center().y).abs() < px(14.0);
                        if near(to) || near(from) {
                            v.trim_drag = Some(near(to) && (!near(from) || (e.position.x - x(to)).abs() < (e.position.x - x(from)).abs()));
                            cx.invalidate();
                            return;
                        }
                    }
                    if t.button.contains(&e.position) {
                        transport.toggle();
                    } else if Bounds::from_corners(t.bar.origin - point(px(0.0), px(6.0)), t.bar.bottom_right() + point(px(0.0), px(6.0))).contains(&e.position) {
                        let f = (f32::from(e.position.x - t.bar.left()) / f32::from(t.bar.size.width).max(1.0)).clamp(0.0, 1.0) as f64;
                        let dur = v.body.as_ref().and_then(|b| if let Body::Media(md) = b { md.duration } else { None }).unwrap_or_else(|| transport.duration());
                        transport.seek(f * dur);
                    } else if v.video.is_some() && (t.button.size.width == px(0.0) || e.position.y < t.button.top()) {
                        // A click on the picture plays or pauses (full
                        // screen, with the transport hidden, anywhere).
                        transport.toggle();
                    } else {
                        return;
                    }
                    cx.invalidate();
                }),
            )
            .on_mouse_move(cx.listener(move |v: &mut PreviewView, e: &MouseMoveEvent, cx| {
                // Full screen, a video's transport shows as the pointer moves.
                if cx.is_fullscreen() && v.video.is_some() {
                    v.show_controls(cx);
                }
                if e.pressed_button != Some(MouseButton::Left) {
                    v.trim_drag = None;
                    return;
                }
                let t = v.transport_at.get();
                // A handle dragged: that end of what's kept, the picture
                // there shown.
                if let (Some(end), Some((from, to)), Some(vid)) = (v.trim_drag, v.trim, &v.video) {
                    let dur = Transport::duration(vid).max(0.001);
                    let f = (f32::from(e.position.x - t.bar.left()) / f32::from(t.bar.size.width).max(1.0)).clamp(0.0, 1.0) as f64;
                    let s = f * dur;
                    v.trim = Some(if end { (from, s.max(from + 0.1)) } else { (s.min(to - 0.1).max(0.0), to) });
                    Transport::seek(vid, if end { s.max(from + 0.1) } else { s.min(to - 0.1).max(0.0) });
                    cx.invalidate();
                    return;
                }
                if t.bar.size.width > px(0.0) && t.bar.left() <= e.position.x && e.position.x <= t.bar.right() && (e.position.y - t.bar.center().y).abs() < px(12.0) {
                    let transport: Option<&dyn Transport> = match (&v.video, &v.player) {
                        (Some(vid), _) => Some(vid),
                        (None, Some(p)) => Some(p),
                        _ => None,
                    };
                    if let Some(transport) = transport {
                        let f = (f32::from(e.position.x - t.bar.left()) / f32::from(t.bar.size.width).max(1.0)).clamp(0.0, 1.0) as f64;
                        transport.seek(f * transport.duration());
                        cx.invalidate();
                    }
                }
            }))
            .into_any_element()
    }
}

impl PreviewView {
    /// Draws sound or video in `r`: where its transport went.
    fn paint_media(&self, r: Bounds<Pixels>, md: &media::Media, art: Option<&Arc<RenderImage>>, g: &mut Painter) -> TransportAt {
        let p = theme::palette();
        g.paint_quad(olo_kit::fill(r, style::c(p.panel_background)));
        let ink = style::c(p.panel_text);
        let dim = style::disabled_text(&p);
        let mut placed = TransportAt::default();
        if md.video {
            // Full screen: the picture is all there is, the transport over
            // it while the pointer moves (or it's paused).
            let full = self.fullscreen;
            let info_h = if full { 0.0 } else { VIDEO_INFO + if self.video.is_some() { TRANSPORT } else { 0.0 } };
            let area = Bounds::from_corners(r.origin, point(r.right(), r.bottom() - px(info_h)));
            g.paint_quad(olo_kit::fill(area, olo_kit::black()));
            // The frame showing: as decoded (the GPU makes RGB of it), or
            // a picture; its size in pixels either way.
            enum Shown {
                Yuv(Arc<olo_kit::YuvFrame>),
                Image(Arc<RenderImage>),
            }
            let shown = match (&self.yuv_frame, &self.frame) {
                (Some(f), _) => Some((Shown::Yuv(f.clone()), (f.width, f.height))),
                (None, Some(img)) => {
                    let sz = img.size(0);
                    Some((Shown::Image(img.clone()), (sz.width.0 as u32, sz.height.0 as u32)))
                }
                (None, None) => None,
            };
            match shown {
                Some((shown, (pw, ph))) => {
                    let s = g.scale_factor();
                    let (iw, ih) = (pw as f32 / s, ph as f32 / s);
                    let (kw, kh) = (f32::from(area.size.width) / iw, f32::from(area.size.height) / ih);
                    let k = match self.video_zoom {
                        // At most a frame pixel a device pixel (full
                        // screen: as big as fits).
                        VideoZoom::Actual if !full => kw.min(kh).min(1.0),
                        VideoZoom::Actual | VideoZoom::Fit => kw.min(kh),
                        VideoZoom::Fill => kw.max(kh),
                    };
                    let ib = style::snap(g, Bounds::centered_at(area.center(), size(px(iw * k), px(ih * k))));
                    self.video_placed.set(Some((ib, area, (pw, ph))));
                    // (Filling, what overflows its place is cut off.)
                    g.with_content_mask(Some(olo_kit::ContentMask { bounds: area }), |g| match shown {
                        Shown::Yuv(f) => g.paint_yuv(ib, f),
                        Shown::Image(img) => {
                            let _ = g.paint_image(ib, olo_kit::Corners::default(), img, 0, false);
                        }
                    });
                    agent::record(
                        g,
                        agent::Node { role: "Image", name: Some(md.title.clone().into()), bounds: ib.intersect(&area), enabled: true, clickable: self.video.is_some(), ..Default::default() },
                    );
                }
                None => {
                    let b = style::snap_centred(g, area.center(), 96.0, 96.0);
                    icons::paint(g, b, "video");
                }
            }
            if full {
                if let Some(t) = &self.video
                    && self.controls_showing()
                {
                    let strip = Bounds::from_corners(point(r.left(), r.bottom() - px(TRANSPORT + 2.0 * m::SPACING)), r.bottom_right());
                    let p = theme::palette();
                    g.paint_quad(olo_kit::fill(strip, style::c(p.panel_background).opacity(0.92)));
                    placed = paint_transport(g, r.left() + px(m::WINDOW_INSET), r.right() - px(m::WINDOW_INSET), strip.center().y, t, md.duration, false, self.trim, &md.chapters);
                }
                return placed;
            }
            let mut y = area.bottom();
            if let Some(t) = &self.video {
                y += px(TRANSPORT / 2.0 + 2.0);
                placed = paint_transport(g, r.left() + px(m::WINDOW_INSET), r.right() - px(m::WINDOW_INSET), y, t, md.duration, false, self.trim, &md.chapters);
                y += px(TRANSPORT / 2.0 - 2.0);
            }
            y += px(m::SPACING + m::LABEL_ROW / 2.0);
            let w = r.size.width - px(2.0 * m::WINDOW_INSET);
            let at = style::label_fit(g, r.left() + px(m::WINDOW_INSET), y, Align::Left, md.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, w);
            agent::record(g, agent::Node::label(md.title.clone(), at));
            y += px(m::LABEL_ROW);
            let facts = md.rows.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join("  ·  ");
            let at = style::label_fit(g, r.left() + px(m::WINDOW_INSET), y, Align::Left, facts.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, w);
            agent::record(g, agent::Node::label(facts, at));
            return placed;
        }
        // Sound: the cover on the left, its title, facts and the
        // transport as one block beside it.
        let pad = px(m::WINDOW_INSET + 4.0);
        let inner = Bounds::from_corners(point(r.left() + pad, r.top() + pad), point(r.right() - pad, r.bottom() - pad));
        let side = f32::from(inner.size.height).min(ART);
        let ar = style::snap(g, Bounds::new(point(inner.left(), inner.center().y - px(side / 2.0)), size(px(side), px(side))));
        match art {
            Some(img) => {
                let sz = img.size(0);
                let s = g.scale_factor();
                let (w, h) = (sz.width.0 as f32 / s, sz.height.0 as f32 / s);
                let ib = style::snap(g, Bounds::new(ar.origin, size(px(w.min(side)), px(h.min(side)))));
                g.paint_quad(olo_kit::fill(Bounds::new(point(ib.left() + px(1.0), ib.top() + px(2.0)), ib.size), style::c(p.drop_shadow)));
                let _ = g.paint_image(ib, olo_kit::Corners::default(), img.clone(), 0, false);
                agent::record(g, agent::Node { role: "Image", name: Some("Cover".into()), bounds: ib, enabled: true, ..Default::default() });
            }
            None => {
                let b = style::snap_centred(g, ar.center(), side * 0.7, side * 0.7);
                icons::paint(g, b, "audio");
            }
        }
        let x = ar.right() + pad;
        let right = inner.right();
        let facts: Vec<&(String, String)> = md.rows.iter().filter(|(k, _)| k != "Length").collect();
        let block = 22.0 + if md.subtitle.is_empty() { 0.0 } else { m::LABEL_ROW } + facts.len() as f32 * m::LABEL_ROW + if self.player.is_some() { m::SPACING * 2.0 + 22.0 } else { 0.0 };
        let mut y = ar.center().y - px(block / 2.0 - 11.0);
        let w = right - x;
        let at = style::label_fit(g, x, y, Align::Left, md.title.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink, w);
        agent::record(g, agent::Node::label(md.title.clone(), at));
        y += px(22.0);
        if !md.subtitle.is_empty() {
            let at = style::label_fit(g, x, y, Align::Left, md.subtitle.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, w);
            agent::record(g, agent::Node::label(md.subtitle.clone(), at));
            y += px(m::LABEL_ROW);
        }
        for (k, val) in facts {
            let text = format!("{k}: {val}");
            let at = style::label_fit(g, x, y, Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, w);
            agent::record(g, agent::Node::label(text, at));
            y += px(m::LABEL_ROW);
        }
        if let Some(player) = &self.player {
            let ty = y - px(m::LABEL_ROW / 2.0) + px(m::SPACING * 2.0 + 11.0);
            placed = paint_transport(g, x, right, ty, player, md.duration, false, None, &md.chapters);
        }
        placed
    }
}
