//! BAlert: a darker stripe down the left with
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
            .child(div().absolute().left(px(ALERT_TEXT_X)).right(px(m::WINDOW_INSET)).top(px(m::WINDOW_INSET)).bottom(px(m::WINDOW_INSET)).flex().flex_col().children(self.children))
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

/// What a password alert can be answered with besides typing, said under
/// its field (sudo's window, the polkit agent): a finger on the
/// fingerprint sensor (PAM listening for it), a touch of a security key
/// set up, and whether the password can be typed now or only once the
/// sensor gives up. None: only typing.
pub fn ways_in(finger: bool, key: bool, typing: bool) -> Option<&'static str> {
    Some(match (finger, key, typing) {
        (true, true, true) => WAYS_IN[0],
        (true, true, false) => WAYS_IN[1],
        (true, false, true) => WAYS_IN[2],
        (true, false, false) => WAYS_IN[3],
        (false, true, _) => WAYS_IN[4],
        (false, false, _) => return None,
    })
}

/// Every note [`ways_in`] gives, to tell its own from PAM's messages.
pub const WAYS_IN: [&str; 5] = [
    "Touch the fingerprint sensor or your security key, or type your password.",
    "Touch the fingerprint sensor or your security key, or wait to type your password.",
    "Touch the fingerprint sensor, or type your password.",
    "Touch the fingerprint sensor, or wait to type your password.",
    // (olo_fido::TOUCH_OR_TYPE, as Passwords and the lock screen say it.)
    "Touch your security key, or type your password.",
];

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
    let text = text.into();
    // Agents read it as a label (GPUI's text says nothing of itself).
    let said = text.clone();
    let say = canvas(|_, _, _| {}, move |b, _, window, cx| crate::agent::record(window, cx, crate::agent::Node::label(said.clone(), b))).absolute().inset_0();
    div().relative().w_full().line_height(gpui::relative(style::LINE_HEIGHT)).child(text).child(say)
}

/// The height of `text` wrapped to `width` in the plain font, for sizing
/// a window before it opens.
pub fn paragraph_height(cx: &App, text: &str, width: f32) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let mut wrapper = cx.text_system().line_wrapper(font, px(style::PLAIN_SIZE));
    let lines: usize = text.split('\n').map(|line| 1 + wrapper.wrap_line(&[gpui::LineFragment::text(line)], px(width)).count()).sum();
    (lines as f32 * style::PLAIN_SIZE * style::LINE_HEIGHT).ceil()
}

/// How wide an alert with `buttons` side by side must be (at least
/// [`ALERT_WIDTH`]): long labels widen it rather than run off its left.
pub fn alert_width(cx: &App, buttons: &[&str]) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let each = |label: &str| ((style::text_width_app(cx, label, style::PLAIN_SIZE, &font) + 2.0 * m::CONTROL_PAD).max(m::BUTTON_MIN_WIDTH) / 2.0).ceil() * 2.0;
    let row = buttons.iter().map(|b| each(b)).sum::<f32>() + (m::SPACING + 2.0) * buttons.len().saturating_sub(1) as f32;
    ((ALERT_TEXT_X + row + m::WINDOW_INSET + 4.0).ceil()).max(ALERT_WIDTH)
}

/// How tall an alert of `width` with `message` must be.
pub fn alert_height(cx: &App, message: &str, width: f32) -> f32 {
    let text = paragraph_height(cx, message, width - ALERT_TEXT_X - m::WINDOW_INSET);
    (m::WINDOW_INSET + text.max(ALERT_ICON) + m::WINDOW_INSET + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil()
}

type OnChoose = std::rc::Rc<dyn Fn(Option<usize>, &mut App)>;

/// An alert that answers itself unless answered in time
/// ([`open_timed_alert`]): new display settings, put back unless kept.
#[derive(Clone)]
pub struct Timeout {
    /// When it answers itself. Set it later to give more time (what it
    /// asks about changed again while it asked).
    pub at: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
    /// The button it answers with then.
    pub choice: usize,
    /// The line under the message, given the seconds left (whole, rounded
    /// up): "Reverting in 12 seconds."
    pub says: std::rc::Rc<dyn Fn(u64) -> String>,
}

impl Timeout {
    /// The seconds left now, rounded up (0: time's up).
    fn left(&self) -> u64 {
        let left = self.at.get().saturating_duration_since(std::time::Instant::now());
        left.as_millis().div_ceil(1000) as u64
    }
}

/// An alert's window: its message and buttons.
struct Alert {
    icon: SharedString,
    message: SharedString,
    buttons: Vec<SharedString>,
    on_choose: OnChoose,
    focus: gpui::FocusHandle,
    /// Answered (or closed): later clicks and keys do nothing.
    done: bool,
    /// It answers itself in time, and a wake for its next second is set.
    timeout: Option<Timeout>,
    ticking: bool,
}

impl Alert {
    fn choose(&mut self, choice: Option<usize>, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        (self.on_choose)(choice, cx);
        window.remove_window();
    }
}

impl gpui::Render for Alert {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::InteractiveElement as _;
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let n = self.buttons.len();
        let view = cx.entity();
        let mut row = button_row();
        for (i, label) in self.buttons.iter().enumerate() {
            let view = view.clone();
            row = row.child(super::Button::new(label.clone()).default(i + 1 == n).on_click(move |window, cx| view.update(cx, |a, cx| a.choose(Some(i), window, cx))));
        }
        div()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            // Return takes the last; Escape the first when there's more
            // than one (usually Cancel).
            .on_action(cx.listener(move |a, _: &Default, window, cx| a.choose(Some(n.saturating_sub(1)), window, cx)))
            .on_action(cx.listener(move |a, _: &Cancel, window, cx| {
                if n > 1 {
                    a.choose(Some(0), window, cx)
                }
            }))
            .child(AlertFrame::new(self.icon.clone()).child(paragraph(self.message.clone())).children(self.countdown(window, cx)).child(div().flex_1()).child(row))
    }
}

impl Alert {
    /// A timed alert's line counting down, with a wake set for when it
    /// next changes (once a second while it's open, nothing between):
    /// time up, it answers itself.
    fn countdown(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = self.timeout.clone()?;
        let left = t.left();
        if !self.ticking && !self.done {
            self.ticking = true;
            // Until the seconds left go down by one (or at once, time up).
            let wait = t.at.get().saturating_duration_since(std::time::Instant::now()).saturating_sub(std::time::Duration::from_secs(left.saturating_sub(1)));
            cx.spawn_in(window, async move |alert, cx| {
                cx.background_executor().timer(wait).await;
                let _ = alert.update_in(cx, |a, window, cx| {
                    a.ticking = false;
                    match &a.timeout {
                        Some(t) if t.left() == 0 => a.choose(Some(t.choice), window, cx),
                        _ => cx.notify(),
                    }
                });
            })
            .detach();
        }
        Some(div().pt(px(m::SPACING)).child(paragraph((t.says)(left.max(1)))))
    }
}

/// Opens a BAlert: `message` beside `icon`, `buttons` along the bottom
/// (the last the default, Return; Escape the first when there's more than
/// one); `on_choose` gets the one chosen, or None if it was closed from
/// its tab.
pub fn open_alert(cx: &mut App, title: &str, icon: &str, message: &str, buttons: &[&str], on_choose: impl Fn(Option<usize>, &mut App) + 'static) -> Option<gpui::WindowHandle<crate::Frame>> {
    open(cx, title, icon, message, buttons, None, std::rc::Rc::new(on_choose))
}

/// Opens a BAlert as [`open_alert`] does that answers itself with
/// `timeout.choice` when its time is up, the seconds left counting down
/// under the message ("Reverting in 12 seconds."): a change that may
/// have left the person unable to answer (a display gone dark), undone
/// unless they keep it. `on_choose` gets the choice either way.
pub fn open_timed_alert(
    cx: &mut App,
    title: &str,
    icon: &str,
    message: &str,
    buttons: &[&str],
    timeout: Timeout,
    on_choose: impl Fn(Option<usize>, &mut App) + 'static,
) -> Option<gpui::WindowHandle<crate::Frame>> {
    open(cx, title, icon, message, buttons, Some(timeout), std::rc::Rc::new(on_choose))
}

fn open(cx: &mut App, title: &str, icon: &str, message: &str, buttons: &[&str], timeout: Option<Timeout>, on_choose: OnChoose) -> Option<gpui::WindowHandle<crate::Frame>> {
    use gpui::AppContext as _;
    let width = alert_width(cx, buttons);
    // The countdown's line under the message, as long as it gets.
    let shown = match &timeout {
        Some(t) => format!("{message}\n{}", (t.says)(t.left().max(1))),
        None => message.to_owned(),
    };
    let height = alert_height(cx, &shown, width) + if timeout.is_some() { m::SPACING } else { 0.0 };
    let alert = cx.new(|cx| Alert {
        icon: SharedString::from(icon.to_owned()),
        message: SharedString::from(message.to_owned()),
        buttons: buttons.iter().map(|b| SharedString::from(b.to_string())).collect(),
        on_choose: on_choose.clone(),
        focus: cx.focus_handle(),
        done: false,
        timeout,
        ticking: false,
    });
    let a = alert.clone();
    crate::open_window(cx, crate::WindowOptions::alert(title.to_owned(), width, height), move |window, cx| {
        window.activate_window();
        let closing = a.clone();
        window.on_window_should_close(cx, move |_, cx| {
            closing.update(cx, |a, cx| {
                if !std::mem::replace(&mut a.done, true) {
                    (a.on_choose)(None, cx);
                }
            });
            true
        });
        a
    })
}

/// What's to become of changes not saved yet, as [`ask_to_save`] was
/// answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsaved {
    Save,
    DontSave,
}

/// Asks whether to save the changes to `name` before `doing` ("closing",
/// "reverting", "moving on"): the same question with the same buttons in
/// every app, Cancel, Don't Save and Save (the default), as BeOS's
/// StyledEdit asked it. `then` hears Save or Don't Save, or None for
/// Cancel, Escape or the alert closed (carry on as if nothing was
/// asked). The alert's window, for an app that brings it forward when
/// asked again.
pub fn ask_to_save(cx: &mut App, app: &str, name: &str, doing: &str, then: impl Fn(Option<Unsaved>, &mut App) + 'static) -> Option<gpui::WindowHandle<crate::Frame>> {
    let message = format!("Save changes to “{name}” before {doing}?");
    open_alert(cx, app, "warning", &message, &["Cancel", "Don't Save", "Save"], move |choice, cx| {
        then(
            match choice {
                Some(1) => Some(Unsaved::DontSave),
                Some(2) => Some(Unsaved::Save),
                _ => None,
            },
            cx,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        rc::Rc,
        time::{Duration, Instant},
    };

    use super::Timeout;

    fn timeout(left: Duration) -> Timeout {
        Timeout { at: Rc::new(Cell::new(Instant::now() + left)), choice: 0, says: Rc::new(|n| format!("{n}")) }
    }

    #[test]
    fn the_seconds_left_round_up_and_end_at_nought() {
        assert_eq!(timeout(Duration::from_millis(14_200)).left(), 15);
        assert_eq!(timeout(Duration::from_millis(900)).left(), 1);
        assert_eq!(timeout(Duration::ZERO).left(), 0);
        // More time given while it asks.
        let t = timeout(Duration::from_millis(2_500));
        t.at.set(Instant::now() + Duration::from_secs(10) + Duration::from_millis(500));
        assert_eq!(t.left(), 11);
    }
}
