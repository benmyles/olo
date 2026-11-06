//! `org.freedesktop.FileManager1`: how other programs ask the desktop's
//! file manager to show something (Chrome's Show in folder, Electron's
//! `shell.showItemInFolder`, GTK's and Qt's "open containing folder").
//! The session's Tracker (the desktop's) serves it, taking the name from
//! whatever file manager had it, and each call is an Open in Tracker
//! (`--reveal`): a file selected in its folder's window, a folder in its
//! own.

use std::path::PathBuf;

pub const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";

struct FileManager {
    reveal: Box<dyn Fn(Vec<PathBuf>) + Send + Sync>,
}

/// The local paths among `uris` (`file://` URIs; others are left out).
pub fn paths(uris: &[String]) -> Vec<PathBuf> {
    uris.iter().flat_map(|u| olo_kit_base::files::uri_list_paths(u)).collect()
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager {
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        (self.reveal)(paths(&uris));
    }

    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        (self.reveal)(paths(&uris));
    }

    /// (Tracker's Get Info is its folder window, the item selected.)
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        (self.reveal)(paths(&uris));
    }
}

/// Serve it on the session bus until the connection drops: at once if
/// the name is free or its owner lets it be taken, else as soon as its
/// owner lets it go; and `true` if it's ours now.
///
/// Waiting in line, not giving up: an owner that doesn't allow the name
/// taken from it (Nautilus, started by D-Bus activation as the system's
/// FileManager1 service while no Tracker had the name, or the last
/// Tracker's connection not yet gone as Tracker restarts after an
/// upgrade) once kept Tracker out for the rest of the session, and every
/// program's "show in folder" went nowhere (Ben's Chrome, 2026-11-04: the
/// journal said "name already taken" at each upgrade). Allowing it taken
/// from us: the next Tracker takes it over at once.
pub fn serve(reveal: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) -> zbus::Result<(zbus::blocking::Connection, bool)> {
    let conn = zbus::blocking::connection::Builder::session()?.serve_at(PATH, FileManager { reveal: Box::new(reveal) })?.build()?;
    let flags = zbus::fdo::RequestNameFlags::ReplaceExisting | zbus::fdo::RequestNameFlags::AllowReplacement;
    let ours = match conn.request_name_with_flags(NAME, flags)? {
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner => true,
        // (In line: the bus gives it to us when its owner goes.)
        _ => false,
    };
    Ok((conn, ours))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_files_only() {
        let uris = vec!["file:///home/ben/Downloads/a%20b.pdf".to_owned(), "https://example.com/x".to_owned(), "file:///tmp".to_owned()];
        assert_eq!(paths(&uris), vec![PathBuf::from("/home/ben/Downloads/a b.pdf"), PathBuf::from("/tmp")]);
    }
}
