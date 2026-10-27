//! tabula-notifications: the session's notification service. Apps'
//! notifications (org.freedesktop.Notifications: notify-send, browsers,
//! chat and mail) appear in the main display's lower right corner,
//! grouped by app under its name as on Haiku: the app's icon or picture,
//! the summary in bold, the body (bold, italics and links kept), a bar
//! for progress, its actions as buttons and, when it can be answered, a
//! reply field. Important ones wear a blue edge, errors a red one and
//! stay. A click on a card is its default action (the app gets an
//! activation token first, so its window comes forward) or brings the
//! app's window forward; the close box dismisses it; it goes by itself
//! after a while, not while the pointer is over it.
//!
//! Only the real session runs it (a test or nested session must not take
//! the user's notifications); `--bus ADDRESS` serves a private bus.

mod markup;
mod server;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use server::{Bus, In, Kind, Note, Reason};
use tabula_kit::{
    App, Cx, Layer, LayerOptions, WindowId, WindowKind, WindowOptions,
    egui::{self, Align, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets::{Button, StatusBar, TextControl},
};

const APP_ID: &str = "tabula.notifications";
/// A card's width (logical).
const WIDTH: f32 = 340.0;
const PAD: f32 = 10.0;
const ICON: f32 = 32.0;
/// Between groups, and from the display's edges.
const GAP: f32 = 8.0;
/// A group's title row: the app's icon and name.
const HEADER: f32 = 22.0;
/// At most this many at once; the rest wait their turn.
const SHOWN: usize = 4;
const BODY_ROWS: usize = 5;
/// How long a click waits for its activation token before the action
/// goes without one.
const TOKEN_WAIT: Duration = Duration::from_millis(500);

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
    /// What is typed into its reply field.
    reply: String,
}

/// An action chosen, waiting for the activation token to send with it.
struct Pending {
    id: u32,
    key: String,
    /// Say it closed, after the action.
    close: bool,
    since: Instant,
}

struct Notifications {
    bus: Arc<Mutex<Option<Bus>>>,
    win: Option<WindowId>,
    /// Oldest first.
    cards: Vec<Card>,
    /// The window's height last asked for.
    height: u32,
    /// Whether the window takes the keyboard (a reply field shows).
    keyboard: bool,
    pending: HashMap<u64, Pending>,
    next_tag: u64,
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
        match self.cards.iter_mut().find(|c| c.note.id == note.id) {
            // In place: what was typed stays, the picture is made again.
            Some(old) => {
                old.note = note;
                old.until = until;
                old.picture = None;
            }
            None => self.cards.push(Card { note, until, picture: None, reply: String::new() }),
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
            self.keyboard = false;
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
        if self.take(cx, id) {
            self.bus(|b| b.closed(id, reason));
        }
    }

    /// Take a card away (saying nothing); whether it was there.
    fn take(&mut self, cx: &mut Cx, id: u32) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.note.id != id);
        let was = self.cards.len() != before;
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
        was
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

    /// Run action `key` of notification `id`: the app gets an activation
    /// token first (from this click), so it may bring its window forward.
    /// The card goes at once; the app hears it closed after the action.
    fn invoke(&mut self, cx: &mut Cx, id: u32, key: String) {
        let resident = self.cards.iter().any(|c| c.note.id == id && c.note.resident);
        let pending = Pending { id, key, close: !resident, since: Instant::now() };
        // The token first: taking the last card closes the window.
        match self.win {
            Some(win) => {
                let tag = self.next_tag;
                self.next_tag += 1;
                self.pending.insert(tag, pending);
                cx.activation_token(win, tag);
                self.tick_after(TOKEN_WAIT);
            }
            None => self.send(pending, None),
        }
        if !resident {
            self.take(cx, id);
        }
    }

    /// Token (when there is one), action, then closed, in the spec's order.
    fn send(&self, p: Pending, token: Option<&str>) {
        self.bus(|b| {
            if let Some(t) = token {
                b.activation_token(p.id, t);
            }
            b.action(p.id, &p.key);
            if p.close {
                b.closed(p.id, Reason::Dismissed);
            }
        });
    }

    /// Actions whose token never came go without one.
    fn flush_pending(&mut self) {
        let late: Vec<u64> = self.pending.iter().filter(|(_, p)| p.since.elapsed() >= TOKEN_WAIT).map(|(t, _)| *t).collect();
        for tag in late {
            if let Some(p) = self.pending.remove(&tag) {
                self.send(p, None);
            }
        }
    }
}

/// Bring forward a window of the app that sent `note` (a click on one
/// with no default action), off the UI thread.
fn raise_app(note: &Note) {
    let mut ids: Vec<String> = note.desktop_entry.iter().map(|e| e.trim_end_matches(".desktop").to_lowercase()).collect();
    if !note.app_name.is_empty() {
        ids.push(note.app_name.to_lowercase());
        ids.push(note.app_name.to_lowercase().replace(' ', "-"));
        // Ours by name ("StyledEdit" is tabula.styled-edit).
        for (bin, name, _) in tabula_xdg::apps::OUR_APPS {
            if name.eq_ignore_ascii_case(&note.app_name) {
                ids.push(bin.replacen("tabula-", "tabula.", 1));
            }
        }
    }
    std::thread::spawn(move || {
        use tabula_protocol::{Client, Reply, Request};
        let Ok(mut client) = Client::connect() else { return };
        let Ok(Reply::Windows { windows }) = client.request(&Request::ListWindows) else { return };
        let matches = |app_id: &str| {
            let a = app_id.to_lowercase();
            ids.iter().any(|i| *i == a || a.rsplit('.').next() == Some(i.as_str()) || i.rsplit('.').next() == Some(a.as_str()))
        };
        if let Some(w) = windows.iter().find(|w| matches(&w.app_id)) {
            let _ = client.request(&Request::Activate { id: w.id });
        }
    });
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
    names.extend(app_icons(note));
    names.push(if note.kind == Kind::Error { "warning" } else { "info" }.into());
    names
}

/// The sending app's own icon names.
fn app_icons(note: &Note) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(e) = &note.desktop_entry {
        names.extend(tabula_xdg::icon_for_app(e.trim_end_matches(".desktop")));
    }
    let app = note.app_name.to_lowercase().replace(' ', "-");
    if !app.is_empty() {
        names.extend(tabula_xdg::icon_for_app(&app));
        names.extend(tabula_xdg::apps::our_app(&app).map(|(_, icon)| icon.to_owned()));
    }
    names
}

/// Text laid out for a card.
fn galley(ui: &egui::Ui, text: &str, color: Color32, width: f32, rows: usize, font: egui::FontId) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The body with its markup: bold, italics, underlines and links.
fn body_galley(ui: &egui::Ui, body: &[markup::Span], p: &tabula_kit::theme::Palette, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    for s in body {
        let font = match (s.bold, s.italic) {
            (true, _) => style::bold_font(),
            (false, true) => style::italic_font(),
            _ => style::plain_font(),
        };
        let mut f = egui::TextFormat::simple(font, if s.link.is_some() { c(p.keyboard_navigation) } else { c(p.panel_text) });
        // (There is no bold italic face: slanted bold.)
        f.italics = s.bold && s.italic;
        if s.underline || s.link.is_some() {
            f.underline = egui::Stroke::new(1.0, f.color);
        }
        job.append(&s.text, 0.0, f);
    }
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: BODY_ROWS, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The link under `pos` in a body drawn at `origin`.
fn link_at(body: &[markup::Span], g: &egui::Galley, origin: egui::Pos2, pos: egui::Pos2) -> Option<String> {
    let local = pos - origin;
    if !Rect::from_min_size(egui::Pos2::ZERO, g.size()).contains(local.to_pos2()) {
        return None;
    }
    let at = g.cursor_from_pos(local).index.0;
    let mut start = 0;
    for s in body {
        let n = s.text.chars().count();
        if at >= start && at < start + n {
            return s.link.clone();
        }
        start += n;
    }
    None
}

/// What happened to a card this frame.
enum Did {
    Dismiss,
    Action(String),
    /// A click with no default action: the app comes forward.
    Raise,
    Reply(String),
    Link(String),
}

impl App for Notifications {
    fn ui(&mut self, cx: &mut Cx, _window: WindowId, ui: &mut egui::Ui) {
        let p = style::palette(ui.ctx());
        let text_x = PAD + ICON + PAD;
        let text_w = WIDTH - text_x - PAD;
        let hover = ui.input(|i| i.pointer.hover_pos());
        // The shown ones, grouped by app (in the order the apps first
        // show), oldest first in each.
        let shown: Vec<usize> = (0..self.cards.len().min(SHOWN)).collect();
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for &i in &shown {
            let app = self.cards[i].note.app_name.clone();
            match groups.iter_mut().find(|(a, _)| *a == app) {
                Some((_, members)) => members.push(i),
                None => groups.push((app, vec![i])),
            }
        }
        // Lay them out.
        struct Laid {
            i: usize,
            rect: Rect,
            title: Arc<egui::Galley>,
            body: Arc<egui::Galley>,
        }
        let mut frames = Vec::new();
        let mut y = 0.0;
        for (app, members) in &groups {
            let top = y;
            y += HEADER;
            let mut laid = Vec::new();
            for &i in members {
                let card = &self.cards[i];
                let title = galley(ui, &card.note.summary, c(p.panel_text), text_w - 18.0, 2, style::bold_font());
                let body = body_galley(ui, &card.note.body, &p, text_w);
                let buttons = card.note.actions.iter().any(|(k, l)| k != "default" && !l.is_empty());
                let mut h = PAD + title.size().y;
                if !card.note.body.is_empty() {
                    h += m::TIGHT + body.size().y;
                }
                if card.note.progress.is_some() {
                    h += m::TIGHT + 10.0;
                }
                h = h.max(ICON + PAD) + PAD;
                if card.note.reply.is_some() {
                    h += m::CONTROL_HEIGHT + PAD;
                }
                if buttons {
                    h += m::CONTROL_HEIGHT + PAD;
                }
                let rect = Rect::from_min_size(pos2(0.0, y), vec2(WIDTH, h.ceil()));
                y += rect.height();
                laid.push(Laid { i, rect, title, body });
            }
            frames.push((app.clone(), Rect::from_min_max(pos2(0.0, top), pos2(WIDTH, y)), laid));
            y += GAP;
        }
        let total = (y - GAP).max(1.0).ceil() as u32;
        if total != self.height {
            self.height = total;
            if let Some(w) = self.win {
                cx.resize(w, (WIDTH as u32, total));
            }
        }
        // The keyboard only while there is something to type into.
        let wants_keys = shown.iter().any(|&i| self.cards[i].note.reply.is_some());
        if wants_keys != self.keyboard
            && let Some(w) = self.win
        {
            self.keyboard = wants_keys;
            cx.set_keyboard(w, if wants_keys { tabula_kit::Keyboard::OnDemand } else { tabula_kit::Keyboard::None });
        }
        let mut hovered = None;
        let mut did = Vec::new();
        let painter = ui.painter().clone();
        for (app, frame, laid) in frames {
            painter.rect_filled(frame, 0.0, c(p.panel_background));
            style::frame_rect(&painter, frame, style::darken(p.panel_background, 1.6));
            style::raised(&painter, frame.shrink(1.0), &p);
            // The group's title: the app's icon and name, and a close box
            // for all of them.
            let head = Rect::from_min_size(frame.min, vec2(WIDTH, HEADER));
            let first = &self.cards[laid[0].i].note;
            let small = Rect::from_min_size(pos2(head.left() + PAD, head.center().y - 8.0), vec2(16.0, 16.0));
            let mut names = app_icons(first);
            names.extend(icon_names(first).into_iter().filter(|n| n != "info" && n != "warning"));
            names.push("application".into());
            let _ = names.iter().any(|n| icons::paint(&painter, small, n));
            let name = if app.is_empty() { "Notification" } else { app.as_str() };
            style::label(&painter, small.right() + 6.0, head.center().y, Align::Min, name, egui::FontId::new(style::small_font().size, style::bold()), c(p.panel_text));
            if laid.len() > 1 {
                let all = Rect::from_center_size(pos2(head.right() - PAD - 6.0, head.center().y), vec2(12.0, 12.0));
                let r = ui.interact(all.expand(3.0), ui.make_persistent_id(("close-all", &app)), Sense::click());
                close_box(&painter, all, if r.hovered() { c(p.panel_text) } else { style::disabled_text(&p) });
                if r.clicked() {
                    for l in &laid {
                        did.push((self.cards[l.i].note.id, Did::Dismiss));
                    }
                }
            }
            style::hline(&painter, frame.left() + 2.0, frame.right() - 2.0, head.bottom() - 1.0, style::darken(p.panel_background, 1.25));
            for (n, Laid { i, rect, title, body }) in laid.into_iter().enumerate() {
                if n > 0 {
                    style::hline(&painter, rect.left() + PAD, rect.right() - PAD, rect.top(), style::darken(p.panel_background, 1.18));
                    style::hline(&painter, rect.left() + PAD, rect.right() - PAD, rect.top() + 1.0, c(p.shine));
                }
                let card = &mut self.cards[i];
                let id = card.note.id;
                if hover.is_some_and(|h| rect.contains(h)) {
                    hovered = Some(id);
                }
                // Important and error ones wear a coloured edge.
                let edge = match card.note.kind {
                    Kind::Important => Some(c(p.keyboard_navigation)),
                    Kind::Error => Some(c(p.failure)),
                    _ => None,
                };
                if let Some(color) = edge {
                    painter.rect_filled(Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(3.0, rect.height() - 4.0)), 0.0, color);
                }
                // The whole card is the default action.
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
                // Its close box.
                let x = rect.left() + text_x;
                let close = Rect::from_center_size(pos2(rect.right() - PAD - 6.0, rect.top() + PAD + 7.0), vec2(12.0, 12.0));
                let close_resp = ui.interact(close.expand(3.0), ui.make_persistent_id(("close", id)), Sense::click());
                close_box(&painter, close, if close_resp.hovered() { c(p.panel_text) } else { style::disabled_text(&p) });
                let mut ty = rect.top() + PAD;
                painter.galley(pos2(x, ty), title.clone(), c(p.panel_text));
                ty += title.size().y + m::TIGHT;
                let mut link = None;
                if !card.note.body.is_empty() {
                    let origin = pos2(x, ty);
                    painter.galley(origin, body.clone(), c(p.panel_text));
                    if let Some(pos) = hover {
                        link = link_at(&card.note.body, &body, origin, pos);
                        if link.is_some() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                    }
                    ty += body.size().y + m::TIGHT;
                }
                if let Some(f) = card.note.progress {
                    let r = Rect::from_min_size(pos2(x, ty), vec2(text_w, 10.0));
                    ui.put(r, StatusBar::new(f).bar_height(10.0));
                }
                let mut by = rect.bottom() - PAD - m::CONTROL_HEIGHT;
                let mut pressed = None;
                // Actions along the bottom, right to left as in an alert.
                let buttons: Vec<(String, String)> = card.note.actions.iter().filter(|(k, l)| k != "default" && !l.is_empty()).cloned().collect();
                if !buttons.is_empty() {
                    let mut bx = rect.right() - PAD;
                    for (key, label) in buttons.iter().rev() {
                        let w = (style::label_width(ui.ctx(), label, style::plain_font()) + 24.0).max(m::BUTTON_MIN_WIDTH);
                        let r = Rect::from_min_size(pos2(bx - w, by), vec2(w, m::CONTROL_HEIGHT));
                        if ui.put(r, Button::new(label.as_str())).clicked() {
                            pressed = Some(key.clone());
                        }
                        bx -= w + m::SPACING;
                    }
                    by -= m::CONTROL_HEIGHT + PAD;
                }
                // The reply field and its Send button.
                if let Some(placeholder) = card.note.reply.clone() {
                    let send_w = m::BUTTON_MIN_WIDTH;
                    let field = Rect::from_min_size(pos2(x, by), vec2(rect.right() - PAD - x - send_w - m::SPACING, m::CONTROL_HEIGHT));
                    let r = ui.put(field, TextControl::new(&mut card.reply).hint(&placeholder));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let send = Rect::from_min_size(pos2(rect.right() - PAD - send_w, by), vec2(send_w, m::CONTROL_HEIGHT));
                    let clicked = ui.add_enabled_ui(!card.reply.trim().is_empty(), |ui| ui.put(send, Button::new("Send")).clicked()).inner;
                    if (enter || clicked) && !card.reply.trim().is_empty() {
                        did.push((id, Did::Reply(card.reply.trim().to_owned())));
                    }
                }
                if close_resp.clicked() {
                    did.push((id, Did::Dismiss));
                } else if let Some(key) = pressed {
                    did.push((id, Did::Action(key)));
                } else if click.clicked() {
                    if let Some(url) = link {
                        did.push((id, Did::Link(url)));
                    } else if card.note.actions.iter().any(|(k, _)| k == "default") {
                        did.push((id, Did::Action("default".into())));
                    } else {
                        did.push((id, Did::Raise));
                    }
                }
            }
        }
        for (id, d) in did {
            match d {
                Did::Dismiss => self.remove(cx, id, Reason::Dismissed),
                Did::Action(key) => self.invoke(cx, id, key),
                Did::Raise => {
                    if let Some(card) = self.cards.iter().find(|c| c.note.id == id) {
                        raise_app(&card.note);
                    }
                    self.remove(cx, id, Reason::Dismissed);
                }
                Did::Reply(text) => {
                    self.bus(|b| b.replied(id, &text));
                    self.remove(cx, id, Reason::Dismissed);
                }
                Did::Link(url) => {
                    std::thread::spawn(move || {
                        if let Err(e) = tabula_kit::open::open(&tabula_kit::open::Target::parse(&url)) {
                            tracing::warn!("notifications: can't open {url}: {e}");
                        }
                    });
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
        let message = match message.downcast::<tabula_kit::ActivationToken>() {
            Ok(t) => {
                if let Some(p) = self.pending.remove(&t.tag) {
                    self.send(p, Some(&t.token));
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::In(In::Show(note)) => self.show(cx, *note),
            Msg::In(In::Close(id)) => self.remove(cx, id, Reason::Closed),
            Msg::Tick => {
                self.flush_pending();
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

/// A small ×.
fn close_box(painter: &egui::Painter, r: Rect, color: Color32) {
    let k = r.shrink(3.0);
    painter.line_segment([k.left_top(), k.right_bottom()], egui::Stroke::new(1.2, color));
    painter.line_segment([k.right_top(), k.left_bottom()], egui::Stroke::new(1.2, color));
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
        Notifications { bus, win: None, cards: Vec::new(), height: 0, keyboard: false, pending: HashMap::new(), next_tag: 0, proxy }
    });
    std::process::ExitCode::SUCCESS
}
