//! Open With, the same wherever a file is: Tracker's windows, the
//! desktop, Quick Look and the file panels. [`menu`] is the submenu for
//! files of one kind (the apps worked out for it, `olo_xdg::open::
//! offer_cached`, the one it opens in marked; Other…; Always Open With ▸
//! to make another the kind's), its commands are [`is_command`]'s, and
//! [`run`] does what one asks. Other… is [`OpenWithWin`]: every app that
//! can open files (`olo_xdg::open::every_app`, Olo's own first), found by
//! typing part of its name, and "Always open files like this one with
//! it". BeOS: Tracker's Open With ▸ and its Open With window.
//!
//! The menu is built on the UI thread from what was worked out before
//! (`olo_xdg::open::warm`, run where the files were listed: off it);
//! finding an app, opening with it and writing a choice read and write
//! files, so they run on a thread.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px};
use olo_kit_base::metrics as m;
use olo_xdg::open::{self, Handler};

use crate::{
    WindowOptions,
    menu::Menu,
    widgets::{
        Button, CheckBox, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
};

/// Open With ▸ Other…: the window to choose any app.
pub const OTHER: &str = "open-with-other";

/// The kind files named so (`(name, is_dir)`) all are, for Open With
/// (None: none, or mixed).
pub fn kind_of<'a>(items: impl IntoIterator<Item = (&'a str, bool)>) -> Option<String> {
    let mut kinds = items.into_iter().map(|(name, is_dir)| open::kind_of(name, is_dir));
    let first = kinds.next()?;
    kinds.all(|k| k == first).then_some(first)
}

/// Open With ▸ for files of `kind`: the apps worked out for it (not
/// worked out yet, no submenu), the one it opens in marked, Other…, and
/// Always Open With ▸ to make another the kind's. No app knows the kind:
/// Other… alone.
pub fn menu(kind: &str) -> Option<Menu> {
    let offer = open::offer_cached(kind)?;
    let ids: Vec<(String, String)> = offer.handlers.iter().filter_map(|h| Some((open::handler_id(h)?, h.name()))).collect();
    if ids.is_empty() {
        return Some(Menu::new("Open With").item("Other…", OTHER));
    }
    let mut menu = Menu::new("Open With");
    for (id, name) in &ids {
        let label = if offer.default.as_ref() == Some(id) { format!("{name} (default)") } else { name.clone() };
        menu = menu.item(label, format!("open-with:{id}"));
    }
    let mut always = Menu::new("Always Open With");
    for (id, name) in &ids {
        always = always.item(name.clone(), format!("always-open-with:{id}")).checked(offer.default.as_ref() == Some(id));
    }
    Some(menu.separator().item("Other…", OTHER).submenu(always))
}

/// Whether `cmd` is one of Open With's.
pub fn is_command(cmd: &str) -> bool {
    cmd == OTHER || cmd.starts_with("open-with:") || cmd.starts_with("always-open-with:")
}

/// Open With's `cmd` for `paths` (of `kind`): opened with the app it
/// names (and, Always, the kind's from now on), or Other…'s window
/// opened, over the window in front.
pub fn run(cmd: &str, kind: String, paths: Vec<PathBuf>, cx: &mut App) {
    if cmd == OTHER {
        other(kind, paths, cx);
        return;
    }
    let always = cmd.starts_with("always-");
    let id = cmd.split_once(':').map(|(_, id)| id.to_owned()).unwrap_or_default();
    open_with(id, kind, paths, always);
}

/// Open `paths` with the app `id` names (`always`: and make it `kind`'s
/// from now on). On a thread: finding the app and writing the choice
/// read and write files.
pub fn open_with(id: String, kind: String, paths: Vec<PathBuf>, always: bool) {
    std::thread::spawn(move || {
        let Some(h) = open::handler_of_id(&id) else { return };
        if always {
            if let Err(e) = open::set_default(&kind, Some(&h)) {
                tracing::warn!("couldn't make {id} the app for {kind}: {e}");
            }
            open::warm([kind]);
        }
        if let Err(e) = open::open_with(&h, &paths) {
            tracing::warn!("open with {id}: {e}");
        }
    });
}

/// Other…'s window: a dialog over the window it was asked in, never a
/// tab.
pub fn window_options() -> WindowOptions {
    WindowOptions::new("Open With", WIDTH, HEIGHT).min_size(320.0, 260.0).dialog().child()
}

/// Other…'s window, opened and closed by itself (an app that keeps its
/// windows' list opens [`OpenWithWin`] in [`window_options`] itself).
pub fn other(kind: String, paths: Vec<PathBuf>, cx: &mut App) {
    let made: std::rc::Rc<std::cell::RefCell<Option<Entity<OpenWithWin>>>> = Default::default();
    let keep = made.clone();
    let Some(handle) = crate::open_window(cx, window_options(), move |window, cx| {
        let v = cx.new(|cx| OpenWithWin::new(kind, paths, window, cx));
        *keep.borrow_mut() = Some(v.clone());
        v
    }) else {
        return;
    };
    let Some(win) = made.borrow_mut().take() else { return };
    let any: gpui::AnyWindowHandle = handle.into();
    cx.subscribe(&win, move |_, _: &OpenWithEvent, cx| cx.close_window(any)).detach();
}

/// The window's size: the field, a screen of apps, the buttons.
pub const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 420.0;

/// An app as the list shows it: its id, name and icon.
#[derive(Clone)]
struct AppRow {
    id: String,
    name: String,
    icon: String,
}

impl AppRow {
    fn of(h: &Handler) -> Option<AppRow> {
        Some(AppRow { id: open::handler_id(h)?, name: h.name(), icon: h.icon() })
    }
}

pub struct OpenWithWin {
    /// What's opened, and the kind they are.
    paths: Vec<PathBuf>,
    kind: String,
    field: Entity<TextField>,
    list: Entity<ListModel>,
    /// Every app (None: still reading them), and those the typing finds.
    apps: Option<Vec<AppRow>>,
    shown: Vec<usize>,
    always: bool,
    _subs: Vec<Subscription>,
}

/// Opened, or not: the window closes.
pub enum OpenWithEvent {
    Done,
    Cancelled,
}

impl gpui::EventEmitter<OpenWithEvent> for OpenWithWin {}

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
            let apps = cx.background_executor().spawn(async { open::every_app().iter().filter_map(AppRow::of).collect::<Vec<_>>() }).await;
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

    fn chosen(&self, cx: &Context<Self>) -> Option<AppRow> {
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
        open_with(app.id, self.kind.clone(), self.paths.clone(), self.always);
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
            .child(CheckBox::new(always, self.always).on_toggle(move |on, _, cx| {
                always_me.update(cx, |w, cx| {
                    w.always = on;
                    cx.notify();
                })
            }))
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
