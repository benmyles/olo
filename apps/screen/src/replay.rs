//! A screen's history in the window: its actions in a list (a search
//! narrows it), the screen as it was at the one chosen or wherever the
//! scrubber is, and playback in time with the actions (play and pause,
//! speed, stepping action by action), each click shown as a ring where it
//! went and each key or text under the picture. Stretches where nothing
//! happened are skipped while playing. Frames are built on a worker
//! (`record::frame_at`), the latest asked for winning; the window never
//! waits on the disk.

use std::sync::{
    Arc,
    mpsc::{Sender, channel},
};

use olo_kit::{
    Proxy,
    egui::{self, Align, Color32, ColorImage, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2},
    metrics as m,
    search::SearchField,
    style,
    widgets::{Button, Cell, Column, ColumnListView, ListState, Slider},
};

use crate::{
    history::{self, Action},
    record,
};

pub enum ReplayMsg {
    Loaded { name: String, actions: Vec<Action>, segments: Arc<Vec<record::Segment>> },
    Frame { name: String, at: u64, image: ColorImage },
}

/// How long a click's ring and a key's caption stay over the picture.
const RING_MS: u64 = 700;
const CAPTION_MS: u64 = 1500;
/// While playing, a stretch longer than this with nothing in it is
/// skipped (to just before what comes next).
const SKIP_MS: u64 = 3000;
const SPEEDS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
const LIST_W: f32 = 300.0;

pub struct Replay {
    pub name: String,
    actions: Vec<Action>,
    segments: Arc<Vec<record::Segment>>,
    loaded: bool,
    query: String,
    shown: Vec<usize>,
    list: ListState,
    playhead: u64,
    playing: bool,
    speed: usize,
    texture: Option<TextureHandle>,
    /// The time of the frame showing, and the one last asked for.
    showing: Option<u64>,
    asked: Option<u64>,
    frames: Sender<(Arc<Vec<record::Segment>>, u64)>,
    /// A frame come from the worker, until drawn.
    pending: Option<ColorImage>,
    /// The screen's logical size, from its start (for placing clicks).
    size: Option<(f32, f32)>,
}

impl Replay {
    pub fn new(name: &str, proxy: Proxy) -> Replay {
        let (tx, rx) = channel::<(Arc<Vec<record::Segment>>, u64)>();
        {
            let (name, proxy) = (name.to_owned(), proxy.clone());
            std::thread::spawn(move || {
                while let Ok(mut ask) = rx.recv() {
                    // Only the latest matters.
                    while let Ok(newer) = rx.try_recv() {
                        ask = newer;
                    }
                    if let Some((at, f)) = record::frame_at(&ask.0, ask.1) {
                        let image = ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
                        proxy.send(ReplayMsg::Frame { name: name.clone(), at, image });
                    }
                }
            });
        }
        let n = name.to_owned();
        std::thread::spawn(move || {
            let actions = history::read(&n);
            let segments = Arc::new(record::index(&n));
            proxy.send(ReplayMsg::Loaded { name: n, actions, segments });
        });
        Replay {
            name: name.to_owned(),
            actions: Vec::new(),
            segments: Arc::default(),
            loaded: false,
            query: String::new(),
            shown: Vec::new(),
            list: ListState::default(),
            playhead: 0,
            playing: false,
            speed: 0,
            texture: None,
            showing: None,
            asked: None,
            frames: tx,
            pending: None,
            size: None,
        }
    }

    pub fn message(&mut self, msg: ReplayMsg) {
        match msg {
            ReplayMsg::Loaded { name, actions, segments } if name == self.name => {
                self.size = actions.iter().find(|a| a.kind == "start").and_then(|a| a.result.as_ref()).and_then(|r| Some((r["width"].as_f64()? as f32, r["height"].as_f64()? as f32)));
                self.actions = actions;
                self.segments = segments;
                self.loaded = true;
                self.filter();
                // At the end: the screen as it was last.
                self.playhead = self.range().1;
            }
            ReplayMsg::Frame { name, at, image } if name == self.name => {
                self.pending = Some(image);
                self.showing = Some(at);
            }
            _ => {}
        }
    }

    fn filter(&mut self) {
        let q = self.query.trim();
        self.shown = (0..self.actions.len()).filter(|&i| q.is_empty() || self.actions[i].mentions(q)).collect();
    }

    /// From the first thing kept to the last.
    fn range(&self) -> (u64, u64) {
        let frames = self.segments.iter().flat_map(|s| s.frames.iter().map(|f| f.at));
        let actions = self.actions.iter().flat_map(|a| [a.at, a.at + a.ms]);
        let (lo, hi) = frames.chain(actions).fold((u64::MAX, 0), |(lo, hi), t| (lo.min(t), hi.max(t)));
        if lo > hi { (0, 0) } else { (lo, hi) }
    }

    /// The time of the frame showing the screen at `t`.
    fn frame_for(&self, t: u64) -> Option<u64> {
        self.segments.iter().flat_map(|s| s.frames.iter()).filter(|f| f.at <= t).map(|f| f.at).max()
    }

    /// The next thing after `t`: a frame or an action.
    fn next_after(&self, t: u64) -> Option<u64> {
        let frames = self.segments.iter().flat_map(|s| s.frames.iter().map(|f| f.at));
        frames.chain(self.actions.iter().map(|a| a.at)).filter(|&x| x > t).min()
    }

    /// The last action begun by `t`.
    fn action_at(&self, t: u64) -> Option<usize> {
        self.actions.iter().rposition(|a| a.at <= t)
    }

    fn step(&mut self, by: isize) {
        let now = self.action_at(self.playhead);
        let to = match (now, by) {
            (None, 1..) => Some(0),
            (Some(i), _) => Some((i as isize + by).clamp(0, self.actions.len() as isize - 1) as usize),
            _ => None,
        };
        if let Some(a) = to.and_then(|i| self.actions.get(i)) {
            self.playhead = a.at + a.ms;
            self.playing = false;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, rect: Rect) {
        let p = style::palette(ui.ctx());
        let c = style::c;
        if let Some(image) = self.pending.take() {
            match &mut self.texture {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture(format!("replay-{}", self.name), image, TextureOptions::LINEAR)),
            }
        }
        if !self.loaded {
            style::label(ui.painter(), rect.center().x, rect.center().y, Align::Center, "Reading the history…", style::plain_font(), c(p.panel_text));
            return;
        }
        let (start, end) = self.range();
        if end == 0 {
            style::label(ui.painter(), rect.center().x, rect.center().y, Align::Center, "Nothing has been done on this screen yet.", style::plain_font(), c(p.panel_text));
            return;
        }

        // The bar: search, stepping, play, speed.
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), m::CONTROL_HEIGHT));
        let field = Rect::from_min_size(bar.min, vec2(LIST_W, m::CONTROL_HEIGHT));
        if ui.put(field, SearchField::new(("replay-search", &self.name), &mut self.query).hint("Search the actions")).changed() {
            self.filter();
        }
        let mut x = field.right() + m::SPACING;
        let mut button = |ui: &mut Ui, label: &str, w: f32| {
            let r = Rect::from_min_size(pos2(x, bar.top()), vec2(w, m::CONTROL_HEIGHT));
            x += w + m::TIGHT;
            ui.put(r, Button::new(label).min_width(w)).clicked()
        };
        if button(ui, "◀", 32.0) {
            self.step(-1);
        }
        if button(ui, if self.playing { "Pause" } else { "Play" }, 72.0) {
            self.playing = !self.playing;
            if self.playing && self.playhead >= end {
                self.playhead = start;
            }
        }
        if button(ui, "▶", 32.0) {
            self.step(1);
        }
        if button(ui, &format!("{}×", SPEEDS[self.speed]), 44.0) {
            self.speed = (self.speed + 1) % SPEEDS.len();
        }
        let when = history::clock(self.playhead);
        style::label(ui.painter(), bar.right(), bar.center().y, Align::Max, &when, style::plain_font(), c(p.panel_text));

        // Playing: on through time, past stretches where nothing happens.
        if self.playing {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.playhead += (dt * 1000.0 * SPEEDS[self.speed]) as u64;
            if let Some(next) = self.next_after(self.playhead)
                && next > self.playhead + SKIP_MS
            {
                self.playhead = next - 1000;
            }
            if self.playhead >= end {
                self.playhead = end;
                self.playing = false;
            }
            ui.ctx().request_repaint();
        }

        // The actions, the one at the playhead selected.
        let body = Rect::from_min_max(pos2(rect.left(), bar.bottom() + m::SPACING), rect.max);
        let list_r = Rect::from_min_size(body.min, vec2(LIST_W, body.height()));
        let current = self.action_at(self.playhead);
        let selected = current.and_then(|i| self.shown.iter().position(|&s| s == i));
        if selected != self.list.single() {
            self.list.selected = selected.into_iter().collect();
        }
        let columns = [Column::new("Time", 90.0), Column::new("Action", LIST_W - 92.0)];
        let (actions, shown) = (&self.actions, &self.shown);
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new(("replay-list", &self.name), &columns, shown.len(), &mut self.list).reveal(selected).show(ui, |cell: &Cell| {
                    let a = &actions[shown[cell.row]];
                    match cell.column {
                        0 => cell.text(history::clock(a.at).get(11..19).unwrap_or("")),
                        _ => {
                            let what = a.matched().map(|m| format!("{} {m}", a.kind)).unwrap_or_else(|| format!("{} {}", a.kind, a.args.join(" ")));
                            cell.text(&if a.error.is_some() { format!("✗ {what}") } else { what });
                        }
                    }
                })
            })
            .inner;
        if out.selection_changed
            && let Some(a) = self.list.single().and_then(|r| self.shown.get(r)).and_then(|&i| self.actions.get(i))
        {
            self.playhead = a.at + a.ms;
            self.playing = false;
        }

        // The screen as it was, fitted, with the scrubber under it.
        let right = Rect::from_min_max(pos2(list_r.right() + m::SPACING, body.top()), body.max);
        let scrub_r = Rect::from_min_max(pos2(right.left(), right.bottom() - 40.0), right.max);
        let area = Rect::from_min_max(right.min, pos2(right.right(), scrub_r.top() - m::SPACING));
        let want = self.frame_for(self.playhead);
        if want.is_some() && want != self.showing && want != self.asked {
            self.asked = want;
            let _ = self.frames.send((self.segments.clone(), self.playhead));
        }
        let size = self.texture.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(16.0, 10.0));
        let aspect = size.x / size.y.max(1.0);
        let (w, h) = if area.width() / area.height() > aspect { (area.height() * aspect, area.height()) } else { (area.width(), area.width() / aspect) };
        let pic = Rect::from_center_size(area.center(), vec2(w, h)).shrink(1.0);
        let painter = ui.painter();
        painter.rect_filled(pic.expand(1.0), 0.0, c(p.control_border));
        match (&self.texture, want) {
            (Some(t), Some(_)) => {
                painter.image(t.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(pic, 0.0, c(p.document_background));
                style::label(painter, pic.center().x, pic.center().y, Align::Center, "No picture from then", style::plain_font(), c(p.panel_text));
            }
        }
        self.overlay(ui, pic);
        let mut secs = (self.playhead.saturating_sub(start)) as f32 / 1000.0;
        let total = (end - start) as f32 / 1000.0;
        let r = Rect::from_min_size(scrub_r.min, vec2(pic.width().max(200.0), scrub_r.height()));
        let text = format!("{secs:.0} of {total:.0} s");
        if ui.put(r, Slider::new(&mut secs, 0.0..=total.max(0.001)).value_text(text)).changed() {
            self.playhead = start + (secs * 1000.0) as u64;
            self.playing = false;
        }
    }

    /// Each click as a ring where it went, each key or text under the
    /// picture, for a moment after it.
    fn overlay(&self, ui: &Ui, pic: Rect) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        let t = self.playhead;
        for a in self.actions.iter().filter(|a| a.at <= t) {
            let since = t - a.at;
            let point = a.result.as_ref().and_then(|r| r.get("clicked")).and_then(|c| Some((c.get(0)?.as_f64()? as f32, c.get(1)?.as_f64()? as f32)));
            if let (Some((x, y)), Some((sw, sh))) = (point, self.size)
                && since < RING_MS
            {
                let at = pos2(pic.left() + x / sw * pic.width(), pic.top() + y / sh * pic.height());
                let grow = since as f32 / RING_MS as f32;
                let color = style::c(p.keyboard_navigation).gamma_multiply(1.0 - grow);
                painter.circle_stroke(at, 6.0 + 10.0 * grow, egui::Stroke::new(2.0, color));
            }
            if (a.kind == "key" || a.kind == "type") && since < CAPTION_MS {
                let text = if a.kind == "type" { format!("“{}”", a.args.join(" ")) } else { a.args.join(" ") };
                let g = painter.layout_no_wrap(text, style::bold_font(), Color32::WHITE);
                let r = Rect::from_center_size(pos2(pic.center().x, pic.bottom() - 24.0), g.size() + vec2(16.0, 8.0));
                painter.rect_filled(r, 6.0, Color32::from_black_alpha(170));
                painter.galley(pos2(r.left() + 8.0, r.top() + 4.0), g, Color32::WHITE);
            }
        }
    }
}
