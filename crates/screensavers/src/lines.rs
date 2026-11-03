//! The line-art modules: Lissart, Spiral, StringTheme and SuperString.

use std::collections::VecDeque;

use crate::{Clock, Color32, Frame, Info, Module, Opt, Pos2, Rect, Rng, STEP, get_bool, get_f32, hsv, pos2, vec2};

/// A point as a fraction of the rect (0–1 both ways).
type Unit = (f32, f32);

fn place(f: &Frame, p: Unit) -> Pos2 {
    pos2(f.rect.left() + p.0 * f.rect.width(), f.rect.top() + p.1 * f.rect.height())
}

/// Bounce `p` off the walls of the unit square.
fn bounce(p: &mut Unit, v: &mut Unit) {
    p.0 += v.0;
    p.1 += v.1;
    if !(0.0..=1.0).contains(&p.0) {
        v.0 = -v.0;
        p.0 = p.0.clamp(0.0, 1.0);
    }
    if !(0.0..=1.0).contains(&p.1) {
        v.1 = -v.1;
        p.1 = p.1.clamp(0.0, 1.0);
    }
}

// ---- Lissart --------------------------------------------------------------

pub const LISSART: Info = Info {
    name: "Lissart",
    about: "Lissart, by Christopher Tate",
    options: &[Opt::Slider { key: "lines", label: "Lines", min: 100.0, max: 800.0, default: 480.0 }, Opt::Slider { key: "saturation", label: "Saturation", min: 0.0, max: 1.0, default: 0.6 }],
    uses_desktop: false,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let figure = Figure::random(&mut rng);
        Box::new(Lissart { rng, figure, old: None, t: 0.0, age: 0.0, lines: get_f32(o, "lines", 480.0) as usize, saturation: get_f32(o, "saturation", 0.6) })
    },
};

/// Two points on Lissajous curves; the art is the line between them,
/// drawn over a stretch of time.
#[derive(Clone)]
struct Figure {
    /// Per point: x and y frequencies and phases.
    freq: [(f32, f32); 2],
    phase: [(f32, f32); 2],
    hue: f32,
    /// How fast the figure turns over (phase drift).
    drift: f32,
}

impl Figure {
    fn random(rng: &mut Rng) -> Figure {
        let f = |rng: &mut Rng| (1 + rng.below(4)) as f32;
        let p = |rng: &mut Rng| rng.range(0.0, std::f32::consts::TAU);
        Figure { freq: [(f(rng), f(rng)), (f(rng), f(rng))], phase: [(p(rng), p(rng)), (p(rng), p(rng))], hue: rng.f32(), drift: rng.range(0.05, 0.15) }
    }

    fn point(&self, i: usize, s: f32, t: f32) -> Unit {
        let (fx, fy) = self.freq[i];
        let (px, py) = self.phase[i];
        let d = self.drift * t;
        (0.5 + 0.48 * (fx * s + px + d).sin(), 0.5 + 0.48 * (fy * s + py - d).sin())
    }

    fn draw(&self, f: &Frame, t: f32, lines: usize, saturation: f32, alpha: f32) {
        // Square, centred: the figure keeps its shape on wide screens.
        let side = f.rect.width().min(f.rect.height());
        let sq = Rect::from_center_size(f.rect.center(), vec2(side * 1.2, side));
        let at = |p: Unit| pos2(sq.left() + p.0 * sq.width(), sq.top() + p.1 * sq.height());
        let width = f.pixel().max(0.6 * f.k().min(1.0));
        for n in 0..lines {
            let s = n as f32 / lines as f32 * std::f32::consts::TAU;
            let a = at(self.point(0, s, t));
            let b = at(self.point(1, s, t));
            // A slow shimmer along the figure.
            let v = 0.45 + 0.35 * (0.5 + 0.5 * (s * 3.0 + t * 0.7).sin());
            let color = hsv(self.hue, saturation, v).gamma_multiply(alpha);
            f.canvas.line_segment(a, b, width, color);
        }
    }
}

struct Lissart {
    rng: Rng,
    figure: Figure,
    /// The figure fading out, and how long it has been fading.
    old: Option<(Figure, f32)>,
    t: f32,
    age: f32,
    lines: usize,
    saturation: f32,
}

const LISSART_HOLD: f32 = 25.0;
const LISSART_FADE: f32 = 2.0;

impl Module for Lissart {
    fn draw(&mut self, f: &Frame) {
        self.t += f.dt;
        self.age += f.dt;
        if self.age > LISSART_HOLD {
            self.old = Some((std::mem::replace(&mut self.figure, Figure::random(&mut self.rng)), 0.0));
            self.age = 0.0;
        }
        f.canvas.rect_filled(f.rect, 0.0, Color32::BLACK);
        // Fewer lines on a small screen (the preview), or it fills in solid.
        let lines = ((self.lines as f32 * f.k().min(1.0)) as usize).max(90);
        if let Some((old, fade)) = &mut self.old {
            *fade += f.dt;
            let a = 1.0 - *fade / LISSART_FADE;
            if a <= 0.0 {
                self.old = None;
            } else {
                old.draw(f, self.t, lines, self.saturation, a);
            }
        }
        let a = (self.age / LISSART_FADE).min(1.0);
        self.figure.draw(f, self.t, lines, self.saturation, a);
    }
}

// ---- Spiral ---------------------------------------------------------------

pub const SPIRAL: Info = Info {
    name: "Spiral",
    about: "Spiral",
    options: &[Opt::Slider { key: "arms", label: "Arms", min: 1.0, max: 6.0, default: 2.0 }],
    uses_desktop: false,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let arms = get_f32(o, "arms", 2.0).round().max(1.0) as usize;
        Box::new(Spiral { shape: SpiralShape::random(&mut rng, arms), rng, arms, grown: 0.0, fade: None, turn: 0.0 })
    },
};

struct SpiralShape {
    /// Radius gained per turn, as a fraction of the half-diagonal.
    pitch: f32,
    hue: f32,
    spin: f32,
    wobble: f32,
}

impl SpiralShape {
    fn random(rng: &mut Rng, arms: usize) -> SpiralShape {
        SpiralShape { pitch: rng.range(0.025, 0.06) * arms as f32, hue: rng.f32(), spin: if rng.f32() < 0.5 { 1.0 } else { -1.0 }, wobble: rng.range(0.0, 0.25) }
    }
}

/// Coloured arms wind out from the centre while the whole turns, then fade
/// for the next.
struct Spiral {
    rng: Rng,
    shape: SpiralShape,
    arms: usize,
    /// Angle grown so far (radians).
    grown: f32,
    fade: Option<f32>,
    turn: f32,
}

impl Module for Spiral {
    fn draw(&mut self, f: &Frame) {
        use std::f32::consts::TAU;
        f.canvas.rect_filled(f.rect, 0.0, Color32::BLACK);
        let s = &self.shape;
        let reach = f.rect.size().length() / 2.0;
        let max_angle = TAU / s.pitch * 1.05;
        match &mut self.fade {
            Some(t) => {
                *t += f.dt;
                if *t > 1.5 {
                    self.shape = SpiralShape::random(&mut self.rng, self.arms);
                    self.grown = 0.0;
                    self.fade = None;
                    return;
                }
            }
            None => {
                self.grown += f.dt * 6.0;
                if self.grown >= max_angle {
                    self.grown = max_angle;
                    self.fade = Some(0.0);
                }
            }
        }
        self.turn += f.dt * 0.35 * s.spin;
        let alpha = self.fade.map_or(1.0, |t| (1.0 - t / 1.5).max(0.0));
        let c = f.rect.center();
        let width = (2.5 * f.k()).max(f.pixel());
        let steps = (self.grown / 0.04) as usize;
        for arm in 0..self.arms {
            let offset = TAU * arm as f32 / self.arms as f32;
            let mut prev: Option<Pos2> = None;
            for i in 0..=steps {
                let a = i as f32 * 0.04;
                let r = reach * s.pitch * a / TAU * (1.0 + s.wobble * (a * 0.5).sin());
                let angle = a + offset + self.turn;
                let p = c + vec2(angle.cos(), angle.sin()) * r;
                if let Some(q) = prev {
                    let color = hsv(s.hue + a / TAU * 0.08 + arm as f32 * 0.5 / self.arms as f32, 0.85, 1.0);
                    f.canvas.line_segment(q, p, width, color.gamma_multiply(alpha));
                }
                prev = Some(p);
            }
        }
    }
}

// ---- SuperString ----------------------------------------------------------

pub const SUPER_STRING: Info = Info {
    name: "SuperString",
    about: "SuperString, by Jon Watte",
    options: &[Opt::Check { key: "fade", label: "Fade Colors", default: false }],
    uses_desktop: false,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let mut speed = || rng.range(0.0022, 0.006) * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
        let v = [(speed(), speed()), (speed(), speed())];
        Box::new(SuperString { ends: [(0.3, 0.4), (0.7, 0.6)], v, lines: VecDeque::new(), color: hsv(rng.f32(), 0.9, 1.0), recolor: 0.0, rng, fade: get_bool(o, "fade", false), clock: Clock::new() })
    },
};

/// The line between two bouncing ends, leaving a trail that bends into
/// ribbons; the colour jumps every few seconds.
struct SuperString {
    ends: [Unit; 2],
    v: [Unit; 2],
    lines: VecDeque<(Unit, Unit, Color32)>,
    color: Color32,
    recolor: f32,
    rng: Rng,
    fade: bool,
    clock: Clock,
}

const STRING_LINES: usize = 220;

impl Module for SuperString {
    fn draw(&mut self, f: &Frame) {
        for _ in 0..self.clock.steps(f.dt) {
            for i in 0..2 {
                bounce(&mut self.ends[i], &mut self.v[i]);
            }
            self.recolor += STEP;
            if self.recolor > 2.5 {
                self.recolor = 0.0;
                self.color = hsv(self.rng.f32(), self.rng.range(0.7, 1.0), 1.0);
            }
            self.lines.push_back((self.ends[0], self.ends[1], self.color));
            if self.lines.len() > STRING_LINES {
                self.lines.pop_front();
            }
        }
        f.canvas.rect_filled(f.rect, 0.0, Color32::BLACK);
        let n = self.lines.len().max(1) as f32;
        let width = f.pixel().max(0.5 * f.k().min(1.0));
        for (i, (a, b, color)) in self.lines.iter().enumerate() {
            let color = if self.fade { color.gamma_multiply((i as f32 + 1.0) / n) } else { *color };
            f.canvas.line_segment(place(f, *a), place(f, *b), width, color);
        }
    }
}

// ---- StringTheme ----------------------------------------------------------

pub const STRING_THEME: Info = Info {
    name: "StringTheme",
    about: "StringTheme",
    options: &[Opt::Slider { key: "strings", label: "Strings", min: 1.0, max: 4.0, default: 2.0 }],
    uses_desktop: false,
    make: |o, seed| {
        let mut rng = Rng::new(seed);
        let n = get_f32(o, "strings", 2.0).round().clamp(1.0, 4.0) as usize;
        let strings = (0..n).map(|_| Strand::random(&mut rng)).collect();
        Box::new(StringTheme { strings, clock: Clock::new(), tick: 0 })
    },
};

/// A closed loop of points, each bouncing on its own.
struct Strand {
    points: Vec<(Unit, Unit)>,
    hue: f32,
    /// Earlier shapes, newest last.
    trail: VecDeque<Vec<Unit>>,
}

impl Strand {
    fn random(rng: &mut Rng) -> Strand {
        let points = (0..4)
            .map(|_| {
                let mut s = || rng.range(0.003, 0.009) * if rng.f32() < 0.5 { -1.0 } else { 1.0 };
                let v = (s(), s());
                ((rng.f32(), rng.f32()), v)
            })
            .collect();
        Strand { points, hue: rng.f32(), trail: VecDeque::new() }
    }
}

/// Mystify-like loops of smooth string, each echoed a few times as it moves.
struct StringTheme {
    strings: Vec<Strand>,
    clock: Clock,
    tick: u32,
}

const ECHOES: usize = 8;

/// A closed Catmull-Rom curve through `pts`.
fn closed_curve(pts: &[Pos2], per: usize) -> Vec<Pos2> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n * per + 1);
    for i in 0..n {
        let (p0, p1, p2, p3) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        for k in 0..per {
            let t = k as f32 / per as f32;
            let (t2, t3) = (t * t, t * t * t);
            let w = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push(pos2(w(p0.x, p1.x, p2.x, p3.x), w(p0.y, p1.y, p2.y, p3.y)));
        }
    }
    out.push(out[0]);
    out
}

impl Module for StringTheme {
    fn draw(&mut self, f: &Frame) {
        for _ in 0..self.clock.steps(f.dt) {
            self.tick += 1;
            for s in &mut self.strings {
                for (p, v) in &mut s.points {
                    bounce(p, v);
                }
                s.hue = (s.hue + STEP * 0.02) % 1.0;
                // An echo every few steps.
                if self.tick.is_multiple_of(5) {
                    s.trail.push_back(s.points.iter().map(|(p, _)| *p).collect());
                    if s.trail.len() > ECHOES {
                        s.trail.pop_front();
                    }
                }
            }
        }
        f.canvas.rect_filled(f.rect, 0.0, Color32::BLACK);
        let width = (1.2 * f.k()).max(f.pixel());
        for s in &self.strings {
            let n = s.trail.len().max(1) as f32;
            for (i, shape) in s.trail.iter().enumerate() {
                let pts: Vec<Pos2> = shape.iter().map(|p| place(f, *p)).collect();
                let color = hsv(s.hue, 0.8, 1.0).gamma_multiply((i as f32 + 1.0) / n);
                f.canvas.polyline(&closed_curve(&pts, 24), width, color);
            }
        }
    }
}
