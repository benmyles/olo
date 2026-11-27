//! Sidebars for Agent Screens (`apps/screen`):
//! - [`MultiSidebar`]: multi-host outline sidebar used whenever one or more Rex
//!   hosts are present.
//! - [`LocalSidebar`]: local-only sidebar matching `olo_kit::widgets::Sidebar`
//!   visually while accepting an external `ScrollHandle` + `suppress_reveal` and
//!   always delivering `on_choose` on plain click in narrow layouts.
//!
//! Design rules (Step 6 Round 3):
//! - Three clear typographic levels:
//!   1. Connected host heading: bold `PLAIN_SIZE` in primary ink (`panel_text`).
//!      Disconnected / not-set-up host heading: bold `PLAIN_SIZE` in secondary
//!      ink, icon faded (`DISABLED_OPACITY`).
//!   2. Running screen: regular `PLAIN_SIZE` in primary ink, 8 pt green dot.
//!   3. Stopped / archived / disconnected screen: secondary or tertiary ink
//!      (>= 3.2:1 WCAG), grey dot or hollow 8 pt ring (matching dot outer size
//!      and centre).
//! - BeOS outline disclosure latch at the LEFT of headings with screens: 7 pt
//!   solid triangle centred at `x = 12 pt` in secondary ink (primary only when
//!   selected), with its own dedicated interactive hit child so `paint_key`
//!   reconciliation never loses latch clicks (Finding 25).
//! - Host icon (16 pt) at `x = 20..36 pt` (centre `x = 28 pt`); screen dots and
//!   rings centred directly underneath at `x = 28 pt`; all host and screen names
//!   share one text edge at `TEXT_X = 42 pt`.
//! - ONE right edge (`RIGHT_INSET = 12 pt`, `x = 216 pt` at 228 pt width) for
//!   ALL screen ages AND ALL host state slots (no reserved empty latch column).
//!   Host state slots are capped at `MAX_SLOT_W = 72 pt`, aligned on the host
//!   name's baseline (`style::baseline_at` + `style::label_on_baseline_fit`),
//!   with at least `NAME_SLOT_GAP = 8 pt` between host name and state slot.
//! - Host and screen names use middle truncation (`style::label_fit_middle`) so
//!   distinguishing suffixes survive (`-04.internal`, `Suite v2`) with full-name
//!   hover tooltips.
//! - Uniform `SECTION_GAP = 8 pt` above every host heading after the first,
//!   whatever stands above it; nothing extra below.
//! - Keyboard: ↑/↓ across headings and visible rows; ← collapses an open heading
//!   or jumps from a screen row to its host heading; → opens a collapsed heading
//!   or steps into its first screen row.

use std::{cell::Cell, collections::HashSet, rc::Rc};

use olo_kit::{
    Bounds, Element, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, KeyDownEvent, Modifiers, MouseButton, Painter, ParentElement as _, PathBuilder, Pixels, Point,
    ScrollHandle, SharedString, Styled as _, WindowCx, agent, canvas, control_canvas, div, icons, metrics as m, point, px, review, style, theme,
    widgets::{DISABLED_OPACITY, ScrollView, sidebar as kit_sidebar},
};
use olo_screen_pane::pane::{paint_status_dot, paint_status_ring, tertiary_ink};

/// Height of every heading row and screen row.
pub const ROW_H: f32 = 24.0;
/// Uniform inter-host gap inserted above every host heading except the first.
pub const SECTION_GAP: f32 = 8.0;
/// Left inset before the outline latch column (`x = 8..16`, centre `12 pt`).
pub const LEFT_INSET: f32 = 8.0;
/// Width of the left outline disclosure latch column (`7 pt` triangle centred at `x = 12 pt`).
pub const LATCH_COL: f32 = 8.0;
/// Gap between the latch column and the 16 pt icon/dot column (`x = 20..36`, centre `28 pt`).
pub const LATCH_GAP: f32 = 4.0;
/// Width of the mini-icon / status-dot column.
pub const ICON_COL: f32 = 16.0;
/// Gap between the icon/dot column and the single shared text edge (`x = 42 pt`).
pub const ICON_GAP: f32 = 6.0;
/// Single shared text edge for every heading name and every screen name (`8 + 8 + 4 + 16 + 6 = 42 pt`).
pub const TEXT_X: f32 = LEFT_INSET + LATCH_COL + LATCH_GAP + ICON_COL + ICON_GAP;
/// Shared right inset for ALL screen ages AND ALL host state slots (`x = 216 pt` at 228 pt width).
pub const RIGHT_INSET: f32 = 12.0;
/// Minimum horizontal gap between a host name and its right-aligned state slot.
pub const NAME_SLOT_GAP: f32 = 8.0;
/// Maximum width reserved for a host heading's right-aligned state slot.
pub const MAX_SLOT_W: f32 = 72.0;
/// Semantic status dot/ring diameter (matches `ScreenPane` header).
pub const DOT_SIZE: f32 = 8.0;

/// Paint a crisp 7 pt BeOS outline disclosure triangle centred in `r` on whole
/// device pixels (pointing down when `open`, right when folded).
pub fn paint_outline_latch(g: &mut Painter, r: Bounds<Pixels>, open: bool, color: Hsla) {
    let m = style::snap_centred(g, r.center(), 7.0, 7.0).center();
    let pts = if open {
        let top = style::snap_coord(g, m.y - px(2.0));
        [point(m.x - px(3.5), top), point(m.x + px(3.5), top), point(m.x, top + px(4.0))]
    } else {
        let left = style::snap_coord(g, m.x - px(2.0));
        [point(left, m.y - px(3.5)), point(left + px(4.0), m.y), point(left, m.y + px(3.5))]
    };
    let mut path = PathBuilder::fill();
    path.move_to(pts[0]);
    path.line_to(pts[1]);
    path.line_to(pts[2]);
    path.close();
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
    }
}

/// Visual state of a screen row's 8 pt status mark and label ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DotKind {
    /// Connected and running: semantic green dot (`p.success`), primary ink.
    Running,
    /// Connected and stopped: muted neutral filled dot, secondary ink.
    Stopped,
    /// Connected and archived: muted neutral filled dot, tertiary ink.
    Archived,
    /// Last-known screen of a disconnected/reconnecting host: hollow 1 pt ring
    /// (kept when selected), tertiary ink >= 3.2:1, no per-row age.
    Disconnected,
}

/// One visual item in the multi-host outline sidebar.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Entry {
    /// Selectable host section heading (`This Computer`, `dev-box`, …).
    Heading {
        /// Stable host key (`"local"` or `RexHost::key`).
        key: SharedString,
        /// Exact person/Rex spelling (never uppercased).
        label: SharedString,
        /// 16 pt icon name (`"desktop"` for This Computer, `"rex"` for hosts).
        icon: SharedString,
        /// Whether the host is offline or not set up (secondary name ink, faded icon).
        disconnected: bool,
        /// Whether this host's screens are currently expanded.
        open: bool,
        /// Whether this host has screens underneath (shows left disclosure latch).
        has_children: bool,
        /// Optional right-aligned state slot (`"No screens"`, `"Not set up"`,
        /// `"Disconnected"`, `"Reconnecting…"`, `"2 running"`, `"3 stopped"`).
        status: Option<SharedString>,
        /// Whether `status` indicates a host error (rendered in `p.failure`).
        status_error: bool,
        /// Accessible heading name for agents.
        agent_name: SharedString,
    },
    /// Selectable screen row with 8 pt semantic status dot/ring and optional
    /// right-aligned short age (always `None` when disconnected).
    Screen { label: SharedString, value: Option<SharedString>, dot: DotKind, agent_name: SharedString },
}

impl Entry {
    pub fn is_heading(&self) -> bool {
        matches!(self, Entry::Heading { .. })
    }

    pub fn is_screen(&self) -> bool {
        matches!(self, Entry::Screen { .. })
    }
}

/// Extra vertical gap above entry `i`: `SECTION_GAP` (8 pt) above every host
/// heading except the first, whatever stands above it.
pub fn gap_above(entries: &[Entry], i: usize) -> f32 {
    if i > 0 && entries[i].is_heading() { SECTION_GAP } else { 0.0 }
}

pub fn tall_at(entries: &[Entry], i: usize) -> f32 {
    ROW_H + gap_above(entries, i)
}

fn top_of(entries: &[Entry], i: usize) -> f32 {
    m::TIGHT + (0..i.min(entries.len())).map(|n| tall_at(entries, n)).sum::<f32>()
}

/// Next entry in direction `step` (`-1` up, `+1` down) over ALL visible rows
/// (both headings and screen rows).
pub fn stepped(entries: &[Entry], from: Option<usize>, step: i32) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }
    let next = match from {
        Some(cur) => cur as i32 + step,
        None if step > 0 => 0,
        None => entries.len() as i32 - 1,
    };
    (next >= 0 && next < entries.len() as i32).then_some(next as usize)
}

/// Index of the parent heading for a screen row `i`.
pub fn parent_heading(entries: &[Entry], i: usize) -> Option<usize> {
    entries[..i].iter().rposition(Entry::is_heading)
}

type ChooseFn = Rc<dyn Fn(&usize, &mut WindowCx)>;
type ToggleFn = Rc<dyn Fn(&usize, &mut WindowCx)>;
type PickFn = Rc<dyn Fn(&(usize, Modifiers), &mut WindowCx)>;
type MenuFn = Rc<dyn Fn(&(usize, Point<Pixels>), &mut WindowCx)>;
type AllFn = Rc<dyn Fn(&(), &mut WindowCx)>;

struct MultiSidebarState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    revealed: Cell<Option<usize>>,
}

pub struct MultiSidebar {
    id: SharedString,
    entries: Rc<[Entry]>,
    chosen: Option<usize>,
    also: Rc<[usize]>,
    scroll: Option<ScrollHandle>,
    suppress_reveal: bool,
    bare: bool,
    edge: bool,
    on_choose: Option<ChooseFn>,
    on_toggle: Option<ToggleFn>,
    on_pick: Option<PickFn>,
    on_menu: Option<MenuFn>,
    on_select_all: Option<AllFn>,
}

impl MultiSidebar {
    pub fn new(id: impl Into<SharedString>, entries: impl Into<Rc<[Entry]>>, chosen: Option<usize>) -> MultiSidebar {
        MultiSidebar {
            id: id.into(),
            entries: entries.into(),
            chosen,
            also: Rc::from([]),
            scroll: None,
            suppress_reveal: false,
            bare: false,
            edge: true,
            on_choose: None,
            on_toggle: None,
            on_pick: None,
            on_menu: None,
            on_select_all: None,
        }
    }

    /// Omit the sidebar's own background/edge canvas when a parent container
    /// (e.g. `Viewer::wrap_sidebar`) already paints the ground and divider.
    pub fn bare(mut self, on: bool) -> Self {
        self.bare = on;
        self
    }

    /// Whether a 1-device-pixel right divider sits on the sidebar's right edge
    /// (`false` in full-width narrow `alone` mode so rows fill flush to the
    /// window edge with no stray hairline — Agy-review R1 #9).
    pub fn edge(mut self, on: bool) -> Self {
        self.edge = on;
        self
    }

    pub fn also(mut self, rows: impl Into<Rc<[usize]>>) -> Self {
        self.also = rows.into();
        self
    }

    /// Supply an externally owned `ScrollHandle` so the host view can save and
    /// restore scroll offset across reload-in-place (Finding 19).
    pub fn scroll(mut self, handle: ScrollHandle) -> Self {
        self.scroll = Some(handle);
        self
    }

    /// Skip auto-scrolling to `chosen` on this build (used right after restoring
    /// a saved scroll offset).
    pub fn suppress_reveal(mut self, on: bool) -> Self {
        self.suppress_reveal = on;
        self
    }

    pub fn on_choose(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }

    /// Invoked when the person clicks a heading's left latch, double-clicks a
    /// heading with screens, or presses ←/→ to collapse/open a heading.
    pub fn on_toggle(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    pub fn on_pick(mut self, f: impl Fn(&(usize, Modifiers), &mut WindowCx) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    pub fn on_menu(mut self, f: impl Fn(&(usize, Point<Pixels>), &mut WindowCx) + 'static) -> Self {
        self.on_menu = Some(Rc::new(f));
        self
    }

    pub fn on_select_all(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.on_select_all = Some(Rc::new(f));
        self
    }

    pub fn build(self, cx: &mut WindowCx) -> Element {
        let state =
            cx.keyed_state(SharedString::from(format!("olo-multi-sidebar-{}", self.id)), |_| MultiSidebarState { focus: FocusHandle::new(), scroll: ScrollHandle::new(), revealed: Cell::new(None) });
        let focused = state.focus.contains_focused(cx);
        let MultiSidebar { id, entries, chosen, also, scroll, suppress_reveal, bare, edge, on_choose, on_toggle, on_pick, on_menu, on_select_all } = self;
        let scroll = scroll.unwrap_or_else(|| state.scroll.clone());
        if suppress_reveal {
            state.revealed.set(chosen);
        } else if let Some(c) = chosen
            && state.revealed.replace(Some(c)) != Some(c)
        {
            let top = top_of(&entries, c) + gap_above(&entries, c);
            scroll.reveal_span(px(top - m::TIGHT), px(top + ROW_H));
        }
        let line = 1.0 / cx.scale_factor();
        // B.11 (#29): O(1) secondary-selection lookup per row (zero allocation when empty)
        // instead of O(N*M) `also.contains(&i)` slice scanning.
        let also_set: Option<HashSet<usize>> = (!also.is_empty()).then(|| also.iter().copied().collect());

        let list = div().w_full().flex().flex_col().pt(px(m::TIGHT)).pb(px(m::TIGHT)).children(entries.iter().enumerate().map(|(i, entry)| {
            let gap = gap_above(&entries, i);
            let selected = chosen == Some(i) || (entry.is_screen() && also_set.as_ref().is_some_and(|s| s.contains(&i)));
            let key = style::key((entry, selected, selected && focused));
            let painted = entry.clone();
            let el = control_canvas(move |g, r, cstate| {
                let p = theme::palette();
                let ground_rgba = sidebar_ground_rgba(&p);
                let tert = tertiary_ink(&p, ground_rgba);
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                if selected {
                    style::fill_rect(g, r, style::c(style::selection_ground(&p, p.panel_background, focused)));
                } else if cstate.hovered {
                    style::fill_rect(g, r, style::shade(&p, p.panel_background, 1.1));
                }
                let sel_text = style::c(style::selection_text(&p, p.panel_text, focused));
                // Optically centred text line inside the 24 pt band (balances lowercase descenders).
                let text_cy = r.center().y - px(0.5);
                let latch_left = r.left() + px(LEFT_INSET);
                let icon_cx = r.left() + px(LEFT_INSET + LATCH_COL + LATCH_GAP + ICON_COL / 2.0);
                let text_left = r.left() + px(TEXT_X);
                let right = r.right() - px(RIGHT_INSET);

                match &painted {
                    Entry::Heading { label, icon, disconnected, open, has_children, status, status_error, agent_name, .. } => {
                        if *has_children {
                            let lbox = Bounds::from_corners(point(latch_left, r.top()), point(latch_left + px(LATCH_COL), r.bottom()));
                            let latch_ink = if selected { sel_text } else { style::secondary_text(&p) };
                            paint_outline_latch(g, lbox, *open, latch_ink);
                        }

                        let icon_box = style::snap_centred(g, point(icon_cx, r.center().y), 16.0, 16.0);
                        if *disconnected {
                            icons::paint_faded(g, icon_box, icon, DISABLED_OPACITY);
                        } else {
                            icons::paint(g, icon_box, icon);
                        }

                        // Right-aligned state slot shares ONE right edge with all screen ages,
                        // capped at MAX_SLOT_W (72 pt), aligned on the heading's exact baseline.
                        let heading_baseline = style::baseline_at(g, text_cy, style::PLAIN_SIZE);
                        let name_right = if let Some(st) = status {
                            let st_color = if selected {
                                if *status_error { sel_text } else { sel_text.opacity(0.85) }
                            } else if *status_error {
                                style::c(p.failure)
                            } else if *disconnected {
                                tert
                            } else {
                                style::secondary_text(&p)
                            };
                            let st_bounds = style::label_on_baseline_fit(g, right, heading_baseline, style::Align::Right, st.clone(), style::DETAIL_SIZE, plain, st_color, px(MAX_SLOT_W));
                            st_bounds.left() - px(NAME_SLOT_GAP)
                        } else {
                            right
                        };

                        let name_color = if selected {
                            sel_text
                        } else if *disconnected {
                            style::secondary_text(&p)
                        } else {
                            style::c(p.panel_text)
                        };
                        let name_room = (name_right - text_left).max(px(0.0));
                        style::label_fit_middle(g, text_left, text_cy, style::Align::Left, label.clone(), style::PLAIN_SIZE, bold, name_color, name_room);

                        g.agent(agent::Node {
                            role: "Heading",
                            name: Some(agent_name.clone()),
                            value: status.clone(),
                            bounds: r,
                            enabled: true,
                            clickable: true,
                            selected: Some(selected),
                            ..Default::default()
                        });
                    }
                    Entry::Screen { label, value, dot, agent_name } => {
                        // Dot/ring centred directly under the 16 pt host icon (x = 28 pt).
                        let dot_center = point(icon_cx, text_cy - px(0.5));
                        match dot {
                            DotKind::Running => paint_status_dot(g, dot_center, DOT_SIZE, style::c(p.success)),
                            DotKind::Stopped => {
                                let dc = if selected { sel_text.opacity(0.65) } else { tert };
                                paint_status_dot(g, dot_center, DOT_SIZE, dc);
                            }
                            DotKind::Archived => {
                                let dc = if selected { sel_text.opacity(0.55) } else { tert };
                                paint_status_dot(g, dot_center, DOT_SIZE, dc);
                            }
                            DotKind::Disconnected => {
                                // Hollow 1 pt ring with exact same outer diameter and centre as dot.
                                let rc = if selected { sel_text.opacity(0.75) } else { tert };
                                paint_status_ring(g, dot_center, DOT_SIZE, rc);
                            }
                        }

                        let label_color = if selected {
                            if matches!(dot, DotKind::Disconnected) { sel_text.opacity(0.88) } else { sel_text }
                        } else {
                            match dot {
                                DotKind::Running => style::c(p.panel_text),
                                DotKind::Stopped => style::secondary_text(&p),
                                DotKind::Archived | DotKind::Disconnected => tert,
                            }
                        };

                        let name_right = if let Some(v) = value {
                            let val_color = if selected {
                                sel_text.opacity(0.9)
                            } else if matches!(dot, DotKind::Archived | DotKind::Disconnected) {
                                tert
                            } else {
                                style::secondary_text(&p)
                            };
                            let vb = style::label(g, right, text_cy, style::Align::Right, v.clone(), style::PLAIN_SIZE, plain.clone(), val_color);
                            vb.left() - px(m::SPACING)
                        } else {
                            right
                        };
                        let name_room = (name_right - text_left).max(px(0.0));
                        style::label_fit_middle(g, text_left, text_cy, style::Align::Left, label.clone(), style::PLAIN_SIZE, plain, label_color, name_room);

                        g.agent(agent::Node {
                            role: "ListItem",
                            name: Some(agent_name.clone()),
                            value: value.clone(),
                            bounds: r,
                            enabled: true,
                            clickable: true,
                            selected: Some(selected),
                            ..Default::default()
                        });
                    }
                }
            })
            .paint_key(key)
            .h_full();
            let el = if edge { el.mr(px(line)) } else { el };

            let focus_r = state.focus.clone();
            let focus_l = state.focus.clone();
            let choose = on_choose.clone();
            let toggle = on_toggle.clone();
            let toggle_latch = on_toggle.clone();
            let pick = on_pick.clone();
            let menu = on_menu.clone();
            let is_heading = entry.is_heading();
            let has_children = matches!(entry, Entry::Heading { has_children: true, .. });

            // Finding 25: plain left-click always delivers `on_choose` (even when
            // `chosen == Some(i)`) so narrow `Front::Pane` clicking the already-selected
            // row reveals the detail pane via `side_pane.show_rest()`.
            let body = el.on_mouse_down(MouseButton::Left, move |e, cx| {
                focus_l.focus(cx);
                if is_heading && has_children && e.click_count >= 2 {
                    if let Some(f) = &toggle {
                        f(&i, cx);
                    }
                    return;
                }
                let held = e.modifiers;
                if !is_heading
                    && (held.platform || held.control || held.shift)
                    && let Some(f) = &pick
                {
                    f(&(i, held), cx);
                } else if let Some(f) = &choose {
                    f(&i, cx);
                }
            });

            // Finding 25: dedicated interactive child hit element for the left
            // disclosure latch so `paint_key` caching can never lose its hit target.
            let latch_hit = (is_heading && has_children).then(|| {
                let focus_t = state.focus.clone();
                div().absolute().left_0().top_0().w(px(LEFT_INSET + LATCH_COL + LATCH_GAP)).h_full().on_mouse_down(MouseButton::Left, move |_, cx| {
                    focus_t.focus(cx);
                    cx.stop_propagation();
                    if let Some(f) = &toggle_latch {
                        f(&i, cx);
                    }
                })
            });

            // Flex column (`width: auto` on `body`) so `.mr(px(line))` reserves
            // the 1-device-pixel right divider column instead of overflowing it
            // (matching `LocalSidebar` and kit `Sidebar` — Agy-review R2 #4).
            let row = div()
                .relative()
                .w_full()
                .h(px(ROW_H))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .on_mouse_down(MouseButton::Right, move |e, cx| {
                    focus_r.focus(cx);
                    if let Some(f) = &menu {
                        f(&(i, e.position), cx);
                    }
                })
                .child(body)
                .children(latch_hit);

            if gap > 0.0 { div().w_full().pt(px(gap)).flex().flex_col().flex_shrink_0().child(row).into_any_element() } else { row.into_any_element() }
        }));

        let grid = canvas(review::own_grid).absolute().inset_0();
        let keys_entries = entries.clone();
        let backdrop = (!bare).then(|| {
            canvas(move |g, b| {
                let p = theme::palette();
                style::fill_rect(g, b, kit_sidebar::ground(&p));
                if edge {
                    style::vline_left_of(g, b.right(), b.top(), b.bottom(), kit_sidebar::edge(&p));
                }
            })
            .paint_key(style::key(edge))
            .absolute()
            .inset_0()
        });

        div()
            .id(SharedString::from(format!("multi-sidebar-{id}")))
            .track_focus(&state.focus)
            .tab_stop()
            .on_focus_changed(|_, cx| cx.invalidate_owner())
            .relative()
            .size_full()
            .on_key_down(move |e: &KeyDownEvent, cx| {
                let key = e.keystroke.key.as_str();
                if (e.keystroke.modifiers.platform || e.keystroke.modifiers.control) && key == "a" {
                    if let Some(f) = &on_select_all {
                        f(&(), cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                match key {
                    "up" | "down" => {
                        let step = if key == "up" { -1 } else { 1 };
                        if let Some(next) = stepped(&keys_entries, chosen, step)
                            && let Some(f) = &on_choose
                        {
                            f(&next, cx);
                            cx.stop_propagation();
                        }
                    }
                    "left" => {
                        if let Some(cur) = chosen {
                            match keys_entries.get(cur) {
                                Some(Entry::Heading { open: true, has_children: true, .. }) => {
                                    if let Some(f) = &on_toggle {
                                        f(&cur, cx);
                                        cx.stop_propagation();
                                    }
                                }
                                Some(Entry::Screen { .. }) => {
                                    if let Some(parent) = parent_heading(&keys_entries, cur)
                                        && let Some(f) = &on_choose
                                    {
                                        f(&parent, cx);
                                        cx.stop_propagation();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "right" => {
                        if let Some(cur) = chosen
                            && let Some(Entry::Heading { open, has_children: true, .. }) = keys_entries.get(cur)
                        {
                            if !open {
                                if let Some(f) = &on_toggle {
                                    f(&cur, cx);
                                    cx.stop_propagation();
                                }
                            } else if cur + 1 < keys_entries.len()
                                && keys_entries[cur + 1].is_screen()
                                && let Some(f) = &on_choose
                            {
                                f(&(cur + 1), cx);
                                cx.stop_propagation();
                            }
                        }
                    }
                    _ => {}
                }
            })
            .children(backdrop)
            .child(grid)
            .child(ScrollView::new(&scroll).border(false).auto_hide(true).child(div().relative().w_full().child(list)).into_element())
            .into_any_element()
    }
}

/// Exact RGBA of the sidebar ground (`kit_sidebar::ground`), for WCAG contrast
/// computations (`tertiary_ink`).
pub fn sidebar_ground_rgba(p: &olo_kit::palette::Palette) -> olo_theme::Rgba {
    p.shade(p.panel_background, 1.04)
}

/// Local-only sidebar matching `olo_kit::widgets::Sidebar` pixel-for-pixel while
/// accepting an external `ScrollHandle` + `suppress_reveal` (B.10 / #19) and
/// always firing `on_choose` on plain click of the already-selected row so
/// narrow local-only mode reveals the detail pane (B.10 / #25).
pub struct LocalSidebar {
    id: SharedString,
    rows: Rc<[kit_sidebar::Row]>,
    chosen: usize,
    also: Rc<[usize]>,
    scroll: Option<ScrollHandle>,
    suppress_reveal: bool,
    bare: bool,
    edge: bool,
    on_choose: Option<ChooseFn>,
    on_pick: Option<PickFn>,
    on_menu: Option<MenuFn>,
    on_select_all: Option<AllFn>,
}

struct LocalSidebarState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    revealed: Cell<Option<usize>>,
}

fn local_top_of(rows: &[kit_sidebar::Row], i: usize) -> f32 {
    m::TIGHT + (0..i.min(rows.len())).map(|n| kit_sidebar::tall_at(rows, n)).sum::<f32>()
}

fn local_stepped(rows: &[kit_sidebar::Row], from: usize, step: i32) -> Option<usize> {
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

impl LocalSidebar {
    pub fn new(id: impl Into<SharedString>, rows: impl Into<Rc<[kit_sidebar::Row]>>, chosen: usize) -> LocalSidebar {
        LocalSidebar {
            id: id.into(),
            rows: rows.into(),
            chosen,
            also: Rc::from([]),
            scroll: None,
            suppress_reveal: false,
            bare: false,
            edge: true,
            on_choose: None,
            on_pick: None,
            on_menu: None,
            on_select_all: None,
        }
    }

    pub fn bare(mut self, on: bool) -> Self {
        self.bare = on;
        self
    }

    pub fn edge(mut self, on: bool) -> Self {
        self.edge = on;
        self
    }

    pub fn also(mut self, rows: impl Into<Rc<[usize]>>) -> Self {
        self.also = rows.into();
        self
    }

    pub fn scroll(mut self, handle: ScrollHandle) -> Self {
        self.scroll = Some(handle);
        self
    }

    pub fn suppress_reveal(mut self, on: bool) -> Self {
        self.suppress_reveal = on;
        self
    }

    pub fn on_choose(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_choose = Some(Rc::new(f));
        self
    }

    pub fn on_pick(mut self, f: impl Fn(&(usize, Modifiers), &mut WindowCx) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    pub fn on_menu(mut self, f: impl Fn(&(usize, Point<Pixels>), &mut WindowCx) + 'static) -> Self {
        self.on_menu = Some(Rc::new(f));
        self
    }

    pub fn on_select_all(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.on_select_all = Some(Rc::new(f));
        self
    }

    pub fn build(self, cx: &mut WindowCx) -> Element {
        let state =
            cx.keyed_state(SharedString::from(format!("olo-local-sidebar-{}", self.id)), |_| LocalSidebarState { focus: FocusHandle::new(), scroll: ScrollHandle::new(), revealed: Cell::new(None) });
        let focused = state.focus.contains_focused(cx);
        let LocalSidebar { id, rows, chosen, also, scroll, suppress_reveal, bare, edge, on_choose, on_pick, on_menu, on_select_all } = self;
        let scroll = scroll.unwrap_or_else(|| state.scroll.clone());
        if suppress_reveal {
            state.revealed.set(Some(chosen));
        } else if state.revealed.replace(Some(chosen)) != Some(chosen) {
            let top = local_top_of(&rows, chosen);
            scroll.reveal_span(px(top - m::TIGHT), px(top + kit_sidebar::ROW));
        }
        let line = 1.0 / cx.scale_factor();
        let also_set: Option<HashSet<usize>> = (!also.is_empty()).then(|| also.iter().copied().collect());
        let list = div().w_full().flex().flex_col().pt(px(m::TIGHT)).children(rows.iter().enumerate().map(|(i, row)| {
            let tall = kit_sidebar::tall_at(&rows, i);
            if row.heading && row.label.is_empty() {
                return div().h(px(tall)).flex_shrink_0();
            }
            let selected = !row.heading && (i == chosen || also_set.as_ref().is_some_and(|s| s.contains(&i)));
            let key = style::key((row, selected, selected && focused));
            let painted = row.clone();
            let el = control_canvas(move |g, r, cstate| {
                let p = theme::palette();
                let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
                let row = &painted;
                if row.heading {
                    let t = style::label(
                        g,
                        r.left() + px(m::WINDOW_INSET),
                        r.bottom() - px(kit_sidebar::HEADING / 2.0 - 2.0),
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
                    style::fill_rect(g, r, style::c(style::selection_ground(&p, p.panel_background, focused)));
                } else if cstate.hovered {
                    style::fill_rect(g, r, style::shade(&p, p.panel_background, 1.1));
                }
                let text = style::c(if selected { style::selection_text(&p, p.panel_text, focused) } else { p.panel_text });
                let icon = Bounds { origin: point(r.left() + px(m::WINDOW_INSET), r.center().y - px(8.0)), size: olo_kit::size(px(16.0), px(16.0)) };
                if let Some(hue) = row.dot {
                    kit_sidebar::paint_dot(g, icon, p.hue(hue), &p);
                } else if !row.icon.is_empty() {
                    icons::paint(g, icon, &row.icon);
                }
                let value_w = row.value.as_ref().map_or(0.0, |v| f32::from(style::text_width(v.clone(), style::PLAIN_SIZE, plain.clone())) + m::SPACING);
                let left = icon.right() + px(6.0);
                let room = (r.right() - px(m::WINDOW_INSET + value_w) - left).max(px(0.0));
                style::label_fit(g, left, r.center().y, style::Align::Left, row.label.clone(), style::PLAIN_SIZE, plain.clone(), text, room);
                if let Some(v) = &row.value {
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
            .flex_shrink_0();
            let el = if edge { el.mr(px(line)) } else { el };
            if row.heading {
                return el;
            }
            let (focus, choose, pick, menu) = (state.focus.clone(), on_choose.clone(), on_pick.clone(), on_menu.clone());
            // B.10 (#25): plain left-click ALWAYS delivers `on_choose` even when
            // `i == chosen` so clicking the already-selected row in a narrow
            // local-only window reveals the detail pane (`side_pane.show_rest()`).
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
                } else if let Some(f) = &choose {
                    f(&i, cx);
                }
            })
        }));

        let grid = canvas(review::own_grid).absolute().inset_0();
        let keys_rows = rows.clone();
        let backdrop = (!bare).then(|| {
            canvas(move |g, b| {
                let p = theme::palette();
                style::fill_rect(g, b, kit_sidebar::ground(&p));
                if edge {
                    style::vline_left_of(g, b.right(), b.top(), b.bottom(), kit_sidebar::edge(&p));
                }
            })
            .paint_key(style::key(edge))
            .absolute()
            .inset_0()
        });

        div()
            .id(SharedString::from(format!("sidebar-{id}")))
            .track_focus(&state.focus)
            .tab_stop()
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
                if let (Some(to), Some(f)) = (local_stepped(&keys_rows, chosen, step), &on_choose) {
                    f(&to, cx);
                }
            })
            .children(backdrop)
            .child(ScrollView::new(&scroll).border(false).auto_hide(true).child(div().relative().w_full().child(grid).child(list)).into_element())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(key: &str, open: bool, has_children: bool) -> Entry {
        Entry::Heading {
            key: key.to_owned().into(),
            label: key.to_owned().into(),
            icon: "rex".into(),
            disconnected: false,
            open,
            has_children,
            status: (!has_children).then(|| SharedString::from("No screens")),
            status_error: false,
            agent_name: key.to_owned().into(),
        }
    }

    fn screen(name: &str, dot: DotKind) -> Entry {
        Entry::Screen { label: name.to_owned().into(), value: (dot != DotKind::Disconnected).then(|| SharedString::from("now")), dot, agent_name: name.to_owned().into() }
    }

    #[test]
    fn spacing_adds_uniform_8pt_gap_above_every_heading_after_first() {
        let entries = vec![
            heading("local", true, true),
            screen("agent", DotKind::Running),
            heading("dev-box", true, true),
            screen("review", DotKind::Running),
            heading("staging", true, false),
            heading("docs-box", true, false),
            heading("build-farm", false, true),
            heading("gpu-box", true, false),
        ];
        assert_eq!(gap_above(&entries, 0), 0.0, "first heading has no extra gap");
        assert_eq!(gap_above(&entries, 1), 0.0, "screen row has no extra gap");
        assert_eq!(gap_above(&entries, 2), SECTION_GAP, "heading after local screen gets 8 pt gap");
        assert_eq!(gap_above(&entries, 4), SECTION_GAP, "heading after dev-box screen gets 8 pt gap");
        assert_eq!(gap_above(&entries, 5), SECTION_GAP, "empty host after empty host gets 8 pt gap");
        assert_eq!(gap_above(&entries, 6), SECTION_GAP, "collapsed host after empty host gets 8 pt gap");
        assert_eq!(gap_above(&entries, 7), SECTION_GAP, "host after collapsed host gets 8 pt gap");
        assert_eq!(TEXT_X, 42.0, "shared left text edge matches BeOS left-latch geometry");
    }

    #[test]
    fn outline_navigation_steps_headings_and_screens_and_finds_parent() {
        let entries = vec![heading("local", true, true), screen("agent", DotKind::Running), screen("design", DotKind::Stopped), heading("staging", true, false)];
        assert_eq!(stepped(&entries, None, 1), Some(0));
        assert_eq!(stepped(&entries, Some(0), 1), Some(1));
        assert_eq!(stepped(&entries, Some(1), 1), Some(2));
        assert_eq!(stepped(&entries, Some(2), 1), Some(3));
        assert_eq!(stepped(&entries, Some(3), 1), None);
        assert_eq!(stepped(&entries, Some(3), -1), Some(2));
        assert_eq!(parent_heading(&entries, 1), Some(0));
        assert_eq!(parent_heading(&entries, 2), Some(0));
    }

    #[test]
    fn agy_r1_f9_sidebar_bare_and_edge_builders_omit_hairline_in_narrow_alone_mode() {
        let multi_default = MultiSidebar::new("test", vec![heading("local", true, false)], Some(0));
        assert!(!multi_default.bare && multi_default.edge, "standalone MultiSidebar keeps ground and right edge");
        let multi_alone = multi_default.bare(true).edge(false);
        assert!(multi_alone.bare && !multi_alone.edge, "narrow alone MultiSidebar omits backdrop and right edge");

        let local_default = LocalSidebar::new("test", vec![kit_sidebar::Row::new("agent-screens", "qa")], 0);
        assert!(!local_default.bare && local_default.edge, "standalone LocalSidebar keeps ground and right edge");
        let local_alone = local_default.bare(true).edge(false);
        assert!(local_alone.bare && !local_alone.edge, "narrow alone LocalSidebar omits backdrop and right edge");
    }
}
