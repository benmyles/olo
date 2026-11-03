//! Flip and Lens, which play with the desktop as it was when the saver
//! started. Without a picture of it (the Preferences preview before one is
//! taken) they use the plain desktop colour.

use crate::{Clock, Color32, Frame, Info, Module, Opt, Pos2, Rng, STEP, Vec2, get_f32, pos2, vec2};

/// The plain desktop colour, without a picture of it.
const DESKTOP: Color32 = Color32::from_rgb(51, 102, 152);

// ---- Flip -----------------------------------------------------------------

pub const FLIP: Info = Info {
    name: "Flip",
    about: "Flip",
    options: &[Opt::Slider { key: "pause", label: "Pause (seconds)", min: 0.0, max: 10.0, default: 2.0 }],
    uses_desktop: true,
    make: |o, seed| Box::new(Flip { rng: Rng::new(seed), angle: 0.0, pause: get_f32(o, "pause", 2.0), wait: 1.0, vertical: false, clock: Clock::new() }),
};

/// The screen turns over like a card, now on one axis, now the other,
/// resting face up (or upside down) between turns.
struct Flip {
    rng: Rng,
    /// Rotation so far in this turn, 0–π.
    angle: f32,
    pause: f32,
    wait: f32,
    /// Turning about the vertical axis (else the horizontal).
    vertical: bool,
    clock: Clock,
}

/// Strips the card is cut into, so the perspective looks right.
const STRIPS: usize = 32;

impl Module for Flip {
    fn draw(&mut self, f: &Frame) {
        use std::f32::consts::PI;
        for _ in 0..self.clock.steps(f.dt) {
            if self.wait > 0.0 {
                self.wait -= STEP;
                continue;
            }
            self.angle += STEP * PI / 2.5;
            if self.angle >= PI {
                self.angle = 0.0;
                self.wait = self.pause;
                self.vertical = self.rng.f32() < 0.5;
            }
        }
        f.canvas.rect_filled(f.rect, 0.0, Color32::BLACK);
        let (w, h) = (f.rect.width(), f.rect.height());
        let c = f.rect.center();
        // Camera distance: a gentle perspective.
        let d = 2.5 * w.max(h);
        let (cos, sin) = (self.angle.cos(), self.angle.sin());
        // Shade the card as it turns away from the light.
        let light = 0.35 + 0.65 * cos.abs();
        let project = |u: f32, v: f32| -> Pos2 {
            // u, v in -0.5..0.5 across the card.
            let (x, y, z) = if self.vertical { (u * w * cos, v * h, u * w * sin) } else { (u * w, v * h * cos, v * h * sin) };
            let p = d / (d + z);
            c + vec2(x * p, y * p)
        };
        let mut triangles = Vec::with_capacity(STRIPS * 2);
        for i in 0..STRIPS {
            let (a, b) = (i as f32 / STRIPS as f32, (i + 1) as f32 / STRIPS as f32);
            let quad = if self.vertical { [(a, 0.0), (b, 0.0), (b, 1.0), (a, 1.0)] } else { [(0.0, a), (1.0, a), (1.0, b), (0.0, b)] };
            let corner = |(u, v): (f32, f32)| (project(u - 0.5, v - 0.5), pos2(u, v));
            let [p0, p1, p2, p3] = quad.map(corner);
            triangles.push([p0, p1, p2]);
            triangles.push([p0, p2, p3]);
        }
        if f.canvas.has_desktop() {
            f.canvas.desktop_triangles(&triangles, light);
        } else {
            let flat: Vec<[Pos2; 3]> = triangles.iter().map(|t| t.map(|(p, _)| p)).collect();
            let shade = |c: u8| (c as f32 * light).round() as u8;
            f.canvas.triangles(&flat, Color32::from_rgb(shade(DESKTOP.r()), shade(DESKTOP.g()), shade(DESKTOP.b())));
        }
    }
}

// ---- Lens -----------------------------------------------------------------

pub const LENS: Info = Info {
    name: "Lens",
    about: "Lens",
    options: &[Opt::Slider { key: "size", label: "Size", min: 0.1, max: 0.4, default: 0.2 }, Opt::Slider { key: "power", label: "Magnification", min: 1.5, max: 4.0, default: 2.0 }],
    uses_desktop: true,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let v = (rng.range(0.0025, 0.004), rng.range(0.002, 0.0035));
        Box::new(Lens { at: (rng.range(0.3, 0.7), rng.range(0.3, 0.7)), v, size: get_f32(o, "size", 0.2), power: get_f32(o, "power", 2.0), clock: Clock::new() })
    },
};

/// A magnifying glass drifting over the desktop.
struct Lens {
    at: (f32, f32),
    v: (f32, f32),
    /// Radius as a fraction of the screen height.
    size: f32,
    power: f32,
    clock: Clock,
}

impl Module for Lens {
    fn draw(&mut self, f: &Frame) {
        let r = self.size * f.rect.height();
        let (rw, rh) = (r / f.rect.width(), r / f.rect.height());
        for _ in 0..self.clock.steps(f.dt) {
            self.at.0 += self.v.0;
            self.at.1 += self.v.1;
            if self.at.0 < rw || self.at.0 > 1.0 - rw {
                self.v.0 = -self.v.0;
                self.at.0 = self.at.0.clamp(rw, 1.0 - rw);
            }
            if self.at.1 < rh || self.at.1 > 1.0 - rh {
                self.v.1 = -self.v.1;
                self.at.1 = self.at.1.clamp(rh, 1.0 - rh);
            }
        }
        let textured = f.canvas.has_desktop();
        // The desktop itself.
        if textured {
            f.canvas.desktop(f.rect);
        } else {
            f.canvas.rect_filled(f.rect, 0.0, DESKTOP);
        }
        // The glass: a disc showing the desktop around its centre, enlarged.
        let centre = pos2(f.rect.left() + self.at.0 * f.rect.width(), f.rect.top() + self.at.1 * f.rect.height());
        if textured {
            let uv_c = pos2(self.at.0, self.at.1);
            let uv_r = Vec2::new(rw, rh) / self.power;
            const SEGMENTS: usize = 96;
            let rim = |i: usize| {
                let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                let (s, c) = a.sin_cos();
                (centre + vec2(c, s) * r, uv_c + vec2(c * uv_r.x, s * uv_r.y))
            };
            let fan: Vec<[(Pos2, Pos2); 3]> = (0..SEGMENTS).map(|i| [(centre, uv_c), rim(i), rim(i + 1)]).collect();
            f.canvas.desktop_triangles(&fan, 1.0);
        } else {
            f.canvas.circle_filled(centre, r, DESKTOP);
        }
        // Rim, a glint and a soft shadow line.
        let k = f.k();
        f.canvas.circle_stroke(centre, r, 4.0 * k, Color32::from_gray(40));
        f.canvas.circle_stroke(centre, r - 3.0 * k, 1.5 * k, Color32::from_white_alpha(90));
        let glint = centre + vec2(-0.45, -0.5) * r;
        f.canvas.circle_filled(glint, 0.12 * r, Color32::from_white_alpha(40));
    }
}
