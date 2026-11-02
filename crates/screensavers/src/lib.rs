//! The screen saver modules of BeOS R5, redrawn for Olo: Blackness,
//! BuyNow, Flip, Icons, Lens, Lissart, Message, Spiral, Spots, StringTheme
//! and SuperString (the list in R5's ScreenSaver preferences).
//!
//! A module draws a whole frame into a rect through a [`Canvas`], so the
//! same code runs full screen in `olo-screensaver` and in the little
//! monitor of Preferences ▸ Screen Saver, on whichever toolkit draws it
//! (`gpui_canvas`, and `egui_canvas` while Preferences is on the egui
//! kit). Sizes scale with the rect ([`Frame::k`]); motion is simulated in
//! fixed steps, so it looks the same at any frame rate.

use std::collections::BTreeMap;

mod basic;
mod desktop;
#[cfg(feature = "egui")]
pub mod egui_canvas;
pub mod geom;
#[cfg(feature = "gpui")]
pub mod gpui_canvas;
mod lines;
mod message;

pub use geom::{Color32, Pos2, Rect, Vec2, hsv, pos2, vec2};

/// Where a piece of text goes, from the point it's placed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Its middle.
    Center,
    /// Its left end, halfway up.
    LeftCenter,
}

/// What a module draws with: a toolkit's painter, clipped to the frame.
/// Methods take `&self` (an implementation keeps what it paints with in
/// a cell) so a module's frame can be shared while it draws.
pub trait Canvas {
    fn rect_filled(&self, r: Rect, radius: f32, color: Color32);
    fn line_segment(&self, a: Pos2, b: Pos2, width: f32, color: Color32);
    /// Joined lines through `points`.
    fn polyline(&self, points: &[Pos2], width: f32, color: Color32);
    fn circle_filled(&self, center: Pos2, radius: f32, color: Color32);
    fn circle_stroke(&self, center: Pos2, radius: f32, width: f32, color: Color32);
    /// Flat triangles.
    fn triangles(&self, triangles: &[[Pos2; 3]], color: Color32);
    /// A word in bold, `size` points high (hundreds of pixels: a toolkit
    /// may show it once it's made), placed by `anchor` at `at`.
    fn big_text(&self, word: &str, size: f32, at: Pos2, anchor: Anchor, color: Color32);
    /// The size `text` takes in the mono font at `size`, wrapped to `width`.
    fn measure(&self, text: &str, size: f32, width: f32) -> Vec2;
    /// `text` in the mono font, wrapped to `width`, its top left at `at`.
    fn text(&self, at: Pos2, text: &str, size: f32, width: f32, color: Color32);
    /// An icon of the system's, faded to `alpha`.
    fn icon(&self, r: Rect, name: &str, alpha: f32);
    /// There's a picture of the desktop (Flip, Lens).
    fn has_desktop(&self) -> bool;
    /// The desktop picture, filling `r`.
    fn desktop(&self, r: Rect);
    /// Triangles of the desktop picture, each corner where it goes and
    /// where in the picture (0–1 across) it shows, darkened to `light`
    /// (1 as it is).
    fn desktop_triangles(&self, triangles: &[[(Pos2, Pos2); 3]], light: f32);
    /// One device pixel, in points (hairlines).
    fn pixel(&self) -> f32;
}

/// One frame to draw.
pub struct Frame<'a> {
    pub canvas: &'a dyn Canvas,
    pub rect: Rect,
    /// Seconds since the last frame.
    pub dt: f32,
}

impl Frame<'_> {
    /// Size factor: 1 on a 900-point-high screen.
    pub fn k(&self) -> f32 {
        (self.rect.height() / 900.0).max(0.02)
    }

    /// One device pixel, in points (hairlines).
    pub fn pixel(&self) -> f32 {
        self.canvas.pixel()
    }
}

pub trait Module: Send {
    fn draw(&mut self, f: &Frame);
}

/// A setting a module offers in Preferences.
pub enum Opt {
    Check { key: &'static str, label: &'static str, default: bool },
    Choice { key: &'static str, label: &'static str, choices: &'static [&'static str], default: usize },
    Slider { key: &'static str, label: &'static str, min: f32, max: f32, default: f32 },
    Text { key: &'static str, label: &'static str, default: &'static str },
}

/// Module settings as stored in Settings (strings, so any kind fits).
pub type Options = BTreeMap<String, String>;

pub fn get_bool(o: &Options, key: &str, default: bool) -> bool {
    o.get(key).map_or(default, |v| v == "true")
}

pub fn get_f32(o: &Options, key: &str, default: f32) -> f32 {
    o.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn get_usize(o: &Options, key: &str, default: usize) -> usize {
    o.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn get_text<'a>(o: &'a Options, key: &str, default: &'a str) -> &'a str {
    o.get(key).map_or(default, String::as_str)
}

pub struct Info {
    pub name: &'static str,
    /// The line Preferences shows above the settings (R5 credited authors
    /// there, e.g. "SuperString, by Jon Watte").
    pub about: &'static str,
    pub options: &'static [Opt],
    /// Whether it draws the desktop (the saver then grabs it first).
    pub uses_desktop: bool,
    make: fn(&Options, u64) -> Box<dyn Module>,
}

impl Info {
    pub fn make(&self, options: &Options, seed: u64) -> Box<dyn Module> {
        (self.make)(options, seed)
    }
}

/// Every module, in R5's order.
pub static MODULES: &[Info] = &[
    basic::BLACKNESS,
    basic::BUY_NOW,
    desktop::FLIP,
    basic::ICONS,
    desktop::LENS,
    lines::LISSART,
    message::MESSAGE,
    lines::SPIRAL,
    basic::SPOTS,
    lines::STRING_THEME,
    lines::SUPER_STRING,
];

pub fn find(name: &str) -> Option<&'static Info> {
    MODULES.iter().find(|m| m.name == name)
}

/// A seed from the clock, for a fresh show each time.
pub fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1
}

/// xorshift64*: small, fast, and plenty for pictures.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// Runs `step` at a fixed 60 Hz for the time that has passed.
pub(crate) struct Clock {
    left: f32,
}

pub(crate) const STEP: f32 = 1.0 / 60.0;

impl Clock {
    pub fn new() -> Clock {
        Clock { left: 0.0 }
    }

    /// How many steps to run now (capped, so a stall doesn't spin).
    pub fn steps(&mut self, dt: f32) -> usize {
        self.left = (self.left + dt.clamp(0.0, 0.25)).min(0.25);
        let n = (self.left / STEP) as usize;
        self.left -= n as f32 * STEP;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_are_r5s() {
        let names: Vec<&str> = MODULES.iter().map(|m| m.name).collect();
        assert_eq!(
            names,
            [
                "Blackness",
                "BuyNow",
                "Flip",
                "Icons",
                "Lens",
                "Lissart",
                "Message",
                "Spiral",
                "Spots",
                "StringTheme",
                "SuperString"
            ]
        );
        assert!(find("SuperString").is_some_and(|m| m.about == "SuperString, by Jon Watte"));
    }

    /// app_server grabs the desktop for these by name (saver.rs
    /// DESKTOP_MODULES); keep the two in step.
    #[test]
    fn desktop_modules() {
        let names: Vec<&str> = MODULES.iter().filter(|m| m.uses_desktop).map(|m| m.name).collect();
        assert_eq!(names, ["Flip", "Lens"]);
    }

    #[test]
    fn rng_spreads() {
        let mut r = Rng::new(42);
        let xs: Vec<f32> = (0..1000).map(|_| r.f32()).collect();
        assert!(xs.iter().all(|x| (0.0..1.0).contains(x)));
        let mean = xs.iter().sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }

    #[test]
    fn clock_steps_fixed() {
        let mut c = Clock::new();
        let n: usize = (0..60).map(|_| c.steps(1.0 / 30.0)).sum();
        assert!((119..=121).contains(&n));
        // A stall runs at most a quarter second's worth.
        assert!((14..=15).contains(&c.steps(10.0)));
    }

    /// Records what a module draws (and nothing else).
    #[derive(Default)]
    struct Count(std::cell::Cell<usize>);

    impl Canvas for Count {
        fn rect_filled(&self, _: Rect, _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn line_segment(&self, _: Pos2, _: Pos2, _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn polyline(&self, _: &[Pos2], _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn circle_filled(&self, _: Pos2, _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn circle_stroke(&self, _: Pos2, _: f32, _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn triangles(&self, _: &[[Pos2; 3]], _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn big_text(&self, _: &str, _: f32, _: Pos2, _: Anchor, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn measure(&self, text: &str, size: f32, _: f32) -> Vec2 {
            vec2(text.len() as f32 * size * 0.6, size)
        }
        fn text(&self, _: Pos2, _: &str, _: f32, _: f32, _: Color32) {
            self.0.set(self.0.get() + 1);
        }
        fn icon(&self, _: Rect, _: &str, _: f32) {
            self.0.set(self.0.get() + 1);
        }
        fn has_desktop(&self) -> bool {
            false
        }
        fn desktop(&self, _: Rect) {}
        fn desktop_triangles(&self, _: &[[(Pos2, Pos2); 3]], _: f32) {}
        fn pixel(&self) -> f32 {
            0.5
        }
    }

    /// Every module draws frames (into a canvas that counts) without
    /// panicking, and draws something.
    #[test]
    fn all_draw() {
        for m in MODULES {
            let mut module = m.make(&Options::new(), 7);
            let canvas = Count::default();
            for _ in 0..3 {
                module.draw(&Frame { canvas: &canvas, rect: Rect::from_min_size(Pos2::default(), vec2(640.0, 400.0)), dt: 0.5 });
            }
            assert!(canvas.0.get() > 0, "{} drew nothing", m.name);
        }
    }
}
