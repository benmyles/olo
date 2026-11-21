//! BTabView: folder tabs over a raised panel holding what the chosen tab
//! shows, drawn as the tab strip's are (one tab look in Olo): BeOS's
//! slanted in Classic, Mac OS 9's rounded in Platinum; the others a shade
//! off the panel, the chosen one in its colour and open into it, its name
//! in bold. A click on a tab chooses it,
//! and so do Next and Previous Tab (⌃Tab and ⌃⇧Tab, ⌃Page Down and
//! ⌃Page Up) wherever the keyboard is in the window, unless what has it
//! takes them (a screen being controlled): the first tab view in the
//! window has them. Agents find each tab as a `Tab`, selected or not.
//!
//! ```ignore
//! TabView::new("tabs", vec!["Live".into(), "History".into()], chosen).on_select(cx.msg_with(|i: &usize| Msg::Tab(*i))).child(content)
//! ```

use std::rc::Rc;

use olo_interface::{
    Bounds, Element, FontWeight, InteractiveElement as _, IntoElement, MouseButton, Painter, ParentElement, Pixels, SharedString, Styled as _, WindowCx, agent, canvas, div, point, px,
};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use super::strip::{self, Folder, Shape};
use crate::{style, theme};

/// The tabs' row.
pub const TAB_HEIGHT: f32 = 22.0;
/// How far in from the panel's left the first tab is.
const FIRST: f32 = 8.0;

type SelectFn = Rc<dyn Fn(&usize, &mut WindowCx)>;

pub struct TabView {
    id: SharedString,
    tabs: Rc<[SharedString]>,
    selected: usize,
    on_select: Option<SelectFn>,
    children: SmallVec<[Element; 2]>,
    grows: bool,
}

impl TabView {
    pub fn new(id: impl Into<SharedString>, tabs: impl Into<Rc<[SharedString]>>, selected: usize) -> TabView {
        TabView { id: id.into(), tabs: crate::words::of_all(tabs.into()), selected, on_select: None, children: SmallVec::new(), grows: false }
    }

    /// As tall as what the chosen tab shows needs, at least (else it is as
    /// tall as it's given, and what it shows fits in that): for a window
    /// that grows to what it shows (Preferences' panels). Lists in it
    /// still shrink to the room there is.
    pub fn grows(mut self) -> Self {
        self.grows = true;
        self
    }

    /// A tab is chosen (clicked, or the tab keys): its index.
    pub fn on_select(mut self, f: impl Fn(&usize, &mut WindowCx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl ParentElement for TabView {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>) {
        self.children.extend(elements);
    }
}

/// How wide a tab view must be to show each of `tabs` whole (each with
/// the least room either side), in the language they're shown in: for a
/// window with many tabs, which is at least that wide (Preferences ▸
/// Network's eight cut "Service Order" short in English, and two names
/// in German, Spanish and French).
pub fn tabs_width(tabs: &[&str]) -> f32 {
    // (A name drawn at a device scale can come out a point or two wider
    // than measured unscaled: German's "Dienstreihenfolge" did.)
    const HINTING: f32 = 2.0;
    let shown: Vec<SharedString> = tabs.iter().map(|t| crate::words::of_text(t)).collect();
    let least = Shape::now().name_pads().1;
    let bold = style::ui_font(FontWeight::BOLD);
    let names: Vec<f32> = shown.iter().map(|t| f32::from(style::text_width(t.clone(), style::PLAIN_SIZE, bold.clone()))).collect();
    // (By `places` itself, at each scale it snaps to: each name whole in
    // its tab, as the tab view draws it, chosen and so in bold.)
    let whole = |w: f32| {
        let outer = Bounds { origin: point(px(0.0), px(0.0)), size: olo_interface::size(px(w), px(TAB_HEIGHT)) };
        [1.0, 1.25, 1.5, 1.75, 2.0].iter().all(|&s| places(s, outer, &shown).iter().zip(&names).all(|(r, n)| f32::from(r.size.width) - 2.0 * least >= *n + HINTING))
    };
    let sum: f32 = names.iter().sum();
    let (mut lo, mut hi) = (sum, sum + 64.0 * names.len().max(1) as f32 + 2.0 * FIRST);
    while hi - lo > 0.25 {
        let mid = (lo + hi) / 2.0;
        if whole(mid) { hi = mid } else { lo = mid }
    }
    hi.ceil()
}

/// Each tab's place along the top of `outer`, the first [`FIRST`] in
/// and the look's gap between: as wide as its name in bold and its room
/// either side; where they don't all fit, with less room (to the least),
/// then the widest cut back, their names cut short
/// ([`strip::fit_widths`]).
fn places(scale: f32, outer: Bounds<Pixels>, tabs: &[SharedString]) -> Vec<Bounds<Pixels>> {
    let sh = Shape::now();
    let (pad, least) = sh.name_pads();
    let bold = style::ui_font(FontWeight::BOLD);
    let names: Vec<f32> = tabs.iter().map(|t| f32::from(style::text_width(t.clone(), style::PLAIN_SIZE, bold.clone()))).collect();
    let n = names.len().max(1) as f32;
    let room = f32::from(outer.size.width) - 2.0 * FIRST - sh.gap * (n - 1.0);
    let pad = ((room - names.iter().sum::<f32>()) / (2.0 * n)).clamp(least, pad);
    let natural: Vec<f32> = names.iter().map(|w| w + 2.0 * pad).collect();
    let widths = strip::fit_widths(&natural, room, 2.0 * least + 24.0);
    let snap = |v: f32| px((v * scale).round() / scale);
    let (mut x, top) = (f32::from(outer.left()) + FIRST, f32::from(outer.top()));
    widths
        .iter()
        .map(|&w| {
            let r = Bounds::from_corners(point(snap(x), snap(top)), point(snap(x + w), snap(top + TAB_HEIGHT)));
            x += w + sh.gap;
            r
        })
        .collect()
}

impl IntoElement for TabView {
    fn into_element(self) -> Element {
        let TabView { id, tabs, selected, on_select, children, grows } = self;
        let n = tabs.len();
        let (painted, pressed) = (tabs.clone(), tabs.clone());
        let (clicked, keys) = (on_select.clone(), on_select);
        let face = canvas(move |g, outer| paint_face(g, outer, &painted, selected))
            .paint_key(style::key((&*tabs, selected)))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |e, cx| {
                let at = places(cx.scale_factor(), cx.listener_bounds(), &pressed);
                let hit = at.iter().position(|r| r.contains(&e.position));
                if let (Some(i), Some(f)) = (hit, &clicked)
                    && i != selected
                {
                    f(&i, cx);
                }
            })
            // The window's tab keys, unless a tab view before this one in
            // the window has them.
            .on_window_key_down(move |e, cx| {
                let step = crate::tab_step(&e.keystroke);
                if step != 0
                    && n > 1
                    && let Some(f) = &keys
                {
                    f(&((selected as i32 + step).rem_euclid(n as i32) as usize), cx);
                    cx.stop_propagation();
                }
            });
        let _ = id;
        if grows {
            // (In the flow, so it is as tall as what's in it.)
            return div()
                .relative()
                .w_full()
                .min_h_full()
                .flex()
                .flex_col()
                .child(face)
                .child(div().flex_1().flex().flex_col().mt(px(TAB_HEIGHT + m::WINDOW_INSET)).mx(px(m::WINDOW_INSET)).mb(px(m::WINDOW_INSET)).children(children));
        }
        div()
            .relative()
            .size_full()
            .child(face)
            // Inside the panel, from under the tabs (its top line is under
            // them), so what's in it is on the two-point grid.
            .child(div().absolute().left(px(m::WINDOW_INSET)).top(px(TAB_HEIGHT + m::WINDOW_INSET)).right(px(m::WINDOW_INSET)).bottom(px(m::WINDOW_INSET)).flex().flex_col().children(children))
    }
}

/// The tabs and the panel under them, as a tab strip's folder tabs
/// ([`strip`](super::strip)'s, Ben's "folder tabs" look): the others a
/// shade off the panel and behind its edge, the chosen one in the
/// panel's colour and open into it; the panel outlined, lit along its
/// top and left and shaded along its bottom and right.
fn paint_face(g: &mut Painter, outer: Bounds<Pixels>, tabs: &[SharedString], selected: usize) {
    let p = theme::palette();
    let sh = Shape::now();
    let least = sh.name_pads().1;
    let f = Folder::of(Some((style::c(p.panel_background), style::c(p.panel_text))));
    // (Raised, its light must read at night too: a strip's chosen tab is
    // lit 8% toward white there, the panel 17%; Astra, 2026-11-14.)
    let lit = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.panel_background, olo_theme::WHITE, 0.17)) } else { f.front_light };
    let s = g.scale_factor();
    let (hair, bev) = strip::lines(g);
    let at = |v: f32| px(v / s);
    let rect = |g: &mut Painter, (l, t, r, b): (f32, f32, f32, f32), c| {
        if r > l && b > t {
            style::fill_rect(g, Bounds::from_corners(point(at(l), at(t)), point(at(r), at(b))), c);
        }
    };
    let rects = places(s, outer, tabs);
    // (The panel's edge: its outline's row, on a device pixel.)
    let y0 = strip::dev(g, outer.top() + px(TAB_HEIGHT));
    let (l, rr, b) = (strip::dev(g, outer.left()), strip::dev(g, outer.right()), strip::dev(g, outer.bottom()));
    // (A tab's foot: a folder over it ends at `y`.)
    let foot = |r: &Bounds<Pixels>, y: f32| Bounds::from_corners(r.origin, point(r.right(), at(y)));
    for (_, r) in rects.iter().enumerate().filter(|&(i, _)| i != selected) {
        strip::folder(g, foot(r, y0 + hair), f.behind, f.light, f.shadow, f.soft, sh);
    }
    rect(g, (l, y0, rr, b), f.outline);
    rect(g, (l + hair, y0 + hair, rr - hair, b - hair), f.front_shadow);
    rect(g, (l + hair, y0 + hair, rr - hair - bev, b - hair - bev), lit);
    rect(g, (l + hair + bev, y0 + hair + bev, rr - hair - bev, b - hair - bev), f.front);
    if let Some(r) = rects.get(selected) {
        strip::folder(g, foot(r, y0 + hair + bev), f.front, lit, f.front_shadow, f.outline, sh);
        // The panel's light runs into the tab's, either side.
        let (tl, tr) = (strip::dev(g, r.left()), strip::dev(g, r.right()));
        rect(g, (l + hair, y0 + hair, tl + hair, y0 + hair + bev), lit);
        rect(g, (tr - hair, y0 + hair, rr - hair - bev, y0 + hair + bev), lit);
    }
    // (What's in it must fit inside its inset.)
    crate::review::pane(
        g,
        Bounds::from_corners(point(outer.left() + px(m::WINDOW_INSET), outer.top() + px(TAB_HEIGHT)), point(outer.right() - px(m::WINDOW_INSET), outer.bottom() - px(m::WINDOW_INSET))),
    );
    // (The tabs go where their names' lengths put them: lined up with
    // what's under them only by chance.)
    crate::review::own_grid(g, Bounds::from_corners(outer.origin, point(outer.right(), outer.top() + px(TAB_HEIGHT))));
    // Every name centred in its tab's face, the chosen one's in bold.
    let cy = (outer.top() + at(hair + bev) + at(y0)) / 2.0;
    for (i, r) in rects.iter().enumerate() {
        let on = i == selected;
        let label = tabs.get(i).cloned().unwrap_or_default();
        let (font, ink) = if on { (style::ui_font(FontWeight::BOLD), f.front_ink) } else { (style::ui_font(FontWeight::NORMAL), f.ink) };
        let room = r.size.width - px(2.0 * least);
        // (A page's name cut short hides what the page is for: the
        // review hears of it. French's Network tabs were "Ordre des se…"
        // and "Mise en form…", and a label cut with "…" fits.)
        if crate::review::recording(g) && style::text_width(label.clone(), style::PLAIN_SIZE, font.clone()) > room {
            crate::review::cut_name(g, *r, &label);
        }
        style::label_fit(g, r.center().x, cy, style::Align::Center, label.clone(), style::PLAIN_SIZE, font, ink, room);
        g.agent(agent::Node { role: "Tab", name: Some(label), bounds: *r, enabled: true, clickable: true, selected: Some(on), ..Default::default() });
    }
}
