//! Editing a video (Edit ▸ Edit Video…, ⌘E; `olo-viewer --edit PATH`, the
//! Edit of Screenshot's card after a recording), as CleanShot X's editor:
//! under the playing video, a timeline (pictures along it, the sound's
//! waveform, the marks, the playhead) with a handle at each end of what's
//! kept, and a bar of what can be done: Cut a stretch dragged out on the
//! timeline, Add and Delete marks, Crop (a frame dragged over the
//! picture), Mute, Export… (MP4 or GIF, size, frame rate, quality: on a
//! worker, with its progress and Cancel) and Done. Nothing is changed
//! until it's exported, as "NAME (edited).mp4" beside it; every step can
//! be undone. While it plays, what's cut is skipped. The export is
//! `olo_video::export`, as `olo-screenshot --export` (an agent's).

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_preview::ui::PreviewView;
use olo_ui::{
    WindowOptions, agent,
    gpui::{
        self, AnyElement, App, AppContext as _, Bounds, Context, Entity, EntityId, FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement as _, Pixels, Render, RenderImage, Styled as _, Window, canvas, div, point, px, size,
    },
    metrics as m,
    style::{self, Align},
    theme,
    widgets::{
        Button, CheckBox, MenuField, StringView,
        buttons::{SMALL_BUTTON_HEIGHT, button_width},
    },
};

use crate::{Content, Doc, Msg, crop::CropFrame};

/// The timeline's rows (points): the marks' flags, the pictures, the
/// waveform.
const MARKS_H: f32 = 16.0;
const PICTURES_H: f32 = 40.0;
const WAVE_H: f32 = 26.0;
/// The whole timeline, with its margins.
pub const TIMELINE_H: f32 = m::TIGHT + MARKS_H + m::TIGHT + PICTURES_H + m::TIGHT + WAVE_H + m::TIGHT;
/// The bar under it.
pub const BAR_H: f32 = SMALL_BUTTON_HEIGHT + 2.0 * m::TIGHT;
/// Pictures made for the timeline, evenly along the video.
const PICTURES: usize = 48;
/// The waveform's peaks.
const PEAKS: usize = 1200;
/// How near a trim handle a press takes it (points).
const NEAR: f32 = 7.0;
/// The shortest stretch kept or cut (seconds).
const SHORTEST: f64 = 0.1;

/// What the edits are (a step of the history).
#[derive(Clone, Debug, PartialEq)]
pub struct Edits {
    /// The stretch kept, before the cuts.
    pub trim: (f64, f64),
    /// Stretches cut out (in order, apart).
    pub cuts: Vec<(f64, f64)>,
    /// What's kept of each picture (its pixels: x, y, w, h).
    pub crop: Option<[u32; 4]>,
    pub mute: bool,
    pub marks: Vec<(f64, String)>,
}

impl Edits {
    /// What's kept, in order.
    pub fn keep(&self) -> Vec<(f64, f64)> {
        let mut out = vec![self.trim];
        for &(ca, cb) in &self.cuts {
            out = out.into_iter().flat_map(|(a, b)| [(a, b.min(ca)), (a.max(cb), b)]).filter(|(a, b)| b - a > 1e-6).collect();
        }
        out
    }

    /// Where playing from `t` goes on: `t` if it's kept, else the start
    /// of the next stretch kept (None past the last).
    pub fn kept_from(&self, t: f64) -> Option<f64> {
        self.keep().into_iter().find_map(|(a, b)| {
            if t < a {
                Some(a)
            } else if t < b {
                Some(t)
            } else {
                None
            }
        })
    }

    /// A stretch cut out (joined with those it touches).
    pub fn cut(&mut self, (a, b): (f64, f64)) {
        let mut all = self.cuts.clone();
        all.push((a.min(b), a.max(b)));
        all.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut out: Vec<(f64, f64)> = Vec::new();
        for (a, b) in all {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        self.cuts = out;
    }
}

/// An export under way.
pub struct Exporting {
    pub progress: f32,
    pub cancel: Arc<AtomicBool>,
}

/// A press on the timeline being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    TrimStart,
    TrimEnd,
    /// A stretch dragged out from here (seconds).
    Select(f64),
}

pub struct VideoEdit {
    pub path: PathBuf,
    pub duration: f64,
    pub now: Edits,
    undo: Vec<(String, Edits)>,
    redo: Vec<(String, Edits)>,
    /// A stretch dragged out on the timeline (to cut).
    pub selection: Option<(f64, f64)>,
    /// The mark chosen (to delete).
    pub mark: Option<usize>,
    /// The crop frame shown over the picture, being chosen.
    pub cropping: Option<CropFrame>,
    /// A press on the picture while cropping, and whether it has moved.
    crop_press: Option<bool>,
    drag: Option<Drag>,
    /// The timeline's pictures (made for `pictures_scale`).
    pictures: Vec<Option<Arc<RenderImage>>>,
    pictures_scale: f32,
    dropped: Vec<Arc<RenderImage>>,
    peaks: Option<Vec<f32>>,
    /// The workers making the pictures and peaks, stopped when it's done.
    work: Arc<AtomicBool>,
    pub exporting: Option<Exporting>,
    /// What the last export made, or why not.
    pub note: Option<String>,
    /// The edits exported last (nothing to lose on Done if they're these).
    exported: Option<Edits>,
    /// Where the timeline's strip was drawn (window points).
    strip: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// The video seen playing last (to skip what's cut).
    _observe: Option<gpui::Subscription>,
}

impl VideoEdit {
    fn new(path: PathBuf, duration: f64) -> VideoEdit {
        let marks = olo_video::export::marks_of(&path);
        VideoEdit {
            path,
            duration,
            now: Edits { trim: (0.0, duration), cuts: Vec::new(), crop: None, mute: false, marks },
            undo: Vec::new(),
            redo: Vec::new(),
            selection: None,
            mark: None,
            cropping: None,
            crop_press: None,
            drag: None,
            pictures: vec![None; PICTURES],
            pictures_scale: 0.0,
            dropped: Vec::new(),
            peaks: None,
            work: Arc::new(AtomicBool::new(false)),
            exporting: None,
            note: None,
            exported: None,
            strip: Default::default(),
            _observe: None,
        }
    }

    /// The edits as they are now kept for Undo, the step named `name`.
    fn step(&mut self, name: &str) {
        self.undo.push((name.to_owned(), self.now.clone()));
        self.redo.clear();
        self.note = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Undo and Redo would take back, by name.
    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|(n, _)| n.as_str())
    }

    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|(n, _)| n.as_str())
    }

    fn undo(&mut self) {
        if let Some((name, e)) = self.undo.pop() {
            self.redo.push((name, std::mem::replace(&mut self.now, e)));
            self.selection = None;
            self.mark = None;
        }
    }

    fn redo(&mut self) {
        if let Some((name, e)) = self.redo.pop() {
            self.undo.push((name, std::mem::replace(&mut self.now, e)));
            self.selection = None;
            self.mark = None;
        }
    }

    /// Edited since it was last exported (or ever, if never).
    pub fn unsaved(&self) -> bool {
        !self.undo.is_empty() && self.exported.as_ref() != Some(&self.now)
    }

    /// The edit for `olo_video::export`.
    fn edit(&self) -> olo_video::export::Edit {
        olo_video::export::Edit { keep: self.now.keep(), crop: self.now.crop, mute: self.now.mute, marks: Some(self.now.marks.clone()) }
    }

    /// The pictures and the waveform made on workers, for `scale`.
    fn start_work(&mut self, id: EntityId, scale: f32, proxy: olo_ui::Proxy) {
        self.work.store(true, Ordering::Relaxed);
        self.work = Arc::new(AtomicBool::new(false));
        self.pictures_scale = scale;
        self.dropped.extend(self.pictures.iter_mut().filter_map(Option::take));
        let (path, stop, duration) = (self.path.clone(), self.work.clone(), self.duration);
        let pictures = proxy.clone();
        std::thread::spawn(move || {
            let Ok(video) = olo_video::read::Video::open(&path) else { return };
            let h = ((PICTURES_H - 4.0) * scale).round().max(1.0) as u32;
            let w = ((h as f32 * video.width as f32 / video.height.max(1) as f32).round() as u32).max(1);
            let times: Vec<f64> = (0..PICTURES).map(|k| (k as f64 + 0.5) / PICTURES as f64 * duration).collect();
            let _ = video.pictures_near(&times, w, h, &stop, &mut |k, rgba| {
                let mut bgra = rgba.to_vec();
                for p in bgra.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                if let Some(img) = image::RgbaImage::from_raw(w, h, bgra) {
                    pictures.send(Msg::EditPicture(id, k, scale, Arc::new(RenderImage::new([image::Frame::new(img)]))));
                }
                Ok(())
            });
        });
        if self.peaks.is_none() {
            let (path, stop) = (self.path.clone(), self.work.clone());
            std::thread::spawn(move || {
                if let Ok(p) = olo_video::export::peaks(&path, PEAKS, &stop) {
                    proxy.send(Msg::EditPeaks(id, p));
                }
            });
        }
    }

    /// The time at window x on the strip.
    fn time_at(&self, x: Pixels) -> f64 {
        let s = self.strip.get();
        (f64::from(f32::from(x - s.left()) / f32::from(s.size.width).max(1.0))).clamp(0.0, 1.0) * self.duration
    }

    /// Window x of time `t` on `strip`.
    fn x_of(strip: Bounds<Pixels>, duration: f64, t: f64) -> Pixels {
        strip.left() + strip.size.width * (t / duration.max(1e-6)).clamp(0.0, 1.0) as f32
    }
}

/// `n` "Mark N" titles' next.
fn next_mark_title(marks: &[(f64, String)]) -> String {
    let n = (1..).find(|n| !marks.iter().any(|(_, t)| *t == format!("Mark {n}"))).unwrap_or(1);
    format!("Mark {n}")
}

/// A name beside `src` for its edited copy ("NAME (edited).mp4", then
/// "NAME (edited 2).mp4"…), not taken.
fn edited_name(src: &Path, ext: &str) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    (1..)
        .map(|n| if n == 1 { src.with_file_name(format!("{stem} (edited).{ext}")) } else { src.with_file_name(format!("{stem} (edited {n}).{ext}")) })
        .find(|p| !p.exists())
        .unwrap_or_else(|| src.with_extension(ext))
}

/// The export's choices, as the panel offers them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choices {
    pub format: usize,
    pub size: usize,
    pub fps: usize,
    pub quality: usize,
}

const FORMATS: [&str; 2] = ["MP4 Video", "Animated GIF"];
const SIZES: [(&str, f32); 4] = [("Full Size", 1.0), ("Three Quarters", 0.75), ("Half Size", 0.5), ("Quarter Size", 0.25)];
const RATES: [(&str, u32); 6] = [("As Recorded", 0), ("60 a Second", 60), ("30 a Second", 30), ("24 a Second", 24), ("15 a Second", 15), ("10 a Second", 10)];
const QUALITIES: [(&str, olo_video::Quality); 3] = [("High", olo_video::Quality::High), ("Medium", olo_video::Quality::Medium), ("Low", olo_video::Quality::Low)];

impl Default for Choices {
    fn default() -> Choices {
        Choices { format: 0, size: 0, fps: 0, quality: 0 }
    }
}

impl Doc {
    /// Edit Video…: the editor under the video (it must be one Olo can
    /// make again: an MP4 of H.264, HEVC or AV1).
    pub(crate) fn start_video_edit(&mut self, cx: &mut Context<Self>) {
        let Content::Other(prev) = &self.content else { return };
        let prev = prev.clone();
        if self.video_edit.is_some() || !prev.read(cx).is_video() {
            return;
        }
        if let Err(e) = olo_video::read::Video::open(&self.path) {
            if let Some(v) = self.viewer.upgrade() {
                v.update(cx, |v, cx| v.show_error(format!("Olo can’t edit this video.\n\n{e}"), cx));
            }
            return;
        }
        let duration = prev.read(cx).duration().unwrap_or(0.0);
        // (Its pictures and waveform are made as it's first drawn, for the
        // window's scale: `video_edit_area`.)
        let mut e = VideoEdit::new(self.path.clone(), duration);
        // While it plays, what's cut is skipped (the video's every frame
        // tells the window; only while editing).
        e._observe = Some(cx.observe(&prev, |d: &mut Doc, prev, cx| d.skip_cuts(&prev, cx)));
        self.video_edit = Some(e);
        prev.update(cx, |p, cx| p.set_trimming(false, cx));
        // The window taller by the editor (within its screen), the video
        // as big as it was.
        if let Some(win) = self.win {
            cx.defer(move |cx| {
                let _ = win.update(cx, |_, window, cx| {
                    let now = window.bounds().size;
                    let room = crate::screen(window, cx).map_or(f32::MAX, |(_, h)| h - 40.0);
                    let h = (f32::from(now.height) + TIMELINE_H + BAR_H).min(room);
                    let w = f32::from(now.width).max(640.0);
                    window.resize(size(px(w), px(h)));
                });
            });
        }
        cx.notify();
    }

    /// Playing into what's cut: on from where it's kept again (or stopped
    /// at the end of what's kept).
    fn skip_cuts(&mut self, prev: &Entity<PreviewView>, cx: &mut Context<Self>) {
        let Some(e) = &self.video_edit else { return };
        if !prev.read(cx).playing_video() || e.drag.is_some() {
            cx.notify();
            return;
        }
        let Some(now) = prev.read(cx).position() else { return };
        match e.now.kept_from(now) {
            Some(t) if t - now > 0.02 => prev.update(cx, |p, cx| p.seek_to(t, cx)),
            Some(_) => {}
            // (Paused, not stopped: stopping lets the video go.)
            None => prev.update(cx, |p, cx| {
                if p.playing_video() {
                    p.toggle_playback(cx);
                }
                p.seek_to(e.now.trim.0, cx);
            }),
        }
        cx.notify();
    }

    /// What the editor's workers made.
    pub(crate) fn video_edit_message(&mut self, m: Msg, cx: &mut Context<Self>) {
        let Some(e) = self.video_edit.as_mut() else { return };
        match m {
            Msg::EditPicture(_, k, scale, img) if scale == e.pictures_scale => {
                if let Some(slot) = e.pictures.get_mut(k) {
                    e.dropped.extend(slot.replace(img));
                }
            }
            Msg::EditPicture(_, _, _, img) => e.dropped.push(img),
            Msg::EditPeaks(_, p) => e.peaks = Some(p.unwrap_or_default()),
            Msg::ExportProgress(_, p) => {
                if let Some(x) = e.exporting.as_mut() {
                    x.progress = p;
                }
            }
            Msg::Exported(_, r, edits) => {
                e.exporting = None;
                e.note = Some(match r {
                    Ok(path) => {
                        e.exported = Some(edits);
                        format!("Exported “{}”.", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                    }
                    Err(err) if err == "cancelled" => "Export cancelled.".into(),
                    Err(err) => format!("The export failed: {err}"),
                });
            }
            _ => {}
        }
        cx.notify();
    }

    /// The editor's commands (its bar's, its menu's).
    pub(crate) fn video_edit_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let prev = match &self.content {
            Content::Other(p) => p.clone(),
            _ => return,
        };
        let now = prev.read(cx).position().unwrap_or(0.0);
        if command == "edit-video" {
            return self.start_video_edit(cx);
        }
        if command == "edit-done" {
            return self.finish_video_edit(false, cx);
        }
        let _ = window;
        let Some(e) = self.video_edit.as_mut() else { return };
        match command {
            "edit-undo" => e.undo(),
            "edit-redo" => e.redo(),
            "edit-cut" => {
                if let Some((a, b)) = e.selection.take().filter(|(a, b)| b - a >= SHORTEST) {
                    e.step("Cut");
                    e.now.cut((a, b));
                } else if let Some(k) = e.mark.take() {
                    e.step("Delete Mark");
                    e.now.marks.remove(k);
                }
            }
            "edit-add-mark" => {
                e.step("Add Mark");
                let title = next_mark_title(&e.now.marks);
                let at = e.now.marks.iter().position(|(t, _)| *t > now).unwrap_or(e.now.marks.len());
                e.now.marks.insert(at, (now, title));
                e.mark = Some(at);
            }
            "edit-delete-mark" => {
                if let Some(k) = e.mark.take().filter(|&k| k < e.now.marks.len()) {
                    e.step("Delete Mark");
                    e.now.marks.remove(k);
                }
            }
            "edit-crop" => match e.cropping.take() {
                // Done choosing: the frame is the crop.
                Some(frame) => {
                    if let Some(op) = frame.op()
                        && let olo_preview::edit::Op::Crop { x, y, w, h } = op
                        && w >= 16
                        && h >= 16
                    {
                        e.step("Crop");
                        e.now.crop = Some([x, y, w, h]);
                    }
                }
                None => {
                    let mut frame = CropFrame::default();
                    frame.sel = e.now.crop.map(|[x, y, w, h]| [x as f32, y as f32, (x + w) as f32, (y + h) as f32]);
                    e.cropping = Some(frame);
                    // (Paused, so the frame is chosen on a still picture.)
                    prev.update(cx, |p, cx| {
                        if p.playing_video() {
                            p.toggle_playback(cx);
                        }
                    });
                }
            },
            "edit-crop-reset" => {
                if e.now.crop.is_some() {
                    e.step("Crop");
                    e.now.crop = None;
                }
                e.cropping = None;
            }
            "edit-mute" => {
                e.step(if e.now.mute { "Unmute" } else { "Mute" });
                e.now.mute = !e.now.mute;
            }
            "edit-export" => {
                if e.exporting.is_none() {
                    let doc = cx.entity();
                    cx.defer(move |cx| {
                        olo_ui::open_window(cx, WindowOptions::new("Export Video", 340.0, 200.0).dialog(), move |_, cx| cx.new(|_| ExportPanel { doc, choices: Choices::default() }));
                    });
                }
            }
            "edit-export-cancel" => {
                if let Some(x) = &e.exporting {
                    x.cancel.store(true, Ordering::Relaxed);
                }
            }
            "edit-show" => {
                if let Some(path) = e.note.as_ref().and(Some(())).and_then(|_| last_export(&e.path)) {
                    let _ = olo_ui::tracker::open(&path);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// The export asked for, on a worker.
    fn export_video(&mut self, choices: Choices, cx: &mut Context<Self>) {
        let Some(e) = self.video_edit.as_mut() else { return };
        if e.exporting.is_some() {
            return;
        }
        let gif = choices.format == 1;
        let width = {
            let full = e.now.crop.map(|c| c[2]).unwrap_or_else(|| olo_video::read::Video::open(&e.path).map(|v| v.width).unwrap_or(0));
            let share = SIZES[choices.size.min(SIZES.len() - 1)].1;
            if share >= 1.0 { 0 } else { (full as f32 * share).round() as u32 }
        };
        let opts = olo_video::export::Options {
            format: if gif { olo_video::export::Format::Gif } else { olo_video::export::Format::Mp4 },
            width,
            fps: RATES[choices.fps.min(RATES.len() - 1)].1,
            quality: QUALITIES[choices.quality.min(QUALITIES.len() - 1)].1,
            encoder: olo_video::Choice::Auto,
            codec: olo_video::Codec::H264,
        };
        let dst = edited_name(&e.path, opts.format.extension());
        let cancel = Arc::new(AtomicBool::new(false));
        e.exporting = Some(Exporting { progress: 0.0, cancel: cancel.clone() });
        e.note = None;
        let (src, edit, edits, id, proxy) = (e.path.clone(), e.edit(), e.now.clone(), cx.entity_id(), olo_ui::proxy(cx));
        std::thread::spawn(move || {
            let last = std::cell::Cell::new(-1i32);
            let progress = |p: f32| {
                let pc = (p * 100.0) as i32;
                if pc != last.get() {
                    last.set(pc);
                    proxy.send(Msg::ExportProgress(id, p));
                }
            };
            let r = olo_video::export::export(&src, &dst, &edit, &opts, &cancel, &progress).map(|_| {
                remember_export(&src, &dst);
                dst
            });
            proxy.send(Msg::Exported(id, r, edits));
        });
        cx.notify();
    }

    /// Done: the editor put away (asked first if what was edited hasn't
    /// been exported: `force` once answered).
    pub(crate) fn finish_video_edit(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(e) = &self.video_edit else { return };
        if !force && e.unsaved() {
            let doc = cx.entity();
            let _ = olo_ui::widgets::open_alert(
                cx,
                "Viewer",
                "warning",
                "Stop editing without exporting? Your edits to this video haven’t been exported; the video itself is as it was.",
                &["Keep Editing", "Stop Editing"],
                move |choice, cx| {
                    if choice == Some(1) {
                        doc.update(cx, |d, cx| d.finish_video_edit(true, cx));
                    }
                },
            );
            return;
        }
        if let Some(e) = self.video_edit.take() {
            e.work.store(true, Ordering::Relaxed);
            if let Some(x) = &e.exporting {
                x.cancel.store(true, Ordering::Relaxed);
            }
        }
        cx.notify();
    }

    /// The editor under the video: the timeline and its bar.
    pub(crate) fn video_edit_area(&mut self, prev: &Entity<PreviewView>, window: &mut Window, cx: &mut Context<Self>) -> Option<(AnyElement, AnyElement, Option<AnyElement>)> {
        let scale = window.scale_factor();
        let id = cx.entity_id();
        let proxy = olo_ui::proxy(cx);
        let e = self.video_edit.as_mut()?;
        if e.pictures_scale != scale {
            e.start_work(id, scale, proxy);
        }
        let position = prev.read(cx).position().unwrap_or(0.0);
        let timeline = self.timeline(position, cx);
        let bar = self.edit_bar(window, cx);
        let crop = self.video_crop_overlay(prev, cx);
        Some((timeline, bar, crop))
    }

    fn timeline(&mut self, position: f64, cx: &mut Context<Self>) -> AnyElement {
        let e = self.video_edit.as_mut().expect("editing");
        let (duration, edits, selection, mark) = (e.duration, e.now.clone(), e.selection, e.mark);
        let pictures = e.pictures.clone();
        let peaks = e.peaks.clone();
        let dropped = std::mem::take(&mut e.dropped);
        let strip_cell = e.strip.clone();
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                for img in dropped {
                    let _ = window.drop_image(img);
                }
                let p = theme::palette(cx);
                style::fill_rect(window, b, style::c(p.panel_background));
                style::hline(window, b.left(), b.right(), b.top(), style::tint(p.panel_background, 1.2));
                let left = b.left() + px(m::WINDOW_INSET);
                let right = b.right() - px(m::WINDOW_INSET);
                let marks_top = b.top() + px(m::TIGHT);
                let pics = style::snap(window, Bounds::from_corners(point(left, marks_top + px(MARKS_H + m::TIGHT)), point(right, marks_top + px(MARKS_H + m::TIGHT + PICTURES_H))));
                let wave = style::snap(window, Bounds::from_corners(point(left, pics.bottom() + px(m::TIGHT)), point(right, pics.bottom() + px(m::TIGHT + WAVE_H))));
                let strip = Bounds::from_corners(pics.origin, wave.bottom_right());
                strip_cell.set(strip);
                let x_of = |t: f64| VideoEdit::x_of(strip, duration, t);
                // The pictures: each tile the one made nearest its middle,
                // one of its pixels a device pixel.
                window.paint_quad(gpui::fill(pics, gpui::black()));
                let inner = style::inset(window, pics, 2.0);
                window.with_content_mask(Some(gpui::ContentMask { bounds: inner }), |window| {
                    let tile = pictures.iter().flatten().next().map(|i| i.size(0));
                    if let Some(tile) = tile {
                        let s = window.scale_factor();
                        let tw = px(tile.width.0 as f32 / s);
                        let mut x = inner.left();
                        while x < inner.right() {
                            let mid = (f32::from(x + tw / 2.0 - inner.left()) / f32::from(inner.size.width).max(1.0)).clamp(0.0, 0.999);
                            let k = (mid * PICTURES as f32) as usize;
                            let near = (0..PICTURES).map(|d| [k.saturating_sub(d), (k + d).min(PICTURES - 1)]).flat_map(|[a, b]| [a, b]).find_map(|i| pictures[i].clone());
                            if let Some(img) = near {
                                let at = Bounds::new(point(x, inner.top()), size(tw, px(tile.height.0 as f32 / s)));
                                let _ = window.paint_image(at, gpui::Corners::default(), img, 0, false);
                            }
                            x += tw;
                        }
                    }
                });
                style::sunken(window, pics, &p, false);
                // The waveform: a bar a device pixel or two wide, from the
                // middle up and down.
                window.paint_quad(gpui::fill(wave, style::c(p.document_background)));
                style::sunken(window, wave, &p, false);
                if let Some(peaks) = peaks.as_ref().filter(|p| !p.is_empty()) {
                    let s = window.scale_factor();
                    let col = px(2.0 / s);
                    let mid = wave.center().y;
                    let half = f32::from(wave.size.height) / 2.0 - 2.0;
                    // (Muted: what won't be heard, greyed.)
                    let ink = if edits.mute { style::disabled_text(&p) } else { style::c(p.keyboard_navigation).opacity(0.85) };
                    let mut x = wave.left() + px(2.0);
                    while x < wave.right() - px(2.0) {
                        let f = f32::from(x - wave.left()) / f32::from(wave.size.width).max(1.0);
                        let v = peaks[((f * peaks.len() as f32) as usize).min(peaks.len() - 1)];
                        let h = (v.sqrt() * half).max(0.5 / s);
                        window.paint_quad(gpui::fill(Bounds::from_corners(point(x, mid - px(h)), point(x + col / 2.0, mid + px(h))), ink));
                        x += col;
                    }
                }
                agent::record(window, cx, agent::Node::label(if peaks.as_ref().is_some_and(|p| !p.is_empty()) { "Sound" } else { "No sound" }, wave));
                // What's not kept: dimmed, before the trim, after it and in
                // the cuts.
                let scrim = style::c(p.scrim);
                let gone = |window: &mut Window, a: f64, b: f64| {
                    let r = Bounds::from_corners(point(x_of(a), strip.top()), point(x_of(b), strip.bottom()));
                    if r.size.width > px(0.0) {
                        window.paint_quad(gpui::fill(r, scrim));
                    }
                };
                gone(window, 0.0, edits.trim.0);
                gone(window, edits.trim.1, duration);
                for &(a, b) in &edits.cuts {
                    gone(window, a, b);
                    let r = Bounds::from_corners(point(x_of(a), strip.top()), point(x_of(b), strip.bottom()));
                    agent::record(window, cx, agent::Node::label(format!("Cut {} to {}", olo_preview::media::format_duration(a), olo_preview::media::format_duration(b)), r));
                }
                // The stretch dragged out.
                if let Some((a, b)) = selection {
                    let r = Bounds::from_corners(point(x_of(a), strip.top()), point(x_of(b), strip.bottom()));
                    window.paint_quad(gpui::fill(r, style::c(p.list_selected_background).opacity(0.45)));
                    window.paint_quad(gpui::outline(r, style::c(p.selection_edge()), gpui::BorderStyle::Solid).border_widths(px(1.0)));
                    agent::record(window, cx, agent::Node::label(format!("Selected {} to {}", olo_preview::media::format_duration(a), olo_preview::media::format_duration(b)), r));
                }
                // The trim's handles.
                for (t, name) in [(edits.trim.0, "Trim Start"), (edits.trim.1, "Trim End")] {
                    let h = style::snap_centred(window, point(x_of(t), strip.center().y), 6.0, f32::from(strip.size.height));
                    window.paint_quad(gpui::fill(h, style::c(p.frame_focused[0])));
                    window.paint_quad(gpui::fill(h.dilate(px(-1.0)), style::c(p.tab_focused)));
                    agent::record(
                        window,
                        cx,
                        agent::Node {
                            role: "Slider",
                            name: Some(name.into()),
                            value: Some(olo_preview::media::format_duration(t).into()),
                            bounds: h,
                            enabled: true,
                            clickable: true,
                            ..Default::default()
                        },
                    );
                }
                // The marks: a flag each, its title beside it.
                let font = style::ui_font(FontWeight::NORMAL);
                for (k, (t, title)) in edits.marks.iter().enumerate() {
                    let x = x_of(*t);
                    let chosen = mark == Some(k);
                    let ink = if chosen { style::c(p.keyboard_navigation) } else { style::c(p.panel_text) };
                    // (Clear of the strip, its handles over it.)
                    let pole = style::snap_centred(window, point(x, marks_top + px(MARKS_H / 2.0)), 1.0, MARKS_H - 6.0);
                    window.paint_quad(gpui::fill(pole, ink));
                    let flag = Bounds::new(point(pole.right(), pole.top()), size(px(6.0), px(5.0)));
                    window.paint_quad(gpui::fill(style::snap(window, flag), ink));
                    // Its title beside it, or before it where it would
                    // pass the timeline's end.
                    let colour = if chosen { ink } else { style::disabled_text(&p) };
                    let wide = style::text_width(window, title.clone(), style::DETAIL_SIZE, font.clone());
                    let at = if x + px(9.0) + wide <= strip.right() {
                        style::label(window, cx, x + px(9.0), marks_top + px(MARKS_H / 2.0), Align::Left, title.clone(), style::DETAIL_SIZE, font.clone(), colour)
                    } else {
                        style::label(window, cx, x - px(m::TIGHT), marks_top + px(MARKS_H / 2.0), Align::Right, title.clone(), style::DETAIL_SIZE, font.clone(), colour)
                    };
                    agent::record(
                        window,
                        cx,
                        agent::Node {
                            role: "Button",
                            name: Some(format!("Mark: {title}").into()),
                            value: Some(olo_preview::media::format_duration(*t).into()),
                            bounds: at.union(&pole),
                            enabled: true,
                            clickable: true,
                            selected: Some(chosen),
                            ..Default::default()
                        },
                    );
                }
                // The playhead.
                let x = x_of(position);
                let line = style::snap_centred(window, point(x, strip.center().y), 2.0, f32::from(strip.size.height) + 6.0);
                window.paint_quad(gpui::fill(line, style::c(p.failure)));
                agent::record(
                    window,
                    cx,
                    // (Round the trim's handles too: they're in it.)
                    agent::Node {
                        role: "Slider",
                        name: Some("Timeline".into()),
                        value: Some(olo_preview::media::format_duration(position).into()),
                        bounds: Bounds::from_corners(strip.origin - point(px(5.0), px(0.0)), strip.bottom_right() + point(px(5.0), px(0.0))),
                        enabled: true,
                        clickable: true,
                        ..Default::default()
                    },
                );
            },
        )
        .size_full();
        let (down, moved, up) = (cx.entity(), cx.entity(), cx.entity());
        div()
            .id("video-timeline")
            .h(px(TIMELINE_H))
            .w_full()
            .flex_none()
            .child(paint)
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, cx| down.update(cx, |d, cx| d.timeline_press(ev, cx)))
            .on_mouse_move(move |ev: &MouseMoveEvent, _, cx| moved.update(cx, |d, cx| d.timeline_drag(ev, cx)))
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| up.update(cx, |d, cx| d.timeline_release(cx)))
            .into_any_element()
    }

    fn preview(&self) -> Option<Entity<PreviewView>> {
        match &self.content {
            Content::Other(p) => Some(p.clone()),
            _ => None,
        }
    }

    fn timeline_press(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(prev) = self.preview() else { return };
        let Some(e) = self.video_edit.as_mut() else { return };
        let strip = e.strip.get();
        let marks_row = Bounds::from_corners(point(strip.left() - px(60.0), strip.top() - px(MARKS_H + m::TIGHT)), point(strip.right() + px(60.0), strip.top() - px(m::TIGHT / 2.0)));
        let x_of = |t: f64| VideoEdit::x_of(strip, e.duration, t);
        // A mark's flag: chosen.
        if marks_row.contains(&ev.position) {
            let near = e
                .now
                .marks
                .iter()
                .enumerate()
                .filter(|(_, (t, _))| (ev.position.x - x_of(*t)).abs() <= px(48.0))
                .min_by(|a, b| (ev.position.x - x_of(a.1.0)).abs().partial_cmp(&(ev.position.x - x_of(b.1.0)).abs()).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((k, (t, _))) = near {
                let t = *t;
                e.mark = Some(k);
                e.selection = None;
                prev.update(cx, |p, cx| p.seek_to(t, cx));
            }
            cx.notify();
            return;
        }
        let tall = Bounds::from_corners(strip.origin - point(px(NEAR), px(4.0)), strip.bottom_right() + point(px(NEAR), px(4.0)));
        if !tall.contains(&ev.position) {
            return;
        }
        let (a, b) = (x_of(e.now.trim.0), x_of(e.now.trim.1));
        e.mark = None;
        e.drag = Some(if (ev.position.x - b).abs() <= px(NEAR) {
            e.step("Trim");
            Drag::TrimEnd
        } else if (ev.position.x - a).abs() <= px(NEAR) {
            e.step("Trim");
            Drag::TrimStart
        } else {
            Drag::Select(e.time_at(ev.position.x))
        });
        e.selection = None;
        let t = e.time_at(ev.position.x);
        prev.update(cx, |p, cx| p.seek_to(t, cx));
        cx.notify();
    }

    fn timeline_drag(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(prev) = self.preview() else { return };
        let Some(e) = self.video_edit.as_mut() else { return };
        let Some(drag) = e.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            e.drag = None;
            return;
        }
        let t = e.time_at(ev.position.x);
        let shown = match drag {
            Drag::TrimStart => {
                e.now.trim.0 = t.min(e.now.trim.1 - SHORTEST).max(0.0);
                e.now.trim.0
            }
            Drag::TrimEnd => {
                e.now.trim.1 = t.max(e.now.trim.0 + SHORTEST).min(e.duration);
                e.now.trim.1
            }
            Drag::Select(from) => {
                e.selection = Some((from.min(t), from.max(t))).filter(|(a, b)| b - a > 0.0);
                t
            }
        };
        prev.update(cx, |p, cx| p.seek_to(shown, cx));
        cx.notify();
    }

    fn timeline_release(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.video_edit.as_mut() else { return };
        if let Some(Drag::Select(_)) = e.drag {
            // (A click, not a drag: only where to play from.)
            if e.selection.is_some_and(|(a, b)| b - a < SHORTEST / 2.0) {
                e.selection = None;
            }
        }
        e.drag = None;
        cx.notify();
    }

    fn edit_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let e = self.video_edit.as_ref().expect("editing");
        let me = cx.entity();
        let cmd = move |c: &'static str| {
            let me = me.clone();
            move |window: &mut Window, cx: &mut App| me.update(cx, |d, cx| d.video_edit_command(c, window, cx))
        };
        let busy = e.exporting.is_some();
        let kept: f64 = e.now.keep().iter().map(|(a, b)| b - a).sum();
        let status: String = match (&e.exporting, &e.note) {
            (Some(x), _) => format!("Exporting… {}%", (x.progress * 100.0).round()),
            (None, Some(n)) => n.clone(),
            (None, None) => format!("{} kept of {}", olo_preview::media::format_duration(kept), olo_preview::media::format_duration(e.duration)),
        };
        let shown_export = e.note.as_ref().is_some_and(|n| n.starts_with("Exported")) && !busy;
        let mute = e.now.mute;
        let mute_me = cx.entity();
        let _ = window;
        // (A bar along the window's foot: its ground painted, so it sets
        // its own insets, as a tool bar's.)
        let ground = canvas(|_, _, _| {}, |b, _, window, cx| style::fill_rect(window, b, style::c(theme::palette(cx).panel_background))).absolute().inset_0();
        let mut row = div()
            .relative()
            .h(px(BAR_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(m::TIGHT))
            .px(px(m::WINDOW_INSET))
            .child(ground)
            .child(Button::new("Cut").small().enabled(!busy && (e.selection.is_some_and(|(a, b)| b - a >= SHORTEST) || e.mark.is_some())).on_click(cmd("edit-cut")))
            .child(Button::new("Add Mark").small().enabled(!busy).on_click(cmd("edit-add-mark")))
            .child(Button::new("Crop").small().pushed(e.cropping.is_some()).enabled(!busy).on_click(cmd("edit-crop")))
            .child(div().flex().items_center().child(
                CheckBox::new("Mute", mute).height(SMALL_BUTTON_HEIGHT).enabled(!busy).on_toggle(move |_, window, cx| mute_me.update(cx, |d, cx| d.video_edit_command("edit-mute", window, cx))),
            ))
            .child(div().flex_1().min_w_0().overflow_hidden().child(StringView::new(status).dim(!busy && e.note.is_none())));
        if shown_export {
            row = row.child(Button::new("Show in Tracker").small().on_click(cmd("edit-show")));
        }
        row = if busy {
            row.child(Button::new("Cancel").small().on_click(cmd("edit-export-cancel")))
        } else {
            row.child(Button::new("Export…").small().enabled(!e.now.keep().is_empty()).on_click(cmd("edit-export")))
        };
        row.child(Button::new("Done").small().default(true).on_click(cmd("edit-done"))).into_any_element()
    }

    /// While cropping: the frame over the video's picture; else, a crop
    /// chosen shown (what goes dimmed).
    fn video_crop_overlay(&mut self, prev: &Entity<PreviewView>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.video_edit.as_ref()?;
        let (shown, view, full) = prev.read(cx).video_picture()?;
        match e.cropping.clone() {
            Some(frame) => {
                let paint = canvas(
                    |b, window, _| window.insert_hitbox(b, gpui::HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        window.set_cursor_style(frame.shape(), &hitbox);
                        frame.paint(window, cx, view, shown, full);
                    },
                )
                .absolute()
                .inset_0();
                let (down, moved, up) = (cx.entity(), cx.entity(), cx.entity());
                Some(
                    div()
                        .id("video-crop")
                        .absolute()
                        .inset_0()
                        .child(paint)
                        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, cx| {
                            down.update(cx, |d, cx| {
                                if let Some(e) = d.video_edit.as_mut()
                                    && let Some(f) = e.cropping.as_mut()
                                {
                                    f.press(ev.position, shown, full);
                                    e.crop_press = Some(false);
                                    cx.notify();
                                }
                            })
                        })
                        .on_mouse_move(move |ev: &MouseMoveEvent, _, cx| {
                            moved.update(cx, |d, cx| {
                                let Some(e) = d.video_edit.as_mut() else { return };
                                let Some(f) = e.cropping.as_mut() else { return };
                                let changed = if ev.pressed_button == Some(MouseButton::Left) && e.crop_press.is_some() {
                                    e.crop_press = Some(true);
                                    f.drag_to(ev.position, shown, full)
                                } else {
                                    f.hover(ev.position, shown, full)
                                };
                                if changed {
                                    cx.notify();
                                }
                            })
                        })
                        .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                            up.update(cx, |d, cx| {
                                let Some(e) = d.video_edit.as_mut() else { return };
                                let moved = e.crop_press.take().unwrap_or(false);
                                if let Some(f) = e.cropping.as_mut() {
                                    f.release(moved);
                                }
                                cx.notify();
                            })
                        })
                        .into_any_element(),
                )
            }
            None => {
                let [x, y, w, h] = e.now.crop?;
                let paint = canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        let p = theme::palette(cx);
                        let k = f32::from(shown.size.width) / (full.0 as f32).max(1.0);
                        let r = Bounds::new(point(shown.left() + px(x as f32 * k), shown.top() + px(y as f32 * k)), size(px(w as f32 * k), px(h as f32 * k)));
                        let scrim = style::c(p.scrim);
                        for b in [
                            Bounds::from_corners(shown.origin, point(shown.right(), r.top())),
                            Bounds::from_corners(point(shown.left(), r.bottom()), shown.bottom_right()),
                            Bounds::from_corners(point(shown.left(), r.top()), point(r.left(), r.bottom())),
                            Bounds::from_corners(point(r.right(), r.top()), point(shown.right(), r.bottom())),
                        ] {
                            if b.size.width > px(0.0) && b.size.height > px(0.0) {
                                window.paint_quad(gpui::fill(b.intersect(&view), scrim));
                            }
                        }
                        agent::record(window, cx, agent::Node::label(format!("Cropped to {w} × {h}"), r));
                    },
                )
                .absolute()
                .inset_0();
                Some(paint.into_any_element())
            }
        }
    }
}

/// Where each video's last export went (this process), for Show in
/// Tracker.
static EXPORTS: std::sync::Mutex<Vec<(PathBuf, PathBuf)>> = std::sync::Mutex::new(Vec::new());

fn remember_export(src: &Path, dst: &Path) {
    let mut all = EXPORTS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(s, _)| s != src);
    all.push((src.to_path_buf(), dst.to_path_buf()));
}

fn last_export(src: &Path) -> Option<PathBuf> {
    EXPORTS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(s, _)| s == src).map(|(_, d)| d.clone())
}

/// Export Video: the choices, then Export (it runs under the video, in the
/// editor's bar).
struct ExportPanel {
    doc: Entity<Doc>,
    choices: Choices,
}

impl ExportPanel {
    fn done(&mut self, export: bool, window: &mut Window, cx: &mut Context<Self>) {
        if export {
            let choices = self.choices;
            self.doc.update(cx, |d, cx| d.export_video(choices, cx));
        }
        window.remove_window();
    }
}

impl Render for ExportPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.choices;
        let me = cx.entity();
        let pick = |id: &'static str, label: &'static str, options: Vec<&'static str>, selected: usize, set: fn(&mut Choices, usize)| {
            let me = me.clone();
            MenuField::new(id, options.into_iter().map(Into::into).collect(), selected).label(label).divider(90.0).width(300.0).on_change(move |i, _, cx| {
                me.update(cx, |p, cx| {
                    set(&mut p.choices, i);
                    cx.notify();
                })
            })
        };
        let gif = c.format == 1;
        let (ok, cancel) = (cx.entity(), cx.entity());
        let bw = button_width(window, "Export", false).max(button_width(window, "Cancel", false));
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|p, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    p.done(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(pick("format", "Format:", FORMATS.to_vec(), c.format, |c, i| c.format = i))
            .child(pick("size", "Size:", SIZES.iter().map(|s| s.0).collect(), c.size, |c, i| c.size = i))
            .child(pick("rate", "Frame rate:", RATES.iter().map(|r| if gif && r.1 == 0 { "15 a Second" } else { r.0 }).collect(), c.fps, |c, i| c.fps = i))
            .child(pick("quality", "Quality:", QUALITIES.iter().map(|q| q.0).collect(), c.quality, |c, i| c.quality = i).enabled(!gif))
            .child(
                div()
                    .pt(px(m::TIGHT))
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").min_width(bw).on_click(move |window, cx| cancel.update(cx, |p, cx| p.done(false, window, cx))))
                    .child(Button::new("Export").default(true).min_width(bw).on_click(move |window, cx| ok.update(cx, |p, cx| p.done(true, window, cx)))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits() -> Edits {
        Edits { trim: (1.0, 9.0), cuts: Vec::new(), crop: None, mute: false, marks: Vec::new() }
    }

    #[test]
    fn what_is_kept_is_the_trim_less_the_cuts() {
        let mut e = edits();
        e.cut((3.0, 4.0));
        e.cut((6.0, 7.5));
        // Touching and overlapping cuts join.
        e.cut((7.0, 8.0));
        assert_eq!(e.cuts, vec![(3.0, 4.0), (6.0, 8.0)]);
        assert_eq!(e.keep(), vec![(1.0, 3.0), (4.0, 6.0), (8.0, 9.0)]);
        // Playing goes on past each cut, and stops past the trim.
        assert_eq!(e.kept_from(0.2), Some(1.0));
        assert_eq!(e.kept_from(3.5), Some(4.0));
        assert_eq!(e.kept_from(5.0), Some(5.0));
        assert_eq!(e.kept_from(9.5), None);
    }

    #[test]
    fn names_its_copy_beside_it() {
        let dir = PathBuf::from(format!("/var/tmp/olo-edit-names-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("Screen Recording.mp4");
        assert_eq!(edited_name(&src, "mp4"), dir.join("Screen Recording (edited).mp4"));
        std::fs::write(dir.join("Screen Recording (edited).mp4"), b"").unwrap();
        assert_eq!(edited_name(&src, "mp4"), dir.join("Screen Recording (edited 2).mp4"));
        assert_eq!(edited_name(&src, "gif"), dir.join("Screen Recording (edited).gif"));
        assert_eq!(next_mark_title(&[(1.0, "Mark 1".into())]), "Mark 2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
