//! A wrapping grid of pictures to pick one from, labelled underneath (a
//! screen to share, a wallpaper), as the egui kit's `ThumbnailPicker`:
//! each in a sunken frame, the chosen one and its name on a selection
//! plate. A click chooses, a double click opens (shares at once); with
//! the keyboard (a click gives it, or Tab) the arrows move the choice.
//! Agents read each as a `RadioButton`, selected or not.

use std::{rc::Rc, sync::Arc};

use gpui::{
    App, Bounds, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement as _, RenderImage, RenderOnce, SharedString,
    Styled as _, Window, canvas, div, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{agent, style, theme};

/// One choice in a [`ThumbnailPicker`].
#[derive(Clone)]
pub struct Thumbnail {
    /// Its picture, made at the frame's device size (one texture pixel to
    /// a device pixel); None while it comes, or if it has none.
    pub picture: Option<Arc<RenderImage>>,
    /// What fills the frame without a picture (a plain colour's choice).
    pub color: Option<Hsla>,
    pub label: SharedString,
}

type ChoiceFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ThumbnailPicker {
    id: SharedString,
    items: Rc<[Thumbnail]>,
    selected: Option<usize>,
    picture: (f32, f32),
    width: f32,
    on_select: Option<ChoiceFn>,
    on_open: Option<ChoiceFn>,
}

struct PickerState {
    focus: FocusHandle,
}

impl ThumbnailPicker {
    pub fn new(id: impl Into<SharedString>, items: impl Into<Rc<[Thumbnail]>>, selected: Option<usize>) -> Self {
        ThumbnailPicker { id: id.into(), items: items.into(), selected, picture: (128.0, 72.0), width: 0.0, on_select: None, on_open: None }
    }

    /// The pictures' size (points; default 128×72, 16:9).
    pub fn picture_size(mut self, width: f32, height: f32) -> Self {
        self.picture = (width, height);
        self
    }

    /// The width it has (points): as many go side by side as fit, and it
    /// is as tall as their rows (`height_for`).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// One is chosen (a click, the arrows).
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// One is double-clicked.
    pub fn on_open(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }
}

/// A choice's cell (its frame and label under it), `picture` big.
pub fn cell_size(picture: (f32, f32)) -> (f32, f32) {
    (picture.0 + 4.0, picture.1 + 4.0 + m::TIGHT + m::LABEL_ROW)
}

/// How many cells go side by side in `width`.
pub fn columns_in(width: f32, picture: (f32, f32)) -> usize {
    let cell = cell_size(picture).0;
    (((width + m::SPACING) / (cell + m::SPACING)).floor() as usize).max(1)
}

/// The height `n` choices take in `width`.
pub fn height_for(n: usize, width: f32, picture: (f32, f32)) -> f32 {
    let rows = n.div_ceil(columns_in(width, picture));
    rows as f32 * cell_size(picture).1 + rows.saturating_sub(1) as f32 * m::SPACING
}

impl RenderOnce for ThumbnailPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state: Entity<PickerState> = window.use_keyed_state(SharedString::from(format!("olo-thumbnails-{}", self.id)), cx, |_, cx| PickerState { focus: cx.focus_handle().tab_stop(true) });
        let focus = state.read(cx).focus.clone();
        let focused = focus.is_focused(window);
        let ThumbnailPicker { id, items, selected, picture, width, on_select, on_open } = self;
        let width = if width > 0.0 { width } else { cell_size(picture).0 };
        let n = items.len();
        // (The columns the paint found, for the arrows.)
        let cols = Rc::new(std::cell::Cell::new(1usize));
        let painted_cols = cols.clone();
        let (select, open) = (on_select.clone(), on_open.clone());
        let paint_focus = focus.clone();
        let grid = canvas(
            move |b, window, _| {
                let width = f32::from(b.size.width);
                let k = columns_in(width, picture);
                let (cw, ch) = cell_size(picture);
                (0..n)
                    .map(|i| {
                        let at = point(b.left() + px((i % k) as f32 * (cw + m::SPACING)), b.top() + px((i / k) as f32 * (ch + m::SPACING)));
                        let r = Bounds { origin: at, size: size(px(cw), px(ch)) };
                        (r, window.insert_hitbox(r, gpui::HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |b, cells, window, cx| {
                // (A tab stop where it's drawn, in drawing order.)
                window.insert_tab_stop(&paint_focus);
                let p = theme::palette(cx);
                painted_cols.set(columns_in(f32::from(b.size.width), picture));
                for (i, (r, _)) in cells.iter().enumerate() {
                    let item = &items[i];
                    let on = selected == Some(i);
                    if on {
                        style::selection_plate(window, *r, &p);
                    }
                    let frame = style::snap(window, Bounds { origin: r.origin, size: size(px(picture.0 + 4.0), px(picture.1 + 4.0)) });
                    let well = style::inset(window, frame, 2.0);
                    style::fill_rect(window, well, style::c(p.document_background));
                    match &item.picture {
                        // Centred in the well, a texture pixel to a device
                        // pixel (made for it: letterboxed if it isn't its
                        // shape).
                        Some(img) => {
                            let s = window.scale_factor();
                            let isz = img.size(0);
                            let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
                            let at = style::snap_centred(window, well.center(), w.min(f32::from(well.size.width)), h.min(f32::from(well.size.height)));
                            let _ = window.paint_image(at, gpui::Corners::default(), img.clone(), 0, false);
                        }
                        None => {
                            if let Some(fill) = item.color {
                                style::fill_rect(window, well, fill);
                            }
                        }
                    }
                    style::bevel(window, style::inset(window, frame, 1.0), style::shade(&p, p.panel_background, 1.45), style::c(p.shine));
                    let label_y = frame.bottom() + px(m::TIGHT + m::LABEL_ROW / 2.0);
                    let (weight, ink) = if on { (FontWeight::BOLD, style::c(p.list_selected_item_text)) } else { (FontWeight::NORMAL, style::c(p.panel_text)) };
                    let t = style::label_fit(window, cx, r.center().x, label_y, style::Align::Center, item.label.clone(), style::PLAIN_SIZE, style::ui_font(weight), ink, r.size.width - px(4.0));
                    if on && paint_focus.is_focused(window) {
                        style::hline_above(window, t.left(), t.right(), t.bottom(), style::c(p.keyboard_navigation));
                    }
                    agent::record(window, cx, agent::Node { role: "RadioButton", name: Some(item.label.clone()), bounds: *r, enabled: true, clickable: true, selected: Some(on), checked: Some(on), ..Default::default() });
                }
                let (select, open, focus) = (select.clone(), open.clone(), paint_focus.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left {
                        return;
                    }
                    let Some(i) = cells.iter().position(|(_, h)| h.is_hovered(window)) else { return };
                    window.focus(&focus);
                    if e.click_count >= 2 {
                        if let Some(f) = &open {
                            f(i, window, cx);
                        }
                    } else if let Some(f) = &select {
                        f(i, window, cx);
                    }
                });
            },
        )
        .size_full();
        let keys_select = on_select;
        let _ = focused;
        div()
            .id(SharedString::from(format!("thumbnails-{id}")))
            .track_focus(&focus)
            .w(px(width))
            .h(px(height_for(n, width, picture)))
            .flex_shrink_0()
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                let (Some(sel), Some(f)) = (selected, keys_select.as_ref()) else { return };
                let k = cols.get().max(1) as isize;
                let step = match e.keystroke.key.as_str() {
                    "right" => 1,
                    "left" => -1,
                    "down" => k,
                    "up" => -k,
                    _ => return,
                };
                let next = sel as isize + step;
                if (0..n as isize).contains(&next) {
                    f(next as usize, window, cx);
                }
                cx.stop_propagation();
            })
            .child(grid)
    }
}
