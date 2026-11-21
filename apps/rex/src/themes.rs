//! Terminal themes for Olo Rex: Olo's native look palettes, popular classic
//! palettes, Ghostty & `.itermcolors` user imports, WCAG contrast legibility,
//! and Rex `com.superlogical.terminal.set_theme` serialization.

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

/// Built-in theme catalog entry `(id, theme)`.
pub fn builtin_themes() -> Vec<(String, TerminalTheme)> {
    let mut out: Vec<(String, TerminalTheme)> = Vec::new();
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        for t in looks::terminals(looks::DEFAULT, mode) {
            let id = t.id.clone();
            if !out.iter().any(|(existing, _)| existing == &id) {
                out.push((id, from_olo_terminal(&t)));
            }
        }
    }
    out.push(("night-owl".into(), night_owl()));
    out.push(("tokyo-night".into(), tokyo_night()));
    out.push(("catppuccin-mocha".into(), catppuccin_mocha()));
    out.push(("catppuccin-latte".into(), catppuccin_latte()));
    out.push(("dracula".into(), dracula()));
    out.push(("nord".into(), nord()));
    out.push(("solarized-dark".into(), solarized_dark()));
    out.push(("solarized-light".into(), solarized_light()));
    out
}

/// Resolve the active [`TerminalTheme`] for the current Olo desktop look and user settings.
pub fn resolve_theme(settings: &Settings, look: &Palette) -> TerminalTheme {
    let want_dark = match settings.scheme_choice {
        SchemeChoice::Auto => look.mode == ThemeMode::Dark,
        SchemeChoice::Light => false,
        SchemeChoice::Dark => true,
    };
    let chosen_id = if want_dark { settings.dark_theme_id.as_deref() } else { settings.light_theme_id.as_deref() };

    let mut theme = if let Some(id) = chosen_id {
        if let Some(custom) = settings.custom_themes.iter().find(|t| t.name == id) {
            custom.clone()
        } else if let Some((_, builtin)) = builtin_themes().into_iter().find(|(bid, _)| bid == id) {
            builtin
        } else {
            default_for_mode(look, want_dark)
        }
    } else {
        default_for_mode(look, want_dark)
    };

    // Ensure ANSI yellow/amber (slot 3) always reaches WCAG AA 4.5:1 contrast on light backgrounds.
    if theme.is_light() && theme.palette.len() > 3 {
        theme.palette[3] = legible(theme.palette[3], theme.background, 4.5);
    }
    if settings.extended_palette && !theme.extended_palette {
        theme = theme.with_generated_extended_palette(true);
    }
    if let Some(bg) = settings.selection_bg {
        theme.selection_bg = Some(bg);
    }
    if let Some(fg) = settings.selection_fg {
        theme.selection_fg = Some(fg);
    }
    theme
}

fn default_for_mode(look: &Palette, want_dark: bool) -> TerminalTheme {
    let look_is_dark = look.mode == ThemeMode::Dark;
    if want_dark == look_is_dark {
        from_olo_terminal(&look.terminal)
    } else {
        let fallback_id = if want_dark { "beos-dark" } else { "beos-light" };
        looks::terminal(fallback_id).map(|t| from_olo_terminal(&t)).unwrap_or_else(|| from_olo_terminal(&look.terminal))
    }
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

fn hex(v: u32) -> Rgb {
    Rgb::new(((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
}

fn make_theme(name: &str, fg: u32, bg: u32, cur: u32, sel: u32, ansi: [u32; 16]) -> TerminalTheme {
    let mut pal = default_palette();
    for (i, c) in ansi.iter().enumerate() {
        pal[i] = hex(*c);
    }
    TerminalTheme {
        name: name.to_owned(),
        foreground: hex(fg),
        background: hex(bg),
        cursor: Some(hex(cur)),
        cursor_text: None,
        selection_bg: Some(hex(sel)),
        selection_fg: None,
        palette: pal.to_vec(),
        extended_palette: false,
    }
}

fn night_owl() -> TerminalTheme {
    make_theme(
        "Night Owl",
        0xd6deeb,
        0x011627,
        0x80a4c2,
        0x1d3b53,
        [0x011627, 0xef5350, 0x22da6e, 0xaddb67, 0x82aaff, 0xc792ea, 0x21c7a8, 0xffffff, 0x575656, 0xef5350, 0x22da6e, 0xffeb95, 0x82aaff, 0xc792ea, 0x7fdbca, 0xffffff],
    )
}

fn tokyo_night() -> TerminalTheme {
    make_theme(
        "Tokyo Night",
        0xc0caf5,
        0x1a1b26,
        0xc0caf5,
        0x33467c,
        [0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5],
    )
}

fn catppuccin_mocha() -> TerminalTheme {
    make_theme(
        "Catppuccin Mocha",
        0xcdd6f4,
        0x1e1e2e,
        0xf5e0dc,
        0x585b70,
        [0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8],
    )
}

fn catppuccin_latte() -> TerminalTheme {
    make_theme(
        "Catppuccin Latte",
        0x4c4f69,
        0xeff1f5,
        0xdc8a78,
        0xacb0be,
        [0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc],
    )
}

fn dracula() -> TerminalTheme {
    make_theme(
        "Dracula",
        0xf8f8f2,
        0x282a36,
        0xf8f8f2,
        0x44475a,
        [0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff],
    )
}

fn nord() -> TerminalTheme {
    make_theme(
        "Nord",
        0xd8dee9,
        0x2e3440,
        0xd8dee9,
        0x4c566a,
        [0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4],
    )
}

fn solarized_dark() -> TerminalTheme {
    make_theme(
        "Solarized Dark",
        0x839496,
        0x002b36,
        0x93a1a1,
        0x073642,
        [0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3],
    )
}

fn solarized_light() -> TerminalTheme {
    make_theme(
        "Solarized Light",
        0x657b83,
        0xfdf6e3,
        0x586e75,
        0xeee8d5,
        [0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3],
    )
}
