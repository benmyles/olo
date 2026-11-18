//! The wallpaper pack (data/wallpapers): pictures in a 16:9 (4K) and a
//! 21:9 (ultrawide) version, some with a dark-mode version, and tiles
//! (`tiles/NAME.webp`: a desktop pattern, repeated across the screen at
//! its size in points, `Wallpaper::tile`). The setting names one as
//! `pack:NAME`; anything else is a file of the person's own.
//!
//! The pack's pictures are lossless WebP (the pixels as a PNG would have
//! them, a quarter smaller); PNG and JPEG are read too (an install from
//! before November 2026, and a pack picture yet to be converted).

use std::path::{Path, PathBuf};

/// The setting's prefix for a picture from the pack.
pub const PACK: &str = "pack:";

/// The formats a pack picture can be in, the one looked for first first.
pub const FORMATS: [&str; 3] = ["webp", "png", "jpg"];

/// Whether a file's name is one of the pack's pictures (not a gain map).
pub fn is_picture(name: &str) -> bool {
    !name.ends_with(".gain.png") && FORMATS.iter().any(|x| name.strip_suffix(x).is_some_and(|n| n.ends_with('.')))
}

/// `dir/NAME` in the first of the pack's formats that's there.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    FORMATS.iter().map(|x| dir.join(format!("{name}.{x}"))).find(|p| p.is_file())
}

/// One of the pack's pictures.
#[derive(Debug, Clone, PartialEq)]
pub struct Wallpaper {
    pub name: String,
    /// For people: "Dusk".
    pub label: String,
    /// Which collection it's in, for people: "Mac OS 9", "Olo".
    pub collection: String,
    /// It has a version for dark mode.
    pub dark: bool,
    /// A tile (a desktop pattern): repeated across the screen, this many
    /// points on a side (as it was on its system's screen); its file is
    /// `tiles/NAME.webp`, drawn larger than that so it's sharp at any scale.
    pub tile: Option<f32>,
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
            let collection = parts.next().filter(|c| !c.is_empty()).unwrap_or("Olo");
            let tile = parts.find_map(|p| p.strip_prefix("tile:")).and_then(|n| n.parse::<f32>().ok()).filter(|n| *n > 0.0);
            let dark = find(&dir.join("4k"), &format!("{name}-dark")).is_some();
            Some(Wallpaper { name: name.to_owned(), label: label.to_owned(), collection: collection.to_owned(), dark, tile })
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
/// the desktop chosen. A picture the pack still lists is never moved,
/// though its file isn't there: an install that's missing it (one made
/// by a version that copied only some) mustn't cost the person their
/// choice, which shows once the file comes.
pub fn moved(setting: &str) -> Option<String> {
    moved_in(&dir()?, setting)
}

fn moved_in(dir: &Path, setting: &str) -> Option<String> {
    let name = setting.trim().strip_prefix(PACK)?;
    let here = |n: &str| find(&dir.join("4k"), n).or_else(|| find(&dir.join("tiles"), n)).is_some();
    if here(name) {
        return None;
    }
    // (By name alone: `parse` looks at the disk for each.)
    let listed = std::fs::read_to_string(dir.join("pack.txt")).unwrap_or_default();
    if listed.lines().map(str::trim).filter(|l| !l.starts_with('#')).any(|l| l.split('|').next().map(str::trim) == Some(name)) {
        return None;
    }
    let instead = olo_theme::looks::resolve(name).wallpaper;
    here(instead).then(|| format!("{PACK}{instead}"))
}

/// [`resolve`] for a pack setting, the pack in `dir`.
fn resolve_in(dir: &Path, setting: &str, size: (u32, u32), dark: bool) -> Option<PathBuf> {
    let name = setting.strip_prefix(PACK)?;
    if let Some(tile) = find(&dir.join("tiles"), name) {
        return Some(tile);
    }
    let shape = if size.0 as f64 >= size.1.max(1) as f64 * ULTRAWIDE { "ultrawide" } else { "4k" };
    // (In any of the pack's formats; one with no ultrawide version is its
    // 4K one, cropped to the screen as it's shown.)
    let in_dir = |sub: &str, n: &str| find(&dir.join(sub), n);
    let file = |n: &str| in_dir(shape, n).or_else(|| in_dir("4k", n));
    let pick = |name: &str| dark.then(|| file(&format!("{name}-dark"))).flatten().or_else(|| file(name));
    pick(name).or_else(|| pick(olo_theme::looks::resolve(name).wallpaper))
}

/// How many points on a side the tile `setting` names is drawn, if it's
/// one of the pack's tiles (`Wallpaper::tile`).
pub fn tile(setting: &str) -> Option<f32> {
    let name = setting.trim().strip_prefix(PACK)?;
    pack().into_iter().find(|w| w.name == name)?.tile
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
        let pack = parse("# comment\ndusk|Dusk|Mac OS 9\nblue|Blue\nweave|Weave|Mac OS 9|tile:64\n", &dir);
        assert_eq!(
            pack.iter().map(|w| (w.name.as_str(), w.label.as_str(), w.collection.as_str(), w.dark, w.tile)).collect::<Vec<_>>(),
            [("dusk", "Dusk", "Mac OS 9", true, None), ("blue", "Blue", "Olo", false, None), ("weave", "Weave", "Mac OS 9", false, Some(64.0))]
        );
        // A tile is its file in tiles/, at any size, light or dark.
        std::fs::create_dir_all(dir.join("tiles")).unwrap();
        std::fs::write(dir.join("tiles/weave.png"), "png").unwrap();
        assert_eq!(resolve_in(&dir, "pack:weave", (300, 100), true), Some(dir.join("tiles/weave.png")));
        assert_eq!(moved_in(&dir, "pack:weave"), None);
        // A JPEG with no ultrawide version: its 4K one on any screen.
        std::fs::write(dir.join("4k/boing.jpg"), "jpg").unwrap();
        assert_eq!(resolve_in(&dir, "pack:boing", (300, 100), false), Some(dir.join("4k/boing.jpg")));
        assert_eq!(moved_in(&dir, "pack:boing"), None);
        // WebP (the pack's own now), before a PNG or JPEG of the same
        // name; a WebP tile and dark version too.
        std::fs::write(dir.join("4k/boing.webp"), "webp").unwrap();
        assert_eq!(resolve_in(&dir, "pack:boing", (300, 100), false), Some(dir.join("4k/boing.webp")));
        std::fs::write(dir.join("tiles/knit.webp"), "webp").unwrap();
        assert_eq!(resolve_in(&dir, "pack:knit", (300, 100), false), Some(dir.join("tiles/knit.webp")));
        assert_eq!(moved_in(&dir, "pack:knit"), None);
        std::fs::write(dir.join("4k/glass.webp"), "webp").unwrap();
        std::fs::write(dir.join("4k/glass-dark.webp"), "webp").unwrap();
        assert!(parse("glass|Glass\n", &dir)[0].dark);
        assert_eq!(resolve_in(&dir, "pack:glass", (100, 100), true), Some(dir.join("4k/glass-dark.webp")));
        assert!(is_picture("glass.webp") && is_picture("glass.png") && is_picture("boing.jpg"));
        assert!(!is_picture("glass.gain.png") && !is_picture("pack.txt") && !is_picture("xwebp"));
        assert_eq!(pack[0].setting(), "pack:dusk");
        assert_eq!(resolve("/x/mine.jpg", (100, 100), true), Some(PathBuf::from("/x/mine.jpg")));
        assert_eq!(resolve("  ", (100, 100), true), None);
        assert_eq!(resolve_in(&dir, "pack:dusk", (100, 100), true), Some(dir.join("4k/dusk-dark.png")));
        assert_eq!(resolve_in(&dir, "pack:blue", (300, 100), true), Some(dir.join("ultrawide/blue.png")));
        // One that's gone: the theme's it's named for, else the default's.
        let (default, platinum) = (olo_theme::looks::resolve(crate::themes::DEFAULT).wallpaper, olo_theme::looks::resolve("platinum").wallpaper);
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
        // One the pack lists whose file isn't there (an install missing
        // it): kept as chosen, the default drawn meanwhile.
        std::fs::write(dir.join("pack.txt"), "dusk|Dusk|Mac OS 9\nlost|Lost|Mac OS 9\n").unwrap();
        assert_eq!(moved_in(&dir, "pack:lost"), None);
        assert_eq!(resolve_in(&dir, "pack:lost", (100, 100), false), Some(dir.join(format!("4k/{default}.png"))));
        assert_eq!(moved_in(&dir, "pack:toybox"), Some(format!("pack:{default}")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
