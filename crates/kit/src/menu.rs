//! Menus: the model apps return from [`crate::App::menu_bar`], the BeOS
//! menu bar the kit draws at the top of a window, and the popup menus
//! (real `xdg_popup` surfaces, so they can extend past the window).

use egui::{Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::style::{self, c};

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    Action(Action),
    Separator,
    Submenu(Menu),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub label: String,
    pub command: String,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    pub checked: bool,
}

/// A command-key shortcut. The command key is Ctrl (which app_server's
/// keymap produces for the Mac's ⌘), optionally with Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub shift: bool,
}

impl Shortcut {
    pub fn label(&self) -> String {
        let key = match self.key {
            Key::Comma => ",".into(),
            Key::Period => ".".into(),
            Key::Slash => "/".into(),
            Key::Plus | Key::Equals => "+".into(),
            Key::Minus => "-".into(),
            Key::Backspace => "⌫".into(),
            Key::Enter => "↵".into(),
            Key::ArrowUp => "↑".into(),
            Key::ArrowDown => "↓".into(),
            Key::ArrowLeft => "←".into(),
            Key::ArrowRight => "→".into(),
            Key::Delete => "⌦".into(),
            k => k.name().to_uppercase(),
        };
        if self.shift { format!("⇧{key}") } else { key }
    }
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Self {
        Menu { title: title.into(), items: Vec::new() }
    }
    /// Add an item: `label`, command id, optional shortcut key.
    pub fn item(mut self, label: impl Into<String>, command: impl Into<String>, key: Option<Key>) -> Self {
        self.items.push(MenuItem::Action(Action {
            label: label.into(),
            command: command.into(),
            shortcut: key.map(|key| Shortcut { key, shift: false }),
            enabled: true,
            checked: false,
        }));
        self
    }
    pub fn item_shift(mut self, label: impl Into<String>, command: impl Into<String>, key: Key) -> Self {
        self.items.push(MenuItem::Action(Action {
            label: label.into(),
            command: command.into(),
            shortcut: Some(Shortcut { key, shift: true }),
            enabled: true,
            checked: false,
        }));
        self
    }
    pub fn action(mut self, action: Action) -> Self {
        self.items.push(MenuItem::Action(action));
        self
    }
    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::Separator);
        self
    }
    pub fn submenu(mut self, menu: Menu) -> Self {
        self.items.push(MenuItem::Submenu(menu));
        self
    }
    /// Mark the last item checked / unchecked.
    pub fn checked(mut self, on: bool) -> Self {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.checked = on;
        }
        self
    }
    /// Enable / disable the last item.
    pub fn enabled(mut self, on: bool) -> Self {
        if let Some(MenuItem::Action(a)) = self.items.last_mut() {
            a.enabled = on;
        }
        self
    }

    /// Find the command bound to a shortcut, searching submenus too.
    pub fn find_shortcut(&self, key: Key, shift: bool) -> Option<&str> {
        self.items.iter().find_map(|item| match item {
            MenuItem::Action(a)
                if a.enabled && a.shortcut.is_some_and(|s| s.key == key && s.shift == shift) =>
            {
                Some(a.command.as_str())
            }
            MenuItem::Submenu(m) => m.find_shortcut(key, shift),
            _ => None,
        })
    }

    /// Follow a path of submenu indices.
    pub fn at_path(&self, path: &[usize]) -> Option<&Menu> {
        let mut menu = self;
        for &i in path {
            match menu.items.get(i)? {
                MenuItem::Submenu(m) => menu = m,
                _ => return None,
            }
        }
        Some(menu)
    }
}

impl Action {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Action { label: label.into(), command: command.into(), shortcut: None, enabled: true, checked: false }
    }
    pub fn key(mut self, key: Key) -> Self {
        self.shortcut = Some(Shortcut { key, shift: false });
        self
    }
    pub fn shift_key(mut self, key: Key) -> Self {
        self.shortcut = Some(Shortcut { key, shift: true });
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
}

pub fn find_shortcut(menus: &[Menu], key: Key, shift: bool) -> Option<String> {
    menus.iter().find_map(|m| m.find_shortcut(key, shift)).map(str::to_owned)
}

// ---- in-window popups ----

/// Where an ad-hoc popup menu (context menu, menu field) appears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuAnchor {
    /// At a point, opening down and right (context menus).
    At(Pos2),
    /// Below a rect (pull-downs).
    Below(Rect),
    /// Over a rect with item `n` lying on top of it (BeOS menu fields).
    Over(Rect, usize),
}

/// A request from a widget for the kit to open a popup menu. The command
/// of the chosen item comes back through [`take_chosen`] on the next frame.
#[derive(Debug, Clone)]
pub struct MenuRequest {
    pub menu: Menu,
    pub source: egui::Id,
    pub anchor: MenuAnchor,
    pub selected: Option<usize>,
}

fn request_key() -> egui::Id {
    egui::Id::unique("tabula-menu-request")
}

/// Ask the kit to open `menu` as a popup once this frame is drawn.
pub fn request_popup(ctx: &egui::Context, request: MenuRequest) {
    ctx.data_mut(|d| d.insert_temp(request_key(), request));
}

pub(crate) fn take_request(ctx: &egui::Context) -> Option<MenuRequest> {
    ctx.data_mut(|d| {
        let r = d.get_temp::<MenuRequest>(request_key());
        d.remove::<MenuRequest>(request_key());
        r
    })
}

pub(crate) fn deliver_chosen(ctx: &egui::Context, source: egui::Id, command: String) {
    ctx.data_mut(|d| d.insert_temp(source.with("menu-chosen"), command));
}

fn open_key() -> egui::Id {
    egui::Id::unique("tabula-menu-open")
}

pub(crate) fn set_open_source(ctx: &egui::Context, source: Option<egui::Id>) {
    ctx.data_mut(|d| match source {
        Some(s) => {
            d.insert_temp(open_key(), s);
        }
        None => d.remove::<egui::Id>(open_key()),
    });
}

/// Whether the popup menu `source` requested is showing right now.
pub fn is_open(ctx: &egui::Context, source: egui::Id) -> bool {
    ctx.data(|d| d.get_temp::<egui::Id>(open_key())) == Some(source)
}

/// The command chosen from the popup `source` requested, if one was.
pub fn take_chosen(ctx: &egui::Context, source: egui::Id) -> Option<String> {
    ctx.data_mut(|d| d.remove_temp::<String>(source.with("menu-chosen")))
}

/// Vertical offset of item `n`'s top within a popup for `menu`.
pub fn item_offset(menu: &Menu, n: usize) -> f32 {
    POPUP_BORDER
        + 2.0
        + menu
            .items
            .iter()
            .take(n)
            .map(|i| if matches!(i, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT })
            .sum::<f32>()
}

// ---- metrics ------------------------------------------------------------

pub const BAR_HEIGHT: f32 = 21.0;
const BAR_PAD: f32 = 8.0;
pub const ITEM_HEIGHT: f32 = 20.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const ITEM_LEFT: f32 = 20.0;
const ITEM_RIGHT: f32 = 14.0;
const SHORTCUT_GAP: f32 = 28.0;
pub const POPUP_BORDER: f32 = 1.0;

fn font() -> FontId {
    FontId::proportional(style::PLAIN_SIZE)
}

/// What the bar wants after a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarState {
    /// Index of the open menu and the title's rect (window coordinates).
    pub open: Option<(usize, Rect)>,
}

/// Draw the menu bar across the top of `ui`; returns its rect. Updates
/// `state.open` when a title is clicked or hovered while a menu is open.
pub fn menu_bar(ui: &mut Ui, menus: &[Menu], state: &mut BarState) -> Rect {
    let p = style::palette(ui.ctx());
    let full = ui.max_rect();
    let rect = Rect::from_min_size(full.min, vec2(full.width(), BAR_HEIGHT));
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 0.0, c(p.menu_background));
    // BeOS bevel: light top/left, dark bottom.
    style::hline(&painter, rect.left(), rect.right(), rect.top(), c(p.shine));
    style::vline(&painter, rect.left(), rect.top(), rect.bottom(), c(p.shine));
    style::hline(&painter, rect.left(), rect.right(), rect.bottom() - 1.0, style::darken(p.menu_background, 1.4));

    let mut x = rect.left() + 3.0;
    for (i, menu) in menus.iter().enumerate() {
        let galley = painter.layout_no_wrap(menu.title.clone(), font(), Color32::BLACK);
        let w = galley.size().x + 2.0 * BAR_PAD;
        let item = Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(w, BAR_HEIGHT - 2.0));
        let resp = ui.interact(item, egui::Id::unique(("menubar", i)), Sense::click());
        let open_here = state.open.is_some_and(|(o, _)| o == i);
        // Menus open on press, as on BeOS. (Not on click: the release of
        // that press may arrive after the menu was dismissed.)
        if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.any_pressed()) {
            if !open_here {
                state.open = Some((i, item));
            }
        } else if resp.hovered() && state.open.is_some() && !open_here {
            // Sliding across the bar switches menus, as on BeOS.
            state.open = Some((i, item));
        }
        let open_here = state.open.is_some_and(|(o, _)| o == i);
        let text_color = if open_here {
            painter.rect_filled(item, 0.0, c(p.menu_selected_background));
            c(p.menu_selected_item_text)
        } else {
            c(p.menu_item_text)
        };
        style::label(&painter, item.left() + BAR_PAD, item.center().y, egui::Align::Min, &menu.title, font(), text_color);
        x += w;
    }
    ui.advance_cursor_after_rect(rect);
    rect
}

/// Logical size of a popup for `menu`.
pub fn popup_size(ctx: &egui::Context, menu: &Menu) -> Vec2 {
    let (mut label_w, mut key_w, mut h) = (0.0f32, 0.0f32, 0.0f32);
    let measure = |s: &str| ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font(), Color32::BLACK).size().x);
    for item in &menu.items {
        match item {
            MenuItem::Action(a) => {
                label_w = label_w.max(measure(&a.label));
                if let Some(s) = a.shortcut {
                    key_w = key_w.max(style::shortcut_width(ctx, &s.label()));
                }
                h += ITEM_HEIGHT;
            }
            MenuItem::Submenu(m) => {
                label_w = label_w.max(measure(&m.title));
                key_w = key_w.max(10.0);
                h += ITEM_HEIGHT;
            }
            MenuItem::Separator => h += SEPARATOR_HEIGHT,
        }
    }
    let gap = if key_w > 0.0 { SHORTCUT_GAP } else { 0.0 };
    let w = (ITEM_LEFT + label_w + gap + key_w + ITEM_RIGHT).max(120.0).ceil();
    vec2(w + 2.0 * POPUP_BORDER, (h + 2.0 * POPUP_BORDER + 4.0).ceil())
}

#[derive(Debug, Clone, Default)]
pub struct PopupState {
    /// Keyboard/pointer highlighted item.
    pub selected: Option<usize>,
    /// Submenu currently open from this popup: item index and its rect.
    pub open_sub: Option<(usize, Rect)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PopupResult {
    None,
    Chosen(String),
    /// Open (or switch to) the submenu at this item.
    OpenSubmenu(usize, Rect),
    CloseSubmenu,
    /// Escape / Left: close this level.
    Back,
}

/// Draw a popup menu filling `ui` and handle pointer/keyboard.
pub fn popup_ui(ui: &mut Ui, menu: &Menu, state: &mut PopupState) -> PopupResult {
    let p = style::palette(ui.ctx());
    let rect = ui.max_rect();
    let painter = ui.painter_at(rect);
    let mut result = PopupResult::None;

    painter.rect_filled(rect, 0.0, c(p.menu_background));
    // Outline, then a light inner bevel on top/left.
    style::frame_rect(&painter, rect, style::darken(p.menu_background, 1.55));
    let inner = rect.shrink(POPUP_BORDER);
    style::hline(&painter, inner.left(), inner.right(), inner.top(), c(p.shine));
    style::vline(&painter, inner.left(), inner.top(), inner.bottom(), c(p.shine));

    // Layout.
    let mut y = inner.top() + 2.0;
    let mut rows = Vec::with_capacity(menu.items.len());
    for item in &menu.items {
        let h = if matches!(item, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT };
        rows.push(Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), h)));
        y += h;
    }

    // Pointer.
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let released = ui.input(|i| i.pointer.any_released());
    let hovered = pointer.and_then(|pos| rows.iter().position(|r| r.contains(pos)));
    if let Some(i) = hovered
        && ui.input(|i| i.pointer.delta() != Vec2::ZERO || i.pointer.any_pressed())
    {
        state.selected = Some(i);
    }

    // Keyboard.
    let selectable = |i: usize| match &menu.items[i] {
        MenuItem::Action(a) => a.enabled,
        MenuItem::Submenu(_) => true,
        MenuItem::Separator => false,
    };
    let step = |from: Option<usize>, dir: isize| -> Option<usize> {
        let n = menu.items.len() as isize;
        let mut i = from.map(|i| i as isize).unwrap_or(if dir > 0 { -1 } else { n });
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if selectable(i as usize) {
                return Some(i as usize);
            }
        }
        None
    };
    ui.input(|input| {
        for event in &input.events {
            if let egui::Event::Key { key, pressed: true, .. } = event {
                match key {
                    Key::ArrowDown => state.selected = step(state.selected, 1),
                    Key::ArrowUp => state.selected = step(state.selected, -1),
                    Key::ArrowRight => {
                        if let Some(i) = state.selected
                            && matches!(menu.items[i], MenuItem::Submenu(_))
                        {
                            result = PopupResult::OpenSubmenu(i, rows[i]);
                        }
                    }
                    Key::ArrowLeft | Key::Escape => result = PopupResult::Back,
                    Key::Enter | Key::Space => {
                        if let Some(i) = state.selected {
                            match &menu.items[i] {
                                MenuItem::Action(a) if a.enabled => result = PopupResult::Chosen(a.command.clone()),
                                MenuItem::Submenu(_) => result = PopupResult::OpenSubmenu(i, rows[i]),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    for (i, (item, row)) in menu.items.iter().zip(&rows).enumerate() {
        let selected = state.selected == Some(i) && selectable(i);
        match item {
            MenuItem::Separator => {
                let y = row.center().y.floor();
                style::hline(&painter, row.left() + 1.0, row.right() - 1.0, y - 1.0, style::shade(&p, p.menu_background, 1.16));
                style::hline(&painter, row.left() + 1.0, row.right() - 1.0, y, c(p.shine));
            }
            MenuItem::Action(a) => {
                let text_color = if !a.enabled {
                    style::disabled_text(&p)
                } else if selected {
                    c(p.menu_selected_item_text)
                } else {
                    c(p.menu_item_text)
                };
                if selected {
                    painter.rect_filled(*row, 0.0, c(p.menu_selected_background));
                }
                if a.checked {
                    style::check_mark(&painter, Rect::from_center_size(pos2(row.left() + 10.0, row.center().y), vec2(9.0, 9.0)), text_color);
                }
                style::label(&painter, row.left() + ITEM_LEFT, row.center().y, egui::Align::Min, &a.label, font(), text_color);
                if let Some(s) = a.shortcut {
                    style::shortcut(&painter, pos2(row.right() - ITEM_RIGHT, row.center().y), &s.label(), text_color);
                }
                if a.enabled && released && hovered == Some(i) && result == PopupResult::None {
                    result = PopupResult::Chosen(a.command.clone());
                }
            }
            MenuItem::Submenu(m) => {
                let open = state.open_sub.is_some_and(|(s, _)| s == i);
                let hl = selected || open;
                if hl {
                    painter.rect_filled(*row, 0.0, c(p.menu_selected_background));
                }
                let text_color = if hl { c(p.menu_selected_item_text) } else { c(p.menu_item_text) };
                style::label(&painter, row.left() + ITEM_LEFT, row.center().y, egui::Align::Min, &m.title, font(), text_color);
                // Right-pointing triangle.
                let cx = row.right() - ITEM_RIGHT;
                let cy = row.center().y;
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(cx - 3.0, cy - 4.0), pos2(cx + 1.0, cy), pos2(cx - 3.0, cy + 4.0)],
                    text_color,
                    Stroke::NONE,
                ));
                if hovered == Some(i) && !open && result == PopupResult::None {
                    result = PopupResult::OpenSubmenu(i, *row);
                }
            }
        }
    }
    if let Some((s, _)) = state.open_sub
        && hovered.is_some_and(|h| h != s)
        && result == PopupResult::None
    {
        result = PopupResult::CloseSubmenu;
    }
    result
}
