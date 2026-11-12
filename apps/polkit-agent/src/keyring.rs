//! The keyring's prompts (`olo_system::keyring_prompt`): unlock the
//! login keyring, choose a new keyring's password, or confirm. One window
//! per prompt; it stays up, "Checking…", until the keyring says it's done
//! or asks again (a wrong password comes back with a warning).

use olo_kit::{
    Child, Cx, Element, FocusHandle, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, WindowOptions, div, metrics as m, px, style, theme,
    widgets::{self, AlertFrame, Button, CheckBox, StringView, TextControl, TextField, TextFieldEvent, button_row, paragraph, paragraph_height, text_field},
};
use olo_system::keyring_prompt::{Answer, Event, Kind, Props};

use super::{Agents, COLUMN, DIVIDER, Msg, PROMPTER, WIDTH};

pub const ICON: &str = "key";

/// What a prompt's window is told.
pub enum AskMsg {
    /// It has opened: its first field has the keyboard.
    Start,
    /// The same prompt again (after a wrong password).
    Again(Kind, Box<Props>),
    /// The keyring is done with the prompt.
    Close,
    Choice(bool),
}

pub struct Ask {
    pub id: u64,
    kind: Kind,
    props: Props,
    password: Child<TextField>,
    confirm: Child<TextField>,
    choice: bool,
    /// Our own complaint (the two passwords differ).
    error: String,
    /// Answered; waiting for the keyring.
    checking: bool,
    focus: FocusHandle,
}

/// The window's height: the title, the description, the fields and the
/// check box, the note under them, and the buttons.
fn height(kind: Kind, props: &Props) -> f32 {
    let mut h = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + m::LABEL_ROW + m::SPACING + m::CONTROL_HEIGHT;
    if !props.description.is_empty() {
        h += paragraph_height(&props.description, COLUMN) + m::SPACING;
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
pub fn event(agents: &mut Agents, e: Event) {
    match e {
        Event::Show { id, kind, props } => match agents.asks.values().find(|(ask, _)| *ask == id) {
            Some((_, window)) => {
                window.messenger().send(AskMsg::Again(kind, Box::new(props)));
            }
            None => {
                let h = height(kind, &props);
                let window = olo_kit::open_window(WindowOptions::alert(title(&props), WIDTH, h), move |cx| Ask::new(id, kind, props, cx));
                agents.asks.insert(window.id(), (id, window));
            }
        },
        Event::Close(id) => {
            for (_, window) in agents.asks.values().filter(|(ask, _)| *ask == id) {
                window.messenger().send(AskMsg::Close);
            }
            agents.asks.retain(|_, (ask, _)| *ask != id);
        }
    }
}

impl Ask {
    fn new(id: u64, kind: Kind, props: Props, cx: &mut Cx<Self>) -> Ask {
        // (Its Return its own: on to the second when there are two.)
        let password = text_field(cx, |f| f.name("Password").password(true).keeps_enter());
        let confirm = text_field(cx, |f| f.name("Confirm").password(true));
        cx.subscribe(&password, |a: &mut Ask, e: &TextFieldEvent, cx| match e {
            // Return in the first of two: on to the second.
            TextFieldEvent::Entered if a.props.password_new => a.confirm.focus(cx),
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.invalidate(),
        });
        cx.subscribe(&confirm, |a: &mut Ask, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => a.submit(cx),
            _ => cx.invalidate(),
        });
        cx.send(AskMsg::Start);
        Ask { id, kind, choice: props.choice_chosen, props, password, confirm, error: String::new(), checking: false, focus: FocusHandle::new() }
    }

    fn clear(&mut self, cx: &mut Cx<Self>) {
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        self.confirm.update(cx, |f, cx| f.clear_secret(cx));
    }

    /// The fields take typing unless the keyring is checking.
    fn sync(&mut self, cx: &mut Cx<Self>) {
        let ready = !self.checking;
        self.password.update(cx, |f, cx| f.set_enabled(ready, cx));
        self.confirm.update(cx, |f, cx| f.set_enabled(ready, cx));
    }

    fn focus_first(&mut self, cx: &mut Cx<Self>) {
        if self.kind == Kind::Password { self.password.focus(cx) } else { self.focus.focus(cx) }
    }

    fn can_answer(&self, cx: &Cx<Self>) -> bool {
        !self.checking && (self.kind == Kind::Confirm || !self.password.read(cx).text().is_empty() || self.props.password_new)
    }

    /// Answers the keyring (it stays up, Checking…, until it's done).
    fn submit(&mut self, cx: &mut Cx<Self>) {
        if !self.can_answer(cx) {
            return;
        }
        let answer = match self.kind {
            Kind::Confirm => Answer::Continue,
            Kind::Password => {
                if self.props.password_new && self.password.read(cx).text() != self.confirm.read(cx).text() {
                    self.error = "The passwords don't match.".into();
                    self.confirm.update(cx, |f, cx| f.clear_secret(cx));
                    self.confirm.focus(cx);
                    cx.invalidate();
                    return;
                }
                Answer::Password(self.password.read(cx).text().to_owned())
            }
        };
        self.clear(cx);
        self.error.clear();
        self.checking = true;
        self.sync(cx);
        self.send(answer);
        cx.invalidate();
    }

    fn send(&self, answer: Answer) {
        if let Some(p) = PROMPTER.get() {
            p.answer(self.id, answer, self.choice);
        }
    }

    fn cancel_ask(&mut self, cx: &mut Cx<Self>) {
        self.checking = true;
        self.sync(cx);
        self.send(Answer::Cancel);
        cx.invalidate();
    }
}

impl View for Ask {
    type Message = AskMsg;

    fn message(&mut self, message: AskMsg, cx: &mut Cx<Self>) {
        match message {
            AskMsg::Start => {
                cx.activate_window();
                self.focus_first(cx);
            }
            // The same prompt again: fresh fields.
            AskMsg::Again(kind, props) => {
                self.kind = kind;
                self.props = *props;
                self.clear(cx);
                self.error.clear();
                self.checking = false;
                self.sync(cx);
                self.focus_first(cx);
                cx.activate_window();
            }
            AskMsg::Close => {
                self.clear(cx);
                return cx.close_window();
            }
            AskMsg::Choice(on) => self.choice = on,
        }
        cx.invalidate();
    }

    /// Closed from its tab: as its Cancel. The window stays until the
    /// keyring is done with the prompt.
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        self.cancel_ask(cx);
        false
    }

    /// (However it closes, the agent forgets it.)
    fn window_closing(&mut self, cx: &mut Cx<Self>) {
        olo_kit::proxy().send(Msg::Closed(cx.window_id()));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let message = if self.props.message.is_empty() { "Authentication required".to_owned() } else { self.props.message.clone() };
        let mut frame = AlertFrame::new(ICON).child(widgets::alert_title(message)).child(div().h(px(m::TIGHT)));
        if !self.props.description.is_empty() {
            frame = frame.child(paragraph(SharedString::from(self.props.description.clone()))).child(div().h(px(m::SPACING)));
        }
        let mut form = div().flex().flex_col().gap(px(m::SPACING));
        if self.kind == Kind::Password {
            form = form.child(TextControl::new(cx, &self.password).label("Password:").divider(DIVIDER));
            if self.props.password_new {
                form = form.child(TextControl::new(cx, &self.confirm).label("Confirm:").divider(DIVIDER));
            }
        }
        if !self.props.choice_label.is_empty() {
            form = form.child(CheckBox::new(self.props.choice_label.clone(), self.choice).enabled(!self.checking).on_toggle(cx.msg_with(|on: &bool| AskMsg::Choice(*on))));
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
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|a: &mut Self, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    a.cancel_ask(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                frame.child(form).child(div().flex_1()).child(
                    button_row()
                        .child(Button::new(stop).on_click(cx.listener(|a: &mut Self, _, cx| a.cancel_ask(cx))))
                        .child(Button::new(go).default(true).enabled(can).on_click(cx.listener(|a: &mut Self, _, cx| a.submit(cx)))),
                ),
            )
            .into_element()
    }
}
