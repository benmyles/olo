//! Previews of every common file format, decoded natively and in process
//! on worker threads: images, SVG, PDF, text and code (coloured by
//! syntax), Markdown and office documents, spreadsheets and CSV, archives
//! and folders, audio (with playback) and video posters, fonts; anything
//! else gets a card. Tracker's Quick Look and the Viewer app are built on
//! them: the decoding here; `ui` (feature `gpui`) draws it with Olo's
//! GPUI.

pub mod archive;
pub mod audio;
pub mod bitmap;
pub mod doc;
pub mod edit;
pub mod input;
pub mod load;
pub mod media;
pub mod opus;
pub mod pdf;
pub mod table;
pub mod text;
#[cfg(feature = "gpui")]
pub mod ui;
pub mod video;

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub use bitmap::Bitmap;
pub use load::{Body, Update};
use olo_kit_base::files::{self, Kind};

/// Show "loading" only if loading takes longer than this.
pub const SLOW: Duration = Duration::from_millis(120);

/// The file being previewed.
#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub kind: Kind,
    /// The icon to show for it (callers may know a better one, e.g. Home).
    pub icon: String,
}

impl Item {
    /// Look `path` up on disk (one stat; call off the UI thread for paths
    /// on slow file systems).
    pub fn stat(path: &Path) -> Option<Item> {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).ok()?;
        let kind = files::kind_for(path, meta.is_dir(), meta.permissions().mode());
        Some(Item {
            path: path.to_owned(),
            name: path.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned()),
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            kind,
            icon: kind.icon().into(),
        })
    }
}

