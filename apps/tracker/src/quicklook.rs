//! Quick Look: Space on a selected item previews it at once, in a window
//! that follows the selection (arrow keys step through the folder, Space
//! or Escape closes). The previews themselves come from `tabula-preview`;
//! this adds the footer (what it is, pop out to Viewer, Open) and the keys.

use tabula_kit::{
    WindowId,
    egui::{self, Key, Rect, Ui, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::Button,
};
pub use tabula_preview::{SLOW, Update};
use tabula_preview::{Item, Preview};

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
        QuickLook { win: None, source, entry, preview, opener, awake: false }
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
        self.entry = entry;
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
        self.preview.ideal_size(screen, FOOTER)
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
            }
        });
        let full = ui.max_rect();
        let content = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - FOOTER));
        let footer = Rect::from_min_max(pos2(full.left(), content.bottom()), full.max);
        self.preview.ui(ui, content);
        self.footer(ui, footer).or(action)
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
        let label = match &self.opener {
            Some(app) => format!("Open with {app}"),
            None => "Open".into(),
        };
        let bw = (style::label_width(ui.ctx(), &label, style::plain_font()) + 28.0).max(80.0);
        // Pop out is a square button showing a window with an arrow.
        let pw = m::CONTROL_HEIGHT + 4.0;
        let pop = !self.entry.is_dir && self.opener.as_deref() != Some("Viewer");
        let text_right = inner.right() - bw - m::SPACING - if pop { pw + m::TIGHT } else { 0.0 };
        let mut job =
            egui::text::LayoutJob::simple_singleline(self.preview.note(), style::plain_font(), style::disabled_text(&p));
        job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - icon.right() - m::TIGHT * 2.0).max(0.0));
        let g = painter.layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, icon.right() + m::TIGHT * 2.0, inner.center().y, style::PLAIN_SIZE);
        painter.galley(pos, g, style::disabled_text(&p));
        let br = Rect::from_min_max(pos2(inner.right() - bw, inner.top()), inner.max);
        let mut action = None;
        if pop {
            let pr = Rect::from_min_size(pos2(br.left() - m::TIGHT - pw, inner.top()), vec2(pw, inner.height()));
            let resp = ui.put(pr, Button::new("").min_width(pw)).on_hover_text("Open in Viewer");
            pop_out_glyph(ui.painter(), pr, &p);
            if resp.clicked() {
                action = Some(Action::PopOut);
            }
        }
        if ui.put(br, Button::new(&label).min_width(bw)).clicked() {
            action = Some(Action::Open);
        }
        action
    }
}

/// A small window frame with an arrow leaving its top-right corner.
fn pop_out_glyph(painter: &egui::Painter, r: Rect, p: &tabula_kit::theme::Palette) {
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
