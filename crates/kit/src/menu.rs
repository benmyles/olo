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
            Key::OpenBracket => "[".into(),
            Key::CloseBracket => "]".into(),
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

fn paste_key() -> egui::Id {
    egui::Id::unique("tabula-paste-request")
}

/// Ask the kit to paste the clipboard's text into this window once the
/// frame is drawn (it arrives as an `egui::Event::Paste`, as ⌘V's does).
pub fn request_paste(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(paste_key(), true));
}

pub(crate) fn take_paste(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(paste_key())).unwrap_or(false)
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
/// The bands with scroll arrows at the ends of a menu too tall for the
/// screen.
const SCROLLER_HEIGHT: f32 = 14.0;
/// How fast a menu scrolls with the pointer resting on an arrow (logical
/// pixels a second).
const SCROLL_SPEED: f32 = 12.0 * ITEM_HEIGHT;

fn font() -> FontId {
    FontId::proportional(style::PLAIN_SIZE)
}

/// What the bar wants after a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarState {
    /// Index of the open menu and the title's rect (window coordinates).
    pub open: Option<(usize, Rect)>,
    /// The open menu's title has been pressed since it opened, so the next
    /// press there puts it away (one that slid open stays for its press).
    pub pressed: bool,
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
        // that press may arrive after the menu was dismissed.) The press
        // is read from the events: a touchpad tap brings its release in
        // the same frame, when the button is no longer down.
        let pressed = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, pos, .. } if item.contains(*pos)))
        });
        if pressed && resp.hovered() {
            // Pressing the open menu's title again puts it away.
            if open_here && state.pressed {
                state.open = None;
                state.pressed = false;
            } else {
                state.open = Some((i, item));
                state.pressed = true;
            }
        } else if resp.hovered() && state.open.is_some() && !open_here {
            // Sliding across the bar switches menus, as on BeOS.
            state.open = Some((i, item));
            state.pressed = false;
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
    /// How far a menu taller than its popup (the screen) is scrolled.
    pub scroll: f32,
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
/// Where a popup menu's items are, in a popup at `rect`.
struct PopupLayout {
    inner: Rect,
    /// Between the scroll arrows, if it scrolls.
    area: Rect,
    heights: Vec<f32>,
    scrolls: bool,
    max_scroll: f32,
}

impl PopupLayout {
    fn new(menu: &Menu, rect: Rect) -> PopupLayout {
        let inner = rect.shrink(POPUP_BORDER);
        let heights: Vec<f32> = menu.items.iter().map(|item| if matches!(item, MenuItem::Separator) { SEPARATOR_HEIGHT } else { ITEM_HEIGHT }).collect();
        let content = heights.iter().sum::<f32>() + 4.0;
        let scrolls = content > inner.height() + 0.5;
        let area = if scrolls { Rect::from_min_max(pos2(inner.left(), inner.top() + SCROLLER_HEIGHT), pos2(inner.right(), inner.bottom() - SCROLLER_HEIGHT)) } else { inner };
        PopupLayout { inner, area, max_scroll: (content - area.height()).max(0.0), heights, scrolls }
    }

    /// Each item's row, scrolled by `scroll`.
    fn rows(&self, scroll: f32) -> Vec<Rect> {
        let mut y = self.area.top() + 2.0 - scroll;
        self.heights
            .iter()
            .map(|h| {
                let r = Rect::from_min_size(pos2(self.inner.left(), y), vec2(self.inner.width(), *h));
                y += h;
                r
            })
            .collect()
    }
}

/// What a popup menu of `size` shows, for agents (`commands::tree`): a
/// `Menu` node, then a `MenuItem` for each item in view (separators left
/// out), named by its label, with its shortcut as the value. Ids start at
/// `first_id`.
pub(crate) fn popup_nodes(menu: &Menu, state: &PopupState, size: (u32, u32), first_id: u64) -> Vec<crate::commands::UiNode> {
    use crate::commands::UiNode;
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(size.0 as f32, size.1 as f32));
    let layout = PopupLayout::new(menu, rect);
    let node = |id: u64, parent: Option<u64>, role: &str, name: Option<String>, r: Rect| UiNode {
        id,
        parent,
        role: role.into(),
        name,
        value: None,
        x: r.left(),
        y: r.top(),
        width: r.width(),
        height: r.height(),
        enabled: true,
        clickable: false,
        focused: false,
        checked: None,
        selected: None,
        row: None,
        rows: None,
    };
    let mut nodes = vec![node(first_id, None, "Menu", (!menu.title.is_empty()).then(|| menu.title.clone()), rect)];
    for (i, (item, row)) in menu.items.iter().zip(layout.rows(state.scroll.clamp(0.0, layout.max_scroll))).enumerate() {
        // Only what shows between the arrows can be clicked.
        let Some(row) = Some(row.intersect(layout.area)).filter(|r| r.height() >= ITEM_HEIGHT / 2.0) else { continue };
        let open = state.open_sub.is_some_and(|(s, _)| s == i);
        let mut n = node(first_id + 1 + i as u64, Some(first_id), "MenuItem", None, row);
        n.clickable = true;
        n.selected = Some(state.selected == Some(i) || open);
        match item {
            MenuItem::Separator => continue,
            MenuItem::Action(a) => {
                n.name = Some(a.label.clone());
                n.value = a.shortcut.map(|s| s.label());
                n.enabled = a.enabled;
                n.checked = a.checked.then_some(true);
            }
            MenuItem::Submenu(m) => n.name = Some(format!("{} ▸", m.title)),
        }
        nodes.push(n);
    }
    nodes
}

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

    // A menu taller than the popup (the compositor shrank it to fit the
    // screen) scrolls, as on BeOS: arrows at the ends scroll while the
    // pointer rests on them, and the wheel and keyboard scroll too.
    let geometry = PopupLayout::new(menu, rect);
    let (area, max_scroll, scrolls) = (geometry.area, geometry.max_scroll, geometry.scrolls);
    let layout = |scroll: f32| geometry.rows(scroll);
    let up = Rect::from_min_max(inner.min, pos2(inner.right(), area.top()));
    let down = Rect::from_min_max(pos2(inner.left(), area.bottom()), inner.max);
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if scrolls {
        let (wheel, dt) = ui.input(|i| {
            let wheel: f32 = i
                .events
                .iter()
                .map(|e| match e {
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => delta.y,
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. } => delta.y * ITEM_HEIGHT,
                    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Page, delta, .. } => delta.y * area.height(),
                    _ => 0.0,
                })
                .sum();
            (wheel, i.stable_dt.min(0.1))
        });
        state.scroll -= wheel;
        if let Some(pos) = pointer {
            if up.contains(pos) && state.scroll > 0.0 {
                state.scroll -= SCROLL_SPEED * dt;
                ui.ctx().request_repaint();
            } else if down.contains(pos) && state.scroll < max_scroll {
                state.scroll += SCROLL_SPEED * dt;
                ui.ctx().request_repaint();
            }
        }
    }
    state.scroll = state.scroll.clamp(0.0, max_scroll);
    let mut rows = layout(state.scroll);

    // Pointer (on the items in view only).
    let released = ui.input(|i| i.pointer.any_released());
    let hovered = pointer.filter(|pos| area.contains(*pos)).and_then(|pos| rows.iter().position(|r| r.contains(pos)));
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
    // Submenus opened from the keyboard: their row once scrolled to.
    let mut open_by_key = None;
    let before = state.selected;
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
                            open_by_key = Some(i);
                        }
                    }
                    Key::ArrowLeft | Key::Escape => result = PopupResult::Back,
                    Key::Enter | Key::Space => {
                        if let Some(i) = state.selected {
                            match &menu.items[i] {
                                MenuItem::Action(a) if a.enabled => result = PopupResult::Chosen(a.command.clone()),
                                MenuItem::Submenu(_) => open_by_key = Some(i),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    // Keep the item chosen from the keyboard in view.
    if scrolls
        && state.selected != before
        && let Some(i) = state.selected
    {
        let r = rows[i];
        if r.top() < area.top() {
            state.scroll -= area.top() - r.top() + if i == 0 { 2.0 } else { 0.0 };
        } else if r.bottom() > area.bottom() {
            state.scroll += r.bottom() - area.bottom() + if i + 1 == rows.len() { 2.0 } else { 0.0 };
        }
        state.scroll = state.scroll.clamp(0.0, max_scroll);
        rows = layout(state.scroll);
    }
    if let Some(i) = open_by_key
        && result == PopupResult::None
    {
        result = PopupResult::OpenSubmenu(i, rows[i]);
    }

    // The arrows, and the items drawn between them.
    if scrolls {
        for (band, more, pointing_up) in [(up, state.scroll > 0.0, true), (down, state.scroll < max_scroll, false)] {
            let color = if more { c(p.menu_item_text) } else { style::disabled_text(&p) };
            let (cx, cy) = (band.center().x, band.center().y);
            let tip = if pointing_up { cy - 3.0 } else { cy + 3.0 };
            let base = if pointing_up { cy + 2.0 } else { cy - 2.0 };
            painter.add(egui::Shape::convex_polygon(vec![pos2(cx - 5.0, base), pos2(cx, tip), pos2(cx + 5.0, base)], color, Stroke::NONE));
        }
    }
    let painter = painter.with_clip_rect(area);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_nodes_name_items_where_they_are() {
        let menu = Menu::new("Level").item("Everything", "all", None).separator().item("Errors Only", "errors", None).checked(true).enabled(false);
        let state = PopupState { selected: Some(0), ..Default::default() };
        let nodes = popup_nodes(&menu, &state, (170, 55), 100);
        let names: Vec<_> = nodes.iter().map(|n| (n.role.as_str(), n.name.as_deref())).collect();
        assert_eq!(names, [("Menu", Some("Level")), ("MenuItem", Some("Everything")), ("MenuItem", Some("Errors Only"))]);
        let (first, last) = (&nodes[1], &nodes[2]);
        assert_eq!((first.x, first.y, first.height, first.selected), (POPUP_BORDER, POPUP_BORDER + 2.0, ITEM_HEIGHT, Some(true)));
        assert_eq!(last.y, first.y + ITEM_HEIGHT + SEPARATOR_HEIGHT);
        assert!(last.checked == Some(true) && !last.enabled && last.parent == Some(100));
    }

    /// One frame of the bar with these events; the title "File" sits at
    /// the bar's left.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, state: &mut BarState) {
        let menus = [Menu::new("File").item("Close", "close", None), Menu::new("Edit").item("Copy", "copy", None)];
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            menu_bar(ui, &menus, state);
        });
        out.textures_delta.clear();
    }

    fn press(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// One frame of a 30-item popup squeezed into 200 points.
    fn popup_frame(ctx: &egui::Context, events: Vec<egui::Event>, state: &mut PopupState) -> PopupResult {
        let mut menu = Menu::new("Long");
        for i in 0..30 {
            menu = menu.item(format!("Item {i}"), format!("item-{i}"), None);
        }
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(160.0, 200.0))), events, ..Default::default() };
        let mut result = PopupResult::None;
        let mut out = ctx.run_ui(raw, |ui| result = popup_ui(ui, &menu, state));
        out.textures_delta.clear();
        result
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn long_menus_scroll() {
        let ctx = egui::Context::default();
        let mut state = PopupState::default();
        popup_frame(&ctx, vec![], &mut state);
        assert_eq!(state.scroll, 0.0);
        // The keyboard keeps the selection in view: the last item is
        // fully above the bottom arrow.
        popup_frame(&ctx, vec![key(Key::ArrowUp)], &mut state);
        assert_eq!(state.selected, Some(29));
        let bottom = 200.0 - POPUP_BORDER - SCROLLER_HEIGHT;
        let last_bottom = POPUP_BORDER + SCROLLER_HEIGHT + 2.0 - state.scroll + 30.0 * ITEM_HEIGHT;
        assert!(state.scroll > 0.0 && last_bottom <= bottom + 0.01, "scroll {}", state.scroll);
        // Choosing it from there.
        assert_eq!(popup_frame(&ctx, vec![key(Key::Enter)], &mut state), PopupResult::Chosen("item-29".into()));
        // The wheel scrolls back to the top, and no further.
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, source: egui::MouseWheelSource::Wheel, delta: vec2(0.0, 100.0), modifiers: Default::default(), phase: egui::TouchPhase::Move };
        popup_frame(&ctx, vec![egui::Event::PointerMoved(pos2(80.0, 100.0)), wheel], &mut state);
        assert_eq!(state.scroll, 0.0);
        // Resting on the bottom arrow scrolls down.
        for _ in 0..5 {
            popup_frame(&ctx, vec![egui::Event::PointerMoved(pos2(80.0, 195.0))], &mut state);
        }
        assert!(state.scroll > 0.0);
        // A click on an arrow chooses nothing.
        assert_eq!(popup_frame(&ctx, vec![press(pos2(80.0, 195.0), true), press(pos2(80.0, 195.0), false)], &mut state), PopupResult::None);
    }

    #[test]
    fn a_tap_opens_and_a_second_closes() {
        let ctx = egui::Context::default();
        let file = pos2(15.0, 10.0);
        let mut state = BarState::default();
        frame(&ctx, vec![egui::Event::PointerMoved(file)], &mut state);
        // A touchpad tap: press and release in the same frame.
        frame(&ctx, vec![press(file, true), press(file, false)], &mut state);
        assert_eq!(state.open.map(|o| o.0), Some(0));
        frame(&ctx, vec![press(file, true), press(file, false)], &mut state);
        assert_eq!(state.open, None);
    }

    #[test]
    fn sliding_switches_and_its_press_keeps_it() {
        let ctx = egui::Context::default();
        let file = pos2(15.0, 10.0);
        let mut state = BarState::default();
        frame(&ctx, vec![egui::Event::PointerMoved(file)], &mut state);
        frame(&ctx, vec![press(file, true), press(file, false)], &mut state);
        // Over to Edit: it opens as the pointer crosses...
        let edit = pos2(file.x + 40.0, 10.0);
        frame(&ctx, vec![egui::Event::PointerMoved(edit)], &mut state);
        assert_eq!(state.open.map(|o| o.0), Some(1));
        // ...and a click there keeps it open; the next one closes it.
        frame(&ctx, vec![press(edit, true), press(edit, false)], &mut state);
        assert_eq!(state.open.map(|o| o.0), Some(1));
        frame(&ctx, vec![press(edit, true), press(edit, false)], &mut state);
        assert_eq!(state.open, None);
    }
}
