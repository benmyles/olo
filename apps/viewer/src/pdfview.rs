//! A PDF in a Viewer window: pages stacked in a scroll view at any zoom,
//! rendered by the worker on demand, with the form fields and marks drawn
//! (and edited) on top.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, mpsc::Sender},
};

use tabula_kit::{
    egui::{
        self, Color32, CursorIcon, FontFamily, FontId, Key, Pos2, Rect, Sense, Stroke, TextureHandle,
        TextureOptions, Ui, Vec2, pos2, vec2,
    },
    menu::Menu,
    style::{self, c},
    widgets::{self, ScrollView},
};

use crate::{
    pdfdoc::{self, Field, FieldKind, Mark, MarkKind, Model, PageGeom, PdfRect, Value},
    pdfwork::{Event, Request},
};

/// Space around and between pages, in logical pixels.
const GAP: f32 = 12.0;
/// The family field values and text marks are drawn in (Helvetica
/// metrics, so what you type wraps as it will in the saved file).
pub const PDF_FONT: &str = "pdf-sans";
/// Grab handles on a selected mark.
const HANDLE: f32 = 7.0;
pub const MIN_ZOOM: f32 = 0.1;
/// Narrowest a PDF window gets: the toolbar's width.
pub const MIN_WIDTH: f32 = 560.0;
pub const MAX_ZOOM: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Text,
    Draw,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    Width,
    Page,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sel {
    Mark(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Editing {
    /// Field index, widget index.
    Field(usize, usize),
    Mark(usize),
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Moving a mark: where it was, where the pointer grabbed (user space).
    Move { mark: usize, start: PdfRect, grab: [f32; 2] },
    /// Resizing from corner `corner` (0 = bottom-left, then counter-clockwise).
    Resize { mark: usize, corner: usize, start: PdfRect },
    Draw { page: usize },
}

struct PageTex {
    tex: TextureHandle,
    scale: f32,
}

pub struct PdfView {
    id: u64,
    tx: Sender<Request>,
    pub geoms: Vec<PageGeom>,
    pub model: Model,
    /// The model as last saved (or opened).
    saved: Model,
    pub editable: bool,
    textures: HashMap<usize, PageTex>,
    /// Rendered pages not yet uploaded as textures.
    arrived: Vec<(usize, f32, Arc<egui::ColorImage>)>,
    /// Scale last asked of the worker, per page.
    asked: HashMap<usize, f32>,
    pub zoom: f32,
    pub fit: Option<Fit>,
    pub tool: Tool,
    pub ink: [f32; 3],
    sel: Option<Sel>,
    editing: Option<Editing>,
    /// Text being typed into the field or mark being edited.
    buffer: String,
    focus_pending: bool,
    drag: Option<Drag>,
    stroke: Vec<[f32; 2]>,
    undo: Vec<Model>,
    redo: Vec<Model>,
    /// An edit session pushed its undo snapshot already.
    snapshot_taken: bool,
    pub current_page: usize,
    /// Content offset and size of the scroll view, last frame.
    offset: Vec2,
    view: Vec2,
    /// Keep this content point (relative to the view) still across a zoom.
    anchor: Option<(Vec2, Vec2, f32)>,
    /// Scroll to the top of this page next frame.
    goto: Option<usize>,
    /// Bring this content range into view next frame.
    reveal: Option<(f32, f32)>,
    pub saving: bool,
}

/// Affine maps as [a b c d e f].
fn apply(t: &[f64; 6], x: f32, y: f32) -> [f32; 2] {
    let (x, y) = (x as f64, y as f64);
    [(t[0] * x + t[2] * y + t[4]) as f32, (t[1] * x + t[3] * y + t[5]) as f32]
}

fn invert(t: &[f64; 6]) -> [f64; 6] {
    let det = t[0] * t[3] - t[1] * t[2];
    if det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let (a, b, cc, d) = (t[3] / det, -t[1] / det, -t[2] / det, t[0] / det);
    [a, b, cc, d, -(a * t[4] + cc * t[5]), -(b * t[4] + d * t[5])]
}

fn rgb(c3: [f32; 3]) -> Color32 {
    Color32::from_rgb((c3[0] * 255.0) as u8, (c3[1] * 255.0) as u8, (c3[2] * 255.0) as u8)
}

/// Ink colours offered in the Tools menu.
pub const INKS: [(&str, [f32; 3]); 3] = [("Black", [0.0, 0.0, 0.0]), ("Blue", [0.05, 0.2, 0.6]), ("Red", [0.75, 0.08, 0.08])];

impl PdfView {
    pub fn new(id: u64, tx: Sender<Request>) -> Self {
        PdfView {
            id,
            tx,
            geoms: Vec::new(),
            model: Model::default(),
            saved: Model::default(),
            editable: false,
            textures: HashMap::new(),
            arrived: Vec::new(),
            asked: HashMap::new(),
            zoom: 1.0,
            fit: Some(Fit::Width),
            tool: Tool::Select,
            ink: INKS[0].1,
            sel: None,
            editing: None,
            buffer: String::new(),
            focus_pending: false,
            drag: None,
            stroke: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            snapshot_taken: false,
            current_page: 0,
            offset: Vec2::ZERO,
            view: Vec2::ZERO,
            anchor: None,
            goto: None,
            reveal: None,
            saving: false,
        }
    }

    pub fn loaded(&self) -> bool {
        !self.geoms.is_empty()
    }

    /// Apply a worker event; true when the window should resize (opened).
    pub fn event(&mut self, ev: Event) -> bool {
        match ev {
            Event::Opened { geoms, model, editable } => {
                self.geoms = geoms;
                self.saved = model.clone();
                self.model = model;
                self.editable = editable;
                true
            }
            Event::Page { page, scale, image } => {
                // Uploaded on the next frame, where there's a context.
                self.arrived.push((page, scale, image));
                false
            }
            Event::Saved { adopted, .. } => {
                self.saving = false;
                if let Some(model) = adopted {
                    // Same values; the marks now have object ids.
                    self.end_editing();
                    self.saved = model.clone();
                    self.model = model;
                    self.sel = None;
                }
                false
            }
            Event::SaveFailed(_) | Event::Failed(_) => {
                self.saving = false;
                false
            }
        }
    }

    pub fn dirty(&self) -> bool {
        self.model.fields != self.saved.fields || self.model.marks != self.saved.marks || self.editing_changed()
    }

    fn editing_changed(&self) -> bool {
        match self.editing {
            Some(Editing::Field(f, _)) => self.model.fields.get(f).is_some_and(|f| f.value != Value::Text(self.buffer.clone())),
            Some(Editing::Mark(i)) => {
                self.model.marks.get(i).is_some_and(|m| !matches!(&m.kind, MarkKind::Text { text, .. } if *text == self.buffer))
            }
            None => false,
        }
    }

    /// Ask the worker to save (the edit in progress included).
    pub fn save(&mut self, to: PathBuf, flatten: bool, adopt: bool) {
        self.end_editing();
        self.saving = true;
        let _ = self.tx.send(Request::Save { model: self.model.clone(), to, flatten, adopt });
    }

    /// The window size that shows the first page whole, within the screen.
    pub fn ideal_size(&mut self, screen: Option<(u32, u32)>, chrome: f32) -> (u32, u32) {
        let (sw, sh) = screen.map_or((1440.0, 900.0), |(w, h)| (w as f32, h as f32));
        let (pw, ph) = self.geoms.first().map_or((612.0, 792.0), |g| g.size);
        let zoom = ((sh * 0.86 - chrome - 2.0 * GAP) / ph).min((sw * 0.8 - 2.0 * GAP - widgets::SCROLL_BAR) / pw).clamp(0.3, 1.5);
        self.zoom = zoom;
        let w = pw * zoom + 2.0 * GAP + widgets::SCROLL_BAR + 4.0;
        let h = ph * zoom + 2.0 * GAP + chrome + 4.0;
        (w.round().max(MIN_WIDTH) as u32, h.round().max(240.0) as u32)
    }

    // ---- editing ---------------------------------------------------------

    fn push_undo(&mut self) {
        self.undo.push(self.model.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.editing_changed()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.end_editing();
        if let Some(m) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.model, m));
            self.sel = None;
        }
    }

    pub fn redo(&mut self) {
        self.end_editing();
        if let Some(m) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.model, m));
            self.sel = None;
        }
    }

    pub fn has_selection(&self) -> bool {
        self.sel.is_some() && self.editing.is_none()
    }

    pub fn delete_selection(&mut self) {
        if let (Some(Sel::Mark(i)), None) = (self.sel, self.editing)
            && i < self.model.marks.len()
        {
            self.push_undo();
            self.model.marks.remove(i);
            self.sel = None;
        }
    }

    /// Commit what is being typed.
    fn end_editing(&mut self) {
        let Some(e) = self.editing.take() else { return };
        let text = std::mem::take(&mut self.buffer);
        match e {
            Editing::Field(f, _) => {
                if let Some(field) = self.model.fields.get(f)
                    && field.value != Value::Text(text.clone())
                {
                    if !self.snapshot_taken {
                        self.push_undo();
                    }
                    self.model.fields[f].value = Value::Text(text);
                }
            }
            Editing::Mark(i) => {
                let Some(mark) = self.model.marks.get(i) else { return };
                let MarkKind::Text { text: old, size } = &mark.kind else { return };
                let size = *size;
                if text.trim().is_empty() {
                    // An empty text box goes away (undoably, if it had text).
                    if !old.is_empty() && !self.snapshot_taken {
                        self.push_undo();
                    }
                    self.model.marks.remove(i);
                    self.sel = None;
                    return;
                }
                if *old != text {
                    if !self.snapshot_taken {
                        self.push_undo();
                    }
                    let mark = &mut self.model.marks[i];
                    let h = pdfdoc::text_mark_height(&text, size, mark.rect[2] - mark.rect[0]);
                    mark.rect[1] = mark.rect[3] - h;
                    mark.kind = MarkKind::Text { text, size };
                }
            }
        }
        self.snapshot_taken = false;
    }

    fn start_field(&mut self, f: usize, w: usize) {
        self.end_editing();
        let field = &self.model.fields[f];
        if field.read_only || !self.editable {
            return;
        }
        match (&field.kind, &field.value) {
            (FieldKind::Check, Value::State(s)) => {
                let on = field.widgets[w].on.clone();
                let new = if s.is_some() { None } else { on };
                self.push_undo();
                self.model.fields[f].value = Value::State(new);
            }
            (FieldKind::Radio, _) => {
                let on = field.widgets[w].on.clone();
                if field.value != Value::State(on.clone()) {
                    self.push_undo();
                    self.model.fields[f].value = Value::State(on);
                }
            }
            (FieldKind::Choice { combo: true, editable: false, .. } | FieldKind::Choice { combo: false, .. }, _) => {}
            (_, Value::Text(t)) => {
                self.buffer = t.clone();
                self.editing = Some(Editing::Field(f, w));
                self.focus_pending = true;
                self.snapshot_taken = false;
            }
            _ => {}
        }
        self.sel = None;
    }

    fn start_mark_text(&mut self, i: usize) {
        self.end_editing();
        if let Some(Mark { kind: MarkKind::Text { text, .. }, .. }) = self.model.marks.get(i) {
            self.buffer = text.clone();
            self.editing = Some(Editing::Mark(i));
            self.sel = Some(Sel::Mark(i));
            self.focus_pending = true;
            self.snapshot_taken = false;
        }
    }

    /// Tab: the next editable text field after the one being edited.
    fn next_field(&mut self, back: bool) {
        let Some(Editing::Field(f, _)) = self.editing else { return };
        let n = self.model.fields.len();
        for k in 1..=n {
            let i = if back { (f + n - k) % n } else { (f + k) % n };
            let field = &self.model.fields[i];
            if !field.read_only && matches!(field.kind, FieldKind::Text { .. } | FieldKind::Choice { editable: true, .. }) {
                let page = field.widgets[0].page;
                self.start_field(i, 0);
                self.goto_rect(page, field_rect(&self.model.fields[i], 0));
                return;
            }
        }
    }

    /// Add a signature (normalised strokes and aspect) to the middle of
    /// the visible part of the current page.
    pub fn place_signature(&mut self, strokes: Vec<Vec<[f32; 2]>>, aspect: f32) {
        if !self.editable || self.geoms.is_empty() {
            return;
        }
        self.end_editing();
        let page = self.current_page.min(self.geoms.len() - 1);
        let g = self.geoms[page];
        let width = (g.size.0 * 0.3).clamp(80.0, 200.0);
        let height = width / aspect.max(0.1);
        // The centre of what's visible of that page, in user space.
        let layout = self.layout();
        let pr = layout[page];
        let visible = Rect::from_min_size(self.offset.to_pos2(), self.view).intersect(pr);
        let centre = if visible.is_positive() { visible.center() } else { pr.center() };
        let inv = invert(&g.transform);
        let p = apply(&inv, (centre.x - pr.left()) / self.zoom, (centre.y - pr.top()) / self.zoom);
        let rect = [p[0] - width / 2.0, p[1] - height / 2.0, p[0] + width / 2.0, p[1] + height / 2.0];
        self.push_undo();
        self.model.marks.push(Mark {
            id: None,
            page,
            rect,
            color: self.ink,
            kind: MarkKind::Ink { strokes, width: 1.6, signature: true },
        });
        self.sel = Some(Sel::Mark(self.model.marks.len() - 1));
        self.tool = Tool::Select;
    }

    pub fn set_tool(&mut self, t: Tool) {
        self.end_editing();
        self.tool = t;
        self.sel = None;
    }

    // ---- zoom and pages ----------------------------------------------------

    pub fn set_zoom(&mut self, z: f32, fit: Option<Fit>) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        // Keep the middle of the view where it is.
        let mid = self.view / 2.0;
        self.anchor = Some((self.offset + mid, mid, self.zoom));
        self.zoom = z;
        self.fit = fit;
    }

    pub fn zoom_step(&mut self, up: bool) {
        const STEPS: [f32; 13] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];
        let z = if up {
            STEPS.iter().copied().find(|s| *s > self.zoom + 0.01).unwrap_or(MAX_ZOOM)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < self.zoom - 0.01).unwrap_or(MIN_ZOOM)
        };
        self.set_zoom(z, None);
    }

    pub fn go_to_page(&mut self, page: usize) {
        if !self.geoms.is_empty() {
            self.goto = Some(page.min(self.geoms.len() - 1));
        }
    }

    /// Page rects in content coordinates at the current zoom.
    fn layout(&self) -> Vec<Rect> {
        let widest = self.geoms.iter().map(|g| g.size.0).fold(0.0, f32::max) * self.zoom;
        let content_w = (widest + 2.0 * GAP).max(self.view.x);
        let mut y = GAP;
        self.geoms
            .iter()
            .map(|g| {
                let (w, h) = (g.size.0 * self.zoom, g.size.1 * self.zoom);
                let r = Rect::from_min_size(pos2(((content_w - w) / 2.0).round(), y.round()), vec2(w.round(), h.round()));
                y += h + GAP;
                r
            })
            .collect()
    }

    fn content_size(&self, layout: &[Rect]) -> Vec2 {
        let w = layout.iter().map(|r| r.width()).fold(0.0, f32::max) + 2.0 * GAP;
        let h = layout.last().map_or(0.0, |r| r.bottom() + GAP);
        vec2(w, h)
    }

    fn goto_rect(&mut self, page: usize, r: PdfRect) {
        let layout = self.layout();
        let Some(pr) = layout.get(page) else { return };
        let g = self.geoms[page];
        let sr = to_screen(&g, self.zoom, pr.min.to_vec2(), r);
        self.reveal = Some((sr.top() - GAP, sr.bottom() + GAP));
    }

    // ---- drawing -----------------------------------------------------------

    pub fn ui(&mut self, ui: &mut Ui, rect: Rect) {
        let p = style::palette(ui.ctx());
        let ppp = ui.ctx().pixels_per_point();
        let family = if style::family_ready(ui.ctx(), PDF_FONT) {
            FontFamily::Name(PDF_FONT.into())
        } else {
            FontFamily::Proportional
        };
        let scroll_id = ("pdf-scroll", self.id);
        for (page, scale, image) in self.arrived.drain(..) {
            let tex = ui.ctx().load_texture(format!("pdf-{}-{page}", self.id), Arc::unwrap_or_clone(image), TextureOptions::LINEAR);
            self.textures.insert(page, PageTex { tex, scale });
        }
        // Fit modes follow the window size.
        let inner_w = rect.width() - widgets::SCROLL_BAR - 2.0 * GAP - 4.0;
        let inner_h = rect.height() - widgets::SCROLL_BAR - 2.0 * GAP - 4.0;
        let (pw, ph) = self.geoms.get(self.current_page).map_or((612.0, 792.0), |g| g.size);
        match self.fit {
            Some(Fit::Width) => self.zoom = (inner_w / pw).clamp(MIN_ZOOM, MAX_ZOOM),
            Some(Fit::Page) => self.zoom = (inner_w / pw).min(inner_h / ph).clamp(MIN_ZOOM, MAX_ZOOM),
            None => {}
        }
        // ⌘-scroll and pinch zoom around the pointer.
        let zoom_delta = ui.input(|i| i.zoom_delta());
        if (zoom_delta - 1.0).abs() > 1e-3 && ui.rect_contains_pointer(rect) {
            let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
            let at = pointer - rect.min;
            self.anchor = Some((self.offset + at, at, self.zoom));
            self.zoom = (self.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            self.fit = None;
        }
        if let Some((point, at, old)) = self.anchor.take() {
            let k = self.zoom / old;
            let off = point * k - at;
            ScrollView::scroll_to(ui.ctx(), scroll_id, off.max(Vec2::ZERO));
        }
        if let Some(page) = self.goto.take() {
            let layout = self.layout();
            if let Some(r) = layout.get(page) {
                ScrollView::scroll_to(ui.ctx(), scroll_id, vec2(self.offset.x, r.top() - GAP));
            }
        }
        if let Some((top, bottom)) = self.reveal.take() {
            ScrollView::reveal(ui.ctx(), scroll_id, top, bottom);
        }

        let bg = style::shade(&p, p.panel_background, 1.18);
        ScrollView::new(scroll_id).horizontal(true).border(false).background(p.panel_background).show_in(
            ui,
            rect,
            |ui, viewport| {
                self.offset = viewport.min.to_vec2();
                self.view = viewport.size();
                let layout = self.layout();
                let size = self.content_size(&layout);
                let origin = ui.max_rect().min;
                ui.allocate_rect(Rect::from_min_size(origin, size.max(viewport.size())), Sense::hover());
                let painter = ui.painter().clone();
                painter.rect_filled(Rect::from_min_size(origin + viewport.min.to_vec2(), viewport.size()), 0.0, bg);
                // The page in the middle of the view is the current one.
                let mid = viewport.center().y;
                if let Some(i) = layout.iter().position(|r| r.bottom() + GAP > mid) {
                    self.current_page = i;
                }
                let near = viewport.expand2(vec2(0.0, viewport.height()));
                let far = viewport.expand2(vec2(0.0, viewport.height() * 4.0));
                let want = self.zoom * ppp;
                for (i, pr) in layout.iter().enumerate() {
                    if !pr.intersects(far) {
                        self.textures.remove(&i);
                        self.asked.remove(&i);
                        continue;
                    }
                    if !pr.intersects(near) {
                        continue;
                    }
                    let have = self.textures.get(&i).map(|t| t.scale);
                    let asked = self.asked.get(&i).copied();
                    let stale = |s: Option<f32>| s.is_none_or(|s| (s / want - 1.0).abs() > 0.02);
                    if stale(have) && stale(asked) {
                        let max_scale = crate::pdfwork::MAX_PX / self.geoms[i].size.0.max(self.geoms[i].size.1).max(1.0);
                        // Past the size cap a new render would look the same.
                        if !(have.is_some_and(|h| h >= max_scale * 0.99) && want > max_scale) {
                            let _ = self.tx.send(Request::Render { page: i, scale: want });
                            self.asked.insert(i, want);
                        }
                    }
                    let sr = pr.translate(origin.to_vec2());
                    if !sr.intersects(ui.clip_rect()) {
                        continue;
                    }
                    // Shadow, then the page (white until it's rendered).
                    painter.rect_filled(sr.translate(vec2(1.0, 1.0)).expand(1.0), 0.0, style::shade(&p, p.panel_background, 1.45));
                    painter.rect_filled(sr, 0.0, Color32::WHITE);
                    if let Some(t) = self.textures.get(&i) {
                        painter.image(t.tex.id(), sr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                }
                self.page_overlays(ui, &layout, origin, &family);
            },
        );
    }

    /// Fields and marks over every visible page, and pointer handling.
    fn page_overlays(&mut self, ui: &mut Ui, layout: &[Rect], origin: Pos2, family: &FontFamily) {
        let clip = ui.clip_rect();
        let zoom = self.zoom;
        let mut clicked_field: Option<(usize, usize)> = None;
        let mut choice_menu: Option<(usize, Rect)> = None;
        // The pages take what nothing on them does, so they go first
        // (egui gives a click to the last thing registered under it).
        let mut pages = Vec::new();
        for (page, pr) in layout.iter().enumerate() {
            let sr = pr.translate(origin.to_vec2());
            if sr.intersects(clip) {
                pages.push((page, sr, ui.interact(sr, ui.make_persistent_id(("page", self.id, page)), Sense::click_and_drag())));
            }
        }
        // Over a mark, the page handles the pointer (to move the mark).
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over_mark = pointer.is_some_and(|pos| {
            self.model.marks.iter().any(|mk| {
                layout.get(mk.page).is_some_and(|pr| {
                    to_screen(&self.geoms[mk.page], zoom, origin.to_vec2() + pr.min.to_vec2(), mk.rect).expand(2.0).contains(pos)
                })
            })
        }) || self.drag.is_some();
        // Fields.
        for (fi, field) in self.model.fields.iter().enumerate() {
            for (wi, w) in field.widgets.iter().enumerate() {
                let Some(pr) = layout.get(w.page) else { continue };
                let g = self.geoms[w.page];
                let r = to_screen(&g, zoom, origin.to_vec2() + pr.min.to_vec2(), w.rect);
                if !r.intersects(clip) {
                    continue;
                }
                let editing_this = self.editing == Some(Editing::Field(fi, wi));
                draw_field(ui, field, w, r, zoom, family, editing_this, self.editable);
                if editing_this || over_mark || self.tool != Tool::Select {
                    continue;
                }
                let resp = ui.interact(r, ui.make_persistent_id(("field", self.id, fi, wi)), Sense::click());
                if !field.read_only && self.editable {
                    let cursor = if matches!(field.kind, FieldKind::Text { .. }) { CursorIcon::Text } else { CursorIcon::PointingHand };
                    let resp = resp.on_hover_cursor(cursor);
                    let resp = match &field.tip {
                        Some(t) if !t.is_empty() => resp.on_hover_text(t),
                        _ => resp,
                    };
                    if resp.clicked() {
                        if matches!(field.kind, FieldKind::Choice { combo: true, .. }) {
                            choice_menu = Some((fi, r));
                        } else if let FieldKind::Choice { combo: false, options, .. } = &field.kind {
                            // A list box: pick the row clicked.
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let size = field_size(field, w, zoom);
                                let row = ((pos.y - r.top() - 1.0) / (size * 1.15)).floor().max(0.0) as usize;
                                if let Some((export, _)) = options.get(row) {
                                    clicked_field = None;
                                    if field.value != Value::Text(export.clone()) {
                                        let v = Value::Text(export.clone());
                                        self.undo.push(self.model.clone());
                                        self.redo.clear();
                                        self.model.fields[fi].value = v;
                                        return;
                                    }
                                }
                            }
                        } else {
                            clicked_field = Some((fi, wi));
                        }
                    }
                }
            }
        }
        if let Some((f, w)) = clicked_field {
            self.start_field(f, w);
        }
        if let Some((fi, r)) = choice_menu {
            self.end_editing();
            if let FieldKind::Choice { options, .. } = &self.model.fields[fi].kind {
                let mut menu = Menu::new("");
                for (k, (_, label)) in options.iter().enumerate() {
                    let current = self.model.fields[fi].value == Value::Text(options[k].0.clone());
                    menu = menu.item(label.clone(), format!("{k}"), None).checked(current);
                }
                widgets::popup_menu_at(ui.ctx(), choice_id(self.id, fi), r.left_bottom(), menu);
            }
        }
        // Choices picked from a popup.
        for fi in 0..self.model.fields.len() {
            if let FieldKind::Choice { combo: true, options, .. } = &self.model.fields[fi].kind
                && let Some(k) = widgets::popup_result(ui.ctx(), choice_id(self.id, fi)).and_then(|s| s.parse::<usize>().ok())
                && let Some((export, _)) = options.get(k)
            {
                let v = Value::Text(export.clone());
                if self.model.fields[fi].value != v {
                    self.push_undo();
                    self.model.fields[fi].value = v;
                }
            }
        }
        // The field being typed in.
        if let Some(Editing::Field(fi, wi)) = self.editing {
            self.field_editor(ui, layout, origin, family, fi, wi);
        }

        // Marks.
        for i in 0..self.model.marks.len() {
            let mark = &self.model.marks[i];
            let Some(pr) = layout.get(mark.page) else { continue };
            let g = self.geoms[mark.page];
            let r = to_screen(&g, zoom, origin.to_vec2() + pr.min.to_vec2(), mark.rect);
            if !r.expand(HANDLE).intersects(clip) {
                continue;
            }
            if self.editing == Some(Editing::Mark(i)) {
                self.mark_editor(ui, r, i, family);
            } else {
                draw_mark(ui.painter(), mark, r, zoom, family);
            }
            if self.sel == Some(Sel::Mark(i)) {
                draw_selection(ui.painter(), r, self.editing != Some(Editing::Mark(i)));
            }
        }

        // The pointer on the pages: the tools, and marks.
        for (page, sr, resp) in pages {
            if !self.editable {
                continue;
            }
            match self.tool {
                Tool::Text => {
                    let resp = resp.on_hover_cursor(CursorIcon::Text);
                    if resp.clicked()
                        && let Some(pos) = resp.interact_pointer_pos()
                    {
                        self.new_text(page, pos - sr.min);
                    }
                }
                Tool::Draw => {
                    let resp = resp.on_hover_cursor(CursorIcon::Crosshair);
                    let g = self.geoms[page];
                    let inv = invert(&g.transform);
                    let user = |pos: Pos2| apply(&inv, (pos.x - sr.left()) / zoom, (pos.y - sr.top()) / zoom);
                    if resp.drag_started() {
                        self.end_editing();
                        self.stroke.clear();
                        self.drag = Some(Drag::Draw { page });
                    }
                    if matches!(self.drag, Some(Drag::Draw { page: p }) if p == page) {
                        if let Some(pos) = resp.interact_pointer_pos().or(ui.input(|i| i.pointer.latest_pos())) {
                            let u = user(pos.clamp(sr.min, sr.max));
                            if self.stroke.last().is_none_or(|l| (l[0] - u[0]).hypot(l[1] - u[1]) * zoom > 1.5) {
                                self.stroke.push(u);
                            }
                        }
                        // The stroke so far.
                        let pts: Vec<Pos2> = self
                            .stroke
                            .iter()
                            .map(|u| {
                                let q = apply(&g.transform, u[0], u[1]);
                                sr.min + vec2(q[0], q[1]) * zoom
                            })
                            .collect();
                        ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5 * zoom, rgb(self.ink))));
                        if resp.drag_stopped() {
                            self.finish_stroke(page);
                        }
                    }
                }
                Tool::Select => self.select_pointer(ui, &resp, page, sr),
            }
        }
        // Keys for the selection.
        if self.editing.is_none() && self.sel.is_some() {
            let (del, esc) = ui.input(|i| (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace), i.key_pressed(Key::Escape)));
            if del {
                self.delete_selection();
            } else if esc {
                self.sel = None;
            }
        }
    }

    /// Select tool: grab, move and resize marks; double-click edits text.
    fn select_pointer(&mut self, ui: &Ui, resp: &egui::Response, page: usize, sr: Rect) {
        let zoom = self.zoom;
        let g = self.geoms[page];
        let inv = invert(&g.transform);
        let user = |pos: Pos2| apply(&inv, (pos.x - sr.left()) / zoom, (pos.y - sr.top()) / zoom);
        let screen = |r: PdfRect| to_screen(&g, zoom, sr.min.to_vec2(), r);
        let hit = |pos: Pos2, marks: &[Mark]| marks.iter().rposition(|m| m.page == page && screen(m.rect).expand(2.0).contains(pos));
        let corner_hit = |pos: Pos2, r: Rect| {
            [r.left_bottom(), r.right_bottom(), r.right_top(), r.left_top()]
                .iter()
                .position(|c| (c.x - pos.x).abs() <= HANDLE && (c.y - pos.y).abs() <= HANDLE)
        };
        if let Some(pos) = resp.hover_pos() {
            let sel_corner = match self.sel {
                Some(Sel::Mark(i)) if self.model.marks.get(i).is_some_and(|m| m.page == page) => {
                    corner_hit(pos, screen(self.model.marks[i].rect))
                }
                _ => None,
            };
            if let Some(k) = sel_corner {
                ui.ctx().set_cursor_icon(if k % 2 == 0 { CursorIcon::ResizeNeSw } else { CursorIcon::ResizeNwSe });
            } else if hit(pos, &self.model.marks).is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::Move);
            }
        }
        if resp.drag_started()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            if let Some(Sel::Mark(i)) = self.sel
                && let Some(mk) = self.model.marks.get(i)
                && mk.page == page
                && let Some(corner) = corner_hit(press, screen(mk.rect))
            {
                self.end_editing();
                self.push_undo();
                self.drag = Some(Drag::Resize { mark: i, corner, start: self.model.marks[i].rect });
            } else if let Some(i) = hit(press, &self.model.marks) {
                if self.editing != Some(Editing::Mark(i)) {
                    self.end_editing();
                }
                self.sel = Some(Sel::Mark(i));
                self.push_undo();
                self.drag = Some(Drag::Move { mark: i, start: self.model.marks[i].rect, grab: user(press) });
            }
        }
        match self.drag {
            Some(Drag::Move { mark, start, grab }) if self.model.marks.get(mark).is_some_and(|m| m.page == page) => {
                if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                    let u = user(pos);
                    let (dx, dy) = (u[0] - grab[0], u[1] - grab[1]);
                    self.model.marks[mark].rect = [start[0] + dx, start[1] + dy, start[2] + dx, start[3] + dy];
                }
                if !ui.input(|i| i.pointer.any_down()) {
                    self.drag = None;
                    if self.model.marks[mark].rect == start {
                        self.undo.pop();
                    }
                }
            }
            Some(Drag::Resize { mark, corner, start }) if self.model.marks.get(mark).is_some_and(|m| m.page == page) => {
                if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                    let u = user(pos);
                    self.resize_mark(mark, corner, start, u);
                }
                if !ui.input(|i| i.pointer.any_down()) {
                    self.drag = None;
                }
            }
            _ => {}
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            match hit(pos, &self.model.marks) {
                Some(i) => {
                    if self.editing != Some(Editing::Mark(i)) {
                        self.end_editing();
                    }
                    self.sel = Some(Sel::Mark(i));
                }
                None => {
                    self.end_editing();
                    self.sel = None;
                }
            }
        }
        if resp.double_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && let Some(i) = hit(pos, &self.model.marks)
            && matches!(self.model.marks[i].kind, MarkKind::Text { .. })
        {
            self.start_mark_text(i);
        }
    }

    fn resize_mark(&mut self, i: usize, corner: usize, start: PdfRect, u: [f32; 2]) {
        let mark = &mut self.model.marks[i];
        let mut r = start;
        // Corners: 0 bottom-left, 1 bottom-right, 2 top-right, 3 top-left
        // (in screen terms; user space is y-up on unrotated pages).
        let (xi, yi) = match corner {
            0 => (0, 1),
            1 => (2, 1),
            2 => (2, 3),
            _ => (0, 3),
        };
        r[xi] = u[0];
        r[yi] = u[1];
        let mut r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
        match &mark.kind {
            MarkKind::Ink { .. } => {
                // Keep the drawing's proportions, anchored at the far corner.
                let aspect = (start[2] - start[0]) / (start[3] - start[1]).max(1e-3);
                let w = (r[2] - r[0]).max(12.0);
                let h = w / aspect;
                let (fx, fy) = (if xi == 0 { start[2] } else { start[0] }, if yi == 1 { start[3] } else { start[1] });
                r = if xi == 0 { [fx - w, 0.0, fx, 0.0] } else { [fx, 0.0, fx + w, 0.0] };
                if yi == 1 {
                    r[1] = fy - h;
                    r[3] = fy;
                } else {
                    r[1] = fy;
                    r[3] = fy + h;
                }
            }
            MarkKind::Text { text, size } => {
                // Width follows the pointer; height follows the text.
                let w = (r[2] - r[0]).max(size * 2.0);
                if xi == 0 {
                    r[0] = start[2] - w;
                    r[2] = start[2];
                } else {
                    r[0] = start[0];
                    r[2] = start[0] + w;
                }
                let h = pdfdoc::text_mark_height(text, *size, w);
                r[3] = start[3];
                r[1] = start[3] - h;
            }
        }
        mark.rect = r;
    }

    fn new_text(&mut self, page: usize, at: Vec2) {
        self.end_editing();
        let g = self.geoms[page];
        let inv = invert(&g.transform);
        let u = apply(&inv, at.x / self.zoom, at.y / self.zoom);
        let size = 12.0;
        let width = 180.0f32.min(g.size.0 - (at.x / self.zoom) - 8.0).max(60.0);
        let h = pdfdoc::text_mark_height("", size, width);
        self.push_undo();
        self.snapshot_taken = true;
        self.model.marks.push(Mark {
            id: None,
            page,
            rect: [u[0], u[1] - h / 2.0, u[0] + width, u[1] + h / 2.0],
            color: self.ink,
            kind: MarkKind::Text { text: String::new(), size },
        });
        let i = self.model.marks.len() - 1;
        self.buffer.clear();
        self.editing = Some(Editing::Mark(i));
        self.sel = Some(Sel::Mark(i));
        self.focus_pending = true;
        self.tool = Tool::Select;
    }

    fn finish_stroke(&mut self, page: usize) {
        self.drag = None;
        let pts = std::mem::take(&mut self.stroke);
        if pts.is_empty() {
            return;
        }
        let width = 1.5;
        let pad = width;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let rect = [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let stroke = pts.iter().map(|p| [(p[0] - rect[0]) / w, (p[1] - rect[1]) / h]).collect();
        self.push_undo();
        self.model.marks.push(Mark {
            id: None,
            page,
            rect,
            color: self.ink,
            kind: MarkKind::Ink { strokes: vec![stroke], width, signature: false },
        });
    }

    /// The text editor over the field being filled in.
    fn field_editor(&mut self, ui: &mut Ui, layout: &[Rect], origin: Pos2, family: &FontFamily, fi: usize, wi: usize) {
        let field = self.model.fields[fi].clone();
        let w = &field.widgets[wi];
        let Some(pr) = layout.get(w.page) else { return };
        let g = self.geoms[w.page];
        let r = to_screen(&g, self.zoom, origin.to_vec2() + pr.min.to_vec2(), w.rect);
        let size = pdfdoc::field_font_size(&field, w.rect, &self.buffer) * self.zoom;
        let font = FontId::new(size.max(4.0), family.clone());
        let pad = 2.0 * self.zoom;
        let (multiline, password, max_len) = match field.kind {
            FieldKind::Text { multiline, password, max_len, .. } => (multiline, password, max_len),
            _ => (false, false, None),
        };
        let id = ui.make_persistent_id(("field-edit", self.id, fi, wi));
        let edit_rect = if multiline { r.shrink(pad) } else { r.shrink2(vec2(pad, 0.0)) };
        ui.painter().rect_filled(r, 0.0, Color32::WHITE);
        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, c(style::palette(ui.ctx()).keyboard_navigation)), egui::StrokeKind::Outside);
        let before = self.buffer.clone();
        let mut te = if multiline { egui::TextEdit::multiline(&mut self.buffer) } else { egui::TextEdit::singleline(&mut self.buffer) };
        te = te
            .id(id)
            .font(font)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .text_color(rgb(field.color))
            .password(password)
            .desired_width(edit_rect.width())
            .min_size(edit_rect.size())
            .align(match (field.align, multiline) {
                (1, false) => egui::Align2::CENTER_CENTER,
                (2, false) => egui::Align2::RIGHT_CENTER,
                (_, false) => egui::Align2::LEFT_CENTER,
                (1, true) => egui::Align2::CENTER_TOP,
                (2, true) => egui::Align2::RIGHT_TOP,
                (_, true) => egui::Align2::LEFT_TOP,
            });
        if let Some(n) = max_len {
            te = te.char_limit(n);
        }
        let resp = ui.put(edit_rect, te);
        widgets::edit_menu(ui, id, &resp, &mut self.buffer, true, password);
        if self.focus_pending {
            resp.request_focus();
            self.focus_pending = false;
        }
        if self.buffer != before && !self.snapshot_taken {
            self.undo.push(self.model.clone());
            self.redo.clear();
            self.snapshot_taken = true;
        }
        let (tab, shift, enter, esc) =
            ui.input(|i| (i.key_pressed(Key::Tab), i.modifiers.shift, i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if tab {
            self.next_field(shift);
        } else if esc || enter && !multiline || resp.lost_focus() && !resp.has_focus() {
            self.end_editing();
        }
    }

    /// The text editor over a text mark.
    fn mark_editor(&mut self, ui: &mut Ui, r: Rect, i: usize, family: &FontFamily) {
        let MarkKind::Text { size, .. } = self.model.marks[i].kind else { return };
        let color = rgb(self.model.marks[i].color);
        let pad = pdfdoc::TEXT_PAD * self.zoom;
        // Grow the box with the text as it is typed.
        let width = self.model.marks[i].rect[2] - self.model.marks[i].rect[0];
        let h = pdfdoc::text_mark_height(&self.buffer, size, width) * self.zoom;
        let r = Rect::from_min_size(r.min, vec2(r.width(), h.max(r.height())));
        let font = FontId::new(size * self.zoom, family.clone());
        let id = ui.make_persistent_id(("mark-edit", self.id, i));
        let before = self.buffer.clone();
        let te = egui::TextEdit::multiline(&mut self.buffer)
            .id(id)
            .font(font)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .text_color(color)
            .desired_width(r.width() - 2.0 * pad)
            .desired_rows(1);
        let resp = ui.put(r.shrink(pad), te);
        widgets::edit_menu(ui, id, &resp, &mut self.buffer, true, false);
        if self.focus_pending {
            resp.request_focus();
            self.focus_pending = false;
        }
        if self.buffer != before && !self.snapshot_taken {
            self.undo.push(self.model.clone());
            self.redo.clear();
            self.snapshot_taken = true;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) || resp.lost_focus() && !resp.has_focus() {
            self.end_editing();
        }
    }
}

fn choice_id(view: u64, field: usize) -> egui::Id {
    egui::Id::unique(("pdf-choice", view, field))
}

fn field_rect(f: &Field, w: usize) -> PdfRect {
    f.widgets[w].rect
}

/// A user-space rect on screen (axis-aligned, any page rotation).
fn to_screen(g: &PageGeom, zoom: f32, page_min: Vec2, r: PdfRect) -> Rect {
    let corners = [[r[0], r[1]], [r[2], r[1]], [r[0], r[3]], [r[2], r[3]]].map(|[x, y]| apply(&g.transform, x, y));
    let x0 = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    let x1 = corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    let y0 = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
    let y1 = corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    Rect::from_min_max(pos2(x0 * zoom, y0 * zoom) + page_min, pos2(x1 * zoom, y1 * zoom) + page_min)
}

/// Font size of a field's text on screen.
fn field_size(field: &Field, w: &pdfdoc::Widget, zoom: f32) -> f32 {
    pdfdoc::field_font_size(field, w.rect, &pdfdoc::display_text(field)) * zoom
}

/// A field as the saved file will show it, with a tint marking it
/// fillable.
#[allow(clippy::too_many_arguments)]
fn draw_field(ui: &Ui, field: &Field, w: &pdfdoc::Widget, r: Rect, zoom: f32, family: &FontFamily, editing: bool, editable: bool) {
    let painter = ui.painter();
    if let Some(bg) = w.background {
        painter.rect_filled(r, 0.0, rgb(bg));
    }
    if editable && !field.read_only && !editing {
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(70, 110, 230, 34));
    }
    if let Some(bc) = w.border {
        painter.rect_stroke(r.shrink(0.5 * zoom), 0.0, Stroke::new(zoom.max(1.0), rgb(bc)), egui::StrokeKind::Inside);
    }
    if editing {
        return;
    }
    let color = rgb(field.color);
    match (&field.kind, &field.value) {
        (FieldKind::Check, Value::State(Some(s))) if w.on.as_deref() == Some(s.as_str()) => {
            let s = r.width().min(r.height());
            let o = r.center() - vec2(s, s) / 2.0;
            let pt = |x: f32, y: f32| o + vec2(x * s, (1.0 - y) * s);
            painter.add(egui::Shape::line(
                vec![pt(0.2, 0.52), pt(0.42, 0.28), pt(0.8, 0.76)],
                Stroke::new((s * 0.12).max(1.0), color),
            ));
        }
        (FieldKind::Radio, Value::State(Some(s))) if w.on.as_deref() == Some(s.as_str()) => {
            painter.circle_filled(r.center(), r.width().min(r.height()) * 0.25, color);
        }
        (FieldKind::Check | FieldKind::Radio, _) => {}
        (FieldKind::Choice { combo: false, options, .. }, Value::Text(v)) => {
            let size = field_size(field, w, zoom);
            let font = FontId::new(size.max(1.0), family.clone());
            let lead = size * 1.15;
            for (k, (export, label)) in options.iter().enumerate() {
                let top = r.top() + 1.0 + k as f32 * lead;
                if top + lead > r.bottom() + 0.5 {
                    break;
                }
                let row = Rect::from_min_size(pos2(r.left() + 1.0, top), vec2(r.width() - 2.0, lead));
                let selected = export == v;
                if selected {
                    painter.rect_filled(row, 0.0, Color32::from_rgb(153, 193, 218));
                }
                let g = painter.layout_no_wrap(label.clone(), font.clone(), color);
                let pos = style::text_origin(ui.ctx(), &g, row.left() + 2.0 * zoom, row.center().y, size);
                painter.galley(pos, g, color);
            }
        }
        _ => {
            let text = pdfdoc::display_text(field);
            let size = field_size(field, w, zoom);
            let font = FontId::new(size.max(1.0), family.clone());
            let pt = size / zoom;
            let x_for = |gw: f32| match field.align {
                1 => r.center().x - gw / 2.0,
                2 => r.right() - 2.0 * zoom - gw,
                _ => r.left() + 2.0 * zoom,
            };
            match field.kind {
                FieldKind::Text { multiline: true, .. } => {
                    let width = w.rect[2] - w.rect[0];
                    for (k, line) in pdfdoc::wrap(&text, pt, width - 4.0).iter().enumerate() {
                        let baseline = r.top() + (2.0 + pt * 0.93 + k as f32 * pt * 1.15) * zoom;
                        let g = painter.layout_no_wrap(line.clone(), font.clone(), color);
                        let pos = style::text_origin(ui.ctx(), &g, x_for(g.size().x), baseline - 0.359 * size, size);
                        painter.galley(pos, g, color);
                    }
                }
                FieldKind::Text { comb: true, max_len: Some(n), .. } if n > 0 => {
                    let cell = r.width() / n as f32;
                    for (k, ch) in text.chars().take(n).enumerate() {
                        let g = painter.layout_no_wrap(ch.to_string(), font.clone(), color);
                        let x = r.left() + cell * k as f32 + (cell - g.size().x) / 2.0;
                        let pos = style::text_origin(ui.ctx(), &g, x, r.center().y, size);
                        painter.galley(pos, g, color);
                    }
                }
                _ => {
                    let g = painter.layout_no_wrap(text.clone(), font, color);
                    let pos = style::text_origin(ui.ctx(), &g, x_for(g.size().x), r.center().y, size);
                    painter.with_clip_rect(r.intersect(painter.clip_rect())).galley(pos, g, color);
                }
            }
            if matches!(field.kind, FieldKind::Choice { combo: true, .. }) && editable {
                // The pop-up arrow.
                let a = r.height().min(14.0 * zoom) * 0.3;
                let cx = r.right() - a * 2.0;
                let cy = r.center().y;
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(cx - a, cy - a / 2.0), pos2(cx + a, cy - a / 2.0), pos2(cx, cy + a / 2.0)],
                    Color32::from_gray(90),
                    Stroke::NONE,
                ));
            }
        }
    }
}

fn draw_mark(painter: &egui::Painter, mark: &Mark, r: Rect, zoom: f32, family: &FontFamily) {
    let color = rgb(mark.color);
    match &mark.kind {
        MarkKind::Text { text, size } => {
            let font = FontId::new(size * zoom, family.clone());
            let width = mark.rect[2] - mark.rect[0];
            for (k, line) in pdfdoc::wrap(text, *size, width - 2.0 * pdfdoc::TEXT_PAD).iter().enumerate() {
                let top = r.top() + (pdfdoc::TEXT_PAD + k as f32 * size * 1.15) * zoom;
                let g = painter.layout_no_wrap(line.clone(), font.clone(), color);
                painter.galley(pos2(r.left() + pdfdoc::TEXT_PAD * zoom, top), g, color);
            }
        }
        MarkKind::Ink { strokes, width, .. } => {
            let sw = width * zoom;
            for s in strokes {
                let pts: Vec<Pos2> = s.iter().map(|p| pos2(r.left() + p[0] * r.width(), r.bottom() - p[1] * r.height())).collect();
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], sw / 2.0, color);
                    continue;
                }
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(sw, color)));
                // Round caps.
                for p in [pts[0], pts[pts.len() - 1]] {
                    painter.circle_filled(p, sw / 2.0, color);
                }
            }
        }
    }
}

/// A dotted frame with corner handles.
fn draw_selection(painter: &egui::Painter, r: Rect, handles: bool) {
    let blue = Color32::from_rgb(52, 120, 246);
    let r = r.expand(2.0);
    for (a, b) in [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())] {
        painter.add(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, blue), 3.0, 2.0));
    }
    if handles {
        for p in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()] {
            let h = Rect::from_center_size(p, vec2(HANDLE, HANDLE));
            painter.rect_filled(h, 0.0, Color32::WHITE);
            painter.rect_stroke(h, 0.0, Stroke::new(1.0, blue), egui::StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_round_trip() {
        let t = [0.0, 1.0, -1.0, 0.0, 792.0, 0.0];
        let inv = invert(&t);
        let p = apply(&t, 100.0, 200.0);
        let q = apply(&inv, p[0], p[1]);
        assert!((q[0] - 100.0).abs() < 1e-3 && (q[1] - 200.0).abs() < 1e-3);
    }
}
