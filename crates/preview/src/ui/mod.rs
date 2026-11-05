//! The previews drawn with Olo's GPUI (the Viewer's and Quick Look's):
//! what [`load`](crate::load) decodes, on its worker. Pictures are drawn exactly
//! (`olo_ui`'s `ImageView`: resampled to their device size, a texel a
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

use futures::StreamExt as _;
use olo_kit_base::{files, metrics as m};
use olo_ui::{
    agent,
    gpui::{
        self, AnyElement, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
        ParentElement as _, PathBuilder, Pixels, Render, RenderImage, ScrollHandle, SharedString, Styled as _, Task, TextRun, Window, canvas, div, point, px, size,
    },
    icons,
    style::{self, Align},
    theme,
    widgets::{Cell, Column, ColumnListView, ImageView, ListModel, ScrollView, image_view, image_view::Rgba},
};

use crate::{Bitmap, Body, Item, SLOW, Update, audio, input, load, media, video};

impl Rgba for Bitmap {
    fn size(&self) -> [usize; 2] {
        self.size
    }

    fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    fn gamut(&self) -> olo_ui::gpui::Gamut {
        if self.p3 { olo_ui::gpui::Gamut::DisplayP3 } else { olo_ui::gpui::Gamut::Srgb }
    }
}

/// A text row's height, and the room either side of the line numbers.
const TEXT_ROW: f32 = 16.0;
const GUTTER_PAD: f32 = 10.0;
/// Below a video: its title and facts, and the transport when it plays.
const VIDEO_INFO: f32 = 2.0 * m::LABEL_ROW + 2.0 * m::SPACING;
const TRANSPORT: f32 = 34.0;
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
}

/// A preview of a file, as Quick Look and the Viewer show one.
pub struct PreviewView {
    pub item: Item,
    pub generation: u64,
    body: Option<Body>,
    failed: Option<String>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    want: load::Want,
    image: Entity<ImageView>,
    player: Option<audio::Player>,
    video: Option<video::Video>,
    /// The video's frame showing, or its poster until one comes.
    frame: Option<Arc<RenderImage>>,
    /// Its video is known to be HDR (told as `PreviewEvent::Gamut`: its
    /// window may draw in HDR10).
    video_hdr: bool,
    /// The cover or poster as drawn (made for its device size).
    art: Option<((u32, u32), Arc<RenderImage>)>,
    art_making: Option<((u32, u32), Task<()>)>,
    /// Images to take out of the window's atlas when next painted.
    dropped: Vec<Arc<RenderImage>>,
    /// A timer that draws it again (a playing sound's transport, the
    /// video's next frame).
    tick: Option<Task<()>>,
    /// Whether the video played when last drawn (what keeps the screen
    /// awake follows it).
    video_played: bool,
    text_scroll: ScrollHandle,
    list: Entity<ListModel>,
    sheet: usize,
    table: Entity<table::TableView>,
    rich: Entity<rich::RichView>,
    /// The font family registered for a font preview.
    font: Option<SharedString>,
    /// What text and listings show, shared with the views that draw them
    /// (taken from the body as it comes, not copied each frame).
    text: Option<Arc<TextRows>>,
    items: Option<Arc<Vec<crate::archive::Item>>>,
    loading: Option<Task<()>>,
}

/// A text's lines (shared: drawn and told to agents each frame without
/// a copy) and their token classes.
type TextRows = (Vec<SharedString>, Vec<Vec<(u32, u32, crate::text::Tok)>>);

fn text_rows(t: &mut crate::text::Text) -> Arc<TextRows> {
    Arc::new((std::mem::take(&mut t.lines).into_iter().map(SharedString::from).collect(), std::mem::take(&mut t.spans)))
}

impl EventEmitter<PreviewEvent> for PreviewView {}

impl PreviewView {
    /// Start loading `item` (`zoomable`: pictures at full size, to zoom
    /// and pan, as the Viewer's; else fitted, as Quick Look's).
    pub fn new(item: Item, generation: u64, zoomable: bool, cx: &mut Context<Self>) -> PreviewView {
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
            frame: None,
            video_hdr: false,
            art: None,
            art_making: None,
            dropped: Vec::new(),
            tick: None,
            video_played: false,
            text_scroll: ScrollHandle::new(),
            list: ListModel::new(cx, false),
            sheet: 0,
            table: cx.new(table::TableView::new),
            rich: cx.new(|_| rich::RichView::default()),
            font: None,
            text: None,
            items: None,
            loading: None,
        };
        v.spawn(cx);
        v
    }

    fn spawn(&mut self, cx: &mut Context<Self>) {
        let (item, cancel, generation, want) = (self.item.clone(), self.cancel.clone(), self.generation, self.want);
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Update>();
        std::thread::Builder::new().name("quicklook".into()).spawn(move || load::load(&item, want, &cancel, &mut |u| drop(tx.unbounded_send(u)))).ok();
        self.loading = Some(cx.spawn(async move |view, cx| {
            while let Some(u) = rx.next().await {
                if view
                    .update(cx, |v, cx| {
                        if v.generation == generation {
                            v.apply(u, cx)
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
        // "Loading" shows only if it takes a while.
        let at = self.generation;
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(SLOW).await;
            let _ = view.update(cx, |v, cx| {
                if v.generation == at && v.body.is_none() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Show another item in the same window (what's shown stays until it
    /// comes, so stepping through a folder doesn't flash).
    pub fn retarget(&mut self, item: Item, generation: u64, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.item = item;
        self.generation = generation;
        self.failed = None;
        self.player = None;
        self.video = None;
        self.tick = None;
        self.sheet = 0;
        self.started = Instant::now();
        self.table.update(cx, |t, _| t.reset());
        self.spawn(cx);
    }

    fn apply(&mut self, u: Update, cx: &mut Context<Self>) {
        match u {
            Update::Body(mut b) => {
                self.forget_pictures();
                self.font = None;
                match &mut b {
                    Body::Text(t) => self.text = Some(text_rows(t)),
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
                    self.player = Some(audio::Player::start(&input::Input::file(&self.item.path)));
                }
                self.video = match &b {
                    // (Frames as GPUI takes them: BGRA.)
                    Body::Media(md) if md.playable && md.video => {
                        Some(video::Video::start_with(&input::Input::file(&self.item.path), md.duration.unwrap_or(0.0), video::Options { playing: true, picture: true, sound: true, bgra: true }))
                    }
                    _ => None,
                };
                // A video's poster shows until its first frame comes.
                if let Body::Media(md) = &b
                    && md.video
                    && let Some(pic) = md.picture.clone()
                {
                    let generation = self.generation;
                    cx.spawn(async move |view, cx| {
                        let made = cx
                            .background_spawn(async move {
                                image::RgbaImage::from_raw(pic.size[0] as u32, pic.size[1] as u32, pic.to_bgra_unmultiplied()).map(|img| Arc::new(RenderImage::new([image::Frame::new(img)])))
                            })
                            .await;
                        let _ = view.update(cx, |v, cx| {
                            if v.generation == generation && v.frame.is_none() {
                                v.frame = made;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
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
        cx.notify();
    }

    fn forget_pictures(&mut self) {
        self.dropped.extend(self.frame.take());
        self.dropped.extend(self.art.take().map(|(_, a)| a));
        self.art_making = None;
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.player = None;
        self.video = None;
        self.tick = None;
    }

    /// What is shown, once loaded.
    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }

    /// The picture's view (zoom, scale, where it's drawn, dragging out).
    pub fn image_view(&self) -> &Entity<ImageView> {
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
    /// Display P3 (the window may draw in it, `Window::set_gamut`), else
    /// sRGB.
    pub fn gamut(&self, cx: &App) -> olo_ui::gpui::Gamut {
        match &self.frame {
            Some(f) if self.video.is_some() => f.gamut(),
            _ => self.image.read(cx).gamut(),
        }
    }

    /// Whether a picture is shown (so zoom commands apply).
    pub fn is_image(&self) -> bool {
        self.failed.is_none() && matches!(self.body, Some(Body::Image { .. }))
    }

    /// Whether the arrow keys are free for the caller (nothing shown
    /// scrolls or has a selection to move).
    pub fn arrows_free(&self) -> bool {
        self.failed.is_some() || matches!(self.body, None | Some(Body::Image { .. } | Body::Media(_) | Body::Font { .. } | Body::Card { .. }))
    }

    /// Show an edited picture in place of the one loaded (see
    /// [`edit`](crate::edit)).
    pub fn show_edited(&mut self, image: load::Image, full: [f32; 2], note: String, cx: &mut Context<Self>) {
        let picture = image.clone();
        self.image.update(cx, |v, cx| v.set_picture(picture, full, cx));
        self.body = Some(Body::Image { image, full, note });
        self.failed = None;
        cx.notify();
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

    /// Toggle sound or video playback (Space or P in a viewer).
    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) -> bool {
        let done = if let Some(v) = &self.video {
            v.toggle();
            true
        } else if let Some(p) = &self.player {
            p.toggle();
            true
        } else {
            false
        };
        cx.notify();
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
    fn art(&mut self, picture: &load::Image, want: (u32, u32), cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some((at, img)) = &self.art
            && *at == want
        {
            return Some(img.clone());
        }
        if self.art_making.as_ref().is_none_or(|(at, _)| *at != want) {
            let src = picture.clone();
            let task = cx.spawn(async move |view, cx| {
                let made = cx.background_spawn(async move { image_view::resampled(&*src, want) }).await;
                let _ = view.update(cx, |v, cx| {
                    if let Some(img) = made {
                        if let Some((_, old)) = v.art.replace((want, img)) {
                            v.dropped.push(old);
                        }
                    }
                    v.art_making = None;
                    cx.notify();
                });
            });
            self.art_making = Some((want, task));
        }
        // The last one, at whatever size, meanwhile.
        self.art.as_ref().map(|(_, a)| a.clone())
    }

    /// Draw it again in `d` (a playing transport, a video's next frame).
    fn tick_in(&mut self, d: Duration, cx: &mut Context<Self>) {
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(d).await;
            let _ = view.update(cx, |v, cx| {
                v.tick = None;
                cx.notify();
            });
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
fn card(window: &mut Window, cx: &mut App, r: Bounds<Pixels>, item: &Item, error: Option<&str>, spinning: bool) {
    let p = theme::palette(cx);
    window.paint_quad(gpui::fill(r, style::c(p.panel_background)));
    let icon = 96.0;
    let top = f32::from(r.center().y) - (icon + 3.0 * m::LABEL_ROW) / 2.0;
    let ib = style::snap_centred(window, point(r.center().x, px(top + icon / 2.0)), icon, icon);
    icons::paint(window, ib, &item.icon);
    let mut y = px(top + icon + m::SPACING + m::LABEL_ROW / 2.0);
    let name = style::label_fit(
        window,
        cx,
        r.center().x,
        y,
        Align::Center,
        item.name.clone(),
        style::PLAIN_SIZE,
        style::ui_font(FontWeight::BOLD),
        style::c(p.panel_text),
        r.size.width - px(2.0 * m::WINDOW_INSET),
    );
    agent::record(window, cx, agent::Node::label(item.name.clone(), name));
    y += px(m::LABEL_ROW);
    let facts = if item.is_dir { files::format_time(item.modified) } else { format!("{} — {}", files::format_size(item.size), files::format_time(item.modified)) };
    let dim = style::disabled_text(&p);
    let at = style::label(window, cx, r.center().x, y, Align::Center, facts.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
    agent::record(window, cx, agent::Node::label(facts, at));
    if let Some(e) = error {
        y += px(m::LABEL_ROW);
        let at = style::label_fit(window, cx, r.center().x, y, Align::Center, e.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, r.size.width - px(2.0 * m::WINDOW_INSET));
        agent::record(window, cx, agent::Node::label(e.to_owned(), at));
    }
    if spinning {
        style::spinner(window, point(r.center().x, r.bottom() - px(40.0)), 9.0, dim);
        window.request_animation_frame();
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let content: AnyElement = match (&self.failed, &self.body) {
            (Some(e), _) => {
                let (item, e) = (self.item.clone(), e.clone());
                canvas(|_, _, _| {}, move |b, _, window, cx| card(window, cx, b, &item, Some(&e), false)).size_full().into_any_element()
            }
            (None, None) => {
                let (item, spinning) = (self.item.clone(), self.started.elapsed() > SLOW);
                canvas(|_, _, _| {}, move |b, _, window, cx| card(window, cx, b, &item, None, spinning)).size_full().into_any_element()
            }
            (None, Some(Body::Image { .. })) => self.image.clone().into_any_element(),
            (None, Some(Body::Card { .. } | Body::Pages { .. })) => {
                let item = self.item.clone();
                canvas(|_, _, _| {}, move |b, _, window, cx| card(window, cx, b, &item, None, false)).size_full().into_any_element()
            }
            (None, Some(Body::Text(_))) => self.text_view(cx),
            (None, Some(Body::Listing { .. })) => self.listing(cx),
            (None, Some(Body::Font { .. })) => self.font_view(window, cx),
            (None, Some(Body::Media(_))) => self.media_view(window, cx),
            (None, Some(Body::Rich { .. })) => self.rich.clone().into_any_element(),
            (None, Some(Body::Table { .. })) => self.table_view(cx),
        };
        let dropped = std::mem::take(&mut self.dropped);
        let _ = me;
        div().size_full().relative().child(content).child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    for img in dropped {
                        let _ = window.drop_image(img);
                    }
                },
            )
            .absolute()
            .size_0(),
        )
    }
}

impl PreviewView {
    fn text_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(lines) = self.text.clone() else { return div().into_any_element() };
        let p = theme::palette(cx);
        let digits = lines.0.len().max(1).to_string().len();
        let longest = lines.0.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        // The face's advance: one width for every character.
        let char_w = style::text_width_app(cx, "0000000000", style::MONO_SIZE, &style::mono_font(FontWeight::NORMAL)) / 10.0;
        let gutter = (digits as f32 * char_w + 2.0 * GUTTER_PAD).ceil();
        let content_w = gutter + longest as f32 * char_w + 2.0 * GUTTER_PAD;
        let rows = lines.0.len();
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let view = window.content_mask().bounds;
                let font = style::mono_font(FontWeight::NORMAL);
                let gutter_bg = style::shade(&p, p.document_background, 1.04);
                let dim = style::disabled_text(&p);
                window.paint_quad(gpui::fill(Bounds::new(b.origin, size(px(gutter), b.size.height)), gutter_bg));
                let first = ((f32::from(view.top() - b.top()) / TEXT_ROW).floor().max(0.0)) as usize;
                let last = (((f32::from(view.bottom() - b.top())) / TEXT_ROW).ceil().max(0.0) as usize).min(rows);
                let (lines, spans) = &*lines;
                // The rows' glyphs in one place in the scene's order, not a
                // place found for each (half the time of drawing them).
                window.paint_layer(view, |window| {
                    for i in first..last {
                        let cy = b.top() + px(i as f32 * TEXT_ROW + TEXT_ROW / 2.0);
                        style::label(window, cx, b.left() + px(gutter - GUTTER_PAD), cy, Align::Right, (i + 1).to_string(), style::MONO_SIZE, font.clone(), dim);
                        let line = &lines[i];
                        if line.is_empty() {
                            continue;
                        }
                        let runs = code_runs(line, spans.get(i).map_or(&[][..], Vec::as_slice), &p, &font);
                        let at = style::label_runs(window, cx, b.left() + px(gutter + GUTTER_PAD), cy, Align::Left, line.clone(), style::MONO_SIZE, &runs);
                        agent::record(window, cx, agent::Node::label(line.clone(), at));
                    }
                });
            },
        )
        .w(px(content_w))
        .h(px(rows as f32 * TEXT_ROW));
        let scroller = div().id("ql-text-rows").min_w_full().w(px(content_w)).h(px(rows as f32 * TEXT_ROW)).child(paint);
        div().size_full().child(ScrollView::new("ql-text", &self.text_scroll).border(false).horizontal(true).background(p.document_background).child(scroller)).into_any_element()
    }

    fn listing(&mut self, _: &mut Context<Self>) -> AnyElement {
        let Some(items) = self.items.clone() else { return div().into_any_element() };
        let cols = vec![Column::new("Name", 360.0), Column::new("Size", 90.0).right()];
        let n = items.len();
        ColumnListView::new(format!("ql-list-{}", self.generation), &self.list, cols, n, move |cell: &mut Cell, window, cx| {
            let it = &items[cell.row];
            match cell.column {
                0 => {
                    let ib = style::snap_centred(window, point(cell.bounds.left() + px(12.0), cell.bounds.center().y), 16.0, 16.0);
                    let kind = files::kind_for(Path::new(&it.name), it.is_dir, 0o644);
                    icons::paint(window, ib, kind.icon());
                    let full = cell.bounds;
                    cell.bounds = Bounds::from_corners(point(full.left() + px(22.0), full.top()), full.bottom_right());
                    cell.text(window, cx, it.name.clone());
                    cell.bounds = full;
                }
                _ => cell.text(window, cx, if it.is_dir { "—".to_owned() } else { files::format_size(it.size) }),
            }
        })
        .into_any_element()
    }

    fn font_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(Body::Font { data, .. }) = &self.body else { return div().into_any_element() };
        if self.font.is_none() {
            let family = family_name(data).unwrap_or_else(|| self.item.name.clone());
            let _ = window.text_system().add_fonts(vec![std::borrow::Cow::Owned(data.to_vec())]);
            self.font = Some(family.into());
        }
        let family = self.font.clone().expect("registered");
        let p = theme::palette(cx);
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
        let font = gpui::Font { family: family.clone(), ..style::ui_font(FontWeight::NORMAL) };
        for (size, text) in samples {
            col = col.child(div().font(font.clone()).text_size(px(size)).line_height(px((size * 1.3).ceil())).text_color(ink).child(text));
        }
        div().size_full().child(ScrollView::new("ql-font", &self.text_scroll).border(false).background(p.document_background).child(col)).into_any_element()
    }

    fn table_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Body::Table { sheets, .. }) = &self.body else { return div().into_any_element() };
        let names: Vec<String> = sheets.iter().map(|s| s.0.clone()).collect();
        let rows = sheets.get(self.sheet).map(|s| s.1.clone());
        let (generation, sheet) = (self.generation, self.sheet);
        if let Some(rows) = rows {
            self.table.update(cx, |t, cx| t.show(rows, (generation, sheet), cx));
        }
        let table = self.table.clone().into_any_element();
        if names.len() < 2 {
            return table;
        }
        // The sheets as tabs over the table (where each was drawn, for
        // the press that picks one).
        let me = cx.entity();
        let hits: std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>> = Default::default();
        let drawn = hits.clone();
        let tabs = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                drawn.borrow_mut().clear();
                let p = theme::palette(cx);
                window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
                let mut x = b.left() + px(m::SPACING);
                for (k, name) in names.iter().enumerate() {
                    let w = f32::from(style::text_width(window, name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 24.0;
                    let br = Bounds::new(point(x, b.top() + px(3.0)), size(px(w), px(20.0)));
                    drawn.borrow_mut().push(br);
                    let selected = me.read(cx).sheet == k;
                    if selected {
                        style::raised(window, br, &p);
                    }
                    let weight = if selected { FontWeight::BOLD } else { FontWeight::NORMAL };
                    style::label(window, cx, br.center().x, br.center().y, Align::Center, name.clone(), style::PLAIN_SIZE, style::ui_font(weight), style::c(p.panel_text));
                    agent::record(window, cx, agent::Node { role: "Tab", name: Some(name.clone().into()), bounds: br, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
                    x += px(w + m::TIGHT);
                }
            },
        )
        .size_full();
        let tab_bar = div().id("ql-sheets").h(px(26.0)).w_full().child(tabs).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                let picked = hits.borrow().iter().position(|b| b.contains(&e.position));
                if let Some(k) = picked
                    && v.sheet != k
                {
                    v.sheet = k;
                    v.table.update(cx, |t, _| t.reset());
                    cx.notify();
                }
            }),
        );
        div().size_full().flex().flex_col().child(tab_bar).child(div().flex_1().min_h_0().child(table)).into_any_element()
    }
}

/// A line of code's runs: its token classes' colours (byte ranges), the
/// rest the document's text colour.
fn code_runs(line: &str, spans: &[(u32, u32, crate::text::Tok)], p: &olo_theme::Palette, font: &gpui::Font) -> Vec<TextRun> {
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
}

/// Where the transport's parts are, as drawn.
#[derive(Clone, Copy, Default)]
struct TransportAt {
    button: Bounds<Pixels>,
    bar: Bounds<Pixels>,
}

/// Play/pause, elapsed time, a seek bar and the time left, centred on
/// `y` between `x` and `right`.
fn paint_transport(window: &mut Window, cx: &mut App, x: Pixels, right: Pixels, y: Pixels, t: &dyn Transport, duration: Option<f64>, pressed: bool) -> TransportAt {
    let p = theme::palette(cx);
    let dim = style::disabled_text(&p);
    if let Some(e) = t.error() {
        let at = style::label(window, cx, x, y, Align::Left, e.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim);
        agent::record(window, cx, agent::Node::label(e, at));
        return TransportAt::default();
    }
    let br = style::snap_centred(window, point(x + px(13.0), y), 26.0, 22.0);
    let playing = t.playing();
    style::raised(window, br, &p);
    if pressed {
        style::sunken(window, br, &p, false);
    }
    let ink = style::c(p.control_text);
    let c = br.center();
    if playing {
        for dx in [-3.5, 2.5] {
            window.paint_quad(gpui::fill(style::snap_centred(window, point(c.x + px(dx), c.y), 3.0, 10.0), ink));
        }
    } else {
        let mut path = PathBuilder::fill();
        path.move_to(point(c.x - px(3.5), c.y - px(5.5)));
        path.line_to(point(c.x + px(5.5), c.y));
        path.line_to(point(c.x - px(3.5), c.y + px(5.5)));
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, ink);
        }
    }
    agent::record(window, cx, agent::Node::button(if playing { "Pause" } else { "Play" }, br));
    let pos = t.position();
    let dur = duration.unwrap_or_else(|| t.duration()).max(0.001);
    let font = style::mono_font(FontWeight::NORMAL);
    style::label(window, cx, br.right() + px(8.0), y, Align::Left, media::format_duration(pos), 11.0, font.clone(), dim);
    style::label(window, cx, right, y, Align::Right, format!("-{}", media::format_duration((dur - pos).max(0.0))), 11.0, font, dim);
    let bar = style::snap(window, Bounds::from_corners(point(br.right() + px(52.0), y - px(3.0)), point(right - px(52.0), y + px(3.0))));
    style::sunken(window, Bounds::from_corners(bar.origin - point(px(1.0), px(1.0)), bar.bottom_right() + point(px(1.0), px(1.0))), &p, false);
    window.paint_quad(gpui::fill(bar, style::c(p.document_background)));
    let frac = (pos / dur).clamp(0.0, 1.0) as f32;
    window.paint_quad(gpui::fill(Bounds::new(bar.origin, size(bar.size.width * frac, bar.size.height)), style::c(p.keyboard_navigation)));
    agent::record(
        window,
        cx,
        agent::Node { role: "Slider", name: Some("Position".into()), value: Some(media::format_duration(pos).into()), bounds: bar, enabled: true, clickable: true, ..Default::default() },
    );
    TransportAt { button: br, bar }
}

impl PreviewView {
    fn media_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let md = match &self.body {
            Some(Body::Media(md)) => md.clone(),
            _ => return div().into_any_element(),
        };
        let s = window.scale_factor();
        // The video's next frame, and when the one after is due.
        let mut next_due = None;
        if let Some(v) = &self.video {
            let (frame, next) = v.tick_picture();
            if v.is_hdr() != self.video_hdr {
                self.video_hdr = !self.video_hdr;
                cx.emit(PreviewEvent::Gamut);
            }
            if let Some(pic) = frame {
                let gamut = if pic.p3() { olo_ui::gpui::Gamut::DisplayP3 } else { olo_ui::gpui::Gamut::Srgb };
                let img = match pic {
                    video::Picture::Bgra { size: [w, h], data, .. } => {
                        image::RgbaImage::from_raw(w as u32, h as u32, (*data).clone()).map(|img| RenderImage::new([image::Frame::new(img)]).with_gamut(gamut))
                    }
                    video::Picture::Rgba(b) => {
                        image::RgbaImage::from_raw(b.size[0] as u32, b.size[1] as u32, b.to_bgra_unmultiplied()).map(|img| RenderImage::new([image::Frame::new(img)]).with_gamut(gamut))
                    }
                    // (HDR10's, for a window drawing in it.)
                    video::Picture::Deep { size: [w, h], data } => Some(RenderImage::deep(w as u32, h as u32, &data, olo_ui::gpui::Gamut::Hdr10)),
                };
                if let Some(img) = img {
                    let was = self.frame.as_ref().map(|f| f.gamut());
                    let now = img.gamut();
                    self.dropped.extend(self.frame.replace(Arc::new(img)));
                    if was != Some(now) {
                        cx.emit(PreviewEvent::Gamut);
                    }
                }
            }
            next_due = next;
        }
        if let Some(d) = next_due {
            self.tick_in(d, cx);
        }
        // Stopped (played to its end) or started as the clock ticked here:
        // the window again, for what its owner drew before this.
        if self.playing_video() != self.video_played {
            self.video_played = !self.video_played;
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let playing = self.player.as_ref().is_some_and(Transport::playing) || self.video.as_ref().is_some_and(Transport::playing);
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
        let me = cx.entity();
        let at: std::rc::Rc<std::cell::Cell<TransportAt>> = Default::default();
        let drawn = at.clone();
        let paint = canvas(
            |_, _, _| {},
            move |r, _, window, cx| {
                me.update(cx, |v, cx| {
                    let t = v.paint_media(r, &md, art.as_ref(), window, cx);
                    drawn.set(t);
                })
            },
        )
        .size_full();
        let seek = at.clone();
        div()
            .id("ql-media")
            .size_full()
            .child(paint)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                    let t = at.get();
                    let transport: Option<&dyn Transport> = match (&v.video, &v.player) {
                        (Some(vid), _) => Some(vid),
                        (None, Some(p)) => Some(p),
                        _ => None,
                    };
                    let Some(transport) = transport else { return };
                    if t.button.contains(&e.position) {
                        transport.toggle();
                    } else if Bounds::from_corners(t.bar.origin - point(px(0.0), px(6.0)), t.bar.bottom_right() + point(px(0.0), px(6.0))).contains(&e.position) {
                        let f = (f32::from(e.position.x - t.bar.left()) / f32::from(t.bar.size.width).max(1.0)).clamp(0.0, 1.0) as f64;
                        let dur = v.body.as_ref().and_then(|b| if let Body::Media(md) = b { md.duration } else { None }).unwrap_or_else(|| transport.duration());
                        transport.seek(f * dur);
                    } else if v.video.is_some() && e.position.y < t.button.top() {
                        // A click on the picture plays or pauses.
                        transport.toggle();
                    } else {
                        return;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |v, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let t = seek.get();
                if t.bar.size.width > px(0.0) && t.bar.left() <= e.position.x && e.position.x <= t.bar.right() && (e.position.y - t.bar.center().y).abs() < px(12.0) {
                    let transport: Option<&dyn Transport> = match (&v.video, &v.player) {
                        (Some(vid), _) => Some(vid),
                        (None, Some(p)) => Some(p),
                        _ => None,
                    };
                    if let Some(transport) = transport {
                        let f = (f32::from(e.position.x - t.bar.left()) / f32::from(t.bar.size.width).max(1.0)).clamp(0.0, 1.0) as f64;
                        transport.seek(f * transport.duration());
                        cx.notify();
                    }
                }
            }))
            .into_any_element()
    }
}

impl PreviewView {
    /// Draws sound or video in `r`: where its transport went.
    fn paint_media(&mut self, r: Bounds<Pixels>, md: &media::Media, art: Option<&Arc<RenderImage>>, window: &mut Window, cx: &mut Context<Self>) -> TransportAt {
        let p = theme::palette(cx);
        window.paint_quad(gpui::fill(r, style::c(p.panel_background)));
        let ink = style::c(p.panel_text);
        let dim = style::disabled_text(&p);
        let mut placed = TransportAt::default();
        if md.video {
            let info_h = VIDEO_INFO + if self.video.is_some() { TRANSPORT } else { 0.0 };
            let area = Bounds::from_corners(r.origin, point(r.right(), r.bottom() - px(info_h)));
            window.paint_quad(gpui::fill(area, gpui::black()));
            match &self.frame {
                Some(img) => {
                    let sz = img.size(0);
                    let s = window.scale_factor();
                    let (iw, ih) = (sz.width.0 as f32 / s, sz.height.0 as f32 / s);
                    // At most a frame pixel a device pixel.
                    let k = (f32::from(area.size.width) / iw).min(f32::from(area.size.height) / ih).min(1.0);
                    let ib = style::snap(window, Bounds::centered_at(area.center(), size(px(iw * k), px(ih * k))));
                    let _ = window.paint_image(ib, gpui::Corners::default(), img.clone(), 0, false);
                    agent::record(window, cx, agent::Node { role: "Image", name: Some(md.title.clone().into()), bounds: ib, enabled: true, clickable: self.video.is_some(), ..Default::default() });
                }
                None => {
                    let b = style::snap_centred(window, area.center(), 96.0, 96.0);
                    icons::paint(window, b, "video");
                }
            }
            let mut y = area.bottom();
            if let Some(t) = &self.video {
                y += px(TRANSPORT / 2.0 + 2.0);
                placed = paint_transport(window, cx, r.left() + px(m::WINDOW_INSET), r.right() - px(m::WINDOW_INSET), y, t, md.duration, false);
                y += px(TRANSPORT / 2.0 - 2.0);
            }
            y += px(m::SPACING + m::LABEL_ROW / 2.0);
            let w = r.size.width - px(2.0 * m::WINDOW_INSET);
            let at = style::label_fit(window, cx, r.left() + px(m::WINDOW_INSET), y, Align::Left, md.title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), ink, w);
            agent::record(window, cx, agent::Node::label(md.title.clone(), at));
            y += px(m::LABEL_ROW);
            let facts = md.rows.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join("  ·  ");
            let at = style::label_fit(window, cx, r.left() + px(m::WINDOW_INSET), y, Align::Left, facts.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, w);
            agent::record(window, cx, agent::Node::label(facts, at));
            return placed;
        }
        // Sound: the cover on the left, its title, facts and the
        // transport as one block beside it.
        let pad = px(m::WINDOW_INSET + 4.0);
        let inner = Bounds::from_corners(point(r.left() + pad, r.top() + pad), point(r.right() - pad, r.bottom() - pad));
        let side = f32::from(inner.size.height).min(ART);
        let ar = style::snap(window, Bounds::new(point(inner.left(), inner.center().y - px(side / 2.0)), size(px(side), px(side))));
        match art {
            Some(img) => {
                let sz = img.size(0);
                let s = window.scale_factor();
                let (w, h) = (sz.width.0 as f32 / s, sz.height.0 as f32 / s);
                let ib = style::snap(window, Bounds::new(ar.origin, size(px(w.min(side)), px(h.min(side)))));
                window.paint_quad(gpui::fill(Bounds::new(point(ib.left() + px(1.0), ib.top() + px(2.0)), ib.size), style::c(p.drop_shadow)));
                let _ = window.paint_image(ib, gpui::Corners::default(), img.clone(), 0, false);
                agent::record(window, cx, agent::Node { role: "Image", name: Some("Cover".into()), bounds: ib, enabled: true, ..Default::default() });
            }
            None => {
                let b = style::snap_centred(window, ar.center(), side * 0.7, side * 0.7);
                icons::paint(window, b, "audio");
            }
        }
        let x = ar.right() + pad;
        let right = inner.right();
        let facts: Vec<&(String, String)> = md.rows.iter().filter(|(k, _)| k != "Length").collect();
        let block = 22.0 + if md.subtitle.is_empty() { 0.0 } else { m::LABEL_ROW } + facts.len() as f32 * m::LABEL_ROW + if self.player.is_some() { m::SPACING * 2.0 + 22.0 } else { 0.0 };
        let mut y = ar.center().y - px(block / 2.0 - 11.0);
        let w = right - x;
        let at = style::label_fit(window, cx, x, y, Align::Left, md.title.clone(), 16.0, style::ui_font(FontWeight::BOLD), ink, w);
        agent::record(window, cx, agent::Node::label(md.title.clone(), at));
        y += px(22.0);
        if !md.subtitle.is_empty() {
            let at = style::label_fit(window, cx, x, y, Align::Left, md.subtitle.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, w);
            agent::record(window, cx, agent::Node::label(md.subtitle.clone(), at));
            y += px(m::LABEL_ROW);
        }
        for (k, val) in facts {
            let text = format!("{k}: {val}");
            let at = style::label_fit(window, cx, x, y, Align::Left, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), dim, w);
            agent::record(window, cx, agent::Node::label(text, at));
            y += px(m::LABEL_ROW);
        }
        if let Some(player) = &self.player {
            let ty = y - px(m::LABEL_ROW / 2.0) + px(m::SPACING * 2.0 + 11.0);
            placed = paint_transport(window, cx, x, right, ty, player, md.duration, false);
        }
        placed
    }
}
