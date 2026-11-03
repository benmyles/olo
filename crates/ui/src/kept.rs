//! Windows that open at the size they were last left at
//! ([`crate::WindowOptions::keep_size`]), as BeOS's apps kept their
//! windows' frames: each one's size noted as it changes (in memory: a
//! live resize writes nothing), and the app's sizes written when one of
//! its windows closes or it quits (`store`), a file for each app in the
//! state folder. A size the compositor chose (a tile, maximized, full
//! screen) isn't the person's, and isn't kept.

use std::{collections::BTreeMap, path::PathBuf};

use gpui::{App, Global, Window};

#[derive(Default)]
struct Kept {
    /// Read from disk yet.
    read: bool,
    sizes: BTreeMap<String, (f32, f32)>,
    /// Changed since written.
    dirty: bool,
}

impl Global for Kept {}

fn path(cx: &App) -> PathBuf {
    olo_xdg::state_dir().join("windows").join(format!("{}.json", crate::app_id(cx)))
}

fn kept(cx: &mut App) -> &mut Kept {
    if !cx.default_global::<Kept>().read {
        let sizes = olo_kit_base::store::load(&path(cx)).unwrap_or_default();
        let k = cx.global_mut::<Kept>();
        (k.read, k.sizes) = (true, sizes);
    }
    cx.global_mut::<Kept>()
}

/// The size window `name` was last left at, if it was.
pub(crate) fn size(cx: &mut App, name: &str) -> Option<(f32, f32)> {
    // (The layout lint lays windows out at the sizes they're given, not
    // the ones someone left them at.)
    if cfg!(feature = "lint") {
        return None;
    }
    kept(cx).sizes.get(name).copied().filter(|&(w, h)| w >= 1.0 && h >= 1.0)
}

/// Window `name` is `window`'s size now (unless the compositor chose it).
pub(crate) fn note(cx: &mut App, name: &str, window: &Window) {
    if cfg!(feature = "lint") || window.tiling().is_tiled() || window.is_maximized() || window.is_fullscreen() {
        return;
    }
    let s = window.viewport_size();
    let size = (f32::from(s.width).round(), f32::from(s.height).round());
    let k = kept(cx);
    if k.sizes.get(name) != Some(&size) {
        k.sizes.insert(name.to_owned(), size);
        k.dirty = true;
    }
}

/// The sizes noted since they were last written, written (off the UI
/// thread).
pub(crate) fn write(cx: &mut App) {
    let path = path(cx);
    if let Some(k) = cx.try_global::<Kept>().is_some().then(|| cx.global_mut::<Kept>())
        && std::mem::take(&mut k.dirty)
    {
        olo_kit_base::store::save(path, &k.sizes);
    }
}
