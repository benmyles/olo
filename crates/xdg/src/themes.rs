//! Themes: whole looks for Olo's icons and wordmark, chosen in
//! Preferences ▸ Themes (`Appearance::icon_theme`). A theme lives in
//! `<data>/themes/<id>/` (`icons/<px>/<name>.png` like `<data>/icons`,
//! and `brand/olo-wordmark.png`) and overlays the classic set in
//! `<data>/icons` and `<data>/brand`, which fills in anything it lacks.
//!
//! Each process draws in one theme at a time, [`current`]: the kit and
//! app_server [`set`] it from the settings and drop what they drew in the
//! old one.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A theme: its id (its directory, and what the setting holds), its
/// name, and a line about it for Preferences.
#[derive(Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
}

/// Every theme, in the order Preferences shows them.
pub const THEMES: &[Theme] = &[
    Theme { id: "bebox", name: "BeBox", about: "Graphite and blinking lights." },
    Theme { id: "soft-be", name: "Soft Be", about: "Soft matte clay on the Be desk." },
    Theme { id: "toybox", name: "Toybox", about: "Painted wooden toys." },
    Theme { id: "papercraft", name: "Papercraft", about: "Folded card, one tone a face." },
    Theme { id: "enamel", name: "Enamel", about: "Enamel pins in polished gold." },
    Theme { id: "halftone", name: "Halftone", about: "Flat ink and halftone dots." },
    Theme { id: CLASSIC, name: "Classic", about: "Olo's first set: glossy, bright." },
];

/// The theme a new user gets (and an unknown setting falls back to).
pub const DEFAULT: &str = "bebox";

/// The set in `<data>/icons` and `<data>/brand` itself, with no overlay.
pub const CLASSIC: &str = "classic";

/// Theme `id`, if there is one.
pub fn get(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id == id)
}

/// Theme `id`, or the default if there is none by that name.
pub fn resolve(id: &str) -> &'static Theme {
    get(id).or_else(|| get(DEFAULT)).expect("the default theme")
}

const UNSET: usize = usize::MAX;
static CURRENT: AtomicUsize = AtomicUsize::new(UNSET);

fn index(id: &str) -> usize {
    let t = resolve(id);
    THEMES.iter().position(|x| std::ptr::eq(x, t)).unwrap_or(0)
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
    let before = current();
    let i = index(id);
    CURRENT.store(i, Ordering::Relaxed);
    !std::ptr::eq(before, &THEMES[i])
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

/// Theme `id`'s wordmark for a light or a `dark` background. A theme
/// can have one file for both (`brand/olo-wordmark.png`) or a separate
/// `olo-wordmark-on-dark.png`.
pub fn wordmark_file(id: &str, dark: bool) -> Option<PathBuf> {
    let (light, on_dark) = ("brand/olo-wordmark.png", "brand/olo-wordmark-on-dark.png");
    let theirs = overlays(id).flat_map(|d| if dark { vec![d.join(on_dark), d.join(light)] } else { vec![d.join(light)] });
    let base = crate::data_dirs().iter().map(|d| d.join(if dark { on_dark } else { light }));
    theirs.chain(base).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn repo_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn manifest() -> Vec<String> {
        std::fs::read_to_string(repo_data().join("icons/manifest.txt"))
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.split('|').next().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn ids_are_unique_and_known() {
        for (i, t) in THEMES.iter().enumerate() {
            assert!(THEMES[..i].iter().all(|u| u.id != t.id), "{} twice", t.id);
        }
        assert!(get(DEFAULT).is_some() && get(CLASSIC).is_some());
        assert_eq!(resolve("no-such-theme").id, DEFAULT);
    }

    #[test]
    fn every_theme_is_complete() {
        // Each theme draws every icon in the manifest at every size, and
        // its own wordmark: none falls back to the classic set.
        let data = repo_data();
        let names = manifest();
        for t in THEMES.iter().filter(|t| t.id != CLASSIC) {
            let dir = data.join("themes").join(t.id);
            let missing: Vec<String> = crate::OWN_SIZES
                .iter()
                .flat_map(|px| names.iter().map(move |n| format!("{px}/{n}")))
                .filter(|p| !dir.join("icons").join(format!("{p}.png")).is_file())
                .collect();
            assert!(missing.is_empty(), "{} lacks {missing:?}", t.id);
            let img = crate::decode_png(&dir.join("brand/olo-wordmark.png")).unwrap_or_else(|| panic!("{}'s wordmark", t.id));
            assert!(img.width > img.height && img.height >= 128);
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
