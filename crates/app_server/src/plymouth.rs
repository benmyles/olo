//! Olo's boot splash: a Plymouth theme of the start-up screen's picture
//! (`veil.rs`: the wordmark on dark, the row of icons lit one by one, the
//! line under them), shown from the moment the computer starts until the
//! login screen takes its place (`olo-login`, which hands over from
//! Plymouth as GDM does). `olo-app-server --plymouth-theme DIR [--scale
//! S]` makes it; `tools/install-login.sh --plymouth` installs it.
//!
//! Plymouth's two-step plugin draws it (installed with Plymouth, as
//! Fedora's own themes are). Two-step draws a theme's pictures at its
//! device scale, 1 or 2 by the screen's density, and resamples them for
//! 2 (`ply_pixels_interpolate`): sharp only at 1. So the theme is made at
//! the scale the computer's own screen is at in Olo (`boot_scale`), and
//! Plymouth is told to draw device pixels (`DeviceScale=1` in
//! plymouthd.conf): that screen shows the picture pixel for pixel, at the
//! size the start-up screen is, and any other screen the same pixels.
//!
//! Two-step centres its pictures (each alignment .5, which every part
//! rounds alike, so they line up to the pixel) and asks a disk's
//! passphrase in a field at the screen's middle. Hence the theme's files:
//!
//! - `progress-0000.png` … `progress-0004.png`: the picture, its icons lit
//!   one to four as the boot goes (two-step shows frame ⌊fraction ×
//!   (frames − 1)⌋ and the fraction only nears 1, so the last frame is the
//!   finished one and four lit is the fourth frame too), saying "Starting
//!   Olo…";
//! - `watermark.png`: the wordmark alone, all that's shown while the
//!   computer shuts down or restarts and over which the field is asked;
//!   the same size as the frames, both opaque on the start-up screen's
//!   dark (a frame drawn over it changes no pixel of the wordmark) and
//!   padded below so the icon row is in their middle, where the field goes;
//! - `lock.png` and `entry.png`: the kit's text well as it is focused
//!   (`style::sunken`, the login screen's field's 328 points wide), cut in
//!   two: two-step starts the bullets where `lock.png` ends, half a bullet
//!   in, so `lock.png` is as much of the well as puts the first bullet
//!   where the kit's text starts (`metrics::TEXT_INSET`);
//! - `bullet.png`: the kit's •, a bullet's advance wide;
//! - `olo.plymouth`: two-step's settings: the background, the prompt's
//!   face and size (Noto Sans, Olo's, at the line's size in whole pixels:
//!   the start-up image's label-freetype reads no fractions), the update
//!   modes' bar and words;
//! - `olo-made.json`: what it was made with (`olo_system::splash::Made`:
//!   the drawing's `FORMAT` and the scale), so Olo Doctor sees an
//!   installed one is old without drawing it again. Change the drawing,
//!   and `FORMAT` goes up by one.

use std::path::{Path, PathBuf};

use tiny_skia::{IntRect, Pixmap, PixmapPaint};

use crate::veil::{self, Parts, Why};

/// Where the theme is installed (its `ImageDir`).
pub const THEME_DIR: &str = olo_system::splash::THEME_DIR;
/// The frames: four stages, and the finished one.
const FRAMES: usize = 5;
/// What the boot's line says.
const LINE: &str = "Starting Olo…";
/// The line's size, points (the start-up screen's, `veil::TEXT`): the
/// prompt under the field is at it too.
const TEXT: f32 = 15.0;
/// The kit's text well: a control's height and where its text starts
/// (`olo_kit_base::metrics`), and the login screen's password field's
/// width (its 360-point panel less a window's inset each side).
const CONTROL_HEIGHT: f32 = 24.0;
const TEXT_INSET: f32 = 6.0;
const FIELD_WIDTH: f32 = 360.0 - 2.0 * 16.0;
/// The kit's status bar's height (an update's progress).
const BAR_HEIGHT: f32 = 14.0;

/// What `write` made.
#[derive(Debug)]
pub struct Made {
    pub scale: f64,
    pub files: Vec<PathBuf>,
    /// The pictures' size and the field's (device pixels).
    pub picture: (u32, u32),
    pub field: (u32, u32),
}

/// `--plymouth-theme DIR [--scale S]`: the theme written into DIR (made
/// at S, else at `boot_scale`), what was made printed as JSON.
pub fn run(args: &[String]) -> i32 {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let Some(dir) = value("--plymouth-theme") else {
        eprintln!("olo-app-server: --plymouth-theme takes the folder to write the theme into");
        return 2;
    };
    let scale = match value("--scale").map(|s| s.parse::<f64>()) {
        Some(Ok(s)) if (0.5..=4.0).contains(&s) => s,
        Some(_) => {
            eprintln!("olo-app-server: --scale takes a scale from 0.5 to 4");
            return 2;
        }
        None => boot_scale(),
    };
    match write(Path::new(dir), scale) {
        Ok(made) => {
            #[allow(clippy::disallowed_macros)]
            {
                println!(
                    "{}",
                    serde_json::json!({
                        "dir": dir, "scale": made.scale, "files": made.files, "picture": made.picture, "field": made.field, "image_dir": THEME_DIR,
                    })
                );
            }
            0
        }
        Err(e) => {
            eprintln!("olo-app-server: the boot splash wasn't made: {e}");
            1
        }
    }
}

/// The scale the computer's own screen is at in Olo: its built-in
/// panel's (eDP, LVDS, DSI) when one is connected, else the main
/// display's, else Olo's default (`olo_system::splash::boot_scale`, which
/// Olo Doctor checks an installed theme against).
pub fn boot_scale() -> f64 {
    olo_system::splash::boot_scale(&crate::desktop::load_settings().display, Path::new("/sys"))
}

/// The theme at `scale`, written into `dir`.
pub fn write(dir: &Path, scale: f64) -> Result<Made, String> {
    let theme = Theme::make(scale).ok_or("Olo's wordmark or icons weren't found (data/brand, data/icons)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files = Vec::new();
    let mut save = |name: &str, pm: &Pixmap| -> Result<(), String> {
        let path = dir.join(name);
        pm.save_png(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        files.push(path);
        Ok(())
    };
    save("watermark.png", &theme.watermark)?;
    for (k, frame) in theme.frames.iter().enumerate() {
        save(&format!("progress-{k:04}.png"), frame)?;
    }
    save("lock.png", &theme.lock)?;
    save("entry.png", &theme.entry)?;
    save("bullet.png", &theme.bullet)?;
    let path = dir.join("olo.plymouth");
    std::fs::write(&path, settings(scale)).map_err(|e| format!("{}: {e}", path.display()))?;
    files.push(path);
    // What it was made with, for Olo Doctor (`olo_system::splash`).
    let path = dir.join(olo_system::splash::STAMP);
    let made = olo_system::splash::Made { format: olo_system::splash::FORMAT, scale };
    std::fs::write(&path, serde_json::to_vec(&made).unwrap_or_default()).map_err(|e| format!("{}: {e}", path.display()))?;
    files.push(path);
    Ok(Made { scale, files, picture: (theme.watermark.width(), theme.watermark.height()), field: (theme.lock.width() + theme.entry.width(), theme.entry.height()) })
}

/// The theme's pictures.
struct Theme {
    watermark: Pixmap,
    frames: Vec<Pixmap>,
    lock: Pixmap,
    entry: Pixmap,
    bullet: Pixmap,
    /// Where the icon row is in the pictures (device pixels): its top,
    /// an icon's side.
    #[cfg_attr(not(test), allow(dead_code))]
    row: (u32, u32),
    /// Where the first bullet's pen goes, from the well's left.
    #[cfg_attr(not(test), allow(dead_code))]
    text_inset: u32,
}

impl Theme {
    fn make(scale: f64) -> Option<Theme> {
        let mark = veil::drawn_as(Why::Starting, scale, 0, Some(LINE), Parts::Wordmark)?;
        let (width, height) = (mark.pixmap.width(), mark.pixmap.height());
        // Padded so the row's middle is the picture's (twice it is the
        // row's top twice and an icon's side).
        let twice = 2 * mark.row_top + mark.icon_slot;
        let (top, padded) = if twice >= height { (0, twice) } else { (height - twice, 2 * height - twice) };
        let background = background();
        let opaque = |pm: &Pixmap| -> Option<Pixmap> {
            let mut out = Pixmap::new(width, padded)?;
            out.fill(tiny_skia::Color::from_rgba8(background[0], background[1], background[2], 255));
            out.draw_pixmap(0, top as i32, pm.as_ref(), &PixmapPaint { quality: tiny_skia::FilterQuality::Nearest, ..Default::default() }, tiny_skia::Transform::identity(), None);
            Some(out)
        };
        let watermark = opaque(&mark.pixmap)?;
        let frames = (0..FRAMES).map(|k| veil::drawn_as(Why::Starting, scale, (k + 1).min(4), Some(LINE), Parts::All).and_then(|d| opaque(&d.pixmap))).collect::<Option<Vec<_>>>()?;
        let (lock, entry, bullet, text_inset) = field(scale)?;
        Some(Theme { watermark, frames, lock, entry, bullet, row: (top + mark.row_top, mark.icon_slot), text_inset })
    }
}

/// The start-up screen's background, as bytes.
fn background() -> [u8; 3] {
    let b = |c: f32| (c * 255.0).round() as u8;
    [b(veil::BACKGROUND[0]), b(veil::BACKGROUND[1]), b(veil::BACKGROUND[2])]
}

/// A rectangle of `rgba` on whole pixels.
fn rect(pm: &mut Pixmap, x: u32, y: u32, w: u32, h: u32, rgba: [u8; 4]) {
    let Some(r) = tiny_skia::Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else { return };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = false;
    pm.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
}

/// The kit's focused text well at `scale` (`style::sunken` in the dark
/// palette: a dark and light outer bevel, the keyboard's colour inside
/// it, the document's background filling it), cut where two-step starts
/// the bullets; the bullet; where the first bullet's pen is.
fn field(scale: f64) -> Option<(Pixmap, Pixmap, Pixmap, u32)> {
    let p = olo_theme::Palette::dark();
    let px = |pt: f32| (f64::from(pt) * scale).round() as u32;
    let (w, h) = (px(FIELD_WIDTH), px(CONTROL_HEIGHT));
    // (A line is a whole number of device pixels: `style::line_pixels`.)
    let l = (scale.floor() as u32).max(1);
    let mut well = Pixmap::new(w, h)?;
    rect(&mut well, 2 * l, 2 * l, w - 4 * l, h - 4 * l, p.document_background);
    let dark = olo_theme::tint(p.panel_background, olo_theme::B_DARKEN_1_TINT);
    rect(&mut well, 0, 0, w - l, l, dark);
    rect(&mut well, 0, l, l, h - 2 * l, dark);
    rect(&mut well, 0, h - l, w, l, p.shine);
    rect(&mut well, w - l, 0, l, h - l, p.shine);
    let k = p.keyboard_navigation;
    rect(&mut well, l, l, w - 2 * l, l, k);
    rect(&mut well, l, h - 2 * l, w - 2 * l, l, k);
    rect(&mut well, l, 2 * l, l, h - 4 * l, k);
    rect(&mut well, w - 2 * l, 2 * l, l, h - 4 * l, k);
    // The bullet, as the kit's password field draws it, its pen at 0.
    let size = olo_theme::fonts::PLAIN_SIZE * scale as f32;
    let advance = (crate::text::measure("•", size, false).round() as u32).max(1);
    let mut bullet = Pixmap::new(advance, h)?;
    crate::text::draw(&mut bullet, "•", 0.0, crate::text::top_for_centre(h as f32 / 2.0, size, false), size, false, p.document_text);
    // Two-step's first bullet is half a bullet past the cut.
    let inset = px(TEXT_INSET);
    let cut = inset.saturating_sub(advance / 2).max(2 * l);
    let lock = well.clone_rect(IntRect::from_xywh(0, 0, cut, h)?)?;
    let entry = well.clone_rect(IntRect::from_xywh(cut as i32, 0, w - cut, h)?)?;
    Some((lock, entry, bullet, cut + advance / 2))
}

/// `0xrrggbb`.
fn hex(c: [u8; 4]) -> String {
    format!("0x{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// A face at `px` device pixels, as both of Plymouth's label plugins
/// read it ("Noto Sans 30px": Pango's, and label-freetype's, the one in
/// the start-up image, which reads whole numbers only).
fn font(face: &str, px: f64) -> String {
    format!("{face} {}px", px.round())
}

/// `olo.plymouth`: two-step's settings for the theme at `scale`.
fn settings(scale: f64) -> String {
    let p = olo_theme::Palette::dark();
    let bg = background();
    let bg = hex([bg[0], bg[1], bg[2], 255]);
    let text = f64::from(TEXT) * scale;
    let px = |pt: f32| (f64::from(pt) * scale).round() as u32;
    let modes = [("updates", "Installing Updates…"), ("system-upgrade", "Upgrading the System…"), ("firmware-upgrade", "Updating the Firmware…"), ("system-reset", "Resetting the System…")];
    let mut out = format!(
        "[Plymouth Theme]
Name=Olo
Description=Olo's start-up screen: its wordmark on dark and its icons lit one by one as the computer starts (made at {scale}× by olo-app-server --plymouth-theme)
ModuleName=two-step

[two-step]
ImageDir={THEME_DIR}
Font={font}
TitleFont={title}
HorizontalAlignment=.5
VerticalAlignment=.5
WatermarkHorizontalAlignment=.5
WatermarkVerticalAlignment=.5
DialogHorizontalAlignment=.5
DialogVerticalAlignment=.5
TitleHorizontalAlignment=.5
TitleVerticalAlignment=.62
Transition=none
TransitionDuration=0.0
BackgroundStartColor={bg}
BackgroundEndColor={bg}
ProgressBarBackgroundColor={bar_bg}
ProgressBarForegroundColor={bar_fg}
ProgressBarWidth={bar_w}
ProgressBarHeight={bar_h}
ProgressBarHorizontalAlignment=.5
ProgressBarVerticalAlignment=.5
MessageBelowAnimation=true

[boot-up]
UseEndAnimation=false
UseFirmwareBackground=false

[shutdown]
UseEndAnimation=false

[reboot]
UseEndAnimation=false
",
        font = font("Noto Sans", text),
        title = font("Noto Sans Bold", text),
        bar_bg = hex(p.document_background),
        bar_fg = hex(p.status_bar),
        bar_w = px(FIELD_WIDTH),
        bar_h = px(BAR_HEIGHT),
    );
    for (mode, title) in modes {
        out.push_str(&format!("\n[{mode}]\nSuppressMessages=true\nUseProgressBar=true\nProgressBarShowPercentComplete=true\nTitle={title}\nSubTitle=Don't turn off the computer.\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    /// Where two-step puts a picture `w` × `h` (its alignments .5) on a
    /// screen: `long x = .5 * W - w / 2.0`, cut toward zero.
    fn placed(screen: (u32, u32), w: u32, h: u32) -> (i64, i64) {
        ((0.5 * f64::from(screen.0) - f64::from(w) / 2.0) as i64, (0.5 * f64::from(screen.1) - f64::from(h) / 2.0) as i64)
    }

    /// The field's left and top on a screen (no box: the dialog is the
    /// lock and the entry side by side, centred).
    fn field_at(screen: (u32, u32), t: &Theme) -> (i64, i64) {
        let w = t.lock.width() + t.entry.width();
        ((f64::from(screen.0 - w) * 0.5) as i64, (f64::from(screen.1 - t.entry.height()) * 0.5) as i64)
    }

    #[test]
    fn the_pictures_line_up() {
        for s in SCALES {
            let t = Theme::make(s).unwrap();
            let (w, h) = (t.watermark.width(), t.watermark.height());
            // Every frame the watermark's size, opaque, and the same as it
            // wherever the wordmark is (drawn over it, it changes nothing).
            for f in &t.frames {
                assert_eq!((f.width(), f.height()), (w, h), "at {s}×");
                assert!(f.pixels().iter().all(|p| p.alpha() == 255), "at {s}×");
                let above_row = (t.row.0 * w * 4) as usize;
                assert_eq!(f.data()[..above_row], t.watermark.data()[..above_row], "at {s}×");
            }
            // The icons lit one by one, the last two the same.
            assert_ne!(t.frames[0].data(), t.frames[1].data());
            assert_eq!(t.frames[3].data(), t.frames[4].data());
            // The field in the icon row's place, to a pixel, on screens of
            // either parity.
            for screen in [(1280, 800), (3200, 2000), (2561, 1601), (1920, 1201)] {
                let (_, py) = placed(screen, w, h);
                let (_, fy) = field_at(screen, &t);
                let row = py as f32 + t.row.0 as f32 + t.row.1 as f32 / 2.0;
                let field = fy as f32 + t.entry.height() as f32 / 2.0;
                assert!((row - field).abs() <= 1.0, "at {s}× on {screen:?}: row {row}, field {field}");
            }
            // The well's two halves are as tall; the first bullet starts
            // where the kit's text does.
            assert_eq!(t.lock.height(), t.entry.height());
            assert_eq!(t.text_inset, (TEXT_INSET as f64 * s).round() as u32, "at {s}×");
            assert!(t.lock.width() >= 2 * (s.floor() as u32).max(1));
        }
    }

    #[test]
    fn its_measures_are_the_kits() {
        use olo_kit_base::metrics as m;
        assert_eq!(CONTROL_HEIGHT, m::CONTROL_HEIGHT);
        assert_eq!(TEXT_INSET, m::TEXT_INSET);
        assert_eq!(FIELD_WIDTH, 360.0 - 2.0 * m::WINDOW_INSET);
    }

    #[test]
    fn its_settings_say_where_it_is() {
        let s = settings(2.0);
        assert!(s.contains("ModuleName=two-step"));
        assert!(s.contains(&format!("ImageDir={THEME_DIR}")));
        assert!(s.contains("BackgroundStartColor=0x1e1e22"));
        // 15 points at 2× is 30 pixels.
        assert!(s.contains("Font=Noto Sans 30px\n"));
        assert!(settings(1.25).contains("Font=Noto Sans 19px\n"));
        assert!(s.contains("ProgressBarWidth=656\n"));
    }

    /// `OLO_PLYMOUTH_SHEET=/var/tmp/dir cargo test -p app_server plymouth_sheet`:
    /// the theme on a screen as two-step draws it (booting, asking a
    /// passphrase, shutting down), at each scale, cropped round the
    /// picture.
    #[test]
    fn plymouth_sheet() {
        let Some(out) = std::env::var_os("OLO_PLYMOUTH_SHEET") else { return };
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let paint = PixmapPaint::default();
        let id = tiny_skia::Transform::identity();
        for s in SCALES {
            let t = Theme::make(s).unwrap();
            let screen = ((1280.0 * s).round() as u32, (800.0 * s).round() as u32);
            let (w, h) = (t.watermark.width(), t.watermark.height());
            let (px, py) = placed(screen, w, h);
            let bg = background();
            let shot = |state: &str| {
                let mut pm = Pixmap::new(screen.0, screen.1).unwrap();
                pm.fill(tiny_skia::Color::from_rgba8(bg[0], bg[1], bg[2], 255));
                pm.draw_pixmap(px as i32, py as i32, t.watermark.as_ref(), &paint, id, None);
                match state {
                    "boot-1" => pm.draw_pixmap(px as i32, py as i32, t.frames[0].as_ref(), &paint, id, None),
                    "boot-3" => pm.draw_pixmap(px as i32, py as i32, t.frames[2].as_ref(), &paint, id, None),
                    "boot-done" => pm.draw_pixmap(px as i32, py as i32, t.frames[4].as_ref(), &paint, id, None),
                    "passphrase" => {
                        let (fx, fy) = field_at(screen, &t);
                        pm.draw_pixmap(fx as i32, fy as i32, t.lock.as_ref(), &paint, id, None);
                        let ex = fx + i64::from(t.lock.width());
                        pm.draw_pixmap(ex as i32, fy as i32, t.entry.as_ref(), &paint, id, None);
                        let bw = i64::from(t.bullet.width());
                        for i in 0..9 {
                            pm.draw_pixmap((ex + i * bw + bw / 2) as i32, fy as i32, t.bullet.as_ref(), &paint, id, None);
                        }
                        // (Pango draws the prompt, 10 pixels under; ours
                        // here, near enough to see the whole.)
                        let size = (f64::from(TEXT) * s) as f32;
                        let prompt = "Please enter passphrase for disk luks-6c8fb2b4";
                        let tw = crate::text::measure(prompt, size, false);
                        crate::text::draw(&mut pm, prompt, ((screen.0 as f32 - tw) / 2.0).round(), (fy + i64::from(t.entry.height()) + 10) as f32, size, false, [255, 255, 255, 255]);
                    }
                    _ => {}
                }
                // Cropped round the picture.
                let crop = IntRect::from_xywh(px as i32 - 40, py as i32 - 40, w + 80, h + 80).unwrap();
                pm.clone_rect(crop).unwrap().save_png(out.join(format!("{s}x-{state}.png"))).unwrap();
            };
            for state in ["boot-1", "boot-3", "boot-done", "passphrase", "shutdown"] {
                shot(state);
            }
        }
    }
}
