//! A theme's colours over the classic set's drawings.
//!
//! The classic set is drawn once (`icons/svg`); a theme that has no
//! drawing of its own for an icon shows the classic one in its own
//! colours, by rules it keeps beside its icons
//! (`themes/ID/icons/recolour.txt`). Ben, 2026-11-17: every icon is to
//! be "consistently angled, sized, styled etc. and fit the respective
//! theme": one drawing, so the angle, the size and the detail are the
//! same in every theme; the colours, the theme's.
//!
//! The rules, one to a line (`#` starts a comment):
//!
//! ```text
//! ink 0e0e1a                     the classic ink (1b1b1f: outlines, darkest fills) becomes this
//! grey 240 0.2                   greys take this hue and saturation (a tint)
//! turn 25-55 to 233 0.6 +0.1 in folder folder-open
//!                                colours with a hue of 25 to 55 degrees take hue 233,
//!                                0.6 of their saturation, 0.1 more lightness, in those icons
//! turn 195-240 by 14 0.85 +0.02  ... turned by 14 degrees, in every icon
//! keep olo slack                 these drawings stay as they are (brands' marks)
//! ```
//!
//! A colour is a grey when its channels differ by less than [`GREY`];
//! any other takes the first `turn` whose hues hold it (and whose icons,
//! if it names any, include this one), so the particular come before the
//! general. Hues are HSL's, in degrees; saturation and lightness, 0 to 1.

use std::hash::{Hash as _, Hasher as _};

/// The classic set's ink: its outlines and its darkest fills.
const INK: [u8; 3] = [0x1b, 0x1b, 0x1f];

/// A colour whose channels differ by less than this (of 1) is a grey.
pub const GREY: f32 = 0.12;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hue {
    By(f32),
    To(f32),
}

#[derive(Debug, Clone, PartialEq)]
struct Turn {
    from: (f32, f32),
    hue: Hue,
    saturation: f32,
    lightness: f32,
    only: Vec<String>,
}

/// A theme's rules for the classic set's drawings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recolour {
    ink: Option<[u8; 3]>,
    grey: Option<(f32, f32)>,
    turns: Vec<Turn>,
    keep: Vec<String>,
    key: u64,
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]),
        3 => Some([((v >> 8) & 15) as u8 * 17, ((v >> 4) & 15) as u8 * 17, (v & 15) as u8 * 17]),
        _ => None,
    }
}

/// `rgb` as hue (degrees), saturation and lightness (HSL).
pub fn hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let c = max - min;
    if c <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = c / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == r {
        ((g - b) / c).rem_euclid(6.0)
    } else if max == g {
        (b - r) / c + 2.0
    } else {
        (r - g) / c + 4.0
    };
    (h * 60.0, s.min(1.0), l)
}

/// The colour of hue `h` (degrees), saturation `s` and lightness `l`.
pub fn rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r, g, b].map(|v| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8)
}

impl Recolour {
    /// The rules in `text`; what's wrong with it, and on which line.
    pub fn parse(text: &str) -> Result<Recolour, String> {
        let mut rules = Recolour::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let words: Vec<&str> = line.split_whitespace().collect();
            let bad = |what: &str| format!("line {}: {what}: {line:?}", n + 1);
            let number = |w: &str| w.parse::<f32>().map_err(|_| bad("not a number"));
            match words[..] {
                [] => {}
                ["ink", colour] => rules.ink = Some(hex(colour).ok_or_else(|| bad("not a colour"))?),
                ["grey", hue, saturation] => rules.grey = Some((number(hue)?, number(saturation)?)),
                ["turn", from, how @ ("by" | "to"), hue, saturation, lightness, ref rest @ ..] => {
                    let (lo, hi) = from.split_once('-').ok_or_else(|| bad("hues are LO-HI"))?;
                    let hue = if how == "by" { Hue::By(number(hue)?) } else { Hue::To(number(hue)?) };
                    let only = match rest {
                        [] => Vec::new(),
                        ["in", names @ ..] if !names.is_empty() => names.iter().map(|&s| s.to_owned()).collect(),
                        _ => return Err(bad("after the lightness, `in NAME…` or nothing")),
                    };
                    rules.turns.push(Turn { from: (number(lo)?, number(hi)?), hue, saturation: number(saturation)?, lightness: number(lightness)?, only });
                }
                ["keep", ref names @ ..] if !names.is_empty() => rules.keep.extend(names.iter().map(|&s| s.to_owned())),
                _ => return Err(bad("not a rule")),
            }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        rules.key = h.finish();
        Ok(rules)
    }

    /// What tells these rules from any others: part of the name a
    /// rendering made with them is kept under.
    pub fn key(&self) -> u64 {
        self.key
    }

    /// The classic colour `rgb`, as these rules have it in icon `name`.
    pub fn colour(&self, name: &str, rgb_in: [u8; 3]) -> [u8; 3] {
        if rgb_in == INK
            && let Some(ink) = self.ink
        {
            return ink;
        }
        let (h, s, l) = hsl(rgb_in);
        let spread = f32::from(rgb_in.iter().copied().max().unwrap_or(0) - rgb_in.iter().copied().min().unwrap_or(0)) / 255.0;
        if spread < GREY {
            return self.grey.map_or(rgb_in, |(hue, saturation)| rgb(hue, saturation, l));
        }
        for t in &self.turns {
            let here = t.only.is_empty() || t.only.iter().any(|n| n == name);
            let (lo, hi) = t.from;
            let held = if lo <= hi { h >= lo && h <= hi } else { h >= lo || h <= hi };
            if here && held {
                let hue = match t.hue {
                    Hue::By(d) => h + d,
                    Hue::To(to) => to,
                };
                return rgb(hue, (s * t.saturation).clamp(0.0, 1.0), (l + t.lightness).clamp(0.0, 1.0));
            }
        }
        rgb_in
    }

    /// Drawing `svg` (icon `name`) with every colour in it as these rules
    /// have it. References (`url(#top)`, `href="#mark"`) aren't colours.
    pub fn apply(&self, name: &str, svg: &str) -> String {
        use std::fmt::Write as _;
        if self.keep.iter().any(|k| k == name) {
            return svg.to_owned();
        }
        let bytes = svg.as_bytes();
        let mut out = String::with_capacity(svg.len() + 64);
        let (mut from, mut copied) = (0, 0);
        while let Some(at) = svg[from..].find('#').map(|k| k + from) {
            from = at + 1;
            let digits = bytes[from..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            let goes_on = bytes.get(from + digits).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'));
            let reference = svg[..at].ends_with("url(") || svg[..at].ends_with("href=\"");
            if !(digits == 6 || digits == 3) || goes_on || reference {
                continue;
            }
            let Some(colour) = hex(&svg[from..from + digits]) else { continue };
            let [r, g, b] = self.colour(name, colour);
            out.push_str(&svg[copied..at]);
            let _ = write!(out, "#{r:02x}{g:02x}{b:02x}");
            copied = from + digits;
            from = copied;
        }
        out.push_str(&svg[copied..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "# A test theme.\nink 0e0e1a\ngrey 240 0.25  # lilac\nturn 25-55 to 233 0.6 +0.1 in folder\nturn 195-240 by 15 1 0\nkeep olo\n";

    #[test]
    fn colours_there_and_back() {
        for c in [[0x23, 0x80, 0xf4], [0xfc, 0xc5, 0x22], [0xf0, 0x3a, 0x30], [0x3f, 0xc6, 0x40], [0x80, 0x80, 0x80], [0, 0, 0], [255, 255, 255]] {
            let (h, s, l) = hsl(c);
            let back = rgb(h, s, l);
            assert!(c.iter().zip(back).all(|(a, b)| a.abs_diff(b) <= 1), "{c:?} came back {back:?}");
        }
    }

    #[test]
    fn rules_are_read() {
        let r = Recolour::parse(RULES).unwrap();
        assert_eq!(r.ink, Some([0x0e, 0x0e, 0x1a]));
        assert_eq!(r.grey, Some((240.0, 0.25)));
        assert_eq!(r.turns.len(), 2);
        assert_eq!(r.turns[0].only, ["folder"]);
        assert_eq!(r.keep, ["olo"]);
        assert_ne!(r.key(), Recolour::parse("ink 000000\n").unwrap().key());
        // What's wrong is said, with its line.
        for (bad, line) in [("ink\n", 1), ("grey 240 0.2\nturn 10 by 5 1 0\n", 2), ("paint it black\n", 1), ("turn 1-2 by 5 1 0 folder\n", 1), ("ink zzzzzz\n", 1)] {
            let e = Recolour::parse(bad).unwrap_err();
            assert!(e.starts_with(&format!("line {line}:")), "{bad:?}: {e}");
        }
    }

    #[test]
    fn a_drawing_takes_the_themes_colours() {
        let r = Recolour::parse(RULES).unwrap();
        let svg = r##"<path class="o" fill="url(#face)" stroke="#1b1b1f"/><use href="#bed"/><stop stop-color="#808080"/><path fill="#fff"/><path fill="#f8ca66"/><path fill="#2380f4"/>"##;
        // The colours in a drawing, in order (what follows `="#`).
        let colours = |text: &str| -> Vec<[u8; 3]> { text.split("=\"#").skip(1).filter_map(|rest| hex(rest.split('"').next()?)).collect() };
        let folder = r.apply("folder", svg);
        // References are left alone (`#face` and `#bed` are hex digits too).
        assert!(folder.contains("url(#face)") && folder.contains(r##"href="#bed""##), "{folder}");
        let [ink, _bed, grey, white, manila, blue] = colours(&folder)[..] else { panic!("{folder}") };
        assert_eq!(ink, [0x0e, 0x0e, 0x1a], "the ink");
        // A grey is tinted (bluer than it is red); white stays white.
        assert!(grey[2] > grey[0] + 20 && grey[0] == grey[1], "{grey:?}");
        assert_eq!(white, [255, 255, 255]);
        // The folder's manila turns blue in a folder, and stays in anything else.
        assert!((hsl(manila).0 - 233.0).abs() < 2.0, "the manila's hue is {}", hsl(manila).0);
        assert!(r.apply("key", svg).contains("#f8ca66"));
        // Blue is turned everywhere.
        assert!((hsl(blue).0 - (hsl([0x23, 0x80, 0xf4]).0 + 15.0)).abs() < 2.0, "{}", hsl(blue).0);
        // A drawing kept is itself.
        assert_eq!(r.apply("olo", svg), svg);
    }
}
