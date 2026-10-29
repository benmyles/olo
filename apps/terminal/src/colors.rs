//! Terminal colours, as alacritty's types: the look's own
//! (`Palette::terminal`), or one chosen in View ▸ Colors from those
//! `olo_theme::looks::terminals` offers (the theme's own, BeOS's and
//! Catppuccin's four flavours; see `Settings::light_colors`).

use alacritty_terminal::{
    term::color::Colors,
    vte::ansi::{Color, NamedColor, Rgb},
};
use olo_kit::egui::Color32;

#[derive(Debug, Clone, PartialEq)]
pub struct Scheme {
    pub name: String,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// Black, red, green, yellow, blue, magenta, cyan, white; then bright.
    pub ansi: [Rgb; 16],
}

fn rgb([r, g, b, _]: olo_kit::theme::Rgba) -> Rgb {
    Rgb { r, g, b }
}

impl From<&olo_kit::theme::Terminal> for Scheme {
    fn from(t: &olo_kit::theme::Terminal) -> Scheme {
        Scheme {
            name: t.name.clone(),
            foreground: rgb(t.foreground),
            background: rgb(t.background),
            cursor: rgb(t.cursor),
            selection: rgb(t.selection),
            ansi: t.ansi.map(rgb),
        }
    }
}

impl Scheme {
    /// BeOS's light terminal: what a shell starts with before a window
    /// has said otherwise.
    #[allow(dead_code, reason = "olo-termd's (olo-terminal starts from the look)")]
    pub fn light() -> Scheme {
        Scheme::from(&olo_kit::theme::Palette::light().terminal)
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

    #[test]
    fn from_the_look() {
        let t = olo_kit::theme::looks::terminal("mocha").unwrap();
        let s = Scheme::from(&t);
        assert_eq!((s.name.as_str(), s.background, s.ansi[1]), ("Catppuccin Mocha", Rgb { r: 0x1e, g: 0x1e, b: 0x2e }, Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }));
        assert_eq!(Scheme::light().background, Rgb { r: 0xfb, g: 0xfa, b: 0xf6 });
    }

    #[test]
    fn cube_and_grays() {
        let s = Scheme::light();
        assert_eq!(s.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(s.indexed(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(s.indexed(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(s.indexed(255), Rgb { r: 238, g: 238, b: 238 });
    }
}
