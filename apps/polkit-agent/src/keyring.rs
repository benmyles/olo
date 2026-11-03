//! The keyring's prompts (`olo_system::keyring_prompt`): unlock the
//! login keyring, choose a new keyring's password, or confirm. One window
//! per prompt; it stays up, "Checking…", until the keyring says it's done
//! or asks again (a wrong password comes back with a warning).

use olo_system::keyring_prompt::{Answer, Event, Kind, Props};
use olo_ui::{
    gpui::{
        AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
        px,
    },
    metrics as m, style, theme,
    widgets::{self, AlertFrame, Button, CheckBox, StringView, TextControl, TextField, TextFieldEvent, alert, button_row, paragraph, paragraph_height, text_field},
};

use super::{Agents, COLUMN, Cancel, DIVIDER};

pub const ICON: &str = "key";

pub struct Ask {
    pub id: u64,
    window: Option<AnyWindowHandle>,
    kind: Kind,
    props: Props,
    password: Entity<TextField>,
    confirm: Entity<TextField>,
    choice: bool,
    /// Our own complaint (the two passwords differ).
    error: String,
    /// Answered; waiting for the keyring.
    checking: bool,
    focus: FocusHandle,
}

/// The window's height: the title, the description, the fields and the
/// check box, the note under them, and the buttons.
fn height(cx: &App, kind: Kind, props: &Props) -> f32 {
    let mut h = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + m::LABEL_ROW + m::SPACING + m::CONTROL_HEIGHT;
    if !props.description.is_empty() {
        h += paragraph_height(cx, &props.description, COLUMN) + m::SPACING;
    }
    if kind == Kind::Password {
        h += (m::CONTROL_HEIGHT + m::SPACING) * if props.password_new { 2.0 } else { 1.0 };
    }
    if !props.choice_label.is_empty() {
        h += m::CONTROL_HEIGHT + m::SPACING;
    }
    h.ceil()
}

fn title(props: &Props) -> String {
    if props.title.is_empty() { "Keyring".to_owned() } else { props.title.clone() }
}

/// What the keyring asks or says.
pub fn event(cx: &mut App, e: Event) {
    match e {
        Event::Show { id, kind, props } => {
            let asks: Vec<Entity<Ask>> = cx.default_global::<Agents>().asks.values().cloned().collect();
            let existing = asks.into_iter().find(|a| a.read(cx).id == id);
            match existing {
                Some(ask) => ask.update(cx, |a, cx| a.again(kind, props, cx)),
                None => open(cx, id, kind, props),
            }
        }
        Event::Close(id) => {
            let asks: Vec<Entity<Ask>> = cx.default_global::<Agents>().asks.values().cloned().collect();
            let asks: Vec<Entity<Ask>> = asks.into_iter().filter(|a| a.read(cx).id == id).collect();
            for ask in asks {
                ask.update(cx, |a, cx| {
                    a.clear(cx);
                    if let Some(w) = a.window.take() {
                        cx.default_global::<Agents>().asks.remove(&w.window_id());
                        cx.close_window(w);
                    }
                });
            }
        }
    }
}

/// Opens a window for a new prompt.
fn open(cx: &mut App, id: u64, kind: Kind, props: Props) {
    let h = height(cx, kind, &props);
    // (Its Return its own: on to the second when there are two.)
    let password = text_field(cx, |f| f.name("Password").password(true).keeps_enter());
    let confirm = text_field(cx, |f| f.name("Confirm").password(true));
    let choice = props.choice_chosen;
    let window_title = title(&props);
    let ask = cx.new(|cx| {
        cx.subscribe(&password, |a: &mut Ask, _, e: &TextFieldEvent, cx| match e {
            // Return in the first of two: on to the second.
            TextFieldEvent::Entered if a.props.password_new => a.focus_confirm(cx),
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.notify(),
        })
        .detach();
        cx.subscribe(&confirm, |a: &mut Ask, _, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.notify(),
        })
        .detach();
        Ask { id, window: None, kind, props, password, confirm, choice, error: String::new(), checking: false, focus: cx.focus_handle() }
    });
    let Some(handle) = super::open(cx, &window_title, h, ask.clone()) else { return };
    cx.default_global::<Agents>().asks.insert(handle.window_id(), ask.clone());
    ask.update(cx, |a, cx| {
        a.window = Some(handle.into());
        a.focus_first(cx);
    });
}

impl Ask {
    /// The same prompt again (after a wrong password): fresh fields.
    fn again(&mut self, kind: Kind, props: Props, cx: &mut Context<Self>) {
        self.kind = kind;
        self.props = props;
        self.clear(cx);
        self.error.clear();
        self.checking = false;
        self.sync(cx);
        self.focus_first(cx);
        if let Some(w) = self.window {
            let _ = w.update(cx, |_, window, _| window.activate_window());
        }
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        self.confirm.update(cx, |f, cx| f.clear_secret(cx));
    }

    /// The fields take typing unless the keyring is checking.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let ready = !self.checking;
        self.password.update(cx, |f, cx| f.set_enabled(ready, cx));
        self.confirm.update(cx, |f, cx| f.set_enabled(ready, cx));
    }

    fn focus_on(&self, handle: FocusHandle, cx: &mut App) {
        if let Some(w) = self.window {
            let _ = w.update(cx, |_, window, _| window.focus(&handle));
        }
    }

    fn focus_first(&mut self, cx: &mut Context<Self>) {
        let handle = if self.kind == Kind::Password { self.password.focus_handle(cx) } else { self.focus.clone() };
        self.focus_on(handle, cx);
    }

    fn focus_confirm(&mut self, cx: &mut Context<Self>) {
        let handle = self.confirm.focus_handle(cx);
        self.focus_on(handle, cx);
    }

    fn can_answer(&self, cx: &App) -> bool {
        !self.checking && (self.kind == Kind::Confirm || !self.password.read(cx).text().is_empty() || self.props.password_new)
    }

    /// Answers the keyring (it stays up, Checking…, until it's done).
    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_answer(cx) {
            return;
        }
        let answer = match self.kind {
            Kind::Confirm => Answer::Continue,
            Kind::Password => {
                if self.props.password_new && self.password.read(cx).text() != self.confirm.read(cx).text() {
                    self.error = "The passwords don't match.".into();
                    self.confirm.update(cx, |f, cx| f.clear_secret(cx));
                    self.focus_confirm(cx);
                    cx.notify();
                    return;
                }
                Answer::Password(self.password.read(cx).text().to_owned())
            }
        };
        self.clear(cx);
        self.error.clear();
        self.checking = true;
        self.sync(cx);
        self.send(answer, cx);
        cx.notify();
    }

    fn send(&self, answer: Answer, cx: &mut App) {
        if let Some(p) = &cx.default_global::<Agents>().prompter {
            p.answer(self.id, answer, self.choice);
        }
    }

    fn cancel_ask(&mut self, cx: &mut Context<Self>) {
        self.checking = true;
        self.sync(cx);
        self.send(Answer::Cancel, cx);
        cx.notify();
    }
}

impl Cancel for Ask {
    /// The window stays until the keyring is done with the prompt.
    fn cancel(&mut self, cx: &mut Context<Self>) -> bool {
        self.cancel_ask(cx);
        false
    }
}

impl Render for Ask {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let view = cx.entity();
        let message = if self.props.message.is_empty() { "Authentication required".to_owned() } else { self.props.message.clone() };
        let mut frame = AlertFrame::new(ICON).child(widgets::alert_title(message)).child(div().h(px(m::TIGHT)));
        if !self.props.description.is_empty() {
            frame = frame.child(paragraph(SharedString::from(self.props.description.clone()))).child(div().h(px(m::SPACING)));
        }
        let mut form = div().flex().flex_col().gap(px(m::SPACING));
        if self.kind == Kind::Password {
            form = form.child(TextControl::new(self.password.clone()).label("Password:").divider(DIVIDER));
            if self.props.password_new {
                form = form.child(TextControl::new(self.confirm.clone()).label("Confirm:").divider(DIVIDER));
            }
        }
        if !self.props.choice_label.is_empty() {
            let v = view.clone();
            form = form.child(CheckBox::new(self.props.choice_label.clone(), self.choice).enabled(!self.checking).on_toggle(move |on, _, cx| {
                v.update(cx, |a, cx| {
                    a.choice = on;
                    cx.notify();
                })
            }));
        }
        let note = if self.checking {
            Some(("Checking…".to_owned(), style::disabled_text(&p)))
        } else if !self.error.is_empty() {
            Some((self.error.clone(), style::c(p.failure)))
        } else if !self.props.warning.is_empty() {
            Some((self.props.warning.clone(), style::c(p.failure)))
        } else {
            None
        };
        if let Some((note, color)) = note {
            form = form.child(StringView::new(note).color(color).height(m::LABEL_ROW));
        }
        let go = if self.props.continue_label.is_empty() { "Continue".to_owned() } else { self.props.continue_label.clone() };
        let stop = if self.props.cancel_label.is_empty() { "Cancel".to_owned() } else { self.props.cancel_label.clone() };
        let can = self.can_answer(cx);
        let (cancel, submit) = (view.clone(), view);
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|a, _: &alert::Cancel, _, cx| a.cancel_ask(cx)))
            .on_action(cx.listener(|a, _: &alert::Default, _, cx| a.submit(cx)))
            .child(
                frame.child(form).child(div().flex_1()).child(
                    button_row()
                        .child(Button::new(stop).on_click(move |_, cx| cancel.update(cx, |a, cx| a.cancel_ask(cx))))
                        .child(Button::new(go).default(true).enabled(can).on_click(move |_, cx| submit.update(cx, |a, cx| a.submit(cx)))),
                ),
            )
    }
}
