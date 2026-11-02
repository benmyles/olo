//! Points, sizes, rects and colours, as the modules use them (named as
//! egui named them, which the modules were first written for), in no
//! toolkit's types. Points are logical (a screen's points).

use std::ops::{Add, AddAssign, Div, Mul, Sub};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

pub const fn pos2(x: f32, y: f32) -> Pos2 {
    Pos2 { x, y }
}

pub const fn vec2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl Add<Vec2> for Pos2 {
    type Output = Pos2;
    fn add(self, v: Vec2) -> Pos2 {
        pos2(self.x + v.x, self.y + v.y)
    }
}

impl AddAssign<Vec2> for Pos2 {
    fn add_assign(&mut self, v: Vec2) {
        self.x += v.x;
        self.y += v.y;
    }
}

impl Sub<Vec2> for Pos2 {
    type Output = Pos2;
    fn sub(self, v: Vec2) -> Pos2 {
        pos2(self.x - v.x, self.y - v.y)
    }
}

impl Sub for Pos2 {
    type Output = Vec2;
    fn sub(self, p: Pos2) -> Vec2 {
        vec2(self.x - p.x, self.y - p.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, v: Vec2) -> Vec2 {
        vec2(self.x + v.x, self.y + v.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, v: Vec2) -> Vec2 {
        vec2(self.x - v.x, self.y - v.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        vec2(self.x * k, self.y * k)
    }
}

impl Mul<Vec2> for f32 {
    type Output = Vec2;
    fn mul(self, v: Vec2) -> Vec2 {
        v * self
    }
}

impl Div<f32> for Vec2 {
    type Output = Vec2;
    fn div(self, k: f32) -> Vec2 {
        vec2(self.x / k, self.y / k)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    pub fn from_min_max(min: Pos2, max: Pos2) -> Rect {
        Rect { min, max }
    }

    pub fn from_min_size(min: Pos2, size: Vec2) -> Rect {
        Rect { min, max: min + size }
    }

    pub fn from_center_size(center: Pos2, size: Vec2) -> Rect {
        Rect { min: center - size / 2.0, max: center + size / 2.0 }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn size(&self) -> Vec2 {
        vec2(self.width(), self.height())
    }

    pub fn center(&self) -> Pos2 {
        pos2((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }

    pub fn left(&self) -> f32 {
        self.min.x
    }

    pub fn right(&self) -> f32 {
        self.max.x
    }

    pub fn top(&self) -> f32 {
        self.min.y
    }

    pub fn bottom(&self) -> f32 {
        self.max.y
    }
}

/// A colour, its channels not multiplied by its alpha.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Color32(pub [u8; 4]);

impl Color32 {
    pub const BLACK: Color32 = Color32([0, 0, 0, 255]);
    pub const WHITE: Color32 = Color32([255, 255, 255, 255]);

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Color32 {
        Color32([r, g, b, 255])
    }

    pub const fn from_gray(v: u8) -> Color32 {
        Color32([v, v, v, 255])
    }

    pub const fn from_black_alpha(a: u8) -> Color32 {
        Color32([0, 0, 0, a])
    }

    pub const fn from_white_alpha(a: u8) -> Color32 {
        Color32([255, 255, 255, a])
    }

    pub fn r(&self) -> u8 {
        self.0[0]
    }

    pub fn g(&self) -> u8 {
        self.0[1]
    }

    pub fn b(&self) -> u8 {
        self.0[2]
    }

    pub fn a(&self) -> u8 {
        self.0[3]
    }

    /// Faded towards transparent by `k` (0–1).
    pub fn gamma_multiply(self, k: f32) -> Color32 {
        let [r, g, b, a] = self.0;
        Color32([r, g, b, (a as f32 * k.clamp(0.0, 1.0)).round() as u8])
    }

    /// The channels 0–1.
    pub fn to_f32(self) -> [f32; 4] {
        self.0.map(|c| c as f32 / 255.0)
    }
}

/// A colour from hue, saturation and value as people see them (applied
/// to the sRGB channels, as R5's modules did).
pub fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |u: f32| ((u + m) * 255.0).round() as u8;
    Color32::from_rgb(to(r), to(g), to(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0), Color32::from_rgb(0, 255, 0));
        assert_eq!(hsv(2.0 / 3.0, 1.0, 1.0), Color32::from_rgb(0, 0, 255));
        assert_eq!(hsv(0.5, 0.0, 0.5), Color32::from_gray(128));
    }

    #[test]
    fn fading_keeps_the_colour() {
        assert_eq!(Color32::from_rgb(10, 20, 30).gamma_multiply(0.5), Color32([10, 20, 30, 128]));
    }
}
