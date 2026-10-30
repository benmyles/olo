//! The AppChooser portal (`org.freedesktop.impl.portal.AppChooser`): which
//! app opens a file or a link, when xdg-desktop-portal's OpenURI asks (a
//! Flatpak opening a document, a type with no default, an app asking to
//! choose). As BeOS's Open With window: what is being opened, a list of
//! the apps that handle it (icon, name, what kind of app), the one chosen
//! last selected; Open (or a double-click, or Enter) answers with the app,
//! Cancel or Escape with none. The list follows UpdateChoices (an app
//! installed meanwhile), the selection kept. xdg-desktop-portal launches
//! the app and remembers the choice.

use std::collections::HashMap;

use olo_kit::{
    Cx, WindowId, WindowOptions,
    egui::{self, Align, Rect, pos2, vec2},
    icons,
    metrics as m,
    style::{self, c},
    widgets::{self, Column, ColumnListView, ListState},
};
use olo_xdg::desktop_entry;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::portal::ToUi;

/// An app offered, as Olo shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    /// Its desktop entry's id (what the portal answers with).
    pub id: String,
    pub name: String,
    /// What kind of app it is ("Web Browser"), if it says.
    pub kind: String,
    pub icon: String,
}

/// The apps `ids` names (desktop entry ids), as Olo shows them. It reads
/// every entry: call it off the UI thread.
pub fn offers(ids: &[String]) -> Vec<Offer> {
    let all = desktop_entry::all();
    ids.iter()
        .map(|id| match desktop_entry::find_among(&all, id) {
            // Olo's own (`olo.viewer`, the entries olo-session writes), by
            // Olo's name and icon.
            e if let Some((name, icon)) = id.starts_with("olo.").then(|| olo_xdg::apps::our_app(id)).flatten() => {
                Offer { id: id.clone(), name: name.into(), kind: e.and_then(|e| e.generic_name.clone()).unwrap_or_default(), icon: icon.into() }
            }
            Some(e) => Offer {
                id: id.clone(),
                name: e.name.clone(),
                kind: e.generic_name.clone().or_else(|| e.comment.clone()).unwrap_or_default(),
                icon: e.icon.clone().filter(|i| !i.is_empty()).unwrap_or_else(|| "application".into()),
            },
            None => Offer { id: id.clone(), name: id.rsplit('.').next().unwrap_or(id).to_owned(), kind: String::new(), icon: "application".into() },
        })
        .collect()
}

/// What the chooser was asked (from the portal).
pub struct Ask {
    pub request: String,
    pub parent: String,
    /// What is opened: a file's name, or the link.
    pub what: String,
    pub offers: Vec<Offer>,
    /// The app chosen last time, if any.
    pub last: String,
    /// The answer: the app's id, or None if cancelled.
    pub reply: async_channel::Sender<Option<String>>,
}

pub struct AppChooser {
    pub to_ui: std::sync::Arc<dyn Fn(ToUi) + Send + Sync>,
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

/// How to name what is opened: a file by its name, a `file:` link by its
/// file's, any other link whole.
pub fn what(filename: &str, uri: &str) -> String {
    if !filename.is_empty() {
        return std::path::Path::new(filename).file_name().map_or_else(|| filename.to_owned(), |n| n.to_string_lossy().into_owned());
    }
    match olo_xdg::open::Target::parse(uri) {
        olo_xdg::open::Target::Path(p) => p.file_name().map_or_else(|| uri.to_owned(), |n| n.to_string_lossy().into_owned()),
        olo_xdg::open::Target::Url(u) => u,
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.AppChooser")]
impl AppChooser {
    async fn choose_application(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        choices: Vec<String>,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let what = what(&string(&options, "filename"), &string(&options, "uri"));
        let offers = blocking::unblock(move || offers(&choices)).await;
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Choose(Ask { request: handle.to_string(), parent: parent_window, what, offers, last: string(&options, "last_choice"), reply: tx }));
        let chosen = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        match chosen {
            Some(id) => (0, HashMap::from([("choice".to_owned(), OwnedValue::try_from(Value::from(id)).expect("a string"))])),
            None => (1, HashMap::new()),
        }
    }

    async fn update_choices(&self, handle: OwnedObjectPath, choices: Vec<String>) {
        let offers = blocking::unblock(move || offers(&choices)).await;
        (self.to_ui)(ToUi::Offers(handle.to_string(), offers));
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

/// The chooser on screen.
pub struct Dialog {
    ask: Ask,
    list: ListState,
}

const SIZE: (u32, u32) = (440, 340);

/// The app chosen last (the default) at the top, as BeOS's Open With put
/// the preferred app first.
fn first(offers: &mut [Offer], last: &str) {
    if let Some(i) = offers.iter().position(|o| o.id == last) {
        offers[..=i].rotate_right(1);
    }
}

impl Dialog {
    pub fn open(cx: &mut Cx, mut ask: Ask) -> (WindowId, Dialog) {
        let mut options = WindowOptions { dialog: true, ..WindowOptions::new("Open With", SIZE.0, SIZE.1).min_size(340, 240) };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        let mut list = ListState::default();
        first(&mut ask.offers, &ask.last);
        list.select(0);
        (id, Dialog { ask, list })
    }

    pub fn request(&self) -> &str {
        &self.ask.request
    }

    /// The apps offered changed: shown, the selection kept.
    pub fn offer(&mut self, offers: Vec<Offer>) {
        let chosen = self.list.single().and_then(|i| self.ask.offers.get(i)).map(|o| o.id.clone());
        self.ask.offers = offers;
        first(&mut self.ask.offers, &self.ask.last);
        self.list.select(chosen.and_then(|id| self.ask.offers.iter().position(|o| o.id == id)).unwrap_or(0));
    }

    /// Answer (None: cancelled); the caller closes the window.
    pub fn answer(&self, chosen: Option<String>) {
        let _ = self.ask.reply.try_send(chosen);
    }

    /// Draw it; Some(the app, or None if cancelled) once answered.
    pub fn ui(&mut self, ui: &mut egui::Ui, win: WindowId) -> Option<Option<String>> {
        let p = style::palette(ui.ctx());
        let full = ui.max_rect().shrink(m::WINDOW_INSET);
        // What is opened, on one line (cut short if it must be).
        let head = Rect::from_min_size(full.min, vec2(full.width(), m::LABEL_ROW));
        let mut job = egui::text::LayoutJob::simple_singleline(format!("Open “{}” with:", self.ask.what), style::bold_font(), c(p.panel_text));
        job.wrap = egui::text::TextWrapping::truncate_at_width(head.width());
        let g = ui.painter().layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, head.left(), head.center().y, style::PLAIN_SIZE);
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &format!("Open “{}” with:", self.ask.what), head, None);
        ui.painter().galley(pos, g, c(p.panel_text));

        let list_r = Rect::from_min_max(pos2(full.left(), head.bottom() + m::TIGHT), pos2(full.right(), full.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        // (The last column fills what the first leaves.)
        let columns = [Column::new("Name", (list_r.width() * 0.5).round()), Column::new("Kind", 100.0)];
        let offers = &self.ask.offers;
        let name = |i: usize| offers[i].name.as_str();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_r));
        let resp = ColumnListView::new(("chooser", win), &columns, offers.len(), &mut self.list)
            .empty_text("No apps open this.")
            .type_select(&name)
            .autofocus(true)
            .show(&mut child, |cell| {
                let Some(o) = offers.get(cell.row) else { return };
                match cell.column {
                    0 => {
                        let ir = Rect::from_center_size(pos2(cell.rect.left() + m::CELL_INSET + 8.0, cell.rect.center().y), vec2(16.0, 16.0));
                        if !icons::paint(cell.painter, ir, &o.icon) {
                            icons::paint(cell.painter, ir, "application");
                        }
                        let r = Rect::from_min_max(pos2(ir.right() + 2.0, cell.rect.top()), cell.rect.max);
                        let width = (r.width() - m::CELL_INSET).max(0.0);
                        style::label_fit(cell.painter, r.left() + m::TIGHT, r.center().y, Align::Min, &o.name, style::plain_font(), cell.text_color, width);
                    }
                    _ => cell.text(&o.kind),
                }
            });
        if let Some(i) = resp.activated
            && let Some(o) = self.ask.offers.get(i)
        {
            return Some(Some(o.id.clone()));
        }
        let chosen = self.list.single().and_then(|i| self.ask.offers.get(i)).map(|o| o.id.clone());
        match widgets::alert_buttons(ui, full, &["Cancel", "Open"], chosen.is_some()) {
            Some(1) => Some(chosen),
            Some(_) => Some(None),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_comes_first() {
        let offer = |id: &str| Offer { id: id.into(), name: id.into(), kind: String::new(), icon: String::new() };
        let mut offers = vec![offer("a"), offer("b"), offer("olo.viewer"), offer("c")];
        first(&mut offers, "olo.viewer");
        assert_eq!(offers.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["olo.viewer", "a", "b", "c"]);
        first(&mut offers, "missing");
        assert_eq!(offers[0].id, "olo.viewer");
    }

    #[test]
    fn what_is_opened() {
        assert_eq!(what("/home/a/Report 2026.pdf", ""), "Report 2026.pdf");
        assert_eq!(what("", "file:///home/a/My%20Notes.md"), "My Notes.md");
        assert_eq!(what("", "https://example.com/a?b"), "https://example.com/a?b");
        assert_eq!(what("", "mailto:someone@example.com"), "mailto:someone@example.com");
    }
}
