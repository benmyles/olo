//! Windows that open at the size they were last left at
//! ([`crate::WindowOptions::keep_size`]), as BeOS's apps kept their
//! windows' frames: each one's size noted as it changes (in memory: a
//! live resize writes nothing), and the app's sizes written when one of
//! its windows closes or it quits (`store`), a file for each app in the
//! state folder. A size the compositor chose (a tile, maximized, full
//! screen) isn't the person's, and isn't kept. So too each table's
//! columns as the person arranged them ([`TableLayout`]: their order and
//! widths, by the table's id), a file for each app beside the sizes.

use std::{collections::BTreeMap, path::PathBuf};

use gpui::{App, Global, Window};

#[derive(Default)]
struct Kept {
    /// Read from disk yet.
    read: bool,
    sizes: BTreeMap<String, (f32, f32)>,
    /// Changed since written.
    dirty: bool,
    tables: BTreeMap<String, TableLayout>,
    tables_dirty: bool,
}

/// A table's columns as the person left them: by their titles, so a
/// column added or taken away since leaves the others as they were.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct TableLayout {
    /// Left to right.
    pub order: Vec<String>,
    /// Those dragged to another width.
    pub widths: BTreeMap<String, f32>,
}

impl Global for Kept {}

fn path(cx: &App) -> PathBuf {
    olo_xdg::state_dir().join("windows").join(format!("{}.json", crate::app_id(cx)))
}

fn tables_path(cx: &App) -> PathBuf {
    olo_xdg::state_dir().join("tables").join(format!("{}.json", crate::app_id(cx)))
}

fn kept(cx: &mut App) -> &mut Kept {
    if !cx.default_global::<Kept>().read {
        let sizes = olo_kit_base::store::load(&path(cx)).unwrap_or_default();
        let tables = olo_kit_base::store::load(&tables_path(cx)).unwrap_or_default();
        let k = cx.global_mut::<Kept>();
        (k.read, k.sizes, k.tables) = (true, sizes, tables);
    }
    cx.global_mut::<Kept>()
}

/// Table `id`'s columns as they were left, if they were arranged.
pub(crate) fn table(cx: &mut App, id: &str) -> Option<TableLayout> {
    // (The layout lint lays tables out as they're made.)
    if cfg!(feature = "lint") {
        return None;
    }
    kept(cx).tables.get(id).cloned()
}

/// Table `id`'s columns are arranged so now (written with the sizes).
pub(crate) fn note_table(cx: &mut App, id: &str, layout: TableLayout) {
    if cfg!(feature = "lint") {
        return;
    }
    let k = kept(cx);
    if k.tables.get(id) != Some(&layout) {
        k.tables.insert(id.to_owned(), layout);
        k.tables_dirty = true;
    }
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
    let (path, tables_path) = (path(cx), tables_path(cx));
    if let Some(k) = cx.try_global::<Kept>().is_some().then(|| cx.global_mut::<Kept>()) {
        if std::mem::take(&mut k.dirty) {
            olo_kit_base::store::save(path, &k.sizes);
        }
        if std::mem::take(&mut k.tables_dirty) {
            olo_kit_base::store::save(tables_path, &k.tables);
        }
    }
}
