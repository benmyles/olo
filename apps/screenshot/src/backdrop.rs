//! A window's recording on a background (`--background`, Record… ▸
//! Window on a Background): each frame the window, its corners rounded,
//! on the person's wallpaper blurred and a little dimmed, a soft shadow
//! under it, as CleanShot X frames a window. The background is made once,
//! when the recording starts; the shadow again only when the window
//! changes size (then centred where it was); each frame only the window's
//! rows are copied in.

use std::path::PathBuf;

/// What the background is made of, as the settings say.
#[derive(Debug, Clone)]
pub struct Look {
    /// The wallpaper setting (`pack:NAME` or a picture file), if any.
    pub wallpaper: Option<String>,
    pub dark: bool,
    /// The desktop's colour: the background without a wallpaper.
    pub colour: [u8; 3],
    /// The window's display's scale: its corners' radius.
    pub scale: f64,
}

/// The margin around a `w` × `h` window: a share of its longer side,
/// within bounds (pixels), less where the video would pass 4096 (what
/// the encoders take).
pub fn margin(w: u32, h: u32) -> u32 {
    let m = ((w.max(h) as f32 * 0.06).round() as u32).clamp(32, 128);
    m.min(4096u32.saturating_sub(w.max(h)) / 2)
}

/// What every frame is drawn on.
pub struct Backdrop {
    /// The background without the shadow (BGRA, `width` × `height`).
    base: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The corners' radius (pixels; less for a window too small for it),
    /// and each corner pixel's coverage at the radius used (the top
    /// left's, r × r, 0–255).
    radius: usize,
    coverage: Vec<u8>,
    /// The window's size and place the canvas was last made for, and
    /// what's under each corner then (top left, top right, bottom left,
    /// bottom right).
    made_for: Option<[usize; 4]>,
    under: [Vec<u8>; 4],
}

impl Backdrop {
    /// For a window `w` × `h` (pixels).
    pub fn new(w: u32, h: u32, look: &Look) -> Backdrop {
        let m = margin(w, h);
        let (width, height) = olo_video::even(w + 2 * m, h + 2 * m);
        // (A pack picture's preview is enough, blurred to an eighth: a
        // tenth of the time of the picture itself.)
        let path: Option<PathBuf> = look.wallpaper.as_deref().map(str::trim).and_then(|s| match s.strip_prefix(olo_xdg::wallpapers::PACK) {
            Some(name) => olo_xdg::wallpapers::thumbnail(name, look.dark).or_else(|| olo_xdg::wallpapers::resolve(s, (width, height), look.dark)),
            None => olo_xdg::wallpapers::resolve(s, (width, height), look.dark),
        });
        let base = path.and_then(|p| picture(&p, width, height)).unwrap_or_else(|| gradient(width, height, look.colour));
        let radius = ((10.0 * look.scale).round() as usize).clamp(4, 40);
        Backdrop { base, width, height, radius, coverage: Vec::new(), made_for: None, under: Default::default() }
    }

    /// A frame: `data`'s `w` × `h` (BGRA, `stride` a row) on the
    /// background, into `canvas` (as big as the backdrop). The canvas
    /// keeps the background between frames: only the window is copied in,
    /// unless its size changed. Whether the background was made again,
    /// and where the window went (x, y, w, h).
    pub fn compose(&mut self, data: &[u8], stride: usize, w: u32, h: u32, canvas: &mut [u8]) -> (bool, [usize; 4]) {
        let (cw, ch) = (self.width as usize, self.height as usize);
        let (w, h) = ((w as usize).min(cw), (h as usize).min(ch));
        let (x0, y0) = ((cw - w) / 2, (ch - h) / 2);
        let r = self.radius.min(w / 2).min(h / 2);
        let anew = self.made_for != Some([x0, y0, w, h]);
        if anew {
            canvas.copy_from_slice(&self.base);
            shadow(canvas, cw, ch, [x0, y0, w, h], r);
            if self.coverage.len() != r * r {
                self.coverage = coverage(r);
            }
            // What shows through each corner.
            for (k, under) in self.under.iter_mut().enumerate() {
                let (cx, cy) = (if k % 2 == 0 { x0 } else { x0 + w - r }, if k < 2 { y0 } else { y0 + h - r });
                under.clear();
                for y in cy..cy + r {
                    let at = (y * cw + cx) * 4;
                    under.extend_from_slice(&canvas[at..at + r * 4]);
                }
            }
            self.made_for = Some([x0, y0, w, h]);
        }
        let row = cw * 4;
        for y in 0..h {
            let from = &data[y * stride..y * stride + w * 4];
            let to = &mut canvas[(y0 + y) * row + x0 * 4..(y0 + y) * row + (x0 + w) * 4];
            // A row in the corners: its ends blended with what's under
            // them (`ty`: rows from the corner's outer edge; `uy`: the
            // row of what's under).
            let (k, ty, uy) = if y < r {
                (0, y, y)
            } else if y >= h - r {
                (2, h - 1 - y, y - (h - r))
            } else {
                to.copy_from_slice(from);
                continue;
            };
            to[r * 4..(w - r) * 4].copy_from_slice(&from[r * 4..(w - r) * 4]);
            let (left, right) = (&self.under[k][uy * r * 4..(uy + 1) * r * 4], &self.under[k + 1][uy * r * 4..(uy + 1) * r * 4]);
            for i in 0..r {
                // (The left corner's column `i` from the edge; the right's
                // `r - 1 - i`.)
                for (x, tx, under) in [(i, i, &left[i * 4..i * 4 + 4]), (w - r + i, r - 1 - i, &right[i * 4..i * 4 + 4])] {
                    let a = u32::from(self.coverage[ty * r + tx]);
                    for c in 0..4 {
                        let (f, b) = (u32::from(from[x * 4 + c]), u32::from(under[c]));
                        to[x * 4 + c] = ((f * a + b * (255 - a) + 127) / 255) as u8;
                    }
                }
            }
        }
        (anew, [x0, y0, w, h])
    }
}

/// A rounded corner's coverage, `r` × `r`: row 0 the corner's outermost
/// row, column 0 the window's edge; 255 inside the curve, 0 outside, the
/// pixels it crosses by how far their centres are within it.
fn coverage(r: usize) -> Vec<u8> {
    let rf = r as f32;
    let mut out = Vec::with_capacity(r * r);
    for y in 0..r {
        for x in 0..r {
            let d = (rf - (x as f32 + 0.5)).hypot(rf - (y as f32 + 0.5));
            out.push(((rf - d + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

/// The wallpaper covering `w` × `h`, blurred and dimmed a little (BGRA).
fn picture(path: &std::path::Path, w: u32, h: u32) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.to_rgb8();
    let (iw, ih) = img.dimensions();
    // Covered (cropped to the shape), made small, blurred there (cheap,
    // and smooth once made big again).
    let k = (w as f32 / iw as f32).max(h as f32 / ih as f32);
    let (sw, sh) = (((w as f32 / k).round() as u32).clamp(1, iw), ((h as f32 / k).round() as u32).clamp(1, ih));
    let crop = image::imageops::crop_imm(&img, (iw - sw) / 2, (ih - sh) / 2, sw, sh).to_image();
    let small = image::imageops::resize(&crop, (w / 8).max(8), (h / 8).max(8), image::imageops::FilterType::Triangle);
    let blurred = image::imageops::blur(&small, 4.0);
    Some(enlarged(&blurred, w, h))
}

/// `small` made `w` × `h` (bilinear, fixed point), a little darker (the
/// window stands out), as BGRA: rows at once on a few threads (`image`'s
/// resize and a conversion took 150 ms at 4K; this, 15).
fn enlarged(small: &image::RgbImage, w: u32, h: u32) -> Vec<u8> {
    let (sw, sh) = small.dimensions();
    let src = small.as_raw();
    // Where each output column and row samples: the left (top) source
    // pixel and the right's (bottom's) weight out of 256, centres aligned.
    let at = |i: u32, n: u32, sn: u32| -> (usize, usize, u32) {
        let f = ((i as f32 + 0.5) * sn as f32 / n as f32 - 0.5).clamp(0.0, (sn - 1) as f32);
        let a = f as usize;
        (a, (a + 1).min(sn as usize - 1), ((f - a as f32) * 256.0) as u32)
    };
    let cols: Vec<_> = (0..w).map(|x| at(x, w, sw)).collect();
    let mut out = vec![255u8; w as usize * h as usize * 4];
    let row_bytes = w as usize * 4;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let rows_each = (h as usize).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (k, part) in out.chunks_mut(rows_each * row_bytes).enumerate() {
            let (cols, at) = (&cols, &at);
            scope.spawn(move || {
                // A source row blended for this output row, dimmed to
                // 220/255 (×256 for the weight).
                let mut line = vec![0u32; sw as usize * 3];
                for (i, row) in part.chunks_exact_mut(row_bytes).enumerate() {
                    let (t, b, wy) = at((k * rows_each + i) as u32, h, sh);
                    let (top, bottom) = (&src[t * sw as usize * 3..(t + 1) * sw as usize * 3], &src[b * sw as usize * 3..(b + 1) * sw as usize * 3]);
                    for ((l, &a), &b) in line.iter_mut().zip(top).zip(bottom) {
                        *l = (u32::from(a) * (256 - wy) + u32::from(b) * wy) * 220 / 255;
                    }
                    for (px, &(l, r, wx)) in row.as_chunks_mut::<4>().0.iter_mut().zip(cols) {
                        let (a, b) = (&line[l * 3..l * 3 + 3], &line[r * 3..r * 3 + 3]);
                        px[2] = ((a[0] * (256 - wx) + b[0] * wx) >> 16) as u8;
                        px[1] = ((a[1] * (256 - wx) + b[1] * wx) >> 16) as u8;
                        px[0] = ((a[2] * (256 - wx) + b[2] * wx) >> 16) as u8;
                    }
                }
            });
        }
    });
    out
}

/// Top to bottom, `c` a little lighter to a little darker (BGRA).
fn gradient(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let k = 1.12 - 0.3 * (y as f32 / h.max(1) as f32);
        let px = c.map(|v| (v as f32 * k).clamp(0.0, 255.0) as u8);
        for _ in 0..w {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    out
}

/// The shadow of the window at `[x, y, w, h]` (corners of radius `r`),
/// a little below it, darkening `pixels` (`cw` × `ch`) around it.
fn shadow(pixels: &mut [u8], cw: usize, ch: usize, [x, y, w, h]: [usize; 4], r: usize) {
    let spread = ((cw - w).min(ch - h) as f32 * 0.22).max(6.0);
    let drop = spread * 0.25;
    // The shadow's rounded rectangle: centre and half size, less its
    // corners' radius.
    let (mx, my) = (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0 + drop);
    let (hx, hy) = (w as f32 / 2.0 - r as f32, h as f32 / 2.0 - r as f32);
    let (top, bottom) = ((my - h as f32 / 2.0 - spread).max(0.0) as usize, ((my + h as f32 / 2.0 + spread).ceil() as usize).min(ch));
    let (left, right) = ((x as f32 - spread).max(0.0) as usize, ((x + w) as f32 + spread).ceil().min(cw as f32) as usize);
    for py in top..bottom {
        // (Rows the window covers but at its corners: only the sides.)
        let covered = py >= y + r && py < y + h - r;
        let spans = if covered { [left..x, x + w..right] } else { [left..right, 0..0] };
        for px in spans.into_iter().flatten() {
            let qx = ((px as f32 + 0.5 - mx).abs() - hx).max(0.0);
            let qy = ((py as f32 + 0.5 - my).abs() - hy).max(0.0);
            let d = ((qx * qx + qy * qy).sqrt() - r as f32).max(0.0);
            if d >= spread {
                continue;
            }
            let a = 0.5 * (1.0 - d / spread).powi(3);
            let i = (py * cw + px) * 4;
            for c in &mut pixels[i..i + 3] {
                *c = (f32::from(*c) * (1.0 - a)) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look() -> Look {
        Look { wallpaper: None, dark: false, colour: [40, 90, 160], scale: 1.0 }
    }

    #[test]
    fn a_window_on_its_backdrop() {
        let mut b = Backdrop::new(200, 100, &look());
        assert_eq!(margin(200, 100), 32);
        assert_eq!((b.width, b.height), (264, 164));
        let window = vec![255u8; 200 * 100 * 4];
        let mut out = vec![0u8; b.base.len()];
        b.compose(&window, 200 * 4, 200, 100, &mut out);
        let at = |out: &[u8], x: usize, y: usize| out[(y * 264 + x) * 4..(y * 264 + x) * 4 + 4].to_vec();
        // The window where it goes, the background around it, the shadow
        // under it darker than above it, the corners rounded.
        assert_eq!(at(&out, 32 + 5, 32 + 5), [255; 4]);
        assert_eq!(at(&out, 32, 32 + 50), [255; 4]);
        assert_ne!(at(&out, 2, 2), [255; 4]);
        assert_ne!(at(&out, 32, 32), [255; 4], "the corner's tip is the background's");
        let edge = at(&out, 32 + 3, 32 + 2)[0];
        assert!(edge > at(&out, 32, 32)[0] && edge < 255, "partly covered: {edge}");
        assert_ne!(at(&out, 32 + 199, 32 + 99), [255; 4], "the bottom right corner rounded too");
        assert_eq!(at(&out, 32 + 199, 32 + 50), [255; 4]);
        // The shadow: below the window darker (against the background
        // there) than above it.
        let shade = |out: &[u8], x: usize, y: usize| f32::from(at(out, x, y)[0]) / f32::from(b.base[(y * 264 + x) * 4]);
        let (below, above) = (shade(&out, 132, 32 + 100 + 4), shade(&out, 132, 32 - 4));
        assert!(below < above && below < 0.8, "{below} {above}");
        // Again, a new picture of the same size: the same around it.
        let darker = vec![10u8; 200 * 100 * 4];
        let before = out.clone();
        b.compose(&darker, 200 * 4, 200, 100, &mut out);
        assert_eq!(at(&out, 2, 2), at(&before, 2, 2));
        assert_eq!(at(&out, 32, 32), at(&before, 32, 32), "the corner's tip still the background's");
        assert_eq!(at(&out, 40, 40), [10; 4]);
    }

    #[test]
    fn a_window_that_shrinks_is_centred_again() {
        let mut b = Backdrop::new(200, 100, &look());
        let mut out = vec![0u8; b.base.len()];
        b.compose(&vec![255u8; 200 * 100 * 4], 200 * 4, 200, 100, &mut out);
        b.compose(&vec![255u8; 100 * 50 * 4], 100 * 4, 100, 50, &mut out);
        let at = |x: usize, y: usize| out[(y * 264 + x) * 4];
        // Centred: (264 - 100) / 2 = 82, (164 - 50) / 2 = 57.
        assert_eq!(at(82 + 20, 57 + 20), 255);
        assert_ne!(at(40, 40), 255, "the old window's place is background again");
    }

    #[test]
    fn margins_stop_at_what_encoders_take() {
        assert_eq!(margin(3840, 2160), 128);
        assert_eq!(margin(4000, 2000), 48);
        assert_eq!(margin(4096, 2000), 0);
    }
}
