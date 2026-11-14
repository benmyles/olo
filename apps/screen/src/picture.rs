//! A screen's picture in a sunken well (the Live tab's, the History
//! tab's): fitted whole, centred, a line round it, or a word in its place
//! ("Connecting…"). A picture is drawn exactly: each frame is resampled
//! on a worker to the very device size it's shown at (Lanczos) and drawn
//! a texture pixel to a device pixel; while one is made (a new frame, a
//! resize) the last stands in, and what it replaced goes from the
//! window's atlas. Frames shown live (`set_live`) are another program's
//! dmabufs the compositor shows in the well's place
//! (`Window::place_external`, at whole points), each as it comes, the
//! window drawing nothing for them. Where the picture went is kept for
//! what goes over it (clicks forwarded to the screen, a replay's rings).

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use olo_kit::{
    Bounds, Cx, Element, FontWeight, Painter, ParentElement as _, Pixels, RenderImage, SharedString, Styled as _, View, agent, div, metrics as m, point, px, style, theme, widgets::image_view::Rgba,
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

/// What the well is told: its drawing wants a frame resampled, or a
/// worker made one.
pub enum Msg {
    Asked,
    Made(Made, Option<Arc<RenderImage>>),
}

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
    making: Option<Made>,
    /// What the last drawing lacked (a drawing can only look): made by
    /// the view, off its looper.
    asked: std::cell::Cell<Option<Made>>,
    me: olo_kit::Messenger<Msg>,
    /// Where the picture went when last painted.
    shown: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    name: SharedString,
    /// Frames shown live, of this size (pixels), under `key`.
    live: Option<(u32, u32)>,
    key: u64,
}

impl PictureWell {
    pub fn new(name: impl Into<SharedString>, cx: &mut Cx<Self>) -> PictureWell {
        static KEYS: AtomicU64 = AtomicU64::new(1);
        PictureWell {
            shot: None,
            generation: 0,
            shape: None,
            text: None,
            exact: None,
            making: None,
            asked: Default::default(),
            me: cx.messenger(),
            shown: Default::default(),
            name: name.into(),
            live: None,
            key: KEYS.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Show frames live, of `size` (`Window::set_external_buffer` under
    /// [`PictureWell::external_key`] gives them): drawn again only when
    /// that changes.
    pub fn set_live(&mut self, size: (u32, u32), cx: &mut Cx<Self>) {
        if self.live != Some(size) || self.text.is_some() || self.shot.is_some() {
            self.live = Some(size);
            self.text = None;
            self.shot = None;
            self.generation += 1;
            self.making = None;
            self.exact = None;
            cx.invalidate();
        }
    }

    /// What the frames shown live are given to the window under.
    pub fn external_key(&self) -> u64 {
        self.key
    }

    /// Show `shot` (a new frame).
    pub fn set_shot(&mut self, shot: Arc<Shot>, cx: &mut Cx<Self>) {
        self.shot = Some(shot);
        self.live = None;
        self.generation += 1;
        self.text = None;
        cx.invalidate();
    }

    /// Show nothing: `text` in its place (None: just the well).
    pub fn clear(&mut self, text: Option<SharedString>, cx: &mut Cx<Self>) {
        if self.shot.is_some() || self.live.is_some() || self.text != text {
            self.shot = None;
            self.live = None;
            self.generation += 1;
            self.making = None;
            self.exact = None;
            self.text = text;
            cx.invalidate();
        }
    }

    /// Say `text` in the picture's place, keeping the frame for when it's
    /// wanted again (None: show the frame).
    pub fn set_text(&mut self, text: Option<SharedString>, cx: &mut Cx<Self>) {
        if self.text != text {
            self.text = text;
            cx.invalidate();
        }
    }

    /// The screen's logical size, for its shape before a frame comes.
    pub fn set_shape(&mut self, shape: Option<(f32, f32)>) {
        self.shape = shape;
    }

    pub fn has_shot(&self) -> bool {
        self.shot.is_some() || self.live.is_some()
    }

    /// Where the picture was last drawn (None: no picture showed), for
    /// what's drawn over it to read as it draws (after the well has, in
    /// the same frame) and for what's pressed on it.
    pub fn shown_at(&self) -> std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>> {
        self.shown.clone()
    }

    fn paint(&self, well: Bounds<Pixels>, g: &mut Painter) {
        let p = theme::palette();
        style::sunken(g, well, &p, false);
        let inner = style::inset(g, well, 2.0);
        style::fill_rect(g, inner, style::shade(&p, p.panel_background, 1.18));
        let picture = self.shot.as_ref().filter(|_| self.text.is_none()).cloned();
        let live = self.live.filter(|_| self.text.is_none());
        let (w, h) = match (&picture, live) {
            (Some(s), _) => (s.size[0] as f32, s.size[1] as f32),
            (None, Some((w, h))) => (w as f32, h as f32),
            (None, None) => self.shape.unwrap_or((16.0, 10.0)),
        };
        let aspect = w / h.max(1.0);
        let room = Bounds::from_corners(inner.origin + point(px(m::SPACING), px(m::SPACING)), inner.bottom_right() - point(px(m::SPACING), px(m::SPACING)));
        let (rw, rh) = (f32::from(room.size.width).max(1.0), f32::from(room.size.height).max(1.0));
        let (pw, ph) = if rw / rh > aspect { (rh * aspect, rh) } else { (rw, rw / aspect) };
        let pic = style::snap_centred(g, room.center(), pw, ph);
        if let Some((lw, lh)) = live {
            // Where the compositor can show it: at whole points.
            let pic = g.external_bounds(pic);
            self.shown.set(Some(pic));
            let hair = style::hairline(g);
            style::fill_rect(g, Bounds::from_corners(pic.origin - point(hair, hair), pic.bottom_right() + point(hair, hair)), style::c(p.control_border));
            g.place_external(self.key, pic);
            agent::record(g, agent::Node { role: "Image", name: Some(self.name.clone()), value: Some(format!("{lw} × {lh}").into()), bounds: pic, enabled: true, ..Default::default() });
            return;
        }
        let Some(shot) = picture else {
            self.shown.set(None);
            let text = self.text.clone().unwrap_or_default();
            if !text.is_empty() {
                let r = style::label_fit(
                    g,
                    inner.center().x,
                    inner.center().y,
                    style::Align::Center,
                    text.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::NORMAL),
                    style::disabled_text(&p),
                    inner.size.width - px(2.0 * m::SPACING),
                );
                agent::record(g, agent::Node::label(text, r));
            }
            return;
        };
        self.shown.set(Some(pic));
        let hair = style::hairline(g);
        let frame = Bounds::from_corners(pic.origin - point(hair, hair), pic.bottom_right() + point(hair, hair));
        style::fill_rect(g, frame, style::c(p.control_border));
        let s = g.scale_factor();
        let device = ((f32::from(pic.size.width) * s).round().max(1.0) as u32, (f32::from(pic.size.height) * s).round().max(1.0) as u32);
        let want = (self.generation, device);
        // Made for this frame at this size: drawn as it is. Else the last
        // one stands in (stretched, for the moment it takes) and this one
        // is asked for.
        if let Some((_, img)) = &self.exact {
            let _ = g.paint_image(pic, Default::default(), img.clone(), 0, false);
        }
        if self.exact.as_ref().is_none_or(|(made, _)| *made != want) && self.making != Some(want) && self.asked.replace(Some(want)) != Some(want) {
            self.me.send(Msg::Asked);
        }
        agent::record(
            g,
            agent::Node { role: "Image", name: Some(self.name.clone()), value: Some(format!("{} × {}", shot.size[0], shot.size[1]).into()), bounds: pic, enabled: true, ..Default::default() },
        );
    }
}

impl View for PictureWell {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Asked => {
                let Some(want) = self.asked.take() else { return };
                let Some(src) = self.shot.clone().filter(|_| want.0 == self.generation && self.making != Some(want)) else { return };
                self.making = Some(want);
                let from = (src.size[0] as u32, src.size[1] as u32);
                cx.work(move || olo_kit::pictures::resampled(&src.rgba, from, want.1), move |img| Msg::Made(want, img));
            }
            Msg::Made(want, img) => {
                if self.making == Some(want) {
                    self.making = None;
                }
                if let Some(img) = img {
                    self.exact = Some((want, img));
                    cx.invalidate();
                }
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div().size_full().child(cx.canvas(|v: &PictureWell, g, b| v.paint(b, g)).size_full())
    }
}
