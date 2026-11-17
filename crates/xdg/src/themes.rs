//! Themes: whole looks for Olo's icons, chosen in Preferences ▸ Themes
//! (`Appearance::icon_theme`). A theme lives in `<data>/themes/<id>/`
//! (`icons/<px>/<name>.png` like `<data>/icons`) and overlays the classic
//! set in `<data>/icons`, which fills in anything it lacks. The wordmark
//! is the brand's, the same in every theme ([`wordmark_file`]).
//!
//! Each process draws in one theme at a time, [`current`]: the kit and
//! app_server [`set`] it from the settings and drop what they drew in the
//! old one. The themes themselves, and their colours, are
//! `olo_theme::looks`.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

pub use olo_theme::looks::{CLASSIC, DEFAULT, THEMES, Theme, get, resolve};

const UNSET: usize = usize::MAX;
static CURRENT: AtomicUsize = AtomicUsize::new(UNSET);

/// Theme `id`'s place in `THEMES`, by id: `THEMES` is a const, so the
/// theme `resolve` returns needn't be at the address of this crate's copy.
fn index(id: &str) -> usize {
    let id = resolve(id).id;
    THEMES.iter().position(|x| x.id == id).unwrap_or(0)
}

/// The theme this process draws in: the last [`set`], else
/// `$OLO_ICON_THEME` (for tests and screenshots outside Olo), else the
/// default.
pub fn current() -> &'static Theme {
    let mut i = CURRENT.load(Ordering::Relaxed);
    if i == UNSET {
        i = index(&std::env::var("OLO_ICON_THEME").unwrap_or_default());
        // A `set` that got there first wins.
        i = match CURRENT.compare_exchange(UNSET, i, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => i,
            Err(now) => now,
        };
    }
    &THEMES[i]
}

/// Draw in theme `id` (the default if unknown) from now on. True if that
/// changed it: the caller then drops what it drew in the old one.
pub fn set(id: &str) -> bool {
    let before = current().id;
    let i = index(id);
    CURRENT.store(i, Ordering::Relaxed);
    before != THEMES[i].id
}

/// Theme `id`'s own wallpaper, as the wallpaper setting names it: the
/// pack picture the theme names (`Theme::wallpaper`: one of Mac OS 9's
/// desktop pictures). Choosing a theme in Preferences switches to it.
pub fn wallpaper(id: &str) -> String {
    format!("{}{}", crate::wallpapers::PACK, resolve(id).wallpaper)
}

/// The directories theme `id` overlays on the classic set, most
/// important first (none for the classic set itself).
fn overlays(id: &str) -> impl Iterator<Item = PathBuf> {
    let id = resolve(id).id;
    crate::data_dirs().iter().filter(move |_| id != CLASSIC).map(move |d| d.join("themes").join(id))
}

/// Where theme `id`'s icons are (`<px>/<name>.png`) and then the classic
/// set's, which fills in for it.
pub(crate) fn icon_dirs(id: &str) -> impl Iterator<Item = PathBuf> {
    overlays(id).map(|d| d.join("icons")).chain(crate::data_dirs().iter().map(|d| d.join("icons")))
}

/// Olo's wordmark for a light or a `dark` background
/// (`brand/olo-wordmark.png`, `brand/olo-wordmark-on-dark.png`): the
/// brand's lettering, the same in every theme. (Themes once had their
/// own, in `themes/<id>/brand`; one left installed there is never read.)
pub fn wordmark_file(dark: bool) -> Option<PathBuf> {
    let name = if dark { "brand/olo-wordmark-on-dark.png" } else { "brand/olo-wordmark.png" };
    crate::data_dirs().iter().map(|d| d.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn repo_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn manifest() -> Vec<String> {
        std::fs::read_to_string(repo_data().join("icons/manifest.txt")).unwrap().lines().filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| l.split('|').next().unwrap().to_owned()).collect()
    }

    #[test]
    fn every_theme_is_complete() {
        // Each theme draws every icon in the manifest at every size: none
        // falls back to the classic set. (The wordmark is the brand's, the
        // same in every theme: none has its own.)
        let data = repo_data();
        let names = manifest();
        for t in THEMES.iter().filter(|t| t.id != CLASSIC) {
            let dir = data.join("themes").join(t.id);
            let missing: Vec<String> =
                crate::OWN_SIZES.iter().flat_map(|px| names.iter().map(move |n| format!("{px}/{n}"))).filter(|p| !dir.join("icons").join(format!("{p}.png")).is_file()).collect();
            assert!(missing.is_empty(), "{} lacks {missing:?}", t.id);
            assert!(!dir.join("brand").exists(), "{} has a wordmark of its own", t.id);
        }
    }

    #[test]
    fn setting_a_theme_draws_in_it() {
        // Each in turn, whichever came before (see `index`).
        for t in THEMES.iter().chain(THEMES.iter().rev()) {
            set(t.id);
            assert_eq!(current().id, t.id);
            assert!(!set(t.id), "{} again changed it", t.id);
        }
    }

    #[test]
    fn every_theme_has_its_wallpaper() {
        // In the pack, light and dark, at each shape; the default theme's
        // is the pack's first, so a new install starts with it.
        let dir = repo_data().join("wallpapers");
        let pack = crate::wallpapers::parse(&std::fs::read_to_string(dir.join("pack.txt")).unwrap(), &dir);
        assert_eq!(pack[0].setting(), wallpaper(DEFAULT));
        assert_eq!(crate::wallpapers::default_setting(), wallpaper(DEFAULT));
        for t in THEMES {
            let w = pack.iter().find(|w| w.setting() == wallpaper(t.id)).unwrap_or_else(|| panic!("{} has no wallpaper", t.id));
            assert!(w.dark, "{} has no dark wallpaper", t.id);
            for name in [t.wallpaper.to_owned(), format!("{}-dark", t.wallpaper)] {
                // The pictures lossless WebP, their previews PNG.
                for (shape, size) in [("4k", (3840, 2160)), ("ultrawide", (3840, 1648))] {
                    assert_eq!(webp_size(&dir.join(shape).join(format!("{name}.webp"))), Some(size), "{shape}/{name}");
                }
                let img = crate::decode_png(&dir.join("thumbs").join(format!("{name}.png"))).unwrap_or_else(|| panic!("thumbs/{name}"));
                assert_eq!((img.width, img.height), (320, 180), "thumbs/{name}");
            }
        }
    }

    /// A lossless WebP's size, from its header (`VP8L`: 14 bits each,
    /// less one).
    fn webp_size(p: &Path) -> Option<(u32, u32)> {
        let b = std::fs::read(p).ok()?;
        if b.len() < 25 || &b[..4] != b"RIFF" || &b[8..16] != b"WEBPVP8L" || b[20] != 0x2f {
            return None;
        }
        let v = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
        Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
    }

    #[test]
    fn the_pack_is_lossless_webp() {
        // Every picture and tile the pack lists (restored, and kept
        // without loss: tools/restorewallpaper.py), none left as the PNG
        // or JPEG it was.
        let dir = repo_data().join("wallpapers");
        let pack = crate::wallpapers::parse(&std::fs::read_to_string(dir.join("pack.txt")).unwrap(), &dir);
        assert!(pack.len() > 500);
        for w in &pack {
            let sub = if w.tile.is_some() { "tiles" } else { "4k" };
            assert!(webp_size(&dir.join(sub).join(format!("{}.webp", w.name))).is_some(), "{sub}/{}", w.name);
            for old in ["png", "jpg"] {
                assert!(!dir.join(sub).join(format!("{}.{old}", w.name)).exists(), "{sub}/{}.{old} is still there", w.name);
            }
        }
    }

    #[test]
    fn the_classic_set_is_complete() {
        let icons = repo_data().join("icons");
        for px in crate::OWN_SIZES {
            for n in manifest() {
                assert!(icons.join(format!("{px}/{n}.png")).is_file(), "classic lacks {px}/{n}");
            }
        }
    }
}
