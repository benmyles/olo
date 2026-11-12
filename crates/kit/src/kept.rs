//! What the person arranged, kept for the app from one run to the
//! next, as BeOS's apps kept their windows' frames: the size each
//! window that asks was last left at (`WindowOptions::keep_size`; one
//! the compositor chose, a tile or maximized, isn't the person's and
//! isn't kept), and each table's columns ([`TableLayout`]: their order
//! and widths, by the table's id). Noted as they change (in memory: a
//! live resize or a drag writes nothing) and written when a window
//! closes or the program quits (`store`), a file of each for each app
//! in the state folder.

use std::{collections::BTreeMap, path::PathBuf};

use olo_sync::Mutex;

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

static KEPT: Mutex<Kept> = Mutex::new(Kept { read: false, sizes: BTreeMap::new(), dirty: false, tables: BTreeMap::new(), tables_dirty: false });

fn path() -> PathBuf {
    olo_xdg::state_dir().join("windows").join(format!("{}.json", olo_kit_base::keys::app_id()))
}

fn tables_path() -> PathBuf {
    olo_xdg::state_dir().join("tables").join(format!("{}.json", olo_kit_base::keys::app_id()))
}

fn with<R>(f: impl FnOnce(&mut Kept) -> R) -> R {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if !kept.read {
        kept.read = true;
        kept.sizes = olo_kit_base::store::load(&path()).unwrap_or_default();
        kept.tables = olo_kit_base::store::load(&tables_path()).unwrap_or_default();
    }
    f(&mut kept)
}

/// Table `id`'s columns as they were left, if they were arranged.
pub(crate) fn table(id: &str) -> Option<TableLayout> {
    // (The layout lint lays tables out as they're made.)
    if cfg!(feature = "lint") {
        return None;
    }
    with(|k| k.tables.get(id).cloned())
}

/// Table `id`'s columns are arranged so now.
pub(crate) fn note_table(id: &str, layout: TableLayout) {
    if cfg!(feature = "lint") {
        return;
    }
    with(|k| {
        if k.tables.get(id) != Some(&layout) {
            k.tables.insert(id.to_owned(), layout);
            k.tables_dirty = true;
        }
    })
}

/// The size window `name` was last left at, if it was.
pub(crate) fn size(name: &str) -> Option<(f32, f32)> {
    // (The layout lint lays windows out at the sizes they're given, not
    // the ones someone left them at.)
    if cfg!(feature = "lint") {
        return None;
    }
    with(|k| k.sizes.get(name).copied()).filter(|&(w, h)| w >= 1.0 && h >= 1.0)
}

/// Window `name` is this size now (the person's doing, not the
/// compositor's).
pub(crate) fn note_size(name: &str, size: (f32, f32)) {
    if cfg!(feature = "lint") {
        return;
    }
    with(|k| {
        if k.sizes.get(name) != Some(&size) {
            k.sizes.insert(name.to_owned(), size);
            k.dirty = true;
        }
    })
}

/// What was noted since it was last written, written (off the calling
/// thread: `store`).
pub(crate) fn write() {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if std::mem::take(&mut kept.dirty) {
        olo_kit_base::store::save(path(), &kept.sizes);
    }
    if std::mem::take(&mut kept.tables_dirty) {
        olo_kit_base::store::save(tables_path(), &kept.tables);
    }
}
