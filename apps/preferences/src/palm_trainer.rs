//! Train for Your Palm (Preferences ▸ Touchpad & Mouse ▸ Touch): a guided
//! run, full screen and driven from the keyboard, since the hands are busy
//! on the touchpad and nothing should need the pointer (Space goes on,
//! Backspace starts a step over, Esc quits). Six exercises, each with a
//! little film of what to do beside the pad live; every touch is judged as
//! it lifts ("A palm, resting. Good." / "That was a fingertip: …"), counted
//! when it's what the step asked for and dropped when it isn't, so no
//! touch is learned under the wrong name. Then Olo's palm model
//! (`olo_palm::baseline`) is refined on the touches that counted, off the
//! UI thread with progress, and kept (in the settings, and on) only if it
//! does at least as well as Olo's own on touches kept back from training;
//! a summary says which, with the numbers, and a try shows each touch as
//! the model in use calls it.
//!
//! Touches come from app_server (`EventKind::TouchContacts`) on a worker
//! that follows each contact (`olo_palm::Tracker`) from landing to lifting
//! ([`Touch`]); the window judges each one ([`judge`]) for the step it's
//! on.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_palm::{FEATURES, Labelled, PadAxes, PalmModel, Report, TouchContact, Tracker};
use olo_protocol::{Client, Event, EventKind, Reply, Request};
use olo_sync::Mutex;
use olo_ui::{
    agent,
    gpui::{
        AnyElement, App, AppContext as _, Bounds, Context, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, PathBuilder, Pixels, Render,
        Styled as _, Window, canvas, div, point, px, size,
    },
    metrics as m, style, theme,
    widgets::{KeyHint, StatusBar, StringView},
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
    Summary,
    Test,
}

const EXERCISES: [Step; 6] = [Step::RestPalms, Step::EdgePalm, Step::TypeWithPalms, Step::PointAndScroll, Step::Taps, Step::PointWithPalm];

impl Step {
    /// Its title, and what to do.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Step::Intro => (
                "Train for Your Palm",
                "Olo tells a palm from a finger by its size. Training makes a palm model of your own hands, which also catches what size misses: a palm at the edge, palms resting as you type. Six short exercises on the touchpad, about three minutes, then a try. Rest your hands as you really do; nothing needs a click: the keyboard moves you along.",
            ),
            Step::RestPalms => ("Rest your palms", "Rest the heels of both hands on the touchpad, as they fall when you type. Hold them still for a moment, then lift them. Do it five times."),
            Step::EdgePalm => {
                ("One palm over the edge", "With one hand only, rest its heel half over the touchpad's left edge for a moment, then lift it: five times. Then five times over the right edge.")
            }
            Step::TypeWithPalms => {
                ("Type with your palms down", "Type the line below with your palms resting on the touchpad, as you do on a laptop. Lift your hands now and then, and rest them again.")
            }
            Step::PointAndScroll => ("Point and scroll", "Palms off the touchpad: move the pointer around with one finger, and scroll up and down with two."),
            Step::Taps => ("Tap", "Palms off the touchpad: tap quickly with one finger, here and there. Two-finger taps count too."),
            Step::PointWithPalm => (
                "Point with a palm down",
                "Rest one palm still at or over the touchpad's edge and keep it there while a finger points, taps and scrolls. Lift the palm and rest it again, three times in all.",
            ),
            Step::Training => ("Learning your hands", "Olo's palm model is being refined on the touches that counted."),
            Step::Summary => ("", ""),
            Step::Test => ("Try it", "Use the touchpad as you usually do, palms and all: each touch shows as your new model sees it."),
        }
    }

    /// What it counts, and how many of each it wants.
    fn goals(self) -> &'static [(&'static str, usize)] {
        match self {
            Step::RestPalms => &[("Palms", 10)],
            Step::EdgePalm => &[("Left edge", 5), ("Right edge", 5)],
            Step::TypeWithPalms => &[("Palms while typing", 8)],
            Step::PointAndScroll => &[("Fingers moving", 12)],
            Step::Taps => &[("Taps", 12)],
            Step::PointWithPalm => &[("Palm rests", 3), ("Fingers", 10)],
            _ => &[],
        }
    }

    fn exercise(self) -> Option<usize> {
        EXERCISES.iter().position(|e| *e == self)
    }
}

/// A contact from landing to lifting, as the step judges it.
#[derive(Clone, Debug, Default)]
pub struct Touch {
    /// Its features in its first `DECIDE_MS` (from `MIN_AGE_MS`).
    pub frames: Vec<[f32; FEATURES]>,
    /// Its largest size (the pad's units).
    pub size: i32,
    pub held_ms: u32,
    /// How far it went from where it landed, in pad widths.
    pub travel: f32,
    /// Where it landed, across and down the pad (0 to 1).
    pub x: f32,
    pub y: f32,
    /// A key was pressed while it was down.
    pub typing: bool,
    /// When it landed, another contact had rested still for a second.
    pub with_palm: bool,
}

/// Still: moved less than this (pad widths).
const STILL: f32 = 0.03;
/// Over an edge: landed within this of the pad's left or right side.
const EDGE: f32 = 0.15;
/// Resting: down at least this long (ms).
const REST_MS: u32 = 400;
/// A tap: up within this (ms).
const TAP_MS: u32 = 350;
/// A palm resting through others' touches (the mixed step): down and
/// still this long (ms).
const PALM_REST_MS: u32 = 1000;

impl Touch {
    fn still(&self) -> bool {
        self.travel < STILL
    }

    /// As big as a palm, or one by Olo's own model.
    fn palm_like(&self, palm_size: u32, base: &PalmModel) -> bool {
        self.size as f32 >= palm_size as f32 * 0.7 || self.frames.iter().any(|f| olo_palm::score(base, f) > base.threshold)
    }
}

/// What a step makes of a touch.
#[derive(Clone, Debug, PartialEq)]
pub enum Judged {
    /// It counts toward goal `tally`, as a palm or a finger.
    Counts { palm: bool, tally: usize, say: &'static str },
    /// It isn't what the step asked for: why (it's dropped).
    Misses(&'static str),
}

const TOO_QUICK: &str = "Too quick: let it rest a moment before you lift it.";
const PALMS_OFF: &str = "That looked like a palm: keep your palms off the touchpad here.";

/// How `step` takes touch `t` (`base`: Olo's own model, for a second
/// opinion on what's a palm).
pub fn judge(step: Step, t: &Touch, palm_size: u32, base: &PalmModel) -> Judged {
    let palm_like = t.palm_like(palm_size, base);
    let counts = |palm, tally, say| Judged::Counts { palm, tally, say };
    match step {
        Step::RestPalms if !palm_like => Judged::Misses("That was a fingertip: rest the heel of your hand, below the thumb."),
        Step::RestPalms if t.held_ms < REST_MS => Judged::Misses(TOO_QUICK),
        Step::RestPalms => counts(true, 0, "A palm, resting. Good."),
        Step::EdgePalm if t.travel >= STILL * 2.0 => Judged::Misses("It moved like a finger: keep the heel of your hand still."),
        Step::EdgePalm if t.held_ms < REST_MS => Judged::Misses(TOO_QUICK),
        Step::EdgePalm if t.x < EDGE => counts(true, 0, "Over the left edge. Good."),
        Step::EdgePalm if t.x > 1.0 - EDGE => counts(true, 1, "Over the right edge. Good."),
        Step::EdgePalm => Judged::Misses("That was in the middle: rest the heel half over the left or right edge."),
        Step::TypeWithPalms if !t.typing => Judged::Misses("No typing while it rested: keep typing with your palms down."),
        Step::TypeWithPalms if t.held_ms < REST_MS => Judged::Misses("Too quick: rest your palms while you type."),
        Step::TypeWithPalms => counts(true, 0, "Resting while you typed. Good."),
        Step::PointAndScroll if palm_like => Judged::Misses(PALMS_OFF),
        Step::PointAndScroll if t.still() => Judged::Misses("That was a tap: move the pointer around, or scroll with two fingers."),
        Step::PointAndScroll => counts(false, 0, "A finger moving. Good."),
        Step::Taps if palm_like => Judged::Misses(PALMS_OFF),
        Step::Taps if t.held_ms > TAP_MS || !t.still() => Judged::Misses("That was a slide: tap quickly, without moving."),
        Step::Taps => counts(false, 0, "A tap. Good."),
        Step::PointWithPalm if t.still() && t.held_ms >= PALM_REST_MS && (palm_like || t.x < EDGE || t.x > 1.0 - EDGE) => counts(true, 0, "Your palm, resting. Good."),
        Step::PointWithPalm if t.still() && t.held_ms >= PALM_REST_MS => Judged::Misses("Rest the heel of your hand, not a fingertip, at or over the edge."),
        Step::PointWithPalm if palm_like => Judged::Misses("That looked like a palm: point with one finger."),
        Step::PointWithPalm if t.with_palm => counts(false, 1, "A finger, with your palm down. Good."),
        Step::PointWithPalm => Judged::Misses("Rest a palm on the touchpad first, then point with a finger."),
        _ => Judged::Misses(""),
    }
}

/// What the worker shares with the window.
#[derive(Default)]
struct Live {
    axes: Option<PadAxes>,
    contacts: Vec<TouchContact>,
    /// What the model in use makes of each contact on the pad now (by
    /// its id), once it's old enough to say.
    calls: HashMap<i32, bool>,
    /// Touches lifted since the window last looked.
    lifted: Vec<Touch>,
    /// The worker's connection, shut to end it at once.
    connection: Option<std::os::unix::net::UnixStream>,
}

/// One contact the worker follows.
struct Open {
    frames: Vec<[f32; FEATURES]>,
    size: i32,
    down_us: u64,
    start: (i32, i32),
    travel: f32,
    typing: bool,
    with_palm: bool,
    call: Option<bool>,
}

/// Follow the pad until `stop` (the window closing), calling each contact
/// with `model`'s view (it may change: the try uses the model kept).
fn watch(live: Arc<Mutex<Live>>, model: Arc<Mutex<PalmModel>>, stop: Arc<AtomicBool>, proxy: olo_ui::Proxy) {
    let _ = std::thread::Builder::new().name("palm-trainer".into()).spawn(move || {
        let mut tracker = Tracker::default();
        let mut open: HashMap<i32, Open> = HashMap::new();
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
                let frames = tracker.frame(&pad, time_us, &contacts);
                let width = (pad.x_max - pad.x_min).max(1) as f32;
                let model = model.lock().unwrap().clone();
                // Resting: down a second and still (a palm in the mixed step).
                let resting = |o: &Open| time_us.saturating_sub(o.down_us) >= PALM_REST_MS as u64 * 1000 && o.travel < STILL;
                let palm_down = open.values().any(resting);
                for (c, (_, f, age)) in contacts.iter().zip(&frames) {
                    let o = open.entry(c.id).or_insert_with(|| Open { frames: Vec::new(), size: 0, down_us: time_us, start: (c.x, c.y), travel: 0.0, typing: false, with_palm: palm_down, call: None });
                    if (olo_palm::MIN_AGE_MS..=olo_palm::DECIDE_MS).contains(age) {
                        o.frames.push(*f);
                        // (As the filter does: a palm once a report in its
                        // first DECIDE_MS says so; after, it stays.)
                        if olo_palm::score(&model, f) > model.threshold {
                            o.call = Some(true);
                        }
                    }
                    if *age > olo_palm::DECIDE_MS && o.call.is_none() {
                        o.call = Some(false);
                    }
                    o.size = o.size.max(c.major.max(c.minor));
                    let (dx, dy) = ((c.x - o.start.0) as f32 / width, (c.y - o.start.1) as f32 / width);
                    o.travel = o.travel.max((dx * dx + dy * dy).sqrt());
                    o.typing |= since_key_us < 1_500_000;
                }
                let mut l = live.lock().unwrap();
                l.axes = Some(pad.clone());
                l.calls = open.iter().filter_map(|(id, o)| Some((*id, o.call?))).collect();
                let lifted: Vec<i32> = open.keys().filter(|id| !contacts.iter().any(|c| c.id == **id)).copied().collect();
                for id in lifted {
                    let o = open.remove(&id).unwrap();
                    if o.frames.is_empty() {
                        continue;
                    }
                    let h = (pad.y_max - pad.y_min).max(1) as f32;
                    l.lifted.push(Touch {
                        frames: o.frames,
                        size: o.size,
                        held_ms: (time_us.saturating_sub(o.down_us) / 1000) as u32,
                        travel: o.travel,
                        x: (o.start.0 - pad.x_min) as f32 / width,
                        y: (o.start.1 - pad.y_min) as f32 / h,
                        typing: o.typing,
                        with_palm: o.with_palm,
                    });
                }
                l.contacts = contacts;
                drop(l);
                proxy.send(Msg::Redraw);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// How a training run came out.
enum Outcome {
    /// Better than (or as good as) Olo's own: kept, and in use.
    Kept(Report),
    /// Worse than Olo's own on the touches kept back: not kept.
    Worse(Report),
    /// Too few palms or fingers to learn from.
    TooFew,
}

/// What a step's touch did, for the pad's marks and the line under it.
struct Said {
    good: bool,
    text: &'static str,
    at: Instant,
    /// Where on the pad (0 to 1), for a mark there.
    place: (f32, f32),
}

/// The trainer's window: full screen, its own (closing it ends the run).
pub struct TrainerWindow {
    prefs: Entity<Prefs>,
    palm_size: u32,
    step: Step,
    started: Instant,
    /// The touches that counted, from the steps finished, and this one's.
    samples: Vec<Labelled>,
    current: Vec<Labelled>,
    /// This step's count toward each of its goals.
    tally: Vec<usize>,
    said: Option<Said>,
    /// Where touches were judged lately (a ring each, fading).
    marks: Vec<Said>,
    /// Esc pressed once with touches gathered: again quits.
    quitting: bool,
    /// The typing step's lesson.
    tutor: crate::typing_tutor::Tutor,
    live: Arc<Mutex<Live>>,
    /// The model calls are made with: the one kept (the try's).
    model: Arc<Mutex<PalmModel>>,
    stop: Arc<AtomicBool>,
    progress: Arc<Mutex<f32>>,
    outcome: Option<Outcome>,
    /// Whether app_server has the model kept, as it says when asked
    /// (None: not heard yet). Preferences' own copy changes at once, so
    /// only app_server's answer says it was saved (one older than this
    /// Preferences drops what it doesn't know).
    saved: Arc<Mutex<Option<bool>>>,
    /// The touches judged in the try: fingers and palms.
    tried: (usize, usize),
    focus: FocusHandle,
    focused: bool,
}

impl Drop for TrainerWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // (Its worker waits on app_server: shut, it ends at once, and
        // app_server stops reading the pad for it.)
        if let Some(c) = self.live.lock().unwrap().connection.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// The content's widest (it's centred on a wider screen).
const COLUMN: f32 = 960.0;
/// Around it, and between the film and the pad.
const MARGIN: f32 = m::SPACING * 4.0;
const PANEL_GAP: f32 = m::SPACING * 3.0;
/// A MacBook pad's shape (157 × 96 mm): the panels', so the pad fills its
/// own.
const PAD_ASPECT: f32 = 157.0 / 96.0;
/// Laid out at this until full screen (and in the layout lint).
const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 800.0;

/// Open it, full screen.
pub fn open(prefs: Entity<Prefs>, palm_size: u32, cx: &mut App) {
    let options = olo_ui::WindowOptions::new("Train for Your Palm", WIDTH, HEIGHT).fullscreen();
    let _ = olo_ui::open_window(cx, options, move |_, cx| {
        let proxy = olo_ui::proxy(cx);
        cx.new(|cx| {
            let w = TrainerWindow::new(prefs, palm_size, cx);
            watch(w.live.clone(), w.model.clone(), w.stop.clone(), proxy);
            w
        })
    });
}

impl TrainerWindow {
    fn new(prefs: Entity<Prefs>, palm_size: u32, cx: &mut Context<Self>) -> TrainerWindow {
        // The worker's news comes as the settings' does (`Msg::Redraw`).
        cx.observe(&prefs, |_, _, cx| cx.notify()).detach();
        TrainerWindow {
            prefs,
            palm_size,
            step: Step::Intro,
            started: Instant::now(),
            samples: Vec::new(),
            current: Vec::new(),
            tally: Vec::new(),
            said: None,
            marks: Vec::new(),
            quitting: false,
            tutor: Default::default(),
            live: Default::default(),
            // (Until a model is kept, nothing is called: the try is of it.)
            model: Arc::new(Mutex::new(olo_palm::baseline())),
            stop: Default::default(),
            progress: Default::default(),
            outcome: None,
            saved: Default::default(),
            tried: (0, 0),
            focus: cx.focus_handle(),
            focused: false,
        }
    }

    fn go(&mut self, step: Step) {
        self.step = step;
        self.started = Instant::now();
        self.current.clear();
        self.tally = vec![0; step.goals().len()];
        self.said = None;
        self.marks.clear();
        self.quitting = false;
        self.tutor = Default::default();
        self.tried = (0, 0);
        // (What lifted before the step began isn't its.)
        self.live.lock().unwrap().lifted.clear();
    }

    /// Every goal of the step met.
    fn done(&self) -> bool {
        self.step.goals().iter().zip(&self.tally).all(|((_, want), got)| got >= want)
    }

    /// Judge the touches lifted since last looked.
    fn take_touches(&mut self) {
        let lifted = std::mem::take(&mut self.live.lock().unwrap().lifted);
        if lifted.is_empty() {
            return;
        }
        if self.step == Step::Test {
            let model = self.model.lock().unwrap().clone();
            for t in &lifted {
                let palm = t.frames.iter().any(|f| olo_palm::score(&model, f) > model.threshold);
                if palm { self.tried.1 += 1 } else { self.tried.0 += 1 }
            }
            return;
        }
        if self.step.exercise().is_none() {
            return;
        }
        let base = olo_palm::baseline();
        for t in lifted {
            let place = (t.x, t.y);
            match judge(self.step, &t, self.palm_size, &base) {
                Judged::Counts { palm, tally, say } => {
                    if let Some(n) = self.tally.get_mut(tally) {
                        *n += 1;
                    }
                    self.current.push(Labelled { frames: t.frames, palm, size: t.size });
                    self.marks.push(Said { good: true, text: say, at: Instant::now(), place });
                    self.said = Some(Said { good: true, text: say, at: Instant::now(), place });
                }
                Judged::Misses(why) => {
                    self.marks.push(Said { good: false, text: why, at: Instant::now(), place });
                    self.said = Some(Said { good: false, text: why, at: Instant::now(), place });
                }
            }
        }
        self.marks.retain(|m| m.at.elapsed().as_secs_f32() < MARK_SECONDS);
        self.quitting = false;
    }

    /// Space (or Return): on to what's next, if the step is done.
    fn go_on(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Intro => self.go(EXERCISES[0]),
            s if s.exercise().is_some() => {
                if !self.done() {
                    return;
                }
                self.samples.append(&mut self.current);
                match EXERCISES.get(s.exercise().unwrap_or(0) + 1) {
                    Some(next) => self.go(*next),
                    None => self.train(cx),
                }
            }
            Step::Summary if matches!(self.outcome, Some(Outcome::Kept(_))) => self.go(Step::Test),
            Step::Summary => window.remove_window(),
            Step::Test => window.remove_window(),
            _ => {}
        }
        cx.notify();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gathered = !self.samples.is_empty() || !self.current.is_empty();
        if self.step.exercise().is_some() && gathered && !self.quitting {
            self.quitting = true;
            cx.notify();
            return;
        }
        window.remove_window();
    }

    /// Start over: the whole run (the summary's and the try's R).
    fn again(&mut self, cx: &mut Context<Self>) {
        self.samples.clear();
        self.outcome = None;
        self.go(EXERCISES[0]);
        cx.notify();
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.platform || k.modifiers.alt {
            return;
        }
        let key = k.key.as_str();
        // The typing step's lesson takes letters and spaces, done or not
        // (a typed space never skips it: Return goes on).
        if self.step == Step::TypeWithPalms
            && key != "escape"
            && key != "enter"
            && let Some(ch) = k.key_char.as_deref().and_then(|s| s.chars().next()).filter(|c| !c.is_control())
        {
            self.tutor.key(ch);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        match key {
            "escape" => self.escape(window, cx),
            "space" | "enter" => self.go_on(window, cx),
            "backspace" if self.step.exercise().is_some() => {
                let step = self.step;
                self.go(step);
                cx.notify();
            }
            "r" if matches!(self.step, Step::Summary | Step::Test) => self.again(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Refine Olo's model on the touches that counted, off the UI thread
    /// with progress; keep it (in the settings, and on) if it does at
    /// least as well as Olo's own on the touches kept back.
    fn train(&mut self, cx: &mut Context<Self>) {
        self.go(Step::Training);
        let samples = self.samples.clone();
        let progress = self.progress.clone();
        *progress.lock().unwrap() = 0.0;
        let (palm_size, proxy) = (self.palm_size, olo_ui::proxy(cx));
        // Kept on disk too (~/.local/state/olo/palm), for Olo's own model
        // to be tuned on real hands later.
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
                w.outcome = Some(match out {
                    Some((model, report)) if report.better_than_baseline() => {
                        let kept = model.clone();
                        w.prefs.update(cx, |p, cx| {
                            p.edit(cx, move |s| {
                                s.touchpad.filter.palm_model = Some(kept);
                                s.touchpad.filter.use_palm_model = true;
                            })
                        });
                        w.confirm(model.trained, cx);
                        *w.model.lock().unwrap() = model;
                        Outcome::Kept(report)
                    }
                    Some((_, report)) => Outcome::Worse(report),
                    None => Outcome::TooFew,
                });
                w.go(Step::Summary);
                cx.notify();
            });
        })
        .detach();
    }
}

impl TrainerWindow {
    /// Ask app_server whether it has the model trained at `trained` (a
    /// few times, for a moment: the settings reach it on another thread).
    fn confirm(&mut self, trained: u64, cx: &mut Context<Self>) {
        let (saved, proxy) = (self.saved.clone(), olo_ui::proxy(cx));
        *saved.lock().unwrap() = None;
        std::thread::spawn(move || {
            let mut has = false;
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(300));
                let now = Client::connect().ok().and_then(|mut c| match c.request(&Request::GetSettings) {
                    Ok(Reply::Settings { settings }) => Some(settings.touchpad.filter.palm_model.is_some_and(|m| m.trained == trained) && settings.touchpad.filter.use_palm_model),
                    _ => None,
                });
                has = now == Some(true);
                if has {
                    break;
                }
            }
            *saved.lock().unwrap() = Some(has);
            proxy.send(Msg::Redraw);
        });
    }
}

/// The pad drawn in `r`: its outline, each contact as an ellipse of its
/// size (by `calls`: orange a palm, blue a finger; grey while it's
/// undecided or isn't being called), and `marks` fading where touches
/// counted (green) or didn't (red).
fn draw_pad(window: &mut Window, r: Bounds<Pixels>, live: &Live, calling: bool, marks: &[(f32, f32, bool, f32)], p: &olo_theme::Palette) -> Bounds<Pixels> {
    // The pad's own shape (its size in millimetres where it says, a
    // MacBook's until it has), fitted and centred.
    let aspect = match &live.axes {
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
    let Some(a) = &live.axes else { return r };
    let (w, h) = ((a.x_max - a.x_min).max(1) as f32, (a.y_max - a.y_min).max(1) as f32);
    let (iw, ih) = (f32::from(inner.size.width), f32::from(inner.size.height));
    // (Within the pad: a palm at its edge is cut by it, as on the glass.)
    window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: inner }), |window| {
        for (x, y, good, age) in marks {
            let ink = if *good { p.success } else { p.failure };
            let fade = (1.0 - age / MARK_SECONDS).clamp(0.0, 1.0);
            let c = point(inner.left() + px(iw * x), inner.top() + px(ih * y));
            ring(window, c, 14.0 + age * 10.0, 14.0 + age * 10.0, style::c(ink).opacity(fade));
        }
        for c in &live.contacts {
            let cx = inner.left() + px(iw * ((c.x - a.x_min) as f32 / w));
            let cy = inner.top() + px(ih * ((c.y - a.y_min) as f32 / h));
            // Its size in the pad's units is a diameter in the pad's
            // coordinate units (both 98/mm on a MacBook).
            let rx = (c.major.max(c.minor) as f32 / w * iw / 2.0).max(3.0);
            let ry = (c.major.min(c.minor) as f32 / h * ih / 2.0).max(3.0);
            // (Called, a contact is a finger until the model says palm, as
            // the filter has it.)
            let ink = match live.calls.get(&c.id) {
                _ if !calling => style::c(p.panel_text).opacity(0.45),
                Some(true) => style::c(p.warning),
                _ => style::c(p.navigation_base),
            };
            ellipse(window, point(cx, cy), rx, ry, ink.opacity(0.8));
        }
    });
    r
}

/// How long a counted or missed touch's mark shows on the pad.
const MARK_SECONDS: f32 = 1.5;

pub(crate) fn ellipse(window: &mut Window, c: olo_ui::gpui::Point<Pixels>, rx: f32, ry: f32, ink: Hsla) {
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

pub(crate) fn ring(window: &mut Window, c: olo_ui::gpui::Point<Pixels>, rx: f32, ry: f32, ink: Hsla) {
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

/// A round mark: a check on green (it counted, the model was kept) or a
/// cross on red, `d` points across, centred in `b`.
fn verdict_mark(window: &mut Window, b: Bounds<Pixels>, good: bool, d: f32, p: &olo_theme::Palette) {
    let c = b.center();
    ellipse(window, c, d / 2.0, d / 2.0, style::c(if good { p.success } else { p.failure }));
    let mut path = PathBuilder::stroke(px((d / 8.0).max(1.5)));
    let k = d / 2.0;
    if good {
        path.move_to(point(c.x - px(k * 0.45), c.y + px(k * 0.02)));
        path.line_to(point(c.x - px(k * 0.12), c.y + px(k * 0.36)));
        path.line_to(point(c.x + px(k * 0.48), c.y - px(k * 0.34)));
    } else {
        path.move_to(point(c.x - px(k * 0.36), c.y - px(k * 0.36)));
        path.line_to(point(c.x + px(k * 0.36), c.y + px(k * 0.36)));
        path.move_to(point(c.x + px(k * 0.36), c.y - px(k * 0.36)));
        path.line_to(point(c.x - px(k * 0.36), c.y + px(k * 0.36)));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, olo_ui::gpui::white());
    }
}

/// The step's little film: hands on a laptop's deck, in Olo's flat style,
/// moving with `t` seconds.
fn animation(window: &mut Window, r: Bounds<Pixels>, step: Step, t: f32, p: &olo_theme::Palette) {
    // A laptop's deck in a sunken well: rows of keys above, the pad
    // below between the palm rests, as the hands find them.
    let r = style::snap(window, r);
    style::sunken(window, r, p, false);
    let deck = style::inset(window, r, 2.0);
    style::fill_rect(window, deck, style::shade(p, p.panel_background, 0.94));
    let (dw, dh) = (f32::from(deck.size.width), f32::from(deck.size.height));
    let keys = Bounds::from_corners(deck.origin + point(px(dw * 0.08), px(dh * 0.06)), deck.origin + point(px(dw * 0.92), px(dh * 0.40)));
    let (rows, cols) = (4, 12);
    let (kw, kh) = (f32::from(keys.size.width) / cols as f32, f32::from(keys.size.height) / rows as f32);
    for row in 0..rows {
        for col in 0..cols {
            let k = style::snap(window, Bounds::new(keys.origin + point(px(kw * col as f32 + 1.0), px(kh * row as f32 + 1.0)), size(px(kw - 2.0), px(kh - 2.0))));
            style::raised(window, k, p);
        }
    }
    // The pad: a MacBook's shape (157 × 96 mm), centred under the keys.
    let pad_w = dw * 0.56;
    let pad_h = (pad_w / 1.64).min(dh * 0.4);
    let pad = style::snap(window, Bounds::new(deck.origin + point(px((dw - pad_w) / 2.0), px(dh * 0.5)), size(px(pad_w), px(pad_h))));
    style::sunken(window, pad, p, false);
    let pad = style::inset(window, pad, 2.0);
    style::fill_rect(window, pad, style::shade(p, p.panel_background, 1.08));
    let hand = style::c(p.panel_text).opacity(0.35);
    let finger = style::c(p.navigation_base);
    let palm = style::c(p.warning);
    let (pw, ph) = (f32::from(pad.size.width), f32::from(pad.size.height));
    let dot = (pw * 0.035).max(4.0);
    // A palm's heel at each of the pad's upper outer edges, half on the
    // deck (where a typing hand rests on a MacBook), down or lifted.
    let heel = |window: &mut Window, left: bool, down: bool| {
        let x = if left { pad.left() + px(pw * 0.04) } else { pad.right() - px(pw * 0.04) };
        let y = pad.top() + px(ph * 0.32) - px(if down { 0.0 } else { ph * 0.08 });
        ellipse(window, point(x, y), pw * 0.16, ph * 0.2, if down { palm.opacity(0.85) } else { hand });
    };
    match step {
        Step::RestPalms => {
            let down = (t % 4.0) < 2.5;
            heel(window, true, down);
            heel(window, false, down);
        }
        Step::TypeWithPalms => {
            heel(window, true, true);
            heel(window, false, true);
            // Fingers on the home row, each pressing a key in turn.
            let home = keys.top() + px(kh * 2.5);
            for k in 0..8 {
                let col = if k < 4 { 1 + k } else { 3 + k };
                let x = keys.left() + px(kw * (col as f32 + 0.5));
                let pressing = ((t * 5.0) as usize) % 8 == k;
                let d = if pressing { dot * 0.8 } else { dot };
                ellipse(window, point(x, home), d, d, if pressing { finger } else { finger.opacity(0.6) });
            }
        }
        Step::PointAndScroll => {
            if t % 6.0 < 3.0 {
                let (x, y) = (0.5 + 0.3 * (t * 1.5).cos(), 0.5 + 0.25 * (t * 1.1).sin());
                ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
            } else {
                let y = 0.5 + 0.3 * (t * 2.0).sin();
                ellipse(window, point(pad.left() + px(pw * 0.45), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
                ellipse(window, point(pad.left() + px(pw * 0.55), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
            }
        }
        Step::Taps => {
            let k = (t * 1.5) as usize % 4;
            let (x, y) = [(0.3, 0.4), (0.7, 0.6), (0.5, 0.3), (0.4, 0.7)][k];
            let down = ((t * 1.5) % 1.0) < 0.3;
            let d = if down { dot * 1.15 } else { dot * 0.6 };
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), d, d * 1.1, if down { finger } else { hand });
        }
        Step::EdgePalm => {
            // One heel, half over the left edge for a while, then the
            // right, resting and lifting.
            let left = (t % 7.0) < 3.5;
            let down = (t % 3.5) < 2.4;
            let x = if left { pad.left() - px(pw * 0.03) } else { pad.right() + px(pw * 0.03) };
            let y = pad.top() + px(ph * 0.4) - px(if down { 0.0 } else { ph * 0.08 });
            ellipse(window, point(x, y), pw * 0.17, ph * 0.22, if down { palm.opacity(0.85) } else { hand });
        }
        Step::PointWithPalm => {
            heel(window, false, true);
            let (x, y) = (0.35 + 0.2 * (t * 1.5).cos(), 0.45 + 0.25 * (t * 1.1).sin());
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
        }
        _ => {}
    }
}

/// A caption over a panel ("Like this", "Your touchpad").
fn caption(text: &'static str) -> impl IntoElement {
    StringView::new(text).bold(true).height(m::LABEL_ROW)
}

impl TrainerWindow {
    /// The top: where in the run this is, as a bar of the six exercises
    /// and the try, and its title and what to do.
    fn top(&self, title: String, lead: &'static str) -> AnyElement {
        let at = match self.step {
            Step::Intro => 0,
            s => s.exercise().map_or(EXERCISES.len() + 1, |i| i + 1),
        };
        let where_ = match self.step.exercise() {
            Some(i) => format!("Step {} of {}", i + 1, EXERCISES.len()),
            None if self.step == Step::Intro => "Six exercises, then a try".to_owned(),
            None => "Done with the exercises".to_owned(),
        };
        let track = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let n = EXERCISES.len() + 1;
                let gap = m::TIGHT;
                let w = (f32::from(b.size.width) - gap * (n - 1) as f32) / n as f32;
                for i in 0..n {
                    let r = style::snap(window, Bounds::new(point(b.left() + px((w + gap) * i as f32), b.top()), size(px(w), b.size.height)));
                    let ink = if i < at { style::c(p.navigation_base) } else { style::shade(&p, p.panel_background, 0.8) };
                    style::fill_rect(window, r, ink);
                }
            },
        )
        .w_full()
        .h(px(m::TIGHT));
        let mut col = div().flex().flex_col().gap(px(m::SPACING)).w_full();
        col = col.child(track).child(StringView::new(where_).dim(true).height(m::LABEL_ROW));
        if !title.is_empty() {
            col = col.child(StringView::new(title).bold(true).size(style::TITLE_SIZE));
        }
        if !lead.is_empty() {
            col = col.child(StringView::new(lead).size(style::LEAD_SIZE).wrap(true));
        }
        col.into_any_element()
    }

    /// The film and the pad live, side by side, each under its caption,
    /// each `w` × `h`.
    fn panels(&self, film: Option<Step>, calling: bool, (w, h): (f32, f32)) -> AnyElement {
        let (started, live) = (self.started, self.live.clone());
        let marks: Vec<(f32, f32, bool, f32)> =
            self.marks.iter().filter(|s| s.at.elapsed().as_secs_f32() < MARK_SECONDS).map(|s| (s.place.0, s.place.1, s.good, s.at.elapsed().as_secs_f32())).collect();
        let fading = !marks.is_empty();
        let pad = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let l = live.lock().unwrap();
                let r = draw_pad(window, b, &l, calling, &marks, &p);
                if l.axes.is_none() {
                    style::label(
                        window,
                        cx,
                        r.center().x,
                        r.center().y,
                        style::Align::Center,
                        "Waiting for the touchpad…",
                        style::PLAIN_SIZE,
                        style::ui_font(FontWeight::NORMAL),
                        style::disabled_text(&p),
                    );
                }
                let n = l.contacts.len();
                agent::record(
                    window,
                    cx,
                    agent::Node { role: "Image", name: Some("Your touchpad".into()), value: Some(format!("{n} touches").into()), bounds: r, enabled: true, ..Default::default() },
                );
                if fading {
                    window.request_animation_frame();
                }
            },
        )
        .w(px(w))
        .h(px(h));
        let pad = div().flex().flex_col().gap(px(m::TIGHT)).child(caption("Your touchpad")).child(pad);
        let mut row = div().flex().flex_row().gap(px(PANEL_GAP)).w_full();
        if let Some(step) = film {
            let next = self.tutor.next();
            let film = canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    let t = started.elapsed().as_secs_f32();
                    // (The introduction shows each exercise in turn.)
                    let (shown, t) = if step == Step::Intro { (EXERCISES[(t / 4.0) as usize % EXERCISES.len()], t % 4.0) } else { (step, t) };
                    if shown == Step::TypeWithPalms && step != Step::Intro {
                        // (The lesson's keyboard: still, but for the key.)
                        crate::typing_tutor::keyboard(window, cx, b, next, &p);
                    } else {
                        animation(window, b, shown, t, &p);
                        window.request_animation_frame();
                    }
                    agent::record(window, cx, agent::Node { role: "Image", name: Some("Like this".into()), value: Some(shown.words().0.into()), bounds: b, enabled: true, ..Default::default() });
                },
            )
            .w(px(w))
            .h(px(h));
            row = row.child(div().flex().flex_col().gap(px(m::TIGHT)).child(caption("Like this")).child(film));
        }
        row.child(pad).into_any_element()
    }

    /// Under the panels: what the last touch did (or the step's news),
    /// and the step's counts.
    fn feedback(&self) -> AnyElement {
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).w_full().h(px(m::CONTROL_HEIGHT * 1.5));
        let done = self.done();
        let (good, text): (Option<bool>, String) = if self.quitting {
            (None, "Press Esc again to quit: the touches so far are dropped. Space goes on.".into())
        } else if done {
            (
                Some(true),
                match self.step {
                    s if s == *EXERCISES.last().unwrap() => "That's all of them. Press Space to train.",
                    Step::TypeWithPalms => "That's enough for this step. Press Return for the next.",
                    _ => "That's enough for this step. Press Space for the next.",
                }
                .into(),
            )
        } else if let Some(s) = &self.said {
            (Some(s.good), s.text.into())
        } else if self.live.lock().unwrap().axes.is_none() {
            (None, "Waiting for the touchpad…".into())
        } else {
            (None, "Go ahead.".into())
        };
        if let Some(good) = good {
            let mark = canvas(|_, _, _| {}, move |b, _, window, cx| verdict_mark(window, b, good, 18.0, &theme::palette(cx))).w(px(m::MARK_ROW)).h(px(m::MARK_ROW)).flex_shrink_0();
            row = row.child(mark);
        }
        row = row.child(div().flex_1().child(StringView::new(text).size(style::LEAD_SIZE).bold(done)));
        for ((name, want), got) in self.step.goals().iter().zip(&self.tally) {
            row = row.child(tally(name, *got, *want));
        }
        row.into_any_element()
    }

    /// The bottom: the keys and what each does now.
    fn keys(&self) -> AnyElement {
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING * 3.0)).w_full().h(px(m::CONTROL_HEIGHT));
        let hint = |key: &'static str, action: &'static str, on: bool| KeyHint::new(key, action).enabled(on);
        match self.step {
            Step::Intro => row = row.child(hint("Space", "Start", true)).child(hint("Esc", "Cancel", true)),
            s if s.exercise().is_some() => {
                let last = s == *EXERCISES.last().unwrap();
                let next = if s == Step::TypeWithPalms { "Return" } else { "Space" };
                row = row.child(hint(next, if last { "Train" } else { "Next step" }, self.done()));
                if s != Step::TypeWithPalms {
                    row = row.child(hint("Backspace", "Start this step over", true));
                }
                row = row.child(hint("Esc", if self.quitting { "Quit, dropping the touches" } else { "Quit" }, true));
            }
            Step::Summary if matches!(self.outcome, Some(Outcome::Kept(_))) => row = row.child(hint("Space", "Try it", true)).child(hint("R", "Train again", true)).child(hint("Esc", "Done", true)),
            Step::Summary => row = row.child(hint("R", "Train again", true)).child(hint("Esc", "Done", true)),
            Step::Test => row = row.child(hint("Space", "Done", true)).child(hint("R", "Train again", true)),
            _ => row = row.child(hint("Esc", "Stop", true)),
        }
        row.into_any_element()
    }

    /// The summary: whether a model was made and kept, and how it did.
    fn summary(&self) -> AnyElement {
        let gathered = self.samples.len();
        let palms = self.samples.iter().filter(|s| s.palm).count();
        let saved = *self.saved.lock().unwrap();
        let (good, title, text) = match &self.outcome {
            Some(Outcome::Kept(_)) if saved == Some(false) => (
                false,
                "Your palm model couldn't be saved",
                format!(
                    "It learned from {gathered} of your touches, but app_server didn't keep it: it's likely older than this Preferences. Upgrade Olo (the new build in Deskbar's tray), then train again."
                ),
            ),
            Some(Outcome::Kept(_)) => (
                true,
                if saved == Some(true) { "Your palm model is saved, and in use" } else { "Your palm model is ready: saving…" },
                format!(
                    "Trained on {gathered} of your touches ({palms} palms, {} fingers). It's kept with your settings (and in backups); Preferences ▸ Touchpad & Mouse ▸ Touch shows it, and can turn it off or forget it.",
                    gathered - palms
                ),
            ),
            Some(Outcome::Worse(r)) => (
                false,
                "The model wasn't good enough to keep",
                format!(
                    "Trained on {gathered} of your touches, it did worse than Olo's untrained model on the {} kept back to test it, so it wasn't saved and nothing changed. Train again, resting your hands as you really do.",
                    r.contacts
                ),
            ),
            Some(Outcome::TooFew) | None => (
                false,
                "Not enough touches to learn from",
                "It needs at least three palms and three fingers among the touches it learns from. Train again and do each exercise as its film shows.".to_owned(),
            ),
        };
        let mark = canvas(|_, _, _| {}, move |b, _, window, cx| verdict_mark(window, b, good, 28.0, &theme::palette(cx))).w(px(32.0)).h(px(32.0)).flex_shrink_0();
        let mut col = div().flex().flex_col().gap(px(m::SPACING * 2.0)).w_full();
        col = col.child(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(mark).child(StringView::new(title).bold(true).size(style::TITLE_SIZE)));
        col = col.child(StringView::new(text).size(style::LEAD_SIZE).wrap(true));
        if let Some(Outcome::Kept(r) | Outcome::Worse(r)) = &self.outcome {
            col = col.child(results(r));
        }
        col.into_any_element()
    }
}

/// A goal's count: its name, and a dot for each touch it wants, filled
/// as they come.
fn tally(name: &'static str, got: usize, want: usize) -> AnyElement {
    const DOT: f32 = 10.0;
    let said = format!("{name}: {} of {want}", got.min(want));
    let dots = canvas(
        |_, _, _| {},
        move |b, _, window, cx| {
            let p = theme::palette(cx);
            for i in 0..want {
                let c = point(b.left() + px(i as f32 * (DOT + m::TIGHT) + DOT / 2.0), b.center().y);
                if i < got {
                    ellipse(window, c, DOT / 2.0, DOT / 2.0, style::c(p.success));
                } else {
                    ring(window, c, DOT / 2.0 - 1.0, DOT / 2.0 - 1.0, style::disabled_text(&p));
                }
            }
            agent::record(window, cx, agent::Node::label(said.clone(), b));
        },
    )
    .w(px(want as f32 * (DOT + m::TIGHT) - m::TIGHT))
    .h(px(DOT))
    .flex_shrink_0();
    div().flex().flex_row().items_center().gap(px(m::SPACING)).child(StringView::new(name).dim(got < want)).child(dots).into_any_element()
}

/// How each model did on the touches kept back: a little table.
fn results(r: &Report) -> AnyElement {
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    let row = |name: &str, caught: String, lost: String, bold: bool| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(div().w(px(220.0)).child(StringView::new(name.to_owned()).bold(bold)))
            .child(div().w(px(160.0)).child(StringView::new(caught).bold(bold)))
            .child(div().w(px(200.0)).child(StringView::new(lost).bold(bold)))
    };
    let heads = div()
        .flex()
        .flex_row()
        .items_center()
        .child(div().w(px(220.0)))
        .child(div().w(px(160.0)).child(StringView::new("Palms caught").dim(true)))
        .child(div().w(px(200.0)).child(StringView::new("Fingers taken for palms").dim(true)));
    div()
        .flex()
        .flex_col()
        .child(
            StringView::new(format!("Tested on the {} touches kept back from training ({} palms, {} fingers):", r.contacts, r.palms, r.fingers))
                .size(style::LEAD_SIZE)
                .height(style::line_row(style::LEAD_SIZE) + m::SPACING),
        )
        .child(heads)
        .child(row("The new model", pct(r.palms_caught), pct(r.fingers_lost), true))
        .child(row("Olo's own model", pct(r.baseline_palms_caught), pct(r.baseline_fingers_lost), false))
        .child(row("Size alone", pct(r.size_rule_palms_caught), pct(r.size_rule_fingers_lost), false))
        .into_any_element()
}

impl Render for TrainerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        self.take_touches();
        let step = self.step;
        let (title, lead) = step.words();
        // The column: as wide as the screen allows, up to COLUMN; the
        // panels in the pad's shape, never so tall the rest doesn't fit.
        let screen = window.viewport_size();
        let (vw, vh) = (f32::from(screen.width), f32::from(screen.height));
        let col_w = ((vw - 2.0 * MARGIN).min(COLUMN) / 2.0).floor() * 2.0;
        let half = ((col_w - PANEL_GAP) / 2.0 / 2.0).floor() * 2.0;
        // (Held to a share of the screen's height, a panel narrows with
        // it, keeping the pad's shape, so its caption stays over it.)
        let fit = |w: f32, most_h: f32| {
            let h = ((w / PAD_ASPECT).min(most_h) / 2.0).floor() * 2.0;
            (((h * PAD_ASPECT).min(w) / 2.0).floor() * 2.0, h)
        };
        let (two, one) = (fit(half, vh * 0.4), fit(col_w, vh * 0.5));
        let mut col = div().flex().flex_col().gap(px(m::SPACING * 2.0)).w(px(col_w));
        match step {
            Step::Intro => {
                col = col.child(self.top(title.into(), lead));
                col = col.child(self.panels(Some(Step::Intro), false, two));
                let list: Vec<String> = EXERCISES.iter().enumerate().map(|(i, e)| format!("{}. {}", i + 1, e.words().0)).collect();
                col = col.child(StringView::new(list.join("     ")).dim(true).wrap(true));
            }
            s if s.exercise().is_some() => {
                col = col.child(self.top(title.into(), lead));
                if s == Step::TypeWithPalms {
                    col = col.child(crate::typing_tutor::lesson(&self.tutor));
                }
                col = col.child(self.panels(Some(s), false, two));
                if s == Step::TypeWithPalms && self.tutor.wrong_now() {
                    window.request_animation_frame();
                }
                col = col.child(self.feedback());
            }
            Step::Training => {
                col = col.child(self.top(title.into(), lead));
                let fraction = *self.progress.lock().unwrap();
                col = col.child(StatusBar::new(fraction).label(format!("Refining Olo's model on {} touches…", self.samples.len())));
            }
            Step::Summary => {
                col = col.child(self.top(String::new(), ""));
                col = col.child(self.summary());
            }
            Step::Test => {
                col = col.child(self.top(title.into(), lead));
                col = col.child(self.panels(None, true, one));
                let legend = |ink: fn(&olo_theme::Palette) -> Hsla, name: String| {
                    let dot = canvas(|_, _, _| {}, move |b, _, window, cx| ellipse(window, b.center(), 6.0, 6.0, ink(&theme::palette(cx)))).w(px(m::MARK_SIZE)).h(px(m::MARK_SIZE)).flex_shrink_0();
                    div().flex().flex_row().items_center().gap(px(m::MARK_GAP)).child(dot).child(StringView::new(name))
                };
                let (fingers, palms) = self.tried;
                col = col.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING * 3.0))
                        .h(px(m::CONTROL_HEIGHT))
                        .child(legend(|p| style::c(p.navigation_base), format!("A finger ({fingers} so far)")))
                        .child(legend(|p| style::c(p.warning), format!("A palm, ignored ({palms} so far)"))),
                );
            }
            _ => {}
        }
        col = col.child(self.keys());
        div().size_full().track_focus(&self.focus).on_key_down(cx.listener(Self::key_down)).flex().flex_col().items_center().justify_center().p(px(MARGIN)).child(col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(size: i32, held_ms: u32, travel: f32, x: f32) -> Touch {
        Touch { size, held_ms, travel, x, y: 0.4, ..Default::default() }
    }

    /// Each step counts what it asks for, and says why it doesn't count
    /// the rest.
    #[test]
    fn steps_judge_touches() {
        // (A model that calls nothing a palm: only sizes and places count.)
        let none = PalmModel { threshold: f32::MAX, ..olo_palm::baseline() };
        let j = |step, t: &Touch| judge(step, t, 1600, &none);
        let palm = touch(1800, 900, 0.0, 0.5);
        let tip = touch(600, 900, 0.0, 0.5);
        assert!(matches!(j(Step::RestPalms, &palm), Judged::Counts { palm: true, tally: 0, .. }));
        assert!(matches!(j(Step::RestPalms, &tip), Judged::Misses(_)));
        assert_eq!(j(Step::RestPalms, &touch(1800, 100, 0.0, 0.5)), Judged::Misses(TOO_QUICK));
        // An edge palm counts by where it is, whatever its size (half off
        // the pad, it looks small).
        assert!(matches!(j(Step::EdgePalm, &touch(700, 900, 0.0, 0.05)), Judged::Counts { palm: true, tally: 0, .. }));
        assert!(matches!(j(Step::EdgePalm, &touch(700, 900, 0.0, 0.95)), Judged::Counts { palm: true, tally: 1, .. }));
        assert!(matches!(j(Step::EdgePalm, &touch(1800, 900, 0.0, 0.5)), Judged::Misses(_)));
        assert!(matches!(j(Step::EdgePalm, &touch(700, 900, 0.2, 0.05)), Judged::Misses(_)));
        let typing = Touch { typing: true, ..palm.clone() };
        assert!(matches!(j(Step::TypeWithPalms, &typing), Judged::Counts { palm: true, .. }));
        assert!(matches!(j(Step::TypeWithPalms, &palm), Judged::Misses(_)));
        let moving = touch(600, 700, 0.2, 0.5);
        assert!(matches!(j(Step::PointAndScroll, &moving), Judged::Counts { palm: false, .. }));
        assert!(matches!(j(Step::PointAndScroll, &tip), Judged::Misses(_)));
        assert_eq!(j(Step::PointAndScroll, &touch(1800, 700, 0.2, 0.5)), Judged::Misses(PALMS_OFF));
        let tap = touch(600, 120, 0.0, 0.5);
        assert!(matches!(j(Step::Taps, &tap), Judged::Counts { palm: false, .. }));
        assert!(matches!(j(Step::Taps, &moving), Judged::Misses(_)));
        // The mixed step: the palm resting, then fingers with it down.
        assert!(matches!(j(Step::PointWithPalm, &touch(800, 3000, 0.0, 0.04)), Judged::Counts { palm: true, tally: 0, .. }));
        assert!(matches!(j(Step::PointWithPalm, &Touch { with_palm: true, ..moving.clone() }), Judged::Counts { palm: false, tally: 1, .. }));
        assert!(matches!(j(Step::PointWithPalm, &moving), Judged::Misses(_)));
    }
}
