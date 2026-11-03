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
    gpui::{Bounds, Hsla, PathBuilder, Pixels, SharedString, Styled as _, canvas, point, px, size},
    metrics as m, style, theme,
    widgets::{Button, StatusBar},
};

use crate::{Msg, form::Ui};

/// The steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Intro,
    RestPalms,
    TypeWithPalms,
    PointAndScroll,
    Taps,
    PointWithPalm,
    Training,
    Test,
}

const EXERCISES: [Step; 5] = [Step::RestPalms, Step::TypeWithPalms, Step::PointAndScroll, Step::Taps, Step::PointWithPalm];

impl Step {
    /// What to do, how many contacts it needs, and how its contacts are
    /// labelled (Some(true): all palms; Some(false): all fingers; None:
    /// the still first one a palm).
    fn plan(self) -> (&'static str, &'static str, usize, Option<bool>) {
        match self {
            Step::Intro => ("Personalize Olo's palm model", "Olo's palm model already tells palms from fingers out of the box. This refines it to your own hands: five short exercises on the touchpad, then a try. Rest your hands as you really do.", 0, None),
            Step::RestPalms => ("Rest your palms", "Rest the heels of both hands on the pad as they fall when you type, lift them, and rest them again. Don't use your fingers.", 12, Some(true)),
            Step::TypeWithPalms => ("Type with your palms resting", "Type a few lines on the keyboard with your palms resting on the pad as usual, lifting and resting them as you go.", 16, Some(true)),
            Step::PointAndScroll => ("Point and scroll", "With your palms off the pad, move the pointer around, then scroll up and down with two fingers.", 20, Some(false)),
            Step::Taps => ("Tap and click", "Tap with one finger, then two, and click, here and there on the pad. Palms off.", 16, Some(false)),
            Step::PointWithPalm => ("Point with a palm resting", "Rest one palm at the pad's edge and keep it still; with the other hand, point, scroll and tap.", 20, None),
            Step::Training => ("Refining Olo's model", "", 0, None),
            Step::Test => ("Try it", "Rest a palm, point and tap. Each touch is drawn as the model calls it: blue a finger, orange a palm. The ring shows the size rule's call.", 0, None),
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
}

pub struct Trainer {
    pub step: Step,
    samples: Vec<Labelled>,
    live: Arc<Mutex<Live>>,
    stop: Arc<AtomicBool>,
    /// The step the worker labels for (its index in EXERCISES, or usize::MAX).
    labelling: Arc<AtomicUsize>,
    started: Instant,
    pub progress: Arc<Mutex<f32>>,
    pub report: Option<Report>,
    pub model: Option<PalmModel>,
    pub failed: Option<String>,
}

impl Drop for Trainer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Trainer {
    pub fn new() -> Trainer {
        Trainer { step: Step::Intro, samples: Vec::new(), live: Default::default(), stop: Default::default(), labelling: Arc::new(AtomicUsize::new(usize::MAX)), started: Instant::now(), progress: Default::default(), report: None, model: None, failed: None }
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
            self.stop.store(true, Ordering::Relaxed);
            self.stop = Default::default();
            self.samples.clear();
            self.model = None;
            self.report = None;
            self.failed = None;
            return;
        }
        if step == EXERCISES[0] {
            self.watch(proxy);
        }
    }

    /// The samples so far, for training.
    pub fn take_samples(&mut self) -> Vec<Labelled> {
        self.samples.clone()
    }

    /// Follow the pad until stopped.
    fn watch(&mut self, proxy: olo_ui::Proxy) {
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
                let Some(events) = Client::connect().ok().and_then(|c| c.into_events_of(&[EventKind::TouchContacts]).ok()) else {
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                };
                for e in events {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(Event::TouchContacts { pad, time_us, contacts, since_key_us }) = e else { break };
                    tracker.since_key_us = since_key_us;
                    let step = labelling.load(Ordering::Relaxed);
                    let label = EXERCISES.get(step).map(|s| s.plan().3);
                    let frames = tracker.frame(&pad, time_us, &contacts);
                    let mut l = live.lock().unwrap();
                    l.axes = Some(pad.clone());
                    for (c, (_, f, age)) in contacts.iter().zip(&frames) {
                        let o = open.entry(c.id).or_insert((Vec::new(), 0, time_us, false, (c.x, c.y)));
                        if *age <= olo_palm::DECIDE_MS {
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
    style::sunken(window, r, p, false);
    let inner = style::inset(window, r, 2.0);
    style::fill_rect(window, inner, style::shade(p, p.panel_background, 1.08));
    let Some(a) = axes else { return };
    let (w, h) = ((a.x_max - a.x_min).max(1) as f32, (a.y_max - a.y_min).max(1) as f32);
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
    style::fill_rect(window, r, style::shade(p, p.panel_background, 0.96));
    let pad = Bounds::from_corners(r.origin + point(r.size.width * 0.3, r.size.height * 0.25), r.bottom_right() - point(r.size.width * 0.3, r.size.height * 0.15));
    style::fill_rect(window, pad, style::shade(p, p.panel_background, 1.1));
    let hand = style::c(p.panel_text).opacity(0.35);
    let finger = style::c(p.navigation_base);
    let palm = style::c(p.warning);
    let pw = f32::from(pad.size.width);
    let ph = f32::from(pad.size.height);
    // A palm heel at each bottom corner, resting (down) or lifted (up).
    let heel = |window: &mut olo_ui::gpui::Window, left: bool, down: f32| {
        let x = if left { pad.left() + px(pw * 0.15) } else { pad.right() - px(pw * 0.15) };
        let y = pad.bottom() - px(ph * 0.1) - px((1.0 - down) * 10.0);
        ellipse(window, point(x, y), pw * 0.13, ph * 0.12, if down > 0.5 { palm } else { hand });
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
            // Fingers tapping keys above the pad.
            for k in 0..6 {
                let x = pad.left() + px(pw * (0.15 + 0.14 * k as f32));
                let y = pad.top() - px(14.0 + 6.0 * ((t * 6.0 + k as f32).sin().max(0.0)));
                ellipse(window, point(x, y), 5.0, 6.0, finger);
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
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), if down { 7.0 } else { 4.0 }, if down { 8.0 } else { 4.0 }, if down { finger } else { hand });
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

/// The box in the Touch tab.
pub fn ui(ui: &mut Ui, tr: &mut Trainer, palm_size: u32) {
    let step = tr.step;
    let (title, text, need, _) = step.plan();
    let live = tr.live.clone();
    let proxy = olo_ui::proxy(ui.cx);
    ui.bbox("Train for Your Palm", |ui| {
        ui.label(title);
        if !text.is_empty() {
            ui.note(text);
        }
        let exercise = EXERCISES.contains(&step) || step == Step::Test;
        if exercise {
            let (started, live2) = (tr.started, live.clone());
            let model = tr.model.clone();
            ui.add(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        let half = px(f32::from(b.size.width) / 2.0 - m::SPACING / 2.0);
                        let film = Bounds { origin: b.origin, size: size(half, b.size.height) };
                        let pad = Bounds { origin: point(b.right() - half, b.top()), size: size(half, b.size.height) };
                        animation(window, film, step, started.elapsed().as_secs_f32(), &p);
                        let l = live2.lock().unwrap();
                        let base = olo_palm::baseline();
                        let m = model.as_ref().unwrap_or(&base);
                        let contacts: Vec<(TouchContact, bool, bool)> = l
                            .contacts
                            .iter()
                            .map(|c| {
                                let big = c.major.max(c.minor) >= palm_size as i32;
                                // (Live: the call by size now, and the model's on
                                // its size and pressure alone, a rough one.)
                                let mut t = Tracker::default();
                                let f = l.axes.as_ref().map(|a| t.frame(a, 0, &[*c])[0].1);
                                let says = f.is_some_and(|f| olo_palm::score(m, &f) > m.threshold) || big;
                                (*c, says, big)
                            })
                            .collect();
                        draw_pad(window, pad, l.axes.as_ref(), &contacts, &p);
                        agent::record(window, cx, agent::Node { role: "Image", name: Some("Touchpad".into()), value: Some(format!("{} contacts", contacts.len()).into()), bounds: pad, enabled: true, ..Default::default() });
                        window.request_animation_frame();
                    },
                )
                .w_full()
                .h(px(120.0)),
            );
        }
        let l = live.lock().unwrap();
        let (got, warn, pad_known) = (l.done.len(), l.warn, l.axes.is_some());
        drop(l);
        match step {
            Step::Intro => {
                let go = proxy.clone();
                ui.add(Button::new("Start").id("palm-train-start").on_click(ui.on_click_view(move |v, _, cx| {
                    if let Some(t) = crate::touchpad::trainer_of(v) {
                        t.advance(go.clone());
                    }
                    cx.notify();
                })));
            }
            s if EXERCISES.contains(&s) => {
                let fraction = (got as f32 / need as f32).min(1.0);
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
                ui.add(StatusBar::new(fraction).label(note).trailing(format!("{got} / {need}")));
                let (go, again) = (proxy.clone(), proxy.clone());
                ui.row(|ui| {
                    ui.add(Button::new("Again").id("palm-train-again").on_click(ui.on_click_view(move |v, _, cx| {
                        if let Some(t) = crate::touchpad::trainer_of(v) {
                            t.redo(again.clone());
                        }
                        cx.notify();
                    })));
                    ui.add(Button::new(if s == *EXERCISES.last().unwrap() { "Train" } else { "Next" }).id("palm-train-next").default(true).enabled(got >= need).on_click(ui.on_click_view(move |v, _, cx| {
                        if let Some(t) = crate::touchpad::trainer_of(v) {
                            t.advance(go.clone());
                            if t.step == Step::Training {
                                crate::touchpad::train(v, cx);
                            }
                        }
                        cx.notify();
                    })));
                });
            }
            Step::Training => {
                ui.add(StatusBar::new(*tr.progress.lock().unwrap()).label("Refining Olo's model on your hands…"));
                if let Some(e) = &tr.failed {
                    ui.note(e.clone());
                }
            }
            _ => {
                if let Some(r) = &tr.report {
                    ui.note(format!(
                        "On {} touches kept back: your model caught {:.0}% of palms and lost {:.0}% of fingers; the size rule caught {:.0}% and lost {:.0}%.",
                        r.contacts,
                        r.palms_caught * 100.0,
                        r.fingers_lost * 100.0,
                        r.size_rule_palms_caught * 100.0,
                        r.size_rule_fingers_lost * 100.0
                    ));
                }
                let go = proxy.clone();
                ui.add(Button::new("Done").id("palm-train-done").default(true).on_click(ui.on_click_view(move |v, _, cx| {
                    if let Some(t) = crate::touchpad::trainer_of(v) {
                        t.advance(go.clone());
                    }
                    cx.notify();
                })));
            }
        }
        let _ = SharedString::from("");
    });
}
