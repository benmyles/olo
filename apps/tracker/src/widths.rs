//! How wide a folder's column is in column view, where the person sized
//! it, kept for that folder: it shows that wide again, in any window,
//! while every other folder's column is as wide as Tracker's setting says
//! (`tracker.column_width`). Each column sized on its own (Ben,
//! 2026-11-25: "change how the column resizing works so that columns can
//! be independently resized and don't all change as one and remember the
//! size for specific folders where their column width is resized"; it had
//! been one width for every column). Kept as `sorts` keeps a folder's
//! order, in a file of Tracker's own in the state folder (`store`, written
//! off the UI thread).

use std::{collections::BTreeMap, path::Path};

use olo_sync::Mutex;

#[derive(Default)]
struct Widths {
    read: bool,
    /// By folder, in points.
    chosen: BTreeMap<String, u16>,
}

/// (Every window's: each on its own looper.)
static WIDTHS: Mutex<Option<Widths>> = Mutex::new(None);

fn file() -> std::path::PathBuf {
    olo_xdg::state_dir().join("tracker").join("column-widths.json")
}

fn with<R>(f: impl FnOnce(&mut Widths) -> R) -> R {
    let mut w = WIDTHS.lock().unwrap_or_else(|e| e.into_inner());
    let w = w.get_or_insert_with(Widths::default);
    if !w.read {
        (w.read, w.chosen) = (true, olo_kit::store::load(&file()).unwrap_or_default());
    }
    f(w)
}

/// The width folder `dir`'s column was sized to (None: Tracker's setting).
pub fn of(dir: &Path) -> Option<u16> {
    with(|w| w.chosen.get(&*dir.to_string_lossy()).copied())
}

/// Folder `dir`'s column is `points` wide (None: back to Tracker's
/// setting); `keep`: written to the file (a drag's frames aren't, only
/// where it is let go). Whether that is a change.
pub fn set(dir: &Path, points: Option<u16>, keep: bool) -> bool {
    let key = dir.to_string_lossy().into_owned();
    with(|w| {
        let changed = match points {
            Some(p) => w.chosen.insert(key, p) != Some(p),
            None => w.chosen.remove(&key).is_some(),
        };
        if keep {
            olo_kit::store::save(file(), &w.chosen);
        }
        changed
    })
}
