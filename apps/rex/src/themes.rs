//! The terminals' colours: Olo's, as Terminal's Colors menu has them
//! (the look's own for light and for dark, followed as Preferences ▸
//! Themes changes; or any theme's, chosen in the Settings menu, which
//! stays), held to a legible contrast, and said to the Rex server
//! (`com.superlogical.terminal.set_theme`) so programs in a pane can
//! ask what they are. (Rex came with eight more palettes of its own and
//! an importer of others' theme files: the desktop's themes are where a
//! palette is added.)

use olo_kit::{Hsla, style};
use olo_rex_vt::{Rgb, TerminalTheme, default_palette};
use olo_theme::{Palette, ThemeMode, looks};

use crate::settings::{SchemeChoice, Settings};

pub fn hsla(c: Rgb) -> Hsla {
    style::c([c.r, c.g, c.b, 255])
}

fn from_olo_rgba([r, g, b, _]: olo_theme::Rgba) -> Rgb {
    Rgb::new(r, g, b)
}

/// Convert an Olo look's terminal palette into a [`TerminalTheme`].
pub fn from_olo_terminal(t: &olo_theme::Terminal) -> TerminalTheme {
    let mut pal = default_palette();
    for (i, c) in t.ansi.iter().enumerate() {
        pal[i] = from_olo_rgba(*c);
    }
    TerminalTheme {
        name: t.name.clone(),
        foreground: from_olo_rgba(t.foreground),
        background: from_olo_rgba(t.background),
        cursor: Some(from_olo_rgba(t.cursor)),
        cursor_text: None,
        selection_bg: Some(from_olo_rgba(t.selection)),
        selection_fg: None,
        palette: pal.to_vec(),
        extended_palette: false,
    }
}

/// The colours to draw with now, for Olo's look and the person's choices.
pub fn resolve_theme(settings: &Settings, look: &Palette) -> TerminalTheme {
    let mode = match settings.scheme_choice {
        SchemeChoice::Auto => look.mode,
        SchemeChoice::Light => ThemeMode::Light,
        SchemeChoice::Dark => ThemeMode::Dark,
    };
    let chosen = if mode == ThemeMode::Dark { settings.dark_theme_id.as_deref() } else { settings.light_theme_id.as_deref() };
    // (One chosen that is no longer among Olo's: the look's own.)
    let mut theme = chosen.and_then(looks::terminal).map_or_else(|| from_olo_terminal(look.terminal_in(mode)), |t| from_olo_terminal(&t));
    // ANSI yellow (slot 3) is held to 4.5:1 on a light ground.
    if theme.is_light() && theme.palette.len() > 3 {
        theme.palette[3] = legible(theme.palette[3], theme.background, 4.5);
    }
    theme
}

/// Build the arguments for `com.superlogical.terminal.set_theme`.
pub fn to_rex_theme_args(theme: &TerminalTheme) -> (String, String, Option<String>, Vec<String>, &'static str) {
    let pal = theme.palette_256();
    let count = if theme.extended_palette { 256 } else { 16 };
    let palette: Vec<String> = (0..count).map(|i| format!("{i}={}", pal[i].to_hex())).collect();
    let scheme = if theme.is_light() { "light" } else { "dark" };
    (theme.foreground.to_hex(), theme.background.to_hex(), theme.cursor.map(Rgb::to_hex), palette, scheme)
}

/// WCAG contrast ratio helper (fast table-backed luminance).
pub fn legible(fg: Rgb, bg: Rgb, min_contrast: f32) -> Rgb {
    if min_contrast <= 1.01 || fg == bg {
        return fg;
    }
    let lb = luminance(bg);
    if ratio(luminance(fg), lb) >= min_contrast {
        return fg;
    }
    let to = if ratio(0.0, lb) >= ratio(1.0, lb) { Rgb::new(0, 0, 0) } else { Rgb::new(255, 255, 255) };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..10 {
        let mid = (lo + hi) * 0.5;
        if ratio(luminance(mix(to, fg, mid)), lb) >= min_contrast {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(to, fg, hi)
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round().clamp(0.0, 255.0) as u8;
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

pub fn is_graphic_char(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0xE0A0..=0xE0D4 | 0x1FB00..=0x1FBFF)
}

fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The look's own colours by default, for the mode chosen; one of
    /// Olo's by its id where chosen; the look's own again for an id
    /// that is no theme's (a palette Rex once had of its own).
    #[test]
    fn colours_are_olos() {
        let look = Palette::light();
        let mut s = Settings::default();
        assert_eq!(resolve_theme(&s, &look).background, from_olo_terminal(look.terminal_in(ThemeMode::Light)).background);
        s.scheme_choice = SchemeChoice::Dark;
        assert_eq!(resolve_theme(&s, &look).background, from_olo_terminal(look.terminal_in(ThemeMode::Dark)).background);
        assert!(!resolve_theme(&s, &look).is_light());
        let other = looks::terminals(looks::DEFAULT, ThemeMode::Dark).into_iter().last().expect("Olo has themes");
        s.dark_theme_id = Some(other.id.clone());
        assert_eq!(resolve_theme(&s, &look).background, from_olo_terminal(&other).background);
        s.dark_theme_id = Some("tokyo-night".into());
        assert_eq!(resolve_theme(&s, &look).background, from_olo_terminal(look.terminal_in(ThemeMode::Dark)).background);
    }
}
