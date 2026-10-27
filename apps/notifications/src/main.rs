//! tabula-notifications: the session's notification service. Apps'
//! notifications (org.freedesktop.Notifications: notify-send, browsers,
//! chat and mail) appear as cards stacked in the main display's lower
//! right corner: the app's icon or picture, the summary in bold, the body,
//! and its actions as buttons. A click on the card is its default action;
//! the close box dismisses it; it goes by itself after a while (critical
//! ones stay), not while the pointer is over it.
//!
//! Only the real session runs it (a test or nested session must not take
//! the user's notifications); `--bus ADDRESS` serves a private bus.

mod server;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use server::{Bus, In, Note, Reason};
use tabula_kit::{
    App, Cx, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Align, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::Button,
};

const APP_ID: &str = "tabula.notifications";
/// A card's width (logical).
const WIDTH: f32 = 340.0;
const PAD: f32 = 10.0;
const ICON: f32 = 32.0;
/// Between cards, and from the display's edges.
const GAP: f32 = 8.0;
/// At most this many at once; the rest wait their turn.
const SHOWN: usize = 4;
const BODY_ROWS: usize = 5;

enum Msg {
    In(In),
    /// Something may have expired.
    Tick,
}

struct Card {
    note: Note,
    /// When it goes (None: when dismissed).
    until: Option<Instant>,
    picture: Option<TextureHandle>,
}

struct Notifications {
    bus: Arc<Mutex<Option<Bus>>>,
    win: Option<WindowId>,
    /// Oldest first.
    cards: Vec<Card>,
    /// The window's height last asked for.
    height: u32,
    proxy: tabula_kit::Proxy,
}

impl Notifications {
    fn bus(&self, f: impl FnOnce(&Bus)) {
        if let Some(b) = self.bus.lock().unwrap().as_ref() {
            f(b);
        }
    }

    fn show(&mut self, cx: &mut Cx, note: Note) {
        let until = note.timeout.map(|t| Instant::now() + t);
        if let Some(t) = note.timeout {
            self.tick_after(t);
        }
        let card = Card { note, until, picture: None };
        match self.cards.iter_mut().find(|c| c.note.id == card.note.id) {
            Some(old) => *old = card,
            None => self.cards.push(card),
        }
        if self.win.is_none() {
            self.win = Some(cx.open_window(WindowOptions {
                title: "Notifications".into(),
                size: (WIDTH as u32, 1),
                kind: WindowKind::Layer(LayerOptions {
                    layer: Layer::Top,
                    anchor: (false, true, false, true),
                    exclusive_zone: 0,
                    margin: (0, GAP as i32, GAP as i32, 0),
                    keyboard: tabula_kit::Keyboard::None,
                    namespace: "notifications".into(),
                    output: None,
                }),
                panel: false,
                ..Default::default()
            }));
            self.height = 1;
        }
        self.redraw(cx);
    }

    fn tick_after(&self, after: Duration) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after + Duration::from_millis(20));
            proxy.send(Msg::Tick);
        });
    }

    /// Take a card away and say why.
    fn remove(&mut self, cx: &mut Cx, id: u32, reason: Reason) {
        let before = self.cards.len();
        self.cards.retain(|c| c.note.id != id);
        if self.cards.len() != before {
            self.bus(|b| b.closed(id, reason));
        }
        // The ones waiting have their time from when they appear.
        let now = Instant::now();
        let mut later = Vec::new();
        for c in self.cards.iter_mut().take(SHOWN) {
            if let (Some(t), Some(until)) = (c.note.timeout, c.until)
                && until < now + Duration::from_secs(1)
            {
                c.until = Some(now + t.min(Duration::from_secs(3)).max(Duration::from_secs(1)));
                later.push(t);
            }
        }
        for t in later {
            self.tick_after(t.min(Duration::from_secs(3)));
        }
        if self.cards.is_empty() {
            if let Some(w) = self.win.take() {
                cx.close_window(w);
            }
        } else {
            self.redraw(cx);
        }
    }

    fn redraw(&self, cx: &mut Cx) {
        if let Some(w) = self.win {
            cx.redraw(w);
        }
    }

    fn expire(&mut self, cx: &mut Cx, hovered: Option<u32>) {
        let now = Instant::now();
        let gone: Vec<u32> = self
            .cards
            .iter()
            .take(SHOWN)
            .filter(|c| c.until.is_some_and(|u| u <= now) && Some(c.note.id) != hovered)
            .map(|c| c.note.id)
            .collect();
        for id in gone {
            self.remove(cx, id, Reason::Expired);
        }
    }
}

/// Icons to try for a card, best first: the one it names, ours for the
/// common theme names, the sending app's, then info (or warning).
fn icon_names(note: &Note) -> Vec<String> {
    let mut names: Vec<String> = note.icon.iter().cloned().collect();
    if let Some(icon) = &note.icon {
        let ours = [
            ("mail", "mail"),
            ("utilities-terminal", "terminal"),
            ("dialog-warning", "warning"),
            ("dialog-error", "stop"),
            ("dialog-information", "info"),
            ("battery", "power"),
            ("network", "network"),
            ("bluetooth", "bluetooth"),
            ("audio", "sound"),
            ("software-update", "software-update"),
            ("system-software-install", "installer"),
            ("camera", "camera"),
            ("printer", "printer"),
            ("drive", "disk"),
            ("media-removable", "usb"),
        ];
        names.extend(ours.iter().filter(|(theme, _)| icon.starts_with(theme)).map(|(_, o)| o.to_string()));
    }
    let app = note.app_name.to_lowercase().replace(' ', "-");
    if !app.is_empty() {
        names.extend(tabula_xdg::icon_for_app(&app));
        names.extend(tabula_xdg::apps::our_app(&app).map(|(_, icon)| icon.to_owned()));
    }
    names.push(if note.critical { "warning" } else { "info" }.into());
    names
}

/// Body text laid out for a card.
fn body_galley(ui: &egui::Ui, text: &str, color: Color32, width: f32, rows: usize, font: egui::FontId) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// What happened to a card this frame.
enum Did {
    Dismiss,
    Action(String),
}

impl App for Notifications {
    fn ui(&mut self, cx: &mut Cx, _window: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let text_x = PAD + ICON + PAD;
        let text_w = WIDTH - text_x - PAD;
        let hover = ui.input(|i| i.pointer.hover_pos());
        // Lay the shown cards out, newest nearest the corner (at the bottom).
        let shown: Vec<usize> = (0..self.cards.len().min(SHOWN)).collect();
        let mut laid = Vec::new();
        let mut y = 0.0;
        for &i in &shown {
            let card = &self.cards[i];
            let title = body_galley(ui, &card.note.summary, c(p.panel_text), text_w - 18.0, 2, style::bold_font());
            let body = body_galley(ui, &card.note.body, c(p.panel_text), text_w, BODY_ROWS, style::plain_font());
            let buttons: Vec<&(String, String)> = card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty()).collect();
            let mut h = PAD + style::SMALL_SIZE + m::TIGHT + title.size().y;
            if !card.note.body.is_empty() {
                h += m::TIGHT + body.size().y;
            }
            h = h.max(ICON + 2.0 * PAD - PAD) + PAD;
            if !buttons.is_empty() {
                h += m::CONTROL_HEIGHT + PAD;
            }
            let rect = Rect::from_min_size(pos2(0.0, y), vec2(WIDTH, h.ceil()));
            y += rect.height() + GAP;
            laid.push((i, rect, title, body));
        }
        let total = (y - GAP).max(1.0).ceil() as u32;
        if total != self.height {
            self.height = total;
            if let Some(w) = self.win {
                cx.resize(w, (WIDTH as u32, total));
            }
        }
        let mut hovered = None;
        let mut did = Vec::new();
        let painter = ui.painter().clone();
        for (i, rect, title, body) in laid {
            let card = &mut self.cards[i];
            let id = card.note.id;
            if hover.is_some_and(|h| rect.contains(h)) {
                hovered = Some(id);
            }
            painter.rect_filled(rect, 0.0, c(p.panel_background));
            style::frame_rect(&painter, rect, style::darken(p.panel_background, 1.6));
            style::raised(&painter, rect.shrink(1.0), &p);
            // The whole card is the default action (or dismisses).
            let click = ui.interact(rect, ui.make_persistent_id(("card", id)), Sense::click());
            // Icon: the picture sent, the app's icon, or a generic one.
            let icon_r = Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(ICON, ICON));
            if card.picture.is_none()
                && let Some(pic) = &card.note.picture
            {
                let img = ColorImage::from_rgba_unmultiplied([pic.width, pic.height], &pic.rgba);
                card.picture = Some(ui.ctx().load_texture(format!("note-{id}"), img, TextureOptions::LINEAR));
            }
            if let Some(tex) = &card.picture {
                let r = icons::fit(ui.ctx(), tex, icon_r);
                painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                let _ = icon_names(&card.note).iter().any(|n| icons::paint(&painter, icon_r, n));
            }
            // App name, small, and the close box.
            let x = rect.left() + text_x;
            let top = rect.top() + PAD + style::SMALL_SIZE / 2.0;
            style::label(&painter, x, top, Align::Min, &card.note.app_name, style::small_font(), style::disabled_text(&p));
            let close = Rect::from_center_size(pos2(rect.right() - PAD - 6.0, top), vec2(12.0, 12.0));
            let close_resp = ui.interact(close.expand(3.0), ui.make_persistent_id(("close", id)), Sense::click());
            let x_color = if close_resp.hovered() { c(p.panel_text) } else { style::disabled_text(&p) };
            let k = close.shrink(3.0);
            painter.line_segment([k.left_top(), k.right_bottom()], egui::Stroke::new(1.2, x_color));
            painter.line_segment([k.right_top(), k.left_bottom()], egui::Stroke::new(1.2, x_color));
            let mut ty = rect.top() + PAD + style::SMALL_SIZE + m::TIGHT;
            painter.galley(pos2(x, ty), title.clone(), c(p.panel_text));
            ty += title.size().y + m::TIGHT;
            if !card.note.body.is_empty() {
                painter.galley(pos2(x, ty), body, c(p.panel_text));
            }
            // Actions along the bottom, right to left as in an alert.
            let mut bx = rect.right() - PAD;
            let by = rect.bottom() - PAD - m::CONTROL_HEIGHT;
            let mut pressed = None;
            for (key, label) in card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty()).rev() {
                let w = (style::label_width(ui.ctx(), label, style::plain_font()) + 24.0).max(m::BUTTON_MIN_WIDTH);
                let r = Rect::from_min_size(pos2(bx - w, by), vec2(w, m::CONTROL_HEIGHT));
                if ui.put(r, Button::new(label.as_str())).clicked() {
                    pressed = Some(key.clone());
                }
                bx -= w + m::SPACING;
            }
            if close_resp.clicked() {
                did.push((id, Did::Dismiss));
            } else if let Some(key) = pressed {
                did.push((id, Did::Action(key)));
            } else if click.clicked() {
                let default = card.note.actions.iter().any(|(k, _)| k == "default");
                did.push((id, if default { Did::Action("default".into()) } else { Did::Dismiss }));
            }
        }
        for (id, d) in did {
            match d {
                Did::Dismiss => self.remove(cx, id, Reason::Dismissed),
                Did::Action(key) => {
                    self.bus(|b| b.action(id, &key));
                    let resident = self.cards.iter().any(|c| c.note.id == id && c.note.resident);
                    if !resident {
                        self.remove(cx, id, Reason::Dismissed);
                    }
                }
            }
        }
        // Hovering keeps a card; it goes a moment after the pointer leaves.
        if let Some(id) = hovered
            && let Some(card) = self.cards.iter_mut().find(|c| c.note.id == id)
            && card.until.is_some_and(|u| u < Instant::now() + Duration::from_secs(2))
        {
            card.until = Some(Instant::now() + Duration::from_secs(2));
            self.tick_after(Duration::from_secs(2));
        }
        self.expire(cx, hovered);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::In(In::Show(note)) => self.show(cx, note),
            Msg::In(In::Close(id)) => self.remove(cx, id, Reason::Closed),
            Msg::Tick => {
                // Let the frame decide (it knows whether the pointer is
                // over a card).
                self.redraw(cx);
                if self.win.is_none() {
                    self.expire(cx, None);
                }
            }
        }
    }

    fn close_requested(&mut self, _cx: &mut Cx, _window: WindowId) {}
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let address = match args.as_slice() {
        [flag, bus] if flag == "--bus" => Some(bus.clone()),
        [] => None,
        _ => {
            eprintln!("usage: tabula-notifications [--bus ADDRESS]");
            return std::process::ExitCode::FAILURE;
        }
    };
    if address.is_none() && std::env::var_os("TABULA_REAL_SESSION").is_none() {
        eprintln!("tabula-notifications: not in a real Tabula session (use --bus ADDRESS to try it)");
        return std::process::ExitCode::FAILURE;
    }
    tabula_kit::run(APP_ID, move |cx| {
        cx.stay_resident();
        let proxy = cx.proxy();
        let bus = Arc::new(Mutex::new(None));
        // Another notification service may have the name (a desktop
        // started before ours): wait for it to go rather than take it.
        let (slot, to_ui) = (bus.clone(), proxy.clone());
        std::thread::Builder::new()
            .name("notifications-bus".into())
            .spawn(move || {
                let mut said = false;
                loop {
                    let to_ui = to_ui.clone();
                    match Bus::serve(address.as_deref(), move |m| to_ui.send(Msg::In(m))) {
                        Ok(b) => {
                            *slot.lock().unwrap() = Some(b);
                            return;
                        }
                        Err(e) if !said => {
                            eprintln!("tabula-notifications: can't serve {} yet ({e}); trying every 30 s", server::NAME);
                            said = true;
                        }
                        Err(_) => {}
                    }
                    std::thread::sleep(Duration::from_secs(30));
                }
            })
            .expect("spawn");
        Notifications { bus, win: None, cards: Vec::new(), height: 0, proxy }
    });
    std::process::ExitCode::SUCCESS
}
