//! The order a folder's items were put in by hand (a column clicked, a
//! Sort By chosen), kept for that folder: it opens in it again, in any
//! window, while every other folder keeps Olo's own (`model::Order`:
//! folders by name, then the files newest first). BeOS kept a window's
//! columns in the folder's attributes; folders the person can't write
//! (the system's) are sorted too, so these are in a file of Tracker's
//! own in the state folder (`store`, written off the UI thread).

use std::{collections::BTreeMap, path::Path};

use olo_ui::gpui::{App, Global};

use crate::model::{Order, SortKey};

#[derive(Default)]
struct Sorts {
    read: bool,
    /// By folder.
    chosen: BTreeMap<String, (SortKey, bool)>,
}

impl Global for Sorts {}

fn file() -> std::path::PathBuf {
    olo_xdg::state_dir().join("tracker").join("sorts.json")
}

fn sorts(cx: &mut App) -> &mut Sorts {
    if !cx.default_global::<Sorts>().read {
        let chosen = olo_ui::store::load(&file()).unwrap_or_default();
        let s = cx.global_mut::<Sorts>();
        (s.read, s.chosen) = (true, chosen);
    }
    cx.global_mut::<Sorts>()
}

/// The order folder `dir` was last put in by hand (None: Olo's own).
pub fn of(cx: &mut App, dir: &Path) -> Order {
    sorts(cx).chosen.get(&*dir.to_string_lossy()).copied()
}

/// Folder `dir` was put in `order` by hand (None: back to Olo's own).
pub fn remember(cx: &mut App, dir: &Path, order: Order) {
    let key = dir.to_string_lossy().into_owned();
    let s = sorts(cx);
    let changed = match order {
        Some(o) => s.chosen.insert(key, o) != Some(o),
        None => s.chosen.remove(&key).is_some(),
    };
    if changed {
        olo_ui::store::save(file(), &s.chosen);
    }
}
