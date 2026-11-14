//! A sidebar of sections (a Mac's source list; Haiku's preference apps
//! list theirs the same way, in a BListView down the left): rows of an
//! icon, a name and, at the right, a quiet value ("12%"), under optional
//! headings, the chosen one in the list's selection. Click a row, or use
//! ↑ and ↓ while it has the keyboard (a click gives it, or Tab); rows
//! past its height scroll. Agents find each row as a selectable
//! `ListItem` named by it, its value as the row's value, and each heading
//! as a `Heading`.
//!
//! Each row is an element of its own: the pointer coming over one draws
//! that row again and nothing else.
//!
//! Several chosen together, where the app takes it: rows chosen with the
//! chosen one ([`Sidebar::also`]) are drawn selected too; ⌘- and ⇧-clicks
//! go to [`Sidebar::on_pick`] (the app adds a row, or the run to it), ⌘A
//! while it has the keyboard to [`Sidebar::on_select_all`], and a
//! secondary click to [`Sidebar::on_menu`] (the row and where).
//!
//! It keeps its scroller and its keyboard with the window, by its id, so
//! it's built with the window to hand:
//!
//! ```ignore
//! let rows = vec![Row::heading("Monitor"), Row::new("cpu", "CPU").value("12%"), Row::new("memory", "Memory")];
//! Sidebar::new("sections", rows, chosen).on_choose(cx.msg_with(|i: &usize| Msg::Section(*i))).build(cx)
//! ```

use std::{cell::Cell, rc::Rc};

use olo_interface::{
    Bounds, Element, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, Modifiers, MouseButton, ParentElement as _, Pixels, Point, ScrollHandle, SharedString,
    Styled as _, WindowCx, agent, canvas, control_canvas, div, point, px, size,
};
use olo_kit_base::metrics as m;

use crate::{icons, review, style, theme, widgets::ScrollView};

/// A row's height.
pub const ROW: f32 = 24.0;
/// A heading's.
pub const HEADING: f32 = 22.0;

/// One row of a [`Sidebar`], or a heading over the rows after it.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Row {
    pub icon: SharedString,
    pub label: SharedString,
    /// Its value at the right, if it has one.
    pub value: Option<SharedString>,
    /// What the value shows as where the name needs the room ("●" for
    /// "Working"): agents read the value all the same.
    pub short: Option<SharedString>,
    /// A heading over the rows that follow (not chosen).
    pub heading: bool,
}

impl Row {
    pub fn new(icon: impl Into<SharedString>, label: impl Into<SharedString>) -> Row {
        Row { icon: icon.into(), label: label.into(), value: None, short: None, heading: false }
    }

    /// A heading; with no words, a gap setting what follows apart
    /// (Passwords' Archive, under its categories).
    pub fn heading(label: impl Into<SharedString>) -> Row {
        Row { icon: SharedString::default(), label: label.into(), value: None, short: None, heading: true }
    }

    /// How tall it stands.
    pub fn tall(&self) -> f32 {
        match (self.heading, self.label.is_empty()) {
            (false, _) => ROW,
            (true, false) => HEADING,
            (true, true) => 2.0 * m::TIGHT,
        }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Row {
        self.value = Some(value.into());
        self
    }

    /// The value's short form, shown instead of it when the name would
    /// otherwise be cut.
    pub fn short(mut self, short: impl Into<SharedString>) -> Row {
        self.short = Some(short.into());
        self
    }
}

/// How tall `rows` stand in a sidebar.
pub fn height(rows: &[Row]) -> f32 {
    m::TIGHT + rows.iter().map(Row::tall).sum::<f32>()
}

/// The sidebar's ground: the panel's, a little darker.
pub fn ground(p: &olo_theme::Palette) -> Hsla {
    style::shade(p, p.panel_background, 1.04)
}

/// The line down its right edge, against what it lists for.
pub fn edge(p: &olo_theme::Palette) -> Hsla {
    style::shade(p, p.panel_background, 1.25)
}

type ChooseFn = Rc<dyn Fn(&usize, &mut WindowCx)>;
type PickFn = Rc<dyn Fn(&(usize, Modifiers), &mut WindowCx)>;
type MenuFn = Rc<dyn Fn(&(usize, Point<Pixels>), &mut WindowCx)>;
type AllFn = Rc<dyn Fn(&(), &mut WindowCx)>;

/// A sidebar filling its box: see the module.
pub struct Sidebar {
    id: SharedString,
    rows: Rc<[Row]>,
    chosen: usize,
    /// Rows chosen along with `chosen`.
    also: Rc<[usize]>,
    on_choose: Option<ChooseFn>,
    on_pick: Option<PickFn>,
    on_menu: Option<MenuFn>,
    on_select_all: Option<AllFn>,
    bare: bool,
}

/// What a sidebar keeps with its window, by its id.
struct SidebarState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The row last brought into view (the chosen one moves: it's shown).
    revealed: Cell<Option<usize>>,
}

impl Sidebar {
    /// `chosen` is a row's index (never a heading's).
    pub fn new(id: impl Into<SharedString>, rows: impl Into<Rc<[Row]>>, chosen: usize) -> Sidebar {
        Sidebar { id: id.into(), rows: rows.into(), chosen, also: Rc::from([]), on_choose: None, on_pick: None, on_menu: None, on_select_all: None, bare: false }
    }

    /// Without its ground and edge: over what's behind it (a list's well:
    /// Passwords' sources).
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// A row is chosen (clicked, or ↑ ↓): its index. (Clicked while
    /// others are chosen with it, it's chosen alone: told again.)
    pub fn on_choose(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }

    /// Rows chosen along with the chosen one, drawn selected too.
    pub fn also(mut self, rows: impl Into<Rc<[usize]>>) -> Self {
        self.also = rows.into();
        self
    }

    /// A row ⌘- or ⇧-clicked: its index and the modifiers (the app adds
    /// it to what's chosen, or the run to it). Without this, such a click
    /// chooses as any does.
    pub fn on_pick(mut self, f: impl Fn(&(usize, Modifiers), &mut WindowCx) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// A row secondary-clicked: its index and where (for its menu).
    pub fn on_menu(mut self, f: impl Fn(&(usize, Point<Pixels>), &mut WindowCx) + 'static) -> Self {
        self.on_menu = Some(Rc::new(f));
        self
    }

    /// ⌘A while it has the keyboard.
    pub fn on_select_all(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.on_select_all = Some(Rc::new(f));
        self
    }
}

/// The row `step` on from `from` (↑ −1, ↓ +1), over headings.
fn stepped(rows: &[Row], from: usize, step: i32) -> Option<usize> {
    let mut i = from as i32;
    loop {
        i += step;
        if i < 0 || i >= rows.len() as i32 {
            return None;
        }
        if !rows[i as usize].heading {
            return Some(i as usize);
        }
    }
}

impl Sidebar {
    /// The sidebar, in the window `cx` is (which keeps its scroller and
    /// its keyboard, by its id).
    pub fn build(self, cx: &mut WindowCx) -> Element {
        let state = cx.keyed_state(SharedString::from(format!("olo-sidebar-{}", self.id)), |_| SidebarState { focus: FocusHandle::new(), scroll: ScrollHandle::new(), revealed: Cell::new(None) });
        let focused = state.focus.contains_focused(cx);
        let Sidebar { id, rows, chosen, also, on_choose, on_pick, on_menu, on_select_all, bare } = self;
        // The chosen row brought into view when it changes.
        if state.revealed.replace(Some(chosen)) != Some(chosen) {
            let top: f32 = m::TIGHT + rows[..chosen.min(rows.len())].iter().map(Row::tall).sum::<f32>();
            state.scroll.reveal_span(px(top - m::TIGHT), px(top + ROW));
        }
        // Up to its edge line, on device pixels.
        let line = 1.0 / cx.scale_factor();
        let several = !also.is_empty();
        let list = div().w_full().flex().flex_col().pt(px(m::TIGHT)).children(rows.iter().enumerate().map(|(i, row)| {
            let tall = row.tall();
            if row.heading && row.label.is_empty() {
                return div().h(px(tall)).flex_shrink_0();
            }
            let selected = !row.heading && (i == chosen || also.contains(&i));
            let key = style::key((row, selected, selected && focused));
            let painted = row.clone();
            let el = control_canvas(move |g, r, state| {
                let p = theme::palette();
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                let row = &painted;
                if row.heading {
                    let t = style::label(g, r.left() + px(m::WINDOW_INSET), r.center().y + px(2.0), style::Align::Left, row.label.to_uppercase(), style::SMALL_SIZE, bold, style::disabled_text(&p));
                    g.agent(agent::Node { role: "Heading", name: Some(row.label.clone()), bounds: t, enabled: true, ..Default::default() });
                    return;
                }
                if selected {
                    let sel = if focused { style::c(p.list_selected_background) } else { style::c(olo_theme::mix(p.list_selected_background, p.panel_background, 0.35)) };
                    style::fill_rect(g, r, sel);
                } else if state.hovered {
                    style::fill_rect(g, r, style::shade(&p, p.panel_background, 1.1));
                }
                let text = if selected { style::c(p.list_selected_item_text) } else { style::c(p.panel_text) };
                let icon = Bounds { origin: point(r.left() + px(m::WINDOW_INSET), r.center().y - px(8.0)), size: size(px(16.0), px(16.0)) };
                if !row.icon.is_empty() {
                    icons::paint(g, icon, &row.icon);
                }
                let width = |v: &SharedString| f32::from(style::text_width(v.clone(), style::PLAIN_SIZE, plain.clone())) + m::SPACING;
                let left = icon.right() + px(6.0);
                let room = |value_w: f32| (r.right() - px(m::WINDOW_INSET + value_w) - left).max(px(0.0));
                // The value, or its short form if the name needs the room.
                let mut shown = row.value.clone();
                if let (Some(v), Some(short)) = (&row.value, &row.short) {
                    if style::text_width(row.label.clone(), style::PLAIN_SIZE, plain.clone()) > room(width(v)) {
                        shown = Some(short.clone());
                    }
                }
                let value_w = shown.as_ref().map_or(0.0, width);
                style::label_fit(g, left, r.center().y, style::Align::Left, row.label.clone(), style::PLAIN_SIZE, plain.clone(), text, room(value_w));
                if let Some(v) = &shown {
                    let quiet = if selected { text } else { style::disabled_text(&p) };
                    style::label(g, r.right() - px(m::WINDOW_INSET), r.center().y, style::Align::Right, v.clone(), style::PLAIN_SIZE, plain, quiet);
                }
                g.agent(agent::Node {
                    role: "ListItem",
                    name: Some(row.label.clone()),
                    value: row.value.clone(),
                    bounds: r,
                    enabled: true,
                    clickable: true,
                    selected: Some(selected),
                    ..Default::default()
                });
            })
            .paint_key(key)
            .h(px(tall))
            .flex_shrink_0()
            .mr(px(line));
            if row.heading {
                return el;
            }
            let (focus, choose, pick, menu) = (state.focus.clone(), on_choose.clone(), on_pick.clone(), on_menu.clone());
            el.on_any_mouse_down(move |e, cx| {
                focus.focus(cx);
                if e.button == MouseButton::Right {
                    if let Some(f) = &menu {
                        f(&(i, e.position), cx);
                    }
                    return;
                }
                if e.button != MouseButton::Left {
                    return;
                }
                let held = e.modifiers;
                if (held.platform || held.control || held.shift)
                    && let Some(f) = &pick
                {
                    f(&(i, held), cx);
                } else if (i != chosen || several)
                    && let Some(f) = &choose
                {
                    f(&i, cx);
                }
            })
        }));
        // (Its rows go where their own heights put them.)
        let grid = canvas(|g, b| review::own_grid(g, b)).paint_key(0).absolute().inset_0();
        let keys_rows = rows.clone();
        let backdrop = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, ground(&p));
            style::vline_left_of(g, b.right(), b.top(), b.bottom(), edge(&p));
        })
        .paint_key(0)
        .absolute()
        .inset_0();
        div()
            .id(SharedString::from(format!("sidebar-{id}")))
            .track_focus(&state.focus)
            .tab_stop()
            // (Its chosen row shows whether it has the keyboard.)
            .on_focus_changed(|_, cx| cx.invalidate_owner())
            .relative()
            .size_full()
            .on_key_down(move |e: &KeyDownEvent, cx| {
                if e.keystroke.key == "a"
                    && (e.keystroke.modifiers.platform || e.keystroke.modifiers.control)
                    && let Some(f) = &on_select_all
                {
                    cx.stop_propagation();
                    f(&(), cx);
                    return;
                }
                let step = match e.keystroke.key.as_str() {
                    "up" if !e.keystroke.modifiers.modified() => -1,
                    "down" if !e.keystroke.modifiers.modified() => 1,
                    _ => return,
                };
                cx.stop_propagation();
                if let (Some(to), Some(f)) = (stepped(&keys_rows, chosen, step), &on_choose) {
                    f(&to, cx);
                }
            })
            .children((!bare).then_some(backdrop))
            .child(ScrollView::new(&state.scroll).border(false).auto_hide(true).child(div().relative().w_full().child(grid).child(list)).into_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_over_headings() {
        let rows = [Row::heading("A"), Row::new("", "one"), Row::heading("B"), Row::new("", "two")];
        assert_eq!(stepped(&rows, 1, 1), Some(3));
        assert_eq!(stepped(&rows, 3, -1), Some(1));
        assert_eq!(stepped(&rows, 1, -1), None);
        assert_eq!(height(&rows), m::TIGHT + 2.0 * HEADING + 2.0 * ROW);
    }
}
