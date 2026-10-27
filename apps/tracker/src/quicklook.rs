//! Quick Look: Space on a selected item previews it at once, in a window
//! that follows the selection (arrow keys step through the folder, Space
//! or Escape closes). The previews themselves come from `olo-preview`;
//! this adds the footer (what it is, rotate, pop out to Viewer, Open) and
//! the keys. Rotating a picture (⌘L / ⌘R, or the footer's buttons) saves it
//! at once, as Quick Look does on the Mac; anything more is Viewer's job.

use olo_kit::{
    WindowId,
    egui::{self, Key, Rect, Ui, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::Button,
};
pub use olo_preview::{SLOW, Update, edit::EditEvent};
use olo_preview::{
    Body, Item, Preview,
    edit::{self, Editor, Op},
};

use crate::{
    fs::{self, Entry},
    view,
};

/// The bar along the bottom: what the item is, pop out, and Open.
const FOOTER: f32 = m::CONTROL_HEIGHT + 2.0 * m::TIGHT + 2.0;

pub enum Action {
    Close,
    Open,
    /// Keep looking at it in its own Viewer window.
    PopOut,
    /// Preview the previous (-1) or next (+1) item in the folder.
    Step(isize),
    /// Fill the screen with it, or come back to the window (F).
    Fullscreen,
    /// Turn the picture a quarter left (false) or right (true) and save it.
    Rotate(bool),
    /// Drag the file out (from the picture, or the footer's icon).
    Drag(view::Drag),
}

pub struct QuickLook {
    pub win: Option<WindowId>,
    /// The folder window (or desktop) whose selection is shown.
    pub source: WindowId,
    pub entry: Entry,
    preview: Preview,
    /// "Open with …": who opens the entry (looked up once per item).
    opener: Option<String>,
    /// The screen is kept awake (a video is playing).
    pub awake: bool,
    /// Rotating the picture (made at the first turn).
    editor: Option<Editor>,
    /// The entry is a picture that can be written back.
    editable: bool,
    /// The last rotation couldn't be saved.
    pub failed: Option<String>,
    /// What the picture is ("JPEG image"), kept for the note as it turns.
    what: String,
}

fn item(e: &Entry) -> Item {
    Item {
        path: e.path.clone(),
        name: e.name.clone(),
        is_dir: e.is_dir,
        size: e.size,
        modified: e.modified,
        kind: e.kind,
        icon: view::icon_of(e).to_string(),
    }
}

impl QuickLook {
    /// Start loading `entry`; `send` delivers updates (tagged with the
    /// generation) to the UI thread.
    pub fn new(source: WindowId, entry: Entry, generation: u64, send: impl Fn(u64, Update) + Send + 'static) -> QuickLook {
        let preview = Preview::new(item(&entry), generation, send);
        let opener = fs::opener_name(&entry);
        let editable = !entry.is_dir && edit::editable(&entry.path);
        QuickLook { win: None, source, entry, preview, opener, awake: false, editor: None, editable, failed: None, what: String::new() }
    }

    pub fn playing_video(&self) -> bool {
        self.preview.playing_video()
    }

    pub fn generation(&self) -> u64 {
        self.preview.generation
    }

    pub fn retarget(&mut self, entry: Entry, generation: u64, send: impl Fn(u64, Update) + Send + 'static) {
        self.preview.retarget(item(&entry), generation, send);
        self.opener = fs::opener_name(&entry);
        // A rotation still being written finishes on its own thread.
        self.editor = None;
        self.failed = None;
        self.editable = !entry.is_dir && edit::editable(&entry.path);
        self.entry = entry;
    }

    /// Turn the picture a quarter and save it in place; the result comes
    /// back to [`QuickLook::edited`] through `send`.
    pub fn rotate(&mut self, right: bool, send: impl Fn(EditEvent) + Send + 'static) {
        if !self.editable {
            return;
        }
        if self.editor.is_none() {
            let Some(Body::Image { full, .. }) = self.preview.body() else { return };
            self.what = self.preview.note().split(", ").next().unwrap_or_default().to_owned();
            self.editor = Some(Editor::new(self.entry.path.clone(), (full[0] as u32, full[1] as u32), send));
        }
        let e = self.editor.as_mut().expect("made");
        e.push(if right { Op::RotateRight } else { Op::RotateLeft });
        e.save(self.entry.path.clone());
        self.failed = None;
    }

    /// The editor reports `event` (for the entry at `path`); true when it
    /// was this one's.
    pub fn edited(&mut self, path: &std::path::Path, event: EditEvent) -> bool {
        let Some(e) = self.editor.as_mut().filter(|e| e.path == path) else { return false };
        e.handle(&event);
        match event {
            EditEvent::Shown { image, smaller, full, .. } => {
                let note = format!("{}, {} × {}", self.what, full[0] as u32, full[1] as u32);
                self.preview.show_edited(image, smaller, full, note);
            }
            EditEvent::Saved(_) => {}
            EditEvent::Failed(why) => self.failed = Some(why),
        }
        true
    }

    /// A rotation is being worked out or written.
    pub fn busy(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.busy || e.saving)
    }

    /// Apply an update; true when a new body arrived (the window may want
    /// a new size).
    pub fn update(&mut self, u: Update) -> bool {
        self.preview.update(u)
    }

    pub fn stop(&mut self) {
        self.preview.stop();
    }

    /// The window size that suits what is shown, within the screen.
    pub fn ideal_size(&self, screen: Option<(u32, u32)>) -> (u32, u32) {
        let (w, h) = self.preview.ideal_size(screen, FOOTER);
        // Room for the note beside the rotate buttons.
        let min = if self.editable && self.preview.is_image() { 460 } else { 0 };
        (w.max(min.min(screen.map_or(u32::MAX, |s| s.0))), h)
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.input(|i| {
            if i.key_pressed(Key::Space) || i.key_pressed(Key::Escape) {
                action = Some(Action::Close);
            } else if i.key_pressed(Key::Enter) {
                action = Some(Action::Open);
            } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
                action = Some(Action::Step(-1));
            } else if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
                action = Some(Action::Step(1));
            } else if i.key_pressed(Key::F) && i.modifiers.is_none() {
                action = Some(Action::Fullscreen);
            } else if i.modifiers.command && (i.key_pressed(Key::L) || i.key_pressed(Key::R)) {
                action = Some(Action::Rotate(i.key_pressed(Key::R)));
            }
        });
        let full = ui.max_rect();
        let content = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - FOOTER));
        let footer = Rect::from_min_max(pos2(full.left(), content.bottom()), full.max);
        self.preview.ui(ui, content);
        if self.preview.take_drag_out() {
            action = Some(Action::Drag(self.drag(vec2(16.0, 16.0))));
        }
        self.footer(ui, footer).or(action)
    }

    /// Dragging the item out, with the pointer `hotspot` into its icon.
    fn drag(&self, hotspot: egui::Vec2) -> view::Drag {
        let e = &self.entry;
        view::Drag { paths: vec![e.path.clone()], icon: view::icon_of(e), size: 32.0, hotspot, ask: false, name: e.name.clone(), below: true }
    }

    fn footer(&mut self, ui: &mut Ui, r: Rect) -> Option<Action> {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        painter.rect_filled(r, 0.0, c(p.panel_background));
        style::hline(painter, r.left(), r.right(), r.top(), style::shade(&p, p.panel_background, 1.2));
        style::hline(painter, r.left(), r.right(), r.top() + 1.0, c(p.shine));
        let inner = r.shrink2(vec2(m::SPACING, m::TIGHT));
        let icon = Rect::from_center_size(pos2(inner.left() + 8.0, inner.center().y + 1.0), vec2(16.0, 16.0));
        icons::paint(painter, icon, view::icon_of(&self.entry));
        // The icon is the item: drag it out, as a document's proxy icon.
        let mut action = None;
        let grab = ui.interact(icon.expand(2.0), ui.make_persistent_id("ql-proxy"), egui::Sense::drag());
        if grab.drag_started() {
            action = Some(Action::Drag(self.drag(vec2(16.0, 16.0))));
        }
        let painter = ui.painter();
        let label = match &self.opener {
            Some(app) => format!("Open with {app}"),
            None => "Open".into(),
        };
        let bw = (style::label_width(ui.ctx(), &label, style::plain_font()) + 28.0).max(80.0);
        // Pop out is a square button showing a window with an arrow.
        let pw = m::CONTROL_HEIGHT + 4.0;
        let pop = !self.entry.is_dir && self.opener.as_deref() != Some("Viewer");
        // Rotate left and right, for pictures that can be saved.
        let rotate = self.editable && self.preview.is_image();
        let rotate_w = if rotate { 2.0 * (pw + m::TIGHT) + m::SPACING } else { 0.0 };
        let text_right = inner.right() - bw - m::SPACING - if pop { pw + m::TIGHT } else { 0.0 } - rotate_w;
        let note = match &self.failed {
            Some(why) => why.clone(),
            None if self.busy() => "Rotating…".into(),
            None => self.preview.note(),
        };
        let mut job = egui::text::LayoutJob::simple_singleline(note, style::plain_font(), style::disabled_text(&p));
        job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - icon.right() - m::TIGHT * 2.0).max(0.0));
        let g = painter.layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, icon.right() + m::TIGHT * 2.0, inner.center().y, style::PLAIN_SIZE);
        painter.galley(pos, g, style::disabled_text(&p));
        let br = Rect::from_min_max(pos2(inner.right() - bw, inner.top()), inner.max);
        let mut left = br.left();
        if pop {
            let pr = Rect::from_min_size(pos2(left - m::TIGHT - pw, inner.top()), vec2(pw, inner.height()));
            let resp = ui.put(pr, Button::new("").min_width(pw)).on_hover_text("Open in Viewer");
            pop_out_glyph(ui.painter(), pr, &p);
            if resp.clicked() {
                action = Some(Action::PopOut);
            }
            left = pr.left();
        }
        if rotate {
            let right_r = Rect::from_min_size(pos2(left - m::SPACING - pw, inner.top()), vec2(pw, inner.height()));
            let left_r = right_r.translate(vec2(-pw - m::TIGHT, 0.0));
            for (r, right, tip) in [(left_r, false, "Rotate Left (⌘L)"), (right_r, true, "Rotate Right (⌘R)")] {
                let resp = ui.put(r, Button::new("").min_width(pw)).on_hover_text(tip);
                rotate_glyph(ui.painter(), r, right, &p);
                if resp.clicked() {
                    action = Some(Action::Rotate(right));
                }
            }
        }
        if ui.put(br, Button::new(&label).min_width(bw)).clicked() {
            action = Some(Action::Open);
        }
        action
    }
}

/// A three-quarter circle with an arrowhead, turning left or right.
fn rotate_glyph(painter: &egui::Painter, r: Rect, right: bool, p: &olo_kit::theme::Palette) {
    let ink = c(p.control_text);
    let s = egui::Stroke::new(1.0, ink);
    let o = r.center() + vec2(0.0, 0.5);
    let flip = if right { 1.0 } else { -1.0 };
    // From the top, round the far side and back up to the near one.
    let points: Vec<egui::Pos2> = (0..=24)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2 - flip * (k as f32 / 24.0) * 1.5 * std::f32::consts::PI;
            pos2(o.x + 5.0 * a.cos(), o.y + 5.0 * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(points, s));
    // The arrowhead at the top, pointing the way it turns.
    let tip = pos2(o.x + flip * 2.5, o.y - 5.0);
    painter.add(egui::Shape::convex_polygon(
        vec![pos2(tip.x - flip * 3.0, tip.y - 3.0), tip, pos2(tip.x - flip * 3.0, tip.y + 3.0)],
        ink,
        egui::Stroke::NONE,
    ));
}

/// A small window frame with an arrow leaving its top-right corner.
fn pop_out_glyph(painter: &egui::Painter, r: Rect, p: &olo_kit::theme::Palette) {
    let ink = c(p.control_text);
    let s = egui::Stroke::new(1.0, ink);
    let o = r.center() + vec2(-6.0, -5.0);
    let px = |x: f32, y: f32| pos2((o.x + x).round() + 0.5, (o.y + y).round() + 0.5);
    // The frame, open at the top-right where the arrow leaves.
    painter.line_segment([px(0.0, 3.0), px(0.0, 11.0)], s);
    painter.line_segment([px(0.0, 11.0), px(8.0, 11.0)], s);
    painter.line_segment([px(8.0, 11.0), px(8.0, 7.0)], s);
    painter.line_segment([px(0.0, 3.0), px(4.0, 3.0)], s);
    // The arrow.
    painter.line_segment([px(4.0, 7.0), px(11.0, 0.0)], s);
    painter.line_segment([px(7.0, 0.0), px(11.0, 0.0)], s);
    painter.line_segment([px(11.0, 0.0), px(11.0, 4.0)], s);
}
