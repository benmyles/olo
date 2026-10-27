//! Blackness, BuyNow, Icons and Spots.

use olo_kit::{
    egui::{Align2, Color32, FontId, Rect, pos2, vec2},
    icons, style,
};

use crate::{Clock, Frame, Info, Module, Opt, Rng, STEP, get_bool, get_f32, hsv};

// ---- Blackness ------------------------------------------------------------

pub const BLACKNESS: Info =
    Info { name: "Blackness", about: "Basic blackness", options: &[], uses_desktop: false, make: |_, _| Box::new(Blackness) };

struct Blackness;

impl Module for Blackness {
    fn draw(&mut self, f: &Frame) {
        f.painter.rect_filled(f.rect, 0.0, Color32::BLACK);
    }
}

// ---- BuyNow ---------------------------------------------------------------

pub const BUY_NOW: Info = Info {
    name: "BuyNow",
    about: "BuyNow, by Ficus Kirkpatrick",
    options: &[Opt::Check { key: "flash", label: "Flash", default: false }],
    uses_desktop: false,
    make: |o, _| Box::new(BuyNow { flash: get_bool(o, "flash", false), t: 0.0 }),
};

/// The BeDC '99 demo-machine classic: BUY NOW, white on blue.
struct BuyNow {
    flash: bool,
    t: f32,
}

pub const BUY_NOW_BLUE: Color32 = Color32::from_rgb(0, 0, 100);

impl Module for BuyNow {
    fn draw(&mut self, f: &Frame) {
        self.t += f.dt;
        f.painter.rect_filled(f.rect, 0.0, BUY_NOW_BLUE);
        if self.flash && self.t % 1.2 > 0.8 {
            return;
        }
        // Whole points, so the glyph atlas holds one size.
        let size = (f.rect.height() * 0.27).round().max(4.0);
        let font = FontId::new(size, style::bold());
        let c = f.rect.center();
        let gap = size * 0.55;
        f.painter.text(pos2(c.x, c.y - gap), Align2::CENTER_CENTER, "BUY", font.clone(), Color32::WHITE);
        f.painter.text(pos2(c.x, c.y + gap), Align2::CENTER_CENTER, "NOW", font, Color32::WHITE);
    }
}

// ---- Icons ----------------------------------------------------------------

pub const ICONS: Info = Info {
    name: "Icons",
    about: "Icons",
    options: &[Opt::Slider { key: "count", label: "Icons at a time", min: 2.0, max: 30.0, default: 12.0 }],
    uses_desktop: false,
    make: |o, seed| {
        Box::new(Icons { rng: Rng::new(seed), shown: Vec::new(), max: get_f32(o, "count", 12.0) as usize, next: 0.0 })
    },
};

/// The system's icons, appearing here and there and fading away.
const ICON_NAMES: &[&str] = &[
    "tracker",
    "terminal",
    "people",
    "mail",
    "music",
    "video",
    "image",
    "folder",
    "home",
    "disk",
    "system",
    "preferences",
    "calculator",
    "styled-edit",
    "browser",
    "network",
    "printer",
    "keyboard",
    "display",
    "sound",
    "power",
    "datetime",
    "activity",
    "archive",
    "document",
    "olo",
    "trash-full",
    "query",
    "screenshot",
    "workspaces",
];

struct Shown {
    name: &'static str,
    /// Position as a fraction of the rect, and size in points at k = 1.
    at: (f32, f32),
    size: f32,
    age: f32,
    life: f32,
}

struct Icons {
    rng: Rng,
    shown: Vec<Shown>,
    max: usize,
    next: f32,
}

const ICON_FADE: f32 = 0.6;

impl Module for Icons {
    fn draw(&mut self, f: &Frame) {
        f.painter.rect_filled(f.rect, 0.0, Color32::BLACK);
        self.next -= f.dt;
        for s in &mut self.shown {
            s.age += f.dt;
        }
        self.shown.retain(|s| s.age < s.life);
        if self.next <= 0.0 && self.shown.len() < self.max {
            let size = [32.0, 48.0, 64.0, 96.0, 128.0][self.rng.below(5)];
            self.shown.push(Shown {
                name: ICON_NAMES[self.rng.below(ICON_NAMES.len())],
                at: (self.rng.f32(), self.rng.f32()),
                size,
                age: 0.0,
                life: self.rng.range(3.0, 6.0),
            });
            self.next = self.rng.range(0.2, 0.6);
        }
        let k = f.k();
        for s in &self.shown {
            let alpha = (s.age / ICON_FADE).min((s.life - s.age) / ICON_FADE).clamp(0.0, 1.0);
            let size = s.size * k;
            let free = f.rect.size() - vec2(size, size);
            let r = Rect::from_min_size(f.rect.min + vec2(free.x * s.at.0, free.y * s.at.1), vec2(size, size));
            icons::paint_tinted(f.painter, r, s.name, Color32::from_white_alpha((alpha * 255.0) as u8));
        }
    }
}

// ---- Spots ----------------------------------------------------------------

pub const SPOTS: Info = Info {
    name: "Spots",
    about: "Spots",
    options: &[Opt::Slider { key: "rate", label: "Spots per second", min: 5.0, max: 120.0, default: 30.0 }],
    uses_desktop: false,
    make: |o, seed| {
        Box::new(Spots {
            rng: Rng::new(seed),
            spots: Vec::new(),
            rate: get_f32(o, "rate", 30.0),
            clock: Clock::new(),
            owed: 0.0,
            fading: None,
        })
    },
};

struct Spot {
    at: (f32, f32),
    radius: f32,
    color: Color32,
}

/// Coloured spots pile up until the screen is full, then fade away.
struct Spots {
    rng: Rng,
    spots: Vec<Spot>,
    rate: f32,
    clock: Clock,
    owed: f32,
    fading: Option<f32>,
}

const MAX_SPOTS: usize = 1500;

impl Module for Spots {
    fn draw(&mut self, f: &Frame) {
        for _ in 0..self.clock.steps(f.dt) {
            if let Some(t) = &mut self.fading {
                *t += STEP;
                if *t >= 1.5 {
                    self.spots.clear();
                    self.fading = None;
                }
                continue;
            }
            self.owed += self.rate * STEP;
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                let radius = 4.0 + 46.0 * self.rng.f32().powi(3);
                let color = hsv(self.rng.f32(), self.rng.range(0.5, 1.0), self.rng.range(0.6, 1.0));
                self.spots.push(Spot { at: (self.rng.f32(), self.rng.f32()), radius, color });
            }
            if self.spots.len() >= MAX_SPOTS {
                self.fading = Some(0.0);
            }
        }
        f.painter.rect_filled(f.rect, 0.0, Color32::BLACK);
        let fade = self.fading.map_or(1.0, |t| (1.0 - t / 1.5).max(0.0));
        let k = f.k();
        for s in &self.spots {
            let c = pos2(f.rect.left() + s.at.0 * f.rect.width(), f.rect.top() + s.at.1 * f.rect.height());
            f.painter.circle_filled(c, s.radius * k, s.color.gamma_multiply(fade));
        }
    }
}
