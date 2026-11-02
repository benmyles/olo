//! Default Apps: the app each kind of file (and of link) opens in, as the
//! person chose it in Tracker's Always Open With or here, else Olo's own
//! choice; each kind a pop-up of the apps Open With offers for it
//! (`olo_xdg::open::handlers_for`). A choice is the kind's for Olo and,
//! through `olo-mimeapps.list`, for other programs. BeOS's FileTypes,
//! as macOS's Get Info ▸ Open With ▸ Change All does it.
//!
//! What's offered is worked out, and a choice written, on a worker (both
//! read the applications' desktop entries); the panel redraws as it
//! comes.

use std::sync::{Arc, mpsc};

use olo_sync::Mutex;
use olo_ui::{gpui, widgets::{EmptyState, MenuField}};
use olo_xdg::open::{self, Handler};

use crate::{Msg, PanelView, form::Ui};

/// The kinds, in their boxes: each a name and the types it stands for
/// (a choice is made for every one; the first says what's offered).
pub const KINDS: &[(&str, &[(&str, &[&str])])] = &[
    (
        "Documents",
        &[
            ("Text", &["text/plain"]),
            ("PDF documents", &["application/pdf"]),
            ("Word documents", &["application/vnd.openxmlformats-officedocument.wordprocessingml.document", "application/vnd.oasis.opendocument.text", "application/msword", "application/rtf"]),
            ("Spreadsheets", &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "application/vnd.oasis.opendocument.spreadsheet", "text/csv"]),
            ("Presentations", &["application/vnd.openxmlformats-officedocument.presentationml.presentation", "application/vnd.oasis.opendocument.presentation"]),
        ],
    ),
    (
        "Pictures, Movies and Music",
        &[
            ("Pictures", &["image/png", "image/jpeg", "image/gif", "image/webp", "image/heic", "image/avif", "image/tiff", "image/bmp", "image/svg+xml"]),
            ("Movies", &["video/mp4", "video/webm", "video/x-matroska", "video/quicktime"]),
            ("Music", &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav", "audio/mp4"]),
        ],
    ),
    (
        "Everything Else",
        &[
            ("Archives", &["application/zip", "application/x-compressed-tar", "application/x-tar", "application/gzip", "application/x-7z-compressed"]),
            ("Folders", &["inode/directory"]),
            ("Web pages and links", &["x-scheme-handler/https", "x-scheme-handler/http", "text/html"]),
            ("Email links", &["x-scheme-handler/mailto"]),
        ],
    ),
];

/// The labels' column, and every pop-up's width (one column of them,
/// whatever each offers).
const LABEL_W: f32 = 150.0;
const MENU_W: f32 = 230.0;

/// A kind as the panel shows it: the apps offered (id, name) and which
/// it opens in.
#[derive(Clone, Default)]
struct Row {
    apps: Vec<(String, String)>,
    current: Option<usize>,
}

/// What the worker found (None: still looking), and where choices go.
pub struct State {
    rows: Arc<Mutex<Option<Vec<Vec<Row>>>>>,
    tx: mpsc::Sender<(usize, usize, String)>,
}

impl State {
    pub fn new(cx: &mut gpui::Context<PanelView>) -> State {
        let rows: Arc<Mutex<Option<Vec<Vec<Row>>>>> = Arc::default();
        let (tx, rx) = mpsc::channel::<(usize, usize, String)>();
        // (Tests: made-up apps at once, not this computer's.)
        if cfg!(test) {
            let made_up = Row { apps: vec![("olo.viewer.desktop".into(), "Viewer".into()), ("org.example.Long.desktop".into(), "A Much Longer Application Name".into())], current: Some(0) };
            *rows.lock().unwrap() = Some(KINDS.iter().map(|(_, kinds)| kinds.iter().map(|_| made_up.clone()).collect()).collect());
            return State { rows, tx };
        }
        let (found, proxy) = (rows.clone(), olo_ui::proxy(cx));
        let _ = std::thread::Builder::new().name("default-apps".into()).spawn(move || {
            *found.lock().unwrap() = Some(KINDS.iter().map(|(_, kinds)| kinds.iter().map(|(_, types)| row(types[0])).collect()).collect());
            proxy.send(Msg::Redraw);
            // A choice: made for each of the kind's types, then the row
            // worked out again.
            while let Ok((b, k, id)) = rx.recv() {
                let types = KINDS[b].1[k].1;
                if let Some(h) = open::handler_of_id(&id) {
                    for t in types {
                        if let Err(e) = open::set_default(t, Some(&h)) {
                            tracing::warn!("couldn't make {id} the app for {t}: {e}");
                        }
                    }
                }
                let fresh = row(types[0]);
                if let Some(rows) = found.lock().unwrap().as_mut() {
                    rows[b][k] = fresh;
                }
                proxy.send(Msg::Redraw);
            }
        });
        State { rows, tx }
    }
}

/// What's offered for `mime`, and which it opens in.
fn row(mime: &str) -> Row {
    let handlers: Vec<Handler> = open::handlers_for(mime);
    let default = open::handler(mime).as_ref().and_then(open::handler_id);
    let apps: Vec<(String, String)> = handlers.iter().filter_map(|h| Some((open::handler_id(h)?, h.name()))).collect();
    let current = apps.iter().position(|(id, _)| Some(id) == default.as_ref());
    Row { apps, current }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let Some(rows) = st.rows.lock().unwrap().clone() else {
        return ui.add(EmptyState::new("Looking for apps…").icon("default-apps"));
    };
    for (b, ((title, kinds), found)) in KINDS.iter().zip(&rows).enumerate() {
        ui.bbox(*title, |ui| {
            for (k, ((name, _), row)) in kinds.iter().zip(found).enumerate() {
                if row.apps.is_empty() {
                    ui.add(MenuField::new(format!("default-app-{b}-{k}"), vec!["No app opens these".into()], 0).label(format!("{name}:")).divider(LABEL_W).width(LABEL_W + MENU_W).enabled(false));
                    continue;
                }
                let names = row.apps.iter().map(|(_, n)| n.clone().into()).collect();
                let (ids, current, tx) = (row.apps.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(), row.current, st.tx.clone());
                ui.add(MenuField::new(format!("default-app-{b}-{k}"), names, current.unwrap_or(0)).label(format!("{name}:")).divider(LABEL_W).width(LABEL_W + MENU_W).on_change(move |i, _, _| {
                    if Some(i) != current
                        && let Some(id) = ids.get(i)
                    {
                        let _ = tx.send((b, k, id.clone()));
                    }
                }));
            }
        });
    }
    ui.note("Each kind opens in this app wherever you open it, in Olo and in other programs. Tracker's Open With ▸ Always Open With chooses too.");
}
