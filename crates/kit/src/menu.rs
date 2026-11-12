//! BeOS menus: a [`Menu`] of items (commands, separators, submenus),
//! shown as a popup over a window ([`popup`], [`context_menu`]): a
//! Wayland popup, so it can reach past the window's edge, which the
//! compositor keeps on screen (flipped or slid). It's a window on the
//! looper of the window it opened over, so what's chosen is handed over
//! at once. Drawn as BeOS drew them: the menu colour in a dark outline
//! with a light inner edge on top and left, the item under the pointer
//! or chosen from the keyboard in the selection colours, check marks,
//! separators as a groove, a triangle for a submenu, which opens beside
//! its item. ↑ ↓ choose, → opens a submenu, ← and Escape close a level,
//! Enter or Space takes the item; a release over an item takes it.
//! Agents read each item (`MenuItem`, with whether it's checked or can
//! be chosen). One taller than its display scrolls, with an arrow at
//! each end there's more past (the pointer resting there scrolls it),
//! the wheel and the keys.
//!
//! What's chosen is a command by name, as BeOS's menu items carried a
//! message's `what`: the same names the command palette and agents
//! choose by. It goes to a handler on the window's looper; a window's
//! own view takes it in `View::command` ([`on_command`]).

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use olo_interface::{
    Bounds, ContentMask, Corner, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, LocalWindow, MouseButton, MouseMoveEvent, MouseUpEvent, NodeRef, Painter,
    ParentElement as _, PathBuilder, Pixels, Point, ScrollWheelEvent, SharedString, Styled as _, TimerId, View, WindowCx, WindowId, WindowKind, WindowSpec, canvas, div, point, px, size,
};

use crate::{
    agent,
    keys::{self, Shortcut},
    style, theme,
};

pub use crate::style::check_mark;

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
fn item_icon(g: &mut Painter, row: Bounds<Pixels>, left: f32, name: &str, enabled: bool) {
    let r = style::snap_centred(g, point(row.left() + px(left + ICON_SIZE / 2.0), row.center().y), ICON_SIZE, ICON_SIZE);
    g.with_element_opacity(Some(if enabled { 1.0 } else { 110.0 / 255.0 }), |g| {
        if !crate::icons::paint(g, r, name) {
            crate::icons::paint(g, r, "application");
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

    /// Move to Agent Screen ▸ (titled `title`): each agent screen running,
    /// by the name the person gave it, then New Agent Screen; commands
    /// `prefix` and the screen's name (`olo_kit_base::moving`'s
    /// `SCREEN_COMMAND`, `NEW_SCREEN_COMMAND` with no prefix of its own).
    /// The program keeps running as it goes (`olo-screen take`).
    pub fn agent_screens(title: &str, screens: &[olo_kit_base::moving::RunningScreen], prefix: &str) -> Menu {
        let mut m = Menu::new(title);
        for s in screens {
            m = m.item(s.label.clone(), format!("{prefix}{}{}", olo_kit_base::moving::SCREEN_COMMAND, s.name));
        }
        if !screens.is_empty() {
            m = m.separator();
        }
        m.item("New Agent Screen", format!("{prefix}{}", olo_kit_base::moving::NEW_SCREEN_COMMAND))
    }

    /// Move to Window ▸ (titled `title`): every tabbed app's tab, and tab
    /// group, offers its app's other windows by their titles, in
    /// alphabetical order, each item's command what `others` gives with
    /// it; with none, "No Other Windows", greyed. Moved there, the tab (or
    /// the group's tabs) goes in after the window's own and shows; a
    /// window whose last tab went closes (Ben, 2026-11-08: "a pattern and
    /// works for all tabbed olo apps … if that leaves no more tabs in the
    /// origin window then that window should close"). Terminal's tabs and
    /// tab groups, and a window's own tab (the window menu: into that
    /// window's group) use it.
    pub fn move_to_window(title: &str, others: impl IntoIterator<Item = (String, String)>) -> Menu {
        let mut others: Vec<(String, String)> = others.into_iter().collect();
        others.sort_by(|a, b| olo_kit_base::files::natural_cmp(&a.0, &b.0));
        let mut m = Menu::new(title);
        if others.is_empty() {
            return m.item("No Other Windows", "none").enabled(false);
        }
        for (label, command) in others {
            m = m.item(label, command);
        }
        m
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

    /// The last item pressed with the command key and `key` (as the kit
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
    pub fn find_shortcut(&self, k: &olo_interface::Keystroke) -> Option<&str> {
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
pub fn find_shortcut(menus: &[Menu], k: &olo_interface::Keystroke) -> Option<String> {
    menus.iter().find_map(|m| m.find_shortcut(k)).map(str::to_owned)
}

/// What to do with the command chosen, in the window the menu opened
/// over.
pub type OnChoose = Rc<dyn Fn(&str, &mut WindowCx)>;
/// What to do once a menu has gone, in the window it opened over.
type OnClose = Rc<dyn Fn(&mut WindowCx)>;

/// Hands a command to the window's own view (`View::command`): what a
/// menu bar and a context menu do with what's chosen unless told
/// otherwise.
pub fn on_command(command: &str, cx: &mut WindowCx) {
    cx.command(command);
}

/// A popup's size for `menu`.
fn popup_size(menu: &Menu) -> (f32, f32) {
    let (mut label_w, mut key_w, mut h) = (0.0f32, 0.0f32, 0.0f32);
    let font = style::ui_font(FontWeight::NORMAL);
    for item in &menu.items {
        match item {
            MenuItem::Action(a) => {
                label_w = label_w.max(f32::from(style::text_width(a.label.clone(), style::PLAIN_SIZE, font.clone())));
                if let Some(keys) = &a.keys {
                    key_w = key_w.max(style::key_combo_width(keys));
                }
                h += ITEM_HEIGHT;
            }
            MenuItem::Submenu(m) => {
                label_w = label_w.max(f32::from(style::text_width(m.title.clone(), style::PLAIN_SIZE, font.clone())));
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

/// Opens `menu` over the window, its top left at `corner` of `anchor`
/// (in the window's content). `on_choose` gets the command chosen, in
/// the window.
pub fn popup(cx: &mut WindowCx, menu: Menu, anchor: Bounds<Pixels>, corner: Corner, on_choose: impl Fn(&str, &mut WindowCx) + 'static) -> Option<LocalWindow<MenuPopup>> {
    open_first(cx, menu, Place { anchor, corner, ..Place::default() }, Rc::new(on_choose), None)
}

/// [`popup`], and `on_close` once it has gone, chosen from or not (after
/// `on_choose`): what opened it shows it open until then (Deskbar's
/// rows).
pub fn popup_then(
    cx: &mut WindowCx,
    menu: Menu,
    anchor: Bounds<Pixels>,
    corner: Corner,
    on_choose: impl Fn(&str, &mut WindowCx) + 'static,
    on_close: impl Fn(&mut WindowCx) + 'static,
) -> Option<LocalWindow<MenuPopup>> {
    open_first(cx, menu, Place { anchor, corner, ..Place::default() }, Rc::new(on_choose), Some(Rc::new(on_close)))
}

/// Opens `menu` over `field` (in the window's content) with item
/// `current` lying over it, chosen, as a menu field's on BeOS, and at
/// least as wide as the field.
pub fn popup_over(cx: &mut WindowCx, menu: Menu, field: Bounds<Pixels>, current: usize, on_choose: impl Fn(&str, &mut WindowCx) + 'static) -> Option<LocalWindow<MenuPopup>> {
    let dy = POPUP_PAD + item_offset(&menu, current) - (f32::from(field.size.height) - ITEM_HEIGHT) / 2.0;
    let place = Place { anchor: field, corner: Corner::TopLeft, offset: point(px(0.0), px(-dy.round())), min_width: f32::from(field.size.width).round(), selected: Some(current) };
    open_first(cx, menu, place, Rc::new(on_choose), None)
}

/// The menu open over a window (not its menu bar's, which keeps its
/// own), from where, and the one a press in the window just put away.
#[derive(Default)]
struct Popped {
    menu: Option<(LocalWindow<MenuPopup>, Bounds<Pixels>)>,
    closed: Option<Bounds<Pixels>>,
}

/// A press in the window (its frame sees every one): a menu open over it
/// goes, as a press outside a menu puts it away (the compositor hands an
/// app's own window the presses outside its menu); and that press
/// doesn't open it again, on what opened it.
pub(crate) fn press_in(at: Point<Pixels>, cx: &mut WindowCx) {
    let popped = cx.window_state::<Popped>();
    let taken = popped.menu.take();
    popped.closed = taken.as_ref().map(|(_, anchor)| *anchor);
    if let Some((menu, _)) = taken {
        menu.update(cx, |m, cx| m.close(cx));
    }
    // The menu bar's menu too: a press anywhere in the window puts it
    // away, its own title's included (which then leaves it away), but
    // for the first press on the title of one that slid open.
    let bar = cx.window_state::<Bar>();
    bar.closed = None;
    let open = match bar.open {
        Some((i, _, title)) if !bar.pressed && title.contains(&at) => {
            bar.pressed = true;
            let _ = i;
            None
        }
        Some((i, ..)) => {
            bar.closed = Some(i);
            bar.open.take()
        }
        None => None,
    };
    if let Some((_, menu, _)) = open {
        bar.shared.open.set(None);
        menu.update(cx, |m, cx| m.close(cx));
        repaint_bar(cx);
    }
}

/// Opens a first menu over the window (see [`Popped`]).
fn open_first(cx: &mut WindowCx, menu: Menu, place: Place, on_choose: OnChoose, on_close: Option<OnClose>) -> Option<LocalWindow<MenuPopup>> {
    let popped = cx.window_state::<Popped>();
    // The press that put it away, on what opened it: away it stays.
    if popped.closed.take() == Some(place.anchor) {
        if let Some(f) = on_close {
            f(cx);
        }
        return None;
    }
    if let Some((old, _)) = popped.menu.take() {
        old.update(cx, |m, cx| m.close(cx));
    }
    let anchor = place.anchor;
    let origin = cx.window_id();
    // Gone (chosen from, put away, a click elsewhere): no menu open over
    // the window, unless another has opened since.
    let mine: Rc<Cell<Option<WindowId>>> = Rc::default();
    let closing = mine.clone();
    let gone: OnClose = Rc::new(move |cx: &mut WindowCx| {
        let popped = cx.window_state::<Popped>();
        if popped.menu.as_ref().is_some_and(|(m, _)| Some(m.id()) == closing.get()) {
            popped.menu = None;
        }
        if let Some(f) = &on_close {
            f(cx);
        }
    });
    let handle = open(cx, menu, place, on_choose, None, origin, Some(gone))?;
    mine.set(Some(handle.id()));
    cx.window_state::<Popped>().menu = Some((handle, anchor));
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
    offset: Point<Pixels>,
    min_width: f32,
    selected: Option<usize>,
}

impl Default for Place {
    fn default() -> Place {
        Place { anchor: Bounds::default(), corner: Corner::TopLeft, offset: Point::default(), min_width: 0.0, selected: None }
    }
}

/// A context menu at `position` (the pointer, in the window's content).
pub fn context_menu(cx: &mut WindowCx, menu: Menu, position: Point<Pixels>, on_choose: impl Fn(&str, &mut WindowCx) + 'static) -> Option<LocalWindow<MenuPopup>> {
    let anchor = Bounds { origin: position, size: size(px(1.0), px(1.0)) };
    popup(cx, menu, anchor, Corner::TopLeft, on_choose)
}

/// [`context_menu`], and `on_close` once it has gone, chosen from or not
/// (after `on_choose`): for a menu that is all its window is for (a
/// window's menu over a clear layer).
pub fn context_menu_then(
    cx: &mut WindowCx,
    menu: Menu,
    position: Point<Pixels>,
    on_choose: impl Fn(&str, &mut WindowCx) + 'static,
    on_close: impl Fn(&mut WindowCx) + 'static,
) -> Option<LocalWindow<MenuPopup>> {
    let anchor = Bounds { origin: position, size: size(px(1.0), px(1.0)) };
    open_first(cx, menu, Place { anchor, ..Place::default() }, Rc::new(on_choose), Some(Rc::new(on_close)))
}

/// Opens a menu over the window `cx` is (the window itself, or the menu
/// it's a submenu of, `parent_menu`); commands go to `origin`.
fn open(cx: &mut WindowCx, menu: Menu, place: Place, on_choose: OnChoose, parent_menu: Option<LocalWindow<MenuPopup>>, origin: WindowId, on_close: Option<OnClose>) -> Option<LocalWindow<MenuPopup>> {
    // (A menu field's items stay as counted: one of them lies over it.)
    let menu = if place.selected.is_none() { menu.tidied() } else { menu };
    let (w, full_h) = popup_size(&menu);
    let w = w.max(place.min_width);
    let Place { anchor, corner, mut offset, selected, .. } = place;
    // Taller than the display: as tall as it allows, its items scrolled
    // (the one chosen in view: a pop-up field's over the field still).
    let room = cx.display_bounds().map_or(f32::MAX, |d| f32::from(d.size.height) - 2.0 * SCREEN_MARGIN);
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
    let owner = cx.window_id();
    let spec = WindowSpec { kind: WindowKind::Popup { parent: owner, anchor, corner, offset }, ..WindowSpec::new(menu.title.clone(), w, h) };
    cx.open_window_here(spec, move |cx| {
        // (A menu isn't a window's layout: the review leaves it out.)
        cx.window_state::<crate::review::NotReviewed>().0 = true;
        MenuPopup { menu, selected, on_choose, origin, parent_menu, sub: None, pressed_inside: false, scroll, content, view_h: h - 2.0 * POPUP_PAD, scrolling: None, pointer: None, on_close }
    })
}

/// A menu showing.
pub struct MenuPopup {
    menu: Menu,
    selected: Option<usize>,
    on_choose: OnChoose,
    /// The window the first menu opened over, where commands go.
    origin: WindowId,
    /// The menu this one is a submenu of.
    parent_menu: Option<LocalWindow<MenuPopup>>,
    /// The submenu open from an item.
    sub: Option<(usize, LocalWindow<MenuPopup>)>,
    /// A press began in it (a click, rather than the release of the
    /// press that opened it).
    pressed_inside: bool,
    /// Scrolled this far down its items (a menu taller than its display),
    /// of `content` in all, `view_h` showing (arrows included).
    scroll: f32,
    content: f32,
    view_h: f32,
    /// Scrolling while the pointer rests on an arrow: which way, the
    /// ticks doing it and when the last was; and where the pointer last
    /// was (the item under it is the one lit as the items scroll by).
    scrolling: Option<(f32, TimerId, Instant)>,
    pointer: Option<Point<Pixels>>,
    /// Told, in the window it opened over, once it has gone.
    on_close: Option<OnClose>,
}

/// What a menu is told: a moment has passed with the pointer resting on
/// a scroll arrow.
#[derive(Clone)]
pub enum PopupMessage {
    Tick,
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

    /// Where item `i` is in a menu `width` wide (as scrolled now).
    fn row(&self, i: usize, width: Pixels) -> Bounds<Pixels> {
        let h = px(if matches!(self.menu.items[i], MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT });
        let y = px(POPUP_PAD - self.scroll + item_offset(&self.menu, i));
        Bounds::from_corners(point(px(POPUP_BORDER), y), point(width - px(POPUP_BORDER), y + h))
    }

    fn row_at(&self, p: Point<Pixels>, width: Pixels) -> Option<usize> {
        // (Not under a scroll arrow.)
        if self.arrow_at(p).is_some() {
            return None;
        }
        (0..self.menu.items.len()).find(|&i| self.row(i, width).contains(&p))
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
    fn arrow_at(&self, p: Point<Pixels>) -> Option<f32> {
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
    /// a tick every few milliseconds, while it rests there and there's
    /// more.
    fn rest_on_arrow(&mut self, dir: Option<f32>, cx: &mut Cx<Self>) {
        if dir == self.scrolling.as_ref().map(|(d, ..)| *d) {
            return;
        }
        if let Some((_, timer, _)) = self.scrolling.take() {
            cx.looper().cancel(timer);
        }
        let Some(dir) = dir else { return };
        let timer = cx.every(Duration::from_millis(8), || PopupMessage::Tick);
        self.scrolling = Some((dir, timer, Instant::now()));
    }

    /// Takes the item: a command goes to the window, every menu closes.
    fn take(&mut self, i: usize, cx: &mut Cx<Self>) {
        match self.menu.items[i].clone() {
            MenuItem::Action(a) if a.enabled => {
                let on_choose = self.on_choose.clone();
                let origin = self.origin;
                olo_interface::with_window_on(cx.looper(), origin, |cx| on_choose(&a.command, cx));
                self.close_all(cx);
            }
            MenuItem::Submenu(_) if self.selectable(i) => self.open_sub(i, cx),
            _ => {}
        }
    }

    fn open_sub(&mut self, i: usize, cx: &mut Cx<Self>) {
        if self.sub.is_some_and(|(s, _)| s == i) {
            return;
        }
        self.close_sub(cx);
        let MenuItem::Submenu(menu) = self.menu.items[i].clone() else { return };
        let row = self.row(i, cx.viewport_size().width);
        let me = LocalWindow::<MenuPopup>::of(cx);
        // Beside the item, its first item level with it, overlapping
        // this menu's edge.
        let anchor = Bounds { origin: point(row.left(), row.top() - px(POPUP_PAD - 1.0)), size: size(row.size.width - px(2.0), row.size.height) };
        let place = Place { anchor, corner: Corner::TopRight, ..Place::default() };
        if let Some(handle) = open(cx, menu, place, self.on_choose.clone(), Some(me), self.origin, None) {
            self.sub = Some((i, handle));
        }
        cx.invalidate();
    }

    fn close_sub(&mut self, cx: &mut WindowCx) {
        if let Some((_, handle)) = self.sub.take() {
            handle.update(cx, |m, cx| m.close(cx));
        }
    }

    /// Closes this menu and its submenus (as a click elsewhere in a
    /// clear layer it popped from: a window's menu).
    pub fn close(&mut self, cx: &mut WindowCx) {
        self.close_sub(cx);
        cx.close_window();
    }

    /// Closes every menu, up to the first.
    fn close_all(&mut self, cx: &mut WindowCx) {
        let parent = self.parent_menu;
        self.close(cx);
        if let Some(parent) = parent {
            parent.update(cx, |m, cx| m.close_all(cx));
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
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
                    self.open_sub(i, cx);
                }
            }
            "left" | "escape" => {
                self.close(cx);
                return;
            }
            "enter" | "space" => {
                if let Some(i) = self.selected {
                    self.take(i, cx);
                }
                return;
            }
            _ => return,
        }
        cx.invalidate();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        self.pointer = Some(e.position);
        self.rest_on_arrow(self.arrow_at(e.position), cx);
        let over = self.row_at(e.position, cx.viewport_size().width).filter(|&i| self.selectable(i));
        if over != self.selected && over.is_some() {
            self.selected = over;
            match over {
                Some(i) if matches!(self.menu.items[i], MenuItem::Submenu(_)) => self.open_sub(i, cx),
                _ => self.close_sub(cx),
            }
            cx.invalidate();
        }
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Cx<Self>) {
        let dy = f32::from(e.delta.pixel_delta(px(ITEM_HEIGHT)).y);
        if self.scroll_to(self.scroll - dy) {
            cx.invalidate();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &mut Cx<Self>) {
        if let Some(i) = self.row_at(e.position, cx.viewport_size().width)
            && matches!(self.menu.items[i], MenuItem::Action(_))
        {
            self.take(i, cx);
        }
        self.pressed_inside = false;
    }

    fn paint(&self, g: &mut Painter, b: Bounds<Pixels>) {
        let p = theme::palette();
        let menu = &self.menu;
        let selected = self.selected;
        let open = self.sub.map(|(i, _)| i);
        let (icon_left, label_left) = columns(menu);
        let (arrow_up, arrow_down) = self.arrows();
        style::fill_rect(g, b, style::c(p.menu_background));
        // The outline, then a light inner edge on top and left.
        style::frame_rect(g, b, style::tint(p.menu_background, 1.55));
        let lined = style::inset(g, b, 1.0);
        style::hline(g, lined.left(), lined.right(), lined.top(), style::c(p.shine));
        style::vline(g, lined.left(), lined.top(), lined.bottom(), style::c(p.shine));
        let (left, right) = (b.left() + px(POPUP_BORDER), b.right() - px(POPUP_BORDER));
        // Scrolled: the items between the arrows, each arrow on the
        // menu's ground.
        let items_top = b.top() + px(POPUP_PAD + if arrow_up { ARROW_HEIGHT } else { 0.0 });
        let items_bottom = b.bottom() - px(POPUP_PAD + if arrow_down { ARROW_HEIGHT } else { 0.0 });
        for (on, dir, top) in [(arrow_up, -1.0f32, b.top() + px(POPUP_PAD)), (arrow_down, 1.0, b.bottom() - px(POPUP_PAD + ARROW_HEIGHT))] {
            if on {
                let c = style::snap_centred(g, point(b.center().x, top + px(ARROW_HEIGHT / 2.0)), 8.0, 8.0).center();
                let mut path = PathBuilder::fill();
                path.move_to(point(c.x - px(4.5), c.y - px(2.25 * dir)));
                path.line_to(point(c.x + px(4.5), c.y - px(2.25 * dir)));
                path.line_to(point(c.x, c.y + px(2.25 * dir)));
                path.close();
                if let Ok(path) = path.build() {
                    g.paint_path(path, style::c(p.menu_item_text));
                }
                g.agent(agent::Node::button(if dir < 0.0 { "Scroll Up" } else { "Scroll Down" }, Bounds::from_corners(point(left, top), point(right, top + px(ARROW_HEIGHT)))));
            }
        }
        let mask = ContentMask { bounds: Bounds::from_corners(point(b.left(), items_top), point(b.right(), items_bottom)) };
        let font = style::ui_font(FontWeight::NORMAL);
        g.with_content_mask(Some(mask), |g| {
            for (i, item) in menu.items.iter().enumerate() {
                let row = self.row(i, b.size.width);
                let row = Bounds { origin: row.origin + b.origin, size: row.size };
                let selectable = self.selectable(i);
                let lit = selectable && (selected == Some(i) || open == Some(i));
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
                        style::hline_above(g, row.left() + px(1.0), row.right() - px(1.0), gy, style::shade(&p, p.menu_background, 1.16));
                        style::hline(g, row.left() + px(1.0), row.right() - px(1.0), gy, style::c(p.shine));
                    }
                    MenuItem::Action(a) => {
                        if lit {
                            style::fill_rect(g, face, style::c(p.menu_selected_background));
                        }
                        let ink = if a.minimized && a.enabled && !lit { style::c(olo_theme::mix(p.menu_item_text, p.menu_background, 0.38)) } else { text(a.enabled) };
                        let m = Bounds { origin: point(row.left() + px(10.0 - 4.5), cy - px(4.5)), size: size(px(9.0), px(9.0)) };
                        if a.checked {
                            check_mark(g, m, ink);
                        } else if a.minimized {
                            diamond(g, m, ink);
                        }
                        if let (Some(left), Some(icon)) = (icon_left, &a.icon) {
                            item_icon(g, row, left, icon, a.enabled);
                        } else if let (Some(left), Some(color)) = (icon_left, a.swatch) {
                            let r = style::snap_centred(g, point(row.left() + px(left + ICON_SIZE / 2.0), row.center().y), ICON_SIZE - 2.0, ICON_SIZE - 2.0);
                            g.with_element_opacity((!a.enabled).then_some(110.0 / 255.0), |g| style::paint_swatch(g, r, color, &p));
                        }
                        style::label(g, row.left() + px(label_left), cy, style::Align::Left, a.label.clone(), style::PLAIN_SIZE, font.clone(), ink);
                        if let Some(keys) = &a.keys {
                            style::key_combo(g, point(row.right() - px(ITEM_RIGHT), cy), keys, ink);
                        }
                        g.agent(agent::Node {
                            role: "MenuItem",
                            name: Some(SharedString::from(a.label.clone())),
                            bounds: row,
                            enabled: a.enabled,
                            clickable: a.enabled,
                            checked: a.checked.then_some(true),
                            selected: Some(lit),
                            minimized: a.minimized,
                            ..Default::default()
                        });
                    }
                    MenuItem::Submenu(m) => {
                        if lit {
                            style::fill_rect(g, face, style::c(p.menu_selected_background));
                        }
                        let ink = text(selectable);
                        if let (Some(left), Some(icon)) = (icon_left, &m.icon) {
                            item_icon(g, row, left, icon, selectable);
                        }
                        style::label(g, row.left() + px(label_left), cy, style::Align::Left, m.title.clone(), style::PLAIN_SIZE, font.clone(), ink);
                        // A triangle pointing right.
                        let tx = row.right() - px(ITEM_RIGHT);
                        let mut path = PathBuilder::fill();
                        path.move_to(point(tx - px(3.0), cy - px(4.0)));
                        path.line_to(point(tx + px(1.0), cy));
                        path.line_to(point(tx - px(3.0), cy + px(4.0)));
                        path.close();
                        if let Ok(path) = path.build() {
                            g.paint_path(path, ink);
                        }
                        g.agent(agent::Node {
                            role: "MenuItem",
                            name: Some(SharedString::from(m.title.clone())),
                            bounds: row,
                            enabled: selectable,
                            clickable: selectable,
                            selected: Some(lit),
                            ..Default::default()
                        });
                    }
                }
            }
        });
    }
}

impl View for MenuPopup {
    type Message = PopupMessage;

    fn message(&mut self, message: PopupMessage, cx: &mut Cx<Self>) {
        match message {
            PopupMessage::Tick => {
                let Some((dir, _, last)) = self.scrolling.as_mut() else { return };
                let now = Instant::now();
                let step = ARROW_SPEED * (now - *last).as_secs_f32();
                *last = now;
                let dir = *dir;
                if self.scroll_to(self.scroll + dir * step) {
                    cx.invalidate();
                } else if let Some((_, timer, _)) = self.scrolling.take() {
                    cx.looper().cancel(timer);
                }
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div()
            .size_full()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(|m: &mut Self, _, _| m.pressed_inside = true))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::wheel))
            .child(cx.canvas(Self::paint).size_full())
    }

    fn window_closing(&mut self, cx: &mut Cx<Self>) {
        self.close_sub(cx);
        if let Some((_, timer, _)) = self.scrolling.take() {
            cx.looper().cancel(timer);
        }
        // Whoever opened it hears, once this is over.
        if let Some(on_close) = self.on_close.take() {
            let origin = self.origin;
            cx.looper().defer(move |looper| {
                olo_interface::with_window_on(looper, origin, |cx| on_close(cx));
            });
        }
    }
}

/// A minimized window's mark: a hollow diamond, its middle on the
/// device grid, a line 1.2 points wide.
pub fn diamond(g: &mut Painter, r: Bounds<Pixels>, color: Hsla) {
    let m = style::snap_centred(g, r.center(), 8.0, 8.0).center();
    let h = f32::from(r.size.width.min(r.size.height)) / 2.0 - 0.5;
    let mut path = PathBuilder::stroke(px(1.2));
    path.move_to(point(m.x, m.y - px(h)));
    path.line_to(point(m.x + px(h), m.y));
    path.line_to(point(m.x, m.y + px(h)));
    path.line_to(point(m.x - px(h), m.y));
    path.close();
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
    }
}

// ---- the menu bar ----------------------------------------------------------

/// As tall as a tab strip's row and a pane's head, so the chrome under it
/// keeps one rhythm.
pub const BAR_HEIGHT: f32 = 26.0;
const BAR_PAD: f32 = 8.0;

/// What a bar's drawing and the bar's state share: which menu is open.
#[derive(Default)]
struct BarShared {
    open: Cell<Option<usize>>,
}

/// A window's menu bar, as the kit keeps it with the window: its menus
/// (keys resolved), where commands go, which menu is open.
struct Bar {
    menus: Vec<Menu>,
    on_command: OnChoose,
    /// The menu open, its popup, and its title (in the window).
    open: Option<(usize, LocalWindow<MenuPopup>, Bounds<Pixels>)>,
    /// The open menu's title was pressed since it opened: the next press
    /// there puts it away (one that slid open stays for its press, as on
    /// BeOS).
    pressed: bool,
    /// The menu the press now under way put away (`press_in`), so a
    /// press on its own title doesn't open it again.
    closed: Option<usize>,
    shared: Rc<BarShared>,
    /// The bar's element, to draw again when the open menu changes.
    node: Option<NodeRef>,
    /// Its keys are a terminal-style window's ([`MenuBar::raw`]).
    raw: bool,
}

impl Default for Bar {
    fn default() -> Bar {
        Bar { menus: Vec::new(), on_command: Rc::new(on_command), open: None, pressed: false, closed: None, shared: Rc::default(), node: None, raw: false }
    }
}

fn repaint_bar(cx: &mut WindowCx) {
    if let Some(node) = cx.window_state::<Bar>().node {
        cx.repaint_node(node);
    }
}

/// BMenuBar: the window's menus across its top (the menu colour, a light
/// top and left edge, a dark line under it; the open menu's title in the
/// selection colours). A press on a title opens its menu as a popup
/// under it; with one open, the pointer sliding across the bar opens the
/// others; a press on the open title puts it away. The menus' keys come
/// from the keyboard profile ([`resolve`]), and the window takes them
/// when nothing in it did (a text field's ⌘C is its own). Agents read
/// each title (`MenuBarItem`), and the command palette finds the
/// commands.
pub struct MenuBar {
    titles: Vec<(SharedString, f32)>,
    shared: Rc<BarShared>,
}

impl MenuBar {
    /// `menus` across the top of the window `cx` is; what's chosen or
    /// pressed goes to the window's own view (`View::command`).
    pub fn new(cx: &mut WindowCx, menus: Vec<Menu>) -> MenuBar {
        Self::with(cx, menus, on_command, false)
    }

    /// For a terminal-style window (one of app_server's `terminal_apps`,
    /// whose command key arrives with Shift: ⌘C as Ctrl+Shift+C): its keys
    /// as they're pressed there ([`resolve`]).
    pub fn raw(cx: &mut WindowCx, menus: Vec<Menu>) -> MenuBar {
        Self::with(cx, menus, on_command, true)
    }

    /// [`MenuBar::new`], what's chosen going to `on_command` in the
    /// window.
    pub fn with(cx: &mut WindowCx, mut menus: Vec<Menu>, on_command: impl Fn(&str, &mut WindowCx) + 'static, raw: bool) -> MenuBar {
        resolve(&mut menus, raw);
        let font = style::ui_font(FontWeight::NORMAL);
        let titles = menus.iter().map(|m| (SharedString::from(m.title.clone()), f32::from(style::text_width(m.title.clone(), style::PLAIN_SIZE, font.clone())) + 2.0 * BAR_PAD)).collect();
        let bar = cx.window_state::<Bar>();
        bar.menus = menus;
        bar.raw = raw;
        bar.on_command = Rc::new(on_command);
        MenuBar { titles, shared: bar.shared.clone() }
    }
}

/// The window's bar's menus as things are now, said outside its body and
/// without drawing it: BeOS's `MenusBeginning`, for a view that looks a
/// shortcut up itself ([`shortcut_command`]). A bar's menus are as the
/// window's last body left them, and an item that's off has no
/// shortcut, so its keystroke goes on to whatever takes it next. Where
/// an item's `enabled` goes by something that may have changed a moment
/// ago (Terminal's Copy, by a selection just made), asking for the
/// window's body isn't soon enough on a busy computer: say the menus
/// first, then look. The bar's titles are as they were.
pub fn menus_beginning(cx: &mut WindowCx, mut menus: Vec<Menu>) {
    let bar = cx.window_state::<Bar>();
    resolve(&mut menus, bar.raw);
    bar.menus = menus;
}

/// Where each title is in a bar at `b`.
fn title_rects(titles: &[(SharedString, f32)], b: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    // The first title's text lines up with the window's content.
    let mut x = b.left() + px(olo_kit_base::metrics::WINDOW_INSET - BAR_PAD);
    titles
        .iter()
        .map(|(_, w)| {
            let r = Bounds { origin: point(x, b.top() + px(1.0)), size: size(px(*w), px(BAR_HEIGHT - 2.0)) };
            x += px(*w);
            r
        })
        .collect()
}

/// The window's menu bar's command for `k` (Frame takes the keys nothing
/// else in the window did), and where to send it.
pub(crate) fn bar_shortcut(cx: &mut WindowCx, k: &olo_interface::Keystroke) -> Option<(String, OnChoose)> {
    let bar = cx.window_state::<Bar>();
    Some((find_shortcut(&bar.menus, k)?, bar.on_command.clone()))
}

/// The command of the window's menu bar that keystroke `k` chooses, if
/// any (for a view that takes the keys for itself, to let one through).
pub fn shortcut_command(cx: &mut WindowCx, k: &olo_interface::Keystroke) -> Option<String> {
    bar_shortcut(cx, k).map(|(command, _)| command)
}

/// The window's menus, for the command palette and agents: its bar's,
/// else what it offers without one ([`set_menus`]).
pub(crate) fn window_menus(cx: &mut WindowCx) -> Vec<Menu> {
    let bar = cx.window_state::<Bar>().menus.clone();
    if !bar.is_empty() {
        return bar;
    }
    cx.window_state::<Offered>().0.clone()
}

/// The menus a window with no menu bar offers the command palette and
/// agents (a calculator's context menu): said as its view says what it
/// shows. Their commands go to the window's own view (`View::command`).
pub fn set_menus(cx: &mut WindowCx, menus: Vec<Menu>) {
    cx.window_state::<Offered>().0 = menus;
}

/// What a window offers without a bar ([`set_menus`]).
#[derive(Default)]
struct Offered(Vec<Menu>);

/// Opens bar menu `i` under its title `title` (in the window), by a
/// press on it or sliding onto it.
fn open_bar_menu(cx: &mut WindowCx, i: usize, title: Bounds<Pixels>, pressed: bool) {
    let bar = cx.window_state::<Bar>();
    let Some(menu) = bar.menus.get(i).cloned() else { return };
    let on_command = bar.on_command.clone();
    close_bar_menu(cx);
    // Its popup gone (chosen, put away, a click elsewhere): no menu open,
    // unless another has opened since. By its popup, not its place: the
    // menu it replaced goes after the new one opens, and a menu that
    // slid open and then was pressed open again is the same menu.
    let mine: Rc<Cell<Option<WindowId>>> = Rc::default();
    let closing = mine.clone();
    let on_close: OnClose = Rc::new(move |cx: &mut WindowCx| {
        let bar = cx.window_state::<Bar>();
        if bar.open.is_some_and(|(_, h, _)| Some(h.id()) == closing.get()) {
            bar.open = None;
            bar.shared.open.set(None);
            repaint_bar(cx);
        }
    });
    let place = Place { anchor: title, corner: Corner::BottomLeft, ..Place::default() };
    let origin = cx.window_id();
    let handle = open(cx, menu, place, on_command, None, origin, Some(on_close));
    mine.set(handle.map(|h| h.id()));
    let bar = cx.window_state::<Bar>();
    bar.open = handle.map(|h| (i, h, title));
    bar.pressed = pressed;
    bar.shared.open.set(handle.map(|_| i));
    repaint_bar(cx);
}

/// Puts the bar's open menu away.
fn close_bar_menu(cx: &mut WindowCx) {
    let bar = cx.window_state::<Bar>();
    bar.shared.open.set(None);
    if let Some((_, handle, _)) = bar.open.take() {
        handle.update(cx, |m, cx| m.close(cx));
    }
    repaint_bar(cx);
}

impl IntoElement for MenuBar {
    fn into_element(self) -> Element {
        let (titles, shared) = (Rc::new(self.titles), self.shared);
        let (down, slide) = (titles.clone(), titles.clone());
        canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.menu_background));
            // BeOS's bevel: light top and left, dark bottom.
            style::hline(g, b.left(), b.right(), b.top(), style::c(p.shine));
            style::vline(g, b.left(), b.top(), b.bottom(), style::c(p.shine));
            style::hline_above(g, b.left(), b.right(), b.bottom(), style::tint(p.menu_background, 1.4));
            let open = shared.open.get();
            let font = style::ui_font(FontWeight::NORMAL);
            for (i, ((title, _), r)) in titles.iter().zip(title_rects(&titles, b)).enumerate() {
                let ink = if open == Some(i) {
                    // Between the bar's bevel lines, on device pixels.
                    let line = style::hairline(g);
                    style::fill_rect(g, Bounds::from_corners(point(r.left(), b.top() + line), point(r.right(), b.bottom() - line)), style::c(p.menu_selected_background));
                    style::c(p.menu_selected_item_text)
                } else {
                    style::c(p.menu_item_text)
                };
                style::label(g, r.left() + px(BAR_PAD), r.center().y, style::Align::Left, title.clone(), style::PLAIN_SIZE, font.clone(), ink);
                g.agent(agent::Node { role: "MenuBarItem", name: Some(title.clone()), bounds: r, enabled: true, clickable: true, selected: Some(open == Some(i)), ..Default::default() });
            }
        })
        // (A window's menus work on what has its keyboard: the bar
        // never takes it.)
        .keeps_focus()
        // Menus open on the press, as on BeOS.
        .on_mouse_down(MouseButton::Left, move |e, cx| {
            let b = cx.listener_bounds();
            let node = cx.listener_node();
            cx.window_state::<Bar>().node = node;
            let rects = title_rects(&down, b);
            let Some(i) = rects.iter().position(|r| r.contains(&e.position)) else { return };
            // Its menu was open: the press put it away (`press_in`), and
            // away it stays; or it slid open, and stays for this press.
            let bar = cx.window_state::<Bar>();
            let (put_away, showing) = (bar.closed.take() == Some(i), bar.open.is_some_and(|(o, ..)| o == i));
            if !put_away && !showing {
                open_bar_menu(cx, i, rects[i], true);
            }
            cx.stop_propagation();
        })
        // Sliding across the bar with one open opens the others.
        .on_mouse_move(move |e, cx| {
            let b = cx.listener_bounds();
            let node = cx.listener_node();
            let bar = cx.window_state::<Bar>();
            bar.node = node;
            let Some((open, ..)) = bar.open else { return };
            let rects = title_rects(&slide, b);
            if let Some(i) = rects.iter().position(|r| r.contains(&e.position))
                && i != open
            {
                open_bar_menu(cx, i, rects[i], false);
            }
        })
        .w_full()
        .h(px(BAR_HEIGHT))
        .flex_shrink_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tabbed app's Move to Window ▸: the other windows by title in
    /// natural order, each its command; none, a greyed line.
    #[test]
    fn move_to_window_lists_the_others() {
        let m = Menu::move_to_window("Move to Window", [("Notes 10".to_owned(), "to:10".to_owned()), ("notes 2".to_owned(), "to:2".to_owned()), ("Build".to_owned(), "to:b".to_owned())]);
        assert_eq!(m.title, "Move to Window");
        let items: Vec<(String, String, bool)> = m
            .items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action(a) => Some((a.label.to_string(), a.command.clone(), a.enabled)),
                _ => None,
            })
            .collect();
        assert_eq!(items, [("Build".into(), "to:b".into(), true), ("notes 2".into(), "to:2".into(), true), ("Notes 10".into(), "to:10".into(), true)]);
        let none = Menu::move_to_window("Move Group to Window", std::iter::empty());
        assert!(matches!(&none.items[..], [MenuItem::Action(a)] if a.label.to_string() == "No Other Windows" && !a.enabled));
    }

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
