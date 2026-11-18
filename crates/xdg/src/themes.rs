//! Icon sets: whole looks for Olo's icons (`olo_theme::looks::ICON_SETS`).
//! Each theme has one (Preferences ▸ Themes; the person can give a theme
//! another, Customize…: `Appearance::icons`). The classic set is the
//! drawings in `<data>/icons/svg` themselves; another lives in
//! `<data>/themes/<id>/icons` (`svg/<name>.svg`, its own drawings, and
//! `recolour.txt`, its colours for the classic ones) and overlays the
//! classic set, which fills in anything it lacks, in the set's colours
//! ([`recolour`]). The wordmark is the brand's, the same in every set
//! ([`wordmark_file`]).
//!
//! Each process draws in one set at a time, [`current`]: the kit and
//! app_server [`set`] it from the settings and drop what they drew in the
//! old one. The themes themselves, and their colours, are
//! `olo_theme::looks`.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

pub use olo_theme::looks::{CLASSIC, DEFAULT, ICON_SETS, IconSet};

const UNSET: usize = usize::MAX;
static CURRENT: AtomicUsize = AtomicUsize::new(UNSET);

/// Icon set `id`, or the classic one if there is none by that name.
pub fn resolve(id: &str) -> &'static IconSet {
    olo_theme::looks::icon_set(id)
}

/// Set `id`'s place in `ICON_SETS`, by id: `ICON_SETS` is a const, so
/// the set `resolve` returns needn't be at the address of this crate's
/// copy.
fn index(id: &str) -> usize {
    let id = resolve(id).id;
    ICON_SETS.iter().position(|x| x.id == id).unwrap_or(0)
}

/// The icon set this process draws in: the last [`set`], else
/// `$OLO_ICON_THEME` (for tests and screenshots outside Olo), else the
/// classic one.
pub fn current() -> &'static IconSet {
    let mut i = CURRENT.load(Ordering::Relaxed);
    if i == UNSET {
        i = index(&std::env::var("OLO_ICON_THEME").unwrap_or_default());
        // A `set` that got there first wins.
        i = match CURRENT.compare_exchange(UNSET, i, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => i,
            Err(now) => now,
        };
    }
    &ICON_SETS[i]
}

/// Draw in icon set `id` (the classic one if unknown) from now on. True
/// if that changed it: the caller then drops what it drew in the old one.
pub fn set(id: &str) -> bool {
    let before = current().id;
    let i = index(id);
    CURRENT.store(i, Ordering::Relaxed);
    before != ICON_SETS[i].id
}

/// Theme `theme`'s own wallpaper, as the wallpaper setting names it: the
/// pack picture the theme names (`olo_theme::looks::Theme::wallpaper`).
/// Choosing a theme in Preferences switches to it (or to the one the
/// person gave the theme: `Appearance::wallpaper_of`).
pub fn wallpaper(theme: &str) -> String {
    format!("{}{}", crate::wallpapers::PACK, olo_theme::looks::resolve(theme).wallpaper)
}

/// The directories icon set `id` overlays on the classic set, most
/// important first (none for the classic set itself).
fn overlays(id: &str) -> impl Iterator<Item = PathBuf> {
    let id = resolve(id).id;
    crate::data_dirs().iter().filter(move |_| id != CLASSIC).map(move |d| d.join("themes").join(id))
}

/// Where icon set `id`'s icons are (`svg/<name>.svg`) and then the
/// classic set's, which fills in for it.
pub(crate) fn icon_dirs(id: &str) -> impl Iterator<Item = PathBuf> {
    overlays(id).map(|d| d.join("icons")).chain(crate::data_dirs().iter().map(|d| d.join("icons")))
}

/// How icon set `id` colours the classic set's drawings where it has
/// none of its own (`themes/<id>/icons/recolour.txt`: `svg::recolour`),
/// read once; none for the classic set, or a set without rules.
pub fn recolour(id: &str) -> Option<&'static crate::svg::Recolour> {
    static RULES: std::sync::OnceLock<Vec<Option<crate::svg::Recolour>>> = std::sync::OnceLock::new();
    RULES
        .get_or_init(|| {
            ICON_SETS
                .iter()
                .map(|t| {
                    let file = overlays(t.id).map(|d| d.join("icons/recolour.txt")).find(|f| f.is_file())?;
                    match crate::svg::Recolour::parse(&std::fs::read_to_string(&file).ok()?) {
                        Ok(rules) => Some(rules),
                        Err(e) => {
                            tracing::warn!("{}: {e}", file.display());
                            None
                        }
                    }
                })
                .collect()
        })
        .get(index(id))?
        .as_ref()
}

/// Whether `path` (an icon's) is icon set `id`'s own, not the classic
/// set's filling in for it.
pub(crate) fn is_its_own(id: &str, path: &std::path::Path) -> bool {
    let id = resolve(id).id;
    id != CLASSIC && path.ancestors().nth(3).is_some_and(|theme| theme.ends_with(std::path::Path::new("themes").join(id)))
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

    /// The classic set is the drawings (`icons/svg`); no theme, and not
    /// the classic set, keeps pictures of an icon at sizes any more, or a
    /// wordmark of its own (the brand's is the same in every theme).
    #[test]
    fn icons_are_drawings() {
        let data = repo_data();
        let drawn = std::fs::read_dir(data.join("icons/svg")).unwrap().flatten().filter(|f| f.path().extension().is_some_and(|e| e == "svg")).count();
        assert!(drawn >= 181, "{drawn} drawings");
        let mut sets = vec![data.join("icons")];
        sets.extend(ICON_SETS.iter().filter(|t| t.id != CLASSIC).map(|t| data.join("themes").join(t.id).join("icons")));
        for set in sets {
            for px in crate::ICON_SIZES {
                assert!(!set.join(px.to_string()).exists(), "{}/{px}: pictures of icons at a size", set.display());
            }
        }
        for t in ICON_SETS.iter().filter(|t| t.id != CLASSIC) {
            assert!(!data.join("themes").join(t.id).join("brand").exists(), "{} has a wordmark of its own", t.id);
            // A set other than the classic one has something of its own.
            assert!(data.join("themes").join(t.id).join("icons").is_dir(), "{} has no icons of its own", t.id);
            // Every icon a set is asked for is a drawing: its own, or the classic one.
            for name in ["folder", "olo", "themes", "phone-iphone-17", "weather-rain"] {
                assert!(crate::own_icon(t.id, name).is_some(), "{}: no {name}", t.id);
            }
        }
    }

    /// A theme's rules for the classic set's drawings can be read, and
    /// Platinum has them: where it has no drawing of its own, the classic
    /// one shows, in its colours.
    #[test]
    fn a_theme_colours_the_classic_drawings() {
        let data = repo_data();
        for t in ICON_SETS.iter().filter(|t| t.id != CLASSIC) {
            let file = data.join("themes").join(t.id).join("icons/recolour.txt");
            if let Ok(text) = std::fs::read_to_string(&file) {
                crate::svg::Recolour::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            }
        }
        assert!(recolour(CLASSIC).is_none());
        let rules = recolour("platinum").expect("Platinum's rules");
        let folder = crate::find_icon_in("platinum", "folder", 32).unwrap();
        assert!(crate::svg::is_own(&folder) && !is_its_own("platinum", &folder), "{folder:?}");
        assert!(is_its_own("platinum", &data.join("themes/platinum/icons/svg/folder.svg")));
        let classic = std::fs::read_to_string(&folder).unwrap();
        let platinum = rules.apply("folder", &classic);
        // The folder's face, at the middle of the icon: manila in Classic,
        // lilac blue in Platinum.
        let face = |svg: &str| {
            let img = crate::svg::render(svg.as_bytes(), 64, true).unwrap();
            let i = (40 * 64 + 32) * 4;
            [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
        };
        let (c, p) = (face(&classic), face(&platinum));
        assert!(c[0] > c[2] + 40, "Classic's folder is manila: {c:?}");
        assert!(p[2] > p[0] + 20, "Platinum's is lilac blue: {p:?}");
        // The brand's ring is the brand's in every theme.
        let ring = std::fs::read_to_string(data.join("icons/svg/olo.svg")).unwrap();
        assert_eq!(rules.apply("olo", &ring), ring);
    }

    #[test]
    fn setting_an_icon_set_draws_in_it() {
        // Each in turn, whichever came before (see `index`).
        for t in ICON_SETS.iter().chain(ICON_SETS.iter().rev()) {
            set(t.id);
            assert_eq!(current().id, t.id);
            assert!(!set(t.id), "{} again changed it", t.id);
        }
        // One there isn't: the classic set.
        set("no-such-set");
        assert_eq!(current().id, CLASSIC);
    }

    #[test]
    fn every_theme_has_its_wallpaper() {
        // In the pack, at each shape, its dark version too where it has
        // one; a new install starts with the default theme's (Classic's:
        // the dunes; the settings start with it too: Preferences' themes
        // test).
        let dir = repo_data().join("wallpapers");
        let pack = crate::wallpapers::parse(&std::fs::read_to_string(dir.join("pack.txt")).unwrap(), &dir);
        assert_eq!(wallpaper(DEFAULT), "pack:olo-dunes");
        assert_eq!(crate::wallpapers::default_setting(), wallpaper(DEFAULT));
        for t in olo_theme::looks::THEMES {
            let w = pack.iter().find(|w| w.setting() == wallpaper(t.id)).unwrap_or_else(|| panic!("{} has no wallpaper", t.id));
            assert!(w.tile.is_none(), "{}'s wallpaper is a tile", t.id);
            let mut names = vec![t.wallpaper.to_owned()];
            if w.dark {
                names.push(format!("{}-dark", t.wallpaper));
            }
            for name in names {
                // The pictures lossless WebP, their previews PNG. (One
                // with no ultrawide version is its 4K one, cropped to the
                // screen as it's shown: `wallpapers::resolve`.)
                assert_eq!(webp_size(&dir.join("4k").join(format!("{name}.webp"))), Some((3840, 2160)), "4k/{name}");
                let wide = dir.join("ultrawide").join(format!("{name}.webp"));
                if wide.exists() {
                    assert_eq!(webp_size(&wide), Some((3840, 1648)), "ultrawide/{name}");
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
}
