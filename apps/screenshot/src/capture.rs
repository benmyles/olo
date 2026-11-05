//! The work, all on worker threads: asking app_server for the picture
//! (hiding our own window while it is taken), and writing it out.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use olo_protocol::{Client, Reply, Request, WindowInfo};
use serde::{Deserialize, Serialize};

/// What to take; Screenshot's settings are these plus where to save.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub window: bool,
    pub border: bool,
    pub pointer: bool,
    pub delay: u32,
    pub format: Format,
    pub folder: Option<PathBuf>,
    /// The window to take (`--window ID`), rather than the active one:
    /// from the command palette, which is itself active by then.
    #[serde(skip)]
    pub window_id: Option<olo_protocol::WindowId>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { window: false, border: true, pointer: false, delay: 0, format: Format::Png, folder: None, window_id: None }
    }
}

fn settings_path() -> PathBuf {
    olo_xdg::config_dir().join("screenshot.json")
}

impl Settings {
    pub fn load() -> Settings {
        olo_ui::store::load(&settings_path()).unwrap_or_default()
    }

    /// Off the UI thread.
    pub fn store(&self) {
        olo_ui::store::save(settings_path(), self);
    }

    /// Where pictures go: the chosen folder, else Pictures, else home.
    pub fn folder(&self) -> PathBuf {
        self.folder.clone().filter(|f| f.is_dir()).unwrap_or_else(default_folder)
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

pub fn default_folder() -> PathBuf {
    olo_xdg::user_dir("PICTURES").unwrap_or_else(home)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Png,
    Jpeg,
    Bmp,
    Tiff,
    WebP,
}

impl Format {
    pub const ALL: [Format; 5] = [Format::Png, Format::Jpeg, Format::Bmp, Format::Tiff, Format::WebP];

    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG image",
            Format::Jpeg => "JPEG image",
            Format::Bmp => "BMP image",
            Format::Tiff => "TIFF image",
            Format::WebP => "WebP image",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
            Format::WebP => "webp",
        }
    }

    fn image_format(self) -> image::ImageFormat {
        match self {
            Format::Png => image::ImageFormat::Png,
            Format::Jpeg => image::ImageFormat::Jpeg,
            Format::Bmp => image::ImageFormat::Bmp,
            Format::Tiff => image::ImageFormat::Tiff,
            Format::WebP => image::ImageFormat::WebP,
        }
    }
}

/// A picture taken: the PNG app_server wrote, and its pixels.
#[derive(Clone)]
pub struct Shot {
    pub png: PathBuf,
    pub image: Arc<image::RgbaImage>,
}

/// Where app_server writes our pictures (one per process; replaced by the
/// next).
pub fn scratch_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join(format!("olo-screenshot-{}.png", std::process::id()))
}

/// Removes pictures left by Screenshots that ended without tidying up
/// (killed): the runtime directory is memory. One whose process is still
/// running (another Screenshot's) stays; a reload keeps its pid, and its
/// own.
pub fn sweep() {
    sweep_in(&std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from));
}

fn sweep_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix("olo-screenshot-")).and_then(|n| n.split('.').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
        let ours = std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("olo-screenshot")));
        if !ours {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn request(client: &mut Client, req: &Request) -> Result<Reply, String> {
    match client.request(req).map_err(|e| e.to_string())? {
        Reply::Error { message } => Err(message),
        reply => Ok(reply),
    }
}

fn windows(client: &mut Client) -> Result<Vec<WindowInfo>, String> {
    match request(client, &Request::ListWindows)? {
        Reply::Windows { windows } => Ok(windows),
        _ => Err("unexpected reply".into()),
    }
}

/// Take a screenshot as `settings` say. With `hide`, our own windows are
/// minimized first (and brought back after), so they aren't in it and
/// the active window is the one behind them.
pub fn take(settings: &Settings, hide: bool) -> Result<Shot, String> {
    let mut client = Client::connect().map_err(|e| e.to_string())?;
    let me = std::process::id() as i32;
    let mine: Vec<u64> = if hide { windows(&mut client)?.iter().filter(|w| w.pid == Some(me) && !w.minimized).map(|w| w.id).collect() } else { Vec::new() };
    for &id in &mine {
        request(&mut client, &Request::Minimize { id })?;
    }
    if !mine.is_empty() {
        // Long enough for the next frame to be drawn without them.
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(settings.delay.min(60) as u64));
    let result = (|| {
        let window = if let Some(id) = settings.window_id {
            Some(id)
        } else if settings.window {
            // The active window, or the whole screen if there is none.
            windows(&mut client)?.into_iter().find(|w| w.focused && !w.minimized && w.pid != Some(me)).map(|w| w.id)
        } else {
            None
        };
        let png = scratch_path();
        request(
            &mut client,
            &Request::Screenshot { path: png.display().to_string(), output: None, window, border: settings.border, pointer: settings.pointer, format: Default::default(), as_shown: false },
        )?;
        let image = image::open(&png).map_err(|e| e.to_string())?.into_rgba8();
        Ok(Shot { png, image: Arc::new(image) })
    })();
    for &id in &mine {
        let _ = request(&mut client, &Request::Activate { id });
    }
    result
}

/// A recording started from Screenshot's window: our windows minimized
/// first (so they aren't in it, and the active window is the one behind
/// them, as for `take`), then the active window, if there is one.
pub fn step_aside() -> Result<Option<u64>, String> {
    let mut client = Client::connect().map_err(|e| e.to_string())?;
    let me = std::process::id() as i32;
    let mine: Vec<u64> = windows(&mut client)?.iter().filter(|w| w.pid == Some(me) && !w.minimized).map(|w| w.id).collect();
    for &id in &mine {
        request(&mut client, &Request::Minimize { id })?;
    }
    if !mine.is_empty() {
        // Long enough for the next frame to be drawn without them.
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(windows(&mut client)?.into_iter().find(|w| w.focused && !w.minimized && w.pid != Some(me)).map(|w| w.id))
}

/// One output's picture, as it is (no pointer), for the region picker.
pub fn take_output(output: Option<String>) -> Result<image::RgbaImage, String> {
    let mut client = Client::connect().map_err(|e| e.to_string())?;
    let png = scratch_path().with_extension(format!("{}.png", output.as_deref().unwrap_or("screen")));
    request(&mut client, &Request::Screenshot { path: png.display().to_string(), output, window: None, border: true, pointer: false, format: Default::default(), as_shown: false })?;
    let image = image::open(&png).map_err(|e| e.to_string())?.into_rgba8();
    let _ = std::fs::remove_file(&png);
    Ok(image)
}

/// `screenshotN`, the first N not taken in `folder` in any format.
pub fn next_name(folder: &Path) -> String {
    (1..).map(|n| format!("screenshot{n}")).find(|stem| !Format::ALL.iter().any(|f| folder.join(format!("{stem}.{}", f.extension())).exists())).unwrap_or_else(|| "screenshot".into())
}

/// The name with `format`'s extension (replacing any image extension).
pub fn file_name(name: &str, format: Format) -> String {
    let name = name.trim();
    let stem = Format::ALL
        .iter()
        .find_map(|f| {
            let ext = format!(".{}", f.extension());
            name.len().checked_sub(ext.len()).filter(|&i| name[i..].eq_ignore_ascii_case(&ext)).map(|i| &name[..i])
        })
        .or_else(|| name.strip_suffix(".jpeg"))
        .unwrap_or(name);
    format!("{stem}.{}", format.extension())
}

/// Write `shot` to `path` as `format`; whole or not at all.
pub fn save(shot: &Shot, path: &Path, format: Format) -> Result<(), String> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let res = match format {
        // Already encoded.
        Format::Png => std::fs::copy(&shot.png, &part).map(|_| ()).map_err(|e| e.to_string()),
        // No alpha: flatten onto white.
        Format::Jpeg => {
            let mut rgb = image::RgbImage::new(shot.image.width(), shot.image.height());
            for (d, s) in rgb.pixels_mut().zip(shot.image.pixels()) {
                let a = s.0[3] as u32;
                *d = image::Rgb(std::array::from_fn(|k| ((s.0[k] as u32 * a + 255 * (255 - a)) / 255) as u8));
            }
            rgb.save_with_format(&part, format.image_format()).map_err(|e| e.to_string())
        }
        _ => shot.image.save_with_format(&part, format.image_format()).map_err(|e| e.to_string()),
    };
    match res.and_then(|()| std::fs::rename(&part, path).map_err(|e| e.to_string())) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeping_takes_only_dead_screenshots_pictures() {
        let dir = std::env::temp_dir().join(format!("olo-sweep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No process has a pid this high.
        let dead = dir.join("olo-screenshot-4000000000.png");
        let dead_region = dir.join("olo-screenshot-4000000000.HEADLESS-1.png");
        let other = dir.join("olo-restore-1.json");
        for f in [&dead, &dead_region, &other] {
            std::fs::write(f, b"x").unwrap();
        }
        sweep_in(&dir);
        assert!(!dead.exists() && !dead_region.exists());
        assert!(other.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names() {
        assert_eq!(file_name("screenshot1", Format::Png), "screenshot1.png");
        assert_eq!(file_name("shot.PNG", Format::Jpeg), "shot.jpg");
        assert_eq!(file_name("shot.jpeg", Format::Png), "shot.png");
        assert_eq!(file_name("my.notes", Format::Bmp), "my.notes.bmp");
        let dir = std::env::temp_dir().join(format!("olo-shot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(next_name(&dir), "screenshot1");
        std::fs::write(dir.join("screenshot1.jpg"), b"").unwrap();
        assert_eq!(next_name(&dir), "screenshot2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_every_format() {
        let dir = std::env::temp_dir().join(format!("olo-shot-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbaImage::new(4, 3);
        img.put_pixel(1, 1, image::Rgba([255, 0, 0, 128]));
        let png = dir.join("src.png");
        img.save(&png).unwrap();
        let shot = Shot { png, image: Arc::new(img) };
        for f in Format::ALL {
            let out = dir.join(file_name("out", f));
            save(&shot, &out, f).unwrap();
            let back = image::open(&out).unwrap();
            assert_eq!((back.width(), back.height()), (4, 3), "{f:?}");
            assert!(!dir.join(format!("out.{}.part", f.extension())).exists());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
