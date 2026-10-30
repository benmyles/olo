//! The Access portal (`org.freedesktop.impl.portal.Access`): the
//! permission dialog every other portal asks through ("Allow Firefox to
//! use the camera?", a sandboxed app taking a screenshot), as Olo's own
//! BAlert: the icon the portal names (else a shield), the question and
//! what it means, any choices it offers (a checkbox for a yes/no, a
//! pop-up menu for several), and the deny and grant buttons (Escape
//! denies). xdg-desktop-portal also only offers the Screenshot portal
//! when there is one.

use std::collections::HashMap;

use olo_kit::{
    Cx, WindowId, WindowOptions,
    egui::{self, pos2},
    metrics as m,
    style::{self, c},
    widgets::{self, CheckBox, MenuField},
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::portal::ToUi;

const WIDTH: u32 = 420;
const ICON: &str = "security";
/// The alert's icon, which the text stands beside at least as tall.
const ICON_SIZE: f32 = 32.0;
/// The text column: the alert's width less its stripe, icon and margins.
const COLUMN: f32 = WIDTH as f32 - widgets::ALERT_TEXT_X - m::WINDOW_INSET;

/// A choice the dialog offers: with no options, yes or no.
#[derive(Debug, Clone)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// What the dialog was asked (from the portal's AccessDialog).
pub struct Ask {
    pub request: String,
    pub parent: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub deny: String,
    pub grant: String,
    pub icon: String,
    pub choices: Vec<Choice>,
    /// The answer: the choices made, if granted (None: denied).
    pub reply: async_channel::Sender<Option<Vec<(String, String)>>>,
}

/// `org.freedesktop.impl.portal.Access`.
pub struct Access {
    pub to_ui: std::sync::Arc<dyn Fn(ToUi) + Send + Sync>,
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Access")]
impl Access {
    #[allow(clippy::too_many_arguments)]
    async fn access_dialog(
        &self,
        handle: OwnedObjectPath,
        _app_id: String,
        parent_window: String,
        title: String,
        subtitle: String,
        body: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let choices: Vec<Choice> = options
            .get("choices")
            .and_then(|v| <Vec<(String, String, Vec<(String, String)>, String)>>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, label, options, initial)| Choice { id, label, options, initial })
            .collect();
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Access(Ask {
            request: handle.to_string(),
            parent: parent_window,
            title,
            subtitle,
            body,
            deny: string(&options, "deny_label"),
            grant: string(&options, "grant_label"),
            icon: string(&options, "icon"),
            choices,
            reply: tx,
        }));
        let answer = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        match answer {
            Some(made) => (0, HashMap::from([("choices".to_owned(), OwnedValue::try_from(Value::from(made)).expect("no file descriptors"))])),
            None => (1, HashMap::new()),
        }
    }
}

/// The dialog on screen.
pub struct Dialog {
    ask: Ask,
    /// Each choice's state: yes/no, or which option.
    on: Vec<bool>,
    picked: Vec<usize>,
}

/// Its height for `ask`: the question, what it means, the choices, the
/// buttons.
fn height(ask: &Ask) -> u32 {
    let mut text = m::LABEL_ROW + m::TIGHT;
    for t in [&ask.subtitle, &ask.body] {
        if !t.is_empty() {
            text += style::plain_text_height(t, COLUMN) + m::TIGHT;
        }
    }
    let choices = ask.choices.iter().map(|ch| if ch.options.is_empty() { m::MARK_ROW } else { m::CONTROL_HEIGHT }).sum::<f32>() + ask.choices.len() as f32 * m::SPACING;
    (2.0 * m::WINDOW_INSET + text.max(ICON_SIZE) + choices + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil() as u32
}

impl Dialog {
    pub fn open(cx: &mut Cx, ask: Ask) -> (WindowId, Dialog) {
        let size = (WIDTH, height(&ask));
        let mut options = WindowOptions { size, min_size: Some(size), max_size: Some(size), ..WindowOptions::alert("Permission", "") };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        let on = ask.choices.iter().map(|ch| ch.initial == "true").collect();
        let picked = ask.choices.iter().map(|ch| ch.options.iter().position(|(o, _)| *o == ch.initial).unwrap_or(0)).collect();
        (id, Dialog { ask, on, picked })
    }

    pub fn request(&self) -> &str {
        &self.ask.request
    }

    /// Answer (None: denied); the caller closes the window.
    pub fn answer(&self, granted: bool) {
        let made = granted.then(|| {
            self.ask
                .choices
                .iter()
                .enumerate()
                .map(|(i, ch)| {
                    let value = if ch.options.is_empty() { self.on[i].to_string() } else { ch.options.get(self.picked[i]).map(|(o, _)| o.clone()).unwrap_or_default() };
                    (ch.id.clone(), value)
                })
                .collect()
        });
        let _ = self.ask.reply.try_send(made);
    }

    /// Draw it; Some(granted) once answered.
    pub fn ui(&mut self, ui: &mut egui::Ui, win: WindowId) -> Option<bool> {
        let p = style::palette(ui.ctx());
        // The portal's icon by its freedesktop name, as Olo's theme has it.
        let named = ["camera", "microphone", "screenshot", "display"].into_iter().find(|n| self.ask.icon.contains(n));
        let icon = named.filter(|n| olo_kit::icons::texture(ui.ctx(), n, ICON_SIZE).is_some()).unwrap_or(ICON);
        let col = widgets::alert_frame(ui, icon);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, self.ask.title.clone(), style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        for t in [self.ask.subtitle.clone(), self.ask.body.clone()] {
            if !t.is_empty() {
                y = widgets::alert_text(ui, col, y, &t) + m::TIGHT;
            }
        }
        y = y.max(col.top() + ICON_SIZE) + m::SPACING;
        let area = egui::Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            for (i, ch) in self.ask.choices.iter().enumerate() {
                if ch.options.is_empty() {
                    ui.add(CheckBox::new(&mut self.on[i], &ch.label));
                } else {
                    let labels: Vec<&str> = ch.options.iter().map(|(_, l)| l.as_str()).collect();
                    ui.add(MenuField::new(("choice", win, i), &mut self.picked[i], &labels).label(&ch.label).width(col.width()));
                }
            }
        });
        let deny = if self.ask.deny.is_empty() { "Deny" } else { self.ask.deny.as_str() };
        let grant = if self.ask.grant.is_empty() { "Allow" } else { self.ask.grant.as_str() };
        widgets::alert_buttons(ui, col, &[deny, grant], true).map(|i| i == 1)
    }
}
