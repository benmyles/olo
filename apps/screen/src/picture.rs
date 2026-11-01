//! A screen's picture in a sunken well (the Live tab's, the History
//! tab's): fitted whole, centred, a line round it, or a word in its place
//! ("Connecting…"). Drawn exactly: each frame is resampled on a worker to
//! the very device size it's shown at (Lanczos) and drawn a texture pixel
//! to a device pixel; while one is made (a new frame, a resize) the last
//! stands in, and what it replaced goes from the window's atlas. Where
//! the picture went is kept for what goes over it (clicks forwarded to
//! the screen, a replay's rings).

use std::sync::Arc;

use olo_ui::{
    gpui::{AppContext as _, Bounds, Context, FontWeight, IntoElement, ParentElement as _, Pixels, Render, RenderImage, SharedString, Styled as _, Task, Window, canvas, div, point, px},
    metrics as m,
    style, theme,
    widgets::image_view::{self, Rgba},
    agent,
};

/// A screen's pixels: RGBA, opaque (so as good as premultiplied).
pub struct Shot {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

impl Rgba for Shot {
    fn size(&self) -> [usize; 2] {
        self.size
    }
    fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// A resampling: of which frame, at what device size.
type Made = (u64, (u32, u32));

pub struct PictureWell {
    shot: Option<Arc<Shot>>,
    /// Counts frames shown, so what was made for one isn't kept for the
    /// next.
    generation: u64,
    /// The screen's shape while there's no frame (its logical size).
    shape: Option<(f32, f32)>,
    /// Said in the picture's place.
    text: Option<SharedString>,
    exact: Option<(Made, Arc<RenderImage>)>,
    making: Option<(Made, Task<()>)>,
    /// Pictures to take out of the window's atlas when next painted.
    dropped: Vec<Arc<RenderImage>>,
    /// Where the picture went when last painted.
    shown: Option<Bounds<Pixels>>,
    name: SharedString,
}

impl PictureWell {
    pub fn new(name: impl Into<SharedString>) -> PictureWell {
        PictureWell { shot: None, generation: 0, shape: None, text: None, exact: None, making: None, dropped: Vec::new(), shown: None, name: name.into() }
    }

    /// Show `shot` (a new frame).
    pub fn set_shot(&mut self, shot: Arc<Shot>, cx: &mut Context<Self>) {
        self.shot = Some(shot);
        self.generation += 1;
        self.text = None;
        cx.notify();
    }

    /// Show nothing: `text` in its place (None: just the well).
    pub fn clear(&mut self, text: Option<SharedString>, cx: &mut Context<Self>) {
        if self.shot.is_some() || self.text != text {
            self.shot = None;
            self.generation += 1;
            self.making = None;
            self.dropped.extend(self.exact.take().map(|(_, i)| i));
            self.text = text;
            cx.notify();
        }
    }

    /// Say `text` in the picture's place, keeping the frame for when it's
    /// wanted again (None: show the frame).
    pub fn set_text(&mut self, text: Option<SharedString>, cx: &mut Context<Self>) {
        if self.text != text {
            self.text = text;
            cx.notify();
        }
    }

    /// The screen's logical size, for its shape before a frame comes.
    pub fn set_shape(&mut self, shape: Option<(f32, f32)>) {
        self.shape = shape;
    }

    pub fn has_shot(&self) -> bool {
        self.shot.is_some()
    }

    /// Where the picture was last drawn (None: no picture showed).
    pub fn shown(&self) -> Option<Bounds<Pixels>> {
        self.shown
    }

    fn paint(&mut self, well: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        for img in self.dropped.drain(..) {
            let _ = window.drop_image(img);
        }
        let p = theme::palette(cx);
        style::sunken(window, well, &p, false);
        let inner = style::inset(window, well, 2.0);
        style::fill_rect(window, inner, style::shade(&p, p.panel_background, 1.18));
        let picture = self.shot.as_ref().filter(|_| self.text.is_none()).cloned();
        let (w, h) = match &picture {
            Some(s) => (s.size[0] as f32, s.size[1] as f32),
            None => self.shape.unwrap_or((16.0, 10.0)),
        };
        let aspect = w / h.max(1.0);
        let room = Bounds::from_corners(inner.origin + point(px(m::SPACING), px(m::SPACING)), inner.bottom_right() - point(px(m::SPACING), px(m::SPACING)));
        let (rw, rh) = (f32::from(room.size.width).max(1.0), f32::from(room.size.height).max(1.0));
        let (pw, ph) = if rw / rh > aspect { (rh * aspect, rh) } else { (rw, rw / aspect) };
        let pic = style::snap_centred(window, room.center(), pw, ph);
        let Some(shot) = picture else {
            self.shown = None;
            let text = self.text.clone().unwrap_or_default();
            if !text.is_empty() {
                let r = style::label_fit(window, cx, inner.center().x, inner.center().y, style::Align::Center, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), inner.size.width - px(2.0 * m::SPACING));
                agent::record(window, cx, agent::Node::label(text, r));
            }
            return;
        };
        self.shown = Some(pic);
        let frame = Bounds::from_corners(pic.origin - point(style::hairline(window), style::hairline(window)), pic.bottom_right() + point(style::hairline(window), style::hairline(window)));
        style::fill_rect(window, frame, style::c(p.control_border));
        let s = window.scale_factor();
        let device = ((f32::from(pic.size.width) * s).round().max(1.0) as u32, (f32::from(pic.size.height) * s).round().max(1.0) as u32);
        let want = (self.generation, device);
        // Made for this frame at this size: drawn as it is. Else the last
        // one stands in (stretched, for the moment it takes) and this one
        // is made.
        if let Some((_, img)) = &self.exact {
            let _ = window.paint_image(pic, Default::default(), img.clone(), 0, false);
        }
        if self.exact.as_ref().is_none_or(|(made, _)| *made != want) && self.making.as_ref().is_none_or(|(made, _)| *made != want) {
            let src = shot.clone();
            let task = cx.background_spawn(async move { image_view::resampled(&*src, device) });
            let making = cx.spawn(async move |me, cx| {
                let img = task.await;
                let _ = me.update(cx, |v, cx| {
                    v.making = None;
                    if let Some(img) = img {
                        if let Some((_, old)) = v.exact.replace((want, img)) {
                            v.dropped.push(old);
                        }
                        cx.notify();
                    }
                });
            });
            self.making = Some((want, making));
        }
        agent::record(window, cx, agent::Node { role: "Image", name: Some(self.name.clone()), value: Some(format!("{} × {}", shot.size[0], shot.size[1]).into()), bounds: pic, enabled: true, ..Default::default() });
    }
}

impl Render for PictureWell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        div().size_full().child(canvas(|_, _, _| {}, move |b, _, window, cx| me.update(cx, |v, cx| v.paint(b, window, cx))).size_full())
    }
}
