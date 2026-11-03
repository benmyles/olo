//! BeOS menus: a [`Menu`] of items (commands, separators, submenus),
//! shown as a popup over a window ([`popup`], [`context_menu`]): a
//! Wayland popup, so it can reach past the window's edge, which the
//! compositor keeps on screen (flipped or slid). Drawn as BeOS drew
//! them: the menu colour in a dark outline with a light inner edge
//! on top and left, the item under the pointer or chosen from the
//! keyboard in the selection colours, check marks, separators as a
//! groove, a triangle for a submenu, which opens beside its item. ↑ ↓
//! choose, → opens a submenu, ← and Escape close a level, Enter or Space
//! takes the item; a release over an item takes it. Agents read each
//! item (`MenuItem`, with whether it's checked or can be chosen). One
//! taller than its display scrolls, with an arrow at each end there's
//! more past (the pointer resting there scrolls it), the wheel and the
//! keys.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, ContentMask, Context, Corner, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Render, RenderOnce, ScrollWheelEvent, SharedString, Styled as _, Task, Window, WindowBounds, WindowHandle, WindowKind,
    canvas, div, point, px, size,
};

use crate::{
    agent,
    keys::{self, Shortcut},
    style, theme,
};

pub const ITEM_HEIGHT: f32 = 20.0;
const SEPARATOR_HEIGHT: f32 = 10.0;
const ITEM_LEFT: f32 = 20.0;
const ITEM_RIGHT: f32 = 14.0;
const SHORTCUT_GAP: f32 = 28.0;
const POPUP_BORDER: f32 = 1.0;
const POPUP_PAD: f32 = 4.0;
const MIN_WIDTH: f32 = 120.0;
/// A menu taller than its display: kept this far from the display's top
/// and bottom, with a scroll arrow this tall at each end there's more
/// past, which scrolls it this fast while the pointer rests on it.
const SCREEN_MARGIN: f32 = 24.0;
const ARROW_HEIGHT: f32 = 16.0;
const ARROW_SPEED: f32 = 480.0;
/// Item icons, and the room between one and its label.
const ICON_SIZE: f32 = 16.0;
const ICON_GAP: f32 = olo_kit_base::metrics::MARK_GAP;

/// Where a popup's item icons go (their left edge, if the menu has any)
/// and where the labels start, from the item's left. Check marks keep
/// their column in front of the icons when the menu has both.
fn columns(menu: &Menu) -> (Option<f32>, f32) {
    if !menu.has_icons() {
        return (None, ITEM_LEFT);
    }
    let checks = menu.items.iter().any(|item| matches!(item, MenuItem::Action(a) if a.checked));
    let icon = if checks { ITEM_LEFT } else { 6.0 };
    (Some(icon), icon + ICON_SIZE + ICON_GAP)
}

/// An item's icon in its column (the generic application's if there's
/// no such icon), faded when it can't be chosen.
fn item_icon(window: &mut Window, row: Bounds<Pixels>, left: f32, name: &str, enabled: bool) {
    let r = style::snap_centred(window, point(row.left() + px(left + ICON_SIZE / 2.0), row.center().y), ICON_SIZE, ICON_SIZE);
    window.with_element_opacity(Some(if enabled { 1.0 } else { 110.0 / 255.0 }), |window| {
        if !crate::icons::paint(window, r, name) {
            crate::icons::paint(window, r, "application");
        }
    });
}

/// A menu: its title (a menu bar's, a submenu's item) and items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    /// Drawn beside the title where this is a submenu (an icon name).
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Action(Action),
    Separator,
    Submenu(Menu),
}

/// A command in a menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Action {
    pub label: String,
    pub command: String,
    pub enabled: bool,
    pub checked: bool,
    /// Its keys as the app presses them (the command key is Ctrl):
    /// apps give the Mac's ([`Menu::key`]), [`resolve`] the profile's.
    pub shortcut: Option<Shortcut>,
    /// Its keys as shown ("⌘C", "Ctrl+C"): the shortcut's, or keys the
    /// window doesn't take itself (a desktop shortcut, a chord).
    pub keys: Option<String>,
    /// Drawn at its left (an icon name, as for `icons::paint`). A menu
    /// with any gives them all a column, between the check marks and the
    /// labels.
    pub icon: Option<String>,
    /// A colour it stands for, drawn as a swatch in the icons' column (a
    /// Color menu's choices).
    pub swatch: Option<Hsla>,
    /// It stands for a minimized window, in a list of windows: a hollow
    /// diamond where the check mark goes and a lighter label, as a Mac's
    /// Window menu shows one. It can still be chosen (to bring it back).
    pub minimized: bool,
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Menu {
        Menu { title: title.into(), items: Vec::new(), icon: None }
    }

    /// The last item's (an action's or a submenu's) icon.
    pub fn icon(mut self, name: impl Into<String>) -> Menu {
        match self.items.last_mut() {
            Some(MenuItem::Action(a)) => a.icon = Some(name.into()),
            Some(MenuItem::Submenu(m)) => m.icon = Some(name.into()),
            _ => {}
        }
        self
    }

    /// Whether any item has an icon, so the popup has an icon column.
    pub fn has_icons(&self) -> bool {
        self.items.iter().any(|item| match item {
            MenuItem::Action(a) => a.icon.is_some() || a.swatch.is_some(),
            MenuItem::Submenu(m) => m.icon.is_some(),
            MenuItem::Separator => false,
        })
    }

    /// Adds a command.
    pub fn item(mut self, label: impl Into<String>, command: impl Into<String>) -> Menu {
        self.items.push(MenuItem::Action(Action { label: label.into(), command: command.into(), enabled: true, ..Default::default() }));
        self
    }

    pub fn separator(mut self) -> Menu {
        self.items.push(MenuItem::Separator);
        self
    }

    /// As it's shown: no separator at either end or two together (as left
    /// where items a menu has only sometimes aren't there).
    fn tidied(mut self) -> Menu {
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            if matches!(item, MenuItem::Separator) && matches!(items.last(), None | Some(MenuItem::Separator)) {
                continue;
            }
            items.push(item);
        }
        if matches!(items.last(), Some(MenuItem::Separator)) {
            items.pop();
        }
        self.items = items;
        self
    }

    pub fn submenu(mut self, menu: Menu) -> Menu {
        self.items.push(MenuItem::Submenu(menu));
        self
    }

    /// Its items in alphabetical order by what they say (natural order,
    /// case aside: "Item 2" before "item 10"), each run between
    /// separators on its own, and its submenus' the same, all the way
    /// down (a list of apps, of panels).
    pub fn sorted(mut self) -> Menu {
        fn label(item: &MenuItem) -> &str {
            match item {
                MenuItem::Action(a) => &a.label,
                MenuItem::Submenu(m) => &m.title,
                MenuItem::Separator => "",
            }
        }
        for item in &mut self.items {
            if let MenuItem::Submenu(m) = item {
                *m = std::mem::replace(m, Menu::new("")).sorted();
            }
        }
        for run in self.items.split_mut(|i| matches!(i, MenuItem::Separator)) {
            run.sort_by(|a, b| olo_kit_base::files::natural_cmp(label(a), label(b)));
        }
        self
    }

    /// The last item's colour, shown as a swatch (see [`Action::swatch`]).
    pub fn swatch(mut self, color: Hsla) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.swatch = Some(color);
        }
        self
    }

    /// Marks the last item a minimized window (see [`Action::minimized`]).
    pub fn minimized(mut self, on: bool) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.minimized = on;
        }
        self
    }

    /// The last item checked or not.
    pub fn checked(mut self, on: bool) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.checked = on;
        }
        self
    }

    /// The last item can be chosen or not.
    pub fn enabled(mut self, on: bool) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.enabled = on;
        }
        self
    }

    /// The last item's keys, as shown ("⌘C"), when not a shortcut of
    /// the window's own (a desktop shortcut, a chord).
    pub fn keys(mut self, keys: impl Into<String>) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.keys = Some(keys.into());
        }
        self
    }

    /// The last item pressed with the command key and `key` (as GPUI
    /// names it: "w", "f3", "="): ⌘W on a Mac's keys, Ctrl+W on
    /// Windows'; the profile's keys for its command win ([`resolve`]).
    pub fn key(self, key: &str) -> Menu {
        self.shortcut(Shortcut::command(keys::evdev(key)))
    }

    /// With Shift too: ⇧⌘Z.
    pub fn shift_key(self, key: &str) -> Menu {
        self.shortcut(Shortcut::command(keys::evdev(key)).shifted())
    }

    /// Any chord: with Alt, or a key of its own.
    pub fn shortcut(mut self, shortcut: Shortcut) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.shortcut = Some(shortcut);
        }
        self
    }

    /// The command its keys press, searching submenus too.
    pub fn find_shortcut(&self, k: &gpui::Keystroke) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            MenuItem::Action(a) if a.enabled && a.shortcut.is_some_and(|s| keys::pressed(&s, k)) => Some(a.command.as_str()),
            MenuItem::Submenu(m) => m.find_shortcut(k),
            _ => None,
        })
    }

    /// The item for `command`, searching submenus too.
    pub fn find_action(&self, command: &str) -> Option<&Action> {
        self.items.iter().find_map(|item| match item {
            MenuItem::Action(a) if a.command == command => Some(a),
            MenuItem::Submenu(m) => m.find_action(command),
            _ => None,
        })
    }

    /// Gives every item without keys the keys its command has in `bar`
    /// (the window's menu bar), so a command shows its shortcut in a
    /// context menu too without the app saying it twice.
    pub fn keys_from(&mut self, bar: &[Menu]) {
        for item in &mut self.items {
            match item {
                MenuItem::Action(a) if a.shortcut.is_none() && a.keys.is_none() => {
                    if let Some(b) = bar.iter().find_map(|m| m.find_action(&a.command)) {
                        a.shortcut = b.shortcut;
                        a.keys = b.keys.clone();
                    }
                }
                MenuItem::Submenu(m) => m.keys_from(bar),
                _ => {}
            }
        }
    }

    fn any_enabled(&self) -> bool {
        self.items.iter().any(|i| match i {
            MenuItem::Action(a) => a.enabled,
            MenuItem::Submenu(m) => m.any_enabled(),
            MenuItem::Separator => false,
        })
    }

    /// Its commands for the command palette, flattened (`agent::set_menus`).
    pub fn entries(&self, app_id: &str) -> olo_kit_base::commands::Menus {
        fn walk(menu: &Menu, path: &mut Vec<String>, out: &mut Vec<olo_kit_base::commands::Entry>) {
            path.push(menu.title.clone());
            for item in &menu.items {
                match item {
                    MenuItem::Action(a) => {
                        out.push(olo_kit_base::commands::Entry { path: path.clone(), label: a.label.clone(), command: a.command.clone(), keys: a.keys.clone(), enabled: a.enabled, checked: a.checked })
                    }
                    MenuItem::Submenu(m) => walk(m, path, out),
                    MenuItem::Separator => {}
                }
            }
            path.pop();
        }
        let mut entries = Vec::new();
        walk(self, &mut Vec::new(), &mut entries);
        olo_kit_base::commands::Menus { app_id: app_id.into(), entries }
    }
}

/// Gives `menus`' items the keys their commands have in the keyboard
/// profile, as the platform writes them; a terminal-style window's
/// (`raw`) as it presses them. Every app's keys (Copy, Close) go to items
/// the app gave keys; the app's own in the profile go to its items
/// whether or not it did. The menu bar does this to its menus.
pub fn resolve(menus: &mut [Menu], raw: bool) {
    fn walk(items: &mut [MenuItem], raw: bool) {
        for item in items {
            match item {
                MenuItem::Action(a) => match olo_kit_base::keys::known(&a.command, raw) {
                    Some(known) if known.own || a.shortcut.is_some() => {
                        a.shortcut = known.keys.as_ref().map(|(s, _)| *s);
                        a.keys = known.keys.map(|(_, label)| label);
                    }
                    Some(_) => {}
                    None => {
                        if let Some(s) = a.shortcut.filter(|_| a.keys.is_none()) {
                            a.keys = Some(s.label());
                            if cfg!(debug_assertions) {
                                olo_kit_base::keys::unlisted(&a.command, &s);
                            }
                        }
                    }
                },
                MenuItem::Submenu(m) => walk(&mut m.items, raw),
                MenuItem::Separator => {}
            }
        }
    }
    for m in menus {
        walk(&mut m.items, raw);
    }
}

/// The command `k` presses in `menus`, if any.
pub fn find_shortcut(menus: &[Menu], k: &gpui::Keystroke) -> Option<String> {
    menus.iter().find_map(|m| m.find_shortcut(k)).map(str::to_owned)
}

/// What to do with the command chosen, in the window the menu opened over.
pub type OnChoose = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// A popup's size for `menu`, measured with `window`'s text.
fn popup_size(window: &mut Window, menu: &Menu) -> (f32, f32) {
    let (mut label_w, mut key_w, mut h) = (0.0f32, 0.0f32, 0.0f32);
    let font = style::ui_font(FontWeight::NORMAL);
    for item in &menu.items {
        match item {
            MenuItem::Action(a) => {
                label_w = label_w.max(f32::from(style::text_width(window, a.label.clone(), style::PLAIN_SIZE, font.clone())));
                if let Some(keys) = &a.keys {
                    key_w = key_w.max(style::key_combo_width(window, keys));
                }
                h += ITEM_HEIGHT;
            }
            MenuItem::Submenu(m) => {
                label_w = label_w.max(f32::from(style::text_width(window, m.title.clone(), style::PLAIN_SIZE, font.clone())));
                key_w = key_w.max(10.0);
                h += ITEM_HEIGHT;
            }
            MenuItem::Separator => h += SEPARATOR_HEIGHT,
        }
    }
    let gap = if key_w > 0.0 { SHORTCUT_GAP } else { 0.0 };
    // On the two-point grid: whole device pixels at 1×, 1.5× and 2×.
    let w = ((columns(menu).1 + label_w + gap + key_w + ITEM_RIGHT).max(MIN_WIDTH) / 2.0).ceil() * 2.0;
    (w + 2.0 * POPUP_BORDER, h + 2.0 * POPUP_PAD)
}

/// Opens `menu` over `window`, its top left at `corner` of `anchor` (in
/// the window's content). `on_choose` gets the command chosen, in
/// `window`.
pub fn popup(window: &mut Window, cx: &mut App, menu: Menu, anchor: Bounds<Pixels>, corner: Corner, on_choose: impl Fn(&str, &mut Window, &mut App) + 'static) -> Option<WindowHandle<MenuPopup>> {
    open_first(window, cx, menu, Place { anchor, corner, ..Place::default() }, Rc::new(on_choose), None)
}

/// [`popup`], and `on_close` once it has gone, chosen from or not (after
/// `on_choose`): what opened it shows it open until then (Deskbar's
/// rows).
pub fn popup_then(
    window: &mut Window,
    cx: &mut App,
    menu: Menu,
    anchor: Bounds<Pixels>,
    corner: Corner,
    on_choose: impl Fn(&str, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&mut App) + 'static,
) -> Option<WindowHandle<MenuPopup>> {
    open_first(window, cx, menu, Place { anchor, corner, ..Place::default() }, Rc::new(on_choose), Some(Rc::new(on_close)))
}

/// Opens `menu` over `field` (in the window's content) with item
/// `current` lying over it, chosen, as a menu field's on BeOS, and at
/// least as wide as the field.
pub fn popup_over(window: &mut Window, cx: &mut App, menu: Menu, field: Bounds<Pixels>, current: usize, on_choose: impl Fn(&str, &mut Window, &mut App) + 'static) -> Option<WindowHandle<MenuPopup>> {
    let dy = POPUP_PAD + item_offset(&menu, current) - (f32::from(field.size.height) - ITEM_HEIGHT) / 2.0;
    let place = Place { anchor: field, corner: Corner::TopLeft, offset: point(px(0.0), px(-dy.round())), min_width: f32::from(field.size.width).round(), selected: Some(current) };
    open_first(window, cx, menu, place, Rc::new(on_choose), None)
}

/// The menu open over each window (not a menu bar's, which keeps its
/// own), from where, and the one a press in its window just put away.
#[derive(Default)]
struct Popped {
    menus: std::collections::HashMap<gpui::WindowId, (WindowHandle<MenuPopup>, Bounds<Pixels>)>,
    closed: Option<(gpui::WindowId, Bounds<Pixels>)>,
}

impl gpui::Global for Popped {}

/// A press in `window` (its frame sees it before anything in it does):
/// a menu open over it goes, as a press outside a menu puts it away
/// (the compositor hands an app's own window the presses outside its
/// menu); and that press doesn't open it again, on what opened it.
pub(crate) fn press_in(at: gpui::Point<Pixels>, window: &Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let popped = cx.default_global::<Popped>();
    let taken = popped.menus.remove(&id);
    popped.closed = taken.as_ref().map(|(_, anchor)| (id, *anchor));
    if let Some((handle, _)) = taken {
        let _ = handle.update(cx, |m, window, cx| m.close(window, cx));
    }
    // The menu bar's menu too: a press anywhere in the window puts it
    // away, its own title's included (which then leaves it away), but
    // for the first press on the title of one that slid open.
    let open = cx.default_global::<Bars>().0.get_mut(&id).and_then(|bar| {
        bar.closed = None;
        let (i, _) = bar.open?;
        if !bar.pressed && bar.titles.get(i).is_some_and(|t| t.contains(&at)) {
            bar.pressed = true;
            return None;
        }
        bar.closed = Some(i);
        bar.open.take()
    });
    if let Some((_, handle)) = open {
        let _ = handle.update(cx, |m, window, cx| m.close(window, cx));
    }
}

/// Opens a first menu over the window (see [`Popped`]).
fn open_first(window: &mut Window, cx: &mut App, menu: Menu, place: Place, on_choose: OnChoose, on_close: Option<Rc<dyn Fn(&mut App)>>) -> Option<WindowHandle<MenuPopup>> {
    let id = window.window_handle().window_id();
    let popped = cx.default_global::<Popped>();
    // The press that put it away, on what opened it: away it stays.
    if popped.closed.take() == Some((id, place.anchor)) {
        if let Some(f) = on_close {
            f(cx);
        }
        return None;
    }
    if let Some((old, _)) = popped.menus.remove(&id) {
        let _ = old.update(cx, |m, window, cx| m.close(window, cx));
    }
    let anchor = place.anchor;
    let handle = open(window, cx, menu, place, on_choose, None, on_close)?;
    cx.default_global::<Popped>().menus.insert(id, (handle, anchor));
    if let Ok(view) = handle.update(cx, |_, _, cx| cx.entity()) {
        cx.observe_release(&view, move |_, cx| {
            let popped = cx.default_global::<Popped>();
            if popped.menus.get(&id).is_some_and(|(h, _)| *h == handle) {
                popped.menus.remove(&id);
            }
        })
        .detach();
    }
    Some(handle)
}

/// Where item `i` begins below the menu's first.
fn item_offset(menu: &Menu, i: usize) -> f32 {
    menu.items.iter().take(i).map(|it| if matches!(it, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT }).sum()
}

/// Where a popup opens and how it starts.
#[derive(Clone, Copy)]
struct Place {
    anchor: Bounds<Pixels>,
    corner: Corner,
    offset: gpui::Point<Pixels>,
    min_width: f32,
    selected: Option<usize>,
}

impl Default for Place {
    fn default() -> Place {
        Place { anchor: Bounds::default(), corner: Corner::TopLeft, offset: gpui::Point::default(), min_width: 0.0, selected: None }
    }
}

/// A context menu at `position` (the pointer, in the window's content).
pub fn context_menu(window: &mut Window, cx: &mut App, menu: Menu, position: gpui::Point<Pixels>, on_choose: impl Fn(&str, &mut Window, &mut App) + 'static) -> Option<WindowHandle<MenuPopup>> {
    let anchor = Bounds { origin: position, size: size(px(1.0), px(1.0)) };
    popup(window, cx, menu, anchor, Corner::TopLeft, on_choose)
}

/// [`context_menu`], and `on_close` once it has gone, chosen from or not
/// (after `on_choose`): for a menu that is all its window is for (a
/// window's menu over a clear layer).
pub fn context_menu_then(
    window: &mut Window,
    cx: &mut App,
    menu: Menu,
    position: gpui::Point<Pixels>,
    on_choose: impl Fn(&str, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&mut App) + 'static,
) -> Option<WindowHandle<MenuPopup>> {
    let anchor = Bounds { origin: position, size: size(px(1.0), px(1.0)) };
    open_first(window, cx, menu, Place { anchor, ..Place::default() }, Rc::new(on_choose), Some(Rc::new(on_close)))
}

fn open(
    window: &mut Window,
    cx: &mut App,
    menu: Menu,
    place: Place,
    on_choose: OnChoose,
    parent_menu: Option<WindowHandle<MenuPopup>>,
    on_close: Option<Rc<dyn Fn(&mut App)>>,
) -> Option<WindowHandle<MenuPopup>> {
    // (A menu field's items stay as counted: one of them lies over it.)
    let menu = if place.selected.is_none() { menu.tidied() } else { menu };
    let (w, full_h) = popup_size(window, &menu);
    let w = w.max(place.min_width);
    let Place { anchor, corner, mut offset, selected, .. } = place;
    // Taller than the display: as tall as it allows, its items scrolled
    // (the one chosen in view: a pop-up field's over the field still).
    let room = window.display(cx).map_or(f32::MAX, |d| f32::from(d.bounds().size.height) - 2.0 * SCREEN_MARGIN);
    let h = if full_h <= room {
        full_h
    } else {
        // Whole items and one arrow: at either end, nothing half shown.
        let n = ((room - 2.0 * POPUP_PAD - ARROW_HEIGHT) / ITEM_HEIGHT).floor().max(4.0);
        2.0 * POPUP_PAD + ARROW_HEIGHT + n * ITEM_HEIGHT
    };
    let content = full_h - 2.0 * POPUP_PAD;
    let mut scroll = 0.0;
    if h < full_h
        && let Some(i) = selected
    {
        let view = h - 2.0 * POPUP_PAD - 2.0 * ARROW_HEIGHT;
        scroll = (item_offset(&menu, i) - (view - ITEM_HEIGHT) / 2.0).clamp(0.0, content - (h - 2.0 * POPUP_PAD));
        offset.y += px(scroll);
    }
    let owner = parent_menu.as_ref().map_or_else(|| window.window_handle(), |_| window.window_handle());
    let origin = parent_menu.and_then(|p| p.update(cx, |m, _, _| m.origin).ok()).unwrap_or(owner);
    let app_id = crate::app::app_id(cx);
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.0), px(0.0)), size: size(px(w), px(h)) })),
            kind: WindowKind::Popup { parent: owner, anchor, corner, offset },
            app_id: Some(app_id),
            focus: true,
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| MenuPopup {
                menu,
                selected,
                focus: cx.focus_handle(),
                on_choose,
                origin,
                owner,
                sub: None,
                rows: Rc::default(),
                pressed_inside: false,
                scroll,
                content,
                view_h: h - 2.0 * POPUP_PAD,
                scrolling: None,
                pointer: None,
            });
            window.focus(&view.focus_handle(cx));
            if let Some(f) = on_close {
                cx.observe_release(&view, move |_, cx| f(cx)).detach();
            }
            view
        },
    )
    .ok()
}

/// A menu showing.
pub struct MenuPopup {
    menu: Menu,
    selected: Option<usize>,
    focus: FocusHandle,
    on_choose: OnChoose,
    /// The window the first menu opened over, where commands go.
    origin: AnyWindowHandle,
    /// The window this one opened over (that window, or a menu).
    owner: AnyWindowHandle,
    /// The submenu open from an item.
    sub: Option<(usize, WindowHandle<MenuPopup>)>,
    rows: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// A press began in it (a click, rather than the release of the
    /// press that opened it).
    pressed_inside: bool,
    /// Scrolled this far down its items (a menu taller than its display),
    /// of `content` in all, `view_h` showing (arrows included).
    scroll: f32,
    content: f32,
    view_h: f32,
    /// Scrolling while the pointer rests on an arrow: which way, and the
    /// ticks doing it; and where the pointer last was (the item under it
    /// is the one lit as the items scroll by).
    scrolling: Option<(f32, Task<()>)>,
    pointer: Option<gpui::Point<Pixels>>,
}

impl Focusable for MenuPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MenuPopup {
    fn selectable(&self, i: usize) -> bool {
        match &self.menu.items[i] {
            MenuItem::Action(a) => a.enabled,
            MenuItem::Submenu(m) => m.any_enabled(),
            MenuItem::Separator => false,
        }
    }

    fn step(&self, dir: isize) -> Option<usize> {
        let n = self.menu.items.len() as isize;
        let mut i = self.selected.map(|i| i as isize).unwrap_or(if dir > 0 { -1 } else { n });
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.selectable(i as usize) {
                return Some(i as usize);
            }
        }
        None
    }

    fn row_at(&self, p: gpui::Point<Pixels>) -> Option<usize> {
        // (Not under a scroll arrow.)
        if self.arrow_at(p).is_some() {
            return None;
        }
        self.rows.borrow().iter().position(|r| r.contains(&p))
    }

    /// How far it can scroll.
    fn max_scroll(&self) -> f32 {
        (self.content - self.view_h).max(0.0)
    }

    /// The arrows shown: more above, more below.
    fn arrows(&self) -> (bool, bool) {
        (self.scroll > 0.5, self.scroll < self.max_scroll() - 0.5)
    }

    /// The scroll arrow under `p` (window coordinates): -1 up, 1 down.
    fn arrow_at(&self, p: gpui::Point<Pixels>) -> Option<f32> {
        let (up, down) = self.arrows();
        let y = f32::from(p.y);
        if up && y < POPUP_PAD + ARROW_HEIGHT {
            Some(-1.0)
        } else if down && y > POPUP_PAD + self.view_h - ARROW_HEIGHT {
            Some(1.0)
        } else {
            None
        }
    }

    fn scroll_to(&mut self, to: f32) -> bool {
        let to = to.clamp(0.0, self.max_scroll());
        let moved = (to - self.scroll).abs() > 0.01;
        self.scroll = to;
        if moved {
            self.follow_pointer();
        }
        moved
    }

    /// The item now under the pointer lit (the items moved under it).
    fn follow_pointer(&mut self) {
        let Some(p) = self.pointer.filter(|p| self.arrow_at(*p).is_none()) else { return };
        let mut y = f32::from(p.y) - POPUP_PAD + self.scroll;
        for (i, item) in self.menu.items.iter().enumerate() {
            let h = if matches!(item, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT };
            if y < h {
                if self.selectable(i) && self.sub.is_none() {
                    self.selected = Some(i);
                }
                return;
            }
            y -= h;
        }
    }

    /// The selected item brought into view (between the arrows).
    fn reveal(&mut self) {
        let Some(i) = self.selected else { return };
        if self.max_scroll() <= 0.0 {
            return;
        }
        let top = item_offset(&self.menu, i);
        let h = if matches!(self.menu.items[i], MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT };
        let (lo, hi) = (self.scroll + ARROW_HEIGHT, self.scroll + self.view_h - ARROW_HEIGHT);
        if top < lo {
            self.scroll_to(top - ARROW_HEIGHT);
        } else if top + h > hi {
            self.scroll_to(top + h - self.view_h + ARROW_HEIGHT);
        }
    }

    /// The pointer on an arrow (`dir`), or off them (None): scrolling,
    /// a tick a frame, while it rests there and there's more.
    fn rest_on_arrow(&mut self, dir: Option<f32>, cx: &mut Context<Self>) {
        if dir == self.scrolling.as_ref().map(|(d, _)| *d) {
            return;
        }
        let Some(dir) = dir else {
            self.scrolling = None;
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            let mut last = std::time::Instant::now();
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(8)).await;
                let now = std::time::Instant::now();
                let step = ARROW_SPEED * (now - last).as_secs_f32();
                last = now;
                let more = this
                    .update(cx, |m, cx| {
                        let moved = m.scroll_to(m.scroll + dir * step);
                        cx.notify();
                        moved
                    })
                    .unwrap_or(false);
                if !more {
                    let _ = this.update(cx, |m, _| m.scrolling = None);
                    return;
                }
            }
        });
        self.scrolling = Some((dir, task));
    }

    /// Takes the item: a command goes to the window, every menu closes.
    fn take(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.menu.items[i].clone() {
            MenuItem::Action(a) if a.enabled => {
                let on_choose = self.on_choose.clone();
                let command = a.command.clone();
                let _ = self.origin.update(cx, |_, window, cx| on_choose(&command, window, cx));
                self.close_all(window, cx);
            }
            MenuItem::Submenu(_) if self.selectable(i) => self.open_sub(i, window, cx),
            _ => {}
        }
    }

    fn open_sub(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.sub.is_some_and(|(s, _)| s == i) {
            return;
        }
        self.close_sub(cx);
        let MenuItem::Submenu(menu) = self.menu.items[i].clone() else { return };
        let Some(row) = self.rows.borrow().get(i).copied() else { return };
        let me = window.window_handle().downcast::<MenuPopup>();
        // Beside the item, its first item level with it, overlapping
        // this menu's edge.
        let anchor = Bounds { origin: point(row.left(), row.top() - px(POPUP_PAD - 1.0)), size: size(row.size.width - px(2.0), row.size.height) };
        let place = Place { anchor, corner: Corner::TopRight, ..Place::default() };
        if let Some(handle) = open(window, cx, menu, place, self.on_choose.clone(), me, None) {
            self.sub = Some((i, handle));
        }
        cx.notify();
    }

    fn close_sub(&mut self, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.sub.take() {
            // reentry: ok (the submenu: another popup)
            let _ = handle.update(cx, |m, window, cx| m.close(window, cx));
        }
    }

    /// Closes this menu and its submenus (as a click elsewhere in a
    /// clear layer it popped from: a window's menu).
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sub(cx);
        window.remove_window();
    }

    /// Closes every menu, up to the first.
    fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.owner;
        self.close(window, cx);
        if let Some(parent) = owner.downcast::<MenuPopup>() {
            // reentry: ok (the menu this one is a submenu of)
            let _ = parent.update(cx, |m, window, cx| m.close_all(window, cx));
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "down" => {
                self.selected = self.step(1);
                self.reveal();
            }
            "up" => {
                self.selected = self.step(-1);
                self.reveal();
            }
            "right" => {
                if let Some(i) = self.selected
                    && matches!(self.menu.items[i], MenuItem::Submenu(_))
                {
                    self.open_sub(i, window, cx);
                }
            }
            "left" | "escape" => {
                self.close(window, cx);
                return;
            }
            "enter" | "space" => {
                if let Some(i) = self.selected {
                    self.take(i, window, cx);
                }
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some(e.position);
        self.rest_on_arrow(self.arrow_at(e.position), cx);
        let over = self.row_at(e.position).filter(|&i| self.selectable(i));
        if over != self.selected && over.is_some() {
            self.selected = over;
            match over {
                Some(i) if matches!(self.menu.items[i], MenuItem::Submenu(_)) => self.open_sub(i, window, cx),
                _ => self.close_sub(cx),
            }
            cx.notify();
        }
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(e.delta.pixel_delta(px(ITEM_HEIGHT)).y);
        if self.scroll_to(self.scroll - dy) {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.row_at(e.position)
            && matches!(self.menu.items[i], MenuItem::Action(_))
        {
            self.take(i, window, cx);
        }
        self.pressed_inside = false;
    }
}

impl Render for MenuPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        agent::begin_frame(window, cx, &self.menu.title);
        let p = theme::palette(cx);
        let menu = self.menu.clone();
        let selected = self.selected;
        let open = self.sub.map(|(i, _)| i);
        let rows = self.rows.clone();
        let selectable: Vec<bool> = (0..menu.items.len()).map(|i| self.selectable(i)).collect();
        let (icon_left, label_left) = columns(&menu);
        let (scroll, (arrow_up, arrow_down)) = (self.scroll, self.arrows());
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                style::fill_rect(window, b, style::c(p.menu_background));
                // The outline, then a light inner edge on top and left.
                style::frame_rect(window, b, style::tint(p.menu_background, 1.55));
                let lined = style::inset(window, b, 1.0);
                style::hline(window, lined.left(), lined.right(), lined.top(), style::c(p.shine));
                style::vline(window, lined.left(), lined.top(), lined.bottom(), style::c(p.shine));
                let mut y = b.top() + px(POPUP_PAD - scroll);
                let (left, right) = (b.left() + px(POPUP_BORDER), b.right() - px(POPUP_BORDER));
                // Scrolled: the items between the arrows, each arrow on
                // the menu's ground.
                let items_top = b.top() + px(POPUP_PAD + if arrow_up { ARROW_HEIGHT } else { 0.0 });
                let items_bottom = b.bottom() - px(POPUP_PAD + if arrow_down { ARROW_HEIGHT } else { 0.0 });
                for (on, dir, top) in [(arrow_up, -1.0f32, b.top() + px(POPUP_PAD)), (arrow_down, 1.0, b.bottom() - px(POPUP_PAD + ARROW_HEIGHT))] {
                    if on {
                        let c = style::snap_centred(window, point(b.center().x, top + px(ARROW_HEIGHT / 2.0)), 8.0, 8.0).center();
                        let mut path = PathBuilder::fill();
                        path.move_to(point(c.x - px(4.5), c.y - px(2.25 * dir)));
                        path.line_to(point(c.x + px(4.5), c.y - px(2.25 * dir)));
                        path.line_to(point(c.x, c.y + px(2.25 * dir)));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, style::c(p.menu_item_text));
                        }
                        agent::record(
                            window,
                            cx,
                            agent::Node::button(if dir < 0.0 { "Scroll Up" } else { "Scroll Down" }, Bounds::from_corners(point(left, top), point(right, top + px(ARROW_HEIGHT)))),
                        );
                    }
                }
                let mask = ContentMask { bounds: Bounds::from_corners(point(b.left(), items_top), point(b.right(), items_bottom)) };
                let mut placed = rows.borrow_mut();
                placed.clear();
                let font = style::ui_font(FontWeight::NORMAL);
                window.with_content_mask(Some(mask), |window| {
                    for (i, item) in menu.items.iter().enumerate() {
                        let h = px(if matches!(item, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT });
                        let row = Bounds::from_corners(point(left, y), point(right, y + h));
                        placed.push(row);
                        y += h;
                        let lit = selectable[i] && (selected == Some(i) || open == Some(i));
                        let face = Bounds::from_corners(point(lined.left(), row.top()), point(lined.right(), row.bottom()));
                        let text = |enabled: bool| {
                            if !enabled {
                                style::disabled_text(&p)
                            } else if lit {
                                style::c(p.menu_selected_item_text)
                            } else {
                                style::c(p.menu_item_text)
                            }
                        };
                        let cy = row.center().y;
                        match item {
                            MenuItem::Separator => {
                                let gy = px(f32::from(cy).floor());
                                style::hline_above(window, row.left() + px(1.0), row.right() - px(1.0), gy, style::shade(&p, p.menu_background, 1.16));
                                style::hline(window, row.left() + px(1.0), row.right() - px(1.0), gy, style::c(p.shine));
                            }
                            MenuItem::Action(a) => {
                                if lit {
                                    style::fill_rect(window, face, style::c(p.menu_selected_background));
                                }
                                let ink = if a.minimized && a.enabled && !lit { style::c(olo_theme::mix(p.menu_item_text, p.menu_background, 0.38)) } else { text(a.enabled) };
                                let m = Bounds { origin: point(row.left() + px(10.0 - 4.5), cy - px(4.5)), size: size(px(9.0), px(9.0)) };
                                if a.checked {
                                    check_mark(window, m, ink);
                                } else if a.minimized {
                                    diamond(window, m, ink);
                                }
                                if let (Some(left), Some(icon)) = (icon_left, &a.icon) {
                                    item_icon(window, row, left, icon, a.enabled);
                                } else if let (Some(left), Some(color)) = (icon_left, a.swatch) {
                                    let r = style::snap_centred(window, point(row.left() + px(left + ICON_SIZE / 2.0), row.center().y), ICON_SIZE - 2.0, ICON_SIZE - 2.0);
                                    window.with_element_opacity((!a.enabled).then_some(110.0 / 255.0), |window| crate::widgets::color::paint_swatch(window, r, color, &p));
                                }
                                let r = style::label(window, cx, row.left() + px(label_left), cy, style::Align::Left, a.label.clone(), style::PLAIN_SIZE, font.clone(), ink);
                                if let Some(keys) = &a.keys {
                                    style::key_combo(window, cx, point(row.right() - px(ITEM_RIGHT), cy), keys, ink);
                                }
                                let _ = r;
                                agent::record(
                                    window,
                                    cx,
                                    agent::Node {
                                        role: "MenuItem",
                                        name: Some(SharedString::from(a.label.clone())),
                                        bounds: row,
                                        enabled: a.enabled,
                                        clickable: a.enabled,
                                        checked: a.checked.then_some(true),
                                        selected: Some(lit),
                                        minimized: a.minimized,
                                        ..Default::default()
                                    },
                                );
                            }
                            MenuItem::Submenu(m) => {
                                if lit {
                                    style::fill_rect(window, face, style::c(p.menu_selected_background));
                                }
                                let ink = text(selectable[i]);
                                if let (Some(left), Some(icon)) = (icon_left, &m.icon) {
                                    item_icon(window, row, left, icon, selectable[i]);
                                }
                                style::label(window, cx, row.left() + px(label_left), cy, style::Align::Left, m.title.clone(), style::PLAIN_SIZE, font.clone(), ink);
                                // A triangle pointing right.
                                let tx = row.right() - px(ITEM_RIGHT);
                                let mut path = PathBuilder::fill();
                                path.move_to(point(tx - px(3.0), cy - px(4.0)));
                                path.line_to(point(tx + px(1.0), cy));
                                path.line_to(point(tx - px(3.0), cy + px(4.0)));
                                path.close();
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, ink);
                                }
                                agent::record(
                                    window,
                                    cx,
                                    agent::Node {
                                        role: "MenuItem",
                                        name: Some(SharedString::from(m.title.clone())),
                                        bounds: row,
                                        enabled: selectable[i],
                                        clickable: selectable[i],
                                        selected: Some(lit),
                                        ..Default::default()
                                    },
                                );
                            }
                        }
                    }
                });
            },
        )
        .size_full();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, _, _| m.pressed_inside = true))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::wheel))
            .child(paint)
    }
}

/// BeOS's check mark: a stroke down and up, 1.6 points wide.
pub fn check_mark(window: &mut Window, r: Bounds<Pixels>, color: Hsla) {
    let mut path = PathBuilder::stroke(px(1.6));
    path.move_to(point(r.left(), r.top() + r.size.height * 0.5));
    path.line_to(point(r.left() + r.size.width * 0.38, r.bottom()));
    path.line_to(point(r.right(), r.top()));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// A minimized window's mark: a hollow diamond, its middle on the
/// device grid, a line 1.2 points wide.
pub fn diamond(window: &mut Window, r: Bounds<Pixels>, color: Hsla) {
    let m = style::snap_centred(window, r.center(), 8.0, 8.0).center();
    let h = f32::from(r.size.width.min(r.size.height)) / 2.0 - 0.5;
    let mut path = PathBuilder::stroke(px(1.2));
    path.move_to(point(m.x, m.y - px(h)));
    path.line_to(point(m.x + px(h), m.y));
    path.line_to(point(m.x, m.y + px(h)));
    path.line_to(point(m.x - px(h), m.y));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

// ---- the menu bar ----------------------------------------------------------

/// As tall as a tab strip's row and a pane's head, so the chrome under it
/// keeps one rhythm.
pub const BAR_HEIGHT: f32 = 26.0;
const BAR_PAD: f32 = 8.0;

/// A window's menu bar, as its frame keeps it: its menus (keys resolved),
/// where commands go, which menu is open.
struct Bar {
    menus: Vec<Menu>,
    on_command: OnChoose,
    /// The menu open, and its popup.
    open: Option<(usize, WindowHandle<MenuPopup>)>,
    /// The open menu's title was pressed since it opened: the next press
    /// there puts it away (one that slid open stays for its press, as on
    /// BeOS).
    pressed: bool,
    /// The titles, where the bar last drew them (in the window).
    titles: Vec<Bounds<Pixels>>,
    /// The menu the press now under way put away (`press_in`), so a
    /// press on its own title doesn't open it again.
    closed: Option<usize>,
}

#[derive(Default)]
struct Bars(std::collections::HashMap<gpui::WindowId, Bar>);

impl gpui::Global for Bars {}

/// BMenuBar: the window's menus across its top (the menu colour, a light top and left edge, a dark line under
/// it; the open menu's title in the selection colours). A press on a
/// title opens its menu as a popup under it; with one open, the pointer
/// sliding across the bar opens the others; a press on the open title
/// puts it away. The menus' keys come from the keyboard profile
/// ([`resolve`]), and the window takes them when nothing in it did (a
/// text field's ⌘C is its own). Agents read each title (`MenuBarItem`),
/// and the command palette finds the commands.
#[derive(IntoElement)]
pub struct MenuBar {
    menus: Vec<Menu>,
    on_command: OnChoose,
    raw: bool,
}

impl MenuBar {
    /// `menus` across the top; `on_command` gets the command chosen or
    /// pressed, in the window.
    pub fn new(menus: Vec<Menu>, on_command: impl Fn(&str, &mut Window, &mut App) + 'static) -> MenuBar {
        MenuBar { menus, on_command: Rc::new(on_command), raw: false }
    }

    /// For a terminal-style window (one of app_server's `terminal_apps`,
    /// whose command key arrives with Shift: ⌘C as Ctrl+Shift+C): its keys
    /// as they're pressed there ([`resolve`]).
    pub fn raw(mut self) -> MenuBar {
        self.raw = true;
        self
    }
}

/// The window's menu bar's command for `k` (Frame takes the keys nothing
/// else in the window did), and where to send it.
pub(crate) fn bar_shortcut(window: &Window, cx: &App, k: &gpui::Keystroke) -> Option<(String, OnChoose)> {
    let bar = cx.try_global::<Bars>()?.0.get(&window.window_handle().window_id())?;
    Some((find_shortcut(&bar.menus, k)?, bar.on_command.clone()))
}

/// The command of `window`'s menu bar that keystroke `k` chooses, if any
/// (for a view that takes the keys for itself, to let one through).
pub fn shortcut_command(window: &Window, cx: &App, k: &gpui::Keystroke) -> Option<String> {
    bar_shortcut(window, cx, k).map(|(command, _)| command)
}

/// The menus of `window`'s bar (for the command palette) and where its
/// commands go.
pub(crate) fn bar_menus(window: gpui::WindowId, cx: &App) -> Option<(Vec<Menu>, OnChoose)> {
    cx.try_global::<Bars>()?.0.get(&window).map(|b| (b.menus.clone(), b.on_command.clone()))
}

/// A window closed: its bar goes.
pub(crate) fn forget_closed(cx: &mut App) {
    let open: std::collections::HashSet<gpui::WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
    cx.default_global::<Bars>().0.retain(|id, _| open.contains(id));
}

/// Opens bar menu `i` under its title `title` (in the window), by a
/// press on it or sliding onto it.
fn open_bar_menu(window: &mut Window, cx: &mut App, i: usize, title: Bounds<Pixels>, pressed: bool) {
    let id = window.window_handle().window_id();
    let Some((menu, on_command)) = cx.try_global::<Bars>().and_then(|b| b.0.get(&id)).and_then(|b| Some((b.menus.get(i)?.clone(), b.on_command.clone()))) else { return };
    close_bar_menu(window, cx);
    let me = window.window_handle();
    // Its popup gone (chosen, put away, a click elsewhere): no menu open,
    // unless another has opened since. By its popup, not its place: the
    // menu it replaced goes after the new one opens, and a menu that
    // slid open and then was pressed open again is the same menu; taken
    // by its place, the bar forgot the new one and opened another over
    // it, which ended the program's connection (2026-11-01).
    let mine: Rc<std::cell::Cell<Option<gpui::AnyWindowHandle>>> = Rc::default();
    let closing = mine.clone();
    let on_close: Rc<dyn Fn(&mut App)> = Rc::new(move |cx: &mut App| {
        let bars = cx.default_global::<Bars>();
        if let Some(bar) = bars.0.get_mut(&id)
            && bar.open.is_some_and(|(_, h)| Some(gpui::AnyWindowHandle::from(h)) == closing.get())
        {
            bar.open = None;
            let _ = me.update(cx, |_, window, _| window.refresh());
        }
    });
    let place = Place { anchor: title, corner: Corner::BottomLeft, ..Place::default() };
    let handle = open(window, cx, menu, place, on_command, None, Some(on_close));
    mine.set(handle.map(Into::into));
    if let Some(bar) = cx.default_global::<Bars>().0.get_mut(&id) {
        bar.open = handle.map(|h| (i, h));
        bar.pressed = pressed;
    }
    window.refresh();
}

/// Puts the bar's open menu away.
fn close_bar_menu(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let open = cx.default_global::<Bars>().0.get_mut(&id).and_then(|b| b.open.take());
    if let Some((_, handle)) = open {
        let _ = handle.update(cx, |m, window, cx| m.close(window, cx));
    }
    window.refresh();
}

impl RenderOnce for MenuBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Whole device pixels high, so a bar under it (a ruler, a tool
        // bar as high) is as high on screen at every scale.
        let height = style::snap_coord(window, px(BAR_HEIGHT));
        let mut menus = self.menus;
        resolve(&mut menus, self.raw);
        let id = window.window_handle().window_id();
        let bars = cx.default_global::<Bars>();
        match bars.0.get_mut(&id) {
            Some(bar) => {
                bar.menus = menus.clone();
                bar.on_command = self.on_command;
            }
            None => {
                bars.0.insert(id, Bar { menus: menus.clone(), on_command: self.on_command, open: None, pressed: false, titles: Vec::new(), closed: None });
            }
        }
        let titles: Vec<SharedString> = menus.iter().map(|m| SharedString::from(m.title.clone())).collect();
        canvas(
            move |b, window, _| {
                // The first title's text lines up with the window's
                // content.
                let font = style::ui_font(FontWeight::NORMAL);
                let mut x = b.left() + px(olo_kit_base::metrics::WINDOW_INSET - BAR_PAD);
                titles
                    .iter()
                    .map(|t| {
                        let w = style::text_width(window, t.clone(), style::PLAIN_SIZE, font.clone()) + px(2.0 * BAR_PAD);
                        let r = Bounds { origin: point(x, b.top() + px(1.0)), size: size(w, px(BAR_HEIGHT - 2.0)) };
                        x += w;
                        (t.clone(), r, window.insert_hitbox(r, gpui::HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |b, items, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, style::c(p.menu_background));
                // BeOS's bevel: light top and left, dark bottom.
                style::hline(window, b.left(), b.right(), b.top(), style::c(p.shine));
                style::vline(window, b.left(), b.top(), b.bottom(), style::c(p.shine));
                style::hline_above(window, b.left(), b.right(), b.bottom(), style::tint(p.menu_background, 1.4));
                let open = cx.try_global::<Bars>().and_then(|bars| bars.0.get(&id)).and_then(|bar| bar.open.map(|(i, _)| i));
                let font = style::ui_font(FontWeight::NORMAL);
                for (i, (title, r, _)) in items.iter().enumerate() {
                    let ink = if open == Some(i) {
                        // Between the bar's bevel lines, on device pixels.
                        let line = style::hairline(window);
                        style::fill_rect(window, Bounds::from_corners(point(r.left(), b.top() + line), point(r.right(), b.bottom() - line)), style::c(p.menu_selected_background));
                        style::c(p.menu_selected_item_text)
                    } else {
                        style::c(p.menu_item_text)
                    };
                    style::label(window, cx, r.left() + px(BAR_PAD), r.center().y, style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), ink);
                    agent::record(
                        window,
                        cx,
                        agent::Node { role: "MenuBarItem", name: Some(title.clone()), bounds: *r, enabled: true, clickable: true, selected: Some(open == Some(i)), ..Default::default() },
                    );
                }

                let hits: Vec<(Bounds<Pixels>, gpui::Hitbox)> = items.iter().map(|(_, r, h)| (*r, h.clone())).collect();
                if let Some(bar) = cx.default_global::<Bars>().0.get_mut(&id) {
                    bar.titles = hits.iter().map(|(r, _)| *r).collect();
                }
                let down = hits.clone();
                // Menus open on the press, as on BeOS.
                window.on_mouse_event(move |e: &gpui::MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left {
                        return;
                    }
                    // By where the press is: the hover a menu's grab
                    // left behind can still be on the title it opened from.
                    let Some(i) = down.iter().position(|(r, _)| r.contains(&e.position)) else { return };
                    // Its menu was open: the press put it away
                    // (`press_in`), and away it stays; or it slid open,
                    // and stays for this press.
                    let bar = cx.default_global::<Bars>().0.get_mut(&id);
                    let (put_away, showing) = bar.map_or((false, false), |b| (b.closed.take() == Some(i), b.open.is_some_and(|(o, _)| o == i)));
                    if !put_away && !showing {
                        open_bar_menu(window, cx, i, down[i].0, true);
                    }
                    cx.stop_propagation();
                });
                // Sliding across the bar with one open opens the others.
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let open = cx.try_global::<Bars>().and_then(|bars| bars.0.get(&id)).and_then(|b| b.open.map(|(i, _)| i));
                    let Some(open) = open else { return };
                    if let Some(i) = hits.iter().position(|(_, h)| h.is_hovered(window))
                        && i != open
                    {
                        open_bar_menu(window, cx, i, hits[i].0, false);
                    }
                });
            },
        )
        .w_full()
        .h(height)
        .flex_shrink_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_by_what_they_say_within_separators_and_down() {
        let m = Menu::new("Olo")
            .item("terminal", "t")
            .submenu(Menu::new("Games").item("Zork", "z").item("chess", "c"))
            .item("Item 10", "10")
            .item("Item 2", "2")
            .separator()
            .item("b", "b")
            .item("A", "a")
            .sorted();
        let said: Vec<&str> = m
            .items
            .iter()
            .map(|i| match i {
                MenuItem::Action(a) => a.label.as_str(),
                MenuItem::Submenu(s) => s.title.as_str(),
                MenuItem::Separator => "|",
            })
            .collect();
        assert_eq!(said, ["Games", "Item 2", "Item 10", "terminal", "|", "A", "b"]);
        let MenuItem::Submenu(games) = &m.items[0] else { panic!() };
        assert!(matches!(&games.items[..], [MenuItem::Action(a), MenuItem::Action(b)] if a.label == "chess" && b.label == "Zork"));
    }

    #[test]
    fn separators_tidied() {
        let menu = Menu::new("M").separator().item("A", "a").separator().separator().item("B", "b").separator().tidied();
        let separators: Vec<bool> = menu.items.iter().map(|item| matches!(item, MenuItem::Separator)).collect();
        assert_eq!(separators, [false, true, false]);
    }
}
