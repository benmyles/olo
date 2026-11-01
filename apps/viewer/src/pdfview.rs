//! A PDF in a Viewer window: pages stacked in a scroll view at any zoom,
//! rendered by the worker for the device scale on demand (a texture
//! pixel to a device pixel, in tiles a few of which go to the GPU a
//! frame), with the form fields and marks drawn (and edited) on top.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, mpsc::Sender},
};

use olo_ui::{
    Menu, agent,
    gpui::{
        self, App, Bounds, Context, CursorStyle, Entity, Focusable as _, Font, FontWeight, Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollHandle, ScrollWheelEvent, Size, Styled as _, Subscription, Window, actions, canvas, div, point, px, size,
    },
    style::{self, Align},
    theme,
    widgets::{SCROLL_BAR, ScrollView, TextField, TextFieldEvent, text_field},
};

use crate::{
    pdfdoc::{self, Field, FieldKind, Mark, MarkKind, Model, PageGeom, PdfRect, Value},
    pdfwork::{Event, Request, Tile},
};

/// Space around and between pages, in points.
const GAP: f32 = 12.0;
/// Grab handles on a selected mark.
const HANDLE: f32 = 7.0;
pub const MIN_ZOOM: f32 = 0.1;
/// Narrowest a PDF window gets: the toolbar's width.
pub const MIN_WIDTH: f32 = 560.0;
pub const MAX_ZOOM: f32 = 8.0;
/// New tiles sent to the GPU a frame at most (bytes): the rest the next.
const UPLOAD_BUDGET: usize = 12 << 20;
/// Faces with Helvetica's metrics, for field values and text boxes (what
/// is typed wraps as it will in the saved file).
const PDF_FACES: [&str; 4] = ["Liberation Sans", "Arimo", "Nimbus Sans", "Helvetica"];

actions!(olo_pdf_field, [NextField, PrevField, EndEdit]);

const FIELD_CONTEXT: &str = "OloPdfField";

/// Tab and ⇧Tab go from field to field; Escape ends the typing.
pub fn bind_keys(cx: &mut App) {
    let c = Some(FIELD_CONTEXT);
    cx.bind_keys([KeyBinding::new("tab", NextField, c), KeyBinding::new("shift-tab", PrevField, c), KeyBinding::new("escape", EndEdit, c)]);
}

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
    Move { mark: usize, start: PdfRect, grab: [f32; 2], moved: bool },
    /// Resizing from corner `corner` (0 = bottom-left, then counter-clockwise).
    Resize { mark: usize, corner: usize, start: PdfRect },
    Draw { page: usize },
}

/// A page as rendered: at what scale, its size in pixels, its tiles and
/// which of them have gone to the GPU.
struct PageImg {
    scale: f32,
    size: (u32, u32),
    tiles: Vec<Tile>,
    uploaded: Vec<bool>,
}

pub struct PdfView {
    tx: Sender<Request>,
    pub geoms: Vec<PageGeom>,
    pub model: Model,
    /// The model as last saved (or opened).
    saved: Model,
    pub editable: bool,
    pages: HashMap<usize, PageImg>,
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
    editor: Option<(Entity<TextField>, Vec<Subscription>)>,
    drag: Option<Drag>,
    stroke: Vec<[f32; 2]>,
    undo: Vec<Model>,
    redo: Vec<Model>,
    /// An edit session pushed its undo snapshot already.
    snapshot_taken: bool,
    pub current_page: usize,
    scroll: ScrollHandle,
    /// The view's size and how far it is scrolled (points), last frame.
    view: Size<f32>,
    offset: Point<f32>,
    /// Keep this content point (relative to the view) still across a zoom.
    anchor: Option<(Point<f32>, Point<f32>, f32)>,
    /// Scroll to the top of this page next frame.
    goto: Option<usize>,
    /// Bring this content range into view next frame.
    reveal: Option<(f32, f32)>,
    pub saving: bool,
    /// Where the content's top left was drawn (window points).
    origin: Point<Pixels>,
    /// Images to take out of the window's atlas.
    dropped: Vec<Arc<RenderImage>>,
    font: Font,
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

fn rgb(c3: [f32; 3]) -> Hsla {
    gpui::Rgba { r: c3[0], g: c3[1], b: c3[2], a: 1.0 }.into()
}

/// A rectangle in content points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x0 && p[0] < self.x1 && p[1] >= self.y0 && p[1] < self.y1
    }
    fn expand(&self, d: f32) -> Rect {
        Rect { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }
    fn intersects(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    fn on(&self, o: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::from_corners(point(o.x + px(self.x0), o.y + px(self.y0)), point(o.x + px(self.x1), o.y + px(self.y1)))
    }
}

/// Ink colours offered in the Tools menu.
pub const INKS: [(&str, [f32; 3]); 3] = [("Black", [0.0, 0.0, 0.0]), ("Blue", [0.05, 0.2, 0.6]), ("Red", [0.75, 0.08, 0.08])];

impl PdfView {
    pub fn new(tx: Sender<Request>, cx: &App) -> Self {
        let names = cx.text_system().all_font_names();
        let family = PDF_FACES.iter().find(|f| names.iter().any(|n| n == *f)).map_or_else(|| style::UI_FONT.to_owned(), |f| (*f).to_owned());
        PdfView {
            tx,
            geoms: Vec::new(),
            model: Model::default(),
            saved: Model::default(),
            editable: false,
            pages: HashMap::new(),
            asked: HashMap::new(),
            zoom: 1.0,
            fit: Some(Fit::Width),
            tool: Tool::Select,
            ink: INKS[0].1,
            sel: None,
            editing: None,
            buffer: String::new(),
            editor: None,
            drag: None,
            stroke: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            snapshot_taken: false,
            current_page: 0,
            scroll: ScrollHandle::new(),
            view: size(0.0, 0.0),
            offset: point(0.0, 0.0),
            anchor: None,
            goto: None,
            reveal: None,
            saving: false,
            origin: point(px(0.0), px(0.0)),
            dropped: Vec::new(),
            font: Font { family: family.into(), ..style::ui_font(FontWeight::NORMAL) },
        }
    }

    pub fn loaded(&self) -> bool {
        !self.geoms.is_empty()
    }

    /// Apply a worker event; true when the window should resize (opened).
    pub fn event(&mut self, ev: Event, cx: &mut Context<Self>) -> bool {
        let opened = match ev {
            Event::Opened { geoms, model, editable } => {
                self.geoms = geoms;
                self.saved = model.clone();
                self.model = model;
                self.editable = editable;
                true
            }
            Event::Page { page, scale, size, tiles } => {
                let uploaded = vec![false; tiles.len()];
                if let Some(old) = self.pages.insert(page, PageImg { scale, size, tiles, uploaded }) {
                    self.dropped.extend(old.tiles.into_iter().map(|t| t.image));
                }
                false
            }
            Event::Saved { adopted, .. } => {
                self.saving = false;
                if let Some(model) = adopted {
                    // Same values; the marks now have object ids.
                    self.end_editing(cx);
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
        };
        cx.notify();
        opened
    }

    pub fn dirty(&self) -> bool {
        self.model.fields != self.saved.fields || self.model.marks != self.saved.marks || self.editing_changed()
    }

    fn editing_changed(&self) -> bool {
        match self.editing {
            Some(Editing::Field(f, _)) => self.model.fields.get(f).is_some_and(|f| f.value != Value::Text(self.buffer.clone())),
            Some(Editing::Mark(i)) => self.model.marks.get(i).is_some_and(|m| !matches!(&m.kind, MarkKind::Text { text, .. } if *text == self.buffer)),
            None => false,
        }
    }

    /// Ask the worker to save (the edit in progress included).
    pub fn save(&mut self, to: PathBuf, flatten: bool, adopt: bool, cx: &mut Context<Self>) {
        self.end_editing(cx);
        self.saving = true;
        let _ = self.tx.send(Request::Save { model: self.model.clone(), to, flatten, adopt });
    }

    /// The window size that shows the first page whole, within the screen.
    pub fn ideal_size(&mut self, screen: Option<(f32, f32)>, chrome: f32) -> (f32, f32) {
        let (sw, sh) = screen.unwrap_or((1440.0, 900.0));
        let (pw, ph) = self.geoms.first().map_or((612.0, 792.0), |g| g.size);
        let zoom = ((sh * 0.86 - chrome - 2.0 * GAP) / ph).min((sw * 0.8 - 2.0 * GAP - SCROLL_BAR) / pw).clamp(0.3, 1.5);
        self.zoom = zoom;
        let w = pw * zoom + 2.0 * GAP + SCROLL_BAR + 4.0;
        let h = ph * zoom + 2.0 * GAP + chrome + 4.0;
        (w.round().max(MIN_WIDTH), h.round().max(240.0))
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

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.end_editing(cx);
        if let Some(m) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.model, m));
            self.sel = None;
        }
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.end_editing(cx);
        if let Some(m) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.model, m));
            self.sel = None;
        }
        cx.notify();
    }

    pub fn has_selection(&self) -> bool {
        self.sel.is_some() && self.editing.is_none()
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if let (Some(Sel::Mark(i)), None) = (self.sel, self.editing)
            && i < self.model.marks.len()
        {
            self.push_undo();
            self.model.marks.remove(i);
            self.sel = None;
            cx.notify();
        }
    }

    /// Commit what is being typed.
    fn end_editing(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.editing.take() else { return };
        self.editor = None;
        let text = std::mem::take(&mut self.buffer);
        cx.notify();
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

    /// The field's text editor, for what's typed in `editing`.
    fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (size, color, multiline, password, max_len, align, name) = match self.editing {
            Some(Editing::Field(fi, wi)) => {
                let field = &self.model.fields[fi];
                let w = &field.widgets[wi];
                let (multiline, password, max_len) = match field.kind {
                    FieldKind::Text { multiline, password, max_len, .. } => (multiline, password, max_len),
                    _ => (false, false, None),
                };
                let align = if field.align == 2 { olo_ui::widgets::text_field::Align::Right } else { olo_ui::widgets::text_field::Align::Left };
                (pdfdoc::field_font_size(field, w.rect, &self.buffer) * self.zoom, rgb(field.color), multiline, password, max_len, align, field.name.clone())
            }
            Some(Editing::Mark(i)) => {
                let MarkKind::Text { size, .. } = self.model.marks[i].kind else { return };
                (size * self.zoom, rgb(self.model.marks[i].color), true, false, None, olo_ui::widgets::text_field::Align::Left, "Text Box".to_owned())
            }
            None => return,
        };
        let font = self.font.clone();
        let text = self.buffer.clone();
        let editor = text_field(cx, move |f| {
            let mut f = f.font(font).font_size(size.max(4.0)).color(color).align(align).password(password).name(name);
            if multiline {
                f = f.multiline(false);
            }
            if let Some(n) = max_len {
                f = f.max_len(n);
            }
            f
        });
        editor.update(cx, |f, cx| f.set_text(text, cx));
        let changed = cx.subscribe(&editor, |v: &mut PdfView, e, ev: &TextFieldEvent, cx| match ev {
            TextFieldEvent::Changed => {
                let now = e.read(cx).text().to_owned();
                if now != v.buffer && !v.snapshot_taken {
                    v.undo.push(v.model.clone());
                    v.redo.clear();
                    v.snapshot_taken = true;
                }
                v.buffer = now;
                cx.notify();
            }
            TextFieldEvent::Entered => v.end_editing(cx),
            TextFieldEvent::Typed(_) => {}
        });
        let focus = editor.read(cx).focus_handle(cx);
        window.focus(&focus);
        let blurred = cx.on_blur(&focus, window, |v: &mut PdfView, _, cx| v.end_editing(cx));
        self.editor = Some((editor, vec![changed, blurred]));
    }

    fn start_field(&mut self, f: usize, w: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.end_editing(cx);
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
                self.snapshot_taken = false;
                self.open_editor(window, cx);
            }
            _ => {}
        }
        self.sel = None;
        cx.notify();
    }

    fn start_mark_text(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.end_editing(cx);
        if let Some(Mark { kind: MarkKind::Text { text, .. }, .. }) = self.model.marks.get(i) {
            self.buffer = text.clone();
            self.editing = Some(Editing::Mark(i));
            self.sel = Some(Sel::Mark(i));
            self.snapshot_taken = false;
            self.open_editor(window, cx);
        }
    }

    /// Tab: the next editable text field after the one being edited.
    fn next_field(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Editing::Field(f, _)) = self.editing else { return };
        let n = self.model.fields.len();
        for k in 1..=n {
            let i = if back { (f + n - k) % n } else { (f + k) % n };
            let field = &self.model.fields[i];
            if !field.read_only && matches!(field.kind, FieldKind::Text { .. } | FieldKind::Choice { editable: true, .. }) {
                let page = field.widgets[0].page;
                let rect = field.widgets[0].rect;
                self.start_field(i, 0, window, cx);
                self.goto_rect(page, rect);
                return;
            }
        }
    }

    /// Add a signature (normalised strokes and aspect) to the middle of
    /// the visible part of the current page.
    pub fn place_signature(&mut self, strokes: Vec<Vec<[f32; 2]>>, aspect: f32, cx: &mut Context<Self>) {
        if !self.editable || self.geoms.is_empty() {
            return;
        }
        self.end_editing(cx);
        let page = self.current_page.min(self.geoms.len() - 1);
        let g = self.geoms[page];
        let width = (g.size.0 * 0.3).clamp(80.0, 200.0);
        let height = width / aspect.max(0.1);
        // The centre of what's visible of that page, in user space.
        let pr = self.layout()[page];
        let view = Rect { x0: self.offset.x, y0: self.offset.y, x1: self.offset.x + self.view.width, y1: self.offset.y + self.view.height };
        let vis = Rect { x0: view.x0.max(pr.x0), y0: view.y0.max(pr.y0), x1: view.x1.min(pr.x1), y1: view.y1.min(pr.y1) };
        let centre = if vis.w() > 0.0 && vis.h() > 0.0 { [(vis.x0 + vis.x1) / 2.0, (vis.y0 + vis.y1) / 2.0] } else { [(pr.x0 + pr.x1) / 2.0, (pr.y0 + pr.y1) / 2.0] };
        let inv = invert(&g.transform);
        let p = apply(&inv, (centre[0] - pr.x0) / self.zoom, (centre[1] - pr.y0) / self.zoom);
        let rect = [p[0] - width / 2.0, p[1] - height / 2.0, p[0] + width / 2.0, p[1] + height / 2.0];
        self.push_undo();
        self.model.marks.push(Mark { id: None, page, rect, color: self.ink, kind: MarkKind::Ink { strokes, width: 1.6, signature: true } });
        self.sel = Some(Sel::Mark(self.model.marks.len() - 1));
        self.tool = Tool::Select;
        cx.notify();
    }

    pub fn set_tool(&mut self, t: Tool, cx: &mut Context<Self>) {
        self.end_editing(cx);
        self.tool = t;
        self.sel = None;
        cx.notify();
    }

    // ---- zoom and pages ----------------------------------------------------

    pub fn set_zoom(&mut self, z: f32, fit: Option<Fit>, cx: &mut Context<Self>) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        // Keep the middle of the view where it is.
        let mid = point(self.view.width / 2.0, self.view.height / 2.0);
        self.anchor = Some((point(self.offset.x + mid.x, self.offset.y + mid.y), mid, self.zoom));
        self.zoom = z;
        self.fit = fit;
        self.end_editing(cx);
        cx.notify();
    }

    pub fn set_fit(&mut self, fit: Fit, cx: &mut Context<Self>) {
        self.fit = Some(fit);
        cx.notify();
    }

    pub fn zoom_step(&mut self, up: bool, cx: &mut Context<Self>) {
        const STEPS: [f32; 13] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];
        let z = if up {
            STEPS.iter().copied().find(|s| *s > self.zoom + 0.01).unwrap_or(MAX_ZOOM)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < self.zoom - 0.01).unwrap_or(MIN_ZOOM)
        };
        self.set_zoom(z, None, cx);
    }

    pub fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if !self.geoms.is_empty() {
            self.goto = Some(page.min(self.geoms.len() - 1));
            cx.notify();
        }
    }

    /// Page rects in content coordinates at the current zoom.
    fn layout(&self) -> Vec<Rect> {
        let widest = self.geoms.iter().map(|g| g.size.0).fold(0.0, f32::max) * self.zoom;
        let content_w = (widest + 2.0 * GAP).max(self.view.width);
        let mut y = GAP;
        self.geoms
            .iter()
            .map(|g| {
                let (w, h) = ((g.size.0 * self.zoom).round(), (g.size.1 * self.zoom).round());
                let x = ((content_w - w) / 2.0).round();
                let r = Rect { x0: x, y0: y.round(), x1: x + w, y1: y.round() + h };
                y += g.size.1 * self.zoom + GAP;
                r
            })
            .collect()
    }

    fn content_size(&self, layout: &[Rect]) -> Size<f32> {
        let w = layout.iter().map(Rect::w).fold(0.0, f32::max) + 2.0 * GAP;
        let h = layout.last().map_or(0.0, |r| r.y1 + GAP);
        size(w.max(self.view.width), h.max(self.view.height))
    }

    fn goto_rect(&mut self, page: usize, r: PdfRect) {
        let layout = self.layout();
        let Some(pr) = layout.get(page) else { return };
        let sr = to_screen(&self.geoms[page], self.zoom, (pr.x0, pr.y0), r);
        self.reveal = Some((sr.y0 - GAP, sr.y1 + GAP));
    }

    /// The content point under window point `at`.
    fn content_at(&self, at: Point<Pixels>) -> [f32; 2] {
        [f32::from(at.x - self.origin.x), f32::from(at.y - self.origin.y)]
    }

    /// The page under content point `at`.
    fn page_at(&self, at: [f32; 2]) -> Option<(usize, Rect)> {
        self.layout().into_iter().enumerate().find(|(_, r)| r.contains(at))
    }

    /// The mark on `page` under content point `at` (the topmost).
    fn mark_at(&self, page: usize, pr: Rect, at: [f32; 2]) -> Option<usize> {
        let g = self.geoms[page];
        self.model.marks.iter().rposition(|mk| mk.page == page && to_screen(&g, self.zoom, (pr.x0, pr.y0), mk.rect).expand(2.0).contains(at))
    }

    /// Which corner of the selected mark (on `page`) is under `at`.
    fn corner_at(&self, page: usize, pr: Rect, at: [f32; 2]) -> Option<(usize, usize)> {
        let Some(Sel::Mark(i)) = self.sel else { return None };
        let mk = self.model.marks.get(i).filter(|mk| mk.page == page)?;
        let r = to_screen(&self.geoms[page], self.zoom, (pr.x0, pr.y0), mk.rect).expand(2.0);
        let corners = [(r.x0, r.y1), (r.x1, r.y1), (r.x1, r.y0), (r.x0, r.y0)];
        corners.iter().position(|c| (c.0 - at[0]).abs() <= HANDLE && (c.1 - at[1]).abs() <= HANDLE).map(|k| (i, k))
    }

    /// The field widget under `at` (one that can be filled in).
    fn field_at(&self, at: [f32; 2]) -> Option<(usize, usize, Rect)> {
        let layout = self.layout();
        for (fi, field) in self.model.fields.iter().enumerate() {
            for (wi, w) in field.widgets.iter().enumerate() {
                let Some(pr) = layout.get(w.page) else { continue };
                let r = to_screen(&self.geoms[w.page], self.zoom, (pr.x0, pr.y0), w.rect);
                if r.contains(at) && !field.read_only && self.editable {
                    return Some((fi, wi, r));
                }
            }
        }
        None
    }

    fn user_point(&self, page: usize, pr: Rect, at: [f32; 2]) -> [f32; 2] {
        let inv = invert(&self.geoms[page].transform);
        apply(&inv, (at[0] - pr.x0) / self.zoom, (at[1] - pr.y0) / self.zoom)
    }

    // ---- the pointer -------------------------------------------------------

    fn press(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.content_at(e.position);
        let Some((page, pr)) = self.page_at(at) else {
            self.end_editing(cx);
            self.sel = None;
            cx.notify();
            return;
        };
        // On a mark (or a selected one's corner), the mark first.
        let on_mark = self.tool == Tool::Select && (self.corner_at(page, pr, at).is_some() || self.mark_at(page, pr, at).is_some());
        if !on_mark && self.tool == Tool::Select {
            if let Some((fi, wi, r)) = self.field_at(at) {
                self.click_field(fi, wi, r, at, window, cx);
                return;
            }
        }
        if !self.editable {
            return;
        }
        match self.tool {
            Tool::Text => {
                self.new_text(page, pr, at, window, cx);
            }
            Tool::Draw => {
                self.end_editing(cx);
                self.stroke.clear();
                self.stroke.push(self.user_point(page, pr, at));
                self.drag = Some(Drag::Draw { page });
            }
            Tool::Select => {
                if let Some((i, corner)) = self.corner_at(page, pr, at) {
                    self.end_editing(cx);
                    self.push_undo();
                    self.drag = Some(Drag::Resize { mark: i, corner, start: self.model.marks[i].rect });
                } else if let Some(i) = self.mark_at(page, pr, at) {
                    if self.editing != Some(Editing::Mark(i)) {
                        self.end_editing(cx);
                    }
                    self.sel = Some(Sel::Mark(i));
                    if e.click_count >= 2 && matches!(self.model.marks[i].kind, MarkKind::Text { .. }) {
                        self.start_mark_text(i, window, cx);
                    } else {
                        self.push_undo();
                        self.drag = Some(Drag::Move { mark: i, start: self.model.marks[i].rect, grab: self.user_point(page, pr, at), moved: false });
                    }
                } else {
                    self.end_editing(cx);
                    self.sel = None;
                }
            }
        }
        cx.notify();
    }

    fn click_field(&mut self, fi: usize, wi: usize, r: Rect, at: [f32; 2], window: &mut Window, cx: &mut Context<Self>) {
        let field = self.model.fields[fi].clone();
        match &field.kind {
            FieldKind::Choice { combo: true, options, .. } => {
                // The pop-up's choices, under the field.
                self.end_editing(cx);
                let mut menu = Menu::new("");
                for (k, (export, label)) in options.iter().enumerate() {
                    menu = menu.item(label.clone(), format!("{k}")).checked(field.value == Value::Text(export.clone()));
                }
                let me = cx.entity();
                let anchor = r.on(self.origin);
                olo_ui::menu::popup(window, cx, menu, anchor, gpui::Corner::BottomLeft, move |cmd, _, cx| {
                    me.update(cx, |v, cx| {
                        if let Some(FieldKind::Choice { options, .. }) = v.model.fields.get(fi).map(|f| &f.kind)
                            && let Some((export, _)) = cmd.parse::<usize>().ok().and_then(|k| options.get(k))
                        {
                            let value = Value::Text(export.clone());
                            if v.model.fields[fi].value != value {
                                v.push_undo();
                                v.model.fields[fi].value = value;
                                cx.notify();
                            }
                        }
                    })
                });
            }
            FieldKind::Choice { combo: false, options, .. } => {
                // A list box: the row clicked.
                let size = field_size(&field, &field.widgets[wi], self.zoom);
                let row = ((at[1] - r.y0 - 1.0) / (size * 1.15)).floor().max(0.0) as usize;
                if let Some((export, _)) = options.get(row) {
                    let value = Value::Text(export.clone());
                    if field.value != value {
                        self.push_undo();
                        self.model.fields[fi].value = value;
                        cx.notify();
                    }
                }
            }
            _ => self.start_field(fi, wi, window, cx),
        }
    }

    fn moved(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let at = self.content_at(e.position);
        let layout = self.layout();
        match self.drag {
            Some(Drag::Draw { page }) => {
                let pr = layout[page];
                let at = [at[0].clamp(pr.x0, pr.x1), at[1].clamp(pr.y0, pr.y1)];
                let u = self.user_point(page, pr, at);
                if self.stroke.last().is_none_or(|l| (l[0] - u[0]).hypot(l[1] - u[1]) * self.zoom > 1.5) {
                    self.stroke.push(u);
                    cx.notify();
                }
            }
            Some(Drag::Move { mark, start, grab, .. }) => {
                let page = self.model.marks[mark].page;
                let u = self.user_point(page, layout[page], at);
                let (dx, dy) = (u[0] - grab[0], u[1] - grab[1]);
                self.model.marks[mark].rect = [start[0] + dx, start[1] + dy, start[2] + dx, start[3] + dy];
                self.drag = Some(Drag::Move { mark, start, grab, moved: true });
                cx.notify();
            }
            Some(Drag::Resize { mark, corner, start }) => {
                let page = self.model.marks[mark].page;
                let u = self.user_point(page, layout[page], at);
                self.resize_mark(mark, corner, start, u);
                cx.notify();
            }
            None => {}
        }
    }

    fn released(&mut self, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Draw { page }) => self.finish_stroke(page),
            Some(Drag::Move { moved: false, .. }) => {
                // A click: nothing moved, nothing to undo.
                self.undo.pop();
            }
            _ => {}
        }
        cx.notify();
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if !(e.modifiers.control || e.modifiers.platform) {
            return;
        }
        // ⌘ and the wheel zoom about the pointer.
        let d = match e.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * 40.0,
        };
        let at = point(f32::from(e.position.x - self.origin.x) - self.offset.x, f32::from(e.position.y - self.origin.y) - self.offset.y);
        self.anchor = Some((point(self.offset.x + at.x, self.offset.y + at.y), at, self.zoom));
        self.zoom = (self.zoom * 1.0025f32.powf(d)).clamp(MIN_ZOOM, MAX_ZOOM);
        self.fit = None;
        self.end_editing(cx);
        cx.stop_propagation();
        cx.notify();
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

    fn new_text(&mut self, page: usize, pr: Rect, at: [f32; 2], window: &mut Window, cx: &mut Context<Self>) {
        self.end_editing(cx);
        let g = self.geoms[page];
        let u = self.user_point(page, pr, at);
        let size = 12.0;
        let width = 180.0f32.min(g.size.0 - ((at[0] - pr.x0) / self.zoom) - 8.0).max(60.0);
        let h = pdfdoc::text_mark_height("", size, width);
        self.push_undo();
        self.snapshot_taken = true;
        self.model.marks.push(Mark { id: None, page, rect: [u[0], u[1] - h / 2.0, u[0] + width, u[1] + h / 2.0], color: self.ink, kind: MarkKind::Text { text: String::new(), size } });
        let i = self.model.marks.len() - 1;
        self.buffer.clear();
        self.editing = Some(Editing::Mark(i));
        self.sel = Some(Sel::Mark(i));
        self.tool = Tool::Select;
        self.open_editor(window, cx);
    }

    fn finish_stroke(&mut self, page: usize) {
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
        self.model.marks.push(Mark { id: None, page, rect, color: self.ink, kind: MarkKind::Ink { strokes: vec![stroke], width, signature: false } });
    }

    // ---- drawing -----------------------------------------------------------

    /// Before drawing: the zoom for a fit mode, scrolls asked for, pages
    /// to render; the hitboxes the pointer's shape follows.
    fn prepaint(&mut self, b: Bounds<Pixels>, window: &mut Window) -> Vec<(Hitbox, CursorStyle)> {
        let s = window.scale_factor();
        self.origin = point(px((f32::from(b.origin.x) * s).round() / s), px((f32::from(b.origin.y) * s).round() / s));
        let view = self.scroll.bounds();
        self.view = size(f32::from(view.size.width), f32::from(view.size.height));
        let off = self.scroll.offset();
        self.offset = point(-f32::from(off.x), -f32::from(off.y));
        let layout = self.layout();
        // The page in the middle of the view is the current one.
        let mid = self.offset.y + self.view.height / 2.0;
        if let Some(i) = layout.iter().position(|r| r.y1 + GAP > mid) {
            self.current_page = i;
        }
        let near = Rect { x0: self.offset.x, y0: self.offset.y - self.view.height, x1: self.offset.x + self.view.width, y1: self.offset.y + 2.0 * self.view.height };
        let far = Rect { x0: f32::MIN, y0: self.offset.y - 4.0 * self.view.height, x1: f32::MAX, y1: self.offset.y + 5.0 * self.view.height };
        let want = self.zoom * s;
        for (i, pr) in layout.iter().enumerate() {
            if !pr.intersects(&far) {
                if let Some(old) = self.pages.remove(&i) {
                    self.dropped.extend(old.tiles.into_iter().map(|t| t.image));
                }
                self.asked.remove(&i);
                continue;
            }
            if !pr.intersects(&near) {
                continue;
            }
            let have = self.pages.get(&i).map(|p| p.scale);
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
        }
        // The pointer's shape over what's on the pages.
        let mut hits = Vec::new();
        let o = self.origin;
        let visible = Rect { x0: self.offset.x, y0: self.offset.y, x1: self.offset.x + self.view.width, y1: self.offset.y + self.view.height };
        let page_cursor = match (self.editable, self.tool) {
            (true, Tool::Text) => Some(CursorStyle::IBeam),
            (true, Tool::Draw) => Some(CursorStyle::Crosshair),
            _ => None,
        };
        for (i, pr) in layout.iter().enumerate() {
            if !pr.intersects(&visible) {
                continue;
            }
            if let Some(c) = page_cursor {
                hits.push((window.insert_hitbox(pr.on(o), HitboxBehavior::Normal), c));
                continue;
            }
            if self.tool != Tool::Select {
                continue;
            }
            for field in &self.model.fields {
                for w in field.widgets.iter().filter(|w| w.page == i) {
                    if field.read_only || !self.editable {
                        continue;
                    }
                    let r = to_screen(&self.geoms[i], self.zoom, (pr.x0, pr.y0), w.rect);
                    let c = if matches!(field.kind, FieldKind::Text { .. }) { CursorStyle::IBeam } else { CursorStyle::PointingHand };
                    hits.push((window.insert_hitbox(r.on(o), HitboxBehavior::Normal), c));
                }
            }
            if self.editable {
                for (k, mk) in self.model.marks.iter().enumerate().filter(|(_, mk)| mk.page == i) {
                    let r = to_screen(&self.geoms[i], self.zoom, (pr.x0, pr.y0), mk.rect).expand(2.0);
                    hits.push((window.insert_hitbox(r.on(o), HitboxBehavior::Normal), CursorStyle::OpenHand));
                    if self.sel == Some(Sel::Mark(k)) && self.editing != Some(Editing::Mark(k)) {
                        for (n, (x, y)) in [(r.x0, r.y1), (r.x1, r.y1), (r.x1, r.y0), (r.x0, r.y0)].into_iter().enumerate() {
                            let h = Rect { x0: x - HANDLE, y0: y - HANDLE, x1: x + HANDLE, y1: y + HANDLE };
                            let c = if n % 2 == 0 { CursorStyle::ResizeUpRightDownLeft } else { CursorStyle::ResizeUpLeftDownRight };
                            hits.push((window.insert_hitbox(h.on(o), HitboxBehavior::Normal), c));
                        }
                    }
                }
            }
        }
        hits
    }

    fn paint(&mut self, hits: Vec<(Hitbox, CursorStyle)>, window: &mut Window, cx: &mut Context<Self>) {
        for img in self.dropped.drain(..) {
            let _ = window.drop_image(img);
        }
        for (hit, c) in &hits {
            window.set_cursor_style(*c, hit);
        }
        let p = theme::palette(cx);
        let s = window.scale_factor();
        let o = self.origin;
        let visible = Rect { x0: self.offset.x, y0: self.offset.y, x1: self.offset.x + self.view.width, y1: self.offset.y + self.view.height };
        window.paint_quad(gpui::fill(visible.on(o), style::shade(&p, p.panel_background, 1.18)));
        let layout = self.layout();
        let want = self.zoom * s;
        let mut budget = UPLOAD_BUDGET;
        let mut more = false;
        for (i, pr) in layout.iter().enumerate() {
            if !pr.intersects(&visible) {
                continue;
            }
            let pb = pr.on(o);
            // Shadow, then the page (white until it's rendered).
            window.paint_quad(gpui::fill(Bounds::new(pb.origin + point(px(0.0), px(0.0)), pb.size + size(px(2.0), px(2.0))), style::shade(&p, p.panel_background, 1.45)));
            window.paint_quad(gpui::fill(pb, gpui::white()));
            agent::record(window, cx, agent::Node { role: "Page", name: Some(format!("Page {}", i + 1).into()), bounds: pb, enabled: true, ..Default::default() });
            let Some(img) = self.pages.get_mut(&i) else { continue };
            // Rendered for this zoom and scale: a texture pixel to a
            // device pixel, from a whole one. Until then, stretched to fit.
            let exact = (img.scale / want - 1.0).abs() < 1e-4;
            let k = if exact { 1.0 / s } else { f32::from(pb.size.width) / img.size.0.max(1) as f32 };
            let at = if exact { point(px((f32::from(pb.origin.x) * s).round() / s), px((f32::from(pb.origin.y) * s).round() / s)) } else { pb.origin };
            window.with_content_mask(Some(gpui::ContentMask { bounds: pb }), |window| {
                for (n, t) in img.tiles.iter().enumerate() {
                    let tb = Bounds::new(point(at.x + px(t.x as f32 * k), at.y + px(t.y as f32 * k)), size(px(t.w as f32 * k), px(t.h as f32 * k)));
                    if !tb.intersects(&visible.on(o)) {
                        continue;
                    }
                    if !img.uploaded[n] {
                        let bytes = (t.w * t.h * 4) as usize;
                        if bytes > budget && budget < UPLOAD_BUDGET {
                            more = true;
                            continue;
                        }
                        budget = budget.saturating_sub(bytes);
                        img.uploaded[n] = true;
                    }
                    let _ = window.paint_image(tb, gpui::Corners::default(), t.image.clone(), 0, false);
                }
            });
        }
        if more {
            window.request_animation_frame();
        }
        self.paint_overlays(&layout, window, cx);
    }

    /// Fields and marks over the pages in view, the stroke being drawn.
    fn paint_overlays(&mut self, layout: &[Rect], window: &mut Window, cx: &mut Context<Self>) {
        let p = theme::palette(cx);
        let o = self.origin;
        let visible = Rect { x0: self.offset.x, y0: self.offset.y, x1: self.offset.x + self.view.width, y1: self.offset.y + self.view.height };
        let zoom = self.zoom;
        for (fi, field) in self.model.fields.iter().enumerate() {
            for (wi, w) in field.widgets.iter().enumerate() {
                let Some(pr) = layout.get(w.page) else { continue };
                let r = to_screen(&self.geoms[w.page], zoom, (pr.x0, pr.y0), w.rect);
                if !r.intersects(&visible) {
                    continue;
                }
                let editing = self.editing == Some(Editing::Field(fi, wi));
                draw_field(window, cx, &p, field, w, r.on(o), zoom, &self.font, editing, self.editable);
            }
        }
        for (i, mark) in self.model.marks.iter().enumerate() {
            let Some(pr) = layout.get(mark.page) else { continue };
            let r = to_screen(&self.geoms[mark.page], zoom, (pr.x0, pr.y0), mark.rect);
            if !r.expand(HANDLE).intersects(&visible) {
                continue;
            }
            if self.editing != Some(Editing::Mark(i)) {
                draw_mark(window, cx, mark, r.on(o), zoom, &self.font);
            }
            if self.sel == Some(Sel::Mark(i)) {
                draw_selection(window, &p, r.on(o), self.editing != Some(Editing::Mark(i)));
            }
        }
        // The stroke so far.
        if let Some(Drag::Draw { page }) = self.drag
            && let Some(pr) = layout.get(page)
        {
            let g = self.geoms[page];
            let pts: Vec<Point<Pixels>> = self
                .stroke
                .iter()
                .map(|u| {
                    let q = apply(&g.transform, u[0], u[1]);
                    point(o.x + px(pr.x0 + q[0] * zoom), o.y + px(pr.y0 + q[1] * zoom))
                })
                .collect();
            for w in pts.windows(2) {
                window.paint_segment(w[0], w[1], px(1.5 * zoom), true, rgb(self.ink));
            }
        }
    }

    /// Where the editor goes (content points), and its padding.
    fn editor_rect(&self) -> Option<(Rect, bool)> {
        let layout = self.layout();
        match self.editing? {
            Editing::Field(fi, wi) => {
                let w = &self.model.fields[fi].widgets[wi];
                let pr = layout.get(w.page)?;
                let multiline = matches!(self.model.fields[fi].kind, FieldKind::Text { multiline: true, .. });
                Some((to_screen(&self.geoms[w.page], self.zoom, (pr.x0, pr.y0), w.rect), multiline))
            }
            Editing::Mark(i) => {
                let mark = &self.model.marks[i];
                let pr = layout.get(mark.page)?;
                let MarkKind::Text { size, .. } = mark.kind else { return None };
                let mut r = to_screen(&self.geoms[mark.page], self.zoom, (pr.x0, pr.y0), mark.rect);
                // Grow the box with the text as it is typed.
                let width = mark.rect[2] - mark.rect[0];
                let h = pdfdoc::text_mark_height(&self.buffer, size, width) * self.zoom;
                r.y1 = r.y0 + h.max(r.h());
                Some((r, true))
            }
        }
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Fit modes follow the window size.
        if self.view.width > 0.0 {
            let inner_w = self.view.width - 2.0 * GAP - 4.0;
            let inner_h = self.view.height - 2.0 * GAP - 4.0;
            let (pw, ph) = self.geoms.get(self.current_page).map_or((612.0, 792.0), |g| g.size);
            match self.fit {
                Some(Fit::Width) => self.zoom = (inner_w / pw).clamp(MIN_ZOOM, MAX_ZOOM),
                Some(Fit::Page) => self.zoom = (inner_w / pw).min(inner_h / ph).clamp(MIN_ZOOM, MAX_ZOOM),
                None => {}
            }
        }
        // Scrolls asked for: a zoom about a point, a page, a range.
        if let Some((at, rel, old)) = self.anchor.take() {
            let k = self.zoom / old;
            let off = point((at.x * k - rel.x).max(0.0), (at.y * k - rel.y).max(0.0));
            self.scroll.set_offset(point(px(-off.x), px(-off.y)));
            self.offset = off;
        }
        let layout = self.layout();
        if let Some(page) = self.goto.take()
            && let Some(r) = layout.get(page)
        {
            self.scroll.set_offset(point(px(-self.offset.x), px(-(r.y0 - GAP).max(0.0))));
        }
        if let Some((top, bottom)) = self.reveal.take() {
            let (y0, h) = (self.offset.y, self.view.height);
            let y = if top < y0 { top } else if bottom > y0 + h { bottom - h } else { y0 };
            self.scroll.set_offset(point(px(-self.offset.x), px(-y.max(0.0))));
        }
        let content = self.content_size(&layout);
        let me = cx.entity();
        let (pre, paint) = (me.clone(), me.clone());
        let pages = canvas(move |b, window, cx| pre.update(cx, |v, _| v.prepaint(b, window)), move |_, hits, window, cx| paint.update(cx, |v, cx| v.paint(hits, window, cx))).size_full();
        let mut body = div()
            .id("pdf-pages")
            .relative()
            .w(px(content.width))
            .h(px(content.height))
            .child(pages)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, window, cx| {
                    v.press(e, window, cx);
                    // A press that starts typing keeps the keyboard there
                    // (else the window's focus takes it back after us).
                    if v.editing.is_some() {
                        window.prevent_default();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| v.moved(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _: &MouseUpEvent, _, cx| v.released(cx)))
            .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| v.wheel(e, cx)));
        // The field or text box being typed in, over it.
        if let (Some((editor, _)), Some((r, multiline))) = (&self.editor, self.editor_rect()) {
            let p = theme::palette(cx);
            let pad = if matches!(self.editing, Some(Editing::Mark(_))) { pdfdoc::TEXT_PAD * self.zoom } else { 2.0 * self.zoom };
            let frame = canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    window.paint_quad(gpui::fill(b, gpui::white()));
                    let ring = Bounds::from_corners(b.origin - point(px(1.0), px(1.0)), b.bottom_right() + point(px(1.0), px(1.0)));
                    style::frame_rect(window, ring, style::c(p.keyboard_navigation));
                },
            )
            .absolute()
            .size_full();
            let mut holder = div()
                .absolute()
                .left(px(r.x0))
                .top(px(r.y0))
                .w(px(r.w()))
                .h(px(r.h()))
                .key_context(FIELD_CONTEXT)
                .on_action(cx.listener(|v, _: &NextField, window, cx| v.next_field(false, window, cx)))
                .on_action(cx.listener(|v, _: &PrevField, window, cx| v.next_field(true, window, cx)))
                .on_action(cx.listener(|v, _: &EndEdit, _, cx| v.end_editing(cx)))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(frame);
            let inner = div().absolute().left(px(pad)).right(px(pad)).top(px(if multiline { pad } else { 0.0 })).bottom(px(if multiline { pad } else { 0.0 })).flex().items_center().child(editor.clone());
            holder = holder.child(if multiline { inner.items_start() } else { inner });
            body = body.child(holder);
        }
        let p = theme::palette(cx);
        let _ = window;
        div().size_full().child(ScrollView::new("pdf-scroll", &self.scroll).horizontal(true).border(false).background(p.panel_background).child(body))
    }
}

/// A user-space rect on screen (axis-aligned, any page rotation), in
/// content points from the page's top left at `page_min`.
fn to_screen(g: &PageGeom, zoom: f32, page_min: (f32, f32), r: PdfRect) -> Rect {
    let corners = [[r[0], r[1]], [r[2], r[1]], [r[0], r[3]], [r[2], r[3]]].map(|[x, y]| apply(&g.transform, x, y));
    let x0 = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    let x1 = corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    let y0 = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
    let y1 = corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    Rect { x0: x0 * zoom + page_min.0, y0: y0 * zoom + page_min.1, x1: x1 * zoom + page_min.0, y1: y1 * zoom + page_min.1 }
}

/// Font size of a field's text on screen.
fn field_size(field: &Field, w: &pdfdoc::Widget, zoom: f32) -> f32 {
    pdfdoc::field_font_size(field, w.rect, &pdfdoc::display_text(field)) * zoom
}

/// Text in the PDF face, its left at `x`, on `baseline`.
fn pdf_text(window: &mut Window, cx: &mut App, x: Pixels, baseline: Pixels, align: Align, text: &str, size: f32, font: &Font, color: Hsla) -> Bounds<Pixels> {
    style::label_on_baseline(window, cx, x, baseline, align, text.to_owned(), size.max(1.0), font.clone(), color)
}

/// A field as the saved file will show it, with a tint marking it
/// fillable.
#[allow(clippy::too_many_arguments)]
fn draw_field(window: &mut Window, cx: &mut App, p: &olo_ui::palette::Palette, field: &Field, w: &pdfdoc::Widget, r: Bounds<Pixels>, zoom: f32, font: &Font, editing: bool, editable: bool) {
    if let Some(bg) = w.background {
        window.paint_quad(gpui::fill(r, rgb(bg)));
    }
    if editable && !field.read_only && !editing {
        // Fields to fill in, tinted in the focus colour (on the page,
        // which is paper white whatever the look).
        window.paint_quad(gpui::fill(r, style::c(p.keyboard_navigation).opacity(34.0 / 255.0)));
    }
    if let Some(bc) = w.border {
        let t = px(zoom.max(1.0));
        let inner = Bounds::from_corners(r.origin + point(px(0.5 * zoom), px(0.5 * zoom)), r.bottom_right() - point(px(0.5 * zoom), px(0.5 * zoom)));
        window.paint_quad(gpui::outline(inner, rgb(bc), gpui::BorderStyle::Solid).border_widths(t));
    }
    // What agents read it as: a check box or radio button is on when
    // the field's state is this widget's.
    let on = matches!(&field.value, Value::State(Some(s)) if w.on.as_deref() == Some(s.as_str()));
    let (role, value, checked) = match &field.kind {
        FieldKind::Check => ("CheckBox", None, Some(on)),
        FieldKind::Radio => ("RadioButton", w.on.clone(), Some(on)),
        FieldKind::Choice { combo: true, .. } => ("ComboBox", Some(pdfdoc::display_text(field)), None),
        FieldKind::Choice { .. } => ("ListBox", Some(pdfdoc::display_text(field)), None),
        FieldKind::Text { .. } => ("TextInput", Some(pdfdoc::display_text(field)), None),
    };
    // (While it's being typed in, its editor is what agents read.)
    if editing {
        return;
    }
    let usable = editable && !field.read_only;
    agent::record(window, cx, agent::Node { role, name: Some(field.name.clone().into()), value: value.map(Into::into), bounds: r, enabled: usable, clickable: usable, checked, ..Default::default() });
    let color = rgb(field.color);
    let (rw, rh) = (f32::from(r.size.width), f32::from(r.size.height));
    match (&field.kind, &field.value) {
        (FieldKind::Check, Value::State(Some(s))) if w.on.as_deref() == Some(s.as_str()) => {
            let side = rw.min(rh);
            let c = r.center();
            let at = |x: f32, y: f32| point(c.x - px(side / 2.0) + px(x * side), c.y - px(side / 2.0) + px((1.0 - y) * side));
            let t = px((side * 0.12).max(1.0));
            window.paint_segment(at(0.2, 0.52), at(0.42, 0.28), t, true, color);
            window.paint_segment(at(0.42, 0.28), at(0.8, 0.76), t, true, color);
        }
        (FieldKind::Radio, Value::State(Some(s))) if w.on.as_deref() == Some(s.as_str()) => {
            window.paint_sector(r.center(), px(0.0), px(rw.min(rh) * 0.25), 0.0, std::f32::consts::TAU, color);
        }
        (FieldKind::Check | FieldKind::Radio, _) => {}
        (FieldKind::Choice { combo: false, options, .. }, Value::Text(v)) => {
            let size = field_size(field, w, zoom);
            let lead = size * 1.15;
            for (k, (export, label)) in options.iter().enumerate() {
                let top = f32::from(r.top()) + 1.0 + k as f32 * lead;
                if top + lead > f32::from(r.bottom()) + 0.5 {
                    break;
                }
                let row = Bounds::new(point(r.left() + px(1.0), px(top)), size_px(rw - 2.0, lead));
                if export == v {
                    window.paint_quad(gpui::fill(row, gpui::rgb(0x99c1da)));
                }
                let baseline = style::baseline_at(window, row.center().y, size);
                pdf_text(window, cx, row.left() + px(2.0 * zoom), baseline, Align::Left, label, size, font, color);
            }
        }
        _ => {
            let text = pdfdoc::display_text(field);
            let size = field_size(field, w, zoom);
            let pt = size / zoom;
            let (x, align) = match field.align {
                1 => (r.center().x, Align::Center),
                2 => (r.right() - px(2.0 * zoom), Align::Right),
                _ => (r.left() + px(2.0 * zoom), Align::Left),
            };
            window.with_content_mask(Some(gpui::ContentMask { bounds: r }), |window| match field.kind {
                FieldKind::Text { multiline: true, .. } => {
                    let width = w.rect[2] - w.rect[0];
                    for (k, line) in pdfdoc::wrap(&text, pt, width - 4.0).iter().enumerate() {
                        let baseline = r.top() + px((2.0 + pt * 0.93 + k as f32 * pt * 1.15) * zoom);
                        pdf_text(window, cx, x, baseline, align, line, size, font, color);
                    }
                }
                FieldKind::Text { comb: true, max_len: Some(n), .. } if n > 0 => {
                    let cell = rw / n as f32;
                    let baseline = style::baseline_at(window, r.center().y, size);
                    for (k, ch) in text.chars().take(n).enumerate() {
                        pdf_text(window, cx, r.left() + px(cell * k as f32 + cell / 2.0), baseline, Align::Center, &ch.to_string(), size, font, color);
                    }
                }
                _ => {
                    let baseline = style::baseline_at(window, r.center().y, size);
                    pdf_text(window, cx, x, baseline, align, &text, size, font, color);
                }
            });
            if matches!(field.kind, FieldKind::Choice { combo: true, .. }) && editable {
                // The pop-up arrow.
                let a = rh.min(14.0 * zoom) * 0.3;
                let (cx_, cy) = (r.right() - px(a * 2.0), r.center().y);
                let mut path = PathBuilder::fill();
                path.move_to(point(cx_ - px(a), cy - px(a / 2.0)));
                path.line_to(point(cx_ + px(a), cy - px(a / 2.0)));
                path.line_to(point(cx_, cy + px(a / 2.0)));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, gpui::rgb(0x5a5a5a));
                }
            }
        }
    }
}

fn size_px(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}

fn draw_mark(window: &mut Window, cx: &mut App, mark: &Mark, r: Bounds<Pixels>, zoom: f32, font: &Font) {
    let color = rgb(mark.color);
    match &mark.kind {
        MarkKind::Text { text, size } => {
            let width = mark.rect[2] - mark.rect[0];
            for (k, line) in pdfdoc::wrap(text, *size, width - 2.0 * pdfdoc::TEXT_PAD).iter().enumerate() {
                let top = r.top() + px((pdfdoc::TEXT_PAD + k as f32 * size * 1.15) * zoom);
                pdf_text(window, cx, r.left() + px(pdfdoc::TEXT_PAD * zoom), top + px(size * zoom * 0.905), Align::Left, line, size * zoom, font, color);
            }
            agent::record(window, cx, agent::Node { role: "TextInput", name: Some("Text Box".into()), value: Some(text.clone().into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
        }
        MarkKind::Ink { strokes, width, signature } => {
            let sw = px(width * zoom);
            let (rw, rh) = (r.size.width, r.size.height);
            for s in strokes {
                let pts: Vec<Point<Pixels>> = s.iter().map(|p| point(r.left() + rw * p[0], r.bottom() - rh * p[1])).collect();
                if pts.len() == 1 {
                    window.paint_sector(pts[0], px(0.0), sw / 2.0, 0.0, std::f32::consts::TAU, color);
                    continue;
                }
                for w in pts.windows(2) {
                    window.paint_segment(w[0], w[1], sw, true, color);
                }
            }
            let name = if *signature { "Signature" } else { "Drawing" };
            agent::record(window, cx, agent::Node { role: "Image", name: Some(name.into()), bounds: r, enabled: true, clickable: true, ..Default::default() });
        }
    }
}

/// A dotted frame with corner handles, in the focus colour.
fn draw_selection(window: &mut Window, p: &olo_ui::palette::Palette, r: Bounds<Pixels>, handles: bool) {
    let blue = style::c(p.keyboard_navigation);
    let r = Bounds::from_corners(r.origin - point(px(2.0), px(2.0)), r.bottom_right() + point(px(2.0), px(2.0)));
    let t = style::hairline(window);
    let dash = |window: &mut Window, horizontal: bool, at: Pixels, from: Pixels, to: Pixels| {
        let mut x = from;
        while x < to {
            let e = (x + px(3.0)).min(to);
            let b = if horizontal { Bounds::from_corners(point(x, at), point(e, at + t)) } else { Bounds::from_corners(point(at, x), point(at + t, e)) };
            window.paint_quad(gpui::fill(b, blue));
            x += px(5.0);
        }
    };
    dash(window, true, r.top(), r.left(), r.right());
    dash(window, true, r.bottom() - t, r.left(), r.right());
    dash(window, false, r.left(), r.top(), r.bottom());
    dash(window, false, r.right() - t, r.top(), r.bottom());
    if handles {
        for c in [r.origin, point(r.right(), r.top()), r.bottom_right(), point(r.left(), r.bottom())] {
            let h = style::snap_centred(window, c, HANDLE, HANDLE);
            window.paint_quad(gpui::fill(h, gpui::white()));
            style::frame_rect(window, h, blue);
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
