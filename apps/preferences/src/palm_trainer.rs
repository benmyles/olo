//! Train for Your Palm (Preferences ▸ Touchpad & Mouse ▸ Touch): a guided
//! run through exercises on the pad, each shown as a little animation of
//! hands on a pad and mirrored live by what the pad sees, with a word on
//! how it's going and how much more it needs; then Olo's own palm model
//! (`olo_palm::baseline`) is refined on what was gathered, off the UI
//! thread with progress, kept in the settings, and tried live: each
//! contact drawn as the model and the size rule each call it.
//!
//! Samples come from app_server (`EventKind::TouchContacts`) on a worker
//! that follows each contact (`olo_palm::Tracker`) and hands a finished
//! one over with the step's label when it lifts: in a step with palms
//! only, every contact is a palm; with fingers only, a finger; mixed, the
//! first contact down that stays still is the palm, the rest fingers.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use olo_palm::{Labelled, PadAxes, PalmModel, Report, TouchContact, Tracker};
use olo_protocol::{Client, Event, EventKind};
use olo_sync::Mutex;
use olo_ui::{
    agent,
    gpui::{App, AppContext as _, Bounds, Context, Entity, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, Render, Styled as _, Window, canvas, div, point, px, size},
    metrics as m, style, theme,
    widgets::{Button, StatusBar, StringView},
};

use crate::{Msg, Prefs};

/// The steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Intro,
    RestPalms,
    EdgePalm,
    TypeWithPalms,
    PointAndScroll,
    Taps,
    PointWithPalm,
    Training,
    Test,
}

const EXERCISES: [Step; 6] = [Step::RestPalms, Step::EdgePalm, Step::TypeWithPalms, Step::PointAndScroll, Step::Taps, Step::PointWithPalm];

impl Step {
    /// What to do, how many contacts it needs, and how its contacts are
    /// labelled (Some(true): all palms; Some(false): all fingers; None:
    /// the still first one a palm).
    fn plan(self) -> (&'static str, &'static str, usize, Option<bool>) {
        match self {
            Step::Intro => (
                "Personalize Olo's palm model",
                "Olo's palm model already tells palms from fingers out of the box. This refines it to your own hands: six short exercises on the touchpad, then a try. Rest your hands as you really do.",
                0,
                None,
            ),
            Step::RestPalms => (
                "Rest your palms",
                "Rest the heels of both hands on the pad as they fall when you type, lift them, and rest them again. Don't use your fingers.",
                12,
                Some(true),
            ),
            Step::EdgePalm => (
                "One hand, its palm over the edge",
                "Using one hand alone, rest its heel half over the pad's left edge, as it falls when that hand types or rests by itself; lift it and rest it again. Then the same over the right edge. Keep your fingers off the glass.",
                14,
                Some(true),
            ),
            Step::TypeWithPalms => (
                "Type with your palms resting",
                "Type a few lines on the keyboard with your palms resting on the pad as usual, lifting and resting them as you go.",
                16,
                Some(true),
            ),
            Step::PointAndScroll => (
                "Point and scroll",
                "With your palms off the pad, move the pointer around, then scroll up and down with two fingers.",
                20,
                Some(false),
            ),
            Step::Taps => (
                "Tap and click",
                "Tap with one finger, then two, and click, here and there on the pad. Palms off.",
                16,
                Some(false),
            ),
            Step::PointWithPalm => (
                "Point with a palm resting",
                "Rest one palm at or over the pad's edge, as you do with one hand, and keep it still. With a finger, of that hand or the other, point, scroll and tap.",
                20,
                None,
            ),
            Step::Training => ("Refining Olo's model", "", 0, None),
            Step::Test => (
                "Try it",
                "Rest a palm, point and tap. Each touch is drawn as the model calls it: blue a finger, orange a palm. The ring shows the size rule's call.",
                0,
                None,
            ),
        }
    }
}

/// What the worker shares with the panel.
#[derive(Default)]
struct Live {
    axes: Option<PadAxes>,
    contacts: Vec<TouchContact>,
    /// Finished contacts with their features, this step's.
    done: Vec<Labelled>,
    /// A word on the last contact: a finger where palms were wanted, or
    /// the other way (by the baseline's call).
    warn: Option<&'static str>,
    /// The worker's connection, shut to end it at once.
    connection: Option<std::os::unix::net::UnixStream>,
}

pub struct Trainer {
    pub step: Step,
    samples: Vec<Labelled>,
    live: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    /// The step the worker labels for (its index in EXERCISES, or usize::MAX).
    labelling: Arc<AtomicUsize>,
    started: Instant,
    /// A worker follows the pad.
    watching: bool,
    pub progress: Arc<Mutex<f32>>,
    pub report: Option<Report>,
    pub model: Option<PalmModel>,
    pub failed: Option<String>,
}

impl Drop for Trainer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // (Its worker waits on app_server: shut, it ends at once, and
        // app_server stops reading the pad for it.)
        if let Some(c) = self.live.lock().unwrap().connection.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Trainer {
    pub fn new() -> Trainer {
        Trainer {
            step: Step::Intro,
            samples: Vec::new(),
            live: Default::default(),
            stop: Default::default(),
            labelling: Arc::new(AtomicUsize::new(usize::MAX)),
            started: Instant::now(),
            watching: false,
            progress: Default::default(),
            report: None,
            model: None,
            failed: None,
        }
    }

    /// Start (or go on to the next step).
    pub fn advance(&mut self, proxy: olo_ui::Proxy) {
        let next = match self.step {
            Step::Intro => EXERCISES[0],
            s if EXERCISES.contains(&s) => {
                self.samples.extend(std::mem::take(&mut self.live.lock().unwrap().done));
                let i = EXERCISES.iter().position(|e| *e == s).unwrap();
                EXERCISES.get(i + 1).copied().unwrap_or(Step::Training)
            }
            Step::Training => Step::Test,
            _ => Step::Intro,
        };
        self.go(next, proxy);
    }

    /// Do this step again.
    pub fn redo(&mut self, proxy: olo_ui::Proxy) {
        self.live.lock().unwrap().done.clear();
        self.go(self.step, proxy);
    }

    fn go(&mut self, step: Step, proxy: olo_ui::Proxy) {
        self.step = step;
        self.started = Instant::now();
        self.labelling.store(EXERCISES.iter().position(|e| *e == step).unwrap_or(usize::MAX), Ordering::Relaxed);
        {
            let mut l = self.live.lock().unwrap();
            l.done.clear();
            l.warn = None;
        }
        if step == Step::Intro {
            self.samples.clear();
            self.model = None;
            self.report = None;
            self.failed = None;
            return;
        }
        if self.stopped() {
            self.watch(proxy);
        }
    }

    /// The samples so far, for training.
    pub fn take_samples(&mut self) -> Vec<Labelled> {
        self.samples.clone()
    }

    /// Whether nothing follows the pad (never started, or stopped).
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || !self.watching
    }

    /// Follow the pad until stopped (the window closing).
    fn watch(&mut self, proxy: olo_ui::Proxy) {
        self.watching = true;
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Default::default();
        let (live, stop, labelling) = (self.live.clone(), self.stop.clone(), self.labelling.clone());
        let _ = std::thread::Builder::new().name("palm-trainer".into()).spawn(move || {
            let base = olo_palm::baseline();
            let mut tracker = Tracker::default();
            // Each contact's frames in its first decide_ms, its largest
            // size, when it landed, and whether it has moved.
            let mut open: std::collections::HashMap<i32, (Vec<[f32; olo_palm::FEATURES]>, i32, u64, bool, (i32, i32))> = Default::default();
            while !stop.load(Ordering::Relaxed) {
                let Some(client) = Client::connect().ok() else {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                };
                live.lock().unwrap().connection = client.closer().ok();
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(events) = client.into_events_of(&[EventKind::TouchContacts]) else {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                };
                for e in events {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(Event::TouchContacts { pad, time_us, contacts, since_key_us }) = e else {
                        break;
                    };
                    tracker.since_key_us = since_key_us;
                    let step = labelling.load(Ordering::Relaxed);
                    let label = EXERCISES.get(step).map(|s| s.plan().3);
                    let frames = tracker.frame(&pad, time_us, &contacts);
                    let mut l = live.lock().unwrap();
                    l.axes = Some(pad.clone());
                    for (c, (_, f, age)) in contacts.iter().zip(&frames) {
                        let o = open.entry(c.id).or_insert((Vec::new(), 0, time_us, false, (c.x, c.y)));
                        if (olo_palm::MIN_AGE_MS..=olo_palm::DECIDE_MS).contains(age) {
                            o.0.push(*f);
                        }
                        o.1 = o.1.max(c.major.max(c.minor));
                        if (c.x - o.4.0).abs() + (c.y - o.4.1).abs() > 400 {
                            o.3 = true;
                        }
                    }
                    // Lifted: labelled and kept (if a step wants it).
                    let lifted: Vec<i32> = open.keys().filter(|id| !contacts.iter().any(|c| c.id == **id)).copied().collect();
                    for id in lifted {
                        let (frames, size, down, moved, _) = open.remove(&id).unwrap();
                        let Some(label) = label else { continue };
                        if frames.is_empty() {
                            continue;
                        }
                        let palm = match label {
                            Some(p) => p,
                            // The first down, still for over a second: the palm.
                            None => !moved && time_us.saturating_sub(down) > 1_000_000 && open.values().all(|o| o.2 >= down),
                        };
                        let says_palm = frames.iter().any(|f| olo_palm::score(&base, f) > base.threshold);
                        l.warn = match (label, says_palm) {
                            (Some(true), false) => Some("That looked like a finger: rest only your palms here."),
                            (Some(false), true) => Some("That looked like a palm: keep your palms off the pad here."),
                            _ => None,
                        };
                        l.done.push(Labelled { frames, palm, size });
                    }
                    l.contacts = contacts;
                    drop(l);
                    proxy.send(Msg::Redraw);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
}

/// The pad drawn in `r`: its outline, and each contact as an ellipse of
/// its size (orange for a palm, blue for a finger, by `palm`), with a
/// ring where the size rule disagrees.
fn draw_pad(window: &mut olo_ui::gpui::Window, r: Bounds<Pixels>, axes: Option<&PadAxes>, contacts: &[(TouchContact, bool, bool)], p: &olo_theme::Palette) {
    // The pad's own shape (its size in millimetres where it says, a
    // MacBook's until it has), fitted and centred.
    let aspect = match axes {
        Some(a) if a.x_res > 0 && a.y_res > 0 => ((a.x_max - a.x_min) as f32 / a.x_res as f32) / ((a.y_max - a.y_min).max(1) as f32 / a.y_res as f32),
        Some(a) => (a.x_max - a.x_min) as f32 / (a.y_max - a.y_min).max(1) as f32,
        None => 157.0 / 96.0,
    };
    let (rw, rh) = (f32::from(r.size.width), f32::from(r.size.height));
    let (w, h) = if rw / rh > aspect { (rh * aspect, rh) } else { (rw, rw / aspect) };
    let r = style::snap_centred(window, r.center(), w, h);
    style::sunken(window, r, p, false);
    let inner = style::inset(window, r, 2.0);
    style::fill_rect(window, inner, style::shade(p, p.panel_background, 1.08));
    let Some(a) = axes else { return };
    let (w, h) = ((a.x_max - a.x_min).max(1) as f32, (a.y_max - a.y_min).max(1) as f32);
    // (Within the pad: a palm at its edge is cut by it, as on the glass.)
    window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: inner }), |window| {
        for (c, palm, by_size) in contacts {
            let cx = inner.left() + inner.size.width * ((c.x - a.x_min) as f32 / w);
            let cy = inner.top() + inner.size.height * ((c.y - a.y_min) as f32 / h);
            // Its size in the pad's units is a diameter in the pad's
            // coordinate units (both 98/mm on a MacBook).
            let rx = (c.major.max(c.minor) as f32 / w * f32::from(inner.size.width) / 2.0).max(3.0);
            let ry = (c.major.min(c.minor) as f32 / h * f32::from(inner.size.height) / 2.0).max(3.0);
            let ink = if *palm { p.warning } else { p.navigation_base };
            ellipse(window, point(cx, cy), rx, ry, style::c(ink).opacity(0.75));
            if palm != by_size {
                ring(window, point(cx, cy), rx + 3.0, ry + 3.0, style::c(p.failure));
            }
        }
    });
}

fn ellipse(window: &mut olo_ui::gpui::Window, c: olo_ui::gpui::Point<Pixels>, rx: f32, ry: f32, ink: Hsla) {
    let mut b = PathBuilder::fill();
    for k in 0..=32 {
        let t = k as f32 / 32.0 * std::f32::consts::TAU;
        let pt = point(c.x + px(rx * t.cos()), c.y + px(ry * t.sin()));
        if k == 0 { b.move_to(pt) } else { b.line_to(pt) }
    }
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, ink);
    }
}

fn ring(window: &mut olo_ui::gpui::Window, c: olo_ui::gpui::Point<Pixels>, rx: f32, ry: f32, ink: Hsla) {
    let mut b = PathBuilder::stroke(style::hairline(window) * 2.0);
    for k in 0..=32 {
        let t = k as f32 / 32.0 * std::f32::consts::TAU;
        let pt = point(c.x + px(rx * t.cos()), c.y + px(ry * t.sin()));
        if k == 0 { b.move_to(pt) } else { b.line_to(pt) }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, ink);
    }
}

/// The step's little film: hands on a pad, in Olo's flat style, moving
/// with `t` seconds.
fn animation(window: &mut olo_ui::gpui::Window, r: Bounds<Pixels>, step: Step, t: f32, p: &olo_theme::Palette) {
    // A laptop's deck in a sunken well: rows of keys above, the pad
    // below between the palm rests, as the hands find them.
    style::sunken(window, r, p, false);
    let deck = style::inset(window, r, 2.0);
    style::fill_rect(window, deck, style::shade(p, p.panel_background, 0.94));
    let (dw, dh) = (f32::from(deck.size.width), f32::from(deck.size.height));
    let keys = Bounds::from_corners(deck.origin + point(px(dw * 0.08), px(dh * 0.06)), deck.origin + point(px(dw * 0.92), px(dh * 0.40)));
    let (rows, cols) = (4, 12);
    let (kw, kh) = (f32::from(keys.size.width) / cols as f32, f32::from(keys.size.height) / rows as f32);
    for row in 0..rows {
        for col in 0..cols {
            let k = style::snap(
                window,
                Bounds::new(
                    keys.origin + point(px(kw * col as f32 + 1.0), px(kh * row as f32 + 1.0)),
                    size(px(kw - 2.0), px(kh - 2.0)),
                ),
            );
            style::raised(window, k, p);
        }
    }
    // The pad: a MacBook's shape (157 × 96 mm), centred under the keys.
    let pad_w = dw * 0.56;
    let pad_h = (pad_w / 1.64).min(dh * 0.4);
    let pad_top = dh * 0.5;
    let pad = style::snap(
        window,
        Bounds::new(deck.origin + point(px((dw - pad_w) / 2.0), px(pad_top)), size(px(pad_w), px(pad_h))),
    );
    style::sunken(window, pad, p, false);
    let pad = style::inset(window, pad, 2.0);
    style::fill_rect(window, pad, style::shade(p, p.panel_background, 1.08));
    let hand = style::c(p.panel_text).opacity(0.35);
    let finger = style::c(p.navigation_base);
    let palm = style::c(p.warning);
    let pw = f32::from(pad.size.width);
    let ph = f32::from(pad.size.height);
    // A palm's heel at each of the pad's upper outer edges, half on the
    // deck (where a typing hand rests on a MacBook), down or lifted.
    let heel = |window: &mut olo_ui::gpui::Window, left: bool, down: f32| {
        let x = if left { pad.left() + px(pw * 0.04) } else { pad.right() - px(pw * 0.04) };
        let y = pad.top() + px(ph * 0.32) - px((1.0 - down) * 8.0);
        let ink = if down > 0.5 { palm.opacity(0.85) } else { hand };
        ellipse(window, point(x, y), pw * 0.16, ph * 0.2, ink);
    };
    let pulse = (t * 1.2).sin() * 0.5 + 0.5;
    match step {
        Step::RestPalms => {
            let down = if (t % 4.0) < 2.5 { 1.0 } else { 0.0 };
            heel(window, true, down);
            heel(window, false, down);
        }
        Step::TypeWithPalms => {
            heel(window, true, 1.0);
            heel(window, false, 1.0);
            // Fingers on the home row, each pressing a key in turn.
            let home = keys.top() + px(kh * 2.5);
            for k in 0..8 {
                let col = if k < 4 { 1 + k } else { 3 + k };
                let x = keys.left() + px(kw * (col as f32 + 0.5));
                let pressing = ((t * 5.0) as usize) % 8 == k;
                ellipse(
                    window,
                    point(x, home),
                    if pressing { 4.0 } else { 5.0 },
                    if pressing { 4.0 } else { 5.0 },
                    if pressing { finger } else { finger.opacity(0.6) },
                );
            }
        }
        Step::PointAndScroll => {
            let phase = t % 6.0;
            if phase < 3.0 {
                let (x, y) = (0.5 + 0.3 * (t * 1.5).cos(), 0.5 + 0.25 * (t * 1.1).sin());
                ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), 6.0, 7.0, finger);
            } else {
                let y = 0.5 + 0.3 * (t * 2.0).sin();
                ellipse(window, point(pad.left() + px(pw * 0.45), pad.top() + px(ph * y)), 6.0, 7.0, finger);
                ellipse(window, point(pad.left() + px(pw * 0.55), pad.top() + px(ph * y)), 6.0, 7.0, finger);
            }
        }
        Step::Taps => {
            let k = (t * 1.5) as i32 % 4;
            let (x, y) = [(0.3, 0.4), (0.7, 0.6), (0.5, 0.3), (0.4, 0.7)][k as usize];
            let down = ((t * 1.5) % 1.0) < 0.3;
            ellipse(
                window,
                point(pad.left() + px(pw * x), pad.top() + px(ph * y)),
                if down { 7.0 } else { 4.0 },
                if down { 8.0 } else { 4.0 },
                if down { finger } else { hand },
            );
        }
        Step::EdgePalm => {
            // One heel, half over the left edge for a while, then the
            // right, resting and lifting.
            let left = (t % 7.0) < 3.5;
            let down = if (t % 3.5) < 2.4 { 1.0 } else { 0.0 };
            let x = if left { pad.left() - px(pw * 0.03) } else { pad.right() + px(pw * 0.03) };
            let y = pad.top() + px(ph * 0.4) - px((1.0 - down) * 8.0);
            ellipse(window, point(x, y), pw * 0.17, ph * 0.22, if down > 0.5 { palm.opacity(0.85) } else { hand });
        }
        Step::PointWithPalm => {
            heel(window, false, 1.0);
            let (x, y) = (0.35 + 0.2 * (t * 1.5).cos(), 0.45 + 0.25 * (t * 1.1).sin());
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), 6.0, 7.0, finger);
        }
        Step::Test => {
            heel(window, true, 1.0);
            ellipse(window, point(pad.center().x, pad.center().y), 6.0 + pulse * 2.0, 7.0 + pulse * 2.0, finger);
        }
        _ => {}
    }
}

/// The trainer's window: a dialog of its own (the Touch tab's box ran
/// off the panel), opened from "Train for Your Palm…".
pub struct TrainerWindow {
    trainer: Trainer,
    prefs: Entity<Prefs>,
    palm_size: u32,
}

const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 440.0;

/// Open it (one at a time: a second ask brings nothing new).
pub fn open(prefs: Entity<Prefs>, palm_size: u32, cx: &mut App) {
    let options = olo_ui::WindowOptions { fixed: true, ..olo_ui::WindowOptions::new("Train for Your Palm", WIDTH, HEIGHT).min_size(WIDTH, HEIGHT).dialog() };
    let _ = olo_ui::open_window(cx, options, move |_, cx| {
        let mut trainer = Trainer::new();
        trainer.watch(olo_ui::proxy(cx));
        cx.new(|_| TrainerWindow { trainer, prefs, palm_size })
    });
}

impl TrainerWindow {
    /// Refine Olo's palm model on the samples, off the UI thread with
    /// progress; keep it in the settings (and turn it on).
    fn train(&mut self, cx: &mut Context<Self>) {
        let samples = self.trainer.take_samples();
        let progress = self.trainer.progress.clone();
        *progress.lock().unwrap() = 0.0;
        let (palm_size, proxy) = (self.palm_size, olo_ui::proxy(cx));
        // Kept on disk too (~/.local/state/olo/palm), for the baseline to
        // be tuned on real hands later.
        let kept = samples.clone();
        std::thread::spawn(move || match olo_palm::keep_samples(&olo_xdg::state_dir().join("olo").join("palm"), &kept) {
            Ok(p) => tracing::info!("palm trainer: samples kept in {}", p.display()),
            Err(e) => tracing::warn!("palm trainer: samples not kept: {e}"),
        });
        let task = cx.background_spawn(async move {
            olo_palm::train(&samples, palm_size, |p| {
                *progress.lock().unwrap() = p;
                proxy.send(Msg::Redraw);
            })
        });
        cx.spawn(async move |me, cx| {
            let out = task.await;
            let _ = me.update(cx, |w, cx| {
                match out {
                    Some((model, report)) => {
                        let kept = model.clone();
                        w.prefs.update(cx, |p, cx| {
                            p.edit(cx, move |s| {
                                s.touchpad.filter.palm_model = Some(kept);
                                s.touchpad.filter.use_palm_model = true;
                            })
                        });
                        w.trainer.model = Some(model);
                        w.trainer.report = Some(report);
                        w.trainer.advance(olo_ui::proxy(cx));
                    }
                    None => w.trainer.failed = Some("Too few touches of one kind to learn from: try again with more.".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for TrainerWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tr = &self.trainer;
        let step = tr.step;
        let (title, text, need, _) = step.plan();
        let live = tr.live.clone();
        let proxy = olo_ui::proxy(cx);
        let me = cx.entity();
        let palm_size = self.palm_size;
        let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING * 2.0));
        col = col.child(StringView::new(title).bold(true));
        if !text.is_empty() {
            col = col.child(StringView::new(text).dim(true).wrap(true));
        }
        // (The introduction shows each exercise's film in turn, beside
        // the pad live: touches show from the start.)
        let exercise = EXERCISES.contains(&step) || matches!(step, Step::Test | Step::Intro);
        if exercise {
            let (started, live2, model) = (tr.started, live.clone(), tr.model.clone());
            col = col.child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        let half = px(f32::from(b.size.width) / 2.0 - m::SPACING / 2.0);
                        let film = Bounds { origin: b.origin, size: size(half, b.size.height) };
                        let pad = Bounds { origin: point(b.right() - half, b.top()), size: size(half, b.size.height) };
                        let t = started.elapsed().as_secs_f32();
                        let shown = if step == Step::Intro { EXERCISES[(t / 4.0) as usize % EXERCISES.len()] } else { step };
                        animation(window, film, shown, if step == Step::Intro { t % 4.0 } else { t }, &p);
                        let l = live2.lock().unwrap();
                        let base = olo_palm::baseline();
                        let mo = model.as_ref().unwrap_or(&base);
                        let contacts: Vec<(TouchContact, bool, bool)> = l
                            .contacts
                            .iter()
                            .map(|c| {
                                let big = c.major.max(c.minor) >= palm_size as i32;
                                let mut t = Tracker::default();
                                let f = l.axes.as_ref().map(|a| t.frame(a, 0, &[*c])[0].1);
                                (*c, f.is_some_and(|f| olo_palm::score(mo, &f) > mo.threshold) || big, big)
                            })
                            .collect();
                        draw_pad(window, pad, l.axes.as_ref(), &contacts, &p);
                        agent::record(
                            window,
                            cx,
                            agent::Node { role: "Image", name: Some("Touchpad".into()), value: Some(format!("{} contacts", contacts.len()).into()), bounds: pad, enabled: true, ..Default::default() },
                        );
                        window.request_animation_frame();
                    },
                )
                .w_full()
                .flex_1()
                .min_h(px(180.0)),
            );
        }
        let l = live.lock().unwrap();
        let (got, warn, pad_known) = (l.done.len(), l.warn, l.axes.is_some());
        drop(l);
        let status: Option<(f32, String, String)> = match step {
            s if EXERCISES.contains(&s) => {
                let note = if !pad_known {
                    "Waiting for the touchpad…".to_owned()
                } else if let Some(w) = warn {
                    w.to_owned()
                } else if got == 0 {
                    "Go ahead.".to_owned()
                } else if got < need {
                    format!("Good. {} more.", need - got)
                } else {
                    "That's enough for this one.".to_owned()
                };
                Some(((got as f32 / need as f32).min(1.0), note, format!("{got} / {need}")))
            }
            Step::Training => Some((
                *tr.progress.lock().unwrap(),
                tr.failed.clone().unwrap_or_else(|| "Refining Olo's model on your hands…".to_owned()),
                String::new(),
            )),
            _ => None,
        };
        if let Some((fraction, note, trailing)) = status {
            col = col.child(StatusBar::new(fraction).label(note).trailing(trailing));
        }
        if step == Step::Test
            && let Some(r) = &tr.report
        {
            col = col.child(
                StringView::new(format!(
                    "On {} touches kept back: your model caught {:.0}% of palms and lost {:.0}% of fingers; the size rule caught {:.0}% and lost {:.0}%.",
                    r.contacts,
                    r.palms_caught * 100.0,
                    r.fingers_lost * 100.0,
                    r.size_rule_palms_caught * 100.0,
                    r.size_rule_fingers_lost * 100.0
                ))
                .dim(true)
                .wrap(true),
            );
        }
        // The bottom row: Cancel at the left, the step's buttons at the right.
        let mut row = div().flex().flex_row().gap(px(m::SPACING)).mt_auto();
        row = row.child(Button::new(if step == Step::Test { "Close" } else { "Cancel" }).id("palm-train-cancel").on_click(|window, _| window.remove_window()));
        row = row.child(div().flex_1());
        match step {
            Step::Intro => {
                let (m, go) = (me.clone(), proxy.clone());
                row = row.child(Button::new("Start").id("palm-train-start").default(true).on_click(move |_, cx| {
                    m.update(cx, |w, cx| {
                        w.trainer.advance(go.clone());
                        cx.notify()
                    })
                }));
            }
            s if EXERCISES.contains(&s) => {
                let (m, again) = (me.clone(), proxy.clone());
                row = row.child(Button::new("Again").id("palm-train-again").on_click(move |_, cx| {
                    m.update(cx, |w, cx| {
                        w.trainer.redo(again.clone());
                        cx.notify()
                    })
                }));
                let (m, go) = (me.clone(), proxy.clone());
                let last = s == *EXERCISES.last().unwrap();
                row = row.child(
                    Button::new(if last { "Train" } else { "Next" }).id("palm-train-next").default(true).enabled(got >= need).on_click(move |_, cx| {
                        m.update(cx, |w, cx| {
                            w.trainer.advance(go.clone());
                            if w.trainer.step == Step::Training {
                                w.train(cx);
                            }
                            cx.notify();
                        })
                    }),
                );
            }
            Step::Training => {}
            _ => {
                let (m, go) = (me.clone(), proxy.clone());
                row = row.child(Button::new("Train Again").id("palm-train-restart").on_click(move |_, cx| {
                    m.update(cx, |w, cx| {
                        w.trainer.advance(go.clone());
                        cx.notify()
                    })
                }));
            }
        }
        col.child(row)
    }
}
