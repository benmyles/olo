//! The work, all on worker threads: asking app_server for the picture
//! (hiding our own window while it is taken), and writing it out.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use olo_protocol::{Client, Reply, Request, WindowInfo, notifications::FileId};
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
        olo_kit::store::load(&settings_path()).unwrap_or_default()
    }

    /// Off the UI thread.
    pub fn store(&self) {
        olo_kit::store::save(settings_path(), self);
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
    // (The notification of the picture before is not in this one:
    // saved.rs.)
    crate::saved::put_away();
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
    if settings.delay > 0 {
        std::thread::sleep(Duration::from_secs(settings.delay.min(60) as u64));
        // (And one that came up while this waited.)
        crate::saved::put_away();
    }
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
    next_name_from(folder, 1).1
}

/// The first free name from number `from` on, and its number. Taken is
/// anything there by that name, a link to nothing too (`symlink_metadata`:
/// `exists` said a dangling link's name was free, and its every save, a
/// thousand tries at that one name, failed).
fn next_name_from(folder: &Path, from: u32) -> (u32, String) {
    (from..)
        .map(|n| (n, format!("screenshot{n}")))
        .find(|(_, stem)| !Format::ALL.iter().any(|f| std::fs::symlink_metadata(folder.join(format!("{stem}.{}", f.extension()))).is_ok()))
        .unwrap_or((from, "screenshot".into()))
}

/// The name with `format`'s extension (replacing any image extension).
pub fn file_name(name: &str, format: Format) -> String {
    let name = name.trim();
    let stem = Format::ALL
        .iter()
        .find_map(|f| {
            let ext = format!(".{}", f.extension());
            // (Where its end begins on a letter's edge: "café123" has none
            // four bytes from its end, and cutting there ended the window.)
            name.len().checked_sub(ext.len()).filter(|&i| name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(&ext)).map(|i| &name[..i])
        })
        .or_else(|| name.strip_suffix(".jpeg"))
        .unwrap_or(name);
    format!("{stem}.{}", format.extension())
}

/// `shot` written as `format` into a file of this save's own in `folder`
/// (made new, never one that's there: two shortcuts at once each write
/// their own), whole and synced; that file, and what it is as written,
/// read from the descriptor it was written through (what its
/// notification's actions act on, and nothing else, ever).
fn write_part(shot: &Shot, folder: &Path, format: Format) -> Result<(PathBuf, FileId), String> {
    use std::io::Write as _;
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    for _ in 0..100 {
        let part = folder.join(format!(".olo-screenshot-{}-{}.part", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", folder.display())),
        };
        let written = (|| -> Result<FileId, String> {
            let mut w = std::io::BufWriter::new(&file);
            match format {
                // Already encoded.
                Format::Png => {
                    let mut png = std::fs::File::open(&shot.png).map_err(|e| e.to_string())?;
                    std::io::copy(&mut png, &mut w).map_err(|e| e.to_string())?;
                }
                // No alpha: flatten onto white.
                Format::Jpeg => {
                    let mut rgb = image::RgbImage::new(shot.image.width(), shot.image.height());
                    for (d, s) in rgb.pixels_mut().zip(shot.image.pixels()) {
                        let a = s.0[3] as u32;
                        *d = image::Rgb(std::array::from_fn(|k| ((s.0[k] as u32 * a + 255 * (255 - a)) / 255) as u8));
                    }
                    rgb.write_to(&mut w, format.image_format()).map_err(|e| e.to_string())?;
                }
                _ => shot.image.write_to(&mut w, format.image_format()).map_err(|e| e.to_string())?,
            }
            w.flush().map_err(|e| e.to_string())?;
            drop(w);
            file.sync_all().map_err(|e| e.to_string())?;
            FileId::of_file(&file).ok_or_else(|| "the file written couldn't be looked at".to_owned())
        })();
        return match written {
            Ok(id) => Ok((part, id)),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        };
    }
    Err(format!("{}: no free name to write in", folder.display()))
}

/// Write `shot` to `path` as `format` (an explicit Save: a name the
/// person gave, put over a file of that name), whole or not at all. The
/// file written, as it was written (whatever has its name since).
pub fn save(shot: &Shot, path: &Path, format: Format) -> Result<FileId, String> {
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let (part, id) = write_part(shot, folder, format)?;
    std::fs::rename(&part, path).map(|()| id).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("{}: {e}", path.display())
    })
}

/// Write `shot` as `format` under the first free name in `folder` (a
/// shortcut's: "screenshot4.png"), whole or not at all, and never over a
/// file: another save taking that name meanwhile, the next. Where it
/// went, and the file as written.
pub fn save_new(shot: &Shot, folder: &Path, format: Format) -> Result<(PathBuf, FileId), String> {
    let (part, id) = write_part(shot, folder, format)?;
    let mut from = 1;
    for _ in 0..1000 {
        let (n, stem) = next_name_from(folder, from);
        let path = folder.join(file_name(&stem, format));
        match crate::saved::rename_new(&part, &path) {
            Ok(()) => return Ok((path, id)),
            // (Taken meanwhile: the next number on, never that one again.)
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                from = n + 1;
                continue;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(format!("{}: {e}", path.display()));
            }
        }
    }
    let _ = std::fs::remove_file(&part);
    Err(format!("{}: no free name", folder.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shortcut's save takes the first free name and never a file's that's there; each writes a file of its
    /// own first (two at once each keep their own picture), and what's said of it is the file as written. An
    /// explicit Save is put over a file of its name.
    #[test]
    fn a_shortcuts_save_never_takes_a_files_place() {
        let dir = PathBuf::from(format!("/var/tmp/olo-shot-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("taken.png");
        let image = image::RgbaImage::from_pixel(4, 3, image::Rgba([9, 8, 7, 255]));
        image.save(&png).unwrap();
        let shot = Shot { png: png.clone(), image: Arc::new(image) };
        std::fs::write(dir.join("screenshot1.png"), b"someone's").unwrap();
        let (a, ida) = save_new(&shot, &dir, Format::Png).unwrap();
        let (b, idb) = save_new(&shot, &dir, Format::Png).unwrap();
        assert_eq!((a.file_name().unwrap().to_str(), b.file_name().unwrap().to_str()), (Some("screenshot2.png"), Some("screenshot3.png")));
        assert_ne!(ida, idb);
        assert_eq!((FileId::of(&a), FileId::of(&b)), (Some(ida), Some(idb)));
        assert_eq!(std::fs::read(dir.join("screenshot1.png")).unwrap(), b"someone's");
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&png).unwrap());
        // A link to nothing by the next name is a taken name: passed over, the save going to the one after
        // (taken for free, a thousand tries at it failed, and the picture was lost).
        std::os::unix::fs::symlink(dir.join("nowhere.png"), dir.join("screenshot4.png")).unwrap();
        let (c, _) = save_new(&shot, &dir, Format::Png).unwrap();
        assert_eq!(c.file_name().unwrap().to_str(), Some("screenshot5.png"));
        assert!(std::fs::symlink_metadata(dir.join("screenshot4.png")).unwrap().file_type().is_symlink());
        // (Each wrote its own file first, and none is left.)
        assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part")));
        // An explicit Save: over the file of its name, said as written.
        let id = save(&shot, &dir.join("screenshot1.png"), Format::Jpeg).unwrap();
        assert_eq!(FileId::of(&dir.join("screenshot1.png")), Some(id));
        assert_ne!(std::fs::read(dir.join("screenshot1.png")).unwrap(), b"someone's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A name is given its format's ending, whatever letters it is in.
    #[test]
    fn a_name_in_any_letters_gets_its_ending() {
        assert_eq!(file_name("café123", Format::Png), "café123.png");
        assert_eq!(file_name("画像", Format::Jpeg), "画像.jpg");
        assert_eq!(file_name("shot.PNG", Format::WebP), "shot.webp");
        assert_eq!(file_name(" shot.jpeg ", Format::Png), "shot.png");
    }

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
