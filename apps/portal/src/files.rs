//! The FileChooser portal (`org.freedesktop.impl.portal.FileChooser`):
//! other programs' Open and Save dialogs (Chrome's, Electron's, GTK 4's,
//! Qt's through the portal) as Olo's own file panel (`olo_ui::
//! file_panel`), in a window over the asking one. OpenFile opens a file
//! (or chooses a folder, with `directory`), SaveFile saves one under the
//! name proposed (asking before replacing), SaveFiles chooses the folder
//! several go in. The app's filters (globs and MIME types) are the panel's
//! Show pop-up, `multiple` lets it choose several files, and the app's
//! `choices` (checkboxes, pop-ups) sit above the buttons. What was chosen
//! goes back as `file://` URIs, with the filter shown and the choices
//! made.

use std::{collections::HashMap, path::PathBuf};

use olo_ui::file_panel::{Choice, Filter};
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
    /// The filters offered (none: every file), and the one shown first.
    pub filters: Vec<Filter>,
    pub current: usize,
    /// Whether several files may be opened.
    pub multiple: bool,
    pub choices: Vec<Choice>,
    /// The answer, or None if cancelled.
    pub reply: async_channel::Sender<Option<Chosen>>,
}

/// What the person chose.
#[derive(Debug, Default)]
pub struct Chosen {
    pub paths: Vec<PathBuf>,
    /// The filter shown (an index into the Ask's).
    pub filter: Option<usize>,
    /// Each choice's id and value.
    pub choices: Vec<(String, String)>,
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

/// A filter as the portal gives it: a name, then (0, glob) or (1, MIME
/// type) each.
type PortalFilter = (String, Vec<(u32, String)>);

fn filter(f: &PortalFilter) -> Filter {
    f.1.iter().fold(Filter::new(&f.0), |acc, (kind, pattern)| if *kind == 0 { acc.glob(pattern) } else { acc.mime(pattern) })
}

/// The app's filters, and which is shown first: `current` if it's one of
/// them, else added (alone, it applies with no pop-up).
pub fn filters(list: &[PortalFilter], current: Option<&PortalFilter>) -> (Vec<PortalFilter>, usize) {
    let mut list = list.to_vec();
    let at = match current {
        Some(c) => list.iter().position(|f| f == c).unwrap_or_else(|| {
            list.push(c.clone());
            list.len() - 1
        }),
        None => 0,
    };
    (list, at)
}

impl FileChooser {
    async fn ask(&self, handle: OwnedObjectPath, parent: String, title: String, purpose: Purpose, options: &HashMap<String, OwnedValue>, server: &zbus::ObjectServer) -> Option<(Chosen, Vec<PortalFilter>)> {
        let given: Vec<PortalFilter> = options.get("filters").and_then(|v| <Vec<PortalFilter>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
        let current: Option<PortalFilter> = options.get("current_filter").and_then(|v| <PortalFilter>::try_from(v.try_clone().ok()?).ok());
        let (list, current) = filters(&given, current.as_ref());
        let choices = options
            .get("choices")
            .and_then(|v| <Vec<(String, String, Vec<(String, String)>, String)>>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, label, options, value)| Choice { id, label, options, value })
            .collect();
        let multiple = purpose == Purpose::Open && options.get("multiple").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let current_file = path(options, "current_file");
        let folder = path(options, "current_folder").or_else(|| current_file.as_ref().and_then(|f| f.parent().map(PathBuf::from)));
        let name = match string(options, "current_name") {
            n if !n.is_empty() => n,
            _ => current_file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        let filters = list.iter().map(filter).collect();
        (self.to_ui)(ToUi::Files(Ask { request: handle.to_string(), parent, title, purpose, folder, name, filters, current, multiple, choices, reply: tx }));
        let chosen = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        chosen.map(|c| (c, list))
    }
}

fn answer(uris: Vec<PathBuf>, chosen: &Chosen, filters: &[PortalFilter]) -> (u32, HashMap<String, OwnedValue>) {
    let uris: Vec<String> = uris.iter().map(|p| olo_ui::files::file_uri(p)).collect();
    let mut results = HashMap::from([("uris".to_owned(), OwnedValue::try_from(Value::from(uris)).expect("strings"))]);
    if !chosen.choices.is_empty() {
        results.insert("choices".into(), OwnedValue::try_from(Value::from(chosen.choices.clone())).expect("strings"));
    }
    if let Some(f) = chosen.filter.and_then(|i| filters.get(i)) {
        results.insert("current_filter".into(), OwnedValue::try_from(Value::from(f.clone())).expect("strings"));
    }
    (0, results)
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
            Some((c, filters)) => answer(c.paths.clone(), &c, &filters),
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
            Some((c, filters)) => answer(c.paths.clone(), &c, &filters),
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
        let Some((c, filters)) = self.ask(handle, parent_window, title, Purpose::SaveFiles, &options, server).await else { return cancelled() };
        let Some(dir) = c.paths.first().cloned() else { return cancelled() };
        let names = files.into_iter().map(|f| PathBuf::from(std::ffi::OsString::from_vec(f.into_iter().take_while(|b| *b != 0).collect())));
        answer(names.filter_map(|n| n.file_name().map(|f| dir.join(f))).collect(), &c, &filters)
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_come_as_bytes() {
        let options = HashMap::from([("current_folder".to_owned(), OwnedValue::try_from(Value::from(b"/home/a/Documents\0".to_vec())).unwrap())]);
        assert_eq!(path(&options, "current_folder"), Some(PathBuf::from("/home/a/Documents")));
        assert_eq!(path(&options, "current_file"), None);
    }

    fn pf(name: &str, patterns: &[(u32, &str)]) -> PortalFilter {
        (name.into(), patterns.iter().map(|(k, p)| (*k, (*p).to_owned())).collect())
    }

    #[test]
    fn the_apps_filters() {
        let pdf = pf("PDF", &[(0, "*.pdf"), (1, "application/pdf")]);
        let images = pf("Images", &[(1, "image/*")]);
        let f = filter(&pdf);
        assert_eq!((f.globs.clone(), f.types.clone()), (vec!["*.pdf".to_owned()], vec!["application/pdf".to_owned()]));
        assert!(f.matches("Report.PDF", "application/octet-stream"));
        assert!(filter(&images).matches("x", "image/png"));
        // The current one is found, or added.
        assert_eq!(filters(&[images.clone(), pdf.clone()], Some(&pdf)).1, 1);
        let (list, at) = filters(&[images.clone()], Some(&pdf));
        assert_eq!((list.len(), at), (2, 1));
        // Alone, it's the only one.
        assert_eq!(filters(&[], Some(&pdf)), (vec![pdf], 0));
        assert_eq!(filters(&[images.clone()], None), (vec![images], 0));
    }
}
