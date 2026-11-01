//! BeOS menus: a [`Menu`] of items (commands, separators, submenus),
//! shown as a popup over a window ([`popup`], [`context_menu`]): a
//! Wayland popup, so it can reach past the window's edge, which the
//! compositor keeps on screen (flipped or slid). Drawn as the egui kit
//! draws them: the menu colour in a dark outline with a light inner edge
//! on top and left, the item under the pointer or chosen from the
//! keyboard in the selection colours, check marks, separators as a
//! groove, a triangle for a submenu, which opens beside its item. ↑ ↓
//! choose, → opens a submenu, ← and Escape close a level, Enter or Space
//! takes the item; a release over an item takes it. Agents read each
//! item (`MenuItem`, with whether it's checked or can be chosen).

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Corner, FocusHandle, Focusable, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Render, SharedString, Styled as _, Window, WindowBounds, WindowHandle, WindowKind, canvas, div, point, px, size,
};

use crate::{agent, style, theme};

pub const ITEM_HEIGHT: f32 = 20.0;
const SEPARATOR_HEIGHT: f32 = 10.0;
const ITEM_LEFT: f32 = 20.0;
const ITEM_RIGHT: f32 = 14.0;
const SHORTCUT_GAP: f32 = 28.0;
const POPUP_BORDER: f32 = 1.0;
const POPUP_PAD: f32 = 4.0;
const MIN_WIDTH: f32 = 120.0;

/// A menu: its title (a menu bar's, a submenu's item) and items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
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
    /// Its keys as shown ("⌘C").
    pub keys: Option<String>,
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Menu {
        Menu { title: title.into(), items: Vec::new() }
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

    pub fn submenu(mut self, menu: Menu) -> Menu {
        self.items.push(MenuItem::Submenu(menu));
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

    /// The last item's keys, as shown ("⌘C").
    pub fn keys(mut self, keys: impl Into<String>) -> Menu {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.keys = Some(keys.into());
        }
        self
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
                    MenuItem::Action(a) => out.push(olo_kit_base::commands::Entry { path: path.clone(), label: a.label.clone(), command: a.command.clone(), keys: a.keys.clone(), enabled: a.enabled, checked: a.checked }),
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
                    key_w = key_w.max(f32::from(style::text_width(window, keys.clone(), style::PLAIN_SIZE, font.clone())));
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
    let w = ((ITEM_LEFT + label_w + gap + key_w + ITEM_RIGHT).max(MIN_WIDTH) / 2.0).ceil() * 2.0;
    (w + 2.0 * POPUP_BORDER, h + 2.0 * POPUP_PAD)
}

/// Opens `menu` over `window`, its top left at `corner` of `anchor` (in
/// the window's content). `on_choose` gets the command chosen, in
/// `window`.
pub fn popup(window: &mut Window, cx: &mut App, menu: Menu, anchor: Bounds<Pixels>, corner: Corner, on_choose: impl Fn(&str, &mut Window, &mut App) + 'static) -> Option<WindowHandle<MenuPopup>> {
    open(window, cx, menu, Place { anchor, corner, ..Place::default() }, Rc::new(on_choose), None)
}

/// Opens `menu` over `field` (in the window's content) with item
/// `current` lying over it, chosen, as a menu field's on BeOS, and at
/// least as wide as the field.
pub fn popup_over(window: &mut Window, cx: &mut App, menu: Menu, field: Bounds<Pixels>, current: usize, on_choose: impl Fn(&str, &mut Window, &mut App) + 'static) -> Option<WindowHandle<MenuPopup>> {
    let dy = POPUP_PAD + item_offset(&menu, current) - (f32::from(field.size.height) - ITEM_HEIGHT) / 2.0;
    let place = Place { anchor: field, corner: Corner::TopLeft, offset: point(px(0.0), px(-dy.round())), min_width: f32::from(field.size.width).round(), selected: Some(current) };
    open(window, cx, menu, place, Rc::new(on_choose), None)
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

fn open(window: &mut Window, cx: &mut App, menu: Menu, place: Place, on_choose: OnChoose, parent_menu: Option<WindowHandle<MenuPopup>>) -> Option<WindowHandle<MenuPopup>> {
    let (w, h) = popup_size(window, &menu);
    let w = w.max(place.min_width);
    let Place { anchor, corner, offset, selected, .. } = place;
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
            let view = cx.new(|cx| MenuPopup { menu, selected, focus: cx.focus_handle(), on_choose, origin, owner, sub: None, rows: Rc::default(), pressed_inside: false });
            window.focus(&view.focus_handle(cx));
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
        self.rows.borrow().iter().position(|r| r.contains(&p))
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
        if let Some(handle) = open(window, cx, menu, place, self.on_choose.clone(), me) {
            self.sub = Some((i, handle));
        }
        cx.notify();
    }

    fn close_sub(&mut self, cx: &mut Context<Self>) {
        if let Some((_, handle)) = self.sub.take() {
            let _ = handle.update(cx, |m, window, cx| m.close(window, cx));
        }
    }

    /// Closes this menu and its submenus.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sub(cx);
        window.remove_window();
    }

    /// Closes every menu, up to the first.
    fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.owner;
        self.close(window, cx);
        if let Some(parent) = owner.downcast::<MenuPopup>() {
            let _ = parent.update(cx, |m, window, cx| m.close_all(window, cx));
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "down" => self.selected = self.step(1),
            "up" => self.selected = self.step(-1),
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
        let paint = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                style::fill_rect(window, b, style::c(p.menu_background));
                // The outline, then a light inner edge on top and left.
                style::frame_rect(window, b, style::tint(p.menu_background, 1.55));
                let lined = style::inset(window, b, 1.0);
                style::hline(window, lined.left(), lined.right(), lined.top(), style::c(p.shine));
                style::vline(window, lined.left(), lined.top(), lined.bottom(), style::c(p.shine));
                let mut y = b.top() + px(POPUP_PAD);
                let (left, right) = (b.left() + px(POPUP_BORDER), b.right() - px(POPUP_BORDER));
                let mut placed = rows.borrow_mut();
                placed.clear();
                let font = style::ui_font(FontWeight::NORMAL);
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
                            let ink = text(a.enabled);
                            if a.checked {
                                let m = Bounds { origin: point(row.left() + px(10.0 - 4.5), cy - px(4.5)), size: size(px(9.0), px(9.0)) };
                                check_mark(window, m, ink);
                            }
                            let r = style::label(window, cx, row.left() + px(ITEM_LEFT), cy, style::Align::Left, a.label.clone(), style::PLAIN_SIZE, font.clone(), ink);
                            if let Some(keys) = &a.keys {
                                style::label(window, cx, row.right() - px(ITEM_RIGHT), cy, style::Align::Right, keys.clone(), style::PLAIN_SIZE, font.clone(), ink);
                            }
                            let _ = r;
                            agent::record(window, cx, agent::Node { role: "MenuItem", name: Some(SharedString::from(a.label.clone())), bounds: row, enabled: a.enabled, clickable: a.enabled, checked: a.checked.then_some(true), selected: Some(lit), ..Default::default() });
                        }
                        MenuItem::Submenu(m) => {
                            if lit {
                                style::fill_rect(window, face, style::c(p.menu_selected_background));
                            }
                            let ink = text(selectable[i]);
                            style::label(window, cx, row.left() + px(ITEM_LEFT), cy, style::Align::Left, m.title.clone(), style::PLAIN_SIZE, font.clone(), ink);
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
                            agent::record(window, cx, agent::Node { role: "MenuItem", name: Some(SharedString::from(m.title.clone())), bounds: row, enabled: selectable[i], clickable: selectable[i], selected: Some(lit), ..Default::default() });
                        }
                    }
                }
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
