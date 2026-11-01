//! BAlert, as the egui kit draws it: a darker stripe down the left with
//! the icon straddling it, the content in a column beside it, and the
//! buttons along the bottom right (the last the default). For alerts
//! with fields (a password), [`Conversation`] keeps PAM's side of it and
//! [`ConversationField`] shows it.

use gpui::{AnyElement, App, Bounds, Entity, FontWeight, IntoElement, KeyBinding, ParentElement, RenderOnce, SharedString, Styled as _, Window, actions, canvas, div, point, px, size};
use olo_kit_base::metrics as m;
use smallvec::SmallVec;

use super::{TextControl, TextField};
use crate::{icons, style, theme};

actions!(olo_alert, [Cancel, Default]);

/// The key context an alert's root takes ([`bind_keys`]).
pub const CONTEXT: &str = "OloAlert";

/// An alert's keys: Escape cancels, Return takes the default button (a
/// field that has the keyboard takes Return first).
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Cancel, Some(CONTEXT)), KeyBinding::new("enter", Default, Some(CONTEXT))]);
}

/// Width of a BAlert window, and of its tinted stripe down the left.
pub const ALERT_WIDTH: f32 = 380.0;
const ALERT_STRIPE: f32 = 30.0;
/// The size of an alert's icon.
pub const ALERT_ICON: f32 = 32.0;
/// The text column: from the stripe's edge, past the icon over it.
pub const ALERT_TEXT_X: f32 = ALERT_STRIPE + ALERT_ICON / 2.0 + m::WINDOW_INSET;

/// The backdrop of an alert: the stripe with `icon` straddling it, what
/// it holds in the column beside it (top down, within the window's
/// margins).
#[derive(IntoElement)]
pub struct AlertFrame {
    icon: SharedString,
    children: SmallVec<[AnyElement; 8]>,
}

impl AlertFrame {
    pub fn new(icon: impl Into<SharedString>) -> AlertFrame {
        AlertFrame { icon: icon.into(), children: SmallVec::new() }
    }
}

impl ParentElement for AlertFrame {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for AlertFrame {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let icon = self.icon.clone();
        let backdrop = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let stripe = Bounds::from_corners(b.origin, point(b.left() + px(ALERT_STRIPE), b.bottom()));
                style::fill_rect(window, stripe, style::shade(&p, p.panel_background, 1.1));
                if !icon.is_empty() {
                    let at = Bounds { origin: point(b.left() + px(ALERT_STRIPE - ALERT_ICON / 2.0), b.top() + px(m::WINDOW_INSET)), size: size(px(ALERT_ICON), px(ALERT_ICON)) };
                    icons::paint(window, at, &icon);
                }
            },
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .size_full()
            .child(backdrop)
            .child(
                div()
                    .absolute()
                    .left(px(ALERT_TEXT_X))
                    .right(px(m::WINDOW_INSET))
                    .top(px(m::WINDOW_INSET))
                    .bottom(px(m::WINDOW_INSET))
                    .flex()
                    .flex_col()
                    .children(self.children),
            )
    }
}

/// Buttons along the bottom right, in reading order (the default last),
/// as BeOS sets them.
pub fn button_row() -> gpui::Div {
    div().flex().flex_row().justify_end().items_center().gap(px(m::SPACING + 2.0))
}

/// An alert's title: one line, bold.
pub fn alert_title(text: impl Into<SharedString>) -> impl IntoElement {
    super::StringView::new(text).bold(true).height(m::LABEL_ROW)
}

/// Where a password alert's conversation with PAM is (polkit's helper,
/// sudo): what it asks and what it last said. The answer is typed in a
/// [`TextField`] (a password one) beside it.
#[derive(Default)]
pub struct Conversation {
    /// PAM is waiting for an answer: (prompt, echo).
    pub prompt: Option<(String, bool)>,
    /// The latest message from PAM ("Place your finger on the reader").
    pub info: String,
    pub error: String,
    /// An answer is with PAM.
    pub checking: bool,
}

impl Conversation {
    /// An answer can be typed.
    pub fn ready(&self) -> bool {
        self.prompt.is_some() && !self.checking
    }

    /// PAM asks (again).
    pub fn ask(&mut self, prompt: String, echo: bool) {
        self.checking = false;
        self.prompt = Some((prompt, echo));
    }

    /// The answer has gone to PAM: Checking… until it says.
    pub fn sent(&mut self) {
        self.prompt = None;
        self.checking = true;
        self.error.clear();
        self.info.clear();
    }

    /// The field's label: PAM's prompt, ending in a colon.
    pub fn label(&self) -> String {
        self.prompt.as_ref().map_or("Password:".to_owned(), |(t, _)| {
            let t = t.trim();
            if t.ends_with(':') { t.to_owned() } else { format!("{t}:") }
        })
    }

    /// What shows under the field: Checking…, an error, or PAM's message,
    /// and whether it's an error.
    pub fn note(&self) -> Option<(String, bool)> {
        if self.checking {
            Some(("Checking…".to_owned(), false))
        } else if !self.error.is_empty() {
            Some((self.error.clone(), true))
        } else if !self.info.is_empty() {
            Some((self.info.clone(), false))
        } else {
            None
        }
    }
}

/// A password alert's answer field, labelled with PAM's prompt, with the
/// note under it (a line, dim, or the failure colour for an error).
#[derive(IntoElement)]
pub struct ConversationField {
    field: Entity<TextField>,
    label: String,
    note: Option<(String, bool)>,
    divider: f32,
}

impl ConversationField {
    /// The answer typed in `field`; keep `field` enabled only while
    /// `conv.ready()` and a password unless PAM's prompt echoes.
    pub fn new(conv: &Conversation, field: Entity<TextField>, divider: f32) -> ConversationField {
        ConversationField { field, label: conv.label(), note: conv.note(), divider }
    }

    /// A note of the alert's own instead (the passwords don't match).
    pub fn note(mut self, note: Option<(String, bool)>) -> Self {
        if note.is_some() {
            self.note = note;
        }
        self
    }
}

impl RenderOnce for ConversationField {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = theme::palette(cx);
        let mut col = div().flex().flex_col().gap(px(m::SPACING)).child(TextControl::new(self.field).label(self.label).divider(self.divider));
        if let Some((note, error)) = self.note {
            let color = if error { style::c(p.failure) } else { style::disabled_text(&p) };
            col = col.child(super::StringView::new(note).color(color).height(m::LABEL_ROW));
        }
        col
    }
}

/// A paragraph wrapped to the width it's given, in the plain font (an
/// alert's message, a description).
pub fn paragraph(text: impl Into<SharedString>) -> gpui::Div {
    div().w_full().line_height(gpui::relative(style::LINE_HEIGHT)).child(text.into())
}

/// The height of `text` wrapped to `width` in the plain font, for sizing
/// a window before it opens.
pub fn paragraph_height(cx: &App, text: &str, width: f32) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let mut wrapper = cx.text_system().line_wrapper(font, px(style::PLAIN_SIZE));
    let lines: usize = text
        .split('\n')
        .map(|line| 1 + wrapper.wrap_line(&[gpui::LineFragment::text(line)], px(width)).count())
        .sum();
    (lines as f32 * style::PLAIN_SIZE * style::LINE_HEIGHT).ceil()
}
