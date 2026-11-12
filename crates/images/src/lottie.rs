//! Lottie animations: Telegram's animated stickers (`.tgs`, gzipped
//! Lottie JSON, 512 × 512 at 60 frames a second, three seconds at most)
//! and plain `.json` ones. Read by velato (vendored with its panics on
//! real files fixed: `vendor/README.md`) and drawn by vello_cpu, both
//! pure Rust, a frame at a time at the size it's shown at (one pixel to a
//! device pixel: nothing resampled), in a [`Painter`] that keeps its
//! buffers from frame to frame. Telegram Desktop's 52 animations load,
//! and draw at 0.5 ms a frame on average at 336 pixels (a sticker at 2×),
//! 4.5 ms at worst.
//!
//! The files come from anyone: reading and drawing are each kept from
//! taking the program down if velato panics on one (`catch_unwind`), and
//! a file over [`MOST_BYTES`] unpacked isn't read.

use std::{io::Read, ops::Range, panic::AssertUnwindSafe, time::Duration};

use velato::{Composition, RenderSink, Renderer, model::ImageAsset};
use vello_cpu::kurbo::{self, Affine, BezPath, Shape};
use vello_cpu::peniko;
use vello_cpu::{Pixmap, RenderContext, Resources};

/// The most a file may unpack to (Telegram's stickers are 64 KB packed).
pub const MOST_BYTES: u64 = 16 << 20;

/// A Lottie animation, read.
pub struct Lottie {
    comp: Composition,
}

impl Lottie {
    /// From a file's bytes: gzipped (a `.tgs`) or not.
    pub fn read(bytes: &[u8]) -> Result<Lottie, String> {
        let json = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes).take(MOST_BYTES + 1).read_to_end(&mut out).map_err(|e| format!("Lottie: {e}"))?;
            if out.len() as u64 > MOST_BYTES {
                return Err("Lottie: too large".into());
            }
            out
        } else {
            bytes.to_vec()
        };
        let comp = std::panic::catch_unwind(|| Composition::from_slice(&json)).map_err(|_| "Lottie: velato couldn’t read it".to_owned())?.map_err(|e| format!("Lottie: {e}"))?;
        // (NaN rates and spans count as none.)
        let frames = comp.frames.end - comp.frames.start;
        if comp.width == 0 || comp.height == 0 || comp.frame_rate.is_nan() || comp.frame_rate <= 0.0 || frames.is_nan() || frames <= 0.0 {
            return Err("Lottie: no frames".into());
        }
        Ok(Lottie { comp })
    }

    pub fn open(path: &std::path::Path) -> Result<Lottie, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Lottie::read(&bytes)
    }

    /// Its own size, in its units (a sticker's 512 × 512).
    pub fn size(&self) -> (u32, u32) {
        (self.comp.width as u32, self.comp.height as u32)
    }

    /// Frames a second.
    pub fn frame_rate(&self) -> f64 {
        self.comp.frame_rate
    }

    /// Its frames (by number, the end not drawn).
    pub fn frames(&self) -> Range<f64> {
        self.comp.frames.clone()
    }

    /// How long it plays, once through.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64((self.comp.frames.end - self.comp.frames.start) / self.comp.frame_rate)
    }

    /// The frame at `t` into it (looping), as a frame number to [`Painter::paint`].
    pub fn frame_at(&self, t: Duration) -> f64 {
        let n = (self.comp.frames.end - self.comp.frames.start).max(1.0);
        self.comp.frames.start + (t.as_secs_f64() * self.comp.frame_rate).floor() % n
    }
}

/// Frames drawn at one size, the buffers kept for the next.
pub struct Painter {
    w: u16,
    h: u16,
    ctx: RenderContext,
    res: Resources,
    pix: Pixmap,
    renderer: Renderer,
}

impl Painter {
    pub fn new(w: u16, h: u16) -> Painter {
        let (w, h) = (w.max(1), h.max(1));
        Painter { w, h, ctx: RenderContext::new(w, h), res: Resources::default(), pix: Pixmap::new(w, h), renderer: Renderer::new() }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }

    /// Frame `frame` of `lottie`, fitted to the painter's size (centred,
    /// its shape kept): RGBA rows, premultiplied.
    pub fn paint(&mut self, lottie: &Lottie, frame: f64) -> Result<&[u8], String> {
        let (cw, ch) = (lottie.comp.width as f64, lottie.comp.height as f64);
        let k = (f64::from(self.w) / cw).min(f64::from(self.h) / ch);
        let at = Affine::translate(((f64::from(self.w) - cw * k) / 2.0, (f64::from(self.h) - ch * k) / 2.0)) * Affine::scale(k);
        let Painter { ctx, res, pix, renderer, .. } = self;
        let drawn = std::panic::catch_unwind(AssertUnwindSafe(|| {
            ctx.reset();
            renderer.append(&lottie.comp, frame, at, 1.0, &mut Sink(ctx));
            ctx.flush();
            pix.data_as_u8_slice_mut().fill(0);
            ctx.render(&mut *pix, res);
        }));
        if drawn.is_err() {
            // (Its state may be half done: begun again next time.)
            *self = Painter::new(self.w, self.h);
            return Err("Lottie: velato couldn’t draw it".into());
        }
        Ok(self.pix.data_as_u8_slice())
    }
}

/// A premultiplied pixel straight (most are opaque or empty: as they
/// are, no dividing).
fn straight(p: [u8; 4]) -> [u8; 4] {
    match p[3] {
        255 => p,
        0 => [0; 4],
        a => {
            let a = u32::from(a);
            let un = |c: u8| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8;
            [un(p[0]), un(p[1]), un(p[2]), p[3]]
        }
    }
}

/// Premultiplied RGBA to straight, in place.
pub fn unpremultiply(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        *p = straight(*p);
    }
}

/// Premultiplied RGBA to straight BGRA (the kit's pictures), in place.
pub fn to_straight_bgra(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let [r, g, b, a] = straight(*p);
        *p = [b, g, r, a];
    }
}

/// Its first frame, at its own size (up to 1024 on its longer side), as
/// a picture: for [`crate::open`].
pub fn first_frame(path: &std::path::Path) -> Result<image::DynamicImage, String> {
    let l = Lottie::open(path)?;
    let (w, h) = l.size();
    let k = (1024.0 / f64::from(w.max(h))).min(1.0);
    let (w, h) = (((f64::from(w) * k).round() as u16).max(1), ((f64::from(h) * k).round() as u16).max(1));
    let mut p = Painter::new(w, h);
    let mut px = p.paint(&l, l.frames().start)?.to_vec();
    unpremultiply(&mut px);
    image::RgbaImage::from_raw(u32::from(w), u32::from(h), px).map(image::DynamicImage::ImageRgba8).ok_or_else(|| "Lottie: no picture".to_owned())
}

/// velato's drawing, done by vello_cpu.
struct Sink<'a>(&'a mut RenderContext);

fn path(shape: &impl Shape) -> BezPath {
    shape.to_path(0.1)
}

impl RenderSink for Sink<'_> {
    fn push_layer(&mut self, blend: impl Into<peniko::BlendMode>, alpha: f32, transform: Affine, shape: &impl Shape) {
        self.0.set_transform(transform);
        self.0.push_layer(Some(&path(shape)), Some(blend.into()), Some(alpha), None, None);
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl Shape) {
        self.0.set_transform(transform);
        self.0.push_clip_layer(&path(shape));
    }

    fn pop_layer(&mut self) {
        self.0.pop_layer();
    }

    fn draw(&mut self, stroke: Option<&kurbo::Stroke>, transform: Affine, brush: &peniko::Brush, shape: &impl Shape) {
        self.0.set_transform(transform);
        self.0.set_paint_transform(Affine::IDENTITY);
        match brush {
            peniko::Brush::Solid(c) => self.0.set_paint(*c),
            peniko::Brush::Gradient(g) => self.0.set_paint(g.clone()),
            // (Telegram's stickers have no pictures in them.)
            _ => return,
        }
        let p = path(shape);
        match stroke {
            Some(s) => {
                self.0.set_stroke(s.clone());
                self.0.stroke_path(&p);
            }
            None => {
                self.0.set_fill_rule(peniko::Fill::NonZero);
                self.0.fill_path(&p);
            }
        }
    }

    fn draw_image(&mut self, _: &ImageAsset, _: Affine, _: f64) {}
}

/// An animation of Olo's own, for the Telegram demo's sticker and for
/// tests: a heart that beats twice, filled with a gradient and outlined,
/// a ring drawn round it and taken away again (trimmed), two seconds at
/// 60 frames a second, 512 × 512 as a sticker is.
pub const HEART: &str = include_str!("lottie_heart.json");

/// [`HEART`] gzipped, as a `.tgs` is.
pub fn heart_tgs() -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = std::io::Write::write_all(&mut gz, HEART.as_bytes());
    gz.finish().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn the_heart_beats() {
        let l = Lottie::read(HEART.as_bytes()).unwrap();
        assert_eq!(l.size(), (512, 512));
        assert_eq!(l.frames(), 0.0..120.0);
        assert_eq!(l.duration(), Duration::from_secs(2));
        let mut p = Painter::new(168, 168);
        // Its middle red, the corners empty.
        let rest = p.paint(&l, 0.0).unwrap().to_vec();
        let mid = at(&rest, 168, 84, 96);
        assert!(mid[3] == 255 && mid[0] > 180 && mid[1] < 160, "middle {mid:?}");
        assert_eq!(at(&rest, 168, 2, 2), [0, 0, 0, 0]);
        // At the top of the beat it's bigger: covering more.
        let beat = p.paint(&l, 15.0).unwrap().to_vec();
        let covered = |px: &[u8]| px.as_chunks::<4>().0.iter().filter(|q| q[3] > 128).count();
        assert!(covered(&beat) > covered(&rest) * 11 / 10, "{} against {}", covered(&beat), covered(&rest));
        // The ring: none at the start, some halfway.
        assert!(covered(&p.paint(&l, 60.0).unwrap().to_vec()) > covered(&rest));
    }

    #[test]
    fn gzipped_as_a_tgs() {
        let l = Lottie::read(&heart_tgs()).unwrap();
        assert_eq!(l.frame_rate(), 60.0);
        assert_eq!(l.frame_at(Duration::from_millis(2500)), 30.0);
    }

    #[test]
    fn what_isnt_lottie() {
        assert!(Lottie::read(b"not json").is_err());
        assert!(Lottie::read(b"{}").is_err());
        assert!(Lottie::read(&[0x1f, 0x8b, 0, 0]).is_err());
    }

    #[test]
    fn straight_bgra() {
        let mut px = [128, 64, 0, 128, 0, 0, 0, 0, 10, 20, 30, 255, 9, 9, 9, 0];
        to_straight_bgra(&mut px);
        assert_eq!(px, [0, 128, 255, 128, 0, 0, 0, 0, 30, 20, 10, 255, 0, 0, 0, 0]);
    }
}
