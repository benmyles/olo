//! The keyring's prompts (`olo_system::keyring_prompt`): unlock the
//! login keyring, choose a new keyring's password, or confirm. One window
//! per prompt; it stays up, "Checking…", until the keyring says it's done
//! or asks again (a wrong password comes back with a warning).

use olo_kit::{
    Cx, WindowId, WindowOptions,
    egui::{self, Key, Rect, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, CheckBox, TextControl},
};
use olo_system::keyring_prompt::{Answer, Kind, Props};

pub const ICON: &str = "key";

pub struct Ask {
    pub id: u64,
    kind: Kind,
    props: Props,
    password: String,
    confirm: String,
    choice: bool,
    /// Our own complaint (the two passwords differ).
    error: String,
    /// Answered; waiting for the keyring.
    checking: bool,
    /// Put the cursor in Confirm (it didn't match).
    refocus: bool,
}

pub enum Action {
    Answer(Answer, bool),
}

fn clear(s: &mut String) {
    // Don't leave a password lying in freed memory.
    // SAFETY: zero bytes keep the string valid UTF-8.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl Drop for Ask {
    fn drop(&mut self) {
        clear(&mut self.password);
        clear(&mut self.confirm);
    }
}

fn height(kind: Kind, props: &Props) -> u32 {
    let mut h = widgets::alert_height(&props.description) as f32 + m::LABEL_ROW + m::TIGHT + m::LABEL_ROW + m::SPACING;
    if kind == Kind::Password {
        h += (m::CONTROL_HEIGHT + m::SPACING) * if props.password_new { 2.0 } else { 1.0 };
    }
    if !props.choice_label.is_empty() {
        h += m::CONTROL_HEIGHT + m::SPACING;
    }
    h.ceil() as u32
}

fn title(props: &Props) -> &str {
    if props.title.is_empty() { "Keyring" } else { &props.title }
}

/// Open a window for a new prompt.
pub fn open(cx: &mut Cx, id: u64, kind: Kind, props: Props) -> (WindowId, Ask) {
    let h = height(kind, &props);
    let win = cx.open_window(WindowOptions::new(title(&props), super::WIDTH, h).min_size(super::WIDTH, h).max_size(super::WIDTH, h));
    let choice = props.choice_chosen;
    (win, Ask { id, kind, props, password: String::new(), confirm: String::new(), choice, error: String::new(), checking: false, refocus: false })
}

impl Ask {
    /// The same prompt again (after a wrong password): fresh fields.
    pub fn again(&mut self, kind: Kind, props: Props) {
        self.kind = kind;
        self.props = props;
        clear(&mut self.password);
        clear(&mut self.confirm);
        self.error.clear();
        self.checking = false;
    }

    fn answer(&mut self) -> Option<Action> {
        if self.checking {
            return None;
        }
        let answer = match self.kind {
            Kind::Confirm => Answer::Continue,
            Kind::Password => {
                if self.props.password_new && self.password != self.confirm {
                    self.error = "The passwords don't match.".into();
                    clear(&mut self.confirm);
                    self.refocus = true;
                    return None;
                }
                Answer::Password(std::mem::take(&mut self.password))
            }
        };
        clear(&mut self.confirm);
        self.error.clear();
        self.checking = true;
        Some(Action::Answer(answer, self.choice))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let p = style::palette(ui.ctx());
        let full = ui.available_rect_before_wrap();
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        let message = if self.props.message.is_empty() { "Authentication required" } else { &self.props.message };
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, message, style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        if !self.props.description.is_empty() {
            let mut job = egui::text::LayoutJob::single_section(self.props.description.clone(), egui::TextFormat::simple(style::plain_font(), c(p.panel_text)));
            job.wrap.max_width = col.width();
            let g = ui.painter().layout_job(job);
            let text_h = g.size().y;
            let pos = style::text_origin(ui.ctx(), &g, col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, style::PLAIN_SIZE);
            ui.painter().galley(pos, g, c(p.panel_text));
            y += text_h + m::SPACING;
        }

        let mut submit = false;
        let form = Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        ui.scope_builder(egui::UiBuilder::new().max_rect(form).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            let divider = 72.0;
            let ready = !self.checking;
            if self.kind == Kind::Password {
                let r = ui
                    .add_enabled_ui(ready, |ui| ui.add(TextControl::new(&mut self.password).label("Password:").divider(divider).password(true).autofocus(ready).width(form.width())))
                    .inner;
                let mut last = r.clone();
                if ready && self.password.is_empty() && self.confirm.is_empty() && !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                    // It moves on the next frame.
                    ui.ctx().request_repaint();
                }
                if self.props.password_new {
                    last = ui.add_enabled_ui(ready, |ui| ui.add(TextControl::new(&mut self.confirm).label("Confirm:").divider(divider).password(true).width(form.width()))).inner;
                    if std::mem::take(&mut self.refocus) {
                        last.request_focus();
                        ui.ctx().request_repaint();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        last.request_focus();
                    }
                }
                if last.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            }
            if !self.props.choice_label.is_empty() {
                ui.add_enabled_ui(ready, |ui| ui.add(CheckBox::new(&mut self.choice, &self.props.choice_label)));
            }
            let (note, color) = if self.checking {
                ("Checking…".to_owned(), style::disabled_text(&p))
            } else if !self.error.is_empty() {
                (self.error.clone(), c(p.failure))
            } else if !self.props.warning.is_empty() {
                (self.props.warning.clone(), c(p.failure))
            } else {
                (String::new(), style::disabled_text(&p))
            };
            if !note.is_empty() {
                let r = ui.allocate_exact_size(vec2(form.width(), m::LABEL_ROW), egui::Sense::hover()).0;
                style::label(ui.painter(), r.left(), r.center().y, egui::Align::Min, note, style::plain_font(), color);
            }
        });

        let row = Rect::from_min_max(pos2(col.left(), col.bottom() - m::CONTROL_HEIGHT), pos2(full.right() - m::WINDOW_INSET, col.bottom()));
        let mut cancel = ui.input(|i| i.key_pressed(Key::Escape));
        let go = if self.props.continue_label.is_empty() { "Continue" } else { &self.props.continue_label };
        let stop = if self.props.cancel_label.is_empty() { "Cancel" } else { &self.props.cancel_label };
        let can = !self.checking && (self.kind == Kind::Confirm || !self.password.is_empty() || self.props.password_new);
        ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
            widgets::button_row(ui, |ui| {
                if ui.add_enabled_ui(can, |ui| ui.add(Button::new(go).default(true))).inner.clicked() {
                    submit = true;
                }
                if ui.add(Button::new(stop)).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.checking = true;
            return Some(Action::Answer(Answer::Cancel, self.choice));
        }
        if submit { self.answer() } else { None }
    }
}
