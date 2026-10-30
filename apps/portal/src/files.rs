//! The FileChooser portal (`org.freedesktop.impl.portal.FileChooser`):
//! other programs' Open and Save dialogs (Chrome's, Electron's, GTK 4's,
//! Qt's through the portal) as Olo's own file panel (`olo_kit::
//! file_panel`), in a window over the asking one. OpenFile opens a file
//! (or chooses a folder, with `directory`), SaveFile saves one under the
//! name proposed (asking before replacing), SaveFiles chooses the folder
//! several go in. A filter's glob patterns (`*.pdf`) become the panel's
//! extensions; MIME types alone show every file. The chosen file goes
//! back as a `file://` URI.

use std::{collections::HashMap, path::PathBuf};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::portal::ToUi;

/// What the panel is for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Purpose {
    Open,
    Folder,
    Save,
    /// The folder several files are saved in.
    SaveFiles,
}

/// What a panel was asked (from the portal).
pub struct Ask {
    pub request: String,
    pub parent: String,
    pub title: String,
    pub purpose: Purpose,
    /// Where it starts.
    pub folder: Option<PathBuf>,
    /// The name proposed for a save.
    pub name: String,
    /// The extensions offered (empty: every file).
    pub types: Vec<String>,
    /// The answer: what was chosen, or None if cancelled.
    pub reply: async_channel::Sender<Option<PathBuf>>,
}

pub struct FileChooser {
    pub to_ui: std::sync::Arc<dyn Fn(ToUi) + Send + Sync>,
}

/// A path the portal gives as bytes (NUL-terminated).
fn path(options: &HashMap<String, OwnedValue>, key: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = <Vec<u8>>::try_from(options.get(key)?.try_clone().ok()?).ok()?;
    let bytes: Vec<u8> = bytes.into_iter().take_while(|b| *b != 0).collect();
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

/// The extensions a filter's globs name (`*.pdf`, `*.[Pp][Nn][Gg]`): the
/// current filter's, else the first's; none if any of its patterns is a
/// MIME type or a glob that isn't a plain extension (then every file).
pub fn extensions(filters: &[(String, Vec<(u32, String)>)], current: Option<&(String, Vec<(u32, String)>)>) -> Vec<String> {
    let Some((_, patterns)) = current.or(filters.first()) else { return Vec::new() };
    let mut out = Vec::new();
    for (kind, pattern) in patterns {
        let ext = (*kind == 0).then(|| pattern.strip_prefix("*.")).flatten().map(|e| {
            // `[Pp][Dd][Ff]`: one letter in either case each.
            let mut plain = String::new();
            let mut chars = e.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '[' {
                    let first = chars.next();
                    for c in chars.by_ref() {
                        if c == ']' {
                            break;
                        }
                    }
                    if let Some(f) = first {
                        plain.push(f);
                    }
                } else {
                    plain.push(ch);
                }
            }
            plain.to_lowercase()
        });
        match ext.filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric())) {
            Some(e) => out.push(e),
            None => return Vec::new(),
        }
    }
    out
}

impl FileChooser {
    async fn ask(&self, handle: OwnedObjectPath, parent: String, title: String, purpose: Purpose, options: &HashMap<String, OwnedValue>, server: &zbus::ObjectServer) -> Option<PathBuf> {
        type Filter = (String, Vec<(u32, String)>);
        let filters: Vec<Filter> = options.get("filters").and_then(|v| <Vec<Filter>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let current: Option<Filter> = options.get("current_filter").and_then(|v| <Filter>::try_from(v.try_clone().ok()?).ok());
        let current_file = path(options, "current_file");
        let folder = path(options, "current_folder").or_else(|| current_file.as_ref().and_then(|f| f.parent().map(PathBuf::from)));
        let name = match string(options, "current_name") {
            n if !n.is_empty() => n,
            _ => current_file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Files(Ask { request: handle.to_string(), parent, title, purpose, folder, name, types: extensions(&filters, current.as_ref()), reply: tx }));
        let chosen = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        chosen
    }
}

fn answer(uris: Vec<PathBuf>) -> (u32, HashMap<String, OwnedValue>) {
    let uris: Vec<String> = uris.iter().map(|p| olo_kit::dnd::file_uri(p)).collect();
    (0, HashMap::from([("uris".to_owned(), OwnedValue::try_from(Value::from(uris)).expect("strings"))]))
}

/// The person cancelled.
fn cancelled() -> (u32, HashMap<String, OwnedValue>) {
    (1, HashMap::new())
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    async fn open_file(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let folder = options.get("directory").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let purpose = if folder { Purpose::Folder } else { Purpose::Open };
        match self.ask(handle, parent_window, title, purpose, &options, server).await {
            Some(p) => answer(vec![p]),
            None => cancelled(),
        }
    }

    async fn save_file(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        match self.ask(handle, parent_window, title, Purpose::Save, &options, server).await {
            Some(p) => answer(vec![p]),
            None => cancelled(),
        }
    }

    async fn save_files(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        use std::os::unix::ffi::OsStringExt;
        let files: Vec<Vec<u8>> = options.get("files").and_then(|v| <Vec<Vec<u8>>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let Some(dir) = self.ask(handle, parent_window, title, Purpose::SaveFiles, &options, server).await else { return cancelled() };
        let names = files.into_iter().map(|f| PathBuf::from(std::ffi::OsString::from_vec(f.into_iter().take_while(|b| *b != 0).collect())));
        answer(names.filter_map(|n| n.file_name().map(|f| dir.join(f))).collect())
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(name: &str, patterns: &[(u32, &str)]) -> (String, Vec<(u32, String)>) {
        (name.into(), patterns.iter().map(|(k, p)| (*k, (*p).to_owned())).collect())
    }

    #[test]
    fn globs_become_extensions() {
        let pdf = filter("PDF", &[(0, "*.pdf"), (0, "*.[Pp][Dd][Ff]")]);
        let images = filter("Images", &[(1, "image/*"), (0, "*.png")]);
        assert_eq!(extensions(std::slice::from_ref(&pdf), None), vec!["pdf", "pdf"]);
        // A MIME type in it: every file (Olo can't tell types by MIME here).
        assert!(extensions(&[images.clone()], None).is_empty());
        // The current one wins.
        assert_eq!(extensions(&[images, pdf.clone()], Some(&pdf)), vec!["pdf", "pdf"]);
        assert!(extensions(&[], None).is_empty());
        assert!(extensions(&[filter("All", &[(0, "*")])], None).is_empty());
    }
}
