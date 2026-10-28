//! Layout lint: finds views that don't fit. A test hands [`check`] a view
//! and the size its window gets; it runs the view headlessly (plain egui,
//! no GPU or Wayland, the kit's own fonts and style), light and dark, at
//! 1× and 2×. While it runs, the kit's containers and controls note where
//! they were put ([`area`], [`control`], [`text`]), and the notes are
//! checked for:
//!
//! - content running past the area it was given (a tab view's panel, a
//!   box, a window's body),
//! - a control outside the window, or over another control,
//! - text that doesn't fit its room (a label running into its field, a
//!   line cut short with "…", tabs wider than their view).
//!
//! Outside a check nothing is noted: each note costs one atomic load.
//! See "Layout lint" in docs/HACKING.md.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use egui::{Context, Id, LayerId, Pos2, Rect, Ui, Vec2, ViewportId};
use olo_theme::Palette;

use crate::style;

/// How far past its room something may be before it counts (rounding).
const SLACK: f32 = 0.5;

/// Checks running now, in any thread.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
enum Note {
    /// Content laid out in `used`, given `max`.
    Area { what: String, max: Rect, used: Rect, clip: Rect },
    /// A control drawn in `rect`, seen through `clip`.
    Control { what: String, rect: Rect, clip: Rect, layer: LayerId },
    /// Text that needs `needed` points across and has `room`.
    Text { what: String, needed: f32, room: f32 },
}

type Notes = Arc<Mutex<Vec<Note>>>;

fn notes_id() -> Id {
    Id::unique("olo-layout-lint")
}

/// Whether a check may be running (so measuring something only a note
/// needs is worth it).
pub fn on() -> bool {
    RUNNING.load(Ordering::Relaxed) > 0
}

fn note(ctx: &Context, make: impl FnOnce() -> Note) {
    if !on() {
        return;
    }
    if let Some(notes) = ctx.data(|d| d.get_temp::<Notes>(notes_id())) {
        notes.lock().unwrap().push(make());
    }
}

/// Content given `max` was laid out in `used` (a child `Ui`'s
/// `min_rect`). Kit containers note themselves; call it for areas a view
/// marks out on its own (a panel's body above its buttons).
pub fn area(ui: &Ui, what: impl FnOnce() -> String, max: Rect, used: Rect) {
    note(ui.ctx(), || Note::Area { what: what(), max, used, clip: ui.clip_rect() });
}

/// A control was drawn in `rect`. The kit's controls note themselves;
/// call it for controls a view draws by hand.
pub fn control(ui: &Ui, what: impl FnOnce() -> String, rect: Rect) {
    note(ui.ctx(), || Note::Control { what: what(), rect, clip: ui.clip_rect(), layer: ui.layer_id() });
}

/// Text needing `needed` points across was given `room`.
pub fn text(ui: &Ui, what: impl FnOnce() -> String, needed: f32, room: f32) {
    if needed > room + SLACK {
        note(ui.ctx(), || Note::Text { what: what(), needed, room });
    }
}

/// `"Label"` in quotes, for notes.
pub(crate) fn quoted(kind: &str, label: &str) -> String {
    format!("{kind} “{label}”")
}

/// Run `view` as a window of content `size` would, light and dark, at 1×
/// and 2×, and say what doesn't fit, one line per problem (with the
/// looks it shows in). Empty: it all fits.
pub fn check(size: Vec2, mut view: impl FnMut(&mut Ui)) -> Vec<String> {
    RUNNING.fetch_add(1, Ordering::Relaxed);
    let _done = Done;
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for palette in [Palette::light(), Palette::dark()] {
        for scale in [1.0, 2.0] {
            let look = format!("{} {scale}×", if palette.mode == olo_theme::ThemeMode::Dark { "dark" } else { "light" });
            for problem in run(size, scale, &Arc::new(palette.clone()), &mut view) {
                found.entry(problem).or_default().push(look.clone());
            }
        }
    }
    found.into_iter().map(|(problem, looks)| format!("{problem} ({})", looks.join(", "))).collect()
}

/// [`check`], panicking with every problem in `name`.
#[track_caller]
pub fn assert_fits(name: &str, size: Vec2, view: impl FnMut(&mut Ui)) {
    let problems = check(size, view);
    if !problems.is_empty() {
        let mut msg = format!("{name} ({}×{}) doesn't fit:", size.x, size.y);
        for p in &problems {
            let _ = write!(msg, "\n  - {p}");
        }
        panic!("{msg}");
    }
}

struct Done;

impl Drop for Done {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Passes before the notes count: views settle (sizes measured in one
/// pass and used in the next) by the third.
const PASSES: usize = 3;

fn run(size: Vec2, scale: f32, palette: &Arc<Palette>, view: &mut impl FnMut(&mut Ui)) -> Vec<String> {
    // As platform.rs makes a window's context.
    let ctx = Context::default();
    ctx.set_fonts(style::all_font_definitions());
    style::apply(&ctx, palette);
    let notes = Notes::default();
    ctx.data_mut(|d| d.insert_temp(notes_id(), notes.clone()));
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let mut root_used = Rect::NOTHING;
    for pass in 0..PASSES {
        let mut raw = egui::RawInput { screen_rect: Some(screen), time: Some(pass as f64), focused: true, ..Default::default() };
        raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
        ctx.run_ui(raw, |ui| {
            notes.lock().unwrap().clear();
            view(ui);
            root_used = ui.min_rect();
        })
        .drop_without_applying_deltas();
    }
    let notes = notes.lock().unwrap().clone();
    let mut problems = Vec::new();
    if root_used.is_positive() && !screen.expand(SLACK).contains_rect(root_used) {
        problems.push(format!("the window's content runs past it: {}", past(screen, root_used)));
    }
    let mut controls = Vec::new();
    for n in &notes {
        match n {
            // Inside something that clips (a scroll view), content may
            // run on.
            Note::Area { what, max, used, clip } if *clip == screen && used.is_positive() => {
                if !max.expand(SLACK).contains_rect(*used) {
                    problems.push(format!("{what}: content runs past it: {}", past(*max, *used)));
                }
            }
            Note::Area { .. } => {}
            Note::Control { what, rect, clip, layer } => {
                let seen = rect.intersect(*clip);
                if !seen.is_positive() {
                    continue;
                }
                if !screen.expand(SLACK).contains_rect(seen) {
                    problems.push(format!("{what} is outside the window: {}", past(screen, seen)));
                }
                controls.push((what, seen, *layer));
            }
            Note::Text { what, needed, room } => {
                problems.push(format!("{what} doesn't fit: needs {needed:.0}, has {room:.0}"));
            }
        }
    }
    for (i, (a, ra, la)) in controls.iter().enumerate() {
        for (b, rb, lb) in &controls[i + 1..] {
            let both = ra.intersect(*rb);
            if la == lb && both.width() > 1.0 && both.height() > 1.0 {
                problems.push(format!("{a} overlaps {b} by {:.0}×{:.0}", both.width(), both.height()));
            }
        }
    }
    problems
}

/// Which way, and how far, `used` goes past `max`.
fn past(max: Rect, used: Rect) -> String {
    let mut sides = Vec::new();
    for (name, by) in [
        ("top", max.top() - used.top()),
        ("bottom", used.bottom() - max.bottom()),
        ("left", max.left() - used.left()),
        ("right", used.right() - max.right()),
    ] {
        if by > SLACK {
            sides.push(format!("{by:.0} px at the {name}"));
        }
    }
    sides.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{BBox, Button, CheckBox, MenuField, StringView, TabView};
    use egui::vec2;

    #[test]
    fn ui_lint_finds_what_doesnt_fit() {
        // A box too tall for its tab view, as Touchpad ▸ Touch once was.
        let problems = check(vec2(300.0, 160.0), |ui| {
            TabView::new(&mut 0, &["One", "Two"]).show(ui, |ui, _| {
                BBox::new("Box").show(ui, |ui| {
                    for i in 0..6 {
                        ui.add(CheckBox::new(&mut true, &format!("Choice {i}")));
                    }
                });
            });
        });
        assert!(problems.iter().any(|p| p.starts_with("tab “One”: content runs past it")), "{problems:?}");
        // A label longer than its divider, and a line cut short.
        let problems = check(vec2(200.0, 100.0), |ui| {
            ui.add(MenuField::new("m", &mut 0, &["One"]).label("A long label:").divider(40.0));
            ui.add(StringView::new("A line of text far too long for this window to hold"));
        });
        assert!(problems.iter().any(|p| p.starts_with("menu field “A long label:”'s label doesn't fit")), "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("text “A line of text")), "{problems:?}");
        // Controls on top of each other.
        let problems = check(vec2(200.0, 100.0), |ui| {
            let r = Rect::from_min_size(Pos2::new(10.0, 10.0), vec2(80.0, 24.0));
            ui.put(r, Button::new("One"));
            ui.put(r.translate(vec2(40.0, 0.0)), Button::new("Two"));
        });
        assert!(problems.iter().any(|p| p.contains("overlaps")), "{problems:?}");
    }

    #[test]
    fn ui_lint_passes_what_fits() {
        assert_fits("a small window", vec2(300.0, 200.0), |ui| {
            crate::widgets::window_content(ui, |ui| {
                BBox::new("Box").show(ui, |ui| {
                    ui.add(CheckBox::new(&mut true, "Choice"));
                    ui.add(MenuField::new("m", &mut 0, &["One", "Two"]).label("Pick:"));
                });
                ui.add(StringView::new("Short."));
            });
        });
    }
}
