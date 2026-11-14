//! Open in Tracker, the one way every app shows a folder, or a file in its
//! folder, in Tracker. Wherever something has a folder or file behind it
//! (a Terminal tab's working folder, a document, a picture), its context
//! menu, and its File menu where it has one, offers [`LABEL`] (command
//! [`COMMAND`]), which calls [`open`].
//!
//! ```ignore
//! menu.item(olo_kit::tracker::LABEL, olo_kit::tracker::COMMAND, None)
//! // and for the command:
//! olo_kit::tracker::open(&cwd)
//! ```

use std::{io, path::Path};

/// The item's label, the same everywhere.
pub const LABEL: &str = "Open in Tracker";
/// Its menu command.
pub const COMMAND: &str = "open-in-tracker";

/// Show `path` in Tracker (`olo-tracker --reveal PATH`): a folder in its
/// window, a file selected in its folder's, on the workspace showing (one
/// there, else a new one). Tracker starts if it isn't running.
pub fn open(path: &Path) -> io::Result<()> {
    let args = ["--reveal".to_owned(), path.display().to_string()];
    if crate::instance::forward("olo.tracker", &args) {
        return Ok(());
    }
    let tracker = crate::find_program("olo-tracker").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Tracker isn’t installed"))?;
    let mut argv = vec![tracker.into_os_string()];
    argv.extend(args.iter().map(Into::into));
    crate::open::spawn(&argv)
}
