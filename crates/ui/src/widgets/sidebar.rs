//! A sidebar of sections (a Mac's source list; Haiku's preference apps
//! list theirs the same way, in a BListView down the left): rows of an icon, a name and, at the right, a quiet value
//! ("12%"), under optional headings, the chosen one in the list's
//! selection. Click a row, or use ↑ and ↓ while it has the keyboard (a
//! click gives it, or Tab); rows past its height scroll. Agents find each
//! row as a selectable `ListItem` named by it, its value as the row's
//! value, and each heading as a `Heading`.
//!
//! ```ignore
//! let rows = vec![Row::heading("Monitor"), Row::new("cpu", "CPU").value("12%"), Row::new("memory", "Memory")];
//! Sidebar::new("sections", rows, chosen).on_choose(move |i, _, cx| …)
//! ```

use std::rc::Rc;

use gpui::{
    App, Bounds, Entity, FocusHandle, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, RenderOnce, ScrollHandle, SharedString, Styled as _, Window, canvas, div, point,
    px, size,
};
use olo_kit_base::metrics as m;

use crate::{agent, icons, review, style, theme, widgets::ScrollView};

/// A row's height.
pub const ROW: f32 = 24.0;
/// A heading's.
pub const HEADING: f32 = 22.0;

/// One row of a [`Sidebar`], or a heading over the rows after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub icon: SharedString,
    pub label: SharedString,
    /// Its value at the right, if it has one.
    pub value: Option<SharedString>,
    /// A heading over the rows that follow (not chosen).
    pub heading: bool,
}

impl Row {
    pub fn new(icon: impl Into<SharedString>, label: impl Into<SharedString>) -> Row {
        Row { icon: icon.into(), label: label.into(), value: None, heading: false }
    }

    /// A heading; with no words, a gap setting what follows apart
    /// (Passwords' Archive, under its categories).
    pub fn heading(label: impl Into<SharedString>) -> Row {
        Row { icon: SharedString::default(), label: label.into(), value: None, heading: true }
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
}

/// How tall `rows` stand in a sidebar.
pub fn height(rows: &[Row]) -> f32 {
    m::TIGHT + rows.iter().map(Row::tall).sum::<f32>()
}

/// The sidebar's ground: the panel's, a little darker.
pub fn ground(p: &olo_theme::Palette) -> gpui::Hsla {
    style::shade(p, p.panel_background, 1.04)
}

/// The line down its right edge, against what it lists for.
pub fn edge(p: &olo_theme::Palette) -> gpui::Hsla {
    style::shade(p, p.panel_background, 1.25)
}

type ChooseFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A sidebar filling its box: see the module.
#[derive(IntoElement)]
pub struct Sidebar {
    id: SharedString,
    rows: Rc<[Row]>,
    chosen: usize,
    on_choose: Option<ChooseFn>,
    bare: bool,
}

struct SidebarState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The row under the pointer, as last painted.
    hover: Option<usize>,
    /// The row last brought into view (the chosen one moves: it's shown).
    revealed: Option<usize>,
}

impl Sidebar {
    /// `chosen` is a row's index (never a heading's).
    pub fn new(id: impl Into<SharedString>, rows: impl Into<Rc<[Row]>>, chosen: usize) -> Sidebar {
        Sidebar { id: id.into(), rows: rows.into(), chosen, on_choose: None, bare: false }
    }

    /// Without its ground and edge: over what's behind it (a list's well:
    /// Passwords' sources).
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// A row is chosen (clicked, or ↑ ↓): its index.
    pub fn on_choose(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
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

impl RenderOnce for Sidebar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state: Entity<SidebarState> = window.use_keyed_state(SharedString::from(format!("olo-sidebar-{}", self.id)), cx, |_, cx| SidebarState {
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            hover: None,
            revealed: None,
        });
        let (focus, scroll) = {
            let s = state.read(cx);
            (s.focus.clone(), s.scroll.clone())
        };
        let Sidebar { id, rows, chosen, on_choose, bare } = self;
        let total = height(&rows);
        // The chosen row brought into view when it changes.
        if state.read(cx).revealed != Some(chosen) {
            let top: f32 = m::TIGHT + rows[..chosen.min(rows.len())].iter().map(Row::tall).sum::<f32>();
            let (view, offset) = (f32::from(scroll.bounds().size.height), -f32::from(scroll.offset().y));
            if view > 0.0 {
                let y = if top < offset { Some(top - m::TIGHT) } else if top + ROW > offset + view { Some(top + ROW - view) } else { None };
                if let Some(y) = y {
                    scroll.set_offset(point(px(0.0), px(-y.max(0.0))));
                }
            }
            state.update(cx, |s, _| s.revealed = Some(chosen));
        }
        let paint_state = state.clone();
        let (hit_rows, paint_rows) = (rows.clone(), rows.clone());
        let choose = on_choose.clone();
        let list = canvas(
            move |b, window, _| {
                let mut y = b.top() + px(m::TIGHT);
                hit_rows
                    .iter()
                    .map(|row| {
                        let h = px(row.tall());
                        let r = Bounds { origin: point(b.left(), y), size: size(b.size.width, h) };
                        y += h;
                        (!row.heading).then(|| window.insert_hitbox(r, HitboxBehavior::Normal))
                    })
                    .collect::<Vec<_>>()
            },
            move |b, hitboxes, window, cx| {
                let p = theme::palette(cx);
                review::own_grid(window, b);
                let focused = paint_state.read(cx).focus.contains_focused(window, cx);
                let hover = hitboxes.iter().position(|h| h.as_ref().is_some_and(|h| h.is_hovered(window)));
                if paint_state.read(cx).hover != hover {
                    paint_state.update(cx, |s, _| s.hover = hover);
                }
                let s = window.scale_factor();
                let snap = |v: gpui::Pixels| px((f32::from(v) * s).round() / s);
                // Up to its edge line, on device pixels.
                let right = snap(b.right()) - px(1.0 / s);
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                let mut y = b.top() + px(m::TIGHT);
                for (i, row) in paint_rows.iter().enumerate() {
                    let h = row.tall();
                    let r = Bounds::from_corners(point(b.left(), snap(y)), point(right, snap(y) + px(h)));
                    y += px(h);
                    if row.heading && row.label.is_empty() {
                        continue;
                    }
                    if row.heading {
                        let t = style::label(window, cx, r.left() + px(m::WINDOW_INSET), r.center().y + px(2.0), style::Align::Left, row.label.to_uppercase(), style::SMALL_SIZE, bold.clone(), style::disabled_text(&p));
                        agent::record(window, cx, agent::Node { role: "Heading", name: Some(row.label.clone()), bounds: t, enabled: true, ..Default::default() });
                        continue;
                    }
                    let selected = i == chosen;
                    if selected {
                        let sel = if focused { style::c(p.list_selected_background) } else { style::c(olo_theme::mix(p.list_selected_background, p.panel_background, 0.35)) };
                        style::fill_rect(window, r, sel);
                    } else if hover == Some(i) {
                        style::fill_rect(window, r, style::shade(&p, p.panel_background, 1.1));
                    }
                    let text = if selected { style::c(p.list_selected_item_text) } else { style::c(p.panel_text) };
                    let icon = Bounds { origin: point(r.left() + px(m::WINDOW_INSET), r.center().y - px(8.0)), size: size(px(16.0), px(16.0)) };
                    if !row.icon.is_empty() {
                        icons::paint(window, icon, &row.icon);
                    }
                    let value_w = row.value.as_ref().map_or(0.0, |v| f32::from(style::text_width(window, v.clone(), style::PLAIN_SIZE, plain.clone())) + m::SPACING);
                    let left = icon.right() + px(6.0);
                    let room = (r.right() - px(m::WINDOW_INSET + value_w) - left).max(px(0.0));
                    style::label_fit(window, cx, left, r.center().y, style::Align::Left, row.label.clone(), style::PLAIN_SIZE, plain.clone(), text, room);
                    if let Some(v) = &row.value {
                        let quiet = if selected { text } else { style::disabled_text(&p) };
                        style::label(window, cx, r.right() - px(m::WINDOW_INSET), r.center().y, style::Align::Right, v.clone(), style::PLAIN_SIZE, plain.clone(), quiet);
                    }
                    agent::record(window, cx, agent::Node { role: "ListItem", name: Some(row.label.clone()), value: row.value.clone(), bounds: r, enabled: true, clickable: true, selected: Some(selected), ..Default::default() });
                }
                let (st, hbs) = (paint_state.clone(), hitboxes.clone());
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                    if phase.bubble() && hbs.iter().position(|h| h.as_ref().is_some_and(|h| h.is_hovered(window))) != st.read(cx).hover {
                        window.refresh();
                    }
                });
                let (st, choose) = (paint_state.clone(), choose.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || e.button != MouseButton::Left {
                        return;
                    }
                    let Some(i) = hitboxes.iter().position(|h| h.as_ref().is_some_and(|h| h.is_hovered(window))) else { return };
                    let focus = st.read(cx).focus.clone();
                    window.focus(&focus);
                    if i != chosen
                        && let Some(f) = &choose
                    {
                        f(i, window, cx);
                    }
                });
            },
        )
        .w_full()
        .h(px(total));
        let keys_rows = rows.clone();
        let backdrop = canvas(
            |_, _, _| {},
            |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, ground(&p));
                style::vline_left_of(window, b.right(), b.top(), b.bottom(), edge(&p));
            },
        )
        .absolute()
        .inset_0();
        div()
            .id(SharedString::from(format!("sidebar-{id}")))
            .track_focus(&focus)
            .relative()
            .size_full()
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                let step = match e.keystroke.key.as_str() {
                    "up" if !e.keystroke.modifiers.modified() => -1,
                    "down" if !e.keystroke.modifiers.modified() => 1,
                    _ => return,
                };
                cx.stop_propagation();
                if let (Some(to), Some(f)) = (stepped(&keys_rows, chosen, step), &on_choose) {
                    f(to, window, cx);
                }
            })
            .children((!bare).then_some(backdrop))
            .child(ScrollView::new(SharedString::from(format!("sidebar-scroll-{id}")), &scroll).border(false).auto_hide(true).child(list))
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
