//! Searching, the same in every app: the query language, filters and
//! suggestions (`olo_kit_base::search`),
//! a search field that colours the filters typed and suggests them as
//! they are ([`SearchField`]), and a bar of filter menus ([`FilterBar`])
//! editing the same text, so what is typed and what is chosen always
//! agree (and an agent can just type a query).

use std::{ops::Range, rc::Rc};

use olo_interface::{
    BorderStyle, Bounds, Child, Corner, Cx, Element, Font, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Painter,
    ParentElement as _, PathBuilder, Pixels, Point, SharedString, Styled as _, TextRun, View, ViewRef, WindowCx, canvas, div, point, px, size,
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
            _ => out.push(TextRun { len: b - a, font: font.clone(), color: ink, background_color: behind, underline: None, strikethrough: None, size: None }),
        }
    }
    if out.is_empty() {
        out.push(TextRun { len: 0, font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None });
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
/// then lets go. Made with [`search_field`]; its list is drawn over what's
/// below it (`InteractiveElement::raised`).
pub struct SearchField {
    field: Child<TextField>,
    facets: Rc<Vec<Facet>>,
    /// The suggestion chosen with the arrows.
    chosen: Option<usize>,
    /// The suggestion under the pointer.
    hover: Option<usize>,
    /// Escape closed the list for this text.
    closed_for: Option<String>,
    /// What's offered now, as last shown.
    list: Vec<Suggestion>,
    /// The pointer is over the clear button.
    over_clear: bool,
    focused: bool,
}

/// What a search field is told (by what's in it).
pub enum SearchMsg {
    Hover(Option<usize>),
    OverClear(bool),
    Focus(bool),
}

/// A search field, hinting `hint` while it's empty ("Search").
pub fn search_field<V: View>(cx: &mut Cx<V>, hint: impl Into<SharedString>) -> Child<SearchField> {
    let hint = hint.into();
    cx.new_view(|cx| {
        let field = text_field(cx, |f| f.name(hint.clone()).hint(hint));
        cx.subscribe(&field, |s: &mut SearchField, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                s.chosen = None;
                cx.emit(SearchEvent::Changed);
                cx.invalidate();
            }
            TextFieldEvent::Entered => cx.emit(SearchEvent::Entered),
            TextFieldEvent::Typed(_) => {}
        });
        SearchField { field, facets: Rc::default(), chosen: None, hover: None, closed_for: None, list: Vec::new(), over_clear: false, focused: false }
    })
}

impl SearchField {
    pub fn text<'a>(&self, cx: &'a WindowCx) -> &'a str {
        self.field.read(cx).text()
    }

    /// Its text chosen (a filter from a bar, [`FilterBar::drives`]): set,
    /// and said as typing it would be (`SearchEvent::Changed`), so one
    /// subscription follows both.
    pub fn choose(&mut self, text: impl Into<String>, cx: &mut Cx<Self>) {
        let text = text.into();
        if text != self.text(cx) {
            self.set_text(text, cx);
            cx.emit(SearchEvent::Changed);
        }
    }

    /// Its text set (a filter chosen from a bar): no event.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Cx<Self>) {
        let text = text.into();
        if text != self.text(cx) {
            self.field.update(cx, |f, cx| f.set_text(text, cx));
            self.chosen = None;
            cx.invalidate();
        }
    }

    /// The filters it colours and suggests. (Only what they are matters
    /// to it, not how many have each, which changes with every new list:
    /// for counts alone it isn't drawn again.)
    pub fn set_facets(&mut self, facets: Rc<Vec<Facet>>, cx: &mut Cx<Self>) {
        let alike = |a: &Facet, b: &Facet| {
            a.key == b.key
                && a.label == b.label
                && a.any == b.any
                && a.multiple == b.multiple
                && a.bar == b.bar
                && a.values.len() == b.values.len()
                && a.values.iter().zip(&b.values).all(|(x, y)| x.value == y.value && x.label == y.label)
        };
        if facets.len() == self.facets.len() && facets.iter().zip(self.facets.iter()).all(|(a, b)| alike(a, b)) {
            self.facets = facets;
            return;
        }
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
        cx.invalidate();
    }

    /// The keyboard to it, its text selected (Find, ⌘F).
    pub fn focus(&self, cx: &mut WindowCx) {
        self.field.update(cx, |f, cx| f.select_all_text(cx));
        self.field.focus(cx);
    }

    pub fn is_focused(&self, cx: &WindowCx) -> bool {
        self.field.is_focused(cx)
    }

    /// Whether it's offering suggestions now (its list showing, which
    /// ↑ ↓ move in): else an app may take them (Passwords' list).
    pub fn suggesting(&self, cx: &WindowCx) -> bool {
        !self.suggestions(cx).is_empty()
    }

    /// What to offer now.
    fn suggestions(&self, cx: &WindowCx) -> Vec<Suggestion> {
        let f = self.field.read(cx);
        let s = f.selection();
        if !self.field.is_focused(cx) || s.start != s.end || self.closed_for.as_deref() == Some(f.text()) {
            return Vec::new();
        }
        suggest(f.text(), s.end, &self.facets)
    }

    /// Takes suggestion `s`: its text in, the caret after it.
    fn take(&mut self, s: &Suggestion, cx: &mut Cx<Self>) {
        let mut text = self.text(cx).to_owned();
        if s.replace.end > text.len() {
            return;
        }
        text.replace_range(s.replace.clone(), &s.insert);
        let at = (s.replace.start + s.insert.len()).min(text.len());
        self.field.update(cx, |f, cx| {
            f.set_text(text, cx);
            f.set_selection(at..at, cx);
        });
        self.field.focus(cx);
        self.chosen = None;
        cx.emit(SearchEvent::Changed);
        cx.invalidate();
    }

    fn clear(&mut self, cx: &mut Cx<Self>) {
        self.field.update(cx, |f, cx| f.set_text("", cx));
        self.chosen = None;
        cx.emit(SearchEvent::Changed);
        cx.invalidate();
    }

    /// Keys for the list (before the field sees them); whether taken.
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) -> bool {
        if e.keystroke.modifiers.modified() {
            return false;
        }
        let list = self.suggestions(cx);
        let key = e.keystroke.key.as_str();
        if list.is_empty() {
            if key != "escape" {
                return false;
            }
            if self.text(cx).is_empty() {
                cx.blur();
            } else {
                self.clear(cx);
            }
            return true;
        }
        match key {
            "down" => self.chosen = Some(self.chosen.map_or(0, |k| (k + 1).min(list.len() - 1))),
            "up" => self.chosen = self.chosen.and_then(|k| k.checked_sub(1)),
            // Tab takes the first suggestion (the one chosen) while they
            // show, rather than the keyboard to the next control; Return
            // the one chosen with the arrows (else it's the field's).
            "tab" | "enter" if key == "tab" || self.chosen.is_some() => {
                let s = list[self.chosen.unwrap_or(0)].clone();
                self.take(&s, cx);
                return true;
            }
            "escape" => self.closed_for = Some(self.text(cx).to_owned()),
            _ => return false,
        }
        cx.invalidate();
        true
    }

    fn paint_well(&self, g: &mut Painter, b: Bounds<Pixels>, has_text: bool) {
        let p = theme::palette();
        style::fill_rect(g, style::inset(g, b, 2.0), c(p.document_background));
        style::sunken(g, b, &p, self.focused);
        let dim = style::disabled_text(&p);
        let cy = style::snap_coord(g, px(f32::from(b.center().y).round()));
        let glass = point(style::snap_coord(g, b.left() + px(m::TEXT_INSET + 5.0)), cy - px(1.0));
        let ring = Bounds { origin: point(glass.x - px(4.75), glass.y - px(4.75)), size: size(px(9.5), px(9.5)) };
        g.paint_quad(olo_interface::quad(ring, px(4.75), olo_interface::transparent_black(), px(1.5), dim, BorderStyle::Solid));
        g.paint_segment(glass + point(px(3.0), px(3.0)), glass + point(px(6.5), px(6.5)), px(1.75), true, dim);
        if has_text {
            let k = clear_bounds(g.scale_factor(), b);
            let fill = if self.over_clear { style::shade(&p, p.document_background, 1.45) } else { style::shade(&p, p.document_background, 1.3) };
            g.paint_quad(olo_interface::fill(k, fill).corner_radii(px(6.0)));
            let x = Bounds { origin: point(k.left() + px(3.5), k.top() + px(3.5)), size: size(px(5.0), px(5.0)) };
            let ink = c(p.document_background);
            g.paint_segment(x.origin, x.bottom_right(), px(1.25), false, ink);
            g.paint_segment(point(x.right(), x.top()), point(x.left(), x.bottom()), px(1.25), false, ink);
            agent::record(g, agent::Node::button("Clear Search", k));
        }
    }

    fn paint_list(&self, g: &mut Painter, frame: Bounds<Pixels>) {
        let p = theme::palette();
        style::soft_shadow(g, frame, &p);
        g.paint_quad(olo_interface::quad(frame, px(3.0), c(p.menu_background), style::hairline(g), style::shade(&p, p.menu_background, 1.4), BorderStyle::Solid));
        let row = |k: usize| Bounds { origin: point(frame.left() + px(1.0), frame.top() + px(m::TIGHT + k as f32 * m::LIST_ROW)), size: size(frame.size.width - px(2.0), px(m::LIST_ROW)) };
        let font = style::ui_font(FontWeight::NORMAL);
        let dim = style::disabled_text(&p);
        for (k, s) in self.list.iter().enumerate() {
            let rr = row(k);
            let lit = self.chosen == Some(k) || self.hover == Some(k);
            if lit {
                style::fill_rect(g, rr, c(p.menu_selected_background));
            }
            let ink = if lit { c(p.menu_selected_item_text) } else { c(p.menu_item_text) };
            let detail = style::label_fit(
                g,
                rr.right() - px(m::TEXT_INSET),
                rr.center().y,
                style::Align::Right,
                s.detail.clone(),
                style::PLAIN_SIZE,
                font.clone(),
                if lit { ink } else { dim },
                rr.size.width / 2.0,
            );
            let room = (detail.left() - px(m::SPACING)) - (rr.left() + px(m::TEXT_INSET));
            style::label_fit(g, rr.left() + px(m::TEXT_INSET), rr.center().y, style::Align::Left, s.label.clone(), style::PLAIN_SIZE, font.clone(), ink, room);
            let name = format!("{} {}", s.label, s.detail);
            agent::record(g, agent::Node { role: "MenuItem", name: Some(name.trim().to_owned().into()), bounds: rr, enabled: true, clickable: true, selected: Some(lit), ..Default::default() });
        }
    }
}

/// The suggestion's row at `y` in a list drawn at `frame`.
fn row_at(frame: Bounds<Pixels>, y: Pixels) -> Option<usize> {
    let k = (f32::from(y - frame.top()) - m::TIGHT) / m::LIST_ROW;
    (k >= 0.0).then_some(k as usize)
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

impl View for SearchField {
    type Message = SearchMsg;

    fn message(&mut self, message: SearchMsg, cx: &mut Cx<Self>) {
        match message {
            SearchMsg::Hover(k) => {
                if self.hover == k {
                    return;
                }
                self.hover = k;
            }
            SearchMsg::OverClear(over) => {
                if self.over_clear == over {
                    return;
                }
                self.over_clear = over;
            }
            SearchMsg::Focus(_) => {}
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.focused = self.is_focused(cx);
        let has_text = !self.text(cx).is_empty();
        self.list = self.suggestions(cx);
        self.chosen = self.chosen.filter(|&k| k < self.list.len());
        self.hover = self.hover.filter(|&k| k < self.list.len());

        // The well, the magnifier and the clear button.
        let well = cx
            .canvas(move |s: &SearchField, g, b| s.paint_well(g, b, has_text))
            .id("search-well")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .on_mouse_move(cx.listener(move |_: &mut Self, e: &MouseMoveEvent, cx| {
                let over = has_text && clear_bounds(cx.scale_factor(), cx.listener_bounds()).contains(&e.position);
                cx.send(SearchMsg::OverClear(over));
            }))
            .on_hover(cx.listener(|_: &mut Self, over: &bool, cx| {
                if !over {
                    cx.send(SearchMsg::OverClear(false));
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s: &mut Self, e: &MouseDownEvent, cx| {
                    if has_text && clear_bounds(cx.scale_factor(), cx.listener_bounds()).contains(&e.position) {
                        s.clear(cx);
                        s.field.focus(cx);
                        cx.stop_propagation();
                    }
                }),
            );

        // The text, between the magnifier and the clear button, its
        // capitals on the well's middle (the field centres them on its
        // own: as tall as the well, that's on device pixels at any scale,
        // as a menu's beside it is).
        let text = div()
            .absolute()
            .left(px(m::TEXT_INSET + 10.0 + m::TIGHT + 5.0))
            .right(px(if has_text { m::TEXT_INSET + 12.0 + m::TIGHT } else { m::TEXT_INSET }))
            .top(px(0.0))
            .h(px(m::CONTROL_HEIGHT))
            .on_focus_changed(cx.msg_with(|on: &bool| SearchMsg::Focus(*on)))
            .child(&self.field);

        let mut root = div().relative().w_full().h(px(m::CONTROL_HEIGHT)).child(well).child(text).capture_key_down(cx.listener(|s: &mut Self, e: &KeyDownEvent, cx| {
            if s.key(e, cx) {
                cx.stop_propagation();
            }
        }));

        // The suggestions, over what's below.
        if !self.list.is_empty() {
            let h = self.list.len() as f32 * m::LIST_ROW + 2.0 * m::TIGHT;
            let n = self.list.len();
            let shown = cx
                .canvas(|s: &SearchField, g, frame| s.paint_list(g, frame))
                .id("search-suggestions")
                .raised()
                .occlude()
                .w_full()
                .min_w(px(280.0))
                .h(px(h))
                .on_mouse_move(cx.listener(move |_: &mut Self, e: &MouseMoveEvent, cx| {
                    let k = row_at(cx.listener_bounds(), e.position.y).filter(|&k| k < n);
                    cx.send(SearchMsg::Hover(k));
                }))
                .on_hover(cx.listener(|_: &mut Self, over: &bool, cx| {
                    if !over {
                        cx.send(SearchMsg::Hover(None));
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    // (At once: the click takes the keyboard from the
                    // field, and the list is gone by the next turn.)
                    cx.listener(move |s: &mut Self, e: &MouseDownEvent, cx| {
                        if let Some(sg) = row_at(cx.listener_bounds(), e.position.y).filter(|&k| k < n).and_then(|k| s.list.get(k).cloned()) {
                            s.take(&sg, cx);
                            cx.stop_propagation();
                        }
                    }),
                );
            root = root.child(div().absolute().top(px(m::CONTROL_HEIGHT + 2.0)).left(px(0.0)).w_full().child(shown));
        }
        root
    }
}

/// The clear button's place in the well `b` (as `style::snap_centred`).
fn clear_bounds(scale: f32, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let center: Point<Pixels> = point(b.right() - px(m::TEXT_INSET + 6.0), b.center().y);
    let d = (12.0 * scale).round();
    let min = point(px(((f32::from(center.x) * scale - d / 2.0).round()) / scale), px(((f32::from(center.y) * scale - d / 2.0).round()) / scale));
    Bounds { origin: min, size: size(px(d / scale), px(d / scale)) }
}

// ---- The filter bar ------------------------------------------------------

type OnChange = Rc<dyn Fn(&String, &mut WindowCx)>;

/// A row of filter menus, one for each of `facets` in the bar ("Program:
/// Any ▾"), each choosing its filter's values in `text` (a [`Query`]'s);
/// those with values chosen are tinted and in bold. Clear, at the end,
/// empties the query. `on_change` gets the new text.
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

    pub fn on_change(mut self, f: impl Fn(&String, &mut WindowCx) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// What its menus choose goes into `field` (`SearchField::choose`):
    /// the field says it changed, as typing does, so the app follows the
    /// field alone.
    pub fn drives(self, field: &Child<SearchField>) -> Self {
        let field: ViewRef<SearchField> = field.downgrade();
        self.on_change(move |text, cx| {
            let text = text.clone();
            field.update(cx, move |s, cx| s.choose(text, cx));
        })
    }

    /// How wide the bar's menus are for `text`.
    pub fn width(text: &str, facets: &[Facet]) -> f32 {
        let q = Query::parse(text, &keys(facets));
        let mut w = 0.0;
        for f in facets.iter().filter(|f| f.bar) {
            w += pill_width(&pill_text(f, &q), !q.values(&f.key).is_empty()) + m::SPACING;
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

/// How wide a menu is for `text` (in bold when it filters), about.
fn pill_width(text: &str, bold: bool) -> f32 {
    let font = style::ui_font(if bold { FontWeight::BOLD } else { FontWeight::NORMAL });
    pill_around(style::text_width_app(text, style::PLAIN_SIZE, &font))
}

/// How wide it is drawn: its text as shaped (kerned, for the display's
/// scale), so it's never cut short.
fn pill_width_shaped(text: &SharedString, bold: bool) -> f32 {
    let font = style::ui_font(if bold { FontWeight::BOLD } else { FontWeight::NORMAL });
    pill_around(f32::from(style::text_width(text.clone(), style::PLAIN_SIZE, font)))
}

fn pill_around(text: f32) -> f32 {
    // (And a pixel more: laid out on device pixels, its edges can each
    // move a fraction of one, taking it from the text's room.)
    (text + m::TEXT_INSET + 15.0 + m::TIGHT + 1.0).ceil()
}

/// A search and its filters' menus over a list, as the kit's apps have
/// them ([`header`]): the field and the menus in one row when the field
/// keeps `min_field` points beside them, else the menus under the field,
/// their first menu's text `indent` in (over the list's first column's);
/// and what the search keeps (`count`, "12 of 340") at the end of the
/// field's row. The menus write the field's text ([`FilterBar::drives`]).
pub struct Header {
    field: Element,
    drives: ViewRef<SearchField>,
    text: String,
    id: SharedString,
    facets: Rc<Vec<Facet>>,
    width: f32,
    count: Option<SharedString>,
    clear: bool,
    min_field: f32,
    indent: f32,
}

/// Where a [`Header`]'s menus start when under its field: their first
/// menu's text over a list's first column's (inside its two lines of
/// frame), as Log Viewer and Replay have them.
pub fn list_indent(cx: &WindowCx) -> f32 {
    let scale = cx.scale_factor();
    let frame = 2.0 * style::line_pixels(scale) / scale;
    frame + m::CELL_INSET - m::TEXT_INSET
}

/// [`Header`] for `field`, its menus `facets`, in `width` points.
pub fn header(cx: &WindowCx, field: &Child<SearchField>, id: impl Into<SharedString>, facets: Rc<Vec<Facet>>, width: f32) -> Header {
    Header { field: field.element(), drives: field.downgrade(), text: field.read(cx).text(cx).to_owned(), id: id.into(), facets, width, count: None, clear: true, min_field: 240.0, indent: 0.0 }
}

impl Header {
    /// What the search keeps, at the end of the field's row.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// No Clear menu (the app has its own).
    pub fn no_clear(mut self) -> Self {
        self.clear = false;
        self
    }

    /// The least the field keeps beside the menus in one row.
    pub fn min_field(mut self, w: f32) -> Self {
        self.min_field = w;
        self
    }

    /// The menus' indent when under the field.
    pub fn indent(mut self, x: f32) -> Self {
        self.indent = x;
        self
    }
}

impl IntoElement for Header {
    fn into_element(self) -> Element {
        let font = style::ui_font(FontWeight::NORMAL);
        let count_w = self.count.as_ref().map_or(0.0, |c| style::text_width_app(c, style::PLAIN_SIZE, &font).ceil() + m::SPACING);
        let bar_w = FilterBar::width(&self.text, &self.facets) + m::SPACING + 40.0;
        let one_row = self.width - bar_w - count_w - m::SPACING >= self.min_field;
        let drives = self.drives;
        let mut bar = FilterBar::new(self.id, self.text, self.facets).on_change(move |text, cx| {
            let text = text.clone();
            drives.update(cx, move |s, cx| s.choose(text, cx));
        });
        if !self.clear {
            bar = bar.no_clear();
        }
        let count = self.count.map(|c| {
            let look = style::key(&c);
            canvas(move |g, b| {
                let p = theme::palette();
                let r = style::label(g, b.right(), b.center().y, style::Align::Right, c.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                agent::record(g, agent::Node::label(c.clone(), r));
            })
            .paint_key(look)
            .w(px((count_w - m::SPACING).max(0.0)))
            .h(px(m::CONTROL_HEIGHT))
            .flex_shrink_0()
        });
        let field_w = if one_row { self.width - bar_w - count_w - m::SPACING } else { self.width - count_w };
        let top = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().w(px(field_w.max(1.0))).child(self.field));
        if one_row {
            top.child(div().flex_1().min_w_0().child(bar.max_width(bar_w))).children(count)
        } else {
            div().flex().flex_col().gap(px(m::SPACING)).child(top.children(count)).child(div().pl(px(self.indent)).child(bar.max_width(self.width - self.indent)))
        }
    }
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

impl IntoElement for FilterBar {
    fn into_element(self) -> Element {
        let keys_owned: Vec<String> = keys(&self.facets).iter().map(|k| k.to_string()).collect();
        let q = Query::parse(&self.text, &keys(&self.facets));
        let mut row = div().flex().flex_row().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT));
        let mut room = self.max_width.unwrap_or(f32::INFINITY);
        for f in self.facets.iter().filter(|f| f.bar) {
            let chosen = chosen_values(f, &q);
            let label: SharedString = pill_text(f, &q).into();
            let w = pill_width_shaped(&label, !chosen.is_empty()).min(room.max(0.0));
            if w < 40.0 {
                break;
            }
            room -= w + m::SPACING;
            let (facet, text, keys, on_change) = (f.clone(), self.text.clone(), keys_owned.clone(), self.on_change.clone());
            let filters = !chosen.is_empty();
            let shown = label.clone();
            let pill = control(SharedString::from(format!("{}-{}", self.id, f.key)), w, m::CONTROL_HEIGHT, move |g, pill, _| {
                // Raised, as a menu field; tinted when it filters.
                let p = theme::palette();
                let base = if filters { olo_theme::mix(p.keyboard_navigation, p.menu_background, 0.82) } else { p.menu_background };
                let inner = style::inset(g, pill, 1.0);
                style::vgradient(g, inner, style::tint(base, 0.82), style::shade(&p, base, 1.05));
                style::frame_rect(g, pill, style::shade(&p, p.panel_background, 1.55));
                style::bevel(g, inner, c(p.shine), style::shade(&p, base, 1.2));
                let ink = c(p.control_text);
                let font = style::ui_font(if filters { FontWeight::BOLD } else { FontWeight::NORMAL });
                // (Its middle on the device grid, as a row beside it puts its own.)
                let cy = style::snap_coord(g, pill.center().y);
                style::label_fit(g, pill.left() + px(m::TEXT_INSET), cy, style::Align::Left, shown.clone(), style::PLAIN_SIZE, font, ink, pill.size.width - px(m::TEXT_INSET + 15.0 + m::TIGHT));
                let ax = pill.right() - px(11.0);
                let ay = px(f32::from(pill.center().y).round());
                let mut path = PathBuilder::fill();
                path.move_to(point(ax - px(4.0), ay - px(2.0)));
                path.line_to(point(ax + px(4.0), ay - px(2.0)));
                path.line_to(point(ax, ay + px(2.5)));
                path.close();
                if let Ok(path) = path.build() {
                    g.paint_path(path, ink);
                }
            })
            .look((label.clone(), filters))
            .agent_value("ComboBox", f.label.clone(), label.clone())
            .on_click(move |_, cx| {
                let pill = cx.listener_bounds();
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
                menu::popup(cx, menu, pill, Corner::BottomLeft, move |cmd, cx| {
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
                        f(&new, cx);
                    }
                });
            });
            row = row.child(pill);
        }
        if self.clear && !self.text.trim().is_empty() {
            let w = (f32::from(style::text_width("Clear", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 2.0 * m::TEXT_INSET).ceil();
            if w <= room {
                let on_change = self.on_change.clone();
                row = row.child(
                    control(SharedString::from(format!("{}-clear", self.id)), w, m::CONTROL_HEIGHT, |g, r, s| {
                        let p = theme::palette();
                        let ink = if s.hovered { c(p.keyboard_navigation) } else { c(p.hues.blue) };
                        style::label(g, r.left() + px(m::TEXT_INSET), r.center().y, style::Align::Left, "Clear", style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
                    })
                    .pointing()
                    .agent("Button", "Clear", None)
                    .on_click(move |_, cx| {
                        if let Some(f) = &on_change {
                            f(&String::new(), cx);
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
