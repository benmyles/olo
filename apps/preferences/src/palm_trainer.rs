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
    widgets::{Banner, KeyHint, StatusBar, StringView, Tone, banner_back, banner_height, verdict_mark},
};

use crate::{Msg, Prefs};

/// The steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Intro,
    RestPalms,
    EdgeLeft,
    EdgeRight,
    TypeWithPalms,
    Point,
    Scroll,
    Taps,
    Clicks,
    PointWithPalm,
    LowPalm,
    ThumbClicks,
    Shirt,
    Training,
    Summary,
    Test,
}

/// The exercises, each asking one thing.
const EXERCISES: [Step; 12] =
    [Step::RestPalms, Step::EdgeLeft, Step::EdgeRight, Step::TypeWithPalms, Step::Point, Step::Scroll, Step::Taps, Step::Clicks, Step::PointWithPalm, Step::LowPalm, Step::ThumbClicks, Step::Shirt];

/// Where a step wants a hand to rest, for its "now" line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Anywhere,
    LeftEdge,
    RightEdge,
    SideEdge,
    Low,
}

impl Zone {
    fn holds(self, x: f32, y: f32) -> bool {
        match self {
            Zone::Anywhere => true,
            Zone::LeftEdge => x < EDGE,
            Zone::RightEdge => x > 1.0 - EDGE,
            Zone::SideEdge => !(EDGE..=1.0 - EDGE).contains(&x),
            Zone::Low => y > 1.0 - LOW,
        }
    }
}

impl Step {
    /// Its title, and the one thing it asks.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Step::Intro => (
                "Train for Your Palm",
                "Olo tells palms from fingers with a palm model made from real hands. Training fits it to yours: twelve short exercises on the touchpad, about four minutes, then a try. Each asks one thing, and the line under it says what to do now. Rest your hands as you really do; the keyboard moves you along.",
            ),
            Step::RestPalms => ("Rest your palms", "Rest the heels of both hands on the touchpad, as they fall when you type, then lift them."),
            Step::EdgeLeft => ("Your palm over the left edge", "Rest the heel of one hand half over the touchpad's left edge, then lift it."),
            Step::EdgeRight => ("Your palm over the right edge", "Rest the heel of one hand half over the touchpad's right edge, then lift it."),
            Step::TypeWithPalms => ("Type with your palms down", "Type the line below with your palms resting on the touchpad, as you do on a laptop."),
            Step::Point => ("Move the pointer", "Palms off the touchpad: move the pointer around with one finger."),
            Step::Scroll => ("Scroll", "Palms off the touchpad: scroll up and down with two fingers."),
            Step::Taps => ("Tap", "Palms off the touchpad: tap quickly with one finger, here and there."),
            Step::Clicks => ("Click", "Palms off the touchpad: press with one finger until it clicks, here and there."),
            Step::PointWithPalm => ("Point with a palm at the edge", "One hand rests at the touchpad's edge while a finger of the other points."),
            Step::LowPalm => ("Point with your hand resting low", "Your hand rests low on the touchpad, as it falls when you point, while a finger points above it."),
            Step::ThumbClicks => ("Click with your thumb", "Hands up: press with your thumb at the bottom of the touchpad until it clicks."),
            Step::Shirt => ("Your shirt against the touchpad", "Hands off: lean in until your shirt rests against the touchpad's bottom edge, then lean back."),
            Step::Training => ("Learning your hands", "Olo's palm model is being refined on the touches that counted."),
            Step::Summary => ("", ""),
            Step::Test => ("Try it", "Use the touchpad as you usually do, palms and all: each touch shows as your new model sees it."),
        }
    }

    /// What it counts, and how many it wants.
    fn goal(self) -> Option<(&'static str, usize)> {
        Some(match self {
            Step::RestPalms => ("Palms", 10),
            Step::EdgeLeft | Step::EdgeRight => ("Rests", 5),
            Step::TypeWithPalms => ("Palms while typing", 8),
            Step::Point => ("Fingers moving", 10),
            Step::Scroll => ("Fingers scrolling", 10),
            Step::Taps => ("Taps", 10),
            Step::Clicks => ("Clicks", 6),
            Step::PointWithPalm => ("Rests", 3),
            Step::LowPalm => ("Rests", 5),
            Step::ThumbClicks => ("Thumb clicks", 5),
            Step::Shirt => ("Shirt touches", 5),
            _ => return None,
        })
    }

    /// Where it wants a hand resting, if it does (and then the "now" line
    /// goes rest, hold, lift).
    fn rest(self) -> Option<Zone> {
        match self {
            Step::RestPalms | Step::TypeWithPalms => Some(Zone::Anywhere),
            Step::EdgeLeft => Some(Zone::LeftEdge),
            Step::EdgeRight => Some(Zone::RightEdge),
            Step::PointWithPalm => Some(Zone::SideEdge),
            Step::LowPalm => Some(Zone::Low),
            _ => None,
        }
    }

    /// A finger pointing while the hand rests is part of it: rest, point,
    /// lift.
    fn points_while_resting(self) -> bool {
        matches!(self, Step::PointWithPalm | Step::LowPalm)
    }

    /// Done without reaching its count (Space skips it): a shirt may
    /// never reach the glass.
    fn optional(self) -> bool {
        self == Step::Shirt
    }

    fn exercise(self) -> Option<usize> {
        EXERCISES.iter().position(|e| *e == self)
    }
}

/// A contact from landing to lifting, as the step judges it.
#[derive(Clone, Debug, Default)]
pub struct Touch {
    /// Its features in its first `DECIDE_MS` on the glass (from
    /// `MIN_AGE_MS`).
    pub frames: Vec<[f32; FEATURES]>,
    /// Its largest size and pressure (the pad's units).
    pub size: i32,
    pub pressed: i32,
    pub held_ms: u32,
    /// How far it went from where it landed, in pad widths.
    pub travel: f32,
    /// Where it landed, across and down the pad (0 to 1).
    pub x: f32,
    pub y: f32,
    /// A key was pressed while it was down.
    pub typing: bool,
    /// When it landed, another contact had rested still for a second; and
    /// one had below it.
    pub with_palm: bool,
    pub above_palm: bool,
    /// Another contact was down with it.
    pub with_others: bool,
    /// How many touches landed and lifted while it rested.
    pub fingers_while: u32,
}

/// Still: moved less than this (pad widths).
const STILL: f32 = 0.03;
/// Over an edge: landed within this of the pad's left or right side.
const EDGE: f32 = 0.15;
/// Low on the pad: landed in its lowest this much (of its height).
const LOW: f32 = 0.35;
/// At the bottom edge (a shirt against it).
const BOTTOM: f32 = 0.15;
/// Resting: down at least this long (ms).
const REST_MS: u32 = 400;
/// A tap: up within this (ms).
const TAP_MS: u32 = 350;
/// A hand resting through others' touches (the steps that point while
/// it rests): down and still this long (ms), with this many touches
/// while it was.
const PALM_REST_MS: u32 = 1000;
const FINGERS_PER_REST: u32 = 3;

impl Touch {
    fn still(&self) -> bool {
        self.travel < STILL
    }

    /// As big as a palm, or one by Olo's own model.
    fn palm_like(&self, palm_size: u32, base: &PalmModel) -> bool {
        self.palm_sized(palm_size) || self.frames.iter().any(|f| olo_palm::score(base, f) > base.threshold)
    }

    /// As big as a palm (for what presses: a pressing finger flattens).
    fn palm_sized(&self, palm_size: u32) -> bool {
        self.size as f32 >= palm_size as f32 * 0.7
    }
}

/// What a step makes of a touch.
#[derive(Clone, Debug, PartialEq)]
pub enum Judged {
    /// It's learned from, as a palm or a finger, and counts toward the
    /// step's goal or not (a finger pointing while a hand rests: the
    /// rests are counted).
    Counts { palm: bool, counted: bool, say: &'static str },
    /// It isn't what the step asked for: why (it's dropped).
    Misses(&'static str),
}

const TOO_QUICK: &str = "Too quick: let it rest a moment before you lift it.";
const PALMS_OFF: &str = "That looked like a palm: keep your palms off the touchpad here.";
const PRESS: &str = "That didn't press: press until the touchpad clicks.";

/// How `step` takes touch `t` (`palm_size` and `press` as the touch
/// filter has them; `base`: Olo's own model, for a second opinion on
/// what's a palm).
pub fn judge(step: Step, t: &Touch, palm_size: u32, press: u32, base: &PalmModel) -> Judged {
    let palm_like = t.palm_like(palm_size, base);
    let pressed = t.pressed >= press as i32;
    let counts = |palm, say| Judged::Counts { palm, counted: true, say };
    let learns = |palm, say| Judged::Counts { palm, counted: false, say };
    let resting = t.held_ms >= PALM_REST_MS && t.travel < STILL * 2.0;
    match step {
        Step::RestPalms if !palm_like => Judged::Misses("That was a fingertip: rest the heel of your hand, below the thumb."),
        Step::RestPalms if t.held_ms < REST_MS => Judged::Misses(TOO_QUICK),
        Step::RestPalms => counts(true, "A palm, resting. Good."),
        Step::EdgeLeft | Step::EdgeRight if t.travel >= STILL * 2.0 => Judged::Misses("It moved like a finger: keep the heel of your hand still."),
        Step::EdgeLeft | Step::EdgeRight if t.held_ms < REST_MS => Judged::Misses(TOO_QUICK),
        Step::EdgeLeft if t.x < EDGE => counts(true, "Over the left edge. Good."),
        Step::EdgeRight if t.x > 1.0 - EDGE => counts(true, "Over the right edge. Good."),
        Step::EdgeLeft => Judged::Misses("That wasn't at the left edge: rest the heel half over it."),
        Step::EdgeRight => Judged::Misses("That wasn't at the right edge: rest the heel half over it."),
        Step::TypeWithPalms if !t.typing => Judged::Misses("No typing while it rested: keep typing with your palms down."),
        Step::TypeWithPalms if t.held_ms < REST_MS => Judged::Misses("Too quick: rest your palms while you type."),
        Step::TypeWithPalms => counts(true, "Resting while you typed. Good."),
        Step::Point | Step::Scroll | Step::Taps | Step::Clicks if t.palm_sized(palm_size) || (step != Step::Clicks && palm_like) => Judged::Misses(PALMS_OFF),
        Step::Point if t.still() => Judged::Misses("That was a tap: move the pointer around."),
        Step::Point => counts(false, "A finger moving. Good."),
        Step::Scroll if t.still() || !t.with_others => Judged::Misses("Scroll with two fingers together, up and down."),
        Step::Scroll => counts(false, "Scrolling. Good."),
        Step::Taps if t.held_ms > TAP_MS || !t.still() => Judged::Misses("That was a slide: tap quickly, without moving."),
        Step::Taps => counts(false, "A tap. Good."),
        Step::Clicks if !pressed => Judged::Misses(PRESS),
        Step::Clicks if t.travel >= STILL * 2.0 => Judged::Misses("It moved: keep the finger still as you press."),
        Step::Clicks => counts(false, "A click. Good."),
        // Rest, point, lift: the rest counts once it has had its touches.
        Step::PointWithPalm | Step::LowPalm if resting => {
            let zone = step.rest().unwrap_or(Zone::Anywhere);
            if !zone.holds(t.x, t.y) && !(step == Step::PointWithPalm && palm_like) {
                Judged::Misses(if step == Step::LowPalm { "That rested too high: rest your hand low on the touchpad." } else { "Rest the heel of your hand at or over the edge." })
            } else if !palm_like && step == Step::PointWithPalm && t.size < palm_size as i32 / 3 {
                Judged::Misses("That was a fingertip: rest the heel of your hand at the edge.")
            } else if t.fingers_while < FINGERS_PER_REST {
                Judged::Misses("Lifted too soon: rest it, point a few times, then lift it.")
            } else {
                counts(true, if step == Step::LowPalm { "Your hand, resting low. Good." } else { "Your palm, resting. Good." })
            }
        }
        Step::PointWithPalm | Step::LowPalm if palm_like => Judged::Misses("That looked like a palm: point with one finger."),
        Step::LowPalm if t.above_palm => learns(false, "A finger, above your hand. Good."),
        Step::PointWithPalm if t.with_palm => learns(false, "A finger, with your palm down. Good."),
        Step::PointWithPalm | Step::LowPalm => Judged::Misses("Rest your hand first, then point with a finger."),
        Step::ThumbClicks if t.palm_sized(palm_size) => Judged::Misses(PALMS_OFF),
        Step::ThumbClicks if t.y < 1.0 - LOW => Judged::Misses("Click at the bottom of the touchpad, with your thumb."),
        Step::ThumbClicks if !pressed => Judged::Misses(PRESS),
        Step::ThumbClicks => counts(false, "A thumb click. Good."),
        Step::Shirt if t.y < 1.0 - BOTTOM => Judged::Misses("That was up on the touchpad: only your shirt, at the bottom edge."),
        Step::Shirt if pressed && !t.palm_sized(palm_size) => Judged::Misses("That pressed like a finger: hands off, only your shirt."),
        Step::Shirt => counts(true, "Your shirt, at the edge. Good."),
        _ => Judged::Misses(""),
    }
}

/// A contact on the glass now, as the step's "now" line needs it.
#[derive(Clone, Copy, Debug, Default)]
struct Down {
    /// Where it landed (0 to 1).
    x: f32,
    y: f32,
    held_ms: u32,
    still: bool,
    /// Touches that landed and lifted while it rested.
    fingers_while: u32,
}

/// What the worker shares with the window.
#[derive(Default)]
struct Live {
    axes: Option<PadAxes>,
    contacts: Vec<TouchContact>,
    /// The contacts on the glass now.
    down: Vec<Down>,
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
    pressed: i32,
    down_us: u64,
    start: (i32, i32),
    travel: f32,
    typing: bool,
    with_palm: bool,
    above_palm: bool,
    with_others: bool,
    fingers_while: u32,
    call: Option<bool>,
}

impl Open {
    /// Resting: down a second and still (a hand that others point beside).
    fn resting(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.down_us) >= PALM_REST_MS as u64 * 1000 && self.travel < STILL * 2.0
    }
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
                let height = (pad.y_max - pad.y_min).max(1) as f32;
                let model = model.lock().unwrap().clone();
                let others = frames.len() > 1;
                for (id, f, age) in &frames {
                    let Some(c) = contacts.iter().find(|c| c.id == *id) else { continue };
                    // (From its first report on the glass: hovering, a
                    // contact has no features yet.)
                    if !open.contains_key(id) {
                        let resting: Vec<i32> = open.values().filter(|o| o.resting(time_us)).map(|o| o.start.1).collect();
                        open.insert(
                            *id,
                            Open {
                                frames: Vec::new(),
                                size: 0,
                                pressed: 0,
                                down_us: time_us,
                                start: (c.x, c.y),
                                travel: 0.0,
                                typing: false,
                                with_palm: !resting.is_empty(),
                                // (Below: further down the pad.)
                                above_palm: resting.iter().any(|y| *y > c.y),
                                with_others: false,
                                fingers_while: 0,
                                call: None,
                            },
                        );
                    }
                    let o = open.get_mut(id).unwrap();
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
                    o.pressed = o.pressed.max(c.pressure);
                    o.with_others |= others;
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
                    // (A touch while a hand rests: one more for it.)
                    for r in open.values_mut().filter(|r| r.down_us < o.down_us && r.resting(time_us)) {
                        r.fingers_while += 1;
                    }
                    if o.frames.is_empty() {
                        continue;
                    }
                    l.lifted.push(Touch {
                        frames: o.frames,
                        size: o.size,
                        pressed: o.pressed,
                        held_ms: (time_us.saturating_sub(o.down_us) / 1000) as u32,
                        travel: o.travel,
                        x: (o.start.0 - pad.x_min) as f32 / width,
                        y: (o.start.1 - pad.y_min) as f32 / height,
                        typing: o.typing,
                        with_palm: o.with_palm,
                        above_palm: o.above_palm,
                        with_others: o.with_others,
                        fingers_while: o.fingers_while,
                    });
                }
                l.down = open
                    .values()
                    .map(|o| Down {
                        x: (o.start.0 - pad.x_min) as f32 / width,
                        y: (o.start.1 - pad.y_min) as f32 / height,
                        held_ms: (time_us.saturating_sub(o.down_us) / 1000) as u32,
                        still: o.travel < STILL * 2.0,
                        fingers_while: o.fingers_while,
                    })
                    .collect();
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
    /// The touch filter's palm size and click (the pad's units).
    palm_size: u32,
    press: u32,
    step: Step,
    started: Instant,
    /// The touches that counted, from the steps finished, and this one's.
    samples: Vec<Labelled>,
    current: Vec<Labelled>,
    /// This step's count toward its goal.
    got: usize,
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
/// The intro's numbers' column ("12.").
const NUMBER: f32 = 22.0;
/// Laid out at this until full screen (and in the layout lint).
const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 800.0;

/// Open it, full screen.
pub fn open(prefs: Entity<Prefs>, palm_size: u32, press: u32, cx: &mut App) {
    let options = olo_ui::WindowOptions::new("Train for Your Palm", WIDTH, HEIGHT).fullscreen();
    let _ = olo_ui::open_window(cx, options, move |_, cx| {
        let proxy = olo_ui::proxy(cx);
        cx.new(|cx| {
            let w = TrainerWindow::new(prefs, palm_size, press, cx);
            watch(w.live.clone(), w.model.clone(), w.stop.clone(), proxy);
            w
        })
    });
}

impl TrainerWindow {
    fn new(prefs: Entity<Prefs>, palm_size: u32, press: u32, cx: &mut Context<Self>) -> TrainerWindow {
        // The worker's news comes as the settings' does (`Msg::Redraw`).
        cx.observe(&prefs, |_, _, cx| cx.notify()).detach();
        TrainerWindow {
            prefs,
            palm_size,
            press,
            step: Step::Intro,
            started: Instant::now(),
            samples: Vec::new(),
            current: Vec::new(),
            got: 0,
            said: None,
            marks: Vec::new(),
            quitting: false,
            tutor: Default::default(),
            live: Default::default(),
            // (Until one is kept, Olo's own: the try is of the one kept.)
            model: Arc::new(Mutex::new(olo_palm::default_model())),
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
        self.got = 0;
        self.said = None;
        self.marks.clear();
        self.quitting = false;
        self.tutor = Default::default();
        self.tried = (0, 0);
        // (What lifted before the step began isn't its.)
        self.live.lock().unwrap().lifted.clear();
    }

    /// The step's goal met.
    fn done(&self) -> bool {
        self.step.goal().is_some_and(|(_, want)| self.got >= want)
    }

    /// What to do now, as the step and the hands on the pad stand: rest,
    /// hold, (point,) lift, for the steps that rest a hand; the step's
    /// own words for the rest.
    fn now(&self) -> String {
        let step = self.step;
        if self.done() {
            return "That's all this step needs.".into();
        }
        let lead = step.words().1.to_owned();
        let Some(zone) = step.rest() else { return lead };
        let l = self.live.lock().unwrap();
        let place = match zone {
            Zone::Anywhere if step == Step::TypeWithPalms => "Rest your palms on the touchpad and type the line below.",
            Zone::Anywhere => "Rest the heels of both hands on the touchpad.",
            Zone::LeftEdge => "Rest the heel of one hand half over the left edge.",
            Zone::RightEdge => "Rest the heel of one hand half over the right edge.",
            Zone::SideEdge => "Rest the heel of one hand at the left or right edge, and leave it there.",
            Zone::Low => "Rest your hand low on the touchpad, as it falls when you point, and leave it there.",
        };
        let hand = l.down.iter().filter(|d| zone.holds(d.x, d.y) && d.still).max_by_key(|d| d.held_ms);
        let Some(hand) = hand else { return place.into() };
        if step.points_while_resting() {
            if hand.held_ms < PALM_REST_MS {
                return "Hold it there…".into();
            }
            return match FINGERS_PER_REST.saturating_sub(hand.fingers_while) {
                0 => "Good. Now lift your hand.".into(),
                n if step == Step::LowPalm => format!("Now point or tap with a finger above it: {n} more."),
                n => format!("Now point or tap with a finger of your other hand: {n} more."),
            };
        }
        if hand.held_ms < REST_MS {
            if step == Step::TypeWithPalms { "Keep typing…".into() } else { "Hold it there…".into() }
        } else if step == Step::TypeWithPalms {
            "Now lift your hands, and rest them again as you type.".into()
        } else {
            "Good. Now lift it.".into()
        }
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
        let base = olo_palm::default_model();
        for t in lifted {
            let place = (t.x, t.y);
            match judge(self.step, &t, self.palm_size, self.press, &base) {
                Judged::Counts { palm, counted, say } => {
                    if counted {
                        self.got += 1;
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
                if !self.done() && !s.optional() {
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
        // (state_dir is Olo's own already: it was olo/olo/palm.)
        std::thread::spawn(move || match olo_palm::keep_samples(&olo_xdg::state_dir().join("palm"), &kept) {
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
    // (What's over the pad's edges stays in the film.)
    window.with_content_mask(Some(olo_ui::gpui::ContentMask { bounds: deck }), |window| film(window, step, t, keys, kw, kh, pad, p));
}

/// The film's hands on `pad` (`keys`, each `kw` × `kh`, above it).
#[allow(clippy::too_many_arguments)]
fn film(window: &mut Window, step: Step, t: f32, keys: Bounds<Pixels>, kw: f32, kh: f32, pad: Bounds<Pixels>, p: &olo_theme::Palette) {
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
        Step::Point => {
            let (x, y) = (0.5 + 0.3 * (t * 1.5).cos(), 0.5 + 0.25 * (t * 1.1).sin());
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
        }
        Step::Scroll => {
            let y = 0.5 + 0.3 * (t * 2.0).sin();
            ellipse(window, point(pad.left() + px(pw * 0.45), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
            ellipse(window, point(pad.left() + px(pw * 0.55), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
        }
        Step::Taps => {
            let k = (t * 1.5) as usize % 4;
            let (x, y) = [(0.3, 0.4), (0.7, 0.6), (0.5, 0.3), (0.4, 0.7)][k];
            let down = ((t * 1.5) % 1.0) < 0.3;
            let d = if down { dot * 1.15 } else { dot * 0.6 };
            ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), d, d * 1.1, if down { finger } else { hand });
        }
        Step::Clicks | Step::ThumbClicks => {
            // Down, pressed until it clicks (a ring), up; a finger here
            // and there, a thumb at the bottom.
            let k = (t / 1.6) as usize % 3;
            let (x, y) = if step == Step::Clicks { [(0.35, 0.4), (0.65, 0.55), (0.5, 0.3)][k] } else { [(0.4, 0.86), (0.5, 0.88), (0.6, 0.86)][k] };
            let phase = t % 1.6;
            let c = point(pad.left() + px(pw * x), pad.top() + px(ph * y));
            let d = if step == Step::Clicks { dot } else { dot * 1.4 };
            if phase < 1.0 {
                let press = (phase / 0.5).min(1.0);
                ellipse(window, c, d * (1.0 + 0.2 * press), d * (1.15 + 0.2 * press), finger);
                if phase >= 0.5 {
                    let r = d * (1.6 + (phase - 0.5) * 3.0);
                    ring(window, c, r, r, finger.opacity(1.0 - (phase - 0.5) * 2.0));
                }
            } else {
                ellipse(window, c, d * 0.6, d * 0.7, hand);
            }
        }
        Step::EdgeLeft | Step::EdgeRight => {
            // One heel, half over the edge, resting and lifting.
            let down = (t % 3.5) < 2.4;
            let x = if step == Step::EdgeLeft { pad.left() - px(pw * 0.03) } else { pad.right() + px(pw * 0.03) };
            let y = pad.top() + px(ph * 0.4) - px(if down { 0.0 } else { ph * 0.08 });
            ellipse(window, point(x, y), pw * 0.17, ph * 0.22, if down { palm.opacity(0.85) } else { hand });
        }
        Step::PointWithPalm | Step::LowPalm => {
            // Rest, point a few times, lift.
            let phase = t % 6.0;
            let down = phase < 4.8;
            let low = step == Step::LowPalm;
            let (hx, hy) = if low { (pad.left() + px(pw * 0.62), pad.bottom() + px(ph * 0.02)) } else { (pad.right() + px(pw * 0.03), pad.top() + px(ph * 0.42)) };
            let lift = if down { 0.0 } else { ph * 0.08 };
            let (rx, ry) = if low { (pw * 0.2, ph * 0.16) } else { (pw * 0.17, ph * 0.22) };
            ellipse(window, point(hx, hy - px(lift)), rx, ry, if down { palm.opacity(0.85) } else { hand });
            if (1.0..4.4).contains(&phase) {
                let (x, y) = if low { (0.5 + 0.12 * (t * 1.5).cos(), 0.35 + 0.15 * (t * 1.1).sin()) } else { (0.35 + 0.2 * (t * 1.5).cos(), 0.45 + 0.25 * (t * 1.1).sin()) };
                ellipse(window, point(pad.left() + px(pw * x), pad.top() + px(ph * y)), dot, dot * 1.15, finger);
            }
        }
        Step::Shirt => {
            // Cloth coming up against the bottom edge, and back.
            let phase = t % 4.0;
            let near = if phase < 1.0 {
                phase
            } else if phase < 2.8 {
                1.0
            } else {
                (4.0 - phase) / 1.2
            };
            let y = pad.bottom() + px(ph * (0.28 - 0.24 * near));
            let ink = if near > 0.9 { palm.opacity(0.7) } else { hand };
            ellipse(window, point(pad.center().x, y), pw * 0.42, ph * 0.18, ink);
        }
        _ => {}
    }
}

/// A caption over a panel ("Like this", "Your touchpad").
fn caption(text: &'static str) -> impl IntoElement {
    StringView::new(text).bold(true).height(m::LABEL_ROW)
}

impl TrainerWindow {
    /// The top: where in the run this is, as a bar of the exercises and
    /// the try, and its title and what to do (now).
    fn top(&self, title: String, lead: String) -> AnyElement {
        let at = match self.step {
            Step::Intro => 0,
            s => s.exercise().map_or(EXERCISES.len() + 1, |i| i + 1),
        };
        let where_ = match self.step.exercise() {
            Some(i) => format!("Step {} of {}", i + 1, EXERCISES.len()),
            None if self.step == Step::Intro => "Twelve exercises, then a try".to_owned(),
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

    /// Under the panels: what the last touch did (or the step's news)
    /// and the step's count; once it's done, a banner saying so, and what
    /// goes on.
    fn feedback(&self) -> AnyElement {
        let slot = div().w_full().h(px(banner_height()));
        if self.done() && !self.quitting {
            let key = if self.step == Step::TypeWithPalms { "Return" } else { "Space" };
            let next = EXERCISES.get(self.step.exercise().unwrap_or(0) + 1);
            let detail = match next {
                Some(n) => format!("Press {key} for the next: {}.", n.words().0),
                None => format!("That's all of them. Press {key} to learn your hands."),
            };
            let banner = Banner::new(Tone::Success, "This step is done", detail).trailing(KeyHint::new(key, if next.is_some() { "Next step" } else { "Train" }));
            return slot.child(banner).into_any_element();
        }
        // (In the banner's box, quiet, so the banner takes its place.)
        let back = canvas(|_, _, _| {}, |b, _, window, cx| banner_back(window, b, None, &theme::palette(cx))).absolute().inset_0();
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).px(px(m::SPACING * 2.0)).size_full();
        let (good, text): (Option<bool>, String) = if self.quitting {
            (None, "Press Esc again to quit: the touches so far are dropped. Space goes on.".into())
        } else if let Some(s) = &self.said {
            (Some(s.good), s.text.into())
        } else if self.live.lock().unwrap().axes.is_none() {
            (None, "Waiting for the touchpad…".into())
        } else if self.step.optional() {
            (None, "If nothing shows on the touchpad, your shirt doesn't reach it: press Space to skip this.".into())
        } else {
            (None, String::new())
        };
        if let Some(good) = good {
            let mark = canvas(|_, _, _| {}, move |b, _, window, cx| verdict_mark(window, b, if good { Tone::Success } else { Tone::Failure }, 18.0, &theme::palette(cx)))
                .w(px(m::MARK_ROW))
                .h(px(m::MARK_ROW))
                .flex_shrink_0();
            row = row.child(mark);
        }
        row = row.child(div().flex_1().min_w_0().child(StringView::new(text).size(style::LEAD_SIZE).wrap(true)));
        if let Some((name, want)) = self.step.goal() {
            row = row.child(tally(name, self.got, want));
        }
        slot.relative().child(back).child(row).into_any_element()
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
                let action = match (self.done(), last) {
                    (false, _) if s.optional() => "Skip this step",
                    (_, true) => "Train",
                    _ => "Next step",
                };
                row = row.child(hint(next, action, self.done() || s.optional()));
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
        // (As a finished step's: the outcome first, in a banner.)
        let short = match (&self.outcome, saved) {
            (Some(Outcome::Kept(_)), Some(true)) => "It's in use now, in place of Olo's own.",
            (Some(Outcome::Kept(_)), Some(false)) => "Olo's own model is still in use.",
            (Some(Outcome::Kept(_)), None) => "Saving it with your settings…",
            _ => "Nothing changed: Olo's own model is still in use.",
        };
        let mut col = div().flex().flex_col().gap(px(m::SPACING * 2.0)).w_full();
        col = col.child(Banner::new(if good { Tone::Success } else { Tone::Failure }, title, short));
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

/// Each screen in each of its states, for the layout lint
/// (`ui_lint_the_palm_trainer`): a name, the step, and which state.
#[cfg(test)]
pub(crate) fn lint_states() -> Vec<(String, Step, u8)> {
    let mut out = vec![("the introduction".to_owned(), Step::Intro, 0)];
    for s in EXERCISES {
        for (v, how) in ["fresh", "halfway", "a miss", "done", "quitting", "a hand resting"].into_iter().enumerate() {
            if v == 5 && s.rest().is_none() {
                continue;
            }
            out.push((format!("{}, {how}", s.words().0), s, v as u8));
        }
    }
    out.push(("training".into(), Step::Training, 0));
    for (v, how) in ["kept, saving", "kept and saved", "kept, not saved", "worse", "too few"].into_iter().enumerate() {
        out.push((format!("the summary, {how}"), Step::Summary, v as u8));
    }
    out.push(("the try".into(), Step::Test, 0));
    out
}

#[cfg(test)]
impl TrainerWindow {
    /// The window at `step` in state `v` of `lint_states`, with a
    /// MacBook's pad, the longest things each shows.
    pub(crate) fn for_lint(prefs: Entity<Prefs>, step: Step, v: u8, cx: &mut Context<Self>) -> TrainerWindow {
        let mut w = TrainerWindow::new(prefs, 1600, 120, cx);
        w.live.lock().unwrap().axes =
            Some(PadAxes { name: "Apple MTP multi-touch".into(), x_min: -7700, x_max: 7700, y_min: -200, y_max: 9200, x_res: 98, y_res: 98, size_max: 4000, ..Default::default() });
        w.step = step;
        let want = step.goal().map_or(0, |(_, n)| n);
        w.got = match v {
            1 => want / 2,
            3 => want,
            _ => 0,
        };
        if v == 5 {
            // (A hand resting a while, in the step's place, a touch in.)
            let (x, y) = match step.rest() {
                Some(Zone::LeftEdge | Zone::SideEdge) => (0.05, 0.4),
                Some(Zone::RightEdge) => (0.95, 0.4),
                Some(Zone::Low) => (0.6, 0.9),
                _ => (0.3, 0.3),
            };
            w.live.lock().unwrap().down = vec![Down { x, y, held_ms: 1500, still: true, fingers_while: 1 }];
        }
        let say = |good, text| Some(Said { good, text, at: Instant::now(), place: (0.5, 0.5) });
        w.said = match v {
            1 => say(true, "A finger, with your palm down. Good."),
            // (The longest of the misses.)
            2 => say(false, "That was a fingertip: rest the heel of your hand, below the thumb."),
            _ => None,
        };
        w.quitting = v == 4;
        if step == Step::TypeWithPalms && v > 0 {
            w.tutor.typed = 12;
        }
        let report = Report {
            contacts: 96,
            palms: 41,
            fingers: 55,
            palms_caught: 0.97,
            fingers_lost: 0.018,
            size_rule_palms_caught: 0.71,
            size_rule_fingers_lost: 0.0,
            baseline_palms_caught: 0.88,
            baseline_fingers_lost: 0.036,
        };
        if step == Step::Summary {
            w.samples = (0..480).map(|i| Labelled { palm: i % 3 == 0, ..Default::default() }).collect();
            w.outcome = Some(match v {
                0..=2 => Outcome::Kept(report),
                3 => Outcome::Worse(report),
                _ => Outcome::TooFew,
            });
            *w.saved.lock().unwrap() = match v {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            };
        }
        if step == Step::Training {
            w.samples = (0..480).map(|_| Labelled::default()).collect();
            *w.progress.lock().unwrap() = 0.4;
        }
        w.tried = (1_284, 316);
        w
    }
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
                col = col.child(self.top(title.into(), lead.into()));
                col = col.child(self.panels(Some(Step::Intro), false, two));
                // The exercises, numbered down two columns, each under a
                // panel, the numbers' ends lined up.
                let rows = EXERCISES.len().div_ceil(2);
                let mut list = div().flex().flex_row().gap(px(PANEL_GAP)).w_full();
                for c in 0..2 {
                    let mut column = div().flex().flex_col().w(px(two.0));
                    for (i, e) in EXERCISES.iter().enumerate().skip(c * rows).take(rows) {
                        let number = div().w(px(NUMBER)).flex().flex_row().justify_end().child(StringView::new(format!("{}.", i + 1)).dim(true));
                        column = column.child(div().flex().flex_row().gap(px(m::TIGHT)).child(number).child(StringView::new(e.words().0).dim(true)));
                    }
                    list = list.child(column);
                }
                col = col.child(list);
            }
            s if s.exercise().is_some() => {
                col = col.child(self.top(title.into(), self.now()));
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
                col = col.child(self.top(title.into(), lead.into()));
                let fraction = *self.progress.lock().unwrap();
                col = col.child(StatusBar::new(fraction).label(format!("Refining Olo's model on {} touches…", self.samples.len())));
            }
            Step::Summary => {
                col = col.child(self.top(String::new(), String::new()));
                col = col.child(self.summary());
            }
            Step::Test => {
                col = col.child(self.top(title.into(), lead.into()));
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
        Touch { size, held_ms, travel, x, y: 0.4, pressed: 40, ..Default::default() }
    }

    /// Each step counts what it asks for, and says why it doesn't count
    /// the rest.
    #[test]
    fn steps_judge_touches() {
        // (A model that calls nothing a palm: only sizes and places count.)
        let none = PalmModel { threshold: f32::MAX, ..olo_palm::baseline() };
        let j = |step, t: &Touch| judge(step, t, 1600, 120, &none);
        let counts = |j: Judged, palm: bool, counted: bool| matches!(j, Judged::Counts { palm: p, counted: c, .. } if p == palm && c == counted);
        let palm = touch(1800, 900, 0.0, 0.5);
        let tip = touch(600, 900, 0.0, 0.5);
        assert!(counts(j(Step::RestPalms, &palm), true, true));
        assert!(matches!(j(Step::RestPalms, &tip), Judged::Misses(_)));
        assert_eq!(j(Step::RestPalms, &touch(1800, 100, 0.0, 0.5)), Judged::Misses(TOO_QUICK));
        // An edge palm counts by where it is, whatever its size (half off
        // the pad, it looks small); each edge its own step.
        assert!(counts(j(Step::EdgeLeft, &touch(700, 900, 0.0, 0.05)), true, true));
        assert!(matches!(j(Step::EdgeLeft, &touch(700, 900, 0.0, 0.95)), Judged::Misses(_)));
        assert!(counts(j(Step::EdgeRight, &touch(700, 900, 0.0, 0.95)), true, true));
        assert!(matches!(j(Step::EdgeRight, &touch(1800, 900, 0.0, 0.5)), Judged::Misses(_)));
        assert!(matches!(j(Step::EdgeLeft, &touch(700, 900, 0.2, 0.05)), Judged::Misses(_)));
        let typing = Touch { typing: true, ..palm.clone() };
        assert!(counts(j(Step::TypeWithPalms, &typing), true, true));
        assert!(matches!(j(Step::TypeWithPalms, &palm), Judged::Misses(_)));
        let moving = touch(600, 700, 0.2, 0.5);
        assert!(counts(j(Step::Point, &moving), false, true));
        assert!(matches!(j(Step::Point, &tip), Judged::Misses(_)));
        assert_eq!(j(Step::Point, &touch(1800, 700, 0.2, 0.5)), Judged::Misses(PALMS_OFF));
        // Scrolling: moving with another finger down.
        assert!(matches!(j(Step::Scroll, &moving), Judged::Misses(_)));
        assert!(counts(j(Step::Scroll, &Touch { with_others: true, ..moving.clone() }), false, true));
        let tap = touch(600, 120, 0.0, 0.5);
        assert!(counts(j(Step::Taps, &tap), false, true));
        assert!(matches!(j(Step::Taps, &moving), Judged::Misses(_)));
        // A click: pressed past the click, still.
        assert_eq!(j(Step::Clicks, &tap), Judged::Misses(PRESS));
        assert!(counts(j(Step::Clicks, &Touch { pressed: 300, ..tap.clone() }), false, true));
        // Rest, point, lift: fingers with the hand down are learned, the
        // rest counts once it has had its touches.
        let rest = Touch { fingers_while: 3, ..touch(800, 3000, 0.0, 0.04) };
        assert!(counts(j(Step::PointWithPalm, &rest), true, true));
        assert!(matches!(j(Step::PointWithPalm, &Touch { fingers_while: 1, ..rest.clone() }), Judged::Misses(_)));
        assert!(counts(j(Step::PointWithPalm, &Touch { with_palm: true, ..moving.clone() }), false, false));
        assert!(matches!(j(Step::PointWithPalm, &moving), Judged::Misses(_)));
        let low = Touch { y: 0.9, x: 0.6, ..rest.clone() };
        assert!(counts(j(Step::LowPalm, &low), true, true));
        assert!(matches!(j(Step::LowPalm, &Touch { y: 0.4, ..low.clone() }), Judged::Misses(_)));
        assert!(counts(j(Step::LowPalm, &Touch { above_palm: true, with_palm: true, ..moving.clone() }), false, false));
        assert!(matches!(j(Step::LowPalm, &Touch { with_palm: true, ..moving.clone() }), Judged::Misses(_)));
        // A thumb's click at the bottom; not up the pad, not unpressed.
        let thumb = Touch { y: 0.9, pressed: 250, ..touch(900, 300, 0.0, 0.5) };
        assert!(counts(j(Step::ThumbClicks, &thumb), false, true));
        assert!(matches!(j(Step::ThumbClicks, &Touch { y: 0.4, ..thumb.clone() }), Judged::Misses(_)));
        assert_eq!(j(Step::ThumbClicks, &Touch { pressed: 30, ..thumb.clone() }), Judged::Misses(PRESS));
        // The shirt: at the bottom edge, a palm (left out), not pressing
        // like a finger.
        let shirt = Touch { y: 0.95, pressed: 4, ..touch(1200, 2000, 0.01, 0.5) };
        assert!(counts(j(Step::Shirt, &shirt), true, true));
        assert!(matches!(j(Step::Shirt, &Touch { y: 0.5, ..shirt.clone() }), Judged::Misses(_)));
        assert!(matches!(j(Step::Shirt, &Touch { pressed: 300, size: 800, ..shirt.clone() }), Judged::Misses(_)));
    }

    /// Every step asks one thing: one goal, one sentence.
    #[test]
    fn each_step_asks_one_thing() {
        for s in EXERCISES {
            assert!(s.goal().is_some(), "{s:?}");
            let lead = s.words().1;
            assert_eq!(lead.matches(". ").count(), 0, "{s:?}: {lead}");
        }
    }
}
