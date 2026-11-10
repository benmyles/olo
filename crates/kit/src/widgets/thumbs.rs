//! A wrapping grid of pictures to pick one from, labelled underneath (a
//! screen to share, a wallpaper): each in a sunken frame, the chosen one
//! and its name on a selection plate. A click chooses, a double click
//! opens (shares at once); with the keyboard (a click gives it, or Tab)
//! the arrows move the choice. Agents read each as a `RadioButton`,
//! selected or not.

use std::{rc::Rc, sync::Arc};

use olo_interface::{
    Bounds, Corners, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, Pixels, RenderImage, SharedString, Styled as _, WindowCx, agent, control_canvas,
    point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{style, theme};

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

type ChoiceFn = Rc<dyn Fn(&usize, &mut WindowCx)>;

pub struct ThumbnailPicker {
    id: SharedString,
    items: Rc<[Thumbnail]>,
    selected: Option<usize>,
    picture: (f32, f32),
    width: f32,
    on_select: Option<ChoiceFn>,
    on_open: Option<ChoiceFn>,
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
    pub fn on_select(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// One is double-clicked.
    pub fn on_open(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
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

/// Each choice's cell in the picker's bounds `b`.
fn cells(b: Bounds<Pixels>, n: usize, picture: (f32, f32)) -> Vec<Bounds<Pixels>> {
    let k = columns_in(f32::from(b.size.width), picture);
    let (cw, ch) = cell_size(picture);
    (0..n).map(|i| Bounds { origin: point(b.left() + px((i % k) as f32 * (cw + m::SPACING)), b.top() + px((i / k) as f32 * (ch + m::SPACING))), size: size(px(cw), px(ch)) }).collect()
}

impl IntoElement for ThumbnailPicker {
    fn into_element(self) -> Element {
        let ThumbnailPicker { id, items, selected, picture, width, on_select, on_open } = self;
        let width = if width > 0.0 { width } else { cell_size(picture).0 };
        let n = items.len();
        let (select, open, keys) = (on_select.clone(), on_open, on_select);
        // (Its pictures by what they are, not what they show: one made
        // again is another.)
        let look = style::key((
            items.iter().map(|t| (t.picture.as_ref().map(|p| Arc::as_ptr(p) as usize), style::ink_key(t.color), t.label.clone())).collect::<Vec<_>>(),
            selected,
            picture.0.to_bits(),
            picture.1.to_bits(),
        ));
        control_canvas(move |g, b, state| {
            let p = theme::palette();
            for (i, r) in cells(b, n, picture).into_iter().enumerate() {
                let item = &items[i];
                let on = selected == Some(i);
                if on {
                    style::selection_plate(g, r, &p);
                }
                let frame = style::snap(g, Bounds { origin: r.origin, size: size(px(picture.0 + 4.0), px(picture.1 + 4.0)) });
                let well = style::inset(g, frame, 2.0);
                style::fill_rect(g, well, style::c(p.document_background));
                match &item.picture {
                    // Centred in the well, a texture pixel to a device
                    // pixel (made for it: letterboxed if it isn't its
                    // shape).
                    Some(img) => {
                        let s = g.scale_factor();
                        let isz = img.size(0);
                        let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
                        let at = style::snap_centred(g, well.center(), w.min(f32::from(well.size.width)), h.min(f32::from(well.size.height)));
                        let _ = g.paint_image(at, Corners::default(), img.clone(), 0, false);
                    }
                    None => {
                        if let Some(fill) = item.color {
                            style::fill_rect(g, well, fill);
                        }
                    }
                }
                let inner = style::inset(g, frame, 1.0);
                style::bevel(g, inner, style::shade(&p, p.panel_background, 1.45), style::c(p.shine));
                let label_y = frame.bottom() + px(m::TIGHT + m::LABEL_ROW / 2.0);
                let (weight, ink) = if on { (FontWeight::BOLD, style::c(p.list_selected_item_text)) } else { (FontWeight::NORMAL, style::c(p.panel_text)) };
                let t = style::label_fit(g, r.center().x, label_y, style::Align::Center, item.label.clone(), style::PLAIN_SIZE, style::ui_font(weight), ink, r.size.width - px(4.0));
                if on && state.focused {
                    style::hline_above(g, t.left(), t.right(), t.bottom(), style::c(p.keyboard_navigation));
                }
                g.agent(agent::Node { role: "RadioButton", name: Some(item.label.clone()), bounds: r, enabled: true, clickable: true, selected: Some(on), checked: Some(on), ..Default::default() });
            }
        })
        .paint_key(look)
        .id(SharedString::from(format!("thumbnails-{id}")))
        .w(px(width))
        .h(px(height_for(n, width, picture)))
        .flex_shrink_0()
        .focusable()
        .tab_stop()
        .on_mouse_down(MouseButton::Left, move |e, cx| {
            let Some(i) = cells(cx.listener_bounds(), n, picture).iter().position(|r| r.contains(&e.position)) else { return };
            if e.click_count >= 2 {
                if let Some(f) = &open {
                    f(&i, cx);
                }
            } else if let Some(f) = &select {
                f(&i, cx);
            }
        })
        .on_key_down(move |e: &KeyDownEvent, cx| {
            let (Some(sel), Some(f)) = (selected, keys.as_ref()) else { return };
            let k = columns_in(f32::from(cx.listener_bounds().size.width), picture) as isize;
            let step = match e.keystroke.key.as_str() {
                "right" => 1,
                "left" => -1,
                "down" => k,
                "up" => -k,
                _ => return,
            };
            let next = sel as isize + step;
            if (0..n as isize).contains(&next) {
                f(&(next as usize), cx);
            }
            cx.stop_propagation();
        })
    }
}
