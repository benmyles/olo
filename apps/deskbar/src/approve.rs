//! Approving requests without asking, for a while (app_server's
//! approve.rs): the key's menu, which is in the Olo menu too, and the
//! window that turns it on, which takes the account password (Terminal
//! answers sudo with it, so Touch ID can't stand in for it).

use olo_kit::{
    Cx, Menu, Proxy, WindowId, WindowOptions,
    egui::{self, Align, Rect, pos2},
    metrics as m,
    style::{self, c},
    widgets::{self, Conversation, MenuField},
};
use olo_protocol::{ClientError, ClockSettings, Request};
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

const WIDTH: u32 = 420;
const ICON: &str = "key";
const TITLE: &str = "Approve Without Asking";
const MESSAGE: &str = "While this is on, sudo in Terminal gets your password, and Passwords unlocks and lets op in, without asking you, for people and agents alike. Each is still shown in a quiet notification. It ends at its time, or when the screen locks.";
/// Text column: the alert's width less its stripe, icon and margins.
const COLUMN: f32 = WIDTH as f32 - widgets::ALERT_TEXT_X - m::WINDOW_INSET;

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
        menu = menu.item(label, format!("approve:{minutes}"), None);
    }
    let label = match until {
        Some(t) => format!("Ask Again ({} left, until {})", left(t), status::time_of_day(t, clock)),
        None => "Ask Again".into(),
    };
    menu.separator().item(label, "approve:off", None).enabled(until.is_some())
}

/// app_server's answer to turning it on.
pub struct Answer(pub Result<(), String>);

/// The window turning it on.
pub struct Ask {
    pub win: WindowId,
    length: usize,
    conv: Conversation,
}

impl Ask {
    pub fn open(cx: &mut Cx, minutes: u32) -> Ask {
        let size = (WIDTH, height());
        let win = cx.open_window(WindowOptions { size, min_size: Some(size), max_size: Some(size), ..WindowOptions::alert(TITLE, MESSAGE) });
        cx.activate(win);
        let mut conv = Conversation::default();
        conv.ask("Password".into(), false);
        let length = LENGTHS.iter().position(|l| l.2 == minutes).unwrap_or(1);
        Ask { win, length, conv }
    }

    /// Chosen again from the menu: that length.
    pub fn choose(&mut self, minutes: u32) {
        if let Some(i) = LENGTHS.iter().position(|l| l.2 == minutes) {
            self.length = i;
        }
    }

    /// Draw it; true once it is cancelled.
    pub fn ui(&mut self, ui: &mut egui::Ui, proxy: &Proxy) -> bool {
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let painter = ui.painter().clone();
        let mut y = col.top();
        style::label(&painter, col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, Align::Min, TITLE, style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        let bottom = widgets::alert_text(ui, col, y, MESSAGE);
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, MESSAGE, Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), bottom)), None);
        y = bottom + m::SPACING;

        let form = Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        let mut submit = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(form).layout(egui::Layout::top_down(Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            let divider = widgets::divider_for(ui, &["For:", "Password:"]);
            let names = LENGTHS.map(|l| l.1);
            ui.add_enabled(!self.conv.checking, MenuField::new("approve-length", &mut self.length, &names).label("For:").divider(divider));
            submit = widgets::conversation(ui, &mut self.conv, divider, form.width());
        });
        match widgets::alert_buttons(ui, col, &["Cancel", "Turn On"], self.conv.can_submit()) {
            Some(0) => return true,
            Some(_) => submit = true,
            None => {}
        }
        if submit && self.conv.can_submit() {
            self.send(proxy);
        }
        false
    }

    /// To app_server, which checks the password (on a thread here: that
    /// takes PAM a moment).
    fn send(&mut self, proxy: &Proxy) {
        let password = std::mem::take(&mut self.conv.answer);
        self.conv.sent();
        let minutes = LENGTHS[self.length].2;
        let proxy = proxy.clone();
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
    }

    /// app_server answered: true when it is on (the window can go).
    pub fn answer(&mut self, answer: Answer) -> bool {
        match answer.0 {
            Ok(()) => true,
            Err(e) => {
                self.conv.error = e;
                self.conv.ask("Password".into(), false);
                false
            }
        }
    }
}

/// The window's height for its message.
fn height() -> u32 {
    let message = style::plain_text_height(MESSAGE, COLUMN);
    let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + m::TIGHT + 3.0 * m::SPACING + 3.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
    (fixed + message).ceil() as u32
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
