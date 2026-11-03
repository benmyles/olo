//! Chrome's downloads in Olo, as the extension tells of them
//! (background.js): `{downloads: [...]}` with the ones under way, every
//! half second while there are any, and `{downloaded: {...}}` when one
//! finishes or fails. Deskbar shows how far along they are on Chrome's
//! entry (a `com.canonical.Unity.LauncherEntry` update, as Chrome itself
//! would send through libunity), the file being written is marked with
//! the size it will be (`olo_kit_base::files::EXPECTED_SIZE`, which Tracker
//! shows it by), and a
//! finished one is an Olo notification: Open, Open in Tracker.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use olo_kit_base::notify::{Note, NoteEvent, NoteId, NoteKind, Notifier};
use serde::Deserialize;
use zbus::zvariant::Value;

/// One download, as the extension tells it (`downloadOf`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Download {
    pub id: u64,
    /// Where it goes once done.
    pub file: PathBuf,
    #[serde(default)]
    pub received: u64,
    pub total: Option<u64>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub url: String,
    /// Why it stopped (Chrome's `InterruptReason`), for a finished one.
    #[serde(default)]
    pub error: Option<String>,
}

pub struct Downloads {
    notes: Notifier,
    /// What each notification is about, for its actions.
    shown: std::sync::Arc<olo_sync::Mutex<HashMap<NoteId, PathBuf>>>,
    bus: Option<zbus::blocking::Connection>,
    /// Marked with `EXPECTED` already (by id).
    marked: Vec<u64>,
    /// What Deskbar was last told (count, progress), to tell it only
    /// changes.
    told: Option<(usize, Option<f64>)>,
}

impl Downloads {
    pub fn new() -> Downloads {
        let shown: std::sync::Arc<olo_sync::Mutex<HashMap<NoteId, PathBuf>>> = Default::default();
        let about = shown.clone();
        let notes = Notifier::with_events("Google Chrome", "google-chrome", move |e| {
            let NoteEvent::Action { id, key, .. } = e else { return };
            let Some(file) = about.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned() else { return };
            let done = match key.as_str() {
                "reveal" => olo_kit_base::tracker::open(&file),
                _ => olo_xdg::open::open(&olo_xdg::open::Target::Path(file)),
            };
            if let Err(e) = done {
                eprintln!("olo-chrome: {e}");
            }
        });
        Downloads { notes, shown, bus: zbus::blocking::Connection::session().ok(), marked: Vec::new(), told: None }
    }

    /// The ones under way now.
    pub fn going(&mut self, list: &[Download]) {
        for d in list {
            if let Some(total) = d.total
                && !self.marked.contains(&d.id)
                && olo_kit_base::files::set_expected_size(&partial_of(&d.file), total)
            {
                self.marked.push(d.id);
            }
        }
        self.marked.retain(|id| list.iter().any(|d| d.id == *id));
        let told = (list.len(), progress(list));
        if self.told != Some(told) {
            self.told = Some(told);
            self.tell_deskbar(told);
        }
    }

    /// One finished, or stopped for good.
    pub fn done(&mut self, d: &Download) {
        let name = d.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let from = host_of(&d.url);
        let note = match d.error.as_deref() {
            None => {
                let mut note = Note::new(name).body(match &from {
                    Some(h) => format!("Downloaded from {h}"),
                    None => "Downloaded".into(),
                });
                if is_picture(&d.file) {
                    note = note.image(&d.file);
                }
                note.action("default", "Open").action("open", "Open").action("reveal", olo_kit_base::tracker::LABEL)
            }
            // Stopped by the person (or by Chrome quitting): nothing to say.
            Some("USER_CANCELED" | "USER_SHUTDOWN") => return,
            Some(why) => Note::new(format!("Couldn’t download {name}")).body(reason(why)).kind(NoteKind::Error),
        };
        let id = self.notes.show(None, note);
        if d.error.is_none() {
            self.shown.lock().unwrap_or_else(|e| e.into_inner()).insert(id, d.file.clone());
        }
    }

    fn tell_deskbar(&self, (count, progress): (usize, Option<f64>)) {
        let Some(bus) = &self.bus else { return };
        let mut props: HashMap<&str, Value> = HashMap::new();
        props.insert("count", Value::from(count as i64));
        props.insert("count-visible", Value::from(count > 0));
        props.insert("progress", Value::from(progress.unwrap_or(0.0)));
        props.insert("progress-visible", Value::from(progress.is_some()));
        let app = format!("application://{}", olo_xdg::open::CHROME_ENTRY);
        if let Err(e) = bus.emit_signal(None::<&str>, "/com/olodesktop/chrome/downloads", "com.canonical.Unity.LauncherEntry", "Update", &(app, props)) {
            eprintln!("olo-chrome: can't tell Deskbar about downloads: {e}");
        }
    }
}

/// How far along they are all together, if every one knows its size.
fn progress(list: &[Download]) -> Option<f64> {
    if list.is_empty() || list.iter().any(|d| d.total.is_none()) {
        return None;
    }
    let total: u64 = list.iter().filter_map(|d| d.total).sum();
    let received: u64 = list.iter().map(|d| d.received).sum();
    // (Rounded to a thousandth, so Deskbar hears of real changes only.)
    (total > 0).then(|| ((received as f64 / total as f64).clamp(0.0, 1.0) * 1000.0).round() / 1000.0)
}

/// The file Chrome writes `file` to until it is done.
pub fn partial_of(file: &Path) -> PathBuf {
    let mut p = file.as_os_str().to_owned();
    p.push(olo_kit_base::files::DOWNLOADING);
    PathBuf::from(p)
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = host.split(':').next()?.trim_start_matches("www.");
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_picture(file: &Path) -> bool {
    olo_xdg::mime::for_path(file).starts_with("image/")
}

/// Chrome's reason, in words.
fn reason(why: &str) -> String {
    match why {
        "FILE_NO_SPACE" => "The disk is full.",
        "FILE_ACCESS_DENIED" => "The folder can’t be written to.",
        "FILE_NAME_TOO_LONG" => "Its name is too long.",
        "FILE_TOO_LARGE" => "It’s too big for the disk.",
        "FILE_VIRUS_INFECTED" | "FILE_BLOCKED" | "FILE_SECURITY_CHECK_FAILED" => "Chrome blocked it.",
        "NETWORK_FAILED" | "NETWORK_TIMEOUT" | "NETWORK_DISCONNECTED" => "The network failed.",
        "NETWORK_SERVER_DOWN" | "SERVER_FAILED" => "The server failed.",
        "SERVER_UNAUTHORIZED" | "SERVER_FORBIDDEN" => "The server refused it.",
        "SERVER_BAD_CONTENT" => "The server doesn’t have it.",
        _ => "It stopped before it was done.",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: u64, received: u64, total: Option<u64>) -> Download {
        Download { id, file: "/tmp/x.zip".into(), received, total, paused: false, url: String::new(), error: None }
    }

    #[test]
    fn progress_of_all() {
        assert_eq!(progress(&[]), None);
        assert_eq!(progress(&[d(1, 25, Some(100)), d(2, 50, Some(100))]), Some(0.375));
        // One of unknown size: no bar, just the count.
        assert_eq!(progress(&[d(1, 25, Some(100)), d(2, 50, None)]), None);
    }

    #[test]
    fn names_and_hosts() {
        assert_eq!(partial_of(Path::new("/home/a/Downloads/x.zip")), Path::new("/home/a/Downloads/x.zip.crdownload"));
        assert_eq!(host_of("https://www.example.com:8080/a/b?c").as_deref(), Some("example.com"));
        assert_eq!(host_of("https://user@dl.example.org/f").as_deref(), Some("dl.example.org"));
        assert_eq!(host_of("blob:null"), None);
        let m: Download = serde_json::from_str(r#"{"id":3,"file":"/a/b.pdf","received":10,"total":null,"url":"https://x.org/b.pdf"}"#).unwrap();
        assert_eq!((m.total, m.error), (None, None));
    }
}
