//! Default Apps: which app each kind of file (and of link) opens in, and
//! what Open With offers for it, as the person chose (in Tracker's Always
//! Open With or here), else Olo's own choice; and kinds of the person's
//! own (a name and the extensions it covers). Three tabs:
//!
//! * **Default Apps**: each kind a pop-up of the apps Open With offers
//!   for it (`olo_xdg::open::handlers_for`); a choice is the kind's for
//!   Olo and, through `olo-mimeapps.list`, for other programs.
//! * **Your Own Kinds**: the person's kinds (`olo_xdg::mime::own_kinds`),
//!   each with its app (any that opens files) and Remove; Add a Kind
//!   names one by its extensions (an unknown extension becomes a type of
//!   Olo's own).
//! * **Open With**: for a kind, the apps Open With ▸ offers, in order:
//!   moved up and down, removed, added; Olo's Choice to let Olo choose
//!   again (`open::set_offered`).
//!
//! BeOS's FileTypes, as macOS's Get Info ▸ Open With ▸ Change All does
//! it. What's offered is worked out, and every change written, on a
//! worker (both read the applications' desktop entries); the panel
//! redraws as it comes.

use std::sync::{Arc, mpsc};

use olo_sync::Mutex;
use olo_ui::{
    gpui::{self, Entity, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{
        BBox, Button, EmptyState, ListModel, MenuField, TabView, TextControl, TextField,
        list::{Column, ColumnListView},
        text_field,
    },
};
use olo_xdg::{
    mime::{self, OwnKind},
    open::{self, Handler},
};

use crate::{Msg, PanelView, form::Ui, panel};

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

const TABS: [&str; 3] = ["Default Apps", "Your Own Kinds", "Open With"];

/// The labels' column, and every pop-up's width (one column of them,
/// whatever each offers).
const LABEL_W: f32 = 150.0;
const MENU_W: f32 = 230.0;
/// The Open With tab's kind label, and its buttons' least width.
const KIND_W: f32 = 50.0;
const BUTTON_W: f32 = 80.0;

/// An app: its id (`olo.viewer.desktop`), name and icon.
#[derive(Clone, Default, PartialEq)]
struct App {
    id: String,
    name: String,
    icon: String,
}

impl App {
    fn of(h: &Handler) -> Option<App> {
        Some(App { id: open::handler_id(h)?, name: h.name(), icon: h.icon() })
    }
}

/// A kind as the panel shows it: the apps offered and which it opens in.
#[derive(Clone, Default)]
struct Row {
    apps: Vec<App>,
    current: Option<usize>,
}

/// What Open With offers for a kind: the person's list (`set`) or Olo's.
#[derive(Clone, Default)]
struct Offered {
    set: bool,
    apps: Vec<App>,
}

/// What the worker found.
#[derive(Clone, Default)]
struct Found {
    builtin: Vec<Vec<Row>>,
    own: Vec<(OwnKind, Row)>,
    every: Vec<App>,
    /// For each kind on the Open With tab (the built-in ones, then the
    /// person's): its name, first type, and what's offered.
    offered: Vec<(String, String, Offered)>,
}

/// What the panel asks of the worker.
enum Job {
    /// Built-in kind (box, row) opens in app.
    Default(usize, usize, String),
    /// The person's kind `i` opens in app.
    OwnDefault(usize, String),
    AddKind { name: String, extensions: Vec<String>, app: Option<String> },
    RemoveKind(usize),
    /// What Open With offers for a type (empty: Olo's choice again).
    Offer(String, Vec<String>),
}

pub struct State {
    tab: usize,
    found: Arc<Mutex<Option<Found>>>,
    tx: mpsc::Sender<Job>,
    name: Entity<TextField>,
    extensions: Entity<TextField>,
    /// The app a new kind opens in (an index into `every`).
    new_app: usize,
    /// The kind the Open With tab shows (an index into `offered`), and
    /// its apps' list.
    offer_kind: usize,
    offers: Entity<ListModel>,
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::DefaultApps(st) => Some(st),
        _ => None,
    }
}

impl State {
    pub fn new(cx: &mut gpui::Context<PanelView>) -> State {
        let found: Arc<Mutex<Option<Found>>> = Arc::default();
        let (tx, rx) = mpsc::channel::<Job>();
        let name = text_field(cx, |f| f.hint("Game saves").name("Name"));
        let extensions = text_field(cx, |f| f.hint("sav, sv2").name("Extensions"));
        let st = State { tab: 0, found: found.clone(), tx, name, extensions, new_app: 0, offer_kind: 0, offers: ListModel::new(cx, false) };
        // (Tests: made-up apps at once, not this computer's.)
        if cfg!(test) {
            let app = |id: &str, name: &str, icon: &str| App { id: id.into(), name: name.into(), icon: icon.into() };
            let apps = vec![app("olo.viewer.desktop", "Viewer", "viewer"), app("org.example.Long.desktop", "A Much Longer Application Name", "application")];
            let row = Row { apps: apps.clone(), current: Some(0) };
            let own = vec![(OwnKind { name: "Game saves".into(), extensions: vec!["sav".into(), "sv2".into()], mime: "application/x-olo-kind-sav".into() }, row.clone())];
            let kinds: Vec<OwnKind> = own.iter().map(|(k, _)| k.clone()).collect();
            let offered = kinds_offered(&kinds).into_iter().map(|(n, t)| (n, t, Offered { set: false, apps: apps.clone() })).collect();
            *found.lock().unwrap() = Some(Found { builtin: KINDS.iter().map(|(_, kinds)| kinds.iter().map(|_| row.clone()).collect()).collect(), own, every: apps, offered });
            return st;
        }
        let proxy = olo_ui::proxy(cx);
        let _ = std::thread::Builder::new().name("default-apps".into()).spawn(move || {
            *found.lock().unwrap() = Some(work_out());
            proxy.send(Msg::Redraw);
            while let Ok(job) = rx.recv() {
                if let Err(e) = run(job) {
                    tracing::warn!("default apps: {e}");
                }
                *found.lock().unwrap() = Some(work_out());
                proxy.send(Msg::Redraw);
            }
        });
        st
    }
}

/// The kinds the Open With tab offers a choice of: (name, first type).
fn kinds_offered(own: &[OwnKind]) -> Vec<(String, String)> {
    let builtin = KINDS.iter().flat_map(|(_, kinds)| kinds.iter().map(|(name, types)| (name.to_string(), types[0].to_string())));
    builtin.chain(own.iter().map(|k| (k.name.clone(), k.mime.clone()))).collect()
}

/// Everything the panel shows, read now.
fn work_out() -> Found {
    let every: Vec<App> = open::every_app().iter().filter_map(App::of).collect();
    let row = |mime: &str, apps: Vec<App>| {
        let default = open::handler(mime).as_ref().and_then(open::handler_id);
        let current = apps.iter().position(|a| Some(&a.id) == default.as_ref());
        Row { apps, current }
    };
    let builtin = KINDS.iter().map(|(_, kinds)| kinds.iter().map(|(_, types)| row(types[0], open::handlers_for(types[0]).iter().filter_map(App::of).collect())).collect()).collect();
    let kinds = mime::own_kinds();
    // (A kind of one's own: any app may open it.)
    let own = kinds.iter().map(|k| (k.clone(), row(&k.mime, every.clone()))).collect();
    let offered = kinds_offered(&kinds)
        .into_iter()
        .map(|(name, t)| {
            let o = Offered { set: open::is_offered_set(&t), apps: open::handlers_for(&t).iter().filter_map(App::of).collect() };
            (name, t, o)
        })
        .collect();
    Found { builtin, own, every, offered }
}

fn run(job: Job) -> std::io::Result<()> {
    let default = |types: &[&str], id: &str| -> std::io::Result<()> {
        let Some(h) = open::handler_of_id(id) else { return Ok(()) };
        for t in types {
            open::set_default(t, Some(&h))?;
        }
        Ok(())
    };
    match job {
        Job::Default(b, k, id) => default(KINDS[b].1[k].1, &id)?,
        Job::OwnDefault(i, id) => {
            if let Some(k) = mime::own_kinds().get(i) {
                default(&[&k.mime], &id)?;
            }
        }
        Job::AddKind { name, extensions, app } => {
            let mut kinds: Vec<OwnKind> = mime::own_kinds().to_vec();
            // Its extensions are this kind's alone now.
            for k in &mut kinds {
                k.extensions.retain(|e| !extensions.contains(e));
            }
            kinds.retain(|k| !k.extensions.is_empty());
            let mime = mime::type_for_extensions(&extensions);
            kinds.push(OwnKind { name, extensions, mime: mime.clone() });
            mime::set_own_kinds(&kinds)?;
            if let Some(id) = app {
                default(&[&mime], &id)?;
            }
        }
        Job::RemoveKind(i) => {
            let mut kinds: Vec<OwnKind> = mime::own_kinds().to_vec();
            if i < kinds.len() {
                let gone = kinds.remove(i);
                mime::set_own_kinds(&kinds)?;
                // Its choices go too, unless a known type was given an app.
                if gone.mime.starts_with("application/x-olo-kind-") {
                    open::set_default(&gone.mime, None)?;
                    open::set_offered(&gone.mime, &[])?;
                }
            }
        }
        Job::Offer(t, ids) => {
            let handlers: Vec<Handler> = ids.iter().filter_map(|id| open::handler_of_id(id)).collect();
            open::set_offered(&t, &handlers)?;
        }
    }
    Ok(())
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let Some(found) = st.found.lock().unwrap().clone() else {
        return ui.add(EmptyState::new("Looking for apps…").icon("default-apps"));
    };
    let items = ui.collect(|ui| match st.tab {
        0 => defaults(ui, st, &found),
        1 => own_kinds(ui, st, &found),
        _ => offered(ui, st, &found),
    });
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.tab = i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
    ui.add(div().flex_1().min_h_0().child(TabView::new("default-apps-tabs", tabs, st.tab).on_select(pick).child(div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))));
}

/// A kind's pop-up: its apps, the one it opens in chosen; `send` the job
/// for another.
fn app_menu(ui: &mut Ui, id: String, label: String, row: &Row, send: impl Fn(String) + 'static) {
    if row.apps.is_empty() {
        ui.add(MenuField::new(id, vec!["No app opens these".into()], 0).label(label).divider(LABEL_W).width(LABEL_W + MENU_W).enabled(false));
        return;
    }
    let names = row.apps.iter().map(|a| a.name.clone().into()).collect();
    let (ids, current) = (row.apps.iter().map(|a| a.id.clone()).collect::<Vec<_>>(), row.current);
    ui.add(MenuField::new(id, names, current.unwrap_or(0)).label(label).divider(LABEL_W).width(LABEL_W + MENU_W).on_change(move |i, _, _| {
        if Some(i) != current
            && let Some(id) = ids.get(i)
        {
            send(id.clone());
        }
    }));
}

fn defaults(ui: &mut Ui, st: &State, found: &Found) {
    for (b, ((title, kinds), rows)) in KINDS.iter().zip(&found.builtin).enumerate() {
        ui.bbox(title, |ui| {
            for (k, ((name, _), row)) in kinds.iter().zip(rows).enumerate() {
                let tx = st.tx.clone();
                app_menu(ui, format!("default-app-{b}-{k}"), format!("{name}:"), row, move |id| {
                    let _ = tx.send(Job::Default(b, k, id));
                });
            }
        });
    }
    ui.note("Each kind opens in this app wherever you open it, in Olo and in other programs. Tracker's Open With ▸ Always Open With chooses too.");
}

fn own_kinds(ui: &mut Ui, st: &State, found: &Found) {
    ui.bbox("Your Kinds", |ui| {
        if found.own.is_empty() {
            ui.note("None yet: add one below, named by its extensions.");
        }
        for (i, (kind, row)) in found.own.iter().enumerate() {
            let exts = kind.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ");
            let tx = st.tx.clone();
            ui.row(|ui| {
                let send = tx.clone();
                app_menu(ui, format!("own-kind-{i}"), format!("{} ({exts}):", kind.name), row, move |id| {
                    let _ = send.send(Job::OwnDefault(i, id));
                });
                ui.button("Remove", move |_, _| {
                    let _ = tx.send(Job::RemoveKind(i));
                });
            });
        }
    });
    ui.bbox("Add a Kind", |ui| {
        ui.add(TextControl::new(st.name.clone()).label("Name:").divider(LABEL_W).width(LABEL_W + MENU_W));
        ui.add(TextControl::new(st.extensions.clone()).label("Extensions:").divider(LABEL_W).width(LABEL_W + MENU_W));
        let names: Vec<SharedString> = found.every.iter().map(|a| a.name.clone().into()).collect();
        let choose = ui.on_view(|v, i: usize, _, cx| {
            if let Some(st) = state_of(v) {
                st.new_app = i;
            }
            cx.notify();
        });
        if !names.is_empty() {
            ui.add(MenuField::new("new-kind-app", names, st.new_app.min(found.every.len() - 1)).label("Opens in:").divider(LABEL_W).width(LABEL_W + MENU_W).on_change(choose));
        }
        let every = found.every.clone();
        let add = ui.on_click_view(move |v, _, cx| {
            let Some(st) = state_of(v) else { return };
            let name = st.name.read(cx).text().trim().to_owned();
            let extensions: Vec<String> = st.extensions.read(cx).text().split([',', ' ', ';']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect();
            if name.is_empty() || extensions.is_empty() {
                return;
            }
            let app = every.get(st.new_app).map(|a| a.id.clone());
            let _ = st.tx.send(Job::AddKind { name, extensions, app });
            st.name.update(cx, |f, cx| f.set_text(String::new(), cx));
            st.extensions.update(cx, |f, cx| f.set_text(String::new(), cx));
        });
        ui.row(|ui| {
            ui.add(div().flex_1());
            ui.button("Add Kind", add);
        });
    });
    ui.note("Files with these extensions are this kind wherever Olo looks at them, and open in its app. An extension another kind had is this one's now.");
}

fn offered(ui: &mut Ui, st: &State, found: &Found) {
    if found.offered.is_empty() {
        return;
    }
    let at = st.offer_kind.min(found.offered.len() - 1);
    let names: Vec<SharedString> = found.offered.iter().map(|(n, _, _)| n.clone().into()).collect();
    let choose = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.offer_kind = i;
            st.offers.update(cx, |l, _| l.state.selected.clear());
        }
        cx.notify();
    });
    ui.add(MenuField::new("open-with-kind", names, at).label("Kind:").divider(KIND_W).width(KIND_W + MENU_W).on_change(choose));
    let (_, mime, offer) = &found.offered[at];
    let ids: Vec<String> = offer.apps.iter().map(|a| a.id.clone()).collect();
    let selected = st.offers.read(ui.cx).state.single().filter(|&i| i < ids.len());
    // A change to the order: sent, and the app moved stays chosen.
    let send = |order: Option<(Vec<String>, Option<usize>)>| {
        let mime = mime.clone();
        ui.on_click_view(move |v, _, cx| {
            let Some(st) = state_of(v) else { return };
            let Some((order, then)) = order.clone() else { return };
            let _ = st.tx.send(Job::Offer(mime.clone(), order));
            st.offers.update(cx, |l, _| match then {
                Some(i) => l.state.select(i),
                None => l.state.selected.clear(),
            });
            cx.notify();
        })
    };
    let moved = |by: isize| {
        let from = selected?;
        let to = from.checked_add_signed(by).filter(|&to| to < ids.len())?;
        let mut o = ids.clone();
        o.swap(from, to);
        Some((o, Some(to)))
    };
    // (Never none: a kind keeps an app to open it.)
    let removed = selected.filter(|_| ids.len() > 1).map(|i| {
        let mut o = ids.clone();
        o.remove(i);
        (o, None)
    });
    let (up, down) = (moved(-1), moved(1));
    let (can_up, can_down, can_remove) = (up.is_some(), down.is_some(), removed.is_some());
    let (up, down, remove) = (send(up), send(down), send(removed));
    let addable: Vec<&App> = found.every.iter().filter(|a| !ids.contains(&a.id)).collect();
    let mut add_names: Vec<SharedString> = vec!["Add an App…".into()];
    add_names.extend(addable.iter().map(|a| SharedString::from(a.name.clone())));
    let add_ids: Vec<String> = addable.iter().map(|a| a.id.clone()).collect();
    let (tx, add_mime, base) = (st.tx.clone(), mime.clone(), ids.clone());
    let add = move |i: usize, _: &mut gpui::Window, _: &mut gpui::App| {
        if let Some(id) = i.checked_sub(1).and_then(|i| add_ids.get(i)) {
            let mut o = base.clone();
            o.push(id.clone());
            let _ = tx.send(Job::Offer(add_mime.clone(), o));
        }
    };
    let apps = offer.apps.clone();
    let list = ColumnListView::new("open-with-apps", &st.offers, vec![Column::new("App", 300.0)], apps.len(), move |cell, window, cx| {
        let a = &apps[cell.row];
        cell.icon_text(window, cx, &a.icon, "application", a.name.clone());
    })
    .no_header()
    .empty_text("No app opens these yet: add one.")
    .on_select(|_, cx| cx.refresh_windows());
    let button = |label: &str, on: bool, f| Button::new(label.to_owned()).enabled(on).min_width(BUTTON_W).on_click(f);
    ui.add(
        BBox::new("Open With Offers")
            .child(div().h(px(m::LIST_ROW * 8.0 + 2.0)).child(list))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(MenuField::new("open-with-add", add_names, 0).width(MENU_W - BUTTON_W).enabled(!addable.is_empty()).on_change(add))
                    .child(div().flex_1())
                    .child(button("Move Up", can_up, up))
                    .child(button("Move Down", can_down, down))
                    .child(button("Remove", can_remove, remove)),
            ),
    );
    let (reset_tx, reset_mime) = (st.tx.clone(), mime.clone());
    ui.row(|ui| {
        ui.note("Tracker's Open With ▸ offers these for this kind, in this order.");
        ui.add(div().flex_1());
        ui.enabled_if(offer.set, |ui| {
            ui.button("Olo's Choice", move |_, _| {
                let _ = reset_tx.send(Job::Offer(reset_mime.clone(), Vec::new()));
            })
        });
    });
}
