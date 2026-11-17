//! Go to File or Folder (Go menu, ⇧⌘G, as Finder's Go to Folder): the
//! kit's window (`olo_kit::go_to`, which the file panels open too), with
//! Tracker's own names in its list (its icons, a backed-up file's mark, a
//! link's slant). Go: a folder opens, a file shows selected in its
//! folder's window.

use std::path::{Path, PathBuf};

use olo_kit::{Painter, WindowHandle, go_to, widgets::list::Cell};

use crate::{
    ToHub,
    folder::name_cell,
    fs::{self, Entry},
    to_hub,
};

pub use go_to::parse;

/// A folder's names as Tracker shows them.
pub struct Names;

impl go_to::Names for Names {
    type Entry = Entry;

    fn list(dir: &Path) -> Vec<Entry> {
        fs::list(dir, true).unwrap_or_default()
    }

    fn name(e: &Entry) -> &str {
        &e.name
    }

    fn is_dir(e: &Entry) -> bool {
        e.is_dir
    }

    fn cell(cell: &mut Cell, g: &mut Painter, e: &Entry) {
        name_cell(cell, g, e, true);
    }

    fn key(e: &Entry) -> u64 {
        crate::folder::row_key(e)
    }
}

pub type GoToWin = go_to::GoTo<Names>;

/// Opens it, for the window whose folder is `base`.
pub fn open(base: PathBuf) -> WindowHandle<GoToWin> {
    go_to::open::<Names>(base, "", |path| to_hub(ToHub::Reveal(path, false)))
}
