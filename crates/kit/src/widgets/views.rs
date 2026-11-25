//! Text and grouping: BStringView (a line of text, or a paragraph) and
//! BBox (an etched frame with its label set into the top edge).

use olo_interface::{AnyElement, Bounds, ClickEvent, Element, FontWeight, Hsla, IntoElement, Painter, ParentElement, Pixels, SharedString, Styled as _, WindowCx, canvas, div, point, px};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use crate::{agent, style, theme};

/// BStringView: a line of plain text, its capitals centred in a control's
/// height (so it lines up with the controls beside it).
pub struct StringView {
    text: SharedString,
    dim: bool,
    bold: bool,
    mono: bool,
    color: Option<Hsla>,
    height: f32,
    wrap: bool,
    cut: bool,
    cut_words: bool,
    size: f32,
    /// The size of the text it sits beside, whose baseline it's drawn on.
    beside: Option<f32>,
    /// On a paragraph's line this tall ([`StringView::line`]).
    line: Option<f32>,
}

impl StringView {
    pub fn new(text: impl Into<SharedString>) -> StringView {
        StringView {
            text: crate::words::of(text),
            dim: false,
            bold: false,
            mono: false,
            color: None,
            height: m::CONTROL_HEIGHT,
            wrap: false,
            cut: false,
            cut_words: false,
            size: style::PLAIN_SIZE,
            beside: None,
            line: None,
        }
    }

    /// On the baseline of a line of `size` points centred in the same
    /// height (the text beside it in its row, in another size: a table's
    /// small column names on its heading's line). Centred itself, smaller
    /// text sits higher than the larger beside it (a point at 13 against
    /// 11: the layout review's "near-level").
    pub fn beside(mut self, size: f32) -> Self {
        self.beside = Some(size);
        self
    }

    /// `points` tall, its words where a paragraph's line that tall has
    /// them (`style::paragraph_baseline`): a label beside a paragraph (a
    /// value that may wrap, set `points` lower than its row's top as the
    /// label is), on its first line at every scale. Centred, About Olo's
    /// labels stood a point below their values at 1× and 2× (level at
    /// 1.5×). [`LinkButton::line`](super::LinkButton::line) for a link.
    pub fn line(mut self, points: f32) -> Self {
        self.line = Some(points);
        self.height = points;
        self
    }

    /// In the small size (a table's column names, a scale's ends).
    pub fn small(mut self, on: bool) -> Self {
        self.size = if on { style::SMALL_SIZE } else { style::PLAIN_SIZE };
        self
    }

    /// At `points` (a full-screen view's title, `style::TITLE_SIZE`); a
    /// line of it is centred in a row of its own height
    /// (`style::line_row`) unless [`Self::height`] says another.
    pub fn size(mut self, points: f32) -> Self {
        self.size = points;
        self.height = self.height.max(style::line_row(points));
        self
    }

    /// A paragraph: wrapped to the width it's given, as tall as it takes.
    pub fn wrap(mut self, on: bool) -> Self {
        self.wrap = on;
        self
    }

    /// One line as wide as it's given (not as its text), cut short with
    /// "…" when that's too narrow, its whole in a tip (and to agents): a
    /// path that changes as work goes on, which mustn't make its window
    /// taller or wider.
    pub fn cut(mut self, on: bool) -> Self {
        self.cut = on;
        self
    }

    /// [`Self::cut`] for a sentence (a reason, a status): cut where a
    /// word ends, never inside one, nor leaving a bracket open
    /// (`style::fit_words`).
    pub fn cut_words(mut self, on: bool) -> Self {
        self.cut = on;
        self.cut_words = on;
        self
    }

    /// The row it's centred in (a control's height unless said: a
    /// note's `LABEL_ROW`).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Secondary text (status, hints), in the disabled text's colour.
    pub fn dim(mut self, on: bool) -> Self {
        self.dim = on;
        self
    }

    pub fn bold(mut self, on: bool) -> Self {
        self.bold = on;
        self
    }

    /// In the fixed-width font: what is typed or run as it stands (a
    /// command, a path, a key's fingerprint), told from the words about
    /// it. Give it `style::MONO_SIZE` beside plain text, or the size of
    /// the small text it is among.
    pub fn mono(mut self, on: bool) -> Self {
        self.mono = on;
        self
    }

    /// A colour of its own (from the palette: an error in `failure`).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl IntoElement for StringView {
    fn into_element(self) -> Element {
        let weight = if self.bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        let (mono, font) = (self.mono, if self.mono { style::mono_font(weight) } else { style::ui_font(weight) });
        if self.wrap {
            let p = theme::palette();
            let ink = self.color.unwrap_or_else(|| if self.dim { style::secondary_text(&p) } else { style::c(p.panel_text) });
            let mut d = div().w_full().line_height(olo_interface::relative(style::LINE_HEIGHT)).font_weight(weight).text_color(ink);
            // (The window's own size unless small: nothing else changes.)
            if self.size != style::PLAIN_SIZE {
                d = d.text_size(px(self.size));
            }
            if mono {
                d = d.font_family(style::MONO_FONT);
            }
            return d.child(self.text);
        }
        let size = self.size;
        if self.cut {
            let (text, dim, color, beside, words) = (self.text, self.dim, self.color, self.beside, self.cut_words);
            let key = style::key((&text, dim, style::ink_key(color), size.to_bits(), self.bold, mono, beside.map(f32::to_bits), words));
            return canvas(move |g, b| {
                let p = theme::palette();
                let ink = color.unwrap_or_else(|| if dim { style::secondary_text(&p) } else { style::c(p.panel_text) });
                // (On the line of what it's beside, cut or not: a cut
                // status beside a title sat half a point off it.)
                let r = match beside {
                    Some(of) => style::label_on_baseline_fit(g, b.left(), style::baseline_at(g, b.center().y, of), style::Align::Left, text.clone(), size, font.clone(), ink, b.size.width),
                    None if words => style::label_fit_words(g, b.left(), b.center().y, style::Align::Left, text.clone(), size, font.clone(), ink, b.size.width),
                    None => style::label_fit(g, b.left(), b.center().y, style::Align::Left, text.clone(), size, font.clone(), ink, b.size.width),
                };
                g.agent(agent::Node::label(text.clone(), r));
            })
            .paint_key(key)
            .w_full()
            .min_w_0()
            .h(px(self.height));
        }
        let w = f32::from(style::text_width(self.text.clone(), size, font.clone())).ceil();
        let (text, dim, color, height, beside, line) = (self.text, self.dim, self.color, self.height, self.beside, self.line);
        let key = style::key((&text, dim, style::ink_key(color), size.to_bits(), self.bold, mono, beside.map(f32::to_bits), line.map(f32::to_bits)));
        canvas(move |g, b| {
            let p = theme::palette();
            let ink = color.unwrap_or_else(|| if dim { style::secondary_text(&p) } else { style::c(p.panel_text) });
            let r = match (line, beside) {
                (Some(h), _) => style::label_on_baseline(g, b.left(), style::paragraph_baseline(g, b.top(), h, size, font.clone()), style::Align::Left, text.clone(), size, font.clone(), ink),
                (None, Some(of)) => style::label_on_baseline(g, b.left(), style::baseline_at(g, b.center().y, of), style::Align::Left, text.clone(), size, font.clone(), ink),
                (None, None) => style::label(g, b.left(), b.center().y, style::Align::Left, text.clone(), size, font.clone(), ink),
            };
            g.agent(agent::Node::label(text.clone(), r));
        })
        .paint_key(key)
        .w(px(w))
        .h(px(height))
        .flex_shrink_0()
    }
}

/// A bold section heading, as in preference panels.
pub fn heading(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).bold(true).height(m::LABEL_ROW)
}

/// A short tag in a pill, saying what kind a thing is (Olo Doctor's
/// Critical, Problem, Recommended, Suggestion): bold, in the detail
/// size, black or white on its colour (`fill`, from the palette's hues),
/// else quiet (grey: what's set aside). As wide as its word, centred in
/// a row `LABEL_ROW` tall, or on the capitals of the first line of a
/// paragraph beside it ([`Self::beside_paragraph`]: a title that may
/// wrap). Agents read it as a label.
pub struct Tag {
    text: SharedString,
    fill: Option<olo_theme::Rgba>,
    height: f32,
    /// Where its middle is, from its top.
    middle: f32,
}

impl Tag {
    pub fn new(text: impl Into<SharedString>) -> Tag {
        Tag { text: crate::words::of(text), fill: None, height: m::LABEL_ROW, middle: m::LABEL_ROW / 2.0 }
    }

    /// Level with the first line of a wrapped paragraph of `size` text
    /// beside it, tops aligned (`items_start`): centred on its capitals
    /// ([`style::paragraph_caps_middle`]), not in its line's height,
    /// half a point lower at 12 points.
    pub fn beside_paragraph(mut self, size: f32) -> Self {
        self.height = style::line_row(size);
        self.middle = style::paragraph_caps_middle(size);
        self
    }

    /// Its colour (else quiet).
    pub fn fill(mut self, rgba: olo_theme::Rgba) -> Self {
        self.fill = Some(rgba);
        self
    }

    /// The row it's centred in.
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self.middle = h / 2.0;
        self
    }
}

impl IntoElement for Tag {
    fn into_element(self) -> Element {
        let font = style::ui_font(FontWeight::BOLD);
        let w = style::tag_width(&self.text, style::DETAIL_SIZE, &font);
        let (text, fill, middle) = (self.text, self.fill, self.middle);
        let key = style::key((&text, fill, middle.to_bits()));
        canvas(move |g, b| {
            let p = theme::palette();
            let (paint, ink) = match fill {
                Some(f) => (style::c(f), style::ink_on(f)),
                None => (style::c(olo_theme::mix(p.document_background, p.document_text, 0.13)), style::c(olo_theme::mix(p.document_background, p.document_text, 0.62))),
            };
            let r = style::tag(g, b.left(), b.top() + px(middle), &text, style::DETAIL_SIZE, font.clone(), paint, ink);
            g.agent(agent::Node::label(text.clone(), r));
        })
        .paint_key(key)
        .w(px(w))
        .h(px(self.height))
        .flex_shrink_0()
    }
}

/// A groove across the width it's given: a dark line over a light one,
/// in a row six points tall.
pub struct Separator;

/// A [`Separator`].
pub fn separator() -> Separator {
    Separator
}

impl IntoElement for Separator {
    fn into_element(self) -> Element {
        canvas(|g, b| {
            let p = theme::palette();
            let y = style::snap_coord(g, b.center().y);
            style::hline_above(g, b.left(), b.right(), y, style::shade(&p, p.panel_background, 1.2));
            style::hline(g, b.left(), b.right(), y, style::c(p.shine));
        })
        .paint_key(1)
        .w_full()
        .h(px(6.0))
        .flex_shrink_0()
    }
}

/// BBox: what it holds in an etched frame, its label (bold) set into the
/// frame's top line. Its rows are `metrics::STACK` apart.
pub struct BBox {
    label: Option<SharedString>,
    width: Option<f32>,
    action: Option<(SharedString, OnAction)>,
    grow: bool,
    children: SmallVec<[AnyElement; 4]>,
}

impl BBox {
    pub fn new(label: impl Into<SharedString>) -> BBox {
        BBox { label: Some(crate::words::of(label)), width: None, action: None, grow: false, children: SmallVec::new() }
    }

    /// Without a label.
    pub fn plain() -> BBox {
        BBox { label: None, width: None, action: None, grow: false, children: SmallVec::new() }
    }

    /// A `LinkButton` at the right end of its top line, for what's in it
    /// (Preferences' Reset: this box's settings back to their defaults).
    pub fn action(mut self, label: impl Into<SharedString>, on_click: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.action = Some((crate::words::of(label), std::rc::Rc::new(on_click)));
        self
    }

    /// What it holds is `w` wide (the frame, its insets wider).
    pub fn content_width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    /// It takes the height its column has spare (a window's designed
    /// height less the rest), and may be shorter than what it holds
    /// needs: for a box with a list in it (a `flex_1().min_h_0()` scroll
    /// view), which shows as much as that room allows and scrolls the
    /// rest ("Windows fit what they show": the list, nothing else).
    pub fn grow(mut self) -> Self {
        self.grow = true;
        self
    }
}

impl ParentElement for BBox {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl IntoElement for BBox {
    fn into_element(self) -> Element {
        let titled = self.label.is_some() || self.action.is_some();
        let label_h = if titled { m::LABEL_ROW } else { 0.0 };
        let pad = m::BOX_INSET;
        // (Under its label as far as its rows are from each other.)
        let content_top = if titled { label_h + m::STACK } else { pad };
        let label_w = self.label.as_ref().map(|l| f32::from(style::text_width(l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil());
        let action_w = self.action.as_ref().map(|(l, _)| super::link_button_width(l));
        let label = self.label.clone();
        let frame = canvas(move |g, b| {
            let p = theme::palette();
            // The frame's top line runs through the middle of the
            // label's row.
            let top = px((label_h / 2.0).floor());
            let frame = Bounds::from_corners(point(b.left(), b.top() + top), b.bottom_right());
            // (What's in it must fit inside the frame's inset: on the
            // device pixels layout puts its children on, which it
            // rounds to; unrounded, a child flush with the inset read
            // as a point past it at 1.25× and 1.75×. The inset, 14
            // points, is a half pixel there, and layout puts a child
            // either side of one: the box gets the outer side. Rounded
            // as it came, 605.2 × 1.25 = 756.49994 went in, and
            // DiskBackup's Key button read as 1.6 points out.)
            let s = g.scale_factor();
            let out = |v: Pixels, outward: f32| px((f32::from(v) * s + outward * 0.01).round() / s);
            crate::review::container(g, Bounds::from_corners(point(out(frame.left() + px(pad), -1.0), frame.top()), point(out(frame.right() - px(pad), 1.0), out(frame.bottom() - px(pad), 1.0))));
            // (Gaps in the top line for the label and the action, as
            // far from them as from the frame's inset.)
            let gaps = [label_w.map(|w| (frame.left() + px(pad - 4.0), frame.left() + px(pad + w + 4.0))), action_w.map(|w| (frame.right() - px(pad + w + 4.0), frame.right() - px(pad - 4.0)))];
            etched(g, frame, &p, &gaps);
            if let Some(l) = &label {
                style::label(g, b.left() + px(pad), b.top() + px(label_h / 2.0), style::Align::Left, l.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
                // What the controls in it are about, for agents (and
                // the settings the palette finds: Preferences' index).
                g.agent(crate::agent::Node { role: "Group", name: Some(l.clone()), bounds: b, enabled: true, ..Default::default() });
            }
        })
        .paint_key(style::key((&self.label, label_w.map(f32::to_bits), action_w.map(f32::to_bits))))
        .absolute()
        .inset_0();
        let mut b = div().relative();
        if self.grow {
            b = b.flex_1().min_h_0();
        }
        if let Some(w) = self.width {
            b = b.w(px(w + 2.0 * pad));
        }
        let action = self.action.map(|(l, f)| {
            let id = SharedString::from(format!("{}-{l}", self.label.as_ref().map_or("box", |s| s.as_ref())));
            div().absolute().top_0().right(px(pad)).child(super::LinkButton::new(l).id(id).on_click(move |event, cx| f(event, cx)))
        });
        b.flex().flex_col().gap(px(m::STACK)).pt(px(content_top)).px(px(pad)).pb(px(pad)).child(frame).children(action).children(self.children)
    }
}

/// The etched rectangle: a dark line, then a light one a line in, on
/// device pixels at any scale; `gaps` leave room in the top line.
fn etched(g: &mut Painter, frame: Bounds<Pixels>, p: &olo_theme::Palette, gaps: &[Option<(Pixels, Pixels)>]) {
    let dark = style::shade(p, p.panel_background, 1.22);
    let light = style::c(p.shine);
    let (f, i1, i2) = (style::inset(g, frame, 0.0), style::inset(g, frame, 1.0), style::inset(g, frame, 2.0));
    let top = |g: &mut Painter, y: Pixels, color: Hsla| {
        // (Left to right, between the gaps.)
        let mut x = f.left();
        for &(a, b) in gaps.iter().flatten() {
            style::hline(g, x, a, y, color);
            x = b;
        }
        style::hline(g, x, f.right(), y, color);
    };
    top(g, f.top(), dark);
    top(g, i1.top(), light);
    style::hline_above(g, f.left(), f.right(), i1.bottom(), dark);
    style::hline_above(g, f.left(), f.right(), f.bottom(), light);
    style::vline(g, f.left(), f.top(), i1.bottom(), dark);
    style::vline(g, i1.left(), i1.top(), i2.bottom(), light);
    style::vline_left_of(g, i1.right(), f.top(), i1.bottom(), dark);
    style::vline_left_of(g, f.right(), f.top(), f.bottom(), light);
}

/// What a view shows while it has nothing to show (no chat chosen, an
/// integration not set up): an icon (32 points) over a line in the
/// disabled text's colour, a smaller detail under it, and a button,
/// centred in what it's given.
pub struct EmptyState {
    text: SharedString,
    icon: Option<SharedString>,
    detail: Option<SharedString>,
    action: Option<(SharedString, OnAction)>,
}

type OnAction = std::rc::Rc<dyn Fn(&ClickEvent, &mut WindowCx)>;

impl EmptyState {
    pub fn new(text: impl Into<SharedString>) -> EmptyState {
        EmptyState { text: crate::words::of(text), icon: None, detail: None, action: None }
    }

    pub fn icon(mut self, name: impl Into<SharedString>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// A line or two more, smaller, under the text.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(crate::words::of(detail));
        self
    }

    /// A button under it all ("Open Integrations…").
    pub fn action(mut self, label: impl Into<SharedString>, on_click: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.action = Some((crate::words::of(label), std::rc::Rc::new(on_click)));
        self
    }
}

impl IntoElement for EmptyState {
    fn into_element(self) -> Element {
        let p = crate::theme::palette();
        let dim = style::disabled_text(&p);
        let icon = self.icon.clone();
        // (As wide as it's given, 320 points at most, its lines wrapped
        // to that: one line each, a pane a third of a screen wide cut its
        // detail off.)
        let mut col = div().w_full().max_w(px(320.0)).flex().flex_col().items_center().gap(px(m::TIGHT)).text_color(dim);
        if let Some(name) = icon {
            col = col.child(
                canvas({
                    let name = name.clone();
                    move |g, b| {
                        crate::icons::paint(g, b, &name);
                    }
                })
                .paint_key(style::key(&name))
                .w(px(32.0))
                .h(px(32.0))
                .mb(px(m::SPACING - m::TIGHT)),
            );
        }
        // (Its line is what the view says: read, in the secondary colour;
        // its detail quieter under it. Both were the disabled colour, and
        // a view with nothing in it looked turned off.)
        col = col.child(div().w_full().text_center().text_color(style::secondary_text(&p)).line_height(olo_interface::relative(style::LINE_HEIGHT)).child(self.text.clone()));
        if let Some(d) = self.detail.clone() {
            col = col.child(div().w_full().text_center().text_size(px(style::DETAIL_SIZE)).line_height(olo_interface::relative(style::LINE_HEIGHT)).child(d));
        }
        if let Some((label, f)) = self.action {
            col = col.child(div().mt(px(m::SPACING * 1.5 - m::TIGHT)).child(super::Button::new(label).on_click(move |event, cx| f(event, cx))));
        }
        div().size_full().flex().items_center().justify_center().px(px(m::SPACING)).child(col)
    }
}
