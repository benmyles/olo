//! A tool bar: the window's commands as icons, along its top or down its
//! side (Gobe Productive's tool and format bars; Haiku's BToolBar). Every
//! tool is one of the window's menu commands: what it says (its tip, what
//! agents read), whether it can be used and whether it's on come from
//! the menus the window has now, so the bar and the menus never
//! disagree, and the keyboard reaches each through the menus and the
//! command palette (the bar's tools aren't tab stops, as a Mac's and
//! Haiku's aren't: Tab is the document's).
//!
//! A tool is a button (`command`), one that stays down while its command
//! is checked (`toggle`: Bold, Align Left), a picker (`picker`: a pop-up
//! of choices showing the one chosen, a font or a size; `colors`, a
//! swatch), a separator between groups, or a space that takes what's
//! left. Flat until the pointer is over a tool, as Haiku's: a bar of
//! twenty bevelled buttons is noise. A bar too narrow for its tools
//! wraps at its separators (up to `rows`), and what still doesn't fit is
//! in a menu at its end (»), so nothing is ever cut off.

use std::{collections::HashMap, rc::Rc};

use olo_interface::{ContentMask, Corner, Element, FontWeight, Hsla, IntoElement, ParentElement as _, PathBuilder, SharedString, Styled as _, WindowCx, agent, canvas, div, point, px};
use olo_kit_base::metrics as m;

use super::{control::control, menu_field::paint_field};
use crate::{
    Menu, MenuItem, icons,
    menu::{self, Action},
    style, theme,
};

/// A tool's square (and a picker's height). A bar has `TIGHT` round its
/// rows and between them, so one row is `metrics::TOOLBAR_HEIGHT` tall, as
/// a toolbar of buttons is, and a side bar that wide.
pub const TOOL: f32 = m::CONTROL_HEIGHT;
/// Between tools in a group: close, as a tray's icons are (flat, their
/// icons are well apart).
const GAP: f32 = 2.0;
/// A separator: a groove with room either side.
const SEPARATOR: f32 = 2.0 * m::TIGHT + 2.0;
/// The menu of what doesn't fit (»).
const MORE: f32 = 16.0;
const ICON: f32 = 16.0;

type OnCommand = Rc<dyn Fn(&str, &mut WindowCx)>;
/// A bar's placing, for the length, tool count and rows it was made for.
type Laid = std::cell::RefCell<Option<((u32, usize, usize), Rc<Placed>)>>;
/// A picker's menu, made as it opens (a Font menu of every family isn't
/// made for each frame).
type MakeMenu = Rc<dyn Fn(&mut WindowCx) -> Menu>;

#[derive(Clone)]
enum Tool {
    Command {
        command: SharedString,
        icon: SharedString,
        toggle: bool,
    },
    /// Showing `value`; a colour picker its swatch too (`Some`: the
    /// colour, if it's one of the menu's).
    Picker {
        name: SharedString,
        value: SharedString,
        swatch: Option<Option<Hsla>>,
        width: f32,
        enabled: bool,
        menu: MakeMenu,
    },
    Separator,
    Space,
}

impl Tool {
    /// Along the bar.
    fn length(&self) -> f32 {
        match self {
            Tool::Command { .. } => TOOL,
            Tool::Picker { width, .. } => *width,
            Tool::Separator => SEPARATOR,
            Tool::Space => 0.0,
        }
    }
}

#[derive(Clone)]
pub struct ToolBar {
    id: SharedString,
    name: SharedString,
    tools: Vec<Tool>,
    /// The window's menus' commands by name (what each tool is now).
    actions: HashMap<String, Action>,
    vertical: bool,
    /// How long it can be: its width (its height down a side). The
    /// window's, unless said.
    length: Option<f32>,
    rows: usize,
    on_command: OnCommand,
    /// Where its tools went at a length (laid out once for its extent,
    /// its key and its drawing).
    placed: Laid,
}

/// Where a bar's tools go: each row's tools (their indexes), and those
/// that don't fit, in the » menu at the last row's end.
#[derive(Debug, PartialEq)]
struct Placed {
    rows: Vec<Vec<usize>>,
    more: Vec<usize>,
}

impl ToolBar {
    /// A bar named `name` (what agents read it as: "Tools", "Format"),
    /// its tools' commands chosen through `on_command`, as the menu
    /// bar's are. Its tools follow, then [`Self::menus`].
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>, on_command: impl Fn(&str, &mut WindowCx) + 'static) -> ToolBar {
        ToolBar { id: id.into(), name: name.into(), tools: Vec::new(), actions: HashMap::new(), vertical: false, length: None, rows: 1, on_command: Rc::new(on_command), placed: Default::default() }
    }

    /// What its tools are now: the window's menus (the ones its menu bar
    /// shows), after its tools. Only its own commands are kept, with the
    /// keys the menu bar shows (a Font menu has hundreds of items).
    pub fn menus(mut self, menus: &[Menu]) -> Self {
        fn walk(items: &[MenuItem], wanted: &std::collections::HashSet<&str>, out: &mut Vec<Action>) {
            for item in items {
                match item {
                    MenuItem::Action(a) if wanted.contains(&a.command.as_str()) && !out.iter().any(|o| o.command == a.command) => out.push(a.clone()),
                    MenuItem::Submenu(m) => walk(&m.items, wanted, out),
                    _ => {}
                }
            }
        }
        let wanted: std::collections::HashSet<&str> = self.tools.iter().filter_map(|t| if let Tool::Command { command, .. } = t { Some(command.as_ref()) } else { None }).collect();
        let mut found = Vec::new();
        for m in menus {
            walk(&m.items, &wanted, &mut found);
        }
        let mut own = [Menu { title: String::new(), items: found.into_iter().map(MenuItem::Action).collect(), icon: None }];
        menu::resolve(&mut own, false);
        let [own] = own;
        self.actions = own.items.into_iter().filter_map(|i| if let MenuItem::Action(a) = i { Some((a.command.clone(), a)) } else { None }).collect();
        self
    }

    /// Down the window's side, not along its top.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// How long it can be (its width; its height down a side), when it
    /// isn't the window's.
    pub fn length(mut self, l: f32) -> Self {
        self.length = Some(l);
        self
    }

    /// Wraps at its separators to as many as `n` rows when it's too
    /// narrow for one.
    pub fn rows(mut self, n: usize) -> Self {
        self.rows = n.max(1);
        self
    }

    /// The menu command `command`, as icon `icon`.
    pub fn command(mut self, command: impl Into<SharedString>, icon: impl Into<SharedString>) -> Self {
        self.tools.push(Tool::Command { command: command.into(), icon: icon.into(), toggle: false });
        self
    }

    /// A command that stays down while it's checked in its menu.
    pub fn toggle(mut self, command: impl Into<SharedString>, icon: impl Into<SharedString>) -> Self {
        self.tools.push(Tool::Command { command: command.into(), icon: icon.into(), toggle: true });
        self
    }

    /// A pop-up of choices, `width` wide, showing `value` (the one
    /// chosen), its menu made by `menu` as it opens (one the window's
    /// menus have too: Font ▸ Size; the choice checked in it).
    pub fn picker(mut self, name: impl Into<SharedString>, value: impl Into<SharedString>, width: f32, menu: impl Fn(&mut WindowCx) -> Menu + 'static) -> Self {
        self.tools.push(Tool::Picker { name: name.into(), value: value.into(), swatch: None, width, enabled: true, menu: Rc::new(menu) });
        self
    }

    /// [`Self::picker`] of colours (a menu with `Menu::swatch`es), showing
    /// `swatch` (None: a colour none of its choices is) and saying `value`.
    pub fn colors(mut self, name: impl Into<SharedString>, value: impl Into<SharedString>, swatch: Option<Hsla>, menu: impl Fn(&mut WindowCx) -> Menu + 'static) -> Self {
        self.tools.push(Tool::Picker { name: name.into(), value: value.into(), swatch: Some(swatch), width: TOOL + 18.0, enabled: true, menu: Rc::new(menu) });
        self
    }

    /// The last picker can be used or not (a command's tool follows its
    /// menu item).
    pub fn enabled(mut self, on: bool) -> Self {
        if let Some(Tool::Picker { enabled, .. }) = self.tools.last_mut() {
            *enabled = on;
        }
        self
    }

    pub fn separator(mut self) -> Self {
        self.tools.push(Tool::Separator);
        self
    }

    /// Room that takes what's left of the row (what follows goes to its
    /// end).
    pub fn space(mut self) -> Self {
        self.tools.push(Tool::Space);
        self
    }

    /// How long it can be: as said, else the window's (the view that
    /// holds it says what it shows again when the window is sized).
    fn length_now(&self) -> f32 {
        self.length.unwrap_or_else(|| {
            let view = olo_interface::viewport_size();
            f32::from(if self.vertical { view.height } else { view.width })
        })
    }

    /// How many rows it takes at `length`.
    pub fn rows_at(&self, length: f32) -> usize {
        self.place(length).rows.len().max(1)
    }

    /// How tall it is (wide, down a side) at `length`: its rows, the room
    /// round them, and the line between it and what it's beside, on
    /// whole device pixels (so what follows starts on one, as after the
    /// ruler: 66.8 points are 83.5 pixels at 1.25×).
    pub fn extent(&self, length: f32) -> f32 {
        let s = olo_interface::scale_factor();
        let lines = if self.vertical { 1 } else { self.rows_at(length) } as f32;
        let line = style::line_pixels(s) / s;
        ((lines * TOOL + (lines + 1.0) * m::TIGHT + line) * s).round() / s
    }

    /// [`Self::lay_out`], once for each length.
    fn place(&self, length: f32) -> Rc<Placed> {
        // (Tools are only added, and their lengths fixed as they are.)
        let at = (length.to_bits(), self.tools.len(), self.rows);
        let mut memo = self.placed.borrow_mut();
        match &*memo {
            Some((was, placed)) if *was == at => placed.clone(),
            _ => {
                let placed = Rc::new(self.lay_out(length));
                *memo = Some((at, placed.clone()));
                placed
            }
        }
    }

    /// Its tools in rows at `length`: groups (between separators) kept
    /// whole on a row while one fits, each row's separator at its end
    /// dropped, and what doesn't fit in the rows allowed in the » menu.
    fn lay_out(&self, length: f32) -> Placed {
        let room = length - 2.0 * m::TIGHT;
        let mut groups: Vec<Vec<usize>> = vec![Vec::new()];
        for (i, t) in self.tools.iter().enumerate() {
            if matches!(t, Tool::Separator) {
                groups.push(vec![i]);
            } else {
                groups.last_mut().unwrap().push(i);
            }
        }
        let width = |tools: &[usize]| -> f32 {
            let real: Vec<f32> = tools.iter().map(|&i| self.tools[i].length()).collect();
            real.iter().sum::<f32>() + GAP * real.len().saturating_sub(1) as f32
        };
        let separator = |i: &usize| matches!(self.tools[*i], Tool::Separator);
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut more = Vec::new();
        for g in groups {
            if !more.is_empty() {
                more.extend(g);
                continue;
            }
            // (A row starts with the group, not its separator.)
            let lead = |g: Vec<usize>| -> Vec<usize> { g.into_iter().skip_while(separator).collect() };
            let mut g = if rows.last().unwrap().is_empty() { lead(g) } else { g };
            let row = rows.last().unwrap();
            if width(&[row.as_slice(), &g].concat()) <= room {
                rows.last_mut().unwrap().extend(g);
                continue;
            }
            if !row.is_empty() && rows.len() < self.rows {
                g = lead(g);
                rows.push(Vec::new());
                if width(&g) <= room {
                    rows.last_mut().unwrap().extend(g);
                    continue;
                }
            }
            // As much of it as fits; the rest, and all after, in the menu.
            for i in g {
                let row = rows.last_mut().unwrap();
                if more.is_empty() && width(&[row.as_slice(), &[i]].concat()) <= room {
                    row.push(i);
                } else {
                    more.push(i);
                }
            }
        }
        if !more.is_empty() {
            // Room for » in the last row: tools from its end go too.
            let last = rows.last_mut().unwrap();
            while !last.is_empty() && width(last) + GAP + MORE > room {
                more.insert(0, last.pop().unwrap());
            }
            while last.last().is_some_and(|&i| matches!(self.tools[i], Tool::Separator)) {
                last.pop();
            }
        }
        // (Its groups apart in the menu too.)
        more.retain(|&i| !matches!(self.tools[i], Tool::Space));
        rows.retain(|r| !r.is_empty());
        Placed { rows, more }
    }

    /// The » menu: the tools that didn't fit, as their menus have them
    /// (a picker's as a submenu), made as it opens.
    fn more_menu(&self, more: &[usize]) -> MakeMenu {
        let tools: Vec<(Tool, Option<Action>)> =
            more.iter().map(|&i| (self.tools[i].clone(), if let Tool::Command { command, .. } = &self.tools[i] { self.actions.get(command.as_ref()).cloned() } else { None })).collect();
        Rc::new(move |cx: &mut WindowCx| {
            let mut menu = Menu::new("More");
            for (tool, action) in &tools {
                match tool {
                    Tool::Command { icon, .. } => {
                        if let Some(a) = action {
                            let mut a = a.clone();
                            a.icon = Some(icon.to_string());
                            menu.items.push(MenuItem::Action(a));
                        }
                    }
                    Tool::Picker { name, enabled, menu: make, .. } => {
                        let mut m = make(cx);
                        m.title = name.to_string();
                        if !enabled {
                            disable(&mut m);
                        }
                        menu = menu.submenu(m);
                    }
                    Tool::Separator => menu = menu.separator(),
                    Tool::Space => {}
                }
            }
            menu
        })
    }
}

impl IntoElement for ToolBar {
    fn into_element(self) -> Element {
        let length = self.length_now();
        let placed = self.place(length);
        let vertical = self.vertical;
        let line_name = self.name.clone();
        let ground = canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.panel_background));
            let line = style::shade(&p, p.panel_background, 1.3);
            if vertical {
                style::vline_left_of(g, b.right(), b.top(), b.bottom(), line);
            } else {
                style::hline_above(g, b.left(), b.right(), b.bottom(), line);
            }
            g.agent(agent::Node { role: "Toolbar", name: Some(line_name.clone()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(style::key((&self.name, vertical)))
        .absolute()
        .inset_0();
        let more = (!placed.more.is_empty()).then(|| self.more_menu(&placed.more));
        let mut lines = Vec::new();
        let last = placed.rows.len().saturating_sub(1);
        for (k, row) in placed.rows.iter().enumerate() {
            let mut line = if vertical { div().flex().flex_col().items_center().w(px(TOOL)).flex_1() } else { div().flex().items_center().h(px(TOOL)) };
            // (Spaced by the bar, not the window's room between controls.)
            let spaced = canvas(|g, b| crate::review::own_grid(g, b)).paint_key(0).absolute().inset_0();
            line = line.relative().gap(px(GAP)).child(spaced);
            for &i in row {
                line = line.child(self.tool(i, vertical));
            }
            if k == last
                && let Some(menu) = more.clone()
            {
                line = line.child(div().flex_1()).child(more_button(&self.id, menu, vertical, self.on_command.clone()));
            }
            lines.push(line);
        }
        let extent = px(self.extent(length));
        let bar = if vertical {
            div().flex().flex_col().h_full().w(extent).py(px(m::TIGHT)).pl(px(m::TIGHT))
        } else {
            div().flex().flex_col().w_full().h(extent).px(px(m::TIGHT)).pt(px(m::TIGHT)).gap(px(m::TIGHT))
        };
        bar.relative().flex_shrink_0().child(ground).children(lines)
    }
}

impl ToolBar {
    fn tool(&self, i: usize, vertical: bool) -> Element {
        let id = format!("{}-{i}", self.id);
        match &self.tools[i] {
            Tool::Command { command, icon, toggle } => {
                let Some(a) = self.actions.get(command.as_ref()) else {
                    debug_assert!(false, "tool bar {}: no menu has command {command}", self.id);
                    return div();
                };
                let label = a.label.trim_end_matches('…').to_owned();
                let tip = match &a.keys {
                    Some(keys) => format!("{label} ({keys})"),
                    None => label.clone(),
                };
                let on = *toggle && a.checked;
                let (icon, command, f) = (icon.clone(), command.clone(), self.on_command.clone());
                let look = (icon.clone(), on);
                control(SharedString::from(id), TOOL, TOOL, move |g, r, s| {
                    let p = theme::palette();
                    let bg = p.panel_background;
                    let down = s.pressed || on;
                    if down {
                        // Sunken: held, or a toggle that's on.
                        style::fill_rect(g, r, style::shade(&p, bg, if s.pressed { 1.16 } else { 1.1 }));
                        style::frame_rect(g, r, style::shade(&p, bg, 1.45));
                        let inner = style::inset(g, r, 1.0);
                        style::bevel(g, inner, style::shade(&p, bg, 1.25), style::shade(&p, bg, 1.04));
                    } else if s.hovered {
                        let base = p.control_background;
                        let inner = style::inset(g, r, 1.0);
                        style::vgradient(g, inner, style::tint(base, 0.8), style::shade(&p, base, 1.04));
                        style::frame_rect(g, r, style::shade(&p, bg, 1.45));
                        style::bevel(g, inner, style::c(p.shine), style::shade(&p, base, 1.15));
                    }
                    // Held, the icon gives a device pixel.
                    let off = if s.pressed { style::hairline(g) } else { px(0.0) };
                    let at = style::snap_centred(g, point(r.center().x + off, r.center().y + off), ICON, ICON);
                    icons::paint(g, at, &icon);
                })
                .look(look)
                .enabled(a.enabled)
                .agent(if *toggle { "ToggleButton" } else { "Button" }, label, (*toggle).then_some(on))
                .tip(tip)
                .on_click(move |_, cx| f(&command, cx))
                .into_element()
            }
            Tool::Picker { name, value, swatch, width, enabled, menu } => {
                let (swatch, enabled) = (*swatch, *enabled);
                let (menu, f) = (menu.clone(), self.on_command.clone());
                let shown = value.clone();
                let look = (value.clone(), swatch.map(style::ink_key));
                control(SharedString::from(id), *width, TOOL, move |g, r, s| {
                    let p = theme::palette();
                    let ink = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
                    let inside = paint_field(g, r, ink, &p);
                    if let Some(color) = swatch {
                        let at = style::snap_centred(g, point(inside.left() + px(m::TEXT_INSET + 7.0), r.center().y), 14.0, 14.0);
                        match color {
                            Some(color) => style::paint_swatch(g, at, color, &p),
                            None => style::sunken(g, at, &p, false),
                        }
                    } else {
                        let room = inside.right() - (inside.left() + px(m::TEXT_INSET));
                        g.with_content_mask(Some(ContentMask { bounds: inside }), |g| {
                            style::label_fit(g, inside.left() + px(m::TEXT_INSET), r.center().y, style::Align::Left, shown.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink, room)
                        });
                    }
                })
                .look(look)
                .enabled(enabled)
                .agent_value("ComboBox", name.clone(), value.clone())
                .tip(name.clone())
                .on_click(move |_, cx| {
                    let f = f.clone();
                    let choose = move |cmd: &str, cx: &mut WindowCx| f(cmd, cx);
                    let at = cx.listener_bounds();
                    let menu = menu(cx);
                    // The choice over the field, as a menu field's; below
                    // it when the choice is in a submenu.
                    match menu.items.iter().position(|item| matches!(item, MenuItem::Action(a) if a.checked)) {
                        Some(current) => menu::popup_over(cx, menu, at, current, choose),
                        None => menu::popup(cx, menu, at, Corner::BottomLeft, choose),
                    };
                })
                .into_element()
            }
            Tool::Separator => {
                let groove = canvas(move |g, b| {
                    let p = theme::palette();
                    let dark = style::shade(&p, p.panel_background, 1.3);
                    let c = style::snap_centred(g, b.center(), 2.0, 2.0);
                    if vertical {
                        let (x0, x1) = (b.left() + px(3.0), b.right() - px(3.0));
                        style::hline(g, x0, x1, c.top(), dark);
                        style::hline(g, x0, x1, c.top() + style::hairline(g), style::c(p.shine));
                    } else {
                        let (y0, y1) = (b.top() + px(3.0), b.bottom() - px(3.0));
                        style::vline(g, c.left(), y0, y1, dark);
                        style::vline(g, c.left() + style::hairline(g), y0, y1, style::c(p.shine));
                    }
                })
                .paint_key(vertical as u64);
                (if vertical { groove.w(px(TOOL)).h(px(SEPARATOR)) } else { groove.w(px(SEPARATOR)).h(px(TOOL)) }).flex_shrink_0()
            }
            Tool::Space => div().flex_1(),
        }
    }
}

/// Every one of `menu`'s commands can't be chosen (a picker that can't
/// be used, in the » menu).
fn disable(menu: &mut Menu) {
    for item in &mut menu.items {
        match item {
            MenuItem::Action(a) => a.enabled = false,
            MenuItem::Submenu(m) => disable(m),
            MenuItem::Separator => {}
        }
    }
}

/// The » at a bar's end: what didn't fit, as a menu.
fn more_button(id: &str, menu: MakeMenu, vertical: bool, f: OnCommand) -> Element {
    let (w, h) = if vertical { (TOOL, MORE) } else { (MORE, TOOL) };
    control(SharedString::from(format!("{id}-more")), w, h, move |g, r, s| {
        let p = theme::palette();
        let ink = style::c(p.panel_text);
        if s.hovered || s.pressed {
            style::fill_rect(g, r, style::shade(&p, p.panel_background, if s.pressed { 1.16 } else { 1.06 }));
        }
        // Two chevrons pointing on along the bar.
        let c = style::snap_centred(g, r.center(), 8.0, 8.0);
        for k in 0..2 {
            let d = px(4.0 * k as f32 - 2.0);
            let mut path = PathBuilder::stroke(style::hairline(g) * 1.5);
            if vertical {
                path.move_to(point(c.left(), c.center().y + d - px(2.0)));
                path.line_to(point(c.center().x, c.center().y + d + px(1.0)));
                path.line_to(point(c.right(), c.center().y + d - px(2.0)));
            } else {
                path.move_to(point(c.center().x + d - px(2.0), c.top()));
                path.line_to(point(c.center().x + d + px(1.0), c.center().y));
                path.line_to(point(c.center().x + d - px(2.0), c.bottom()));
            }
            if let Ok(path) = path.build() {
                g.paint_path(path, ink);
            }
        }
    })
    .look(vertical)
    .agent("Button", "More Tools", None)
    .on_click(move |_, cx| {
        let f = f.clone();
        let at = cx.listener_bounds();
        let menu = menu(cx);
        menu::popup(cx, menu, at, if vertical { Corner::TopRight } else { Corner::BottomLeft }, move |cmd: &str, cx: &mut WindowCx| f(cmd, cx));
    })
    .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(tools: &[&str]) -> ToolBar {
        let mut menu = Menu::new("M");
        for t in tools {
            if !t.starts_with('|') {
                menu = menu.item(*t, *t);
            }
        }
        let mut b = ToolBar::new("t", "Tools", |_, _| {});
        for t in tools {
            b = if *t == "|" { b.separator() } else { b.command(t.to_string(), "document") };
        }
        b.menus(&[menu])
    }

    #[test]
    fn rows_wrap_at_separators_and_the_rest_goes_in_more() {
        // Two groups of three: 3 × 24 + 2 × 2 = 76 each, 14 between.
        let b = bar(&["a", "b", "c", "|", "d", "e", "f"]);
        let all = 2.0 * m::TIGHT + 76.0 + SEPARATOR + 2.0 * GAP + 76.0;
        assert_eq!(*b.place(all), Placed { rows: vec![vec![0, 1, 2, 3, 4, 5, 6]], more: vec![] });
        // Narrower: the second group on a row of its own, without its
        // separator, when two rows are allowed.
        let b = b.rows(2);
        assert_eq!(*b.place(all - 1.0), Placed { rows: vec![vec![0, 1, 2], vec![4, 5, 6]], more: vec![] });
        // One row: as much as fits, the rest in the menu, with room for ».
        let b = bar(&["a", "b", "c", "|", "d", "e", "f"]);
        assert_eq!(*b.place(all - 1.0), Placed { rows: vec![vec![0, 1, 2, 3, 4, 5]], more: vec![6] });
        assert_eq!(*b.place(all - 20.0), Placed { rows: vec![vec![0, 1, 2, 3, 4]], more: vec![5, 6] });
        // A group's separator goes to the menu with it.
        assert_eq!(b.place(2.0 * m::TIGHT + 76.0 + GAP + MORE).more, vec![3, 4, 5, 6]);
        // Too narrow for even a group: what fits, the rest in the menu.
        let p = b.place(2.0 * m::TIGHT + 24.0 + GAP + MORE);
        assert_eq!(*p, Placed { rows: vec![vec![0]], more: vec![1, 2, 3, 4, 5, 6] });
    }

    #[test]
    fn a_tool_is_what_its_menu_says() {
        let menus = [Menu::new("Font").item("Bold", "bold").key("b").checked(true).item("Open…", "open").enabled(false)];
        let b = ToolBar::new("t", "Tools", |_, _| {}).toggle("bold", "format-bold").command("open", "folder-open").menus(&menus);
        let bold = &b.actions["bold"];
        assert!(bold.checked && bold.enabled);
        assert!(bold.keys.is_some(), "its keys, as the menu bar shows them");
        assert!(!b.actions["open"].enabled);
        assert_eq!(b.actions.len(), 2, "its own commands, and only those");
    }
}
