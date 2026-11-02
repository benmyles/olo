//! Drag and drop between windows and apps, over Wayland's data device.
//!
//! As on BeOS, a drag carries its data in several types and offers some
//! actions, and the place it is dropped on picks the type and the action
//! it wants (the Be Book's "negotiated" drag and drop; see
//! vendor/bebook/DragAndDrop.md). Here the types are MIME types, the
//! actions Wayland's copy, move and ask, and it works the same within a
//! window, between Olo's apps and with any other Wayland program.
//!
//! **Dragging out:** when an egui drag starts on something draggable, call
//! [`crate::Cx::start_drag`] with a [`DragData`] (only while the pointer
//! button is still down). The window gets [`crate::App::drag_ended`] when
//! it is over.
//!
//! **Taking drops:** while something is dragged over a window, each frame
//! [`hover`] says where and with what. A part of the window that would
//! take it (and highlights itself) calls [`accept`] with its own name, the
//! type it wants and the action; the drag's cursor follows. On release the
//! kit reads the data off the UI thread and calls [`crate::App::dropped`]
//! with a [`Drop`] naming that part.

use std::sync::Arc;

use egui::{Context, Id, Pos2};

/// What can be done with dragged data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Actions {
    pub copy: bool,
    pub move_: bool,
    /// Let the place it is dropped on ask (a menu of Move/Copy/Link, as a
    /// right-button drag in BeOS's Tracker does).
    pub ask: bool,
}

impl Actions {
    pub const COPY: Actions = Actions { copy: true, move_: false, ask: false };
    pub const MOVE: Actions = Actions { copy: false, move_: true, ask: false };
    pub const COPY_OR_MOVE: Actions = Actions { copy: true, move_: true, ask: false };
    pub const ALL: Actions = Actions { copy: true, move_: true, ask: true };

    pub fn allows(self, action: Action) -> bool {
        match action {
            Action::Copy => self.copy,
            Action::Move => self.move_,
            Action::Ask => self.ask,
        }
    }
}

/// The one action a drop does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Copy,
    Move,
    Ask,
}

/// A picture to drag under the pointer.
#[derive(Clone)]
pub struct DragIcon {
    /// In device pixels at `scale`.
    pub image: egui::ColorImage,
    pub scale: u32,
    /// Where the pointer is on it, in logical pixels from its top left.
    pub hotspot: egui::Vec2,
}

/// What a drag carries: the same data in each type, most faithful first.
#[derive(Clone)]
pub struct DragData {
    pub types: Vec<(String, Arc<[u8]>)>,
    pub actions: Actions,
    pub icon: Option<DragIcon>,
}

impl DragData {
    /// Files: `text/uri-list` (file:// URIs, what file managers, browsers
    /// and editors take) and plain text (the paths, a line each, for
    /// terminals and text fields).
    pub fn files(paths: &[std::path::PathBuf], actions: Actions) -> DragData {
        let uris: String = paths.iter().map(|p| format!("{}\r\n", file_uri(p))).collect();
        let text = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
        DragData {
            types: vec![
                ("text/uri-list".into(), Arc::from(uris.into_bytes())),
                ("text/plain;charset=utf-8".into(), Arc::from(text.clone().into_bytes())),
                ("text/plain".into(), Arc::from(text.into_bytes())),
            ],
            actions,
            icon: None,
        }
    }

    /// Text.
    pub fn text(text: &str) -> DragData {
        DragData {
            types: vec![
                ("text/plain;charset=utf-8".into(), Arc::from(text.as_bytes())),
                ("text/plain".into(), Arc::from(text.as_bytes())),
                ("UTF8_STRING".into(), Arc::from(text.as_bytes())),
            ],
            actions: Actions::COPY,
            icon: None,
        }
    }

    pub fn icon(mut self, icon: DragIcon) -> DragData {
        self.icon = Some(icon);
        self
    }

    /// Says which folder the files come from (a type naming it, with no
    /// data), so a folder window can refuse them being dropped back where
    /// they are ([`Hover::from_folder`]) before the drop.
    pub fn from_folder(self, dir: &std::path::Path) -> DragData {
        self.with(&folder_type(dir), Vec::new())
    }

    /// Asks a folder to make links to the files rather than copies (an
    /// application dragged out of Deskbar).
    pub fn as_link(self) -> DragData {
        self.with(LINK_TYPE, Vec::new())
    }

    /// Adds a type only Olo's apps know (put first: it is the most
    /// faithful).
    pub fn with(mut self, mime: &str, data: impl Into<Arc<[u8]>>) -> DragData {
        self.types.insert(0, (mime.into(), data.into()));
        self
    }
}

/// The type (with no data) that [`DragData::as_link`] adds.
pub const LINK_TYPE: &str = "application/x-olo-make-link";

/// Something being dragged over a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    /// Where, in window coordinates.
    pub pos: Pos2,
    /// What it can be had as.
    pub types: Vec<String>,
    /// What the source allows.
    pub actions: Actions,
    /// Started in this window (with [`crate::Cx::start_drag`]).
    pub from_here: bool,
    /// Started by this app (any of its windows).
    pub from_us: bool,
}

impl Hover {
    pub fn has(&self, mime: &str) -> bool {
        self.types.iter().any(|t| t == mime)
    }

    /// The first of `wanted` that it can be had as.
    pub fn best<'a>(&self, wanted: &[&'a str]) -> Option<&'a str> {
        wanted.iter().copied().find(|w| self.has(w))
    }

    /// The files come from folder `dir` (see [`DragData::from_folder`]).
    pub fn from_folder(&self, dir: &std::path::Path) -> bool {
        self.has(&folder_type(dir))
    }

    /// Files (or URIs).
    pub fn has_files(&self) -> bool {
        self.has("text/uri-list")
    }

    /// Text in any of the usual types.
    pub fn text_type(&self) -> Option<&'static str> {
        self.best(&["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"])
    }

    /// A file's contents offered under its name (what Chrome and Electron
    /// drag a picture out of a page as: `application/octet-stream;name="sun.png"`,
    /// with no `text/uri-list`): the type, to accept; [`named_file`] reads
    /// the name.
    pub fn named_file_type(&self) -> Option<&str> {
        self.types.iter().find(|t| named_file(t).is_some()).map(String::as_str)
    }
}

pub use olo_kit_base::files::{moz_url, named_file};

/// The type naming a folder (a hash of its path: types are short names).
fn folder_type(dir: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut h);
    format!("application/x-olo-from-folder-{:016x}", h.finish())
}

/// A drop: what [`accept`] asked for, read in full.
#[derive(Clone, Debug)]
pub struct Drop {
    /// The name the accepting part gave.
    pub target: String,
    pub mime: String,
    pub data: Vec<u8>,
    pub action: Action,
    pub pos: Pos2,
    pub from_here: bool,
    pub from_us: bool,
    /// All the types it came in (e.g. an Olo-only type that says how
    /// it was dragged).
    pub types: Vec<String>,
}

impl Drop {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    /// The files should be linked to, not copied ([`DragData::as_link`]).
    pub fn wants_link(&self) -> bool {
        self.types.iter().any(|t| t == LINK_TYPE)
    }

    /// The local paths in a `text/uri-list` (others are left out).
    pub fn paths(&self) -> Vec<std::path::PathBuf> {
        uri_list_paths(&self.text())
    }
}

/// What a window's part asked for in this frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Accepted {
    pub target: String,
    pub mime: String,
    pub action: Action,
}

fn hover_id() -> Id {
    Id::unique("olo-dnd-hover")
}

fn accept_id() -> Id {
    Id::unique("olo-dnd-accept")
}

/// What is being dragged over this window now, if anything.
pub fn hover(ctx: &Context) -> Option<Hover> {
    ctx.data(|d| d.get_temp::<Option<Hover>>(hover_id())).flatten()
}

/// Take what is dragged over this window, here: `target` names the part
/// (it comes back in [`Drop::target`]), `mime` is one of [`Hover::types`],
/// `action` one the source allows. Call it every frame the drag is over
/// the part; a frame without it refuses the drag. The last call wins.
pub fn accept(ctx: &Context, target: impl Into<String>, mime: &str, action: Action) {
    let a = Accepted { target: target.into(), mime: mime.into(), action };
    ctx.data_mut(|d| d.insert_temp(accept_id(), Some(a)));
}

/// For a window that opens files dropped anywhere on it (a viewer, an
/// editor): takes files dragged from elsewhere as `target`, outlines the
/// window in the keyboard-navigation colour while they are over it, and
/// says whether it did. The drop comes to [`crate::App::dropped`];
/// [`Drop::paths`] lists them.
pub fn open_files(ui: &egui::Ui, target: &str) -> bool {
    let Some(h) = hover(ui.ctx()) else { return false };
    if !h.has_files() || h.from_here {
        return false;
    }
    accept(ui.ctx(), target, "text/uri-list", if h.actions.copy { Action::Copy } else { Action::Move });
    outline(ui, ui.ctx().content_rect());
    true
}

/// Show that `rect` (a view, a whole window) would take what is dragged
/// over it: a 2 px frame inside it in the keyboard-navigation colour,
/// above everything. (A single item, like a folder in a list, highlights
/// itself instead.)
pub fn outline(ui: &egui::Ui, rect: egui::Rect) {
    let p = crate::style::palette(ui.ctx());
    ui.ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("olo-dnd-frame")))
        .with_clip_rect(ui.clip_rect())
        .rect_stroke(rect.shrink(1.0), 0.0, egui::Stroke::new(2.0, crate::style::c(p.keyboard_navigation)), egui::StrokeKind::Inside);
}

/// Drops on the kit's own widgets (text into a text field) are named with
/// this; they go to the widget, not to [`crate::App::dropped`].
pub(crate) const WIDGET: &str = "olo-widget:";

fn start_id() -> Id {
    Id::unique("olo-dnd-start")
}

fn widget_drops_id() -> Id {
    Id::unique("olo-dnd-widget-drops")
}

/// Start dragging `data` out of this window from a widget, which has no
/// [`crate::Cx`] (what [`crate::Cx::start_drag`] does, after the frame).
pub fn start(ctx: &Context, data: DragData) {
    ctx.data_mut(|d| d.insert_temp(start_id(), Some(data)));
}

pub(crate) fn take_start(ctx: &Context) -> Option<DragData> {
    ctx.data_mut(|d| d.remove_temp::<Option<DragData>>(start_id())).flatten()
}

/// A drop for one of the kit's widgets, kept for it to take next frame.
pub(crate) fn deliver(ctx: &Context, drop: Drop) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Drop>>(widget_drops_id()).push(drop));
    ctx.request_repaint();
}

/// The drop on the widget that accepted as `target` (with [`WIDGET`]).
pub(crate) fn take_widget_drop(ctx: &Context, target: &str) -> Option<Drop> {
    ctx.data_mut(|d| {
        let drops = d.get_temp_mut_or_default::<Vec<Drop>>(widget_drops_id());
        let i = drops.iter().position(|x| x.target == target)?;
        Some(drops.remove(i))
    })
}

pub(crate) fn begin_frame(ctx: &Context, hover: Option<Hover>) {
    ctx.data_mut(|d| {
        d.insert_temp(hover_id(), hover);
        d.insert_temp::<Option<Accepted>>(accept_id(), None);
    });
}

pub(crate) fn end_frame(ctx: &Context) -> Option<Accepted> {
    ctx.data(|d| d.get_temp::<Option<Accepted>>(accept_id())).flatten()
}

pub use olo_kit_base::files::{file_uri, uri_list_paths};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_files_and_links() {
        assert_eq!(named_file(r#"application/octet-stream;name="sun.png""#).as_deref(), Some("sun.png"));
        assert_eq!(named_file(r#"application/octet-stream;name="../../.bashrc""#).as_deref(), Some("bashrc"));
        assert_eq!(named_file("application/octet-stream"), None);
        assert_eq!(moz_url(b"https://example.com/doc\nA document"), Some(("https://example.com/doc".into(), "A document".into())));
        let utf16: Vec<u8> = "https://a.org/\nTitle".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(moz_url(&utf16), Some(("https://a.org/".into(), "Title".into())));
    }

    #[test]
    fn uris_round_trip() {
        let paths = vec![std::path::PathBuf::from("/home/me/My Photos/été #1.png"), std::path::PathBuf::from("/tmp/a%b")];
        let data = DragData::files(&paths, Actions::COPY);
        let list = String::from_utf8(data.types[0].1.to_vec()).unwrap();
        assert!(list.starts_with("file:///home/me/My%20Photos/%C3%A9t%C3%A9%20%231.png\r\n"), "{list}");
        assert_eq!(uri_list_paths(&list), paths);
        assert_eq!(uri_list_paths("# comment\nfile://localhost/etc/hosts\nhttps://example.com/\n"), vec![std::path::PathBuf::from("/etc/hosts")]);
    }
}
