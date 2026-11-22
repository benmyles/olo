//! Color utilities, Ghostty theme parser, and `.itermcolors` XML plist importer
//! backed by `libghostty-vt`'s color parsing and CIELAB palette generator.

use serde::{Deserialize, Serialize};

use crate::ffi;

/// Even verbose XML palettes are small. Limit scanning work on imported files.
pub const MAX_THEME_BYTES: usize = 1 << 20;

/// 8-bit sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn from_ffi(c: ffi::GhosttyColorRgb) -> Self {
        Self { r: c.r, g: c.g, b: c.b }
    }

    pub fn to_ffi(self) -> ffi::GhosttyColorRgb {
        ffi::GhosttyColorRgb { r: self.r, g: self.g, b: self.b }
    }

    /// W3C relative luminance (0.0..=1.0).
    pub fn luminance(self) -> f64 {
        let _ = crate::sys::init();
        let c = self.to_ffi();
        // SAFETY: The local RGB value is initialized and borrowed only for this calculation.
        unsafe { ffi::ghostty_color_luminance(&c) }
    }

    /// Perceived luminance (0.0..=1.0); > 0.5 is treated as light by Ghostty.
    pub fn perceived_luminance(self) -> f64 {
        let _ = crate::sys::init();
        let c = self.to_ffi();
        // SAFETY: The local RGB value is initialized and borrowed only for this calculation.
        unsafe { ffi::ghostty_color_perceived_luminance(&c) }
    }

    /// WCAG contrast ratio (1.0..=21.0).
    pub fn contrast(self, other: Self) -> f64 {
        let _ = crate::sys::init();
        let a = self.to_ffi();
        let b = other.to_ffi();
        // SAFETY: Both RGB values are live, initialized inputs for this call.
        unsafe { ffi::ghostty_color_contrast(&a, &b) }
    }
}

/// Complete terminal color theme (foreground, background, cursor, selection, and
/// 256-color palette).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTheme {
    pub name: String,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    pub cursor_text: Option<Rgb>,
    pub selection_bg: Option<Rgb>,
    pub selection_fg: Option<Rgb>,
    /// Full 256-color palette (indices 0..16 are ANSI 16, 16..256 extended).
    pub palette: Vec<Rgb>,
    /// True when the extended 16..255 palette was explicitly supplied or
    /// generated from the theme colors.
    pub extended_palette: bool,
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            name: "Default".to_owned(),
            foreground: Rgb::new(0xd6, 0xde, 0xeb),
            background: Rgb::new(0x01, 0x16, 0x27),
            cursor: Some(Rgb::new(0x80, 0xa4, 0xc2)),
            cursor_text: None,
            selection_bg: None,
            selection_fg: None,
            palette: default_palette().to_vec(),
            extended_palette: false,
        }
    }
}

impl TerminalTheme {
    /// Ensure palette has 256 entries, filling missing entries with Ghostty's
    /// default 256-color palette.
    pub fn palette_256(&self) -> [Rgb; 256] {
        let mut out = default_palette();
        for (i, c) in self.palette.iter().take(256).enumerate() {
            out[i] = *c;
        }
        out
    }

    /// Generate a harmonious or dark-to-light 256-color extended palette from
    /// indices 0..16, background, and foreground via Ghostty's CIELAB generator.
    pub fn with_generated_extended_palette(mut self, harmonious: bool) -> Self {
        let base = self.palette_256();
        let generated = generate_extended_palette(&base, None, self.background, self.foreground, harmonious);
        self.palette = generated.to_vec();
        self.extended_palette = true;
        self
    }

    /// True if the theme background is light (`perceived_luminance > 0.5`).
    pub fn is_light(&self) -> bool {
        self.background.perceived_luminance() > 0.5
    }
}

/// Parse any Ghostty color syntax (`#rrggbb`, `rrggbb`, `#rgb`, `rgb:r/g/b`, X11 name).
pub fn parse_color(value: &str) -> Option<Rgb> {
    let _ = crate::sys::init();
    let mut out = ffi::GhosttyColorRgb::default();
    // SAFETY: Ghostty takes a counted UTF-8 slice and writes the typed local outputs synchronously.
    let res = unsafe { ffi::ghostty_color_parse(value.as_ptr().cast(), value.len(), &mut out) };
    (res == ffi::GHOSTTY_SUCCESS).then(|| Rgb::from_ffi(out))
}

/// Parse a Ghostty palette entry (`N=COLOR`, e.g. `1=#ef5350`).
pub fn parse_palette_entry(value: &str) -> Option<(u8, Rgb)> {
    let _ = crate::sys::init();
    let mut idx = 0u8;
    let mut out = ffi::GhosttyColorRgb::default();
    // SAFETY: Ghostty takes a counted UTF-8 slice and writes the typed local outputs synchronously.
    let res = unsafe { ffi::ghostty_color_parse_palette_entry(value.as_ptr().cast(), value.len(), &mut idx, &mut out) };
    (res == ffi::GHOSTTY_SUCCESS).then(|| (idx, Rgb::from_ffi(out)))
}

/// Ghostty's built-in default 256-color palette.
pub fn default_palette() -> [Rgb; 256] {
    let _ = crate::sys::init();
    let mut raw = [ffi::GhosttyColorRgb::default(); 256];
    // SAFETY: color.h requires exactly 256 writable RGB entries.
    unsafe { ffi::ghostty_color_palette_default(raw.as_mut_ptr()) };
    std::array::from_fn(|i| Rgb::from_ffi(raw[i]))
}

/// Generate a 256-color palette with CIELAB interpolation from `base[0..16]`,
/// `bg`, and `fg`. Any index present in `preserve_indices` keeps its base value.
pub fn generate_extended_palette(base: &[Rgb; 256], preserve_indices: Option<&[u8]>, bg: Rgb, fg: Rgb, harmonious: bool) -> [Rgb; 256] {
    let _ = crate::sys::init();
    let base_ffi: [ffi::GhosttyColorRgb; 256] = std::array::from_fn(|i| base[i].to_ffi());
    let mut mask = ffi::GhosttyColorPaletteMask::default();
    let mask_ptr = if let Some(indices) = preserve_indices {
        for &idx in indices {
            let word = (idx >> 6) as usize;
            let bit = 1u64 << (idx & 63);
            mask.bits[word] |= bit;
        }
        &mask as *const ffi::GhosttyColorPaletteMask
    } else {
        std::ptr::null()
    };
    let bg_ffi = bg.to_ffi();
    let fg_ffi = fg.to_ffi();
    let mut out_ffi = [ffi::GhosttyColorRgb::default(); 256];
    // SAFETY: Both palettes contain 256 RGB values; the optional mask and colors live through the call.
    unsafe {
        ffi::ghostty_color_palette_generate(base_ffi.as_ptr(), mask_ptr, &bg_ffi, &fg_ffi, harmonious, out_ffi.as_mut_ptr());
    }
    std::array::from_fn(|i| Rgb::from_ffi(out_ffi[i]))
}

/// Parse a Ghostty theme file (`key = value` lines: `background`, `foreground`,
/// `cursor-color`, `cursor-text`, `selection-background`, `selection-foreground`,
/// `palette = N=#rrggbb`).
pub fn parse_ghostty_theme(name: impl Into<String>, text: &str) -> Result<TerminalTheme, String> {
    if text.len() > MAX_THEME_BYTES {
        return Err("theme exceeds 1 MiB".into());
    }
    let mut theme = TerminalTheme { name: name.into(), ..TerminalTheme::default() };
    let mut palette = default_palette();
    let mut saw_any = false;
    let mut saw_extended = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"');
        match key {
            "foreground" => {
                if let Some(c) = parse_color(val) {
                    theme.foreground = c;
                    saw_any = true;
                }
            }
            "background" => {
                if let Some(c) = parse_color(val) {
                    theme.background = c;
                    saw_any = true;
                }
            }
            "cursor-color" => {
                if let Some(color) = parse_color(val) {
                    theme.cursor = Some(color);
                    saw_any = true;
                }
            }
            "cursor-text" => {
                if let Some(color) = parse_color(val) {
                    theme.cursor_text = Some(color);
                    saw_any = true;
                }
            }
            "selection-background" => {
                if let Some(color) = parse_color(val) {
                    theme.selection_bg = Some(color);
                    saw_any = true;
                }
            }
            "selection-foreground" => {
                if let Some(color) = parse_color(val) {
                    theme.selection_fg = Some(color);
                    saw_any = true;
                }
            }
            "palette" => {
                if let Some((idx, c)) = parse_palette_entry(val) {
                    palette[idx as usize] = c;
                    if idx >= 16 {
                        saw_extended = true;
                    }
                    saw_any = true;
                }
            }
            _ => {}
        }
    }

    if !saw_any {
        return Err("no Ghostty theme color entries found".to_owned());
    }
    theme.palette = palette.to_vec();
    theme.extended_palette = saw_extended;
    Ok(theme)
}

/// Parse an iTerm2 `.itermcolors` XML plist into a [`TerminalTheme`].
pub fn parse_itermcolors(name: impl Into<String>, xml: &str) -> Result<TerminalTheme, String> {
    if xml.len() > MAX_THEME_BYTES {
        return Err("theme exceeds 1 MiB".into());
    }
    let mut theme = TerminalTheme { name: name.into(), ..TerminalTheme::default() };
    let mut palette = default_palette();
    let mut saw_any = false;

    let mut pos = 0usize;
    while let Some(k_start) = xml[pos..].find("<key>") {
        let k_abs = pos + k_start + 5;
        let Some(k_end) = xml[k_abs..].find("</key>") else {
            break;
        };
        let key = xml[k_abs..k_abs + k_end].trim();
        let after_key = k_abs + k_end + 6;
        let rest = xml[after_key..].trim_start();
        if !rest.starts_with("<dict>") {
            pos = after_key;
            continue;
        }
        let dict_offset = after_key + (xml[after_key..].len() - rest.len()) + 6;
        let Some(dict_end_rel) = xml[dict_offset..].find("</dict>") else {
            break;
        };
        let dict_body = &xml[dict_offset..dict_offset + dict_end_rel];
        pos = dict_offset + dict_end_rel + 7;

        let Some(rgb) = parse_iterm_color_dict(dict_body) else {
            continue;
        };
        if let Some(rest_idx) = key.strip_prefix("Ansi ")
            && let Some(num_str) = rest_idx.strip_suffix(" Color")
            && let Ok(idx) = num_str.parse::<usize>()
            && idx < 16
        {
            palette[idx] = rgb;
            saw_any = true;
            continue;
        }
        match key {
            "Foreground Color" => {
                theme.foreground = rgb;
                saw_any = true;
            }
            "Background Color" => {
                theme.background = rgb;
                saw_any = true;
            }
            "Cursor Color" => {
                theme.cursor = Some(rgb);
                saw_any = true;
            }
            "Cursor Text Color" => {
                theme.cursor_text = Some(rgb);
                saw_any = true;
            }
            "Selection Color" => {
                theme.selection_bg = Some(rgb);
                saw_any = true;
            }
            "Selected Text Color" => {
                theme.selection_fg = Some(rgb);
                saw_any = true;
            }
            _ => {}
        }
    }

    if !saw_any {
        return Err("no iTerm2 color entries found in plist".to_owned());
    }
    theme.palette = palette.to_vec();
    Ok(theme)
}

fn parse_iterm_color_dict(dict: &str) -> Option<Rgb> {
    let mut r = None;
    let mut g = None;
    let mut b = None;
    let mut p = 0usize;
    while let Some(ks) = dict[p..].find("<key>") {
        let ka = p + ks + 5;
        let ke = dict[ka..].find("</key>")?;
        let key = dict[ka..ka + ke].trim();
        let after = ka + ke + 6;
        let rest = dict[after..].trim_start();
        let (tag_open, tag_close) = if rest.starts_with("<real>") {
            ("<real>", "</real>")
        } else if rest.starts_with("<integer>") {
            ("<integer>", "</integer>")
        } else {
            p = after;
            continue;
        };
        let v_start = after + (dict[after..].len() - rest.len()) + tag_open.len();
        let v_end = dict[v_start..].find(tag_close)?;
        let val: f64 = dict[v_start..v_start + v_end].trim().parse().ok()?;
        if !val.is_finite() {
            return None;
        }
        let byte = (val.clamp(0.0, 1.0) * 255.0).round() as u8;
        match key {
            "Red Component" => r = Some(byte),
            "Green Component" => g = Some(byte),
            "Blue Component" => b = Some(byte),
            _ => {}
        }
        p = v_start + v_end + tag_close.len();
    }
    Some(Rgb::new(r?, g?, b?))
}
