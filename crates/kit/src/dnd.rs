//! Drag and drop between windows and apps, over Wayland's data device.
//!
//! As on BeOS, a drag carries its data in several types and offers some
//! actions, and the place it is dropped on picks the type and the action
//! it wants (the Be Book's "negotiated" drag and drop; see
//! vendor/bebook/DragAndDrop.md). Here the types are MIME types, the
//! actions Wayland's copy, move and ask, and it works the same within a
//! window, between Tabula's apps and with any other Wayland program.
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

    /// Adds a type only Tabula's apps know (put first: it is the most
    /// faithful).
    pub fn with(mut self, mime: &str, data: impl Into<Arc<[u8]>>) -> DragData {
        self.types.insert(0, (mime.into(), data.into()));
        self
    }
}

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

    /// Files (or URIs).
    pub fn has_files(&self) -> bool {
        self.has("text/uri-list")
    }

    /// Text in any of the usual types.
    pub fn text_type(&self) -> Option<&'static str> {
        self.best(&["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"])
    }
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
    /// All the types it came in (e.g. a Tabula-only type that says how
    /// it was dragged).
    pub types: Vec<String>,
}

impl Drop {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
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
    Id::unique("tabula-dnd-hover")
}

fn accept_id() -> Id {
    Id::unique("tabula-dnd-accept")
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
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("tabula-dnd-frame")))
        .with_clip_rect(ui.clip_rect())
        .rect_stroke(rect.shrink(1.0), 0.0, egui::Stroke::new(2.0, crate::style::c(p.keyboard_navigation)), egui::StrokeKind::Inside);
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

/// `file:///path`, with what URIs can't hold percent-encoded.
pub fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The local paths in a `text/uri-list`: one URI a line, `#` comments.
pub fn uri_list_paths(list: &str) -> Vec<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            // file://host/path: only this computer's.
            let path = if rest.starts_with('/') { rest } else { &rest[rest.find('/')?..] };
            let bytes = path.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%'
                    && i + 2 < bytes.len()
                    && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
                {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
