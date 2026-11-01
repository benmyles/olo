//! The egui kit's preview (Tracker's Quick Look until it moves to Olo's
//! GPUI): the pictures, pages, text, documents, sheets, listings, sound,
//! video and fonts the loader decodes, drawn with egui.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{self, Align, Color32, FontFamily, FontId, Key, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2},
    files,
    icons, metrics as m,
    style::{self, Syntax, c},
    widgets::{Column, ColumnListView, ListState, ScrollView},
};

use crate::{Body, Item, SLOW, Update, audio, input, load, media, rich, table, text, video};

const TEXT_ROW: f32 = 16.0;
const GUTTER_PAD: f32 = 10.0;

/// A picture as an egui image (its pixels copied: egui takes its own).
pub(crate) fn color_image(b: &crate::Bitmap) -> egui::ColorImage {
    egui::ColorImage::from_rgba_premultiplied(b.size, &b.rgba)
}

pub struct Preview {
    pub item: Item,
    pub generation: u64,
    body: Option<Body>,
    failed: Option<String>,
    cancel: Arc<AtomicBool>,
    textures: HashMap<usize, TextureHandle>,
    /// PDF pages asked of the worker and not yet back.
    requested: HashSet<usize>,
    player: Option<audio::Player>,
    video: Option<video::Video>,
    sheet: usize,
    list: ListState,
    /// The font family registered for a font preview.
    font: Option<String>,
    started: Instant,
    /// Largest image side decoded (bigger for a zoomable view).
    max_image: u32,
    /// Images can be zoomed and panned (Viewer), not just fitted.
    zoomable: bool,
    view: ImageView,
    /// Pan and zoom are paused (a tool over the picture takes the drags).
    still: bool,
    /// A drag started on the picture while there was nothing to pan: it
    /// is a drag of the file out (see [`Preview::take_drag_out`]).
    drag_out: bool,
    /// A formatted document's page, as laid out.
    rich: rich::RichView,
    /// A spreadsheet's search and the rows it shows.
    table: table::TableView,
}

/// Zoom and pan of an image: `zoom` is points per image pixel (None fits
/// the window), `center` the image point at the middle of the view.
#[derive(Clone, Copy, Default)]
struct ImageView {
    zoom: Option<f32>,
    center: egui::Pos2,
    /// The scale the image was last shown at, and the fit scale.
    shown: f32,
    fit: f32,
    /// Where it was last drawn, and in what.
    rect: Option<(Rect, Rect)>,
}

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 16.0;
const ZOOM_STEPS: [f32; 15] = [0.05, 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0];

impl Preview {
    /// Start loading `entry`; `send` delivers updates (tagged with the
    /// generation) to the UI thread.
    pub fn new(item: Item, generation: u64, send: impl Fn(u64, Update) + Send + 'static) -> Preview {
        Preview::make(item, generation, false, send)
    }

    /// The same, with images at full resolution that can be zoomed
    /// (pinch, ⌘-scroll, double-click, [`Preview::zoom`]) and panned.
    pub fn new_zoomable(item: Item, generation: u64, send: impl Fn(u64, Update) + Send + 'static) -> Preview {
        Preview::make(item, generation, true, send)
    }

    fn make(item: Item, generation: u64, zoomable: bool, send: impl Fn(u64, Update) + Send + 'static) -> Preview {
        let mut q = Preview {
            item,
            generation,
            body: None,
            failed: None,
            cancel: Arc::new(AtomicBool::new(false)),
            textures: HashMap::new(),
            requested: HashSet::new(),
            player: None,
            video: None,
            sheet: 0,
            list: ListState::default(),
            font: None,
            started: Instant::now(),
            max_image: if zoomable { load::MAX_ZOOMABLE } else { load::MAX_IMAGE },
            zoomable,
            view: ImageView::default(),
            still: false,
            drag_out: false,
            rich: rich::RichView::default(),
            table: table::TableView::default(),
        };
        q.spawn(send);
        q
    }

    fn spawn(&mut self, send: impl Fn(u64, Update) + Send + 'static) {
        let (item, cancel, generation, max) = (self.item.clone(), self.cancel.clone(), self.generation, self.max_image);
        std::thread::Builder::new()
            .name("quicklook".into())
            .spawn(move || load::load(&item, max, &cancel, &mut |u| send(generation, u)))
            .ok();
    }

    /// Show another item in the same window.
    pub fn retarget(&mut self, item: Item, generation: u64, send: impl Fn(u64, Update) + Send + 'static) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.item = item;
        self.generation = generation;
        self.failed = None;
        self.textures.clear();
        self.requested.clear();
        self.player = None;
        self.video = None;
        self.sheet = 0;
        self.list = ListState::default();
        self.table.reset();
        self.started = Instant::now();
        // Keep showing the previous body until the new one lands, so
        // stepping through a folder doesn't flash.
        self.spawn(send);
    }

    /// Apply an update; true when a new body arrived (the window may want
    /// a new size).
    pub fn update(&mut self, u: Update) -> bool {
        match u {
            Update::Body(b) => {
                self.textures.clear();
                self.requested.clear();
                self.view = ImageView::default();
                if let Body::Media(m) = &b
                    && m.playable
                    && !m.video
                {
                    self.player = Some(audio::Player::start(&input::Input::file(&self.item.path)));
                }
                self.video = match &b {
                    Body::Media(m) if m.playable && m.video => Some(video::Video::start(&input::Input::file(&self.item.path), m.duration.unwrap_or(0.0))),
                    _ => None,
                };
                self.body = Some(b);
                self.failed = None;
                return true;
            }
            Update::Page(i, img) => {
                if let Some(Body::Pages { pages, .. }) = &mut self.body
                    && let Some(slot) = pages.get_mut(i)
                {
                    *slot = Some(img);
                    self.textures.remove(&(i + 1));
                }
                self.requested.remove(&i);
            }
            Update::Highlighted(t) => {
                if let Some(Body::Text(old)) = &mut self.body {
                    *old = t;
                }
            }
            Update::Failed(e) => {
                self.body = None;
                self.failed = Some(e);
                return true;
            }
        }
        false
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.player = None;
        self.video = None;
    }

    /// The window size that suits what is shown, within the screen.
    /// `chrome` is the height the caller adds around the preview (bars).
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
                // One image pixel per point, like a 72 dpi image on macOS.
                Body::Image { full, .. } => fit(full[0], full[1]),
                Body::Pages { page_size, .. } => {
                    let (pw, ph) = (page_size.0.max(1.0), page_size.1.max(1.0));
                    let h = max_h;
                    fit(h * pw / ph + 2.0 * m::SPACING, h)
                }
                Body::Text(t) => {
                    let longest = t.lines.iter().take(400).map(|l| l.chars().count()).max().unwrap_or(40);
                    let w = (longest.clamp(40, 110) as f32 * 7.3 + 60.0).min(max_w);
                    (w, (t.lines.len() as f32 * TEXT_ROW + 2.0 * m::SPACING).clamp(200.0, max_h))
                }
                Body::Rich { .. } => (744.0f32.min(max_w), max_h),
                Body::Table { sheets, .. } => {
                    let rows = sheets.first().map_or(0, |s| s.1.len());
                    let tabs = if sheets.len() > 1 { 26.0 } else { 0.0 };
                    (880.0f32.min(max_w), (rows as f32 * m::LIST_ROW + 22.0 + tabs).clamp(200.0, max_h))
                }
                Body::Listing { items, .. } => (560.0, (items.len() as f32 * m::LIST_ROW + 22.0).clamp(200.0, max_h)),
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

    /// Draw the preview filling `content`.
    pub fn ui(&mut self, ui: &mut Ui, content: Rect) {
        let p = style::palette(ui.ctx());
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content));
        cui.set_clip_rect(content);
        match (&self.failed, self.body.is_some()) {
            (Some(e), _) => {
                let e = e.clone();
                self.card(&mut cui, content, Some(&e));
            }
            (None, false) => {
                self.card(&mut cui, content, None);
                if self.started.elapsed() > SLOW {
                    let r = Rect::from_center_size(pos2(content.center().x, content.bottom() - 40.0), vec2(20.0, 20.0));
                    cui.put(r, egui::Spinner::new().size(18.0).color(style::disabled_text(&p)));
                } else {
                    ui.ctx().request_repaint_after(SLOW);
                }
            }
            (None, true) => self.body_ui(&mut cui, content),
        }
    }

    /// What is shown, once loaded.
    pub fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }

    /// Whether the arrow keys are free for the caller (nothing shown
    /// scrolls or has a selection to move).
    pub fn arrows_free(&self) -> bool {
        self.failed.is_some() || matches!(self.body, None | Some(Body::Image { .. } | Body::Media(_) | Body::Font { .. } | Body::Card { .. }))
    }

    /// Whether an image is shown (so zoom commands apply).
    pub fn is_image(&self) -> bool {
        self.failed.is_none() && matches!(self.body, Some(Body::Image { .. }))
    }

    /// The image's scale as last shown (1.0 = one image pixel per point).
    pub fn image_scale(&self) -> f32 {
        self.view.shown
    }

    /// Show an edited picture in place of the one loaded (see [`edit`]).
    pub fn show_edited(&mut self, image: load::Image, smaller: Vec<load::Image>, full: [f32; 2], note: String) {
        self.textures.clear();
        // Same size (a flip): stay where we were; otherwise fit it again.
        let same = matches!(&self.body, Some(Body::Image { full: f, .. }) if *f == full);
        if !same {
            self.view = ImageView::default();
        }
        self.body = Some(Body::Image { image, smaller, full, note });
        self.failed = None;
    }

    /// Where the picture was last drawn, and the area it was drawn in.
    pub fn image_rect(&self) -> Option<(Rect, Rect)> {
        self.view.rect.filter(|_| self.is_image())
    }

    /// Pause pan and zoom by dragging (a tool over the picture wants the
    /// drags).
    /// A drag started on the picture where it doesn't pan (it fits the
    /// view, or the view doesn't zoom): the app drags the file out.
    pub fn take_drag_out(&mut self) -> bool {
        std::mem::take(&mut self.drag_out)
    }

    pub fn set_still(&mut self, on: bool) {
        self.still = on;
    }

    /// Whether the image is fitted to the window (no zoom chosen).
    pub fn image_fitted(&self) -> bool {
        self.view.zoom.is_none()
    }

    /// Zoom the image: `Some(true)` in a step, `Some(false)` out a step,
    /// `None` back to fitting the window. [`Preview::set_image_scale`]
    /// sets a scale (1.0 for Actual Size).
    pub fn zoom(&mut self, step: Option<bool>) {
        let cur = self.view.zoom.unwrap_or(self.view.fit);
        self.view.zoom = match step {
            None => None,
            Some(true) => Some(ZOOM_STEPS.iter().copied().find(|s| *s > cur + 1e-3).unwrap_or(MAX_ZOOM)),
            Some(false) => Some(ZOOM_STEPS.iter().rev().copied().find(|s| *s < cur - 1e-3).unwrap_or(MIN_ZOOM)),
        };
    }

    pub fn set_image_scale(&mut self, k: f32) {
        self.view.zoom = Some(k.clamp(MIN_ZOOM, MAX_ZOOM));
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
    pub fn toggle_playback(&self) -> bool {
        if let Some(v) = &self.video {
            v.toggle();
            return true;
        }
        match &self.player {
            Some(p) => {
                p.toggle();
                true
            }
            None => false,
        }
    }

    /// The icon, name and facts; with `error`, why there is no preview.
    fn card(&self, ui: &mut Ui, r: Rect, error: Option<&str>) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        painter.rect_filled(r, 0.0, c(p.panel_background));
        let size = 96.0;
        let top = r.center().y - (size + 3.0 * m::LABEL_ROW) / 2.0;
        let icon = Rect::from_center_size(pos2(r.center().x, top + size / 2.0), vec2(size, size));
        icons::paint(painter, icon, &self.item.icon);
        let mut y = icon.bottom() + m::SPACING + m::LABEL_ROW / 2.0;
        style::label(painter, r.center().x, y, Align::Center, &self.item.name, style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW;
        let facts = if self.item.is_dir {
            files::format_time(self.item.modified)
        } else {
            format!("{} — {}", files::format_size(self.item.size), files::format_time(self.item.modified))
        };
        style::label(painter, r.center().x, y, Align::Center, &facts, style::plain_font(), style::disabled_text(&p));
        if let Some(e) = error {
            y += m::LABEL_ROW;
            style::label(painter, r.center().x, y, Align::Center, e, style::plain_font(), style::disabled_text(&p));
        }
    }

    fn body_ui(&mut self, ui: &mut Ui, r: Rect) {
        let p = style::palette(ui.ctx());
        let id = self.generation;
        let Some(body) = &mut self.body else { return };
        match body {
            Body::Image { image, smaller, full, .. } => {
                ui.painter().rect_filled(r, 0.0, c(p.panel_background));
                let full = vec2(full[0].max(1.0), full[1].max(1.0));
                let fit = ((r.width() - 2.0 * m::SPACING) / full.x).min((r.height() - 2.0 * m::SPACING) / full.y).clamp(0.001, 1.0);
                let v = &mut self.view;
                v.fit = fit;
                let mut drag = false;
                if self.zoomable && !self.still {
                    drag = pan_and_zoom(ui, r, full, v, (id, "image"));
                }
                let k = v.zoom.unwrap_or(fit);
                if v.zoom.is_none() {
                    v.center = (full / 2.0).to_pos2();
                }
                v.shown = k;
                let ir = Rect::from_min_size(r.center() - v.center.to_vec2() * k, full * k);
                let ir = if v.zoom.is_none() { style::snap(ui.ctx(), ir) } else { ir };
                // The smallest level still at least as big as it is shown.
                let shown_px = ir.width() * ui.ctx().pixels_per_point();
                let level = smaller.iter().rposition(|l| l.size[0] as f32 >= shown_px);
                let levels: Vec<(usize, &load::Image)> =
                    std::iter::once((0, &*image)).chain(smaller.iter().enumerate().map(|(j, l)| (usize::MAX - j, l))).collect();
                let (key, img) = levels[level.map_or(0, |j| j + 1)];
                let mut tex = texture(ui.ctx(), &mut self.textures, key, img).id();
                // A big level is uploaded on a worker: meanwhile the
                // sharpest that is in, else one small enough to go at once.
                if olo_kit::textures::uploading(ui.ctx(), tex) {
                    let ctx = ui.ctx();
                    let ready = levels.iter().find(|(k, _)| self.textures.get(k).is_some_and(|t| !olo_kit::textures::uploading(ctx, t.id())));
                    let small = || levels.iter().find(|(_, l)| l.size[0] * l.size[1] * 4 < olo_kit::textures::STAGE_BYTES);
                    if let Some(&(k, l)) = ready.or_else(small) {
                        tex = texture(ctx, &mut self.textures, k, l).id();
                    }
                }
                v.rect = Some((ir, r));
                if !self.zoomable {
                    drag = ui.interact(ir.intersect(r), ui.make_persistent_id((id, "image-drag")), Sense::drag()).drag_started();
                }
                // Only from the picture itself, where the button went down.
                if drag && ui.input(|i| i.pointer.press_origin()).is_some_and(|o| ir.intersect(r).contains(o)) {
                    self.drag_out = true;
                }
                checker(ui.painter(), ir.intersect(r), &p);
                ui.painter().image(tex, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            Body::Pages { pages, page_size, want, .. } => {
                let (pw, ph) = (page_size.0.max(1.0), page_size.1.max(1.0));
                let gap = m::SPACING;
                let page_w = (r.width() - 2.0 * gap - 16.0).max(50.0);
                let page_h = (page_w * ph / pw).round();
                let n = pages.len();
                let textures = &mut self.textures;
                let requested = &mut self.requested;
                let bg = style::shade(&p, p.panel_background, 1.08);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ScrollView::new(("ql-pages", id)).border(false).background(p.panel_background).show_rows(ui, page_h + gap, n, |ui, range| {
                    ui.painter().rect_filled(ui.clip_rect(), 0.0, bg);
                    for i in range.clone() {
                        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), page_h + gap), Sense::hover());
                        let pr = style::snap(ui.ctx(), Rect::from_min_size(pos2(row.center().x - page_w / 2.0, row.top() + gap), vec2(page_w, page_h)));
                        ui.painter().rect_filled(pr.translate(vec2(1.0, 2.0)), 0.0, style::c(p.drop_shadow));
                        ui.painter().rect_filled(pr, 0.0, Color32::WHITE);
                        match &pages[i] {
                            Some(img) => {
                                let tex = texture(ui.ctx(), textures, i + 1, img);
                                ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                if requested.insert(i) {
                                    let _ = want.send(i);
                                }
                                ui.put(Rect::from_center_size(pr.center(), vec2(20.0, 20.0)), egui::Spinner::new().size(18.0).color(style::disabled_text(&p)));
                            }
                        }
                    }
                    // Free pages far from view (they are rendered again if
                    // scrolled back to).
                    let keep = range.start.saturating_sub(4)..range.end + 4;
                    for (i, slot) in pages.iter_mut().enumerate() {
                        if !keep.contains(&i) && slot.is_some() {
                            *slot = None;
                            textures.remove(&(i + 1));
                        }
                    }
                });
            }
            Body::Text(t) => text_view(ui, r, t, id),
            Body::Rich { blocks, .. } => self.rich.show(ui, r, blocks, id, self.item.path.parent()),
            Body::Table { sheets, .. } => {
                let names: Vec<&str> = sheets.iter().map(|s| s.0.as_str()).collect();
                let mut area = r;
                if sheets.len() > 1 {
                    let tabs = Rect::from_min_max(r.min, pos2(r.right(), r.top() + 26.0));
                    ui.painter().rect_filled(tabs, 0.0, c(p.panel_background));
                    let mut x = tabs.left() + m::SPACING;
                    for (k, name) in names.iter().enumerate() {
                        let w = style::label_width(ui.ctx(), name, style::plain_font()) + 24.0;
                        let br = Rect::from_min_size(pos2(x, tabs.top() + 3.0), vec2(w, 20.0));
                        let resp = ui.interact(br, ui.make_persistent_id(("ql-sheet", k)), Sense::click());
                        let selected = self.sheet == k;
                        if selected {
                            style::raised(ui.painter(), br, &p);
                        }
                        let font = if selected { style::bold_font() } else { style::plain_font() };
                        style::label(ui.painter(), br.center().x, br.center().y, Align::Center, *name, font, c(p.panel_text));
                        if resp.clicked() && self.sheet != k {
                            self.sheet = k;
                            self.list = ListState::default();
                        }
                        x += w + m::TIGHT;
                    }
                    area = Rect::from_min_max(pos2(r.left(), tabs.bottom()), r.max);
                }
                if let Some((_, rows)) = sheets.get(self.sheet) {
                    self.table.show(ui, area, rows, &mut self.list, (id, self.sheet));
                }
            }
            Body::Listing { items, .. } => {
                let cols = [Column::new("Name", 360.0), Column::new("Size", 90.0).right()];
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                let items = &*items;
                ColumnListView::new(("ql-list", id), &cols, items.len(), &mut self.list).show(&mut ui, |cell| {
                    let it = &items[cell.row];
                    match cell.column {
                        0 => {
                            let ir = Rect::from_center_size(pos2(cell.rect.left() + 12.0, cell.rect.center().y), vec2(16.0, 16.0));
                            let kind = files::kind_for(Path::new(&it.name), it.is_dir, 0o644);
                            icons::paint(cell.painter, ir, kind.icon());
                            let tr = Rect::from_min_max(pos2(cell.rect.left() + 22.0, cell.rect.top()), cell.rect.max);
                            let sub = olo_kit::widgets::Cell { rect: tr, ..*cell };
                            sub.text(&it.name);
                        }
                        _ => cell.text(&if it.is_dir { "—".into() } else { files::format_size(it.size) }),
                    }
                });
            }
            Body::Media(md) => media_view(ui, r, md, &mut self.textures, self.player.as_ref(), self.video.as_ref()),
            Body::Font { data, .. } => {
                let family = format!("ql-font-{id}");
                if self.font.as_deref() != Some(&family) {
                    ui.ctx().add_font(egui::epaint::text::FontInsert::new(
                        &family,
                        egui::FontData::from_owned(data.to_vec()),
                        vec![egui::epaint::text::InsertFontFamily {
                            family: FontFamily::Name(family.clone().into()),
                            priority: egui::epaint::text::FontPriority::Highest,
                        }],
                    ));
                    self.font = Some(family.clone());
                    ui.ctx().request_repaint();
                    return;
                }
                font_view(ui, r, FontFamily::Name(family.into()));
            }
            Body::Card { .. } => self.card(ui, r, None),
        }
    }
}

fn note_of(b: &Body) -> String {
    match b {
        Body::Image { note, .. }
        | Body::Pages { note, .. }
        | Body::Rich { note, .. }
        | Body::Table { note, .. }
        | Body::Listing { note, .. }
        | Body::Font { note, .. }
        | Body::Card { note } => note.clone(),
        Body::Text(t) => t.note.clone(),
        Body::Media(m) => m.note.clone(),
    }
}

/// Pinch, ⌘-scroll and double-click zoom about the pointer; drag and
/// scroll pan an image bigger than the view. Keeps the image covering the
/// view where it can.
/// Pan and zoom the picture; true when a drag starts while it fits (so
/// there is nothing to pan).
fn pan_and_zoom(ui: &mut Ui, r: Rect, full: egui::Vec2, v: &mut ImageView, id: impl egui::AsIdSalt) -> bool {
    let resp = ui.interact(r, ui.make_persistent_id(id), Sense::click_and_drag());
    let k = v.zoom.unwrap_or(v.fit);
    if v.zoom.is_none() {
        v.center = (full / 2.0).to_pos2();
    }
    let pointer = resp.hover_pos().unwrap_or(r.center());
    // The image point under `at` stays there as the scale changes.
    let rescale = |v: &mut ImageView, to: f32, at: egui::Pos2| {
        let to = to.clamp(MIN_ZOOM.min(v.fit), MAX_ZOOM);
        let under = v.center + (at - r.center()) / k;
        v.center = under - (at - r.center()) / to;
        v.zoom = Some(to);
    };
    let zoom_delta = ui.input(|i| i.zoom_delta());
    if resp.hovered() && (zoom_delta - 1.0).abs() > 1e-3 {
        rescale(v, k * zoom_delta, pointer);
    }
    if resp.double_clicked() {
        if v.zoom.is_some_and(|z| (z - 1.0).abs() < 1e-3) || v.fit >= 1.0 && v.zoom.is_some() {
            v.zoom = None;
        } else {
            rescale(v, 1.0, pointer);
        }
    }
    let k = v.zoom.unwrap_or(v.fit);
    let bigger = full.x * k > r.width() + 0.5 || full.y * k > r.height() + 0.5;
    if bigger {
        if resp.dragged() {
            v.center -= resp.drag_delta() / k;
        }
        let scroll = if resp.hovered() { ui.input(|i| if i.modifiers.command { egui::Vec2::ZERO } else { i.smooth_scroll_delta }) } else { egui::Vec2::ZERO };
        v.center -= scroll / k;
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else if resp.hovered() { egui::CursorIcon::Grab } else { egui::CursorIcon::Default });
    }
    // Keep the image over the view: centred along an axis where it fits,
    // edge to edge where it doesn't.
    if v.zoom.is_some() {
        for (c, len, view) in [(&mut v.center.x, full.x, r.width()), (&mut v.center.y, full.y, r.height())] {
            let half = view / 2.0 / k;
            *c = if len * k <= view { len / 2.0 } else { c.clamp(half, len - half) };
        }
    }
    resp.drag_started() && !bigger
}

fn texture<'a>(ctx: &egui::Context, cache: &'a mut HashMap<usize, TextureHandle>, key: usize, img: &load::Image) -> &'a TextureHandle {
    cache.entry(key).or_insert_with(|| {
        // Small images are magnified with crisp pixels.
        let options = if img.size[0].max(img.size[1]) <= 256 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
        ctx.load_texture(format!("quicklook-{key}"), color_image(img), options)
    })
}

/// A light checkerboard behind images, so transparency reads as such:
/// one quad of a repeating texture made at the scale, a texel to a
/// device pixel (as rects, a full screen of cells took 8 ms a frame).
fn checker(painter: &egui::Painter, r: Rect, p: &olo_kit::theme::Palette) {
    // Always light, in either mode: dark artwork must stay visible.
    let _ = p;
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let cell = (8.0 * ppp).round().max(1.0) as usize;
    let id = egui::Id::unique(("preview-checker", cell));
    let tex = ctx.data(|d| d.get_temp::<TextureHandle>(id)).unwrap_or_else(|| {
        let n = 2 * cell;
        let pixels = (0..n * n)
            .map(|i| if ((i % n) / cell + (i / n) / cell) % 2 == 0 { Color32::from_gray(255) } else { Color32::from_gray(232) })
            .collect();
        let image = egui::ColorImage::new([n, n], pixels);
        let tex = ctx.load_texture("preview-checker", image, TextureOptions::NEAREST_REPEAT);
        ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
        tex
    });
    // On whole device pixels, so the texels land on them.
    let r = style::snap(ctx, r);
    let n = (2 * cell) as f32;
    let uv = Rect::from_min_size(pos2(0.0, 0.0), r.size() * ppp / n);
    painter.image(tex.id(), r, uv, Color32::WHITE);
}

fn text_view(ui: &mut Ui, r: Rect, t: &text::Text, id: u64) {
    let p = style::palette(ui.ctx());
    let font = style::mono_font(style::MONO_SIZE);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let digits = t.lines.len().max(1).to_string().len();
    let gutter = digits as f32 * char_w + 2.0 * GUTTER_PAD;
    let longest = t.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let content_w = gutter + longest as f32 * char_w + 2.0 * GUTTER_PAD;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    ScrollView::new(("ql-text", id)).border(false).horizontal(true).background(p.document_background).show_rows(
        &mut ui,
        TEXT_ROW,
        t.lines.len(),
        |ui, range| {
            ui.set_min_width(content_w);
            let gutter_bg = style::shade(&p, p.document_background, 1.04);
            let left = ui.min_rect().left();
            for i in range {
                let (row, _) = ui.allocate_exact_size(vec2(content_w.max(ui.available_width()), TEXT_ROW), Sense::hover());
                let g = Rect::from_min_max(row.min, pos2(left + gutter, row.bottom()));
                ui.painter().rect_filled(g, 0.0, gutter_bg);
                style::label(ui.painter(), g.right() - GUTTER_PAD, row.center().y, Align::Max, (i + 1).to_string(), font.clone(), style::disabled_text(&p));
                let line = &t.lines[i];
                let mut job = egui::text::LayoutJob::default();
                let fmt = |s: Syntax| egui::TextFormat::simple(font.clone(), style::syntax_color(&p, s));
                match t.spans.get(i) {
                    Some(spans) if !spans.is_empty() => {
                        let mut at = 0;
                        for &(a, b, tok) in spans {
                            let (a, b) = (a as usize, (b as usize).min(line.len()));
                            if a > at && line.is_char_boundary(at) && line.is_char_boundary(a) {
                                job.append(&line[at..a], 0.0, fmt(Syntax::Plain));
                            }
                            if a < b && line.is_char_boundary(a) && line.is_char_boundary(b) {
                                job.append(&line[a..b], 0.0, fmt(tok));
                            }
                            at = b;
                        }
                        if at < line.len() && line.is_char_boundary(at) {
                            job.append(&line[at..], 0.0, fmt(Syntax::Plain));
                        }
                    }
                    _ => job.append(line, 0.0, fmt(Syntax::Plain)),
                }
                let galley = ui.painter().layout_job(job);
                let pos = style::text_origin(ui.ctx(), &galley, g.right() + GUTTER_PAD, row.center().y, style::MONO_SIZE);
                ui.painter().galley(pos, galley, c(p.document_text));
            }
        },
    );
}

/// Below a video: its title and facts, and the transport when it plays.
const VIDEO_INFO: f32 = 2.0 * m::LABEL_ROW + 2.0 * m::SPACING;
const TRANSPORT: f32 = 34.0;

/// What the transport row drives: a sound or a video.
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

/// Play/pause, elapsed time, a seek bar and the time left, centred on
/// `y` between `x` and `right`.
fn transport(ui: &mut Ui, x: f32, right: f32, y: f32, t: &dyn Transport, duration: Option<f64>) {
    let p = style::palette(ui.ctx());
    if let Some(e) = t.error() {
        style::label(ui.painter(), x, y, Align::Min, &e, style::plain_font(), style::disabled_text(&p));
        return;
    }
    let br = Rect::from_center_size(pos2(x + 13.0, y), vec2(26.0, 22.0));
    let playing = t.playing();
    let resp = ui.interact(br, ui.make_persistent_id("ql-play"), Sense::click());
    style::raised(ui.painter(), br, &p);
    if resp.is_pointer_button_down_on() {
        style::sunken(ui.painter(), br, &p, false);
    }
    let gc = c(p.control_text);
    if playing {
        for dx in [-3.5, 2.5] {
            ui.painter().rect_filled(Rect::from_center_size(br.center() + vec2(dx, 0.0), vec2(3.0, 10.0)), 0.0, gc);
        }
    } else {
        let cx = br.center();
        ui.painter().add(egui::Shape::convex_polygon(
            vec![cx + vec2(-3.5, -5.5), cx + vec2(5.5, 0.0), cx + vec2(-3.5, 5.5)],
            gc,
            egui::Stroke::NONE,
        ));
    }
    if resp.clicked() {
        t.toggle();
    }
    if ui.input(|i| i.key_pressed(Key::P)) {
        t.toggle();
    }
    let pos = t.position();
    let dur = duration.unwrap_or_else(|| t.duration()).max(0.001);
    let font = style::mono_font(11.0);
    let left_t = media::format_duration(pos);
    let right_t = format!("-{}", media::format_duration((dur - pos).max(0.0)));
    style::label(ui.painter(), br.right() + 8.0, y, Align::Min, &left_t, font.clone(), style::disabled_text(&p));
    style::label(ui.painter(), right, y, Align::Max, &right_t, font, style::disabled_text(&p));
    let bar = Rect::from_min_max(pos2(br.right() + 52.0, y - 3.0), pos2(right - 52.0, y + 3.0));
    let bar = style::snap(ui.ctx(), bar);
    style::sunken(ui.painter(), bar.expand(1.0), &p, false);
    ui.painter().rect_filled(bar, 0.0, c(p.document_background));
    let frac = (pos / dur).clamp(0.0, 1.0) as f32;
    ui.painter().rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * frac, bar.bottom())), 0.0, c(p.keyboard_navigation));
    let seek = ui.interact(bar.expand2(vec2(0.0, 6.0)), ui.make_persistent_id("ql-seek"), Sense::click_and_drag());
    if (seek.clicked() || seek.dragged())
        && let Some(pt) = seek.interact_pointer_pos()
    {
        let f = ((pt.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64;
        t.seek(f * dur);
    }
    if playing {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn media_view(
    ui: &mut Ui,
    r: Rect,
    md: &media::Media,
    textures: &mut HashMap<usize, TextureHandle>,
    player: Option<&audio::Player>,
    video: Option<&video::Video>,
) {
    let p = style::palette(ui.ctx());
    ui.painter().rect_filled(r, 0.0, c(p.panel_background));
    let ink = c(p.panel_text);
    if md.video {
        // The picture, the transport, then the facts beneath.
        let info_h = VIDEO_INFO + if video.is_some() { TRANSPORT } else { 0.0 };
        let area = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - info_h));
        if let Some(v) = video {
            // Texture 1 is the playing frame, 0 the poster until it comes.
            let (frame, next) = v.tick();
            if let Some(img) = frame {
                match textures.get_mut(&1) {
                    Some(t) if t.size() == img.size => t.set(color_image(&img), egui::TextureOptions::LINEAR),
                    _ => {
                        textures.insert(1, ui.ctx().load_texture("ql-video", color_image(&img), egui::TextureOptions::LINEAR));
                    }
                }
            }
            if let Some(d) = next {
                ui.ctx().request_repaint_after(d);
            }
        }
        let shown = textures.get(&1).cloned().or_else(|| md.picture.as_ref().map(|img| texture(ui.ctx(), textures, 0, img).clone()));
        match shown {
            Some(tex) => {
                let size = tex.size_vec2();
                let k = (area.width() / size.x).min(area.height() / size.y).min(1.0);
                let ir = style::snap(ui.ctx(), Rect::from_center_size(area.center(), size * k));
                ui.painter().rect_filled(area, 0.0, Color32::BLACK);
                ui.painter().image(tex.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                // A click on the picture plays or pauses.
                if let Some(v) = video
                    && ui.interact(area, ui.make_persistent_id("ql-video"), Sense::click()).clicked()
                {
                    v.toggle();
                }
            }
            None => {
                let ir = Rect::from_center_size(area.center(), vec2(96.0, 96.0));
                icons::paint(ui.painter(), ir, "video");
            }
        }
        let mut y = area.bottom();
        if let Some(v) = video {
            y += TRANSPORT / 2.0 + 2.0;
            transport(ui, r.left() + m::WINDOW_INSET, r.right() - m::WINDOW_INSET, y, v, md.duration);
            y += TRANSPORT / 2.0 - 2.0;
        }
        y += m::SPACING + m::LABEL_ROW / 2.0;
        style::label(ui.painter(), r.left() + m::WINDOW_INSET, y, Align::Min, &md.title, style::bold_font(), ink);
        y += m::LABEL_ROW;
        let facts = md.rows.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join("  ·  ");
        style::label(ui.painter(), r.left() + m::WINDOW_INSET, y, Align::Min, &facts, style::plain_font(), style::disabled_text(&p));
        return;
    }
    // Audio: cover on the left, title, facts and the transport.
    let inner = r.shrink(m::WINDOW_INSET + 4.0);
    let art = (inner.height()).min(160.0);
    let ar = Rect::from_min_size(pos2(inner.left(), inner.center().y - art / 2.0), vec2(art, art));
    match &md.picture {
        Some(img) => {
            let tex = texture(ui.ctx(), textures, 0, img);
            let ar = style::snap(ui.ctx(), ar);
            ui.painter().rect_filled(ar.translate(vec2(1.0, 2.0)), 2.0, style::c(p.drop_shadow));
            ui.painter().image(tex.id(), ar, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            icons::paint(ui.painter(), ar.shrink(art * 0.15), "audio");
        }
    }
    let x = ar.right() + m::WINDOW_INSET + 4.0;
    let right = inner.right();
    // Title, subtitle, facts and the transport as one block, centred
    // beside the art.
    let facts: Vec<&(String, String)> = md.rows.iter().filter(|(k, _)| k != "Length").collect();
    let block = 22.0
        + if md.subtitle.is_empty() { 0.0 } else { m::LABEL_ROW }
        + facts.len() as f32 * m::LABEL_ROW
        + if player.is_some() { m::SPACING * 2.0 + 22.0 } else { 0.0 };
    let mut y = ar.center().y - block / 2.0 + 11.0;
    let line = |ui: &Ui, y: f32, text: &str, font: FontId, color: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(right - x);
        let g = ui.painter().layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, x, y, font.size);
        ui.painter().galley(pos, g, color);
    };
    line(ui, y, &md.title, FontId::new(16.0, style::bold()), ink);
    y += 22.0;
    if !md.subtitle.is_empty() {
        line(ui, y, &md.subtitle, style::plain_font(), ink);
        y += m::LABEL_ROW;
    }
    for (k, v) in facts {
        line(ui, y, &format!("{k}: {v}"), style::plain_font(), style::disabled_text(&p));
        y += m::LABEL_ROW;
    }
    let Some(player) = player else { return };
    let ty = y - m::LABEL_ROW / 2.0 + m::SPACING * 2.0 + 11.0;
    transport(ui, x, right, ty, player, md.duration);
}

fn font_view(ui: &mut Ui, r: Rect, family: FontFamily) {
    let p = style::palette(ui.ctx());
    let ink = c(p.document_text);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r));
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    ScrollView::new("ql-font").border(false).background(p.document_background).show(&mut ui, |ui| {
        let x = ui.min_rect().left() + 2.0 * m::SPACING + 4.0;
        let w = ui.available_width() - 4.0 * m::SPACING;
        ui.add_space(m::SPACING * 2.0);
        let samples: [(f32, &str); 6] = [
            (72.0, "Aa Bb Gg"),
            (28.0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
            (28.0, "abcdefghijklmnopqrstuvwxyz"),
            (28.0, "0123456789 &@?!%(){}[]"),
            (18.0, "The quick brown fox jumps over the lazy dog."),
            (12.0, "The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs."),
        ];
        for (size, text) in samples {
            let mut job = egui::text::LayoutJob::simple(text.to_owned(), FontId::new(size, family.clone()), ink, w);
            job.wrap.max_width = w;
            let g = ui.painter().layout_job(job);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 10.0), Sense::hover());
            ui.painter().galley(pos2(x, rect.top()), g, ink);
        }
    });
}
