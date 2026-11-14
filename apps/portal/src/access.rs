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
    Bounds, ClickEvent, Cx, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div, metrics as m, point, px,
    size, style,
    widgets::{
        AlertFrame, Button, CheckBox, MenuField,
        alert::{ALERT_ICON, ALERT_TEXT_X, alert_title, button_row, paragraph, paragraph_height},
    },
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{
    portal::ToUi,
    ui::{self, Answer},
};

const WIDTH: f32 = 420.0;
const ICON: &str = "security";
/// The text column: the alert's width less its stripe, icon and margins.
const COLUMN: f32 = WIDTH - ALERT_TEXT_X - m::WINDOW_INSET;

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
    answer: Answer<Option<Vec<(String, String)>>>,
    title: String,
    subtitle: String,
    body: String,
    deny: String,
    grant: String,
    icon: String,
    /// The icon shown, when there's no picture.
    named: &'static str,
    choices: Vec<Choice>,
    /// Each choice's state: yes/no, or which option.
    on: Vec<bool>,
    picked: Vec<usize>,
}

/// Its height for `ask`: the question, what it means, the choices, the
/// buttons.
fn height(ask: &Ask) -> f32 {
    let mut text = m::LABEL_ROW + m::TIGHT;
    for t in [&ask.subtitle, &ask.body] {
        if !t.is_empty() {
            text += paragraph_height(t, COLUMN) + m::TIGHT;
        }
    }
    let choices = ask.choices.iter().map(|ch| if ch.options.is_empty() { m::MARK_ROW } else { m::CONTROL_HEIGHT }).sum::<f32>() + ask.choices.len() as f32 * m::SPACING;
    (2.0 * m::WINDOW_INSET + text.max(ALERT_ICON) + choices + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil()
}

/// The icon the portal names, as Olo's theme has it (else a shield).
/// (It looks in the icon theme: once, as the dialog opens.)
fn icon(named: &str) -> &'static str {
    ["camera", "microphone", "screenshot", "display", "people"].into_iter().find(|n| named.contains(n)).filter(|n| olo_xdg::find_icon(n, 32).is_some()).unwrap_or(ICON)
}

pub fn open(ask: Ask) -> WindowHandle<Dialog> {
    let options = ui::over(WindowOptions::alert("Permission", WIDTH, height(&ask)), &ask.parent);
    ui::open(options, move |_| {
        let on = ask.choices.iter().map(|ch| ch.initial == "true").collect();
        let picked = ask.choices.iter().map(|ch| ch.options.iter().position(|(o, _)| *o == ch.initial).unwrap_or(0)).collect();
        Dialog {
            answer: Answer::new(ask.reply, None),
            title: ask.title,
            subtitle: ask.subtitle,
            body: ask.body,
            deny: ask.deny,
            grant: ask.grant,
            named: icon(&ask.icon),
            icon: ask.icon,
            choices: ask.choices,
            on,
            picked,
        }
    })
}

impl Dialog {
    /// Answered: granted with the choices made, or denied; the window goes.
    fn finish(&mut self, granted: bool, cx: &mut Cx<Self>) {
        let made = granted.then(|| {
            self.choices
                .iter()
                .enumerate()
                .map(|(i, ch)| {
                    let value = if ch.options.is_empty() { self.on[i].to_string() } else { ch.options.get(self.picked[i]).map(|(o, _)| o.clone()).unwrap_or_default() };
                    (ch.id.clone(), value)
                })
                .collect()
        });
        self.answer.give(made);
        cx.close_window();
    }
}

impl View for Dialog {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // A picture (the Account portal's: the person's) where the icon
        // goes, as it is (pictures of people are often round, with clear
        // corners); else the portal's icon by its freedesktop name.
        let picture = self.icon.starts_with('/').then(|| std::path::PathBuf::from(&self.icon));
        let mut frame = AlertFrame::new(if picture.is_some() { "" } else { self.named });
        if let Some(path) = picture {
            frame = frame.child(
                canvas(move |g, b| {
                    let s = g.scale_factor();
                    let side = (ALERT_ICON * s).round() as u32;
                    // Where the alert's icon goes: straddling its stripe.
                    let place = Bounds { origin: point(b.left() - px(ALERT_TEXT_X - m::WINDOW_INSET + ALERT_ICON / 2.0), b.top()), size: size(px(ALERT_ICON), px(ALERT_ICON)) };
                    if let Some(img) = olo_kit::pictures::fitted(g, &path, (side, side)) {
                        let isz = img.size(0);
                        let at = style::snap_centred(g, place.center(), isz.width.0 as f32 / s, isz.height.0 as f32 / s);
                        let _ = g.paint_image(at, Default::default(), img, 0, false);
                    }
                })
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .size_full(),
            );
        }
        frame = frame.child(alert_title(SharedString::from(self.title.clone())));
        for t in [&self.subtitle, &self.body] {
            if !t.is_empty() {
                frame = frame.child(div().mt(px(m::TIGHT)).child(paragraph(t.clone())));
            }
        }
        // (A paragraph's own space after it, then the gap: as tall as
        // `height` reckons.)
        let mut choices = div().flex().flex_col().gap(px(m::SPACING)).mt(px(m::TIGHT + m::SPACING));
        for (i, ch) in self.choices.iter().enumerate() {
            if ch.options.is_empty() {
                choices = choices.child(CheckBox::new(SharedString::from(ch.label.clone()), self.on[i]).on_toggle(cx.listener(move |d: &mut Dialog, on: &bool, cx| {
                    d.on[i] = *on;
                    cx.invalidate();
                })));
            } else {
                let labels: Vec<SharedString> = ch.options.iter().map(|(_, l)| SharedString::from(l.clone())).collect();
                choices = choices.child(MenuField::new(SharedString::from(format!("choice-{i}")), labels, self.picked[i]).label(SharedString::from(ch.label.clone())).width(COLUMN).on_change(
                    cx.listener(move |d: &mut Dialog, k: &usize, cx| {
                        d.picked[i] = *k;
                        cx.invalidate();
                    }),
                ));
            }
        }
        let deny = if self.deny.is_empty() { "Deny".to_owned() } else { self.deny.clone() };
        let grant = if self.grant.is_empty() { "Allow".to_owned() } else { self.grant.clone() };
        let no = Button::new(deny).on_click(cx.listener(|d: &mut Dialog, _: &ClickEvent, cx| d.finish(false, cx)));
        // (Return grants: the default button. Escape denies.)
        let yes = Button::new(grant).default(true).on_click(cx.listener(|d: &mut Dialog, _: &ClickEvent, cx| d.finish(true, cx)));
        div()
            .size_full()
            .on_window_key_down(cx.listener(|d: &mut Dialog, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    d.finish(false, cx);
                }
            }))
            .child(frame.child(div().min_h(px(0.0)).child(choices)).child(div().flex_1()).child(button_row().child(no).child(yes)))
    }
}
