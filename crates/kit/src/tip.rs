//! Tooltips that may reach past their window's edges, each in a popup of
//! its own (an xdg_popup with no grab that takes no input), for windows
//! too small to hold their tips: Deskbar's tray, whose window is narrower
//! than what its icons say. They come and go as egui's own
//! (`on_hover_text`): once the pointer has rested on the widget for
//! `tooltip_delay`, at once if another was showing a moment ago, not
//! after a click; they look the same; and the text is the widget's
//! description for agents, as egui's is.
//!
//! ```ignore
//! use olo_kit::tip::Tip;
//! ui.interact(rect, id, Sense::click()).tip("Click to stay awake")
//! ```

use egui::{Color32, Id, Rect, Response, Ui, Vec2, emath::GuiRounding, vec2};

/// What a window's frame asks the kit to show, until a frame doesn't.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TipRequest {
    pub text: String,
    /// The widget's rect, in the window's points: the tip goes below it,
    /// else above, slid to stay on the screen.
    pub anchor: Rect,
}

/// The gap between the widget and its tip, as egui's.
pub(crate) const GAP: f32 = 4.0;

fn request_key() -> Id {
    Id::unique("olo-tip-request")
}

/// The widget whose tip was asked for last, in which pass and when.
fn shown_key() -> Id {
    Id::unique("olo-tip-shown")
}

pub trait Tip {
    /// Show `text` in a tip of its own when the pointer rests on this.
    fn tip(self, text: impl Into<String>) -> Self;
}

impl Tip for Response {
    fn tip(self, text: impl Into<String>) -> Self {
        let text = text.into();
        describe(&self, &text);
        if due(&self) {
            let (pass, now) = (self.ctx.cumulative_pass_nr(), self.ctx.input(|i| i.time));
            self.ctx.data_mut(|d| {
                d.insert_temp(request_key(), TipRequest { text, anchor: self.rect });
                d.insert_temp(shown_key(), (self.id, pass, now));
            });
        }
        self
    }
}

/// egui's `describe_for_accessibility`: the text is the widget's
/// description, and its name if it has none.
fn describe(r: &Response, text: &str) {
    if !r.ctx.has_accesskit_node(r.id) {
        return;
    }
    r.ctx.accesskit_node_builder(r.id, |node| {
        if node.description().is_none() {
            node.set_description(text);
        }
        if node.role() != egui::accesskit::Role::Label && node.labelled_by().is_empty() && node.label().is_none_or(str::is_empty) {
            node.set_label(text);
        }
    });
}

/// Whether `r`'s tip shows this frame.
fn due(r: &Response) -> bool {
    let ctx = &r.ctx;
    let (pointer, down, moved, clicked, scrolled, now) = ctx.input(|i| {
        (
            i.pointer.has_pointer(),
            i.pointer.any_down(),
            i.pointer.time_since_last_movement(),
            i.pointer.time_since_last_click(),
            i.time_since_last_scroll(),
            i.time,
        )
    });
    if !r.hovered() || !pointer || down {
        return false;
    }
    let style = ctx.global_style();
    let (delay, grace) = (style.interaction.tooltip_delay, style.interaction.tooltip_grace_time);
    if scrolled < delay {
        ctx.request_repaint_after_secs(delay - scrolled);
        return false;
    }
    // Clicked and rested there: not until it moves on.
    if clicked < moved + 0.1 {
        return false;
    }
    if let Some((id, pass, at)) = ctx.data(|d| d.get_temp::<(Id, u64, f64)>(shown_key())) {
        // Showing already, or another was a moment ago.
        if (id == r.id && pass + 1 >= ctx.cumulative_pass_nr()) || now - at < grace as f64 {
            return true;
        }
    }
    if moved < delay {
        ctx.request_repaint_after_secs(delay - moved);
        return false;
    }
    true
}

/// This frame's tip, if a widget asked for one.
pub(crate) fn take(ctx: &egui::Context) -> Option<TipRequest> {
    ctx.data_mut(|d| {
        let r = d.get_temp::<TipRequest>(request_key());
        d.remove::<TipRequest>(request_key());
        r
    })
}

fn galley(ctx: &egui::Context, text: &str) -> std::sync::Arc<egui::Galley> {
    let style = ctx.global_style();
    let width = style.spacing.tooltip_width;
    ctx.fonts_mut(|f| f.layout(text.to_owned(), crate::style::plain_font(), Color32::PLACEHOLDER, width))
}

/// The plate's margin and border, as egui's tooltips (`Frame::popup`).
fn inset(ctx: &egui::Context) -> f32 {
    let style = ctx.global_style();
    style.spacing.menu_margin.left as f32 + style.visuals.window_stroke.width
}

/// The tip's size, in whole points.
pub(crate) fn size(ctx: &egui::Context, text: &str) -> (u32, u32) {
    let s = galley(ctx, text).size() + Vec2::splat(2.0 * inset(ctx));
    (s.x.ceil() as u32, s.y.ceil() as u32)
}

/// Draw the tip filling its popup.
pub(crate) fn ui(ui: &mut Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let visuals = ui.visuals().clone();
    let plate = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(plate, 0.0, visuals.window_fill);
    crate::style::frame_rect(painter, plate, visuals.window_stroke.color);
    let text_color = visuals.override_text_color.unwrap_or(visuals.widgets.noninteractive.fg_stroke.color);
    let at = (plate.min + vec2(inset(&ctx), inset(&ctx))).round_to_pixels(ctx.pixels_per_point());
    painter.galley(at, galley(&ctx, text), text_color);
}
