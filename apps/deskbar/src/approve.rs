//! Approving requests without asking, for a while (app_server's
//! approve.rs): the key's menu, which is in the Olo menu too, and the
//! window that turns it on, which takes the account password (Terminal
//! answers sudo with it, so Touch ID can't stand in for it).

use olo_kit::{
    Child, ClickEvent, Cx, Element, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, ParentElement as _, Styled as _, View, WindowHandle, WindowOptions, div,
    metrics as m, px, style,
    widgets::{self, AlertFrame, Button, Conversation, ConversationField, MenuField, TextField, TextFieldEvent, button_row, paragraph, paragraph_height, text_field},
};
use olo_protocol::{ClientError, ClockSettings, Request};
use zeroize::Zeroize;

use crate::status;

/// The lengths offered: in the menu, in the window, and in minutes
/// (app_server takes 5 to a day).
pub const LENGTHS: [(&str, &str, u32); 5] =
    [("For 5 Minutes", "5 minutes", 5), ("For 15 Minutes", "15 minutes", 15), ("For 1 Hour", "1 hour", 60), ("For 4 Hours", "4 hours", 240), ("For a Day", "a day", 24 * 60)];

const WIDTH: f32 = 420.0;
const ICON: &str = "key";
const TITLE: &str = "Approve Without Asking";
const MESSAGE: &str = "While this is on, sudo in Terminal gets your password, and Passwords unlocks and lets op in, without asking you, for people and agents alike. Each is still shown in a quiet notification. It ends at its time, or when the screen locks.";
/// Text column: the alert's width less its stripe, icon and margins.
const COLUMN: f32 = WIDTH - widgets::ALERT_TEXT_X - m::WINDOW_INSET;

/// "12 min", "3 h 5 min": what is left until `until` (seconds since the
/// epoch).
pub fn left(until: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let minutes = until.saturating_sub(now).div_ceil(60).max(1);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The key's menu: how long (with the password, unless it is on already,
/// when only the length changes), or ask again.
pub fn menu(until: Option<u64>, clock: &ClockSettings) -> Menu {
    let mut menu = Menu::new(TITLE);
    for (label, _, minutes) in LENGTHS {
        let label = if until.is_some() { label.to_owned() } else { format!("{label}…") };
        menu = menu.item(label, format!("approve:{minutes}"));
    }
    let label = match until {
        Some(t) => format!("Ask Again ({} left, until {})", left(t), status::time_of_day(t, clock)),
        None => "Ask Again".into(),
    };
    menu.separator().item(label, "approve:off").enabled(until.is_some()).separator().item("Security Preferences…", "pref:security")
}

/// app_server's answer to turning it on.
pub struct Answer(pub Result<(), String>);

/// What the window is told.
pub enum Msg {
    /// Chosen again from the menu: that length, and the window forward.
    Choose(u32),
    Answer(Answer),
    /// A length picked in the window.
    Length(usize),
    Cancel,
    Submit,
}

/// The window turning it on.
pub struct Ask {
    length: usize,
    conv: Conversation,
    field: Child<TextField>,
    focus: FocusHandle,
}

impl Ask {
    /// Opens the window, `minutes` chosen.
    pub fn open(minutes: u32) -> WindowHandle<Ask> {
        let h = height();
        olo_kit::open_window(WindowOptions::alert(TITLE.to_owned(), WIDTH, h), move |cx| {
            let field = text_field(cx, |f| f.name("Password").password(true));
            cx.subscribe(&field, |a: &mut Ask, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => a.submit(cx),
                _ => cx.invalidate(),
            });
            let mut conv = Conversation::default();
            conv.ask("Password".into(), false);
            let length = LENGTHS.iter().position(|l| l.2 == minutes).unwrap_or(1);
            cx.activate_window();
            field.focus(cx);
            Ask { length, conv, field, focus: FocusHandle::new() }
        })
    }

    fn can_submit(&self, cx: &Cx<Self>) -> bool {
        self.conv.ready() && !self.field.read(cx).text().is_empty()
    }

    /// To app_server, which checks the password (on a thread here: that
    /// takes PAM a moment).
    fn submit(&mut self, cx: &mut Cx<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let password = self.field.read(cx).text().to_owned();
        self.field.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_enabled(false, cx);
        });
        self.conv.sent();
        let minutes = LENGTHS[self.length].2;
        let me = cx.messenger();
        std::thread::spawn(move || {
            let mut req = Request::SetAutoApprove { minutes: Some(minutes), password: Some(password) };
            let answer = olo_protocol::Client::connect().and_then(|mut c| c.request(&req)).map(|_| ()).map_err(|e| match e {
                ClientError::Server(message) => message,
                e => e.to_string(),
            });
            if let Request::SetAutoApprove { password: Some(p), .. } = &mut req {
                p.zeroize();
            }
            me.send(Msg::Answer(Answer(answer)));
        });
        cx.invalidate();
    }

    /// app_server answered: on (the window goes), or not (why, and the
    /// password asked for again).
    fn answer(&mut self, answer: Answer, cx: &mut Cx<Self>) {
        match answer.0 {
            Ok(()) => self.close(cx),
            Err(e) => {
                self.conv.error = e;
                self.conv.ask("Password".into(), false);
                self.field.update(cx, |f, cx| f.set_enabled(true, cx));
                self.field.focus(cx);
                cx.invalidate();
            }
        }
    }

    /// Closes the window (cancelled, or on): what was typed goes with it.
    fn close(&mut self, cx: &mut Cx<Self>) {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        cx.close_window();
    }
}

impl View for Ask {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Choose(minutes) => {
                if let Some(i) = LENGTHS.iter().position(|l| l.2 == minutes) {
                    self.length = i;
                }
                cx.activate_window();
                cx.invalidate();
            }
            Msg::Answer(answer) => self.answer(answer, cx),
            Msg::Length(i) => {
                self.length = i;
                cx.invalidate();
            }
            Msg::Cancel => self.close(cx),
            Msg::Submit => self.submit(cx),
        }
    }

    /// The window is going: what was typed goes with it.
    fn window_closing(&mut self, cx: &mut Cx<Self>) {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let font = style::ui_font(FontWeight::NORMAL);
        let divider = ["For:", "Password:"].iter().map(|l| style::text_width_app(l, style::PLAIN_SIZE, &font).ceil()).fold(0.0, f32::max) + m::LABEL_GAP;
        let names: Vec<olo_kit::SharedString> = LENGTHS.iter().map(|l| l.1.into()).collect();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(MenuField::new("approve-length", names, self.length).label("For:").divider(divider).width(COLUMN).enabled(!self.conv.checking).on_change(cx.msg_with(|i: &usize| Msg::Length(*i))))
            .child(ConversationField::new(cx, &self.conv, &self.field, divider));
        let can = self.can_submit(cx);
        // (The keyboard stays in the window: Return and Escape are its,
        // whatever was last clicked.)
        self.focus.focus_if_none(cx);
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|a: &mut Self, e: &KeyDownEvent, cx| {
                if e.keystroke.modifiers.modified() {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "escape" => a.close(cx),
                    "enter" => a.submit(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                AlertFrame::new(ICON).child(widgets::alert_title(TITLE)).child(div().h(px(m::TIGHT))).child(paragraph(MESSAGE)).child(div().h(px(m::SPACING))).child(form).child(div().flex_1()).child(
                    button_row()
                        .child(Button::new("Cancel").on_click(cx.msg_with(|_: &ClickEvent| Msg::Cancel)))
                        .child(Button::new("Turn On").default(true).enabled(can).on_click(cx.msg_with(|_: &ClickEvent| Msg::Submit))),
                ),
            )
            .into_any_element()
    }
}

/// The window's height for its message.
fn height() -> f32 {
    let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + 3.0 * m::SPACING + 3.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
    (fixed + paragraph_height(MESSAGE, COLUMN)).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_what_app_server_takes() {
        for (_, _, minutes) in LENGTHS {
            assert!((5..=24 * 60).contains(&minutes));
        }
    }
}
