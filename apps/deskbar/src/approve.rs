//! Approving requests without asking, for a while (app_server's
//! approve.rs): the key's menu, which is in the Olo menu too, and the
//! window that turns it on, which takes the account password (Terminal
//! answers sudo with it, so Touch ID can't stand in for it).

use olo_protocol::{ClientError, ClockSettings, Request};
use olo_ui::{
    Frame, Menu, WindowOptions,
    gpui::{App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window, WindowHandle, div, px},
    metrics as m,
    style,
    widgets::{self, AlertFrame, Button, Conversation, ConversationField, MenuField, TextField, TextFieldEvent, alert, button_row, paragraph, paragraph_height, text_field},
};
use zeroize::Zeroize;

use crate::status;

/// The lengths offered: in the menu, in the window, and in minutes
/// (app_server takes 5 to a day).
pub const LENGTHS: [(&str, &str, u32); 5] = [
    ("For 5 Minutes", "5 minutes", 5),
    ("For 15 Minutes", "15 minutes", 15),
    ("For 1 Hour", "1 hour", 60),
    ("For 4 Hours", "4 hours", 240),
    ("For a Day", "a day", 24 * 60),
];
/// What clicking the key turns it on for.
pub const USUAL: u32 = 15;

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
    menu.separator().item(label, "approve:off").enabled(until.is_some())
}

/// app_server's answer to turning it on.
pub struct Answer(pub Result<(), String>);

/// The window turning it on.
pub struct Ask {
    pub window: Option<WindowHandle<Frame>>,
    length: usize,
    conv: Conversation,
    field: Entity<TextField>,
    focus: FocusHandle,
}

impl Ask {
    /// Opens the window, `minutes` chosen.
    pub fn open(cx: &mut App, minutes: u32) -> Option<Entity<Ask>> {
        let field = text_field(cx, |f| f.name("Password").password(true));
        let mut conv = Conversation::default();
        conv.ask("Password".into(), false);
        let length = LENGTHS.iter().position(|l| l.2 == minutes).unwrap_or(1);
        let ask = cx.new(|cx| {
            cx.subscribe(&field, |a: &mut Ask, _, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Entered => a.submit(cx),
                _ => cx.notify(),
            })
            .detach();
            Ask { window: None, length, conv, field: field.clone(), focus: cx.focus_handle() }
        });
        let h = height(cx);
        let view = ask.clone();
        let window = olo_ui::open_window(cx, WindowOptions::alert(TITLE.to_owned(), WIDTH, h), move |window, cx| {
            window.activate_window();
            let closing = view.clone();
            window.on_window_should_close(cx, move |_, cx| {
                closing.update(cx, |a, cx| a.forget(cx));
                true
            });
            let focus = olo_ui::gpui::Focusable::focus_handle(&field, cx);
            window.focus(&focus);
            view
        })?;
        ask.update(cx, |a, _| a.window = Some(window));
        Some(ask)
    }

    /// Chosen again from the menu: that length, and the window forward.
    pub fn choose(&mut self, minutes: u32, cx: &mut Context<Self>) {
        if let Some(i) = LENGTHS.iter().position(|l| l.2 == minutes) {
            self.length = i;
        }
        if let Some(w) = self.window {
            let _ = w.update(cx, |_, window, _| window.activate_window());
        }
        cx.notify();
    }

    fn can_submit(&self, cx: &App) -> bool {
        self.conv.ready() && !self.field.read(cx).text().is_empty()
    }

    /// To app_server, which checks the password (on a thread here: that
    /// takes PAM a moment).
    fn submit(&mut self, cx: &mut Context<Self>) {
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
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            let mut req = Request::SetAutoApprove { minutes: Some(minutes), password: Some(password) };
            let answer = olo_protocol::Client::connect().and_then(|mut c| c.request(&req)).map(|_| ()).map_err(|e| match e {
                ClientError::Server(message) => message,
                e => e.to_string(),
            });
            if let Request::SetAutoApprove { password: Some(p), .. } = &mut req {
                p.zeroize();
            }
            proxy.send(Answer(answer));
        });
        cx.notify();
    }

    /// app_server answered: true when it is on (the window can go).
    pub fn answer(&mut self, answer: Answer, cx: &mut Context<Self>) -> bool {
        match answer.0 {
            Ok(()) => {
                self.close(cx);
                true
            }
            Err(e) => {
                self.conv.error = e;
                self.conv.ask("Password".into(), false);
                self.field.update(cx, |f, cx| f.set_enabled(true, cx));
                if let Some(w) = self.window {
                    let focus = olo_ui::gpui::Focusable::focus_handle(&self.field, cx);
                    let _ = w.update(cx, |_, window, _| window.focus(&focus));
                }
                cx.notify();
                false
            }
        }
    }

    /// The window is going: what was typed goes with it.
    fn forget(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        self.window = None;
    }

    /// Closes the window (cancelled, or on; from its own key or button
    /// too: `close_window`).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.clear_secret(cx));
        if let Some(w) = self.window.take() {
            cx.close_window(w);
        }
    }
}

impl Render for Ask {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let font = style::ui_font(FontWeight::NORMAL);
        let divider = ["For:", "Password:"].iter().map(|l| style::text_width_app(cx, l, style::PLAIN_SIZE, &font).ceil()).fold(0.0, f32::max) + m::LABEL_GAP;
        let names = LENGTHS.iter().map(|l| l.1.into()).collect();
        let pick = view.clone();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(MenuField::new("approve-length", names, self.length).label("For:").divider(divider).width(COLUMN).enabled(!self.conv.checking).on_change(move |i, _, cx| {
                pick.update(cx, |a, cx| {
                    a.length = i;
                    cx.notify();
                })
            }))
            .child(ConversationField::new(&self.conv, self.field.clone(), divider));
        let can = self.can_submit(cx);
        let (cancel, submit) = (view.clone(), view);
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|a, _: &alert::Cancel, _, cx| a.close(cx)))
            .on_action(cx.listener(|a, _: &alert::Default, _, cx| a.submit(cx)))
            .child(
                AlertFrame::new(ICON)
                    .child(widgets::alert_title(TITLE))
                    .child(div().h(px(m::TIGHT)))
                    .child(paragraph(MESSAGE))
                    .child(div().h(px(m::SPACING)))
                    .child(form)
                    .child(div().flex_1())
                    .child(
                        button_row()
                            .child(Button::new("Cancel").on_click(move |_, cx| cancel.update(cx, |a, cx| a.close(cx))))
                            .child(Button::new("Turn On").default(true).enabled(can).on_click(move |_, cx| submit.update(cx, |a, cx| a.submit(cx)))),
                    ),
            )
    }
}

/// The window's height for its message.
fn height(cx: &App) -> f32 {
    let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + 3.0 * m::SPACING + 3.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
    (fixed + paragraph_height(cx, MESSAGE, COLUMN)).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_what_app_server_takes() {
        assert!(LENGTHS.iter().any(|l| l.2 == USUAL));
        for (_, _, minutes) in LENGTHS {
            assert!((5..=24 * 60).contains(&minutes));
        }
    }
}
