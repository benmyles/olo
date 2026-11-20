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
//! Rows the person may put in their own order ([`Sidebar::on_reorder`]:
//! Weather's places) are dragged: a press on one, moved past a few
//! points, shows a line where it would go among the rows of its section
//! (between its headings), and the release there tells the app where
//! from and where to. Its keyboard and agents' way is the app's (a menu's
//! Move Up and Move Down).
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
/// What a heading after other rows has over it besides, so its group
/// stands apart from the one before (with a heading a row's step from
/// the row over it, two groups read as one list with a small word in it:
/// Opus's reviews of To-Do, 2026-11-19).
pub const HEADING_OVER: f32 = 6.0;

/// How tall row `i` of `rows` stands.
pub fn tall_at(rows: &[Row], i: usize) -> f32 {
    let Some(row) = rows.get(i) else { return 0.0 };
    row.tall() + if i > 0 && row.heading && !row.label.is_empty() { HEADING_OVER } else { 0.0 }
}

/// Where row `i` starts under the sidebar's top (past the last: where
/// they end).
fn top_of(rows: &[Row], i: usize) -> f32 {
    m::TIGHT + (0..i.min(rows.len())).map(|n| tall_at(rows, n)).sum::<f32>()
}

/// One row of a [`Sidebar`], or a heading over the rows after it.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Row {
    pub icon: SharedString,
    pub label: SharedString,
    /// Its value at the right, if it has one.
    pub value: Option<SharedString>,
    /// A heading over the rows that follow (not chosen).
    pub heading: bool,
    /// A dot of this colour where its icon would be (To-Do's lists,
    /// each in its own colour).
    pub dot: Option<olo_theme::Hue>,
}

impl Row {
    pub fn new(icon: impl Into<SharedString>, label: impl Into<SharedString>) -> Row {
        Row { icon: icon.into(), label: crate::words::of(label), value: None, heading: false, dot: None }
    }

    /// A heading; with no words, a gap setting what follows apart
    /// (Passwords' Archive, under its categories).
    pub fn heading(label: impl Into<SharedString>) -> Row {
        Row { icon: SharedString::default(), label: crate::words::of(label), value: None, heading: true, dot: None }
    }

    /// How tall it stands by itself ([`tall_at`]: among its rows).
    pub fn tall(&self) -> f32 {
        match (self.heading, self.label.is_empty()) {
            (false, _) => ROW,
            (true, false) => HEADING,
            (true, true) => 2.0 * m::TIGHT,
        }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Row {
        self.value = Some(crate::words::of(value));
        self
    }

    /// A dot of `hue` in its icon's place: something of the person's
    /// own, told apart by its colour (a list, a calendar).
    pub fn dot(mut self, hue: olo_theme::Hue) -> Row {
        self.dot = Some(hue);
        self
    }
}

/// A dot of `fill` in the middle of `icon` (an icon's 16 points): 12
/// points across, its edge a little darker, so a pale colour holds its
/// own on a pale ground and a dark one on a dark.
pub fn paint_dot(g: &mut olo_interface::Painter, icon: Bounds<Pixels>, fill: olo_theme::Rgba, p: &olo_theme::Palette) {
    let across = 12.0;
    let r = style::snap_centred(g, icon.center(), across, across);
    let rim = if p.mode == olo_theme::ThemeMode::Light { olo_theme::mix(fill, p.panel_text, 0.28) } else { olo_theme::mix(fill, [255, 255, 255, 255], 0.3) };
    g.paint_quad(olo_interface::fill(r, style::c(rim)).corner_radii(px(across / 2.0)));
    let line = 1.0 / g.scale_factor();
    let inner = Bounds { origin: point(r.left() + px(line), r.top() + px(line)), size: size(r.size.width - px(2.0 * line), r.size.height - px(2.0 * line)) };
    g.paint_quad(olo_interface::fill(inner, style::c(fill)).corner_radii(px(across / 2.0 - line)));
}

/// How tall `rows` stand in a sidebar.
pub fn height(rows: &[Row]) -> f32 {
    top_of(rows, rows.len())
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
type ReorderFn = Rc<dyn Fn(&(usize, usize), &mut WindowCx)>;

/// How far a press moves before it's a drag (points).
const DRAG: f32 = 4.0;

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
    on_reorder: Option<ReorderFn>,
    bare: bool,
}

/// What a sidebar keeps with its window, by its id.
struct SidebarState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The row last brought into view (the chosen one moves: it's shown).
    revealed: Cell<Option<usize>>,
    /// A press on a row (its index, where), and once it's a drag, where
    /// the row would go: the index of the row it would go before (one
    /// past its section's last: after it).
    press: Cell<Option<(usize, Point<Pixels>)>>,
    drop_at: Cell<Option<usize>>,
    /// The top of its rows as last drawn (in the window: scrolled too).
    top: Cell<Pixels>,
}

impl Sidebar {
    /// `chosen` is a row's index (never a heading's).
    pub fn new(id: impl Into<SharedString>, rows: impl Into<Rc<[Row]>>, chosen: usize) -> Sidebar {
        Sidebar { id: id.into(), rows: rows.into(), chosen, also: Rc::from([]), on_choose: None, on_pick: None, on_menu: None, on_select_all: None, on_reorder: None, bare: false }
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

    /// Rows dragged into another order, within their section: the row's
    /// index and the index it's to have (both among `rows`, headings
    /// counted), as the app should move it.
    pub fn on_reorder(mut self, f: impl Fn(&(usize, usize), &mut WindowCx) + 'static) -> Self {
        self.on_reorder = Some(Rc::new(f));
        self
    }
}

/// The rows of row `i`'s section: from after the heading before it to
/// the next heading (or the end).
fn section(rows: &[Row], i: usize) -> std::ops::Range<usize> {
    let start = rows[..i].iter().rposition(|r| r.heading).map_or(0, |h| h + 1);
    let end = rows[i..].iter().position(|r| r.heading).map_or(rows.len(), |n| i + n);
    start..end
}

/// Where row `from` would go with the pointer `y` points below the rows'
/// top: the index of the row it would go before, within its section
/// (its end: after the last).
fn drop_slot(rows: &[Row], from: usize, y: f32) -> usize {
    let s = section(rows, from);
    (s.start..s.end).find(|&i| y < top_of(rows, i) + tall_at(rows, i) / 2.0).unwrap_or(s.end)
}

/// The index row `from` is to have, dropped before `slot`.
fn moved_to(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
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
        let state = cx.keyed_state(SharedString::from(format!("olo-sidebar-{}", self.id)), |_| SidebarState {
            focus: FocusHandle::new(),
            scroll: ScrollHandle::new(),
            revealed: Cell::new(None),
            press: Cell::new(None),
            drop_at: Cell::new(None),
            top: Cell::new(px(0.0)),
        });
        let focused = state.focus.contains_focused(cx);
        let Sidebar { id, rows, chosen, also, on_choose, on_pick, on_menu, on_select_all, on_reorder, bare } = self;
        // The chosen row brought into view when it changes.
        if state.revealed.replace(Some(chosen)) != Some(chosen) {
            let top: f32 = top_of(&rows, chosen);
            state.scroll.reveal_span(px(top - m::TIGHT), px(top + ROW));
        }
        // Up to its edge line, on device pixels.
        let line = 1.0 / cx.scale_factor();
        let several = !also.is_empty();
        let list = div().w_full().flex().flex_col().pt(px(m::TIGHT)).children(rows.iter().enumerate().map(|(i, row)| {
            let tall = tall_at(&rows, i);
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
                    // (Low in its row, where it's taller: over its own rows.)
                    let t = style::label(
                        g,
                        r.left() + px(m::WINDOW_INSET),
                        r.bottom() - px(HEADING / 2.0 - 2.0),
                        style::Align::Left,
                        row.label.to_uppercase(),
                        style::SMALL_SIZE,
                        bold,
                        style::secondary_text(&p),
                    );
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
                if let Some(hue) = row.dot {
                    paint_dot(g, icon, p.hue(hue), &p);
                } else if !row.icon.is_empty() {
                    icons::paint(g, icon, &row.icon);
                }
                let value_w = row.value.as_ref().map_or(0.0, |v| f32::from(style::text_width(v.clone(), style::PLAIN_SIZE, plain.clone())) + m::SPACING);
                let left = icon.right() + px(6.0);
                let room = (r.right() - px(m::WINDOW_INSET + value_w) - left).max(px(0.0));
                style::label_fit(g, left, r.center().y, style::Align::Left, row.label.clone(), style::PLAIN_SIZE, plain.clone(), text, room);
                if let Some(v) = &row.value {
                    // (Read, not disabled: a count, a temperature.)
                    let quiet = if selected { text } else { style::secondary_text(&p) };
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
            let (kept, reorders) = (state.clone(), on_reorder.is_some());
            el.on_any_mouse_down(move |e, cx| {
                focus.focus(cx);
                // (A press that may become a drag into another order.)
                kept.drop_at.set(None);
                kept.press.set((reorders && e.button == MouseButton::Left && !e.modifiers.modified()).then_some((i, e.position)));
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
        // (Its rows go where their own heights put them; where they start,
        // kept for a drag.)
        let kept = state.clone();
        let grid = canvas(move |g, b| {
            kept.top.set(b.top());
            review::own_grid(g, b)
        })
        .absolute()
        .inset_0();
        // A drag's line, where the row would go.
        let (kept, line_rows) = (state.clone(), rows.clone());
        let drop_line = canvas(move |g, b| {
            let Some(slot) = kept.drop_at.get() else { return };
            let y = b.top() + px(top_of(&line_rows, slot));
            let p = theme::palette();
            let thick = 2.0;
            let r = style::snap_centred(g, point(b.center().x, y), f32::from(b.size.width) - 2.0 * m::TIGHT, thick);
            g.paint_quad(olo_interface::fill(r, style::c(p.control_highlight)).corner_radii(px(thick / 2.0)));
        })
        .paint_key(style::key(state.drop_at.get()))
        .absolute()
        .inset_0();
        let keys_rows = rows.clone();
        let backdrop = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, ground(&p));
            style::vline_left_of(g, b.right(), b.top(), b.bottom(), edge(&p));
        })
        .paint_key(0)
        .absolute()
        .inset_0();
        // A press on a row moved past `DRAG`: the row's new place followed
        // (anywhere in the window), and taken on release.
        let (kept, drag_rows) = (state.clone(), rows.clone());
        let on_drag = move |e: &olo_interface::MouseMoveEvent, cx: &mut WindowCx| {
            let Some((from, at)) = kept.press.get() else { return };
            if e.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let d = e.position - at;
            if kept.drop_at.get().is_none() && (f32::from(d.x).powi(2) + f32::from(d.y).powi(2)).sqrt() < DRAG {
                return;
            }
            let slot = drop_slot(&drag_rows, from, f32::from(e.position.y - kept.top.get()));
            if kept.drop_at.replace(Some(slot)) != Some(slot) {
                cx.invalidate_owner();
            }
        };
        let (kept, done) = (state.clone(), on_reorder.clone());
        let on_release = move |_: &olo_interface::MouseUpEvent, cx: &mut WindowCx| {
            let (press, slot) = (kept.press.take(), kept.drop_at.take());
            if let (Some((from, _)), Some(slot)) = (press, slot) {
                cx.invalidate_owner();
                let to = moved_to(from, slot);
                if to != from
                    && let Some(f) = &done
                {
                    f(&(from, to), cx);
                }
            }
        };
        let on_release_out = on_release.clone();
        let reorders = on_reorder.is_some();
        let mut sidebar = div().id(SharedString::from(format!("sidebar-{id}"))).track_focus(&state.focus);
        if reorders {
            sidebar = sidebar.on_drag_move(on_drag).on_mouse_up(MouseButton::Left, on_release).on_mouse_up_out(MouseButton::Left, on_release_out);
        }
        sidebar
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
            .child(ScrollView::new(&state.scroll).border(false).auto_hide(true).child(div().relative().w_full().child(grid).child(list).children(reorders.then_some(drop_line))).into_element())
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
        // (The second heading has air over it; the first none.)
        assert_eq!(height(&rows), m::TIGHT + 2.0 * HEADING + HEADING_OVER + 2.0 * ROW);
        assert_eq!((tall_at(&rows, 0), tall_at(&rows, 2), tall_at(&rows, 1)), (HEADING, HEADING + HEADING_OVER, ROW));
    }

    /// A row dragged goes among its section's rows: before the one whose
    /// middle the pointer is above, after the last past them all, never
    /// into another section.
    #[test]
    fn a_dragged_row_stays_in_its_section() {
        let rows = [Row::heading("Places"), Row::new("", "a"), Row::new("", "b"), Row::new("", "c"), Row::heading("Other"), Row::new("", "x")];
        assert_eq!(section(&rows, 2), 1..4);
        assert_eq!(section(&rows, 5), 5..6);
        // Rows' tops: a at TIGHT + HEADING, then each ROW on.
        let a = m::TIGHT + HEADING;
        assert_eq!(drop_slot(&rows, 3, a + 2.0), 1, "over a's top half: before a");
        assert_eq!(drop_slot(&rows, 1, a + ROW + ROW * 0.6), 3, "past b's middle: before c");
        assert_eq!(drop_slot(&rows, 1, 1000.0), 4, "far below: after c, not into Other");
        assert_eq!(drop_slot(&rows, 2, -50.0), 1, "far above: first");
        assert_eq!((moved_to(1, 3), moved_to(3, 1), moved_to(2, 2), moved_to(2, 3)), (2, 1, 2, 2));
    }
}
