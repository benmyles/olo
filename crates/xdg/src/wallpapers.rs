//! The wallpaper pack (data/wallpapers): pictures in a 16:9 (4K) and a
//! 21:9 (ultrawide) version, some with a dark-mode version. The setting
//! names one as `pack:NAME`; anything else is a file of the person's own.

use std::path::{Path, PathBuf};

/// The setting's prefix for a picture from the pack.
pub const PACK: &str = "pack:";

/// One of the pack's pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper {
    pub name: String,
    /// For people: "Dusk".
    pub label: String,
    /// Which collection it's in, for people: "Mac OS 9", "Olo".
    pub collection: String,
    /// It has a version for dark mode.
    pub dark: bool,
}

impl Wallpaper {
    /// What the setting holds for it.
    pub fn setting(&self) -> String {
        format!("{PACK}{}", self.name)
    }
}

/// Where the pack is: the first data directory that has one.
pub fn dir() -> Option<PathBuf> {
    crate::data_dirs().iter().map(|d| d.join("wallpapers")).find(|d| d.join("pack.txt").is_file())
}

/// The pack's pictures, in its order (the first is the default).
pub fn pack() -> Vec<Wallpaper> {
    let Some(dir) = dir() else { return Vec::new() };
    parse(&std::fs::read_to_string(dir.join("pack.txt")).unwrap_or_default(), &dir)
}

pub(crate) fn parse(text: &str, dir: &Path) -> Vec<Wallpaper> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split('|').map(str::trim);
            let (name, label) = (parts.next()?, parts.next()?);
            let collection = parts.next().unwrap_or("Olo");
            let dark = dir.join("4k").join(format!("{name}-dark.png")).is_file();
            Some(Wallpaper { name: name.to_owned(), label: label.to_owned(), collection: collection.to_owned(), dark })
        })
        .collect()
}

/// The setting a new install starts with: the default theme's own.
pub fn default_setting() -> String {
    crate::themes::wallpaper(crate::themes::DEFAULT)
}

/// Wider than this (width over height) takes the ultrawide version.
const ULTRAWIDE: f64 = 2.0;

/// The file to show for `setting` on a screen of `size` pixels, in dark
/// mode or not: a pack picture's version for the screen's shape and the
/// mode (falling back to the light one), or the person's own file. A pack
/// picture that's gone shows another: one named for a theme (the themes'
/// own were named after them until November 2026: `pack:platinum`) that
/// theme's wallpaper now, any other (BeBox's, Toybox's, …) the default
/// theme's.
pub fn resolve(setting: &str, size: (u32, u32), dark: bool) -> Option<PathBuf> {
    let setting = setting.trim();
    if !setting.starts_with(PACK) {
        return (!setting.is_empty()).then(|| PathBuf::from(setting));
    }
    resolve_in(&dir()?, setting, size, dark)
}

/// The setting `setting` stands for now, if it names a pack picture that
/// has gone (`pack:platinum`, from before the themes' pictures were
/// Mac OS 9's; BeBox's): the one [`resolve`] shows instead. app_server
/// keeps settings this way, so Backgrounds shows the picture that's on
/// the desktop chosen.
pub fn moved(setting: &str) -> Option<String> {
    moved_in(&dir()?, setting)
}

fn moved_in(dir: &Path, setting: &str) -> Option<String> {
    let name = setting.trim().strip_prefix(PACK)?;
    let here = |n: &str| dir.join("4k").join(format!("{n}.png")).is_file();
    if here(name) {
        return None;
    }
    let instead = crate::themes::resolve(name).wallpaper;
    here(instead).then(|| format!("{PACK}{instead}"))
}

/// [`resolve`] for a pack setting, the pack in `dir`.
fn resolve_in(dir: &Path, setting: &str, size: (u32, u32), dark: bool) -> Option<PathBuf> {
    let name = setting.strip_prefix(PACK)?;
    let shape = if size.0 as f64 >= size.1.max(1) as f64 * ULTRAWIDE { "ultrawide" } else { "4k" };
    let file = |n: &str| Some(dir.join(shape).join(format!("{n}.png"))).filter(|p| p.is_file());
    let pick = |name: &str| dark.then(|| file(&format!("{name}-dark"))).flatten().or_else(|| file(name));
    pick(name).or_else(|| pick(crate::themes::resolve(name).wallpaper))
}

/// `picture` has an HDR version (its gain map beside it,
/// `olo_images::gain::path_for`: `NAME.gain.png`), shown on a display in
/// HDR.
pub fn has_hdr(picture: &Path) -> bool {
    picture.with_extension("gain.png").is_file()
}

/// A pack picture's preview (light, or dark).
pub fn thumbnail(name: &str, dark: bool) -> Option<PathBuf> {
    let dir = dir()?.join("thumbs");
    let file = |n: &str| Some(dir.join(format!("{n}.png"))).filter(|p| p.is_file());
    dark.then(|| file(&format!("{name}-dark"))).flatten().or_else(|| file(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_pack_and_picks_by_shape_and_mode() {
        let dir = std::env::temp_dir().join(format!("olo-wallpapers-{}", std::process::id()));
        for d in ["4k", "ultrawide"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            for n in ["dusk", "dusk-dark", "blue"] {
                std::fs::write(dir.join(d).join(format!("{n}.png")), "png").unwrap();
            }
        }
        let pack = parse("# comment\ndusk|Dusk|Mac OS 9\nblue|Blue\n", &dir);
        assert_eq!(pack.iter().map(|w| (w.name.as_str(), w.label.as_str(), w.collection.as_str(), w.dark)).collect::<Vec<_>>(), [("dusk", "Dusk", "Mac OS 9", true), ("blue", "Blue", "Olo", false)]);
        assert_eq!(pack[0].setting(), "pack:dusk");
        assert_eq!(resolve("/x/mine.jpg", (100, 100), true), Some(PathBuf::from("/x/mine.jpg")));
        assert_eq!(resolve("  ", (100, 100), true), None);
        assert_eq!(resolve_in(&dir, "pack:dusk", (100, 100), true), Some(dir.join("4k/dusk-dark.png")));
        assert_eq!(resolve_in(&dir, "pack:blue", (300, 100), true), Some(dir.join("ultrawide/blue.png")));
        // One that's gone: the theme's it's named for, else the default's.
        let (default, platinum) = (crate::themes::resolve(crate::themes::DEFAULT).wallpaper, crate::themes::resolve("platinum").wallpaper);
        for d in ["4k", "ultrawide"] {
            for n in [default, platinum] {
                std::fs::write(dir.join(d).join(format!("{n}.png")), "png").unwrap();
            }
        }
        assert_eq!(resolve_in(&dir, "pack:bebox", (100, 100), true), Some(dir.join(format!("4k/{default}.png"))));
        assert_eq!(resolve_in(&dir, "pack:platinum", (100, 100), true), Some(dir.join(format!("4k/{platinum}.png"))));
        // And the setting kept as what's shown.
        assert_eq!(moved_in(&dir, "pack:platinum"), Some(format!("pack:{platinum}")));
        assert_eq!(moved_in(&dir, "pack:toybox"), Some(format!("pack:{default}")));
        assert_eq!(moved_in(&dir, "pack:dusk"), None);
        assert_eq!(moved_in(&dir, "/x/mine.jpg"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
