//! Searching in the GPUI kit, as in the egui one: the query language,
//! filters and suggestions (`olo_kit_base::search`, shared by both kits),
//! a search field that colours the filters typed and suggests them as
//! they are ([`SearchField`]), and a bar of filter menus ([`FilterBar`])
//! editing the same text, so what is typed and what is chosen always
//! agree (and an agent can just type a query).

use std::{ops::Range, rc::Rc};

use gpui::{
    App, AppContext as _, Bounds, Context, Corner, Entity, EventEmitter, Font, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, RenderOnce, SharedString, Styled as _, TextRun, Window, canvas, deferred, div, point, px, size,
};
use olo_kit_base::metrics as m;
pub use olo_kit_base::search::*;
use olo_theme::Palette;

use crate::{
    Menu, agent, menu, style,
    style::c,
    theme,
    widgets::{TextField, TextFieldEvent, control, text_field},
};

/// The tint behind what a search found.
pub fn mark_color(p: &Palette) -> Hsla {
    c(olo_theme::mix(p.hues.yellow, p.document_background, 0.55))
}

/// Runs for `text` in `font`: `spans` (byte ranges, in order) in their
/// colours, the rest in `color`, and `marks` (what a search found, as
/// [`found`] gives) with `mark` behind.
pub fn runs(text: &str, font: &Font, color: Hsla, spans: &[(Range<usize>, Hsla)], marks: &[Range<usize>], mark: Hsla) -> Vec<TextRun> {
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for (r, _) in spans {
        cuts.extend([r.start.min(text.len()), r.end.min(text.len())]);
    }
    for r in marks {
        cuts.extend([r.start.min(text.len()), r.end.min(text.len())]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<TextRun> = Vec::with_capacity(cuts.len());
    let mut span = 0;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        while span < spans.len() && spans[span].0.end <= a {
            span += 1;
        }
        let ink = spans.get(span).filter(|(r, _)| r.start <= a).map_or(color, |(_, c)| *c);
        let behind = marks.iter().any(|r| r.start <= a && a < r.end).then_some(mark);
        match out.last_mut() {
            Some(last) if last.color == ink && last.background_color == behind => last.len += b - a,
            _ => out.push(TextRun { len: b - a, font: font.clone(), color: ink, background_color: behind, underline: None, strikethrough: None }),
        }
    }
    if out.is_empty() {
        out.push(TextRun { len: 0, font: font.clone(), color, background_color: None, underline: None, strikethrough: None });
    }
    out
}

/// `text` in `font` and `color`, what `query`'s words find marked: for a
/// list's cell or a label.
pub fn highlight(text: &str, query: &Query, font: &Font, color: Hsla, mark: Hsla) -> Vec<TextRun> {
    runs(text, font, color, &[], &found(text, query), mark)
}

// ---- The search field -----------------------------------------------------

/// What a [`SearchField`] says.
#[derive(Clone, Debug)]
pub enum SearchEvent {
    /// Its text changed (typed, a suggestion taken, cleared).
    Changed,
    /// Return, with no suggestion chosen.
    Entered,
}

/// A search field: a magnifier, the query (filters coloured, ones left
/// out in red), a clear button, and while typing a list under it of the
/// filters and values that fit ([`suggest`]): ↑ and ↓ choose, Tab or
/// Return takes one (Tab the first), Escape closes the list, then clears,
/// then lets go. Made with [`search_field`].
pub struct SearchField {
    field: Entity<TextField>,
    facets: Rc<Vec<Facet>>,
    /// The suggestion chosen with the arrows.
    chosen: Option<usize>,
    /// The suggestion under the pointer.
    hover: Option<usize>,
    /// Escape closed the list for this text.
    closed_for: Option<String>,
}

impl EventEmitter<SearchEvent> for SearchField {}

/// A search field, hinting `hint` while it's empty ("Search").
pub fn search_field(cx: &mut App, hint: impl Into<SharedString>) -> Entity<SearchField> {
    let hint = hint.into();
    let field = text_field(cx, |f| f.name(hint.clone()).hint(hint));
    cx.new(|cx| {
        cx.subscribe(&field, |s: &mut SearchField, _, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                s.chosen = None;
                cx.emit(SearchEvent::Changed);
                cx.notify();
            }
            TextFieldEvent::Entered => cx.emit(SearchEvent::Entered),
            TextFieldEvent::Typed(_) => {}
        })
        .detach();
        // The caret moving changes what's suggested.
        cx.observe(&field, |_, _, cx| cx.notify()).detach();
        SearchField { field, facets: Rc::default(), chosen: None, hover: None, closed_for: None }
    })
}

impl SearchField {
    pub fn text<'a>(&self, cx: &'a App) -> &'a str {
        self.field.read(cx).text()
    }

    /// Its text set (a filter chosen from a bar): no event.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.text(cx) {
            self.field.update(cx, |f, cx| f.set_text(text, cx));
            self.chosen = None;
            cx.notify();
        }
    }

    /// The filters it colours and suggests.
    pub fn set_facets(&mut self, facets: Rc<Vec<Facet>>, cx: &mut Context<Self>) {
        let keys: Vec<String> = keys(&facets).iter().map(|k| k.to_string()).collect();
        self.field.update(cx, |f, cx| {
            f.set_colors(
                Some(Rc::new(move |text: &str, p: &Palette| {
                    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                    coloured(text, &keys, p)
                })),
                cx,
            )
        });
        self.facets = facets;
        cx.notify();
    }

    /// The keyboard to it, its text selected (Find, ⌘F).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.field.update(cx, |f, cx| {
            f.select_all_text(cx);
            f.focus(window);
        });
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.field.read(cx).is_focused(window)
    }

    /// What to offer now.
    fn suggestions(&self, window: &Window, cx: &App) -> Vec<Suggestion> {
        let f = self.field.read(cx);
        let s = f.selection();
        if !f.is_focused(window) || s.start != s.end || self.closed_for.as_deref() == Some(f.text()) {
            return Vec::new();
        }
        suggest(f.text(), s.end, &self.facets)
    }

    /// Takes suggestion `s`: its text in, the caret after it.
    fn take(&mut self, s: &Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = self.text(cx).to_owned();
        if s.replace.end > text.len() {
            return;
        }
        text.replace_range(s.replace.clone(), &s.insert);
        let at = (s.replace.start + s.insert.len()).min(text.len());
        self.field.update(cx, |f, cx| {
            f.set_text(text, cx);
            f.set_selection(at..at, cx);
            f.focus(window);
        });
        self.chosen = None;
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.set_text("", cx));
        self.chosen = None;
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    /// Keys for the list (before the field sees them); whether taken.
    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if e.keystroke.modifiers.modified() {
            return false;
        }
        let list = self.suggestions(window, cx);
        let key = e.keystroke.key.as_str();
        if list.is_empty() {
            if key != "escape" {
                return false;
            }
            if self.text(cx).is_empty() {
                window.blur();
            } else {
                self.clear(cx);
            }
            return true;
        }
        match key {
            "down" => self.chosen = Some(self.chosen.map_or(0, |k| (k + 1).min(list.len() - 1))),
            "up" => self.chosen = self.chosen.and_then(|k| k.checked_sub(1)),
            "tab" | "enter" if key == "tab" || self.chosen.is_some() => {
                let s = list[self.chosen.unwrap_or(0)].clone();
                self.take(&s, window, cx);
                return true;
            }
            "escape" => self.closed_for = Some(self.text(cx).to_owned()),
            _ => return false,
        }
        cx.notify();
        true
    }
}

/// The colours of a query's text: filters' keys in blue, what is left
/// out in red.
fn coloured(text: &str, keys: &[&str], p: &Palette) -> Vec<(Range<usize>, Hsla)> {
    let (key, not) = (c(p.hues.blue), c(p.failure));
    let mut out = Vec::new();
    for t in tokens(text, keys) {
        let mut j = t.range.start;
        if t.not {
            out.push((j..j + 1, not));
            j += 1;
        }
        if let Some((_, kr)) = &t.key {
            let end = (kr.end + 1).min(t.range.end);
            out.push((j..end, key));
            j = end;
        }
        if t.not && j < t.range.end {
            out.push((j..t.range.end, not));
        }
    }
    out
}

impl Render for SearchField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let focused = self.is_focused(window, cx);
        let has_text = !self.text(cx).is_empty();
        let list = self.suggestions(window, cx);
        self.chosen = self.chosen.filter(|&k| k < list.len());
        self.hover = self.hover.filter(|&k| k < list.len());

        // The well, the magnifier and the clear button.
        let well_me = me.clone();
        let well = canvas(
            |b, window, _| window.insert_hitbox(b, gpui::HitboxBehavior::Normal),
            move |b, hitbox, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, style::inset(window, b, 2.0), c(p.document_background));
                style::sunken(window, b, &p, focused);
                let dim = style::disabled_text(&p);
                let cy = style::snap_coord(window, px(f32::from(b.center().y).round()));
                let glass = point(style::snap_coord(window, b.left() + px(m::TEXT_INSET + 5.0)), cy - px(1.0));
                let ring = Bounds { origin: point(glass.x - px(4.75), glass.y - px(4.75)), size: size(px(9.5), px(9.5)) };
                window.paint_quad(gpui::quad(ring, px(4.75), gpui::transparent_black(), px(1.5), dim, gpui::BorderStyle::Solid));
                window.paint_segment(glass + point(px(3.0), px(3.0)), glass + point(px(6.5), px(6.5)), px(1.75), true, dim);
                if has_text {
                    let k = clear_bounds(window, b);
                    let over = hitbox.is_hovered(window) && k.contains(&window.mouse_position());
                    let fill = if over { style::shade(&p, p.document_background, 1.45) } else { style::shade(&p, p.document_background, 1.3) };
                    window.paint_quad(gpui::fill(k, fill).corner_radii(px(6.0)));
                    let x = Bounds { origin: point(k.left() + px(3.5), k.top() + px(3.5)), size: size(px(5.0), px(5.0)) };
                    let ink = c(p.document_background);
                    window.paint_segment(x.origin, x.bottom_right(), px(1.25), false, ink);
                    window.paint_segment(point(x.right(), x.top()), point(x.left(), x.bottom()), px(1.25), false, ink);
                    agent::record(window, cx, agent::Node::button("Clear Search", k));
                    let (me, hb) = (well_me.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble() && e.button == MouseButton::Left && hb.is_hovered(window) && k.contains(&e.position) {
                            me.update(cx, |s, cx| {
                                s.clear(cx);
                                s.field.update(cx, |f, _| f.focus(window));
                            });
                            cx.stop_propagation();
                        }
                    });
                }
            },
        )
        .absolute()
        .inset_0();

        // The text, between the magnifier and the clear button, its
        // capitals on the well's middle (the field centres them on its
        // own: as tall as the well, that's on device pixels at any scale,
        // as a menu's beside it is).
        let text = div()
            .absolute()
            .left(px(m::TEXT_INSET + 10.0 + m::TIGHT + 5.0))
            .right(px(if has_text { m::TEXT_INSET + 12.0 + m::TIGHT } else { m::TEXT_INSET }))
            .top_0()
            .h(px(m::CONTROL_HEIGHT))
            .child(self.field.clone());

        let (keys, tab, enter) = (me.clone(), me.clone(), me.clone());
        let mut root = div()
            .relative()
            .w_full()
            .h(px(m::CONTROL_HEIGHT))
            .child(well)
            .child(text)
            .capture_key_down(move |e: &KeyDownEvent, window, cx| {
                if keys.update(cx, |s, cx| s.key(e, window, cx)) {
                    cx.stop_propagation();
                }
            })
            // Tab takes the first suggestion (the one chosen) while they
            // show, rather than the keyboard to the next control.
            .capture_action(move |_: &crate::FocusNext, window, cx| {
                let taken = tab.update(cx, |s, cx| {
                    let list = s.suggestions(window, cx);
                    let Some(sg) = list.get(s.chosen.unwrap_or(0)).cloned() else { return false };
                    s.take(&sg, window, cx);
                    true
                });
                if taken {
                    cx.stop_propagation();
                }
            })
            // Return takes the suggestion chosen with the arrows (else
            // it's the field's).
            .capture_action(move |_: &crate::widgets::text_field::Enter, window, cx| {
                let taken = enter.update(cx, |s, cx| {
                    let list = s.suggestions(window, cx);
                    let Some(sg) = s.chosen.and_then(|k| list.get(k).cloned()) else { return false };
                    s.take(&sg, window, cx);
                    true
                });
                if taken {
                    cx.stop_propagation();
                }
            });

        // The suggestions, over what's below.
        if !list.is_empty() {
            let (chosen, hover) = (self.chosen, self.hover);
            let h = list.len() as f32 * m::LIST_ROW + 2.0 * m::TIGHT;
            let rows_me = me.clone();
            let shown = canvas(
                |b, window, _| window.insert_hitbox(b, gpui::HitboxBehavior::BlockMouse),
                move |frame, hitbox, window, cx| {
                    let p = theme::palette(cx);
                    style::soft_shadow(window, frame, &p);
                    window.paint_quad(gpui::quad(frame, px(3.0), c(p.menu_background), style::hairline(window), style::shade(&p, p.menu_background, 1.4), gpui::BorderStyle::Solid));
                    let row = |k: usize| Bounds { origin: point(frame.left() + px(1.0), frame.top() + px(m::TIGHT + k as f32 * m::LIST_ROW)), size: size(frame.size.width - px(2.0), px(m::LIST_ROW)) };
                    let font = style::ui_font(FontWeight::NORMAL);
                    let dim = style::disabled_text(&p);
                    for (k, s) in list.iter().enumerate() {
                        let rr = row(k);
                        let lit = chosen == Some(k) || hover == Some(k);
                        if lit {
                            style::fill_rect(window, rr, c(p.menu_selected_background));
                        }
                        let ink = if lit { c(p.menu_selected_item_text) } else { c(p.menu_item_text) };
                        let detail = style::label_fit(window, cx, rr.right() - px(m::TEXT_INSET), rr.center().y, style::Align::Right, s.detail.clone(), style::PLAIN_SIZE, font.clone(), if lit { ink } else { dim }, rr.size.width / 2.0);
                        let room = (detail.left() - px(m::SPACING)) - (rr.left() + px(m::TEXT_INSET));
                        style::label_fit(window, cx, rr.left() + px(m::TEXT_INSET), rr.center().y, style::Align::Left, s.label.clone(), style::PLAIN_SIZE, font.clone(), ink, room);
                        let name = format!("{} {}", s.label, s.detail);
                        agent::record(window, cx, agent::Node { role: "MenuItem", name: Some(name.trim().to_owned().into()), bounds: rr, enabled: true, clickable: true, selected: Some(lit), ..Default::default() });
                    }
                    let at = move |y: Pixels| {
                        let k = (f32::from(y - frame.top()) - m::TIGHT) / m::LIST_ROW;
                        (k >= 0.0).then_some(k as usize)
                    };
                    let n = list.len();
                    let (me, hb) = (rows_me.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if !phase.bubble() {
                            return;
                        }
                        let k = if hb.is_hovered(window) { at(e.position.y).filter(|&k| k < n) } else { None };
                        me.update(cx, |s, cx| {
                            if s.hover != k {
                                s.hover = k;
                                cx.notify();
                            }
                        });
                    });
                    let (me, hb) = (rows_me.clone(), hitbox.clone());
                    let taken = list.clone();
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble()
                            && e.button == MouseButton::Left
                            && hb.is_hovered(window)
                            && let Some(s) = at(e.position.y).and_then(|k| taken.get(k).cloned())
                        {
                            me.update(cx, |sf, cx| sf.take(&s, window, cx));
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .w_full()
            .min_w(px(280.0))
            .h(px(h));
            root = root.child(div().absolute().top(px(m::CONTROL_HEIGHT + 2.0)).left_0().w_full().child(deferred(shown).with_priority(1)));
        }
        root
    }
}

/// The clear button's place in the well `b`.
fn clear_bounds(window: &Window, b: Bounds<Pixels>) -> Bounds<Pixels> {
    style::snap_centred(window, point(b.right() - px(m::TEXT_INSET + 6.0), b.center().y), 12.0, 12.0)
}

// ---- The filter bar ------------------------------------------------------

type OnChange = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// A row of filter menus, one for each of `facets` in the bar ("Program:
/// Any ▾"), each choosing its filter's values in `text` (a [`Query`]'s);
/// those with values chosen are tinted and in bold. Clear, at the end,
/// empties the query. `on_change` gets the new text.
#[derive(IntoElement)]
pub struct FilterBar {
    id: SharedString,
    text: String,
    facets: Rc<Vec<Facet>>,
    clear: bool,
    max_width: Option<f32>,
    on_change: Option<OnChange>,
}

impl FilterBar {
    pub fn new(id: impl Into<SharedString>, text: impl Into<String>, facets: Rc<Vec<Facet>>) -> FilterBar {
        FilterBar { id: id.into(), text: text.into(), facets, clear: true, max_width: None, on_change: None }
    }

    /// No Clear button (the app has its own).
    pub fn no_clear(mut self) -> Self {
        self.clear = false;
        self
    }

    /// The room it has: menus that don't fit are left out.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }

    pub fn on_change(mut self, f: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// How wide the bar's menus are for `text`.
    pub fn width(cx: &App, text: &str, facets: &[Facet]) -> f32 {
        let q = Query::parse(text, &keys(facets));
        let mut w = 0.0;
        for f in facets.iter().filter(|f| f.bar) {
            w += pill_width(cx, &pill_text(f, &q), !q.values(&f.key).is_empty()) + m::SPACING;
        }
        w - m::SPACING
    }
}

fn chosen_values(f: &Facet, q: &Query) -> Vec<String> {
    q.values(&f.key).iter().map(|n| f.values.iter().find(|v| n.is(&v.value)).map_or_else(|| n.as_str().to_owned(), |v| v.value.clone())).collect()
}

fn pill_text(f: &Facet, q: &Query) -> String {
    let chosen = chosen_values(f, q);
    let summary = match chosen.as_slice() {
        [] => f.any.clone(),
        [one] => f.label_of(one),
        [first, rest @ ..] => format!("{} +{}", f.label_of(first), rest.len()),
    };
    if f.label.is_empty() { summary } else { format!("{}: {summary}", f.label) }
}

/// How wide a menu is for `text` (in bold when it filters).
fn pill_width(cx: &App, text: &str, bold: bool) -> f32 {
    let font = style::ui_font(if bold { FontWeight::BOLD } else { FontWeight::NORMAL });
    (style::text_width_app(cx, text, style::PLAIN_SIZE, &font) + m::TEXT_INSET + 15.0 + m::TIGHT).ceil()
}

/// A number with thousands set apart ("12,345"), as counts in menus are.
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl RenderOnce for FilterBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let keys_owned: Vec<String> = keys(&self.facets).iter().map(|k| k.to_string()).collect();
        let q = Query::parse(&self.text, &keys(&self.facets));
        let mut row = div().flex().flex_row().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT));
        let mut room = self.max_width.unwrap_or(f32::INFINITY);
        for f in self.facets.iter().filter(|f| f.bar) {
            let chosen = chosen_values(f, &q);
            let label: SharedString = pill_text(f, &q).into();
            let w = pill_width(cx, &label, !chosen.is_empty()).min(room.max(0.0));
            if w < 40.0 {
                break;
            }
            room -= w + m::SPACING;
            let (facet, text, keys, on_change) = (f.clone(), self.text.clone(), keys_owned.clone(), self.on_change.clone());
            let filters = !chosen.is_empty();
            let shown = label.clone();
            let pill = control(SharedString::from(format!("{}-{}", self.id, f.key)), w, m::CONTROL_HEIGHT, move |pill, _, window, cx| {
                // Raised, as a menu field; tinted when it filters.
                let p = theme::palette(cx);
                let base = if filters { olo_theme::mix(p.keyboard_navigation, p.menu_background, 0.82) } else { p.menu_background };
                let inner = style::inset(window, pill, 1.0);
                style::vgradient(window, inner, style::tint(base, 0.82), style::shade(&p, base, 1.05));
                style::frame_rect(window, pill, style::shade(&p, p.panel_background, 1.55));
                style::bevel(window, inner, c(p.shine), style::shade(&p, base, 1.2));
                let ink = c(p.control_text);
                let font = style::ui_font(if filters { FontWeight::BOLD } else { FontWeight::NORMAL });
                // (Its middle on the device grid, as a row beside it puts its own.)
                let cy = style::snap_coord(window, pill.center().y);
                style::label_fit(window, cx, pill.left() + px(m::TEXT_INSET), cy, style::Align::Left, shown.clone(), style::PLAIN_SIZE, font, ink, pill.size.width - px(m::TEXT_INSET + 15.0 + m::TIGHT));
                let ax = pill.right() - px(11.0);
                let ay = px(f32::from(pill.center().y).round());
                let mut path = gpui::PathBuilder::fill();
                path.move_to(point(ax - px(4.0), ay - px(2.0)));
                path.line_to(point(ax + px(4.0), ay - px(2.0)));
                path.line_to(point(ax, ay + px(2.5)));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, ink);
                }
            })
            .agent_value("ComboBox", f.label.clone(), label.clone())
            .on_click(move |pill, window, cx| {
                let q = Query::parse(&text, &keys.iter().map(String::as_str).collect::<Vec<_>>());
                let chosen = chosen_values(&facet, &q);
                let mut menu = Menu::new(facet.label.clone()).item(facet.any.clone(), "any").checked(chosen.is_empty()).separator();
                for v in &facet.values {
                    menu = menu.item(v.label.clone(), format!("v:{}", v.value)).checked(chosen.iter().any(|c| c.eq_ignore_ascii_case(&v.value)));
                    // How many have it, where a shortcut would be.
                    if let Some(n) = v.count {
                        menu = menu.keys(thousands(n));
                    }
                }
                let (facet, text, keys, on_change) = (facet.clone(), text.clone(), keys.clone(), on_change.clone());
                menu::popup(window, cx, menu, pill, Corner::BottomLeft, move |cmd, window, cx| {
                    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                    let mut chosen = chosen_values(&facet, &Query::parse(&text, &keys));
                    if cmd == "any" {
                        chosen.clear();
                    } else if let Some(v) = cmd.strip_prefix("v:") {
                        match chosen.iter().position(|c| c.eq_ignore_ascii_case(v)) {
                            Some(i) if facet.multiple => {
                                chosen.remove(i);
                            }
                            Some(_) => {}
                            None if facet.multiple => chosen.push(v.to_owned()),
                            None => chosen = vec![v.to_owned()],
                        }
                    }
                    let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                    let mut new = text.clone();
                    set_filter(&mut new, &keys, &facet.key, &refs);
                    if let Some(f) = &on_change
                        && new != text
                    {
                        f(new, window, cx);
                    }
                });
            });
            row = row.child(pill);
        }
        if self.clear && !self.text.trim().is_empty() {
            let w = (style::text_width_app(cx, "Clear", style::PLAIN_SIZE, &style::ui_font(FontWeight::NORMAL)) + 2.0 * m::TEXT_INSET).ceil();
            if w <= room {
                let on_change = self.on_change.clone();
                row = row.child(
                    control(SharedString::from(format!("{}-clear", self.id)), w, m::CONTROL_HEIGHT, |r, s, window, cx| {
                        let p = theme::palette(cx);
                        let ink = if s.hovered { c(p.keyboard_navigation) } else { c(p.hues.blue) };
                        style::label(window, cx, r.left() + px(m::TEXT_INSET), r.center().y, style::Align::Left, "Clear", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                    })
                    .pointing()
                    .agent("Button", "Clear", None)
                    .on_click(move |_, window, cx| {
                        if let Some(f) = &on_change {
                            f(String::new(), window, cx);
                        }
                    }),
                );
            }
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_colour_and_mark() {
        let font = style::ui_font(FontWeight::NORMAL);
        let (base, red, mark) = (Hsla::black(), Hsla::red(), Hsla::white());
        let text = "pool: quota 429";
        let r = runs(text, &font, base, &[(0..4, red)], &[2..8], mark);
        let got: Vec<(usize, bool, bool)> = r.iter().map(|r| (r.len, r.color == red, r.background_color.is_some())).collect();
        assert_eq!(got, vec![(2, true, false), (2, true, true), (4, false, true), (7, false, false)]);
        assert_eq!(r.iter().map(|r| r.len).sum::<usize>(), text.len());
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }
}
