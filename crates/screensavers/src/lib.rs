//! The screen saver modules of BeOS R5, redrawn for Tabula: Blackness,
//! BuyNow, Flip, Icons, Lens, Lissart, Message, Spiral, Spots, StringTheme
//! and SuperString (the list in R5's ScreenSaver preferences).
//!
//! A module draws a whole frame into a rect with an egui painter, so the
//! same code runs full screen in `tabula-screensaver` and in the little
//! monitor of Preferences ▸ Screen Saver. Sizes scale with the rect
//! ([`Frame::k`]); motion is simulated in fixed steps, so it looks the same
//! at any frame rate.

use std::collections::BTreeMap;

use tabula_kit::egui::{Color32, Painter, Rect, TextureId, Vec2, ecolor::HsvaGamma};

mod basic;
mod desktop;
mod lines;
mod message;

/// One frame to draw.
pub struct Frame<'a> {
    pub painter: &'a Painter,
    pub rect: Rect,
    /// Seconds since the last frame.
    pub dt: f32,
    /// The desktop as it was when the saver started, with its size in
    /// pixels (Lens and Flip work on it).
    pub desktop: Option<(TextureId, Vec2)>,
}

impl Frame<'_> {
    /// Size factor: 1 on a 900-point-high screen.
    pub fn k(&self) -> f32 {
        (self.rect.height() / 900.0).max(0.02)
    }

    /// One device pixel, in points (hairlines).
    pub fn pixel(&self) -> f32 {
        1.0 / self.painter.ctx().pixels_per_point()
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

/// A colour from hue, saturation and value as people see them (sRGB).
pub fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    HsvaGamma { h: h.rem_euclid(1.0), s, v, a: 1.0 }.into()
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

    /// Every module draws frames headless (painting into a context with no
    /// renderer) without panicking.
    #[test]
    fn all_draw() {
        let ctx = tabula_kit::egui::Context::default();
        for m in MODULES {
            let mut module = m.make(&Options::new(), 7);
            for _ in 0..3 {
                let mut out = ctx.run_ui(Default::default(), |ui| {
                    let f = Frame {
                        painter: ui.painter(),
                        rect: Rect::from_min_size(Default::default(), tabula_kit::egui::vec2(640.0, 400.0)),
                        dt: 0.5,
                        desktop: None,
                    };
                    module.draw(&f);
                });
                out.textures_delta.clear();
            }
        }
    }
}
