//! The order a folder's items were put in by hand (a column clicked, a
//! Sort By chosen), kept for that folder: it opens in it again, in any
//! window, while every other folder keeps Olo's own (`model::Order`:
//! folders by name, then the files newest first). BeOS kept a window's
//! columns in the folder's attributes; folders the person can't write
//! (the system's) are sorted too, so these are in a file of Tracker's
//! own in the state folder (`store`, written off the UI thread).

use std::{collections::BTreeMap, path::Path};

use olo_sync::Mutex;

use crate::model::{Order, SortKey};

#[derive(Default)]
struct Sorts {
    read: bool,
    /// By folder.
    chosen: BTreeMap<String, (SortKey, bool)>,
}

/// (Every window's: each on its own looper.)
static SORTS: Mutex<Option<Sorts>> = Mutex::new(None);

fn file() -> std::path::PathBuf {
    olo_xdg::state_dir().join("tracker").join("sorts.json")
}

fn with<R>(f: impl FnOnce(&mut Sorts) -> R) -> R {
    let mut s = SORTS.lock().unwrap_or_else(|e| e.into_inner());
    let s = s.get_or_insert_with(Sorts::default);
    if !s.read {
        (s.read, s.chosen) = (true, olo_kit::store::load(&file()).unwrap_or_default());
    }
    f(s)
}

/// The order folder `dir` was last put in by hand (None: Olo's own).
pub fn of(dir: &Path) -> Order {
    with(|s| s.chosen.get(&*dir.to_string_lossy()).copied())
}

/// Folder `dir` was put in `order` by hand (None: back to Olo's own).
pub fn remember(dir: &Path, order: Order) {
    let key = dir.to_string_lossy().into_owned();
    with(|s| {
        let changed = match order {
            Some(o) => s.chosen.insert(key, o) != Some(o),
            None => s.chosen.remove(&key).is_some(),
        };
        if changed {
            olo_kit::store::save(file(), &s.chosen);
        }
    })
}
