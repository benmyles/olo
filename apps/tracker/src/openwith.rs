//! Open With ▸ Other…: every app that can open files (`olo_xdg::open::
//! every_app`, Olo's own first), found by typing part of its name, and
//! "Always open files like this one with it" to make the choice the
//! kind's (`open::set_default`). The apps are read off the UI thread;
//! the list shows them as they come. A dialog over the window it was
//! asked in, as Go to File or Folder is.

use std::path::PathBuf;

use olo_kit_base::metrics as m;
use olo_ui::{
    gpui::{
        Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
    },
    widgets::{
        Button, CheckBox, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
};
use olo_xdg::open::{self, Handler};

/// The window's size: the field, a screen of apps, the buttons.
pub const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 420.0;

/// An app as the list shows it: its id, name and icon.
#[derive(Clone)]
struct App {
    id: String,
    name: String,
    icon: String,
}

impl App {
    fn of(h: &Handler) -> Option<App> {
        let icon = match h {
            Handler::Ours(bin) => olo_xdg::apps::app_id_of(bin).and_then(|id| olo_xdg::apps::our_app(&id)).map_or("application", |(_, i)| i).to_owned(),
            Handler::Entry(e) => e.icon.clone().unwrap_or_else(|| "application".into()),
        };
        Some(App { id: open::handler_id(h)?, name: h.name(), icon })
    }
}

pub struct OpenWithWin {
    /// What's opened, and the kind they are.
    paths: Vec<PathBuf>,
    kind: String,
    field: Entity<TextField>,
    list: Entity<ListModel>,
    /// Every app (None: still reading them), and those the typing finds.
    apps: Option<Vec<App>>,
    shown: Vec<usize>,
    always: bool,
    _subs: Vec<Subscription>,
}

/// Opened, or not: the window closes.
pub enum OpenWithEvent {
    Done,
    Cancelled,
}

impl olo_ui::gpui::EventEmitter<OpenWithEvent> for OpenWithWin {}

impl OpenWithWin {
    pub fn new(kind: String, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> OpenWithWin {
        let field = text_field(cx, |f| f.hint("Part of an app's name").name("Find an app"));
        field.read(cx).focus(window);
        let list = ListModel::new(cx, false);
        let subs = vec![
            cx.subscribe(&field, |w: &mut OpenWithWin, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Changed => w.filter(cx),
                TextFieldEvent::Entered => w.open(cx),
                TextFieldEvent::Typed(_) => {}
            }),
            cx.observe(&list, |_, _, cx| cx.notify()),
        ];
        // (Every desktop entry is read: on a worker.)
        cx.spawn(async move |me, cx| {
            let apps = cx.background_executor().spawn(async { open::every_app().iter().filter_map(App::of).collect::<Vec<_>>() }).await;
            let _ = me.update(cx, |w, cx| {
                w.apps = Some(apps);
                w.filter(cx);
            });
        })
        .detach();
        OpenWithWin { paths, kind, field, list, apps: None, shown: Vec::new(), always: false, _subs: subs }
    }

    /// The apps whose names have every word typed, the first chosen.
    fn filter(&mut self, cx: &mut Context<Self>) {
        let typed = self.field.read(cx).text().to_lowercase();
        let words: Vec<&str> = typed.split_whitespace().collect();
        self.shown = self.apps.iter().flatten().enumerate().filter(|(_, a)| words.iter().all(|w| a.name.to_lowercase().contains(w))).map(|(i, _)| i).collect();
        let any = !self.shown.is_empty();
        self.list.update(cx, |l, _| {
            l.state.selected.clear();
            if any {
                l.state.select(0);
                l.reveal(0);
            }
        });
        cx.notify();
    }

    fn chosen(&self, cx: &Context<Self>) -> Option<App> {
        let i = self.list.read(cx).state.single()?;
        self.apps.as_ref()?.get(*self.shown.get(i)?).cloned()
    }

    /// ↑↓: the next app up or down.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let last = self.shown.len() as isize - 1;
        let at = self.list.read(cx).state.single().map_or(0, |i| (i as isize + by).clamp(0, last)) as usize;
        self.list.update(cx, |l, _| {
            l.state.select(at);
            l.reveal(at);
        });
        cx.notify();
    }

    /// Open with the app chosen (and, Always, make it the kind's).
    fn open(&mut self, cx: &mut Context<Self>) {
        let Some(app) = self.chosen(cx) else { return };
        crate::open_with_app(app.id, self.kind.clone(), self.paths.clone(), self.always);
        cx.emit(OpenWithEvent::Done);
    }
}

impl Render for OpenWithWin {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let (cell_me, open_me, button_me, cancel_me, always_me) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        let looking = self.apps.is_none();
        let list = ColumnListView::new("open-with-apps", &self.list, vec![Column::new("App", 120.0)], self.shown.len(), move |cell, window, cx| {
            let w = cell_me.read(cx);
            let Some(app) = w.shown.get(cell.row).and_then(|&i| w.apps.as_ref()?.get(i)).cloned() else { return };
            cell.icon_text(window, cx, &app.icon, "application", app.name);
        })
        .no_header()
        .empty_text(SharedString::from(if looking { "Looking for apps…" } else { "No app's name has that in it." }))
        .on_open(move |_, _, cx| open_me.update(cx, |w, cx| w.open(cx)));
        let name = self.paths.first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let always = if self.paths.len() == 1 { format!("Always open files like “{name}” with it") } else { "Always open files like these with it".to_owned() };
        let can_open = self.list.read(cx).state.single().is_some();
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .capture_key_down(cx.listener(|w, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "down" => w.step(1, cx),
                    "up" => w.step(-1, cx),
                    "escape" => cx.emit(OpenWithEvent::Cancelled),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(TextControl::new(self.field.clone()).label("Find:"))
            .child(div().flex_1().min_h_0().flex().flex_col().child(list))
            .child(CheckBox::new(always, self.always).on_toggle(move |on, _, cx| always_me.update(cx, |w, cx| {
                w.always = on;
                cx.notify();
            })))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(div().flex_1().child(StringView::new("↑↓ choose an app.").dim(true)))
                    .child(Button::new("Cancel").min_width(80.0).on_click(move |_, cx| cancel_me.update(cx, |_, cx| cx.emit(OpenWithEvent::Cancelled))))
                    .child(Button::new("Open").default(true).enabled(can_open).min_width(80.0).on_click(move |_, cx| button_me.update(cx, |w, cx| w.open(cx)))),
            )
    }
}
