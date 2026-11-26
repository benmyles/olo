//! BeOS's push button, check box and radio button: bevels and gradients
//! on device pixels, the label's capitals centred, pushed in by a line
//! while held.

use std::rc::Rc;

use olo_interface::{Bounds, ClickEvent, Element, FontWeight, IntoElement, Painter, ParentElement as _, Pixels, SharedString, Styled as _, WindowCx, div, fill, point, px, size};
use olo_kit_base::metrics as m;

use super::control::{ControlState, control};
use crate::{style, theme};

// The radio's circular bevel is one point wider than a checkbox.
const RADIO_WIDTH: f32 = 13.0;

fn described(mark: impl IntoElement, width: f32, text: impl Into<SharedString>) -> Element {
    div().flex().flex_col().child(mark).child(div().pl(px(width + m::MARK_GAP)).child(super::StringView::new(text).dim(true).wrap(true))).into_element()
}

/// As [`described`], in a note's small words: for a window whose notes
/// under its lines are so. Under a mark that can't be chosen
/// (`enabled` false) the note is as dim as its label is: left as it was,
/// it stood out over the label it explains (a reviewer's, of Rex's Host
/// window).
fn noted(mark: impl IntoElement, width: f32, text: impl Into<SharedString>, enabled: bool) -> Element {
    let mut note = super::StringView::new(text).size(style::SMALL_SIZE).dim(true).wrap(true);
    if !enabled {
        note = note.color(style::disabled_text(&theme::palette()));
    }
    div().flex().flex_col().child(mark).child(div().pl(px(width + m::MARK_GAP)).child(note)).into_element()
}

/// A small button's height (`Button::small`).
pub const SMALL_BUTTON_HEIGHT: f32 = 20.0;

type Handler = Rc<dyn Fn(&ClickEvent, &mut WindowCx)>;

/// BButton: a bevelled push button. The default one sits in a recessed
/// ring (drawn outside its bounds, so a row of buttons stays aligned).
pub struct Button {
    id: SharedString,
    label: SharedString,
    default: bool,
    pushed: bool,
    small: bool,
    enabled: bool,
    min_width: f32,
    /// Working on what it started (`Button::working`): how far, if known.
    working: Option<Option<f32>>,
    /// It opens a menu (`Button::menu`).
    menu: bool,
    /// As tall as what it stands beside (`Button::height`).
    height: Option<f32>,
    /// What it does, said when the pointer rests on it (`Button::tip`).
    tip: Option<SharedString>,
    on_click: Option<Handler>,
}

/// How much wider a button is that opens a menu: its triangle and the
/// room before it.
pub const MENU_MARK_ROOM: f32 = 14.0;

impl Button {
    pub fn new(label: impl Into<SharedString>) -> Button {
        let label = crate::words::of(label);
        Button {
            id: label.clone(),
            label,
            default: false,
            pushed: false,
            small: false,
            enabled: true,
            min_width: m::BUTTON_MIN_WIDTH,
            working: None,
            menu: false,
            height: None,
            tip: None,
            on_click: None,
        }
    }

    /// A tip saying `text`, once the pointer has rested on it: what a
    /// button under a list does to the row chosen, where its two words
    /// can't say it all (Rex's Stop Screen: "Ends what runs on it. The
    /// screen is kept, stopped."). It shows whether the button can be
    /// pressed or not, as a check box's does.
    pub fn tip(mut self, text: impl Into<SharedString>) -> Self {
        self.tip = Some(crate::words::of(text));
        self
    }

    /// Its id, when two buttons share a label.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn default(mut self, on: bool) -> Self {
        self.default = on;
        self
    }

    /// Drawn held down: the chosen one of a set of tool buttons.
    pub fn pushed(mut self, on: bool) -> Self {
        self.pushed = on;
        self
    }

    /// It opens a menu: a triangle pointing down after its label, as a
    /// pop-up's and the filter bar's have, so it doesn't look like it
    /// does what it says at once (Opus's look at Passwords' New for an SSH
    /// key, 2026-11-24: "Nobody will expect a menu from New"); the button
    /// `MENU_MARK_ROOM` wider for it, and held down while its menu is
    /// open where its click says so (`pushed`).
    pub fn menu(mut self) -> Self {
        self.menu = true;
        self
    }

    /// As tall as the thing it stands beside and acts on, where that
    /// isn't a control's height (a sunken well whose frame is in device
    /// pixels, so 24 to 26 points by the scale: Rex's error banner, whose
    /// Try Again at a control's height stood stepped against its
    /// detail's well, Gemini 4 Neon's look, 2026-11-26). Never shorter
    /// than a small button.
    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h.max(SMALL_BUTTON_HEIGHT));
        self
    }

    /// Smaller, in the small font, for actions inside something dense.
    pub fn small(mut self) -> Self {
        self.small = true;
        self.min_width = self.min_width.min(48.0);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// Working on the job it started, its progress shown on itself while
    /// it can't be pressed again: its face the status bar's sunken well,
    /// filled to `done` (0–1) as the job goes, its label over it (light on
    /// what's filled, dark on the rest); with `None` (how far isn't known)
    /// a band sweeping across it. Only for a job expected to take five
    /// seconds or more (an export, an install, a download), never a quick
    /// one: a flash of a meter says nothing. Agents read it as a button
    /// not enabled, its value how far ("42%", or "Working").
    pub fn working(mut self, done: Option<f32>) -> Self {
        self.working = Some(done);
        self.enabled = false;
        self
    }

    /// Takes a click. (Where the button is, for a menu to open under
    /// it: `WindowCx::listener_bounds`.)
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// A button's width for `label` (on the two-point grid), in the
/// language its button shows it in (`Button::new` looks a literal up: so
/// does this, or buttons sized to match were sized for their English).
pub fn button_width(label: &str, small: bool) -> f32 {
    let size = if small { style::DETAIL_SIZE } else { style::PLAIN_SIZE };
    let pad = if small { m::CONTROL_PAD * 0.6 } else { m::CONTROL_PAD };
    let min = if small { 48.0 } else { m::BUTTON_MIN_WIDTH };
    let tw = f32::from(style::text_width(crate::words::of_text(label), size, style::ui_font(FontWeight::NORMAL)));
    (((tw + 2.0 * pad).max(min)) / 2.0).ceil() * 2.0
}

impl IntoElement for Button {
    fn into_element(self) -> Element {
        let font_size = if self.small { style::DETAIL_SIZE } else { style::PLAIN_SIZE };
        let pad = if self.small { m::CONTROL_PAD * 0.6 } else { m::CONTROL_PAD };
        let tw = f32::from(style::text_width(self.label.clone(), font_size, style::ui_font(FontWeight::NORMAL)));
        let mark = if self.menu { MENU_MARK_ROOM } else { 0.0 };
        let w = (((tw + 2.0 * pad + mark).max(self.min_width)) / 2.0).ceil() * 2.0;
        let h = self.height.unwrap_or(if self.small { SMALL_BUTTON_HEIGHT } else { m::CONTROL_HEIGHT });
        let (label, default, pushed, working, menu) = (self.label.clone(), self.default, self.pushed, self.working, self.menu);
        let mut c = control(self.id.clone(), w, h, move |g, r, s| match working {
            Some(done) => paint_working(g, r, &label, done, font_size),
            None => {
                paint_button(g, r, s, &label, default, pushed, font_size, menu);
            }
        })
        // (How far, to a thousandth: drawn again as it moves, not for
        // each tick of a job's count.)
        .look((&self.label, default, pushed, self.small, menu, working.map(|d| d.map(|f| (f.clamp(0.0, 1.0) * 1000.0).round() as u32))))
        .enabled(self.enabled)
        .focusable();
        // (Its meter says it can't be pressed now: not faded too. Dimmed in
        // a dark look, it dims itself, its frame kept: `paint_button`.)
        if working.is_some() || (!self.enabled && dims_itself()) {
            c = c.unfaded();
        }
        c = match working {
            Some(Some(f)) => c.agent_value("Button", self.label.clone(), format!("{:.0}%", f.clamp(0.0, 1.0) * 100.0)),
            Some(None) => c.agent_value("Button", self.label.clone(), "Working"),
            None => c.agent("Button", self.label.clone(), None),
        };
        if default {
            c = c.default_key();
        }
        if let Some(f) = self.on_click {
            c = c.on_click(move |event, cx| f(event, cx));
        }
        match self.tip {
            Some(text) if self.enabled => c.tip(text).into_element(),
            // (What isn't enabled hears nothing of the pointer: its tip
            // is its place's, around it, as a check box's.)
            Some(text) => crate::tip::on(div().flex_shrink_0().child(c.into_element()), format!("tip-{}", self.id), text),
            None => c.into_element(),
        }
    }
}

/// A square push button with an arrow on it, back or on: for stepping
/// through what a window shows (Calendar's days, weeks and months), set
/// in a pair. It says what it does to agents and in its tip, having no
/// words on it.
pub struct ArrowButton {
    id: SharedString,
    name: SharedString,
    back: bool,
    enabled: bool,
    on_click: Option<Handler>,
}

impl ArrowButton {
    /// The one that goes back, named `name` ("Previous Week").
    pub fn back(name: impl Into<SharedString>) -> ArrowButton {
        let name = crate::words::of(name);
        ArrowButton { id: name.clone(), name, back: true, enabled: true, on_click: None }
    }

    /// The one that goes on ("Next Week").
    pub fn on(name: impl Into<SharedString>) -> ArrowButton {
        ArrowButton { back: false, ..ArrowButton::back(name) }
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl IntoElement for ArrowButton {
    fn into_element(self) -> Element {
        let back = self.back;
        let mut c = control(self.id.clone(), m::CONTROL_HEIGHT, m::CONTROL_HEIGHT, move |g, r, s| {
            paint_button(g, r, s, "", false, false, style::PLAIN_SIZE, false);
            let p = theme::palette();
            let off = if s.pressed { style::hairline(g) } else { px(0.0) };
            let at = Bounds { origin: point(r.left() + off, r.top() + off), size: r.size };
            style::arrow(g, at, back, if s.enabled { style::c(p.control_text) } else { style::disabled_text(&p) });
        })
        .look((&self.name, back))
        .enabled(self.enabled)
        .focusable()
        .agent("Button", self.name.clone(), None);
        if let Some(f) = self.on_click {
            c = c.on_click(move |event, cx| f(event, cx));
        }
        crate::tip::on(c.into_element(), self.id, self.name)
    }
}

/// What a [`GlyphButton`] has on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// Add one (a session to a host, a row to a list).
    Plus,
    /// Close, or take away, what it is set against.
    Close,
    /// Split what it is set against, side by side.
    SplitRight,
    /// Split it, one above the other.
    SplitDown,
}

/// A glyph button's size: square, lower than a small button, for a
/// pane's header or a list's heading row (22 points); its mark is
/// eleven points across at most.
pub const GLYPH_BUTTON: (f32, f32) = (18.0, 18.0);

/// A small button that is only its mark, for an action set right
/// against what it acts on in a dense row: a pane's header (split it,
/// close it), a heading in a list (add to it). At rest the mark alone,
/// quiet, as a tab's × and a tab strip's + are (one look for those in
/// Olo, the same marks); under the pointer a push button's face, and
/// pressed as one. It says what it does to agents and in its tip. Its
/// mark is drawn in whole device pixels at every scale
/// ([`style::plus_mark`], [`style::split_mark`], [`style::close_mark`]).
/// (Rex drew four of its own at fractions of a point, blurred at 1× and
/// 1.5×; framed at rest, a row of them in each pane's header was the
/// loudest thing in the window: Opus's review, 2026-11-22.)
pub struct GlyphButton {
    id: SharedString,
    name: SharedString,
    glyph: Glyph,
    enabled: bool,
    quiet: bool,
    on_click: Option<Handler>,
}

impl GlyphButton {
    /// `name`: what it does ("New Session", "Split Right"), for agents
    /// and its tip.
    pub fn new(name: impl Into<SharedString>, glyph: Glyph) -> GlyphButton {
        let name = crate::words::of(name);
        GlyphButton { id: name.clone(), name, glyph, enabled: true, quiet: false, on_click: None }
    }

    /// Its mark fainter at rest: on something that isn't the one in use
    /// (a pane without the keyboard, whose title is quieter too: thin
    /// lines in the same colour as dimmed words read louder than the
    /// words). Under the pointer it is as any.
    pub fn quiet(mut self, on: bool) -> Self {
        self.quiet = on;
        self
    }

    /// Its own id, where several in a window do the same to different
    /// things (a + for each host).
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl IntoElement for GlyphButton {
    fn into_element(self) -> Element {
        let (glyph, quiet) = (self.glyph, self.quiet);
        let mut c = control(self.id.clone(), GLYPH_BUTTON.0, GLYPH_BUTTON.1, move |g, r, s| {
            let p = theme::palette();
            let over = s.enabled && (s.hovered || s.pressed);
            if over {
                paint_button(g, r, s, "", false, false, style::DETAIL_SIZE, false);
            }
            let ink = match (s.enabled, over) {
                (false, _) => style::disabled_text(&p),
                (true, true) => style::c(p.control_text),
                (true, false) if quiet => style::secondary_text(&p).opacity(0.7),
                (true, false) => style::secondary_text(&p),
            };
            let off = if s.pressed { style::hairline(g) } else { px(0.0) };
            let center = point(r.center().x + off, r.center().y + off);
            match glyph {
                Glyph::Plus => style::plus_mark(g, center, 10.0, 1.5, ink),
                Glyph::Close => style::close_mark(g, center, 7.0, ink),
                Glyph::SplitRight => style::split_mark(g, center, false, ink),
                Glyph::SplitDown => style::split_mark(g, center, true, ink),
            }
        })
        .look((&self.name, glyph, quiet))
        .enabled(self.enabled)
        .agent("Button", self.name.clone(), None);
        if let Some(f) = self.on_click {
            c = c.on_click(move |event, cx| f(event, cx));
        }
        crate::tip::on(c.into_element(), self.id, self.name)
    }
}

/// A button that is only its words, in the small font and the link
/// colour, underlined under the pointer: a small action beside what it
/// acts on, in a line that has no room for a push button (a box's Reset,
/// in its top line: `BBox::action`). `LABEL_ROW` tall.
pub struct LinkButton {
    id: SharedString,
    label: SharedString,
    enabled: bool,
    beside: Option<f32>,
    /// The widest it may be: wider words wrap onto more lines.
    wrap: Option<f32>,
    /// Its words' size: the detail size unless said.
    size: f32,
    /// On a paragraph's line this tall ([`LinkButton::line`]).
    line: Option<f32>,
    /// Cut short to its room ([`LinkButton::cut`]).
    cut: bool,
    on_click: Option<Handler>,
}

impl LinkButton {
    pub fn new(label: impl Into<SharedString>) -> LinkButton {
        let label = crate::words::of(label);
        LinkButton { id: label.clone(), label, enabled: true, beside: None, wrap: None, size: style::DETAIL_SIZE, line: None, cut: false, on_click: None }
    }

    /// Its words in `points`, not the detail size: a link that is a value
    /// among text of that size (an event's address in Calendar's preview).
    pub fn size(mut self, points: f32) -> Self {
        self.size = points;
        self
    }

    /// `points` tall, not `LABEL_ROW`, its words where a paragraph's line
    /// that tall has them (`style::paragraph_baseline`): a link among text
    /// set as a paragraph, on its line at every scale (Calendar's preview:
    /// `TextView::line_height`).
    pub fn line(mut self, points: f32) -> Self {
        self.line = Some(points);
        self
    }

    /// At most as wide as its room: words wider than that are cut short
    /// with an ellipsis, the whole of them in its tip and to agents (an
    /// address in a column: Calendar's preview).
    pub fn cut(mut self) -> Self {
        self.cut = true;
        self
    }

    /// At most `width` wide: words that don't fit go onto the next line,
    /// and it's as tall as its lines (a question offered in a narrow
    /// column: Weather's assistant). Still one control: focused, pressed
    /// and read by agents as one.
    pub fn wrap_at(mut self, width: f32) -> Self {
        self.wrap = Some(width);
        self
    }

    /// Its words on the baseline that text of `size` centred in the same
    /// row has (a push button's label beside it), not centred themselves:
    /// a row of buttons and links reads as one line (`chosen::choose`).
    pub fn beside(mut self, size: f32) -> Self {
        self.beside = Some(size);
        self
    }

    /// Its id, when two share a label (each box's Reset).
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

/// A link button's width for `label` (on the two-point grid).
pub fn link_button_width(label: &str) -> f32 {
    link_width(label, style::DETAIL_SIZE)
}

/// `label`'s width in `size` (on the two-point grid).
fn link_width(label: &str, size: f32) -> f32 {
    let tw = f32::from(style::text_width(label.to_string(), size, style::ui_font(FontWeight::NORMAL)));
    (tw / 2.0).ceil() * 2.0
}

/// `label`'s words in lines no wider than `width` in `size` (a word wider
/// than that alone on its line).
fn link_lines(label: &str, width: f32, size: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in label.split_whitespace() {
        match lines.last_mut() {
            Some(line) if link_width(&format!("{line} {word}"), size) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

impl IntoElement for LinkButton {
    fn into_element(self) -> Element {
        // Wrapped: its lines, a row each (the first the usual row).
        let (size, line, cut) = (self.size, self.line, self.cut);
        let height = line.unwrap_or(m::LABEL_ROW);
        let lines: Option<Vec<String>> = self.wrap.filter(|w| link_width(&self.label, size) > *w).map(|w| link_lines(&self.label, w, size));
        if let Some(lines) = lines {
            let row = style::line_row(size);
            let w = lines.iter().map(|l| link_width(l, size)).fold(0.0, f32::max);
            let h = height + row * (lines.len() - 1) as f32;
            let first = (height - row) / 2.0;
            let mut c = control(self.id.clone(), w, h, move |g, r, s| {
                let p = theme::palette();
                let link = olo_theme::readable(p.hue(olo_theme::Hue::Blue), p.panel_background, 4.5);
                let color = if !s.enabled {
                    style::disabled_text(&p)
                } else if s.pressed {
                    style::shade(&p, link, 1.25)
                } else {
                    style::c(link)
                };
                let font = style::ui_font(FontWeight::NORMAL);
                for (i, line) in lines.iter().enumerate() {
                    let cy = r.top() + px(first + row * (i as f32 + 0.5));
                    let text = style::label(g, r.left(), cy, style::Align::Left, line.clone(), size, font.clone(), color);
                    if s.focused {
                        focus_underline(g, text, &p);
                    } else if s.hovered && s.enabled {
                        style::hline_above(g, text.left(), text.right(), text.bottom(), color);
                    }
                }
            })
            .look((&self.label, self.wrap.map(f32::to_bits), size.to_bits(), line.map(f32::to_bits)))
            .enabled(self.enabled)
            .focusable()
            .pointing()
            .agent("Button", self.label.clone(), None);
            if let Some(f) = self.on_click {
                c = c.on_click(move |event, cx| f(event, cx));
            }
            return c.into_element();
        }
        let w = link_width(&self.label, size);
        let (label, beside) = (self.label.clone(), self.beside);
        let mut c = control(self.id.clone(), w, height, move |g, r, s| {
            let p = theme::palette();
            // (The blue of links, readable on the panel.)
            let link = olo_theme::readable(p.hue(olo_theme::Hue::Blue), p.panel_background, 4.5);
            let color = if !s.enabled {
                style::disabled_text(&p)
            } else if s.pressed {
                style::shade(&p, link, 1.25)
            } else {
                style::c(link)
            };
            let font = style::ui_font(FontWeight::NORMAL);
            // (On a paragraph's line, `line`; beside text of another size,
            // `beside`; else in the middle. Cut, what fits of it: `cut`.)
            let baseline = match (line, beside) {
                (Some(h), _) => Some(style::paragraph_baseline(g, r.top(), h, size, font.clone())),
                (None, Some(of)) => Some(style::baseline_at(g, r.center().y, of)),
                (None, None) => None,
            };
            let text = match (baseline, cut) {
                (Some(on), false) => style::label_on_baseline(g, r.left(), on, style::Align::Left, label.to_string(), size, font, color),
                (Some(on), true) => style::label_on_baseline_fit(g, r.left(), on, style::Align::Left, label.to_string(), size, font, color, r.size.width),
                (None, false) => style::label(g, r.left(), r.center().y, style::Align::Left, label.to_string(), size, font, color),
                (None, true) => style::label_fit(g, r.left(), r.center().y, style::Align::Left, label.to_string(), size, font, color, r.size.width),
            };
            if s.focused {
                focus_underline(g, text, &p);
            } else if s.hovered && s.enabled {
                style::hline_above(g, text.left(), text.right(), text.bottom(), color);
            }
        })
        .look((&self.label, self.beside.map(f32::to_bits), size.to_bits(), line.map(f32::to_bits), cut))
        .enabled(self.enabled)
        .focusable()
        .pointing()
        .agent("Button", self.label.clone(), None);
        if cut {
            c = c.shrinks().tip(self.label.clone());
        }
        if let Some(f) = self.on_click {
            c = c.on_click(move |event, cx| f(event, cx));
        }
        c.into_element()
    }
}

#[allow(clippy::too_many_arguments)]
/// Whether a dimmed button draws its own dimness (a dark look), rather
/// than being faded as a whole: faded, its frame went halfway to the
/// panel, and over a dark one the button all but went (12,21,27 on
/// 24,43,54, where light's is 153,159,167 on 219,227,238: Opus's look at
/// About's Software Update).
fn dims_itself() -> bool {
    theme::palette().mode == olo_theme::ThemeMode::Dark
}

#[allow(clippy::too_many_arguments)]
fn paint_button(g: &mut Painter, r: Bounds<Pixels>, s: ControlState, label: &str, default: bool, pushed: bool, font_size: f32, menu: bool) {
    let p = theme::palette();
    let bg = p.panel_background;
    // (Dimmed in a dark look: its face, groove and bevel as faded as a
    // control is, its frame whole, so it still reads as a button.)
    let own_dim = !s.enabled && dims_itself();
    let fade = |c: olo_interface::Hsla| if own_dim { c.opacity(olo_interface::DISABLED_OPACITY) } else { c };
    if default {
        // A recessed groove the button sits in.
        let ring = px(3.0);
        let outer = style::snap(g, Bounds::from_corners(point(r.left() - ring, r.top() - ring), point(r.right() + ring, r.bottom() + ring)));
        style::fill_rect(g, outer, fade(style::shade(&p, bg, 1.06)));
        style::bevel(g, outer, fade(style::shade(&p, bg, 1.3)), fade(style::c(p.shine)));
    }
    let pressed = s.pressed || pushed;
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let base = p.control_background;
    let (top, bottom) = if pressed {
        if dark {
            // Genuine recessed dark fill so pushed toggles read unmistakably against raised buttons.
            let hole = p.document_background;
            (style::c(hole), style::c(olo_theme::mix(hole, base, 0.25)))
        } else {
            (style::shade(&p, base, 1.24), style::shade(&p, base, 1.14))
        }
    } else if s.hovered {
        (style::tint(base, 0.7), style::shade(&p, base, 1.02))
    } else {
        (style::tint(base, 0.85), style::shade(&p, base, 1.06))
    };
    let border = if s.enabled || own_dim { style::shade(&p, bg, 1.6) } else { style::shade(&p, bg, 1.3) };
    let inner = style::inset(g, r, 1.0);
    style::vgradient(g, inner, fade(top), fade(bottom));
    style::frame_rect(g, r, border);
    if pressed {
        if dark {
            style::bevel(g, inner, fade(style::c([0, 0, 0, 180])), fade(style::c(olo_theme::mix(base, p.panel_text, 0.22))));
        } else {
            style::bevel(g, inner, fade(style::shade(&p, base, 1.38)), fade(style::c(p.shine)));
        }
    } else {
        style::bevel(g, inner, fade(style::c(p.shine)), fade(style::shade(&p, base, 1.2)));
    }
    // Held by the pointer, the label gives a device pixel; one that
    // stays down (a chosen tool) keeps it level with its row, in semibold.
    let off = if s.pressed && !pushed { style::hairline(g) } else { px(0.0) };
    let color = if s.enabled { style::c(p.control_text) } else { style::disabled_text(&p) };
    let c = r.center();
    // (One that opens a menu: its label and the mark, the filter bar's
    // triangle, centred together, the mark 7 points after the label as
    // the bar has it: the room it was given went between the word and the
    // mark, which read as a stray mark. Opus's look, 2026-11-24.)
    const GAP: f32 = 7.0;
    const MARK: f32 = 8.0;
    let weight = if pushed { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
    let tw = if menu { style::text_width_app(label, font_size, &style::ui_font(weight)) } else { 0.0 };
    let unit_left = c.x - px((tw + GAP + MARK) / 2.0);
    let label_x = if menu { unit_left + px(tw / 2.0) } else { c.x };
    let text = style::label(g, label_x + off, c.y + off, style::Align::Center, label.to_string(), font_size, style::ui_font(weight), color);
    if menu {
        let ax = style::snap_coord(g, unit_left + px(tw + GAP + MARK / 2.0)) + off;
        let ay = px(f32::from(c.y).round()) + off;
        let mut path = olo_interface::PathBuilder::fill();
        path.move_to(point(ax - px(4.0), ay - px(2.0)));
        path.line_to(point(ax + px(4.0), ay - px(2.0)));
        path.line_to(point(ax, ay + px(2.5)));
        path.close();
        if let Ok(path) = path.build() {
            g.paint_path(path, color);
        }
    }
    if s.focused {
        focus_underline(g, text, &p);
    }
}

/// A working button's face (`Button::working`): the status bar's sunken
/// well, filled to `done` as `level_bar` fills it, or, how far not known,
/// a band of its fill sweeping across every 1.6 s (drawn again each frame
/// only then: a known fraction is drawn again only as it moves). The
/// label centred over it, in the control's text colour, and over what's
/// filled in white or black, whichever reads better over all of the fill
/// (its top and its darker bottom).
fn paint_working(g: &mut Painter, r: Bounds<Pixels>, label: &str, done: Option<f32>, font_size: f32) {
    let p = theme::palette();
    let filled = match done {
        Some(f) => super::status::level_bar(g, r, f, None, &p),
        None => {
            static SINCE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let inner = style::inset(g, r, 1.0);
            style::fill_rect(g, inner, style::c(p.document_background));
            let t = SINCE.get_or_init(std::time::Instant::now).elapsed().as_secs_f32() % 1.6 / 1.6;
            let band = inner.size.width * 0.3;
            let left = inner.left() - band + (inner.size.width + band) * t;
            let (x0, x1) = (style::snap_coord(g, left.max(inner.left())), style::snap_coord(g, (left + band).min(inner.right())));
            let swept = Bounds::from_corners(point(x0, inner.top()), point(x1.max(x0), inner.bottom()));
            if swept.size.width > px(0.0) {
                let rgb = olo_interface::Rgba::from(style::c(p.status_bar));
                let lower: olo_interface::Hsla = olo_interface::Rgba { r: rgb.r * 0.8, g: rgb.g * 0.8, b: rgb.b * 0.8, a: 1.0 }.into();
                style::vgradient(g, swept, style::c(p.status_bar), lower);
                style::hline(g, swept.left(), swept.right(), swept.top(), style::c(p.gloss));
            }
            style::bevel(g, r, style::shade(&p, p.panel_background, 1.45), style::c(p.shine));
            g.paint_again_within(0.0);
            swept
        }
    };
    let on_fill = ink_over_fill(p.status_bar);
    let c = r.center();
    let font = style::ui_font(FontWeight::NORMAL);
    let rest = [Bounds::from_corners(r.origin, point(filled.left(), r.bottom())), Bounds::from_corners(point(filled.right(), r.top()), r.bottom_right())];
    for part in rest.into_iter().filter(|b| b.size.width > px(0.0)) {
        g.with_content_mask(Some(olo_interface::ContentMask { bounds: part }), |g| {
            style::label(g, c.x, c.y, style::Align::Center, label.to_string(), font_size, font.clone(), style::c(p.control_text));
        });
    }
    if filled.size.width > px(0.0) {
        g.with_content_mask(Some(olo_interface::ContentMask { bounds: filled }), |g| {
            style::label(g, c.x, c.y, style::Align::Center, label.to_string(), font_size, font.clone(), style::c(on_fill));
        });
    }
}

/// White or black, whichever reads better over all of a working
/// button's fill of `fill`: it darkens to 0.8 of itself at the bottom
/// (`level_bar`). (Judged by its top alone, Platinum's purple took black,
/// 3:1 lower down: the study of its theme, 2026-11-19.)
fn ink_over_fill(fill: olo_theme::Rgba) -> olo_theme::Rgba {
    let c8 = |v: u8| (f32::from(v) * 0.8).round() as u8;
    let lower: olo_theme::Rgba = [c8(fill[0]), c8(fill[1]), c8(fill[2]), fill[3]];
    let worst = |ink| olo_theme::contrast(ink, fill).min(olo_theme::contrast(ink, lower));
    if worst(olo_theme::WHITE) >= worst(olo_theme::BLACK) { olo_theme::WHITE } else { olo_theme::BLACK }
}

/// BeOS marks the control with the keyboard by underlining its label in
/// the navigation colour.
pub(crate) fn focus_underline(g: &mut Painter, text: Bounds<Pixels>, p: &olo_theme::Palette) {
    style::hline_above(g, text.left(), text.right(), text.bottom(), style::c(p.keyboard_navigation));
}

/// The sunken well check boxes share.
fn mark_well(g: &mut Painter, r: Bounds<Pixels>, p: &olo_theme::Palette, pressed: bool, enabled: bool) {
    let fill = if pressed {
        style::shade(p, p.document_background, 1.12)
    } else if enabled {
        style::c(p.document_background)
    } else {
        style::c(p.panel_background)
    };
    style::fill_rect(g, r, fill);
    style::bevel(g, r, style::shade(p, p.panel_background, 1.18), style::c(p.shine));
    let inner = style::inset(g, r, 1.0);
    if p.mode == olo_theme::ThemeMode::Dark {
        // (By night the rim all round, lighter than the panel: see
        // `style::well_rim`.)
        let rim = style::well_rim(p, 1.6);
        style::bevel(g, inner, rim, rim);
    } else {
        style::bevel(g, inner, style::shade(p, p.panel_background, 1.6), style::shade(p, p.panel_background, 1.08));
    }
}

/// BCheckBox.
pub struct CheckBox {
    id: SharedString,
    label: SharedString,
    checked: bool,
    mixed: bool,
    enabled: bool,
    mark_only: bool,
    height: f32,
    tip: Option<SharedString>,
    on_toggle: Option<Rc<dyn Fn(&bool, &mut WindowCx)>>,
}

impl CheckBox {
    pub fn new(label: impl Into<SharedString>, checked: bool) -> CheckBox {
        let label = crate::words::of(label);
        CheckBox { id: label.clone(), label, checked, mixed: false, enabled: true, mark_only: false, height: m::MARK_ROW, tip: None, on_toggle: None }
    }

    /// A tip saying `text`, once the pointer has rested on it: for a mark
    /// that isn't enabled, why not (Doctor's fix that is never done with
    /// the others keeps its mark, greyed, and says so here). It shows
    /// whether the mark is enabled or not.
    pub fn tip(mut self, text: impl Into<SharedString>) -> Self {
        self.tip = Some(text.into());
        self
    }

    /// Only its mark, beside what it chooses that names it already (an
    /// item's title in a list acted on by what's checked: `chosen`); its
    /// label is still its name for agents.
    pub fn mark_only(mut self) -> Self {
        self.mark_only = true;
        self
    }

    /// As tall as the controls beside it (`metrics::CONTROL_HEIGHT` beside
    /// a text field or button): placed alike, its label and theirs land
    /// on the same device pixels at every scale (a shorter box centred
    /// beside them rounds onto another at 1.25×).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    /// A dash, not a check: on for some of what it stands for. A click
    /// turns it on.
    pub fn mixed(mut self, on: bool) -> Self {
        self.mixed = on;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Add a wrapped explanation directly beneath this mark's label.
    pub fn description(self, text: impl Into<SharedString>) -> Element {
        described(self, m::MARK_SIZE, text)
    }

    /// Gets the new value when clicked.
    pub fn on_toggle(mut self, f: impl Fn(&bool, &mut WindowCx) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl IntoElement for CheckBox {
    fn into_element(self) -> Element {
        // Its id is its label unless given one: check boxes without a
        // label (a table's columns) shared the id "", and a click on one
        // was lost among them (Preferences ▸ Notifications' apps).
        debug_assert!(!self.id.is_empty(), "a CheckBox without a label needs an id of its own (CheckBox::id)");
        let mark_only = self.mark_only;
        let w = if mark_only {
            m::MARK_SIZE
        } else {
            let tw = f32::from(style::text_width(self.label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
            m::MARK_SIZE + m::MARK_GAP + tw
        };
        let (label, checked, mixed) = (self.label.clone(), self.checked, self.mixed);
        let mut c = control(self.id.clone(), w, self.height, move |g, r, s| {
            let p = theme::palette();
            // Centred on the label's x-height, on the device grid.
            let top = style::snap_coord(g, px((f32::from(r.center().y) - 6.5).round()));
            let well = Bounds { origin: point(style::snap_coord(g, r.left()), top), size: size(px(m::MARK_SIZE), px(m::MARK_SIZE)) };
            mark_well(g, well, &p, s.pressed, s.enabled);
            let ink = if s.enabled { style::c(p.control_mark) } else { style::disabled_text(&p) };
            if mixed {
                let y = style::snap_coord(g, well.center().y - px(0.75));
                let h = style::snap_coord(g, px(1.5)).max(style::hairline(g));
                style::fill_rect(g, Bounds::from_corners(point(well.left() + px(3.5), y), point(well.right() - px(3.5), y + h)), ink);
            } else if checked {
                let mark = Bounds::from_corners(point(well.left() + px(3.0), well.top() + px(3.5)), point(well.right() - px(2.5), well.bottom() - px(3.5)));
                style::check_mark(g, mark, ink);
            }
            if mark_only {
                // (The keyboard's line under the mark, clear of its well.)
                if s.focused {
                    let under = Bounds { origin: point(well.left(), well.top()), size: size(well.size.width, well.size.height + px(2.0)) };
                    focus_underline(g, under, &p);
                }
                return;
            }
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let text = style::label(g, well.right() + px(m::MARK_GAP), r.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(g, text, &p);
            }
        })
        .look((&self.label, checked, mixed, mark_only))
        .enabled(self.enabled)
        .focusable()
        .agent("CheckBox", self.label.clone(), Some(checked && !mixed));
        if let Some(f) = self.on_toggle {
            let next = mixed || !checked;
            c = c.on_click(move |_, cx| f(&next, cx));
        }
        match self.tip {
            Some(text) if self.enabled => c.tip(text).into_element(),
            // (What isn't enabled hears nothing of the pointer, its coming
            // and going neither: its tip is its place's, around it.)
            Some(text) => crate::tip::on(div().flex_shrink_0().child(c.into_element()), format!("tip-{}", self.id), text),
            None => c.into_element(),
        }
    }
}

/// BeOS's Installer's pane switch ("More Options"): a latch and, beside
/// it, what it opens. A click (Space, with the keyboard on it) turns it;
/// whoever shows it keeps whether it's open, says so in its label if it
/// likes ("Fewer Options"), and puts the pane under it. For a part of a
/// window most people leave alone, shown only when asked for, the window
/// growing to hold it (its window follows its content).
pub struct PaneSwitch {
    id: SharedString,
    label: SharedString,
    open: bool,
    enabled: bool,
    on_toggle: Option<Rc<dyn Fn(&bool, &mut WindowCx)>>,
}

/// The room a pane switch's latch takes, before its label.
const LATCH: f32 = 12.0;

impl PaneSwitch {
    pub fn new(label: impl Into<SharedString>, open: bool) -> PaneSwitch {
        let label = crate::words::of(label);
        PaneSwitch { id: label.clone(), label, open, enabled: true, on_toggle: None }
    }

    /// An id that stays while its label changes with what it shows
    /// ("More Options", "Fewer Options"): the keyboard stays on it.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Gets whether it's open now, when clicked.
    pub fn on_toggle(mut self, f: impl Fn(&bool, &mut WindowCx) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl IntoElement for PaneSwitch {
    fn into_element(self) -> Element {
        let tw = f32::from(style::text_width(self.label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let w = LATCH + m::MARK_GAP + tw;
        let (label, open) = (self.label.clone(), self.open);
        let mut c = control(self.id.clone(), w, m::MARK_ROW, move |g, r, s| {
            let p = theme::palette();
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            // (Held down, the latch is the keyboard's colour: BeOS drew
            // its own darker while pressed.)
            let ink = if s.pressed && s.enabled { style::c(p.keyboard_navigation) } else { color };
            style::latch(g, Bounds::centered_at(point(r.left() + px(LATCH / 2.0), r.center().y), size(px(LATCH), px(LATCH))), open, ink);
            let text = style::label(g, r.left() + px(LATCH + m::MARK_GAP), r.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(g, text, &p);
            }
        })
        .look((&self.label, open))
        .enabled(self.enabled)
        .focusable()
        .agent("DisclosureTriangle", self.label.clone(), Some(open));
        if let Some(f) = self.on_toggle {
            let next = !open;
            c = c.on_click(move |_, cx| f(&next, cx));
        }
        c.into_element()
    }
}

/// A check box's look alone, in `well` (`MARK_SIZE` square), checked or
/// not: what isn't a control but shows one (a document's task list).
pub fn check_box_look(g: &mut Painter, well: Bounds<Pixels>, p: &olo_theme::Palette, checked: bool) {
    mark_well(g, well, p, false, true);
    if checked {
        let mark = Bounds::from_corners(point(well.left() + px(3.0), well.top() + px(3.5)), point(well.right() - px(2.5), well.bottom() - px(3.5)));
        style::check_mark(g, mark, style::c(p.control_mark));
    }
}

/// BRadioButton: chosen by a click; the caller keeps which of its group
/// is chosen.
pub struct RadioButton {
    id: SharedString,
    label: SharedString,
    selected: bool,
    enabled: bool,
    height: f32,
    on_select: Option<Rc<dyn Fn(&(), &mut WindowCx)>>,
}

impl RadioButton {
    /// How far in from its left edge its label begins (its mark and
    /// the gap): where a line of an app's own that goes with the
    /// choices lines up with their words.
    pub const LABEL_INSET: f32 = RADIO_WIDTH + m::MARK_GAP;

    pub fn new(label: impl Into<SharedString>, selected: bool) -> RadioButton {
        let label = crate::words::of(label);
        RadioButton { id: label.clone(), label, selected, enabled: true, height: m::MARK_ROW, on_select: None }
    }

    /// Add a wrapped explanation directly beneath this mark's label.
    pub fn description(self, text: impl Into<SharedString>) -> Element {
        described(self, RADIO_WIDTH, text)
    }

    /// The same in a note's small dim words (`style::SMALL_SIZE`), where
    /// the window's other notes are so (Rex's Host window: what each
    /// choice means, under it).
    pub fn note(self, text: impl Into<SharedString>) -> Element {
        let enabled = self.enabled;
        noted(self, RADIO_WIDTH, text, enabled)
    }

    /// As tall as what's beside it, as [`CheckBox::height`]: beside a
    /// label or a button (`metrics::CONTROL_HEIGHT`), its text and theirs
    /// share a baseline at every scale (20 points tall, centred beside 24,
    /// it was a device pixel off at 1.25× and 1.75×).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = id.into();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&(), &mut WindowCx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl IntoElement for RadioButton {
    fn into_element(self) -> Element {
        // (As a CheckBox: without a label, an id of its own.)
        debug_assert!(!self.id.is_empty(), "a RadioButton without a label needs an id of its own (RadioButton::id)");
        let tw = f32::from(style::text_width(self.label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let w = RADIO_WIDTH + m::MARK_GAP + tw;
        let (label, selected) = (self.label.clone(), self.selected);
        let mut c = control(self.id.clone(), w, self.height, move |g, r, s| {
            let p = theme::palette();
            let bg = p.panel_background;
            let cx0 = r.left() + px(6.5);
            let cy0 = px(f32::from(r.center().y).round() - 0.5);
            let circle = |g: &mut Painter, x: Pixels, y: Pixels, radius: f32, color: olo_interface::Hsla| {
                let b = Bounds { origin: point(x - px(radius), y - px(radius)), size: size(px(2.0 * radius), px(2.0 * radius)) };
                g.paint_quad(fill(b, color).corner_radii(px(radius)));
            };
            // The outer ring: dark upper left, light lower right.
            circle(g, cx0 + px(0.5), cy0 + px(0.5), 6.5, style::c(p.shine));
            circle(g, cx0 - px(0.3), cy0 - px(0.3), 6.5, style::shade(&p, bg, 1.25));
            let face = if s.pressed {
                style::shade(&p, p.document_background, 1.12)
            } else if s.enabled {
                style::c(p.document_background)
            } else {
                style::c(bg)
            };
            // The ring a point wide outside the face.
            circle(g, cx0, cy0, 6.5, style::well_rim(&p, 1.55));
            circle(g, cx0, cy0, 5.5, face);
            if selected {
                let ink = if s.enabled { p.control_mark } else { olo_theme::mix(p.panel_text, p.panel_background, 0.55) };
                circle(g, cx0, cy0, 3.0, style::c(ink));
                // A small highlight keeps the dot from looking flat: the
                // mark's colour, half again as bright.
                let lit = [ink[0], ink[1], ink[2]].map(|v| (f32::from(v) * 1.4).min(255.0) as u8);
                circle(g, cx0 - px(1.0), cy0 - px(1.0), 0.9, style::c([lit[0], lit[1], lit[2], 255]));
            }
            let color = if s.enabled { style::c(p.panel_text) } else { style::disabled_text(&p) };
            let text = style::label(g, r.left() + px(RADIO_WIDTH + m::MARK_GAP), r.center().y, style::Align::Left, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color);
            if s.focused {
                focus_underline(g, text, &p);
            }
        })
        .look((&self.label, selected))
        .enabled(self.enabled)
        .focusable()
        .agent("RadioButton", self.label.clone(), Some(selected));
        if let Some(f) = self.on_select {
            c = c.on_click(move |_, cx| f(&(), cx));
        }
        c.into_element()
    }
}

#[cfg(test)]
mod ink_tests {
    /// A working button's label over its fill reads in the ink that's
    /// better over all of it, the darker bottom too: white on a purple
    /// as Platinum's, black on a bright blue.
    #[test]
    fn the_ink_over_a_fill() {
        assert_eq!(super::ink_over_fill([91, 91, 204, 255]), olo_theme::WHITE);
        assert_eq!(super::ink_over_fill([90, 170, 255, 255]), olo_theme::BLACK);
    }
}
