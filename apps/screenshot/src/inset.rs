//! The camera in a window's recording on a background (`--background`
//! with `--camera`): the on-screen bubble (`bubble.rs`) can't be in it, a
//! window's capture being the window alone, so the recorder opens the
//! camera itself and draws it on each frame, a disc on the background's
//! bottom right corner over the window's, ringed as the bubble is: the
//! middle square, mirrored, the latest frame the camera has given (a
//! worker keeps it, scaled to the disc once as it comes).
//!
//! The backdrop keeps the background between frames and copies in only
//! the window, so what the disc covers outside the window is kept from
//! when the background was made (`under`) and put back each frame before
//! the disc is drawn again: its soft edge never piles up on itself.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// The disc: its side (BGRA, premultiplied by its coverage) and the
/// coverage of each pixel (0–255, the ring's included).
struct Disc {
    side: usize,
    bgra: Vec<u8>,
}

pub struct Inset {
    /// The latest frame, scaled to `side` and masked to the disc.
    latest: Arc<Mutex<Option<Disc>>>,
    stop: Arc<AtomicBool>,
    pub name: String,
    side: usize,
    /// Where the disc goes on the canvas (x, y), and what's under it
    /// outside the window, as the background was made (for the window's
    /// place `at`).
    at: (usize, usize),
    under: Vec<u8>,
    made_for: Option<[usize; 4]>,
}

/// The disc's side for a `w` × `h` video: a fifth of its shorter side,
/// within bounds (pixels).
fn side_for(w: u32, h: u32) -> usize {
    ((w.min(h) as f32 * 0.2).round() as usize).clamp(96, 360)
}

/// The ring round the disc (the bubble's: the panel's light grey), and
/// its width (pixels).
const RING: [u8; 3] = [0xe0, 0xe0, 0xe0];

impl Inset {
    /// Open camera `id` ("auto": the first colour camera) for a `w` × `h`
    /// video, once its first frame is in.
    pub fn start(id: &str, w: u32, h: u32) -> Result<Inset, String> {
        let id = (id != "auto").then_some(id);
        let mut camera = olo_camera::open(id, (640, 480))?;
        let name = camera.info.name.clone();
        let side = side_for(w, h);
        let ring = (side / 64).max(2);
        let latest: Arc<Mutex<Option<Disc>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (first_tx, first) = std::sync::mpsc::channel::<Result<(), String>>();
        {
            let (latest, stop) = (latest.clone(), stop.clone());
            std::thread::Builder::new()
                .name("recording camera".into())
                .spawn(move || {
                    let mask = mask(side, ring);
                    let mut told = false;
                    while !stop.load(Ordering::Relaxed) {
                        match camera.frame() {
                            Ok(f) => {
                                let Some(sq) = crate::bubble::square(&f) else { continue };
                                let small = image::imageops::resize(&sq, side as u32, side as u32, image::imageops::FilterType::Triangle);
                                let disc = disc(small.as_raw(), side, ring, &mask);
                                *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(disc);
                                if !std::mem::replace(&mut told, true) {
                                    let _ = first_tx.send(Ok(()));
                                }
                            }
                            Err(e) => {
                                if !told {
                                    let _ = first_tx.send(Err(e));
                                }
                                return;
                            }
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        match first.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("the camera: {e}")),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                return Err("the camera took too long to start".into());
            }
        }
        let inset = (w.min(h) as usize / 40).max(12);
        let at = ((w as usize).saturating_sub(side + inset), (h as usize).saturating_sub(side + inset));
        Ok(Inset { latest, stop, name, side, at, under: Vec::new(), made_for: None })
    }

    /// Draw the latest frame onto `canvas` (`cw` wide, BGRA), the window
    /// at `window` (x, y, w, h) on it; `anew`: the background was just
    /// made again (what's under the disc is taken then).
    pub fn draw(&mut self, canvas: &mut [u8], cw: usize, window: [usize; 4], anew: bool) {
        let (s, (ax, ay)) = (self.side, self.at);
        if ay + s > canvas.len() / (cw * 4) || ax + s > cw {
            return;
        }
        let row = cw * 4;
        let inside = |x: usize, y: usize| x >= window[0] && x < window[0] + window[2] && y >= window[1] && y < window[1] + window[3];
        if anew || self.made_for != Some(window) {
            self.under.clear();
            for y in ay..ay + s {
                self.under.extend_from_slice(&canvas[y * row + ax * 4..y * row + (ax + s) * 4]);
            }
            self.made_for = Some(window);
        } else {
            // What it covered outside the window, back as it was.
            for y in 0..s {
                for x in 0..s {
                    if !inside(ax + x, ay + y) {
                        let (c, u) = ((ay + y) * row + (ax + x) * 4, (y * s + x) * 4);
                        canvas[c..c + 4].copy_from_slice(&self.under[u..u + 4]);
                    }
                }
            }
        }
        let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        let Some(d) = latest.as_ref().filter(|d| d.side == s) else { return };
        for y in 0..s {
            for x in 0..s {
                let p = (y * s + x) * 4;
                let a = d.bgra[p + 3] as u32;
                if a == 0 {
                    continue;
                }
                let c = (ay + y) * row + (ax + x) * 4;
                for k in 0..3 {
                    canvas[c + k] = (d.bgra[p + k] as u32 + canvas[c + k] as u32 * (255 - a) / 255).min(255) as u8;
                }
                canvas[c + 3] = 255;
            }
        }
    }
}

impl Drop for Inset {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Each pixel's coverage of a disc `side` across (four samples a side
/// within each pixel), and whether it's in the ring (`ring` pixels wide
/// at the edge): (coverage 0–255, ring share 0–255).
fn mask(side: usize, ring: usize) -> Vec<(u8, u8)> {
    let r = side as f32 / 2.0;
    let inner = r - ring as f32;
    let mut out = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            let (mut cover, mut ringed) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let (px, py) = (x as f32 + (sx as f32 + 0.5) / 4.0 - r, y as f32 + (sy as f32 + 0.5) / 4.0 - r);
                    let d = (px * px + py * py).sqrt();
                    if d <= r {
                        cover += 1;
                        if d > inner {
                            ringed += 1;
                        }
                    }
                }
            }
            out.push(((cover * 255 / 16) as u8, (ringed * 255 / 16) as u8));
        }
    }
    out
}

/// The frame `bgra` (`side` square) as the disc: premultiplied by its
/// coverage, the ring's share in the ring's colour.
fn disc(bgra: &[u8], side: usize, _ring: usize, mask: &[(u8, u8)]) -> Disc {
    let mut out = vec![0u8; side * side * 4];
    for (i, &(cover, ringed)) in mask.iter().enumerate() {
        if cover == 0 {
            continue;
        }
        let p = i * 4;
        let rs = ringed as u32;
        for k in 0..3 {
            // (The ring's colour over the picture by its share, then the
            // whole by the coverage.)
            let v = (bgra[p + k] as u32 * (255 - rs) + RING[k] as u32 * rs) / 255;
            out[p + k] = (v * cover as u32 / 255) as u8;
        }
        out[p + 3] = cover;
    }
    Disc { side, bgra: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disc_is_round_and_ringed() {
        let m = mask(100, 3);
        // The middle covered, not ringed; the corners empty; the edge ringed.
        assert_eq!(m[50 * 100 + 50], (255, 0));
        assert_eq!(m[0].0, 0);
        assert!(m[50 * 100 + 1].1 > 0);
        let d = disc(&vec![10u8; 100 * 100 * 4], 100, 3, &m);
        assert_eq!(&d.bgra[(50 * 100 + 50) * 4..(50 * 100 + 50) * 4 + 4], &[10, 10, 10, 255]);
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// What a frame costs the recorder: run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn draw_per_frame() {
        let (cw, ch) = (1920usize, 1200usize);
        let side = side_for(cw as u32, ch as u32);
        let m = mask(side, 3);
        let frame = vec![128u8; side * side * 4];
        let latest = Arc::new(Mutex::new(Some(disc(&frame, side, 3, &m))));
        let mut inset = Inset { latest, stop: Arc::new(AtomicBool::new(false)), name: String::new(), side, at: (cw - side - 30, ch - side - 30), under: Vec::new(), made_for: None };
        let mut canvas = vec![50u8; cw * ch * 4];
        let window = [100, 100, 1700, 1000];
        inset.draw(&mut canvas, cw, window, true);
        let t = std::time::Instant::now();
        for _ in 0..100 {
            inset.draw(&mut canvas, cw, window, false);
        }
        eprintln!("draw: {:.3} ms a frame ({side} px disc)", t.elapsed().as_secs_f64() * 10.0);
        let sq = image::RgbaImage::from_raw(480, 480, vec![90u8; 480 * 480 * 4]).unwrap();
        let t = std::time::Instant::now();
        for _ in 0..30 {
            let small = image::imageops::resize(&sq, side as u32, side as u32, image::imageops::FilterType::Triangle);
            let _ = disc(small.as_raw(), side, 3, &m);
        }
        eprintln!("each camera frame scaled and masked: {:.3} ms", t.elapsed().as_secs_f64() * 1000.0 / 30.0);
    }
}
