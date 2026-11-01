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

use olo_ui::{
    Frame, WindowOptions,
    gpui::{App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, WindowHandle, canvas, div, px},
    metrics as m,
    style::{self, Align},
    theme,
    widgets::{Button, Column, ColumnListView, ListModel, button_row},
};
use olo_xdg::desktop_entry;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{
    portal::ToUi,
    ui::{self, Answer},
};

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
    answer: Answer<Option<String>>,
    request: String,
    what: String,
    offers: Vec<Offer>,
    last: String,
    list: Entity<ListModel>,
}

const SIZE: (f32, f32) = (440.0, 340.0);

/// The app chosen last (the default) at the top, as BeOS's Open With put
/// the preferred app first.
fn first(offers: &mut [Offer], last: &str) {
    if let Some(i) = offers.iter().position(|o| o.id == last) {
        offers[..=i].rotate_right(1);
    }
}

/// Opens the chooser; its view too, for the apps offered as they change.
pub fn open(cx: &mut App, mut ask: Ask) -> Option<(WindowHandle<Frame>, WeakEntity<Dialog>)> {
    let options = ui::over(WindowOptions::new("Open With", SIZE.0, SIZE.1).min_size(340.0, 240.0).dialog(), &ask.parent);
    let mut made = None;
    let win = olo_ui::open_window(cx, options, |window, cx| {
        window.activate_window();
        first(&mut ask.offers, &ask.last);
        let list = ListModel::new(cx, false);
        list.update(cx, |l, _| l.state.select(0));
        list.read(cx).focus(window);
        let view = cx.new(|_| Dialog { answer: Answer::new(ask.reply, None), request: ask.request, what: ask.what, offers: ask.offers, last: ask.last, list });
        made = Some(view.downgrade());
        view
    })?;
    Some((win, made?))
}

impl Dialog {
    pub fn request(&self) -> &str {
        &self.request
    }

    /// The apps offered changed: shown, the selection kept.
    pub fn offer(&mut self, offers: Vec<Offer>, cx: &mut Context<Self>) {
        let chosen = self.list.read(cx).state.single().and_then(|i| self.offers.get(i)).map(|o| o.id.clone());
        self.offers = offers;
        first(&mut self.offers, &self.last);
        let at = chosen.and_then(|id| self.offers.iter().position(|o| o.id == id)).unwrap_or(0);
        self.list.update(cx, |l, _| l.state.select(at));
        cx.notify();
    }

    fn chosen(&self, cx: &App) -> Option<String> {
        self.list.read(cx).state.single().and_then(|i| self.offers.get(i)).map(|o| o.id.clone())
    }

    /// Answered (None: cancelled); the window goes.
    fn finish(&mut self, chosen: Option<String>, window: &mut Window) {
        self.answer.give(chosen);
        window.remove_window();
    }
}

impl Render for Dialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        // What is opened, on one line (cut short if it must be).
        let head = format!("Open “{}” with:", self.what);
        let heading = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let r = style::label_fit(window, cx, b.left(), b.center().y, Align::Left, head.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), b.size.width);
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(head.clone(), r));
            },
        )
        .w_full()
        .h(px(m::LABEL_ROW));
        // (The last column fills what the first leaves.)
        let width = SIZE.0 - 2.0 * m::WINDOW_INSET;
        let columns = [Column::new("Name", (width * 0.5).round()), Column::new("Kind", 100.0)];
        let offers: std::rc::Rc<Vec<Offer>> = std::rc::Rc::new(self.offers.clone());
        let (names, cells) = (offers.clone(), offers.clone());
        let list = ColumnListView::new("chooser", &self.list, columns, offers.len(), move |cell, window, cx| {
            let Some(o) = cells.get(cell.row) else { return };
            match cell.column {
                0 => cell.icon_text(window, cx, &o.icon, "application", o.name.clone()),
                _ => cell.text(window, cx, o.kind.clone()),
            }
        })
        .empty_text("No apps open this.")
        .type_select(move |i| SharedString::from(names.get(i).map(|o| o.name.clone()).unwrap_or_default()))
        .on_open({
            let me = me.clone();
            move |i, window, cx| {
                me.update(cx, |d, _| {
                    let id = d.offers.get(i).map(|o| o.id.clone());
                    if id.is_some() {
                        d.finish(id, window);
                    }
                })
            }
        })
        .on_select({
            let me = me.clone();
            move |_, cx| me.update(cx, |_, cx| cx.notify())
        });
        let chosen = self.chosen(cx);
        let cancel = Button::new("Cancel").on_click({
            let me = me.clone();
            move |window, cx| me.update(cx, |d, _| d.finish(None, window))
        });
        let open = Button::new("Open").default(true).enabled(chosen.is_some()).on_click({
            let me = me.clone();
            move |window, cx| {
                me.update(cx, |d, cx| {
                    let chosen = d.chosen(cx);
                    if chosen.is_some() {
                        d.finish(chosen, window);
                    }
                })
            }
        });
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|d, e: &olo_ui::gpui::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    d.finish(None, window);
                    cx.stop_propagation();
                }
            }))
            .child(heading)
            .child(div().mt(px(m::TIGHT)).flex_1().min_h_0().child(list))
            .child(div().mt(px(m::SPACING)).child(button_row().child(cancel).child(open)))
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
