//! The clipboard for a program with no window (a screenshot taken by a
//! shortcut): [`hold`] puts something on it and serves it until something
//! else is copied. An app with windows copies with [`Cx::copy`](crate::Cx::copy).
//!
//! The usual way to set the clipboard (`wl_data_device`) is only for the
//! client with the keyboard, and a program with no window never has it,
//! so this goes through `ext_data_control_v1`, which app_server offers
//! every client (as clipboard tools use it).
//!
//! ```ignore
//! println!("{}", path.display());
//! olo_kit::clipboard::hold(olo_kit::clipboard::text(&path.display().to_string()))?;
//! ```

/// The forms of what is on the clipboard, as the app last heard (it
/// hears while one of its windows has the keyboard).
static TYPES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn note_types(types: Vec<String>) {
    *TYPES.lock().unwrap_or_else(|e| e.into_inner()) = types;
}

/// Whether the clipboard has any of `wanted` (MIME types; one ending in
/// `/`, as `image/`, stands for any of that kind): for enabling Paste.
pub fn offers(wanted: &[&str]) -> bool {
    let types = TYPES.lock().unwrap_or_else(|e| e.into_inner());
    wanted.iter().any(|w| types.iter().any(|t| if w.ends_with('/') { t.starts_with(w) } else { t == w }))
}

pub use olo_kit_base::clipboard::{FILES, copied_files, files, hold, text};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_go_both_ways() {
        let paths = vec![std::path::PathBuf::from("/home/a/My Notes.txt"), std::path::PathBuf::from("/tmp/b")];
        let clip = files(&paths, true);
        let gnome = clip.iter().find(|(t, _)| t == FILES[0]).unwrap();
        assert_eq!(&*gnome.1, b"cut\nfile:///home/a/My%20Notes.txt\nfile:///tmp/b");
        assert_eq!(copied_files(&gnome.0, &gnome.1), Some((paths.clone(), true)));
        let list = clip.iter().find(|(t, _)| t == FILES[1]).unwrap();
        assert_eq!(copied_files(&list.0, &list.1), Some((paths.clone(), false)));
        assert!(clip.iter().any(|(t, _)| t == "application/x-kde-cutselection"));
        assert!(!files(&paths, false).iter().any(|(t, _)| t == "application/x-kde-cutselection"));
        // Web links aren't files here.
        assert_eq!(copied_files("text/uri-list", b"https://example.com/\r\n"), None);
        note_types(vec!["image/png".into(), "text/plain".into()]);
        assert!(offers(&["image/"]) && offers(&["text/plain"]) && !offers(&FILES));
    }
}
