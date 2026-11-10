//! What the person arranged, kept for the app from one run to the
//! next, as BeOS's apps kept their windows' frames: each table's
//! columns ([`TableLayout`]: their order and widths, by the table's id),
//! noted as they change (in memory: a drag writes nothing) and written
//! when a window closes or the program quits (`store`), a file for each
//! app in the state folder.

use std::{collections::BTreeMap, path::PathBuf};

use olo_sync::Mutex;

#[derive(Default)]
struct Kept {
    /// Read from disk yet.
    read: bool,
    tables: BTreeMap<String, TableLayout>,
    /// Changed since written.
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

static KEPT: Mutex<Kept> = Mutex::new(Kept { read: false, tables: BTreeMap::new(), tables_dirty: false });

fn tables_path() -> PathBuf {
    olo_xdg::state_dir().join("tables").join(format!("{}.json", olo_kit_base::keys::app_id()))
}

fn with<R>(f: impl FnOnce(&mut Kept) -> R) -> R {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if !kept.read {
        kept.read = true;
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

/// What was noted since it was last written, written (off the calling
/// thread: `store`).
pub(crate) fn write() {
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if std::mem::take(&mut kept.tables_dirty) {
        olo_kit_base::store::save(tables_path(), &kept.tables);
    }
}
