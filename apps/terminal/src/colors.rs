//! Terminal colours, as alacritty's types: the look's own
//! (`Palette::terminal`), or one chosen in View ▸ Colors from those
//! `olo_theme::looks::terminals` offers (the theme's own, BeOS's and
//! Catppuccin's four flavours; see `Settings::light_colors`).

use alacritty_terminal::{
    term::color::Colors,
    vte::ansi::{Color, NamedColor, Rgb},
};

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

fn rgb([r, g, b, _]: olo_theme::Rgba) -> Rgb {
    Rgb { r, g, b }
}

impl From<&olo_theme::Terminal> for Scheme {
    fn from(t: &olo_theme::Terminal) -> Scheme {
        Scheme { name: t.name.clone(), foreground: rgb(t.foreground), background: rgb(t.background), cursor: rgb(t.cursor), selection: rgb(t.selection), ansi: t.ansi.map(rgb) }
    }
}

impl Scheme {
    /// BeOS's light terminal: what a shell starts with before a window
    /// has said otherwise.
    #[allow(dead_code, reason = "olo-termd's (olo-terminal starts from the look)")]
    pub fn light() -> Scheme {
        Scheme::from(&olo_theme::Palette::light().terminal)
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

/// The least contrast text keeps against its cell (WCAG's ratio, 1 to
/// 21: 4.5 is what body text needs, AA; a terminal's default text is
/// 17 or so, so dim text stays dim).
pub const MIN_CONTRAST: f32 = 4.5;

/// `fg` as text on `bg`, at least `min` apart: a colour a program chose
/// for another background (a dark terminal's pale greys on a light one,
/// a dark band under the default text) moved toward black or white, its
/// hue kept, just as far as it takes, as iTerm's minimum contrast does.
/// Text the colour of its cell is left hidden (a program hides it so).
pub fn legible(fg: Rgb, bg: Rgb, min: f32) -> Rgb {
    if fg == bg {
        return fg;
    }
    let lb = luminance(bg);
    if ratio(luminance(fg), lb) >= min {
        return fg;
    }
    // Toward the end with more room from the cell.
    let to = if ratio(0.0, lb) >= ratio(1.0, lb) { Rgb { r: 0, g: 0, b: 0 } } else { Rgb { r: 255, g: 255, b: 255 } };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..10 {
        let mid = (lo + hi) / 2.0;
        if ratio(luminance(mix(to, fg, mid)), lb) >= min {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(to, fg, hi)
}

/// WCAG's contrast between two relative luminances.
fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// WCAG's relative luminance (sRGB made linear by a table).
fn luminance(c: Rgb) -> f32 {
    static LINEAR: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
        std::array::from_fn(|i| {
            let v = i as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        })
    });
    let l = &*LINEAR;
    0.2126 * l[c.r as usize] + 0.7152 * l[c.g as usize] + 0.0722 * l[c.b as usize]
}

/// A character that's a picture, not text (box drawing, blocks and
/// shades, Powerline's arrows): its colours are kept as they are.
pub fn graphic(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0xE0A0..=0xE0D4 | 0x1FB00..=0x1FBFF)
}

/// `a` weighted `t` against `b`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_the_look() {
        let t = olo_theme::looks::terminal("mocha").unwrap();
        let s = Scheme::from(&t);
        assert_eq!((s.name.as_str(), s.background, s.ansi[1]), ("Catppuccin Mocha", Rgb { r: 0x1e, g: 0x1e, b: 0x2e }, Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }));
        assert_eq!(Scheme::light().background, Rgb { r: 0xfb, g: 0xfa, b: 0xf6 });
    }

    #[test]
    fn text_kept_legible() {
        let white = Rgb { r: 0xfb, g: 0xfa, b: 0xf6 };
        let black = Rgb { r: 0x1e, g: 0x1e, b: 0x2e };
        // A dark terminal's pale grey on a light cell: darkened to 3:1.
        let pale = Rgb { r: 0xc0, g: 0xc0, b: 0xc8 };
        let fixed = legible(pale, white, MIN_CONTRAST);
        assert!(ratio(luminance(fixed), luminance(white)) >= MIN_CONTRAST && fixed.r < pale.r, "{fixed:?}");
        // Its hue kept (bluish still), and no darker than it takes.
        assert!(fixed.b > fixed.r && ratio(luminance(fixed), luminance(white)) < MIN_CONTRAST + 0.2, "{fixed:?}");
        // Default text on a near-black band a program chose: lightened.
        let band = Rgb { r: 0x10, g: 0x24, b: 0x18 };
        let text = legible(black, band, MIN_CONTRAST);
        assert!(ratio(luminance(text), luminance(band)) >= MIN_CONTRAST && text.g > black.g, "{text:?}");
        // Legible already, or hidden on purpose: as it was.
        assert_eq!(legible(black, white, MIN_CONTRAST), black);
        assert_eq!(legible(white, white, MIN_CONTRAST), white);
        assert!(graphic('█') && graphic('─') && graphic('\u{e0b0}') && !graphic('a'));
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
