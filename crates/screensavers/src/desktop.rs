//! Flip and Lens, which play with the desktop as it was when the saver
//! started. Without a picture of it (the Preferences preview before one is
//! taken) they use the plain desktop colour.

use olo_kit::egui::{Color32, Mesh, Pos2, Rect, Shape, Stroke, TextureId, Vec2, epaint::Vertex, pos2, vec2};

use crate::{Clock, Frame, Info, Module, Opt, Rng, STEP, get_f32};

const DESKTOP: Color32 = Color32::from_rgb(51, 102, 152);

/// The texture and the uv to use: the desktop picture, or the font
/// atlas's white pixel tinted the desktop colour.
fn source(f: &Frame) -> (TextureId, bool) {
    match f.desktop {
        Some((id, _)) => (id, true),
        None => (TextureId::default(), false),
    }
}

fn vertex(pos: Pos2, uv: Pos2, textured: bool, tint: Color32) -> Vertex {
    if textured {
        Vertex { pos, uv, color: tint }
    } else {
        Vertex { pos, uv: olo_kit::egui::epaint::WHITE_UV, color: DESKTOP.gamma_multiply(tint.r() as f32 / 255.0) }
    }
}

// ---- Flip -----------------------------------------------------------------

pub const FLIP: Info = Info {
    name: "Flip",
    about: "Flip",
    options: &[Opt::Slider { key: "pause", label: "Pause (seconds)", min: 0.0, max: 10.0, default: 2.0 }],
    uses_desktop: true,
    make: |o, seed| {
        Box::new(Flip {
            rng: Rng::new(seed),
            angle: 0.0,
            pause: get_f32(o, "pause", 2.0),
            wait: 1.0,
            vertical: false,
            clock: Clock::new(),
        })
    },
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
        f.painter.rect_filled(f.rect, 0.0, Color32::BLACK);
        let (tex, textured) = source(f);
        let (w, h) = (f.rect.width(), f.rect.height());
        let c = f.rect.center();
        // Camera distance: a gentle perspective.
        let d = 2.5 * w.max(h);
        let (cos, sin) = (self.angle.cos(), self.angle.sin());
        // Shade the card as it turns away from the light.
        let light = 0.35 + 0.65 * cos.abs();
        let tint = Color32::from_gray((light * 255.0) as u8);
        let project = |u: f32, v: f32| -> Pos2 {
            // u, v in -0.5..0.5 across the card.
            let (x, y, z) = if self.vertical {
                (u * w * cos, v * h, u * w * sin)
            } else {
                (u * w, v * h * cos, v * h * sin)
            };
            let p = d / (d + z);
            c + vec2(x * p, y * p)
        };
        let mut mesh = Mesh::with_texture(tex);
        for i in 0..STRIPS {
            let (a, b) = (i as f32 / STRIPS as f32, (i + 1) as f32 / STRIPS as f32);
            let base = mesh.vertices.len() as u32;
            let quad = if self.vertical {
                [(a, 0.0), (b, 0.0), (b, 1.0), (a, 1.0)]
            } else {
                [(0.0, a), (1.0, a), (1.0, b), (0.0, b)]
            };
            for (u, v) in quad {
                mesh.vertices.push(vertex(project(u - 0.5, v - 0.5), pos2(u, v), textured, tint));
            }
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        f.painter.add(Shape::mesh(mesh));
    }
}

// ---- Lens -----------------------------------------------------------------

pub const LENS: Info = Info {
    name: "Lens",
    about: "Lens",
    options: &[
        Opt::Slider { key: "size", label: "Size", min: 0.1, max: 0.4, default: 0.2 },
        Opt::Slider { key: "power", label: "Magnification", min: 1.5, max: 4.0, default: 2.0 },
    ],
    uses_desktop: true,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let v = (rng.range(0.0025, 0.004), rng.range(0.002, 0.0035));
        Box::new(Lens {
            at: (rng.range(0.3, 0.7), rng.range(0.3, 0.7)),
            v,
            size: get_f32(o, "size", 0.2),
            power: get_f32(o, "power", 2.0),
            clock: Clock::new(),
        })
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
        let (tex, textured) = source(f);
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
        // The desktop itself.
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if textured {
            f.painter.image(tex, f.rect, full, Color32::WHITE);
        } else {
            f.painter.rect_filled(f.rect, 0.0, DESKTOP);
        }
        // The glass: a disc showing the desktop around its centre, enlarged.
        let centre = pos2(f.rect.left() + self.at.0 * f.rect.width(), f.rect.top() + self.at.1 * f.rect.height());
        let uv_c = pos2(self.at.0, self.at.1);
        let uv_r = Vec2::new(rw, rh) / self.power;
        let mut mesh = Mesh::with_texture(tex);
        mesh.vertices.push(vertex(centre, uv_c, textured, Color32::WHITE));
        const SEGMENTS: usize = 96;
        for i in 0..=SEGMENTS {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            mesh.vertices.push(vertex(centre + vec2(c, s) * r, uv_c + vec2(c * uv_r.x, s * uv_r.y), textured, Color32::WHITE));
            if i > 0 {
                mesh.indices.extend_from_slice(&[0, i as u32, i as u32 + 1]);
            }
        }
        f.painter.add(Shape::mesh(mesh));
        // Rim, a glint and a soft shadow line.
        let k = f.k();
        f.painter.circle_stroke(centre, r, Stroke::new(4.0 * k, Color32::from_gray(40)));
        f.painter.circle_stroke(centre, r - 3.0 * k, Stroke::new(1.5 * k, Color32::from_white_alpha(90)));
        let glint = centre + vec2(-0.45, -0.5) * r;
        f.painter.circle_filled(glint, 0.12 * r, Color32::from_white_alpha(40));
    }
}
