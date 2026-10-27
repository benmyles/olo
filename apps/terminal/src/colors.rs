//! Terminal colour schemes. The defaults are Olo's own: a warm paper
//! white with ink-dark text in light mode and a soft graphite in dark
//! mode, with ANSI colours tuned so every one of the 16 reads clearly on
//! the background (no neon green on black).

use alacritty_terminal::{
    term::color::Colors,
    vte::ansi::{Color, NamedColor, Rgb},
};
use olo_kit::egui::Color32;

#[derive(Debug, Clone, PartialEq)]
pub struct Scheme {
    pub name: &'static str,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// Black, red, green, yellow, blue, magenta, cyan, white; then bright.
    pub ansi: [Rgb; 16],
}

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

impl Scheme {
    pub fn light() -> Scheme {
        Scheme {
            name: "Olo Light",
            foreground: rgb(0x26292e),
            background: rgb(0xfbfaf6),
            cursor: rgb(0x2f6fdb),
            selection: rgb(0xc9dcf7),
            ansi: [
                rgb(0x3b3f45), rgb(0xc4314b), rgb(0x2f8a3e), rgb(0x9a6a00),
                rgb(0x2f6fdb), rgb(0x9d3fb5), rgb(0x0f8494), rgb(0x9a9ca2),
                rgb(0x6a6e75), rgb(0xe0475f), rgb(0x3aa34d), rgb(0xb98400),
                rgb(0x4b8af0), rgb(0xb85bd0), rgb(0x13939f), rgb(0xd4d5d8),
            ],
        }
    }

    pub fn dark() -> Scheme {
        Scheme {
            name: "Olo Dark",
            foreground: rgb(0xd9dce1),
            background: rgb(0x1d2025),
            cursor: rgb(0x7cb4ff),
            selection: rgb(0x2f405a),
            ansi: [
                rgb(0x2c3037), rgb(0xec6a78), rgb(0x8fcf7d), rgb(0xe8c17a),
                rgb(0x6aaef5), rgb(0xc98ae6), rgb(0x5cc4cf), rgb(0xb4b8bf),
                rgb(0x5f6570), rgb(0xff8592), rgb(0xa9e395), rgb(0xf7d595),
                rgb(0x8cc2ff), rgb(0xdca5f2), rgb(0x7ad6df), rgb(0xeceef1),
            ],
        }
    }

    pub fn for_mode(dark: bool) -> Scheme {
        if dark { Scheme::dark() } else { Scheme::light() }
    }

    /// The colour `index` of the 256-colour palette.
    fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..16 => self.ansi[i as usize],
            16..232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }

    /// Resolve a cell colour, honouring colours the program changed with
    /// OSC 4/10/11.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(c) => c,
            Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => overrides[n].unwrap_or_else(|| self.named(n)),
        }
    }

    fn named(&self, n: NamedColor) -> Rgb {
        use NamedColor::*;
        match n {
            Foreground | BrightForeground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            DimForeground => mix(self.foreground, self.background, 0.66),
            DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
                let base = n as usize - DimBlack as usize;
                mix(self.ansi[base], self.background, 0.66)
            }
            _ => self.ansi[(n as usize).min(15)],
        }
    }
}

/// `a` weighted `t` against `b`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

pub fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn lum(c: Rgb) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn readable() {
        for s in [Scheme::light(), Scheme::dark()] {
            assert!(contrast(s.foreground, s.background) > 10.0, "{} text", s.name);
            // Colours used as text: everything but the one that matches
            // the background (white in light mode, black in dark mode).
            let skip = if s.name.contains("Light") { [7, 15] } else { [0, 8] };
            for (i, c) in s.ansi.iter().enumerate() {
                if skip.contains(&i) {
                    continue;
                }
                assert!(contrast(*c, s.background) >= 3.0, "{} colour {i}: {:.2}", s.name, contrast(*c, s.background));
            }
            assert!(contrast(s.foreground, s.selection) > 7.0, "{} selection", s.name);
        }
    }

    #[test]
    fn cube_and_grays() {
        let s = Scheme::dark();
        assert_eq!(s.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(s.indexed(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(s.indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(s.indexed(255), Rgb { r: 238, g: 238, b: 238 });
    }
}
